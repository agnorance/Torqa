//! Bridges and tunnels of the road ridden and the railways. Short, low bridges are stone arch
//! bridges (#75): arched openings between piers, the walls over the arches and the vaults under
//! them; longer and higher ones, and those over a road, a street or a track, are viaducts, their
//! deck on piers as wide as itself. Piers keep off the ways below (#98). Tunnels are an arched
//! tube, starting where the ground rises over it: there a headwall stands in the hill at the end
//! of the cutting leading to it (#135). Parallel tracks share one bridge or tunnel, as wide as
//! they need (#99).

use std::collections::HashMap;
use std::sync::LazyLock;

use torqa_routes::{ElevationModel, LocalProjection, Surface};

use crate::road::{CentrePoint, Plane, RoadIndex, TunnelRun};
use crate::streets::Street;
use crate::{LEVEL_REACH, MeshData, ROAD_HALF_WIDTH, Shapers, palette, railways, shape};

static CONCRETE: LazyLock<[f32; 4]> = LazyLock::new(|| palette::srgb("structure.concrete", 0.0));
static STONE: LazyLock<[f32; 4]> = LazyLock::new(|| palette::srgb("structure.stone", 0.0));
static TUNNEL_WALL: LazyLock<[f32; 4]> = LazyLock::new(|| palette::srgb("structure.tunnel", 0.0));

/// Half the deck width: the road plus a narrow kerb.
const DECK_HALF_WIDTH: f64 = 3.6;
const DECK_THICKNESS: f64 = 1.2;
const PARAPET_HEIGHT: f64 = 1.0;
const PARAPET_THICKNESS: f64 = 0.3;
const PILLAR_SPACING: f64 = 30.0;
/// Viaduct piers: this thick along the deck, and as wide as the deck less this on each side.
const PIER_HALF_LENGTH: f64 = 1.0;
const PIER_INSET: f64 = 0.4;
/// Pillars only where the ground is at least this far below the deck.
const MIN_PILLAR_HEIGHT: f64 = 2.0;
/// Bridges at most this long and high are stone arch bridges...
const ARCH_BRIDGE_LENGTH: f64 = 60.0;
const ARCH_BRIDGE_HEIGHT: f64 = 15.0;
/// ...with arches about this wide between piers this thick...
const ARCH_SPAN: f64 = 14.0;
const ARCH_PIER: f64 = 2.0;
/// ...their crowns this far under the deck, and drawn in this many pieces.
const ARCH_CROWN: f64 = 0.5;
const ARCH_PIECES: usize = 10;
/// The ground under a bridge is looked at this often.
const GROUND_STEP: f64 = 2.0;
/// Piers and walls reach this far below the natural ground: channels may cut it (`channels`).
const FOOTING: f64 = 2.5;
/// Tunnels are this wide either side of their line, and at most this high.
const TUNNEL_RADIUS: f64 = 5.0;
const TUNNEL_HEIGHT: f64 = 7.5;
/// Ground left over a railway tunnel where it passes under the road ridden (#138).
const TUNNEL_COVER: f64 = 0.3;
/// A railway this far or more below the road ridden passes under it, rather than beside it.
const UNDERPASS: f64 = 1.0;
/// The road has shaped the ground where it differs from the natural ground by more than this.
const SHAPED: f64 = 0.01;
const ARCH_SEGMENTS: usize = 12;
/// A tunnel enters the hill where the ground lies this far over its crown, looked at this far
/// inside (#135)...
const PORTAL_COVER: f64 = 0.5;
const PORTAL_PROBE: f64 = 1.0;
/// ...and its headwall rises this far over the crown, this thick, reaching out beside the
/// opening until the ground before it is as high, by at most this much.
const HEADWALL_RISE: f64 = 1.5;
const HEADWALL_THICKNESS: f64 = 0.8;
const HEADWALL_REACH: f64 = 8.0;
/// Ground this far before a portal and this far inside it is kept out of the opening, within
/// this margin of the tube.
const HOLLOW_FRONT: f64 = 4.0;
const HOLLOW_DEPTH: f64 = 6.0;
const HOLLOW_MARGIN: f64 = 0.5;
/// Railway bridges or tunnels running alongside one another this close (centre to centre) are
/// one.
const BUNDLE_REACH: f64 = 15.0;
/// Ways lying this far below a deck pass under it: no pier or wall stands within this margin of
/// their edges...
const CLEARANCE: f64 = 2.5;
const CLEAR_MARGIN: f64 = 1.0;
/// ...a pier moves along the bridge off them by up to this much, or is left out.
const PIER_SHIFT: f64 = PILLAR_SPACING / 2.0;
/// Index cell size of the streets in [`Below`].
const BELOW_CELL: f64 = 10.0;

/// Street points with the streets' half widths, by index cell.
type StreetPoints = HashMap<(i64, i64), Vec<(f64, f64, f64)>>;

/// What runs on the ground under bridges — the road ridden, the railways and the other streets —
/// for piers and walls to keep off it (#98).
pub(crate) struct Below<'a> {
    road: &'a RoadIndex,
    railways: &'a RoadIndex,
    /// Street points, a few metres apart.
    streets: StreetPoints,
}

impl<'a> Below<'a> {
    pub(crate) fn new(road: &'a RoadIndex, railways: &'a RoadIndex, streets: &[Street]) -> Self {
        let mut cells = StreetPoints::new();
        for street in streets.iter().filter(|s| !s.on_bridge()) {
            for &(east, north) in street.points() {
                cells.entry(below_cell(east, north)).or_default().push((
                    east,
                    north,
                    street.half_width(),
                ));
            }
        }
        Self {
            road,
            railways,
            streets: cells,
        }
    }

    /// The road ridden.
    pub(crate) fn road(&self) -> &RoadIndex {
        self.road
    }

