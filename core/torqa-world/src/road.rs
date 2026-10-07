//! The road: a spatial index of its centre line and its mesh.

use std::collections::HashMap;

use torqa_routes::{LocalProjection, Route, Surface, catmull_rom};

use crate::MeshData;

/// Size of the index cells.
const CELL: f64 = 100.0;
/// The road is drawn as a smooth curve through the route's points, sampled this often.
const DRAW_STEP: f64 = 2.0;
/// The road's edges bevel this far out and down, a little below the level ground beside it
/// (`ROAD_SINK`): a low, soft edge rather than a kerb...
const BEVEL_REACH: f64 = 0.4;
const BEVEL_DROP: f64 = 0.25;
/// ...and skirts hang on from there this far down and out, so wherever the ground falls away the
/// road shows an edge rather than a gap below it.
const SKIRT_DEPTH: f64 = 1.2;
const SKIRT_REACH: f64 = 0.8;

/// Vertices across the road: skirt, bevel and edge on either side.
const RING: usize = 6;
/// Where the road turns more than this at a point (radians), it turns in a fan of rings this
/// far apart.
const FAN_STEP: f64 = 0.4;
/// A stretch this close to an earlier one of the route rides the same road again, if it comes
/// back the other way (past the turn) or this far further on (as in `torqa_routes`).
const SAME_ROAD: f64 = 3.0;
const APART: f64 = 40.0;
/// Open line this far before a tunnel portal leads into its cutting, which ends in the portal's
/// plane.
const APPROACH: f64 = 60.0;

/// Where another street meets the road: distance along the road, side (1 right of travel, −1
/// left) and the street's half width.
pub(crate) type Mouth = (f64, f64, f64);

/// A tunnel portal's plane, square to the line: a point of it on the line and the unit
/// direction into the tunnel (east, north).
pub(crate) type Plane = ((f64, f64), (f64, f64));

#[derive(Debug, Clone, Copy)]
struct Segment {
    /// Start and end in metres east/north.
    a: (f64, f64),
    b: (f64, f64),
    /// Road elevation at start and end.
    elevation_a: f64,
    elevation_b: f64,
    /// Distance along the route at the start.
    distance_a: f64,
    distance_b: f64,
    /// What carries the road on this segment.
    surface: Surface,
    /// The route rides this stretch of road a second time: it is drawn by the first pass.
    repeat: bool,
    /// The planes of the tunnel portals this open stretch leads to, ahead and behind
    /// ([`RoadIndex::set_portals`]).
    portals: [Option<Plane>; 2],
}

/// A point of a centre line.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Centre {
    /// Metres east/north.
    pub(crate) position: (f64, f64),
    pub(crate) elevation: f64,
    /// Distance along the line.
    pub(crate) distance: f64,
    pub(crate) surface: Surface,
}

/// Centre lines in local coordinates, indexed for nearest-point queries: the route's (a smooth
/// curve through its points, so bends look like a road's, not a polygon's), or the railways'.
pub(crate) struct RoadIndex {
    segments: Vec<Segment>,
    /// The lines, as ranges of `segments`.
    lines: Vec<std::ops::Range<usize>>,
    cells: HashMap<(i64, i64), Vec<usize>>,
}

impl RoadIndex {
    pub(crate) fn new(route: &Route, projection: &LocalProjection) -> Self {
        let points: Vec<Centre> = route
            .points()
            .iter()
            .map(|p| Centre {
                position: projection.project(p.lat, p.lon),
                elevation: p.elevation.0,
                distance: p.distance.0,
                surface: p.surface,
            })
            .collect();
        let mut index = Self::from_lines(&[smooth_curve(&points)]);
        index.mark_repeats();
        index
    }