    /// Whether a way passes at `position` (within `margin` of its edge) under a deck whose
    /// underside lies at `deck`, the ground there at `ground` (streets lie on it).
    pub(crate) fn blocked(
        &self,
        (east, north): (f64, f64),
        margin: f64,
        deck: f64,
        ground: f64,
    ) -> bool {
        let low = deck - CLEARANCE;
        let lines = [
            (self.road, ROAD_HALF_WIDTH),
            (self.railways, railways::BED_M / 2.0),
        ];
        let on_a_line = lines.iter().any(|&(line, half)| {
            line.near(east, north, half + margin)
                .iter()
                .any(|&(_, height, surface)| surface == Surface::Ground && height < low)
        });
        if on_a_line {
            return true;
        }
        if ground >= low {
            return false;
        }
        let (ce, cn) = below_cell(east, north);
        (ce - 1..=ce + 1).any(|x| {
            (cn - 1..=cn + 1).any(|y| {
                self.streets.get(&(x, y)).is_some_and(|points| {
                    // Points lie a few metres apart: allow half a step along.
                    points
                        .iter()
                        .any(|&(e, n, half)| (e - east).hypot(n - north) < half + margin + 1.5)
                })
            })
        })
    }
}

#[allow(clippy::cast_possible_truncation)] // local metres stay far below 2^63 cells
fn below_cell(east: f64, north: f64) -> (i64, i64) {
    (
        (east / BELOW_CELL).floor() as i64,
        (north / BELOW_CELL).floor() as i64,
    )
}

/// A point of a structure's line, and how far the structure reaches left and right of it.
#[derive(Debug, Clone, Copy)]
struct Section {
    centre: CentrePoint,
    left: f64,
    right: f64,
}

impl Section {
    /// How far right of the line the structure's edge on `side` (−1 left, 1 right) lies.
    fn edge(&self, side: f64) -> f64 {
        if side < 0.0 { -self.left } else { self.right }
    }
}

/// Where a tunnel enters the hill (#135): the plane of its headwall, and the line's height
/// there and how steeply it rises into the tunnel.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Portal {
    plane: Plane,
    floor: f64,
    rise: f64,
}

impl Portal {
    /// How far past the plane into the tunnel (`east`, `north`) lies, and how far right of the
    /// line looking in.
    fn local(&self, east: f64, north: f64) -> (f64, f64) {
        let ((pe, pn), (de, dn)) = self.plane;
        let (x, y) = (east - pe, north - pn);
        (x * de + y * dn, x * dn - y * de)
    }

    /// Whether (`east`, `north`) lies within `reach` of the portal's plane, beside the cutting
    /// that ends there: the ground steps from the cutting up to the hill there.
    pub(crate) fn near(&self, east: f64, north: f64, reach: f64) -> bool {
        let (along, across) = self.local(east, north);
        along.abs() <= reach && across.abs() <= LEVEL_REACH + reach
    }

    /// Whether a point of the ground (east, north, height) lies in the tunnel's opening, just
    /// before the portal or just inside it: ground there would close it.
    pub(crate) fn hollow(&self, [east, north, height]: [f64; 3]) -> bool {
        let (along, across) = self.local(east, north);
        if !(-HOLLOW_FRONT..HOLLOW_DEPTH).contains(&along) {
            return false;
        }
        let half = TUNNEL_RADIUS + HOLLOW_MARGIN;
        let up = height - (self.floor + self.rise * along);
        let arch = (TUNNEL_RADIUS.min(TUNNEL_HEIGHT) + HOLLOW_MARGIN)
            * (1.0 - (across / half).powi(2)).max(0.0).sqrt();
        across.abs() < half && up > 0.05 && up < arch
    }
}

/// Finds where each tunnel of the road ridden and the railways enters the hill, where the
/// ground first lies over its crown, and opens the line before that onto the ground (#135): the
/// tube starts there, the approach runs in a cutting up to the portal and the hill over the
/// tunnel stays as it is. Tunnels the ground never covers (galleries, or hills the terrain data
/// misses) stay as mapped, without portals.
pub(crate) async fn open_portals<M: ElevationModel>(
    road: &mut RoadIndex,
    rails: &mut RoadIndex,
    projection: &LocalProjection,
    model: &mut M,
) -> Vec<Portal> {
    let (road_runs, rail_runs) = (road.tunnels(), rails.tunnels());
    // The hill over a tunnel is looked at with the cuttings leading to it ending where it is
    // mapped to begin, not rounding off into the hill.
    for run in &road_runs {
        road.set_portals(run, Some((0, 0)));
    }
    for run in &rail_runs {
        rails.set_portals(run, Some((0, 0)));
    }
    let mut road_openings = Vec::new();
    let mut rail_openings = Vec::new();
    {
        let shapers = Shapers {
            road: &*road,
            rails: &*rails,
            portals: &[],
        };
        for run in &road_runs {
            road_openings.push(open_ends(run, &shapers, projection, model).await);
        }
        for run in &rail_runs {
            rail_openings.push(open_ends(run, &shapers, projection, model).await);
        }
    }
    let mut portals = Vec::new();
    for (index, runs, openings) in [
        (road, &road_runs, road_openings),
        (rails, &rail_runs, rail_openings),
    ] {
        for (run, opened) in runs.iter().zip(openings) {
            portals.extend(
                index
                    .set_portals(run, opened)
                    .into_iter()
                    .map(|(plane, floor, rise)| Portal { plane, floor, rise }),
            );
        }
    }
    portals
}