    /// Marks the segments where the route rides the same road again (#101), as
    /// `torqa_routes` finds them to give them one height: the road is drawn once.
    fn mark_repeats(&mut self) {
        for i in 0..self.segments.len() {
            let segment = self.segments[i];
            let middle = (
                f64::midpoint(segment.a.0, segment.b.0),
                f64::midpoint(segment.a.1, segment.b.1),
            );
            let way = direction(&segment);
            let (low, high) = (
                cell_of(middle.0 - SAME_ROAD, middle.1 - SAME_ROAD),
                cell_of(middle.0 + SAME_ROAD, middle.1 + SAME_ROAD),
            );
            let repeat = (low.0..=high.0)
                .flat_map(|x| (low.1..=high.1).map(move |y| (x, y)))
                .flat_map(|cell| self.cells.get(&cell).into_iter().flatten())
                .any(|&j| {
                    let earlier = &self.segments[j];
                    let gap = segment.distance_a - earlier.distance_b;
                    let back = {
                        let other = direction(earlier);
                        way.0 * other.0 + way.1 * other.1 < -0.7
                    };
                    j < i
                        && earlier.surface == segment.surface
                        && (gap >= APART || (back && gap >= 2.0 * SAME_ROAD))
                        && closest_on_segment(earlier, middle.0, middle.1).0 < SAME_ROAD
                });
            self.segments[i].repeat = repeat;
        }
    }

    /// An index of several centre lines, each a list of points a few metres apart.
    pub(crate) fn from_lines(lines: &[Vec<Centre>]) -> Self {
        let mut segments: Vec<Segment> = Vec::new();
        let mut ranges = Vec::new();
        for line in lines {
            let start = segments.len();
            segments.extend(line.windows(2).map(|w| Segment {
                a: w[0].position,
                b: w[1].position,
                elevation_a: w[0].elevation,
                elevation_b: w[1].elevation,
                distance_a: w[0].distance,
                distance_b: w[1].distance,
                repeat: false,
                portals: [None; 2],
                // A segment touching a bridge or tunnel belongs to it.
                surface: if w[0].surface == Surface::Ground {
                    w[1].surface
                } else {
                    w[0].surface
                },
            }));
            if segments.len() > start {
                ranges.push(start..segments.len());
            }
        }
        let mut cells: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        for (index, segment) in segments.iter().enumerate() {
            // Segments are a few metres long, far shorter than a cell: both end cells cover
            // them.
            for point in [segment.a, segment.b] {
                let list = cells.entry(cell_of(point.0, point.1)).or_default();
                if list.last() != Some(&index) {
                    list.push(index);
                }
            }
        }
        Self {
            segments,
            lines: ranges,
            cells,
        }
    }

    /// Whether a way through (`east`, `north`) heading in `direction` (unit) runs along the road
    /// within `reach` there, rather than joining or crossing it.
    pub(crate) fn runs_along(
        &self,
        east: f64,
        north: f64,
        reach: f64,
        direction: (f64, f64),
    ) -> bool {
        let (low_e, low_n) = cell_of(east - reach, north - reach);
        let (high_e, high_n) = cell_of(east + reach, north + reach);
        (low_e..=high_e).any(|ce| {
            (low_n..=high_n).any(|cn| {
                self.cells
                    .get(&(ce, cn))
                    .into_iter()
                    .flatten()
                    .any(|&index| {
                        let segment = &self.segments[index];
                        let (d, _, _) = closest_on_segment(segment, east, north);
                        let (de, dn) = self::direction(segment);
                        d <= reach && (de * direction.0 + dn * direction.1).abs() > 0.8
                    })
            })
        })
    }

    /// Every piece of road within `reach`: its distance, the road's elevation there and what
    /// carries it. Where the road passes a place more than once (hairpins), each pass is there.
    pub(crate) fn near(&self, east: f64, north: f64, reach: f64) -> Vec<(f64, f64, Surface)> {
        let (low_e, low_n) = cell_of(east - reach, north - reach);
        let (high_e, high_n) = cell_of(east + reach, north + reach);
        let mut found = Vec::new();
        for ce in low_e..=high_e {
            for cn in low_n..=high_n {
                for &index in self.cells.get(&(ce, cn)).into_iter().flatten() {
                    let segment = &self.segments[index];
                    // Listed in both its ends' cells: look at it from the first one in range.
                    let first = cell_of(segment.a.0, segment.a.1);
                    let in_range = |(e, n): (i64, i64)| {
                        (low_e..=high_e).contains(&e) && (low_n..=high_n).contains(&n)
                    };
                    if first != (ce, cn) && in_range(first) {
                        continue;
                    }
                    let candidate = closest_on_segment(segment, east, north);
                    if candidate.0 <= reach {
                        found.push(candidate);
                    }
                }
            }
        }
        found
    }

    /// The nearest stretch of a line within `reach` of (`east`, `north`) running along `way`
    /// (either way round): its height there and what carries it.
    pub(crate) fn alongside(
        &self,
        east: f64,
        north: f64,
        way: (f64, f64),
        reach: f64,
    ) -> Option<(f64, Surface)> {
        let (low_e, low_n) = cell_of(east - reach, north - reach);
        let (high_e, high_n) = cell_of(east + reach, north + reach);
        let mut best: Option<(f64, f64, Surface)> = None;
        for ce in low_e..=high_e {
            for cn in low_n..=high_n {
                for &index in self.cells.get(&(ce, cn)).into_iter().flatten() {
                    let segment = &self.segments[index];
                    let (de, dn) = direction(segment);
                    if (de * way.0 + dn * way.1).abs() < 0.9 {
                        continue;
                    }
                    let candidate = closest_on_segment(segment, east, north);
                    if candidate.0 <= reach && best.is_none_or(|b| candidate.0 < b.0) {
                        best = Some(candidate);
                    }
                }
            }
        }
        best.map(|b| (b.1, b.2))
    }

    /// Distance to the closest point of the road within `max_distance`, the road's elevation
    /// there and what carries the road.
    pub(crate) fn nearest(
        &self,
        east: f64,
        north: f64,
        max_distance: f64,
    ) -> Option<(f64, f64, Surface)> {
        let reach = if max_distance.is_finite() {
            max_distance
        } else {
            // Unbounded: the whole road.
            return self
                .segments
                .iter()
                .map(|s| closest_on_segment(s, east, north))
                .min_by(|a, b| a.0.total_cmp(&b.0));
        };
        let (low_e, low_n) = cell_of(east - reach, north - reach);
        let (high_e, high_n) = cell_of(east + reach, north + reach);
        let mut best: Option<(f64, f64, Surface)> = None;
        for ce in low_e..=high_e {
            for cn in low_n..=high_n {
                let Some(indices) = self.cells.get(&(ce, cn)) else {
                    continue;
                };
                for &index in indices {
                    let candidate = closest_on_segment(&self.segments[index], east, north);
                    if candidate.0 <= reach && best.is_none_or(|b| candidate.0 < b.0) {
                        best = Some(candidate);
                    }
                }
            }
        }
        best
    }

    /// The nearest point of the road within `reach` of (`east`, `north`): the distance to it,
    /// how far along the road it is and on which side the point lies (1 right of travel, −1
    /// left).
    pub(crate) fn locate(&self, east: f64, north: f64, reach: f64) -> Option<(f64, f64, f64)> {
        let (low_e, low_n) = cell_of(east - reach, north - reach);
        let (high_e, high_n) = cell_of(east + reach, north + reach);
        let mut best: Option<(f64, f64, f64)> = None;
        for ce in low_e..=high_e {
            for cn in low_n..=high_n {
                for &index in self.cells.get(&(ce, cn)).into_iter().flatten() {
                    let segment = &self.segments[index];
                    let (de, dn) = (segment.b.0 - segment.a.0, segment.b.1 - segment.a.1);
                    let length_squared = (de * de + dn * dn).max(1e-12);
                    let t = (((east - segment.a.0) * de + (north - segment.a.1) * dn)
                        / length_squared)
                        .clamp(0.0, 1.0);
                    let (pe, pn) = (segment.a.0 + de * t, segment.a.1 + dn * t);
                    let distance = (east - pe).hypot(north - pn);
                    if distance <= reach && best.is_none_or(|b| distance < b.0) {
                        // Right of travel is where the direction turns clockwise.
                        let cross = de * (north - segment.a.1) - dn * (east - segment.a.0);
                        let along =
                            segment.distance_a + (segment.distance_b - segment.distance_a) * t;
                        best = Some((distance, along, if cross > 0.0 { -1.0 } else { 1.0 }));
                    }
                }
            }
        }
        best
    }