/// How many segments at either end of a tunnel `run` lie before the ground rises over its
/// crown, or `None` where it never covers it.
async fn open_ends<M: ElevationModel>(
    run: &TunnelRun,
    shapers: &Shapers<'_>,
    projection: &LocalProjection,
    model: &mut M,
) -> Option<(usize, usize)> {
    let points = &run.points;
    let segments = points.len() - 1;
    let mut start = None;
    for (k, &point) in points[..segments].iter().enumerate() {
        // Where the line begins in the tunnel there is no portal to look for.
        if !run.open.0 || covered(point, 1.0, shapers, projection, model).await {
            start = Some(k);
            break;
        }
    }
    let start = start?;
    let mut end = None;
    for (k, &point) in points[start + 1..].iter().rev().enumerate() {
        if !run.open.1 || covered(point, -1.0, shapers, projection, model).await {
            end = Some(k);
            break;
        }
    }
    let end = end?;
    // A little of the tube is left at least.
    (start + end + 2 <= segments).then_some((start, end))
}

/// Whether the ground lies over the crown of a tube at `point` across its whole width, a little
/// way into the tunnel (`inward` 1 along the line, −1 against it).
async fn covered<M: ElevationModel>(
    point: CentrePoint,
    inward: f64,
    shapers: &Shapers<'_>,
    projection: &LocalProjection,
    model: &mut M,
) -> bool {
    let (de, dn) = point.direction;
    let probe = CentrePoint {
        position: (
            point.position.0 + de * inward * PORTAL_PROBE,
            point.position.1 + dn * inward * PORTAL_PROBE,
        ),
        ..point
    };
    let crown = point.elevation + TUNNEL_RADIUS.min(TUNNEL_HEIGHT) + PORTAL_COVER;
    for across in [-TUNNEL_RADIUS, 0.0, TUNNEL_RADIUS] {
        let height = ground(aside(probe, across), shapers, projection, model).await;
        if height.is_none_or(|h| h < crown) {
            return false;
        }
    }
    true
}

/// The ground at a point as the terrain has it, shaped by the road ridden and the railways;
/// `None` without terrain data there.
async fn ground<M: ElevationModel>(
    (east, north): (f64, f64),
    shapers: &Shapers<'_>,
    projection: &LocalProjection,
    model: &mut M,
) -> Option<f64> {
    let (lat, lon) = projection.unproject(east, north);
    let natural = model.elevation(lat, lon).await.ok()?;
    Some(shape(natural, &shapers.near(east, north, LEVEL_REACH)))
}

/// Geometry of the bridges and tunnels of the road ridden and the railways, in route
/// coordinates, with headwalls at the tunnels' `portals`.
pub(crate) async fn build_all<M: ElevationModel>(
    (road, railways): (&RoadIndex, &RoadIndex),
    below: &Below<'_>,
    portals: &[Portal],
    projection: &LocalProjection,
    model: &mut M,
) -> MeshData {
    let mut mesh = MeshData::default();
    let own = road
        .structure_runs()
        .into_iter()
        .map(|(surface, run)| (surface, sections(&run, half_width(surface)), None));
    let rails = bundles(&railways.structure_runs())
        .into_iter()
        .map(|(surface, run)| (surface, run, Some(road)));
    for (surface, run, over) in own.chain(rails) {
        match surface {
            Surface::Bridge => bridge(&mut mesh, &run, below, projection, model).await,
            Surface::Tunnel => {
                let over = over.map(|road| (road, projection, &mut *model));
                tunnel(&mut mesh, &run, over).await;
                let shapers = Shapers {
                    road,
                    rails: railways,
                    portals,
                };
                for (end, outward) in [(run.first(), -1.0), (run.last(), 1.0)] {
                    let Some(&end) = end else {
                        continue;
                    };
                    let at_portal = portals
                        .iter()
                        .any(|p| distance(p.plane.0, end.centre.position) < 1.0);
                    if at_portal {
                        headwall(&mut mesh, end, outward, &shapers, projection, model).await;
                    }
                }
            }
            Surface::Ground => {}
        }
    }
    mesh
}

/// How far a structure reaches either side of a single line.
fn half_width(surface: Surface) -> f64 {
    if surface == Surface::Tunnel {
        TUNNEL_RADIUS
    } else {
        DECK_HALF_WIDTH
    }
}

fn sections(run: &[CentrePoint], half: f64) -> Vec<Section> {
    run.iter()
        .map(|&centre| Section {
            centre,
            left: half,
            right: half,
        })
        .collect()
}

/// The railways' structures: those running alongside a longer one of their kind (parallel
/// tracks) are carried by it, which widens to cover them; their stretches beyond it stand on
/// their own.
fn bundles(runs: &[(Surface, Vec<CentrePoint>)]) -> Vec<(Surface, Vec<Section>)> {
    let length = |run: &[CentrePoint]| {
        run.windows(2)
            .map(|w| distance(w[0].position, w[1].position))
            .sum::<f64>()
    };
    let mut order: Vec<usize> = (0..runs.len()).collect();
    order.sort_by(|&a, &b| length(&runs[b].1).total_cmp(&length(&runs[a].1)));
    let mut taken = vec![false; runs.len()];
    let mut bundles = Vec::new();
    for &guide in &order {
        if taken[guide] {
            continue;
        }
        taken[guide] = true;
        let (kind, line) = (runs[guide].0, &runs[guide].1);
        let half = half_width(kind);
        let mut widths = vec![(half, half); line.len()];
        for &other in &order {
            if taken[other] || runs[other].0 != kind {
                continue;
            }
            let placed: Vec<Option<(usize, f64)>> =
                runs[other].1.iter().map(|p| across(line, p)).collect();
            if placed.iter().flatten().count() * 2 < placed.len() {
                continue;
            }
            taken[other] = true;
            for &(index, offset) in placed.iter().flatten() {
                for width in &mut widths[index.saturating_sub(1)..=(index + 1).min(line.len() - 1)]
                {
                    width.0 = width.0.max(half - offset);
                    width.1 = width.1.max(half + offset);
                }
            }
            let mut stretch = Vec::new();
            for (point, place) in runs[other].1.iter().zip(&placed) {
                if place.is_none() {
                    stretch.push(*point);
                    continue;
                }
                if stretch.len() >= 2 {
                    bundles.push((kind, sections(&stretch, half)));
                }
                stretch.clear();
            }
            if stretch.len() >= 2 {
                bundles.push((kind, sections(&stretch, half)));
            }
        }
        let carried = line
            .iter()
            .zip(widths)
            .map(|(&centre, (left, right))| Section {
                centre,
                left,
                right,
            })
            .collect();
        bundles.push((kind, carried));
    }
    bundles
}