    /// The nearest point of the road within `reach` of (`east`, `north`) and the road's
    /// direction there (unit, the way the route rides it).
    pub(crate) fn heading(
        &self,
        east: f64,
        north: f64,
        reach: f64,
    ) -> Option<((f64, f64), (f64, f64))> {
        let (low_e, low_n) = cell_of(east - reach, north - reach);
        let (high_e, high_n) = cell_of(east + reach, north + reach);
        // Distance, segment and share along it.
        let mut best: Option<(f64, usize, f64)> = None;
        for ce in low_e..=high_e {
            for cn in low_n..=high_n {
                for &index in self.cells.get(&(ce, cn)).into_iter().flatten() {
                    let segment = &self.segments[index];
                    let (de, dn) = (segment.b.0 - segment.a.0, segment.b.1 - segment.a.1);
                    let length_squared = de * de + dn * dn;
                    if length_squared < 1e-12 {
                        continue;
                    }
                    let t = (((east - segment.a.0) * de + (north - segment.a.1) * dn)
                        / length_squared)
                        .clamp(0.0, 1.0);
                    let d = (east - segment.a.0 - de * t).hypot(north - segment.a.1 - dn * t);
                    if d <= reach && best.is_none_or(|b| d < b.0) {
                        best = Some((d, index, t));
                    }
                }
            }
        }
        best.map(|(_, index, t)| {
            let segment = &self.segments[index];
            (
                (
                    segment.a.0 + (segment.b.0 - segment.a.0) * t,
                    segment.a.1 + (segment.b.1 - segment.a.1) * t,
                ),
                direction(segment),
            )
        })
    }

    /// Points along the road roughly every `spacing` metres, in metres east/north.
    pub(crate) fn samples(&self, spacing: f64) -> Vec<(f64, f64)> {
        let mut samples = Vec::new();
        let mut next = 0.0;
        for segment in &self.segments {
            if segment.distance_a >= next {
                samples.push(segment.a);
                next = segment.distance_a + spacing;
            }
        }
        if let Some(last) = self.segments.last() {
            samples.push(last.b);
        }
        samples
    }

    /// Consecutive centre-line points on bridges and in tunnels, one list per structure.
    pub(crate) fn structure_runs(&self) -> Vec<(Surface, Vec<CentrePoint>)> {
        let mut runs: Vec<(Surface, Vec<CentrePoint>)> = Vec::new();
        let mut previous = Surface::Ground;
        for (index, segment) in self.segments.iter().enumerate() {
            // A structure never runs on from one line into another.
            if self.lines.iter().any(|line| line.start == index) {
                previous = Surface::Ground;
            }
            let point = |position: (f64, f64), elevation: f64| CentrePoint {
                position,
                elevation,
                direction: direction(segment),
            };
            if segment.surface != Surface::Ground {
                if segment.surface != previous {
                    runs.push((segment.surface, vec![point(segment.a, segment.elevation_a)]));
                }
                if let Some((_, points)) = runs.last_mut() {
                    points.push(point(segment.b, segment.elevation_b));
                }
            }
            previous = segment.surface;
        }
        runs
    }

    /// The tunnels of the lines, each as one run of segments.
    pub(crate) fn tunnels(&self) -> Vec<TunnelRun> {
        let mut runs = Vec::new();
        for line in &self.lines {
            let mut k = line.start;
            while k < line.end {
                if self.segments[k].surface != Surface::Tunnel {
                    k += 1;
                    continue;
                }
                let start = k;
                while k < line.end && self.segments[k].surface == Surface::Tunnel {
                    k += 1;
                }
                let run = &self.segments[start..k];
                let mut points: Vec<CentrePoint> = run
                    .iter()
                    .map(|s| centre_point(s, s.a, s.elevation_a))
                    .collect();
                let last = &run[run.len() - 1];
                points.push(centre_point(last, last.b, last.elevation_b));
                runs.push(TunnelRun {
                    segments: start..k,
                    line: line.clone(),
                    points,
                    open: (start > line.start, k < line.end),
                });
            }
        }
        runs
    }