/// Where `point` lies across `line`, if it runs alongside it within `BUNDLE_REACH`: the index
/// of the line's nearest point and how far right of the line it lies (left negative).
fn across(line: &[CentrePoint], point: &CentrePoint) -> Option<(usize, f64)> {
    let mut best: Option<(f64, usize, f64)> = None;
    for (k, pair) in line.windows(2).enumerate() {
        let (a, b) = (pair[0].position, pair[1].position);
        let (de, dn) = (b.0 - a.0, b.1 - a.1);
        let length = de.hypot(dn);
        if length <= 0.0 {
            continue;
        }
        let (ue, un) = (de / length, dn / length);
        let t = ((point.position.0 - a.0) * ue + (point.position.1 - a.1) * un) / length;
        // Beyond the line's ends it is not alongside.
        let beyond = (k == 0 && t < 0.0) || (k + 2 == line.len() && t > 1.0);
        let t = t.clamp(0.0, 1.0);
        let foot = (a.0 + de * t, a.1 + dn * t);
        let gap = distance(point.position, foot);
        let parallel = (ue * point.direction.0 + un * point.direction.1).abs() >= 0.9;
        if beyond || !parallel || gap > BUNDLE_REACH || best.is_some_and(|b| b.0 <= gap) {
            continue;
        }
        // Right of travel is the direction turned clockwise by 90°.
        let offset = (point.position.0 - foot.0) * un - (point.position.1 - foot.1) * ue;
        best = Some((gap, if t < 0.5 { k } else { k + 1 }, offset));
    }
    best.map(|b| (b.1, b.2))
}

async fn bridge<M: ElevationModel>(
    mesh: &mut MeshData,
    run: &[Section],
    below: &Below<'_>,
    projection: &LocalProjection,
    model: &mut M,
) {
    let path = Path::new(run);
    // The ground under the bridge, every few metres.
    let mut ground = Vec::new();
    let mut along = 0.0;
    while along <= path.length {
        let point = path.at(along).centre;
        let (lat, lon) = projection.unproject(point.position.0, point.position.1);
        ground.push(model.elevation(lat, lon).await.ok());
        along += GROUND_STEP;
    }
    let ground_at = |along: f64| {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // short bridges
        let k = ((along / GROUND_STEP).round() as usize).min(ground.len().saturating_sub(1));
        ground.get(k).copied().flatten()
    };
    #[allow(clippy::cast_precision_loss)] // a few hundred samples
    let alongs = (0..ground.len()).map(|k| k as f64 * GROUND_STEP);
    let height = alongs
        .clone()
        .filter_map(|along| {
            ground_at(along).map(|g| path.at(along).centre.elevation - DECK_THICKNESS - g)
        })
        .fold(0.0, f64::max);
    // Whether no way passes under the deck at `along`, within `margin` of it.
    let clear = |along: f64, margin: f64| {
        let section = path.at(along);
        let deck = section.centre.elevation - DECK_THICKNESS;
        let Some(ground) = ground_at(along) else {
            return true;
        };
        let mut across = -section.left;
        while across <= section.right + 1e-6 {
            if below.blocked(aside(section.centre, across), margin, deck, ground) {
                return false;
            }
            across += 1.0;
        }
        true
    };
    // Over a way the bridge is a viaduct, its piers either side of it.
    let over_a_way = alongs.clone().any(|along| !clear(along, CLEAR_MARGIN));
    let arched = !over_a_way
        && path.length <= ARCH_BRIDGE_LENGTH
        && (MIN_PILLAR_HEIGHT..=ARCH_BRIDGE_HEIGHT).contains(&height);
    let color = if arched { *STONE } else { *CONCRETE };
    deck(mesh, run, color);
    if arched {
        arches(mesh, &path, &ground_at);
    } else {
        piers(mesh, &path, &ground_at, &clear);
    }
}

/// The deck: its sides from below the road up to the parapets, the parapets' inner faces and
/// tops, and its underside.
fn deck(mesh: &mut MeshData, run: &[Section], color: [f32; 4]) {
    for pair in run.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        for side in [-1.0, 1.0] {
            let outer = (a.edge(side), b.edge(side));
            let inner = (
                outer.0 - PARAPET_THICKNESS * side,
                outer.1 - PARAPET_THICKNESS * side,
            );
            let outward = right(a.centre).map(|v| v * side);
            // Deck side, from the road surface down.
            wall(
                mesh,
                (a.centre, b.centre),
                outer,
                (-DECK_THICKNESS, PARAPET_HEIGHT),
                outward,
                color,
            );
            // Parapet: inner face and top.
            wall(
                mesh,
                (a.centre, b.centre),
                inner,
                (0.0, PARAPET_HEIGHT),
                outward.map(|v| -v),
                color,
            );
            flat(
                mesh,
                (a.centre, b.centre),
                (inner, outer),
                PARAPET_HEIGHT,
                1.0,
                color,
            );
        }
        flat(
            mesh,
            (a.centre, b.centre),
            ((-a.left, -b.left), (a.right, b.right)),
            -DECK_THICKNESS,
            -1.0,
            color,
        );
    }
}

/// A viaduct's piers: slabs as wide as the deck, down to the ground wherever it lies low
/// enough; one that would stand on a way below moves along the bridge off it, or is left out.
fn piers(
    mesh: &mut MeshData,
    path: &Path,
    ground_at: &dyn Fn(f64) -> Option<f64>,
    clear: &dyn Fn(f64, f64) -> bool,
) {
    let mut along = PILLAR_SPACING / 2.0;
    while along < path.length {
        let shifts =
            std::iter::successors(Some(0.0), |s| Some(s + 1.0)).take_while(|&s| s <= PIER_SHIFT);
        let spot = shifts
            .flat_map(|shift| [along + shift, along - shift])
            .filter(|&at| at > 0.0 && at < path.length)
            .find(|&at| clear(at, PIER_HALF_LENGTH + CLEAR_MARGIN));
        if let Some(at) = spot {
            let section = path.at(at);
            let deck = section.centre.elevation - DECK_THICKNESS;
            if let Some(ground) = ground_at(at)
                && deck - ground >= MIN_PILLAR_HEIGHT
            {
                let middle = (section.right - section.left) / 2.0;
                let half = f64::midpoint(section.left, section.right) - PIER_INSET;
                pier(
                    mesh,
                    shifted(section.centre, middle),
                    (PIER_HALF_LENGTH, half),
                    (ground - FOOTING, deck),
                    *CONCRETE,
                );
            }
        }
        along += PILLAR_SPACING;
    }
}

/// A stone arch bridge's arches: piers between arched openings, the walls over the arches up
/// to the deck, and the vaults under them. Where the ground leaves too little room for an arch,
/// a solid wall stands instead.
fn arches(mesh: &mut MeshData, path: &Path, ground_at: &dyn Fn(f64) -> Option<f64>) {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // a few spans
    let spans = (path.length / ARCH_SPAN).round().max(1.0) as usize;
    #[allow(clippy::cast_precision_loss)] // a few spans
    let span = path.length / spans as f64;
    let half_pier = ARCH_PIER / 2.0;
    let deck_at = |along: f64| path.at(along).centre.elevation - DECK_THICKNESS;
    // The lowest ground along a stretch: piers and walls reach down to it.
    let lowest = |from: f64, to: f64| {
        let mut low = f64::MAX;
        let mut along = from;
        while along <= to + 1e-6 {
            if let Some(g) = ground_at(along) {
                low = low.min(g);
            }
            along += GROUND_STEP;
        }
        (low < f64::MAX).then_some(low - FOOTING)
    };
    // Piers at both ends (abutments, half as thick) and between the spans.
    for k in 0..=spans {
        #[allow(clippy::cast_precision_loss)] // a few spans
        let middle = k as f64 * span;
        let (from, to) = (
            (middle - half_pier).max(0.0),
            (middle + half_pier).min(path.length),
        );
        if let Some(bottom) = lowest(from, to) {
            sides(mesh, path, (from, to), &|_| bottom, &deck_at, *STONE);
        }
    }
    for k in 0..spans {
        #[allow(clippy::cast_precision_loss)] // a few spans
        let (from, to) = (
            k as f64 * span + half_pier,
            (k + 1) as f64 * span - half_pier,
        );
        let Some(floor) = lowest(from, to) else {
            continue;
        };
        opening(mesh, path, (from, to), floor, &deck_at);
    }
}

/// One arched opening of a stone bridge along `(from, to)` over ground at `floor`: the faces
/// of the piers either side up to the arch's springing, the walls over the arch up to the
/// deck, and the vault under it. A semicircle, or flatter where the ground lies higher; a solid
/// wall where there is no room for an arch at all.
fn opening(
    mesh: &mut MeshData,
    path: &Path,
    (from, to): (f64, f64),
    floor: f64,
    deck_at: &dyn Fn(f64) -> f64,
) {
    let middle = f64::midpoint(from, to);
    let radius = (to - from) / 2.0;
    let crown = deck_at(middle) - ARCH_CROWN;
    let rise = radius.min(crown - floor - 1.5);
    if rise < 1.0 {
        sides(mesh, path, (from, to), &|_| floor, deck_at, *STONE);
        return;
    }
    let spring = crown - rise;
    let arch = |t: f64| {
        let angle = std::f64::consts::PI * t;
        (middle - radius * angle.cos(), spring + rise * angle.sin())
    };
    for (at, facing) in [(from, 1.0), (to, -1.0)] {
        let section = path.at(at);
        let point = section.centre;
        quad(
            mesh,
            [
                offset(point, -section.left, floor - point.elevation),
                offset(point, section.right, floor - point.elevation),
                offset(point, section.right, spring - point.elevation),
                offset(point, -section.left, spring - point.elevation),
            ],
            forward(point).map(|v| v * facing),
            *STONE,
        );
    }
    for piece in 0..ARCH_PIECES {
        #[allow(clippy::cast_precision_loss)] // a few pieces
        let (t0, t1) = (
            piece as f64 / ARCH_PIECES as f64,
            (piece + 1) as f64 / ARCH_PIECES as f64,
        );
        let ((x0, y0), (x1, y1)) = (arch(t0), arch(t1));
        let (s0, s1) = (path.at(x0), path.at(x1));
        let (p0, p1) = (s0.centre, s1.centre);
        // The walls over the arch, up to the deck.
        for side in [-1.0, 1.0] {
            quad(
                mesh,
                [
                    offset(p0, s0.edge(side), y0 - p0.elevation),
                    offset(p1, s1.edge(side), y1 - p1.elevation),
                    offset(p1, s1.edge(side), -DECK_THICKNESS),
                    offset(p0, s0.edge(side), -DECK_THICKNESS),
                ],
                right(p0).map(|v| v * side),
                *STONE,
            );
        }
        // The vault, facing down into the opening.
        let (xm, ym) = arch(f64::midpoint(t0, t1));
        let into = forward(path.at(xm).centre).map(|v| v * (middle - xm));
        quad(
            mesh,
            [
                offset(p0, -s0.left, y0 - p0.elevation),
                offset(p0, s0.right, y0 - p0.elevation),
                offset(p1, s1.right, y1 - p1.elevation),
                offset(p1, -s1.left, y1 - p1.elevation),
            ],
            [into[0], -(ym - spring).max(0.1), into[2]],
            *STONE,
        );
    }
}