    /// Opens the first `start` and last `end` segments of a tunnel `run` (from
    /// [`RoadIndex::tunnels`], before any is opened) onto the ground, where it has not yet
    /// entered the hill (#135), and gives the open line leading to each end its portal's plane:
    /// the cutting it lies in ends there instead of rounding off into the hill over the tunnel.
    /// `None` leaves the tunnel as it is, without portals. Returns the portals: their plane,
    /// the line's height there and how steeply it rises into the tunnel.
    pub(crate) fn set_portals(
        &mut self,
        run: &TunnelRun,
        opened: Option<(usize, usize)>,
    ) -> Vec<(Plane, f64, f64)> {
        let (first, last) = (run.segments.start, run.segments.end);
        // Planes set before, for the line as mapped, give way.
        for segment in self.segments[run.line.start..first]
            .iter_mut()
            .rev()
            .take_while(|s| s.surface != Surface::Tunnel)
        {
            segment.portals[0] = None;
        }
        for segment in self.segments[last..run.line.end]
            .iter_mut()
            .take_while(|s| s.surface != Surface::Tunnel)
        {
            segment.portals[1] = None;
        }
        let Some((start, end)) = opened else {
            return Vec::new();
        };
        let (first, last) = (first + start, last - end);
        for segment in &mut self.segments[run.segments.start..first] {
            segment.surface = Surface::Ground;
        }
        for segment in &mut self.segments[last..run.segments.end] {
            segment.surface = Surface::Ground;
        }
        let mut portals = Vec::new();
        if run.open.0 {
            let inside = self.segments[first];
            let plane = (inside.a, direction(&inside));
            let rise = (inside.elevation_b - inside.elevation_a) / length(&inside).max(1e-9);
            portals.push((plane, inside.elevation_a, rise));
            let mut along = 0.0;
            for segment in self.segments[run.line.start..first].iter_mut().rev() {
                along += length(segment);
                if segment.surface != Surface::Ground
                    || along > APPROACH
                    || behind(plane, segment.a)
                    || behind(plane, segment.b)
                {
                    break;
                }
                segment.portals[0] = Some(plane);
            }
        }
        if run.open.1 {
            let inside = self.segments[last - 1];
            let (de, dn) = direction(&inside);
            let plane = (inside.b, (-de, -dn));
            let rise = (inside.elevation_a - inside.elevation_b) / length(&inside).max(1e-9);
            portals.push((plane, inside.elevation_b, rise));
            let mut along = 0.0;
            for segment in &mut self.segments[last..run.line.end] {
                along += length(segment);
                if segment.surface != Surface::Ground
                    || along > APPROACH
                    || behind(plane, segment.a)
                    || behind(plane, segment.b)
                {
                    break;
                }
                segment.portals[1] = Some(plane);
            }
        }
        portals
    }

    /// A ribbon `2 × half_width` wide along the centre line, its edges bevelled down to the
    /// ground and skirts hanging on below. Texture coordinates: `u` 0–1 across the road (below
    /// 0 and above 1 on bevels and skirts, which are shoulders — except where another street
    /// meets the road, `mouths`, where the bevel is road too), `v` the distance in metres.
    pub(crate) fn mesh(&self, half_width: f64, mouths: &[Mouth]) -> MeshData {
        let mut mesh = MeshData::default();
        for line in &self.lines {
            for run in drawn(&self.segments[line.clone()]) {
                mesh.append(line_mesh(run, half_width, mouths));
            }
        }
        mesh
    }
}

/// The stretches of a line's segments to draw: all but those ridden a second time, each
/// reaching a few metres into them so it meets the road drawn by the first pass without a gap.
fn drawn(segments: &[Segment]) -> Vec<&[Segment]> {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // a few segments
    let overlap = (2.0 * SAME_ROAD / DRAW_STEP).ceil() as usize;
    let mut runs: Vec<std::ops::Range<usize>> = Vec::new();
    let mut k = 0;
    while k < segments.len() {
        if segments[k].repeat {
            k += 1;
            continue;
        }
        let start = k;
        while k < segments.len() && !segments[k].repeat {
            k += 1;
        }
        let run = start.saturating_sub(overlap)..(k + overlap).min(segments.len());
        match runs.last_mut() {
            Some(last) if last.end >= run.start => last.end = run.end,
            _ => runs.push(run),
        }
    }
    runs.into_iter().map(|run| &segments[run]).collect()
}