/// The bridge's sides along `(from, to)`, from `bottom(along)` up to `top(along)`, and their
/// ends facing along the bridge: a pier, an abutment or a solid wall.
fn sides(
    mesh: &mut MeshData,
    path: &Path,
    (from, to): (f64, f64),
    bottom: &dyn Fn(f64) -> f64,
    top: &dyn Fn(f64) -> f64,
    color: [f32; 4],
) {
    let pieces = ((to - from) / GROUND_STEP).ceil().max(1.0);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // short stretches
    for piece in 0..pieces as usize {
        #[allow(clippy::cast_precision_loss)] // short stretches
        let (x0, x1) = (
            from + (to - from) * piece as f64 / pieces,
            from + (to - from) * (piece + 1) as f64 / pieces,
        );
        let (s0, s1) = (path.at(x0), path.at(x1));
        let (p0, p1) = (s0.centre, s1.centre);
        for side in [-1.0, 1.0] {
            quad(
                mesh,
                [
                    offset(p0, s0.edge(side), bottom(x0) - p0.elevation),
                    offset(p1, s1.edge(side), bottom(x1) - p1.elevation),
                    offset(p1, s1.edge(side), top(x1) - p1.elevation),
                    offset(p0, s0.edge(side), top(x0) - p0.elevation),
                ],
                right(p0).map(|v| v * side),
                color,
            );
        }
    }
}

/// The structure's line by distance along it.
struct Path<'a> {
    run: &'a [Section],
    /// Distance along at each point.
    at: Vec<f64>,
    length: f64,
}

impl<'a> Path<'a> {
    fn new(run: &'a [Section]) -> Self {
        let mut at = vec![0.0];
        for pair in run.windows(2) {
            let step = distance(pair[0].centre.position, pair[1].centre.position);
            at.push(at[at.len() - 1] + step);
        }
        let length = at.last().copied().unwrap_or(0.0);
        Self { run, at, length }
    }

    /// The line and the structure's reach `along` metres from the start, clamped to it.
    fn at(&self, along: f64) -> Section {
        let along = along.clamp(0.0, self.length);
        let index = self
            .at
            .partition_point(|&d| d <= along)
            .clamp(1, self.run.len().max(2) - 1)
            - 1;
        let (a, b) = (
            self.run[index],
            self.run[(index + 1).min(self.run.len() - 1)],
        );
        let span = (self.at.get(index + 1).copied().unwrap_or(along) - self.at[index]).max(1e-9);
        let t = ((along - self.at[index]) / span).clamp(0.0, 1.0);
        let (ca, cb) = (a.centre, b.centre);
        Section {
            centre: CentrePoint {
                position: (
                    ca.position.0 + (cb.position.0 - ca.position.0) * t,
                    ca.position.1 + (cb.position.1 - ca.position.1) * t,
                ),
                elevation: ca.elevation + (cb.elevation - ca.elevation) * t,
                direction: ca.direction,
            },
            left: a.left + (b.left - a.left) * t,
            right: a.right + (b.right - a.right) * t,
        }
    }
}

/// An arched tube from its left edge to its right one, at most `TUNNEL_HEIGHT` high. A railway
/// tunnel passing under the road ridden (`over`) is cut flat below the ground the road shapes
/// there, so it never shows through the road or its cuttings and embankments (#138).
async fn tunnel<M: ElevationModel>(
    mesh: &mut MeshData,
    run: &[Section],
    over: Option<(&RoadIndex, &LocalProjection, &mut M)>,
) {
    #[allow(clippy::cast_precision_loss)] // small segment counts
    let angle = |k: usize| std::f64::consts::PI * k as f64 / ARCH_SEGMENTS as f64;
    let mut rings: Vec<Vec<[f64; 3]>> = run
        .iter()
        .map(|&s| {
            let half = f64::midpoint(s.left, s.right);
            let middle = (s.right - s.left) / 2.0;
            (0..=ARCH_SEGMENTS)
                .map(|k| {
                    let (sin, cos) = angle(k).sin_cos();
                    offset(s.centre, middle + half * cos, half.min(TUNNEL_HEIGHT) * sin)
                })
                .collect()
        })
        .collect();
    if let Some((road, projection, model)) = over {
        for (section, ring) in run.iter().zip(&mut rings) {
            let floor = section.centre.elevation;
            for point in ring {
                let (east, north) = (point[0], -point[2]);
                if let Some(ground) =
                    ground_over(road, projection, model, (east, north), floor).await
                {
                    point[1] = point[1].min((ground - TUNNEL_COVER).max(floor));
                }
            }
        }
    }
    for (i, pair) in run.windows(2).enumerate() {
        let a = pair[0];
        for k in 0..ARCH_SEGMENTS {
            let corners = [
                rings[i][k],
                rings[i + 1][k],
                rings[i + 1][k + 1],
                rings[i][k + 1],
            ];
            let middle = f64::midpoint(angle(k), angle(k + 1));
            let (sin, cos) = middle.sin_cos();
            let right = right(a.centre);
            // Square to the arch (an ellipse where flattened), facing the axis, seen from inside
            // the tunnel...
            let half = f64::midpoint(a.left, a.right);
            let (across, up) = (cos * half.min(TUNNEL_HEIGHT), sin * half);
            let inward = [-right[0] * across, -up, -right[2] * across];
            quad(mesh, corners, inward, *TUNNEL_WALL);
            // ...and the outside, visible at the portals.
            quad(mesh, corners, inward.map(|v| -v), *CONCRETE);
        }
    }
}