/// The ribbon of [`RoadIndex::mesh`] along one line's segments.
#[allow(clippy::cast_possible_truncation)] // geometry is stored as f32 for the GPU
fn line_mesh(segments: &[Segment], half_width: f64, mouths: &[Mouth]) -> MeshData {
    let mut mesh = MeshData::default();
    let Some(last) = segments.last() else {
        return mesh;
    };
    let centres: Vec<((f64, f64), f64, f64)> = segments
        .iter()
        .map(|s| (s.a, s.elevation_a, s.distance_a))
        .chain([(last.b, last.elevation_b, last.distance_b)])
        .collect();
    let joined = |distance: f64, side: f64| {
        mouths
            .iter()
            .any(|&(along, at, half)| at * side > 0.0 && (along - distance).abs() <= half + 1.0)
    };
    let mut rings: u32 = 0;
    for (index, &((east, north), elevation, distance)) in centres.iter().enumerate() {
        // Across the road square to the curve: halfway between the pieces either side. Sharp
        // corners get a fan of rings turning from the one piece to the other, so they are
        // round rather than pointed, and a turn back the same way ends round (#101).
        let before = segments[index.saturating_sub(1)];
        let after = segments[index.min(segments.len() - 1)];
        let (d1, d2) = (direction(&before), direction(&after));
        let turn = (d1.0 * d2.1 - d1.1 * d2.0).atan2(d1.0 * d2.0 + d1.1 * d2.1);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // a few rings
        let fan = (turn.abs() / FAN_STEP).ceil() as usize;
        let ways: Vec<(f64, f64)> = if fan <= 1 {
            let (de, dn) = (d1.0 + d2.0, d1.1 + d2.1);
            let length = de.hypot(dn).max(f64::EPSILON);
            vec![(de / length, dn / length)]
        } else {
            (0..=fan)
                .map(|k| {
                    #[allow(clippy::cast_precision_loss)] // a few rings
                    let (sin, cos) = (turn * k as f64 / fan as f64).sin_cos();
                    (d1.0 * cos - d1.1 * sin, d1.0 * sin + d1.1 * cos)
                })
                .collect()
        };
        for (de, dn) in ways {
            // Right of travel is the direction turned clockwise by 90°.
            let (re, rn) = (dn, -de);
            ring(
                &mut mesh,
                rings,
                ((east, north), elevation, distance),
                (re, rn),
                half_width,
                (joined(distance, -1.0), joined(distance, 1.0)),
            );
            rings += 1;
        }
    }
    mesh
}

/// Ring `index` of vertices across the road at a centre point (its position, elevation and
/// distance along), square to the unit vector right of travel and joined to the ring before
/// it; the last argument says whether streets meet the road on its left and right there.
#[allow(clippy::cast_possible_truncation)] // geometry is stored as f32 for the GPU
fn ring(
    mesh: &mut MeshData,
    index: u32,
    ((east, north), elevation, distance): ((f64, f64), f64, f64),
    (re, rn): (f64, f64),
    half_width: f64,
    (joined_left, joined_right): (bool, bool),
) {
    let at = |across: f64, drop: f64| {
        [
            (east + re * across) as f32,
            (elevation - drop) as f32,
            (-(north + rn * across)) as f32,
        ]
    };
    let (bevel, skirt) = (half_width + BEVEL_REACH, half_width + SKIRT_REACH);
    let left = if joined_left { 0.0 } else { -0.08 };
    let right = if joined_right { 1.0 } else { 1.08 };
    let tilt = BEVEL_DROP / BEVEL_REACH;
    // Left skirt, left bevel, left edge, right edge, right bevel, right skirt.
    let ring = [
        (
            at(-skirt, SKIRT_DEPTH),
            [-re as f32, 0.5, rn as f32],
            -0.2_f32,
        ),
        (
            at(-bevel, BEVEL_DROP),
            [(-re * tilt) as f32, 1.0, (rn * tilt) as f32],
            left,
        ),
        (at(-half_width, 0.0), [0.0, 1.0, 0.0], 0.0),
        (at(half_width, 0.0), [0.0, 1.0, 0.0], 1.0),
        (
            at(bevel, BEVEL_DROP),
            [(re * tilt) as f32, 1.0, (-rn * tilt) as f32],
            right,
        ),
        (at(skirt, SKIRT_DEPTH), [re as f32, 0.5, -rn as f32], 1.2),
    ];
    for (vertex, normal, u) in ring {
        mesh.vertices.push(vertex);
        let length = (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2]).sqrt();
        mesh.normals.push(normal.map(|v| v / length));
        mesh.uvs.push([u, distance as f32]);
    }
    if index > 0 {
        let size = u32::try_from(RING).expect("small");
        let base = index * size;
        let previous = base - size;
        for k in 0..size - 1 {
            let (a0, b0, a1, b1) = (previous + k, previous + k + 1, base + k, base + k + 1);
            mesh.indices.extend([a0, a1, b1, a0, b1, b0]);
        }
    }
}

/// A smooth curve through the route's points (centripetal Catmull-Rom), every `DRAW_STEP`
/// metres or so. Elevation and distance change linearly between the points, so the road's
/// profile stays as smoothed on import.
fn smooth_curve(points: &[Centre]) -> Vec<Centre> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let mut out = Vec::with_capacity(points.len() * 5);
    for i in 0..points.len() - 1 {
        let (p1, p2) = (points[i], points[i + 1]);
        let p0 = points[i.saturating_sub(1)];
        let p3 = points[(i + 2).min(points.len() - 1)];
        let length = distance(p1.position, p2.position);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // short segments
        let pieces = ((length / DRAW_STEP).ceil() as usize).max(1);
        for k in 0..pieces {
            #[allow(clippy::cast_precision_loss)] // few pieces
            let u = k as f64 / pieces as f64;
            out.push(Centre {
                position: catmull_rom([p0.position, p1.position, p2.position, p3.position], u),
                elevation: p1.elevation + (p2.elevation - p1.elevation) * u,
                distance: p1.distance + (p2.distance - p1.distance) * u,
                // Between a structure's point and the ground, the piece belongs to the
                // structure, as the route's segments do.
                surface: if u == 0.0 || p1.surface != Surface::Ground {
                    p1.surface
                } else {
                    p2.surface
                },
            });
        }
    }
    out.extend(points.last());
    out
}

fn distance(a: (f64, f64), b: (f64, f64)) -> f64 {
    (b.0 - a.0).hypot(b.1 - a.1)
}

/// A tunnel of one of the lines, as [`RoadIndex::tunnels`] finds it.
pub(crate) struct TunnelRun {
    /// Its segments, and those of its line.
    segments: std::ops::Range<usize>,
    line: std::ops::Range<usize>,
    /// Its centre line: each segment's start and the last one's end.
    pub(crate) points: Vec<CentrePoint>,
    /// Whether open line leads into it at its start and out of it at its end, rather than the
    /// line beginning or ending in it.
    pub(crate) open: (bool, bool),
}

/// A point on the road's centre line with its direction of travel.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CentrePoint {
    /// Metres east/north.
    pub(crate) position: (f64, f64),
    /// Road surface elevation.
    pub(crate) elevation: f64,
    /// Unit direction of travel (east, north).
    pub(crate) direction: (f64, f64),
}

fn cell_of(east: f64, north: f64) -> (i64, i64) {
    #[allow(clippy::cast_possible_truncation)] // local coordinates stay far below 2^63 cells
    ((east / CELL).floor() as i64, (north / CELL).floor() as i64)
}

/// Unit direction of travel of a segment (east, north).
fn direction(segment: &Segment) -> (f64, f64) {
    let (de, dn) = (segment.b.0 - segment.a.0, segment.b.1 - segment.a.1);
    let length = de.hypot(dn).max(f64::EPSILON);
    (de / length, dn / length)
}

/// Distance from a point to a segment, the road elevation at the closest point and the
/// segment's surface.
fn closest_on_segment(segment: &Segment, east: f64, north: f64) -> (f64, f64, Surface) {
    let (de, dn) = (segment.b.0 - segment.a.0, segment.b.1 - segment.a.1);
    let length_squared = de * de + dn * dn;
    let t = if length_squared > 0.0 {
        (((east - segment.a.0) * de + (north - segment.a.1) * dn) / length_squared).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (pe, pn) = (segment.a.0 + de * t, segment.a.1 + dn * t);
    let elevation = segment.elevation_a + (segment.elevation_b - segment.elevation_a) * t;
    // Past a portal the line is in its tunnel, so the ground there is the hill's (#135).
    let past = segment
        .portals
        .iter()
        .flatten()
        .any(|&plane| behind(plane, (east, north)));
    let surface = if past {
        Surface::Tunnel
    } else {
        segment.surface
    };
    ((east - pe).hypot(north - pn), elevation, surface)
}

/// Whether `point` lies past a portal's `plane`, on the tunnel's side.
fn behind(((pe, pn), (de, dn)): Plane, (east, north): (f64, f64)) -> bool {
    (east - pe) * de + (north - pn) * dn > 1e-9
}

fn length(segment: &Segment) -> f64 {
    distance(segment.a, segment.b)
}

fn centre_point(segment: &Segment, position: (f64, f64), elevation: f64) -> CentrePoint {
    CentrePoint {
        position,
        elevation,
        direction: direction(segment),
    }
}