/// The ground at `(east, north)` as the road ridden shapes it, where it does and a railway at
/// `floor` passes under it there: `None` where the road leaves the ground as it is, or the
/// line runs beside the road rather than below it (its portals stay whole).
async fn ground_over<M: ElevationModel>(
    road: &RoadIndex,
    projection: &LocalProjection,
    model: &mut M,
    (east, north): (f64, f64),
    floor: f64,
) -> Option<f64> {
    let pieces = road.near(east, north, LEVEL_REACH);
    let under = pieces.iter().any(|&(_, elevation, surface)| {
        surface != Surface::Tunnel && elevation - floor >= UNDERPASS
    });
    if !under {
        return None;
    }
    let (lat, lon) = projection.unproject(east, north);
    let natural = model.elevation(lat, lon).await.ok()?;
    let ground = shape(natural, &pieces);
    ((ground - natural).abs() > SHAPED).then_some(ground)
}

/// A portal's headwall (#135): a stone face square to the tube at its `end` section, facing out
/// of the tunnel (`outward` 1 along the line, −1 against it), around the tube's arched opening.
/// It rises over the crown and reaches out either side until the ground before it is as high,
/// so the cutting ends against it; a coping caps it.
async fn headwall<M: ElevationModel>(
    mesh: &mut MeshData,
    end: Section,
    outward: f64,
    shapers: &Shapers<'_>,
    projection: &LocalProjection,
    model: &mut M,
) {
    let front = end.centre;
    let (de, dn) = front.direction;
    let moved = |by: f64| CentrePoint {
        position: (
            front.position.0 + de * outward * by,
            front.position.1 + dn * outward * by,
        ),
        ..front
    };
    let back = moved(-HEADWALL_THICKNESS);
    let sideways = right(front);
    let half = f64::midpoint(end.left, end.right);
    let middle = (end.right - end.left) / 2.0;
    let crown = half.min(TUNNEL_HEIGHT);
    let (top, bottom) = (crown + HEADWALL_RISE, -FOOTING);
    let (before, height) = (moved(1.0), front.elevation + top);
    let far_left = wall_end(before, (end.left, -1.0), height, shapers, projection, model).await;
    let far_right = wall_end(before, (end.right, 1.0), height, shapers, projection, model).await;
    #[allow(clippy::cast_precision_loss)] // small segment counts
    let arch = |k: usize| {
        let angle = std::f64::consts::PI * k as f64 / ARCH_SEGMENTS as f64;
        let (sin, cos) = angle.sin_cos();
        (middle + half * cos, crown * sin)
    };
    let facing = forward(front).map(|v| v * outward);
    for (plane, normal) in [(front, facing), (back, facing.map(|v| -v))] {
        // Over the opening...
        for k in 0..ARCH_SEGMENTS {
            let ((x0, y0), (x1, y1)) = (arch(k), arch(k + 1));
            quad(
                mesh,
                [
                    offset(plane, x0, y0),
                    offset(plane, x1, y1),
                    offset(plane, x1, top),
                    offset(plane, x0, top),
                ],
                normal,
                *STONE,
            );
        }
        // ...and beside it, from below the ground.
        for (from, to) in [(-far_left, -end.left), (end.right, far_right)] {
            quad(
                mesh,
                [
                    offset(plane, from, bottom),
                    offset(plane, to, bottom),
                    offset(plane, to, top),
                    offset(plane, from, top),
                ],
                normal,
                *STONE,
            );
        }
    }
    quad(
        mesh,
        [
            offset(front, -far_left, top),
            offset(front, far_right, top),
            offset(back, far_right, top),
            offset(back, -far_left, top),
        ],
        [0.0, 1.0, 0.0],
        *CONCRETE,
    );
    for (across, side) in [(-far_left, -1.0), (far_right, 1.0)] {
        quad(
            mesh,
            [
                offset(front, across, bottom),
                offset(back, across, bottom),
                offset(back, across, top),
                offset(front, across, top),
            ],
            sideways.map(|v| v * side),
            *STONE,
        );
    }
}

/// How far from the line on `side` (−1 left, 1 right) a headwall reaches out past the opening's
/// `edge`: until the ground before it (beside `before`) rises to `height`, by at most
/// `HEADWALL_REACH`.
async fn wall_end<M: ElevationModel>(
    before: CentrePoint,
    (edge, side): (f64, f64),
    height: f64,
    shapers: &Shapers<'_>,
    projection: &LocalProjection,
    model: &mut M,
) -> f64 {
    let mut out = edge + 1.0;
    while out < edge + HEADWALL_REACH {
        let found = ground(aside(before, side * out), shapers, projection, model).await;
        if found.is_some_and(|g| g >= height) {
            break;
        }
        out += 1.0;
    }
    out
}

/// A vertical strip along the line `across` metres right of it at either end, from `low` to
/// `high` relative to the road surface.
fn wall(
    mesh: &mut MeshData,
    (a, b): (CentrePoint, CentrePoint),
    across: (f64, f64),
    (low, high): (f64, f64),
    normal: [f64; 3],
    color: [f32; 4],
) {
    let corners = [
        offset(a, across.0, low),
        offset(b, across.1, low),
        offset(b, across.1, high),
        offset(a, across.0, high),
    ];
    quad(mesh, corners, normal, color);
}

/// A horizontal strip between `left` and `right` metres across (at either end), at `height`
/// above the road, facing up (`facing` 1) or down (−1).
fn flat(
    mesh: &mut MeshData,
    (a, b): (CentrePoint, CentrePoint),
    (left, right): ((f64, f64), (f64, f64)),
    height: f64,
    facing: f64,
    color: [f32; 4],
) {
    let corners = [
        offset(a, left.0, height),
        offset(b, left.1, height),
        offset(b, right.1, height),
        offset(a, right.0, height),
    ];
    quad(mesh, corners, [0.0, facing, 0.0], color);
}

/// A box under the deck at `point`, `half` long and wide (along, across), from `bottom` to `top`.
fn pier(
    mesh: &mut MeshData,
    point: CentrePoint,
    (half_along, half_across): (f64, f64),
    (bottom, top): (f64, f64),
    color: [f32; 4],
) {
    let corner = |along: f64, across: f64, height: f64| {
        let shifted = CentrePoint {
            position: (
                point.position.0 + point.direction.0 * along,
                point.position.1 + point.direction.1 * along,
            ),
            ..point
        };
        offset(shifted, across, height - point.elevation)
    };
    let (l, w) = (half_along, half_across);
    let ahead = forward(point);
    let side = right(point);
    for (corners, normal) in [
        ([(l, -w), (l, w)], ahead),
        ([(-l, w), (-l, -w)], ahead.map(|v| -v)),
        ([(-l, w), (l, w)], side),
        ([(l, -w), (-l, -w)], side.map(|v| -v)),
    ] {
        let [(a0, c0), (a1, c1)] = corners;
        quad(
            mesh,
            [
                corner(a0, c0, bottom),
                corner(a1, c1, bottom),
                corner(a1, c1, top),
                corner(a0, c0, top),
            ],
            normal,
            color,
        );
    }
}

/// The point `across` metres right of `p`, in metres east/north.
fn aside(p: CentrePoint, across: f64) -> (f64, f64) {
    let (de, dn) = p.direction;
    (p.position.0 + dn * across, p.position.1 - de * across)
}

/// `p` moved `across` metres to its right.
fn shifted(p: CentrePoint, across: f64) -> CentrePoint {
    CentrePoint {
        position: aside(p, across),
        ..p
    }
}

fn distance(a: (f64, f64), b: (f64, f64)) -> f64 {
    (b.0 - a.0).hypot(b.1 - a.1)
}

/// Unit vector along the direction of travel, in Godot coordinates.
fn forward(p: CentrePoint) -> [f64; 3] {
    let (de, dn) = p.direction;
    [de, 0.0, -dn]
}

/// Unit vector to the right of travel, in Godot coordinates.
fn right(p: CentrePoint) -> [f64; 3] {
    let (de, dn) = p.direction;
    [dn, 0.0, de]
}

/// A point `across` metres right of the centre line and `up` metres above the road surface,
/// in Godot coordinates.
fn offset(p: CentrePoint, across: f64, up: f64) -> [f64; 3] {
    let (de, dn) = p.direction;
    let (re, rn) = (dn, -de);
    [
        p.position.0 + re * across,
        p.elevation + up,
        -(p.position.1 + rn * across),
    ]
}

/// Adds a quad with corners in order around its edge, wound to face `normal` in Godot's
/// clockwise front-face order.
pub(crate) fn quad(mesh: &mut MeshData, corners: [[f64; 3]; 4], normal: [f64; 3], color: [f32; 4]) {
    quad_uv(mesh, corners, [[0.0; 2]; 4], normal, color);
}

/// Like [`quad`], with a texture coordinate per corner.
pub(crate) fn quad_uv(
    mesh: &mut MeshData,
    corners: [[f64; 3]; 4],
    uvs: [[f32; 2]; 4],
    normal: [f64; 3],
    color: [f32; 4],
) {
    let base = push_facing(mesh, &corners, &uvs, normal, color);
    mesh.indices
        .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
}

/// Adds a triangle wound to face `normal`.
pub(crate) fn triangle_uv(
    mesh: &mut MeshData,
    corners: [[f64; 3]; 3],
    uvs: [[f32; 2]; 3],
    normal: [f64; 3],
    color: [f32; 4],
) {
    let base = push_facing(mesh, &corners, &uvs, normal, color);
    mesh.indices.extend([base, base + 1, base + 2]);
}

/// Pushes a polygon's vertices, reversed if needed so they run clockwise seen from `normal`'s
/// side; returns the index of the first vertex.
#[allow(clippy::cast_possible_truncation)] // geometry is stored as f32 for the GPU
fn push_facing(
    mesh: &mut MeshData,
    corners: &[[f64; 3]],
    uvs: &[[f32; 2]],
    normal: [f64; 3],
    color: [f32; 4],
) -> u32 {
    let (first, second, third) = (corners[0], corners[1], corners[2]);
    let edge_1 = [0, 1, 2].map(|k| second[k] - first[k]);
    let edge_2 = [0, 1, 2].map(|k| third[k] - first[k]);
    let cross = [
        edge_1[1] * edge_2[2] - edge_1[2] * edge_2[1],
        edge_1[2] * edge_2[0] - edge_1[0] * edge_2[2],
        edge_1[0] * edge_2[1] - edge_1[1] * edge_2[0],
    ];
    // Clockwise seen from the front means the right-hand normal points away from the viewer.
    let facing_viewer = cross[0] * normal[0] + cross[1] * normal[1] + cross[2] * normal[2] > 0.0;
    let mut order: Vec<usize> = (0..corners.len()).collect();
    if facing_viewer {
        order[1..].reverse();
    }
    let base = u32::try_from(mesh.vertices.len()).expect("mesh fits u32");
    let length = (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2]).sqrt();
    let unit = normal.map(|n| (n / length) as f32);
    for i in order {
        mesh.vertices.push(corners[i].map(|v| v as f32));
        mesh.normals.push(unit);
        mesh.uvs.push(uvs[i]);
        mesh.colors.push(color);
    }
    base
}
