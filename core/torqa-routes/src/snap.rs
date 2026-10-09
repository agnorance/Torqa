//! Puts a recorded track onto the roads it rides (map matching) and smooths it. GPS positions
//! wander a few metres either side of the road and sparse files cut corners between their
//! points; the route drawn and ridden should follow the road, and above all look like one: no
//! hops onto side streets, sidewalks or parallel roads, no corners cut at junctions, no kinks.
//! Looking natural matters more than lying exactly on the map's lines.
//!
//! The road of every point is chosen for the whole track at once (the cheapest sequence, as in
//! a hidden Markov model): near roads are cheap, roads running across the track, footpaths and
//! lesser roads right beside bigger ones cost extra, and so does changing to another road unless
//! the two meet.

use std::collections::HashMap;

use torqa_osm::{Road, RoadClass, Structure, StructureKind};

use crate::gpx::RawPoint;

/// Roads further than this from a point are not considered for it.
const MAX_SNAP_M: f64 = 20.0;
/// What leaving a point where it was recorded costs, in metres of distance to a road.
const OFF_ROAD_COST: f64 = 14.0;
/// Going on or off road.
const LEAVE_COST: f64 = 4.0;
/// Changing to a road that does not meet the current one: a rider cannot hop to a parallel
/// road or a sidewalk, so only overwhelming evidence makes the track do it.
const HOP_COST: f64 = 60.0;
/// Continuing onto a road that meets the current one (ways of one street, or a turn).
const TURN_COST: f64 = 1.0;
/// Extra cost of a road running across the track rather than along it, at right angles.
const CROSSING_COST: f64 = 14.0;
/// Extra cost of a lesser road (a service road, track or path) running alongside a bigger one
/// within `SIDE_REACH_M`: frontage roads, car park aisles and cycle paths are mapped right
/// beside the road, GPS wanders onto them, and every hop on and off draws an S-bend (#74). Only
/// a track clearly following the lesser road takes it.
const SIDE_ROAD_COST: f64 = 5.0;
const SIDE_REACH_M: f64 = 15.0;
/// Roads within this angle (cosine) of each other run alongside.
const ALONGSIDE: f64 = 0.9;
/// A road detour between two points much longer than the straight line is not what was ridden
/// (e.g. a loop of the road between them): then the points are joined straight.
const MAX_DETOUR: f64 = 2.5;
/// Where the track changes road, it turns at the junction if that is this close to both of
/// its points there (sparse files have points 100 m apart; the detour limit keeps the way
/// sensible).
const JUNCTION_REACH_M: f64 = 150.0;
/// A receiver that loses its fix in a tunnel records points wandering far from any road until
/// it finds the sky again; the tunnel between the last point on a road and the next was ridden,
/// not the wandering (#166). Points off road are taken for that where a tunnel joins the roads
/// either side and they run at most this many times the way through it; a real detour off the
/// map, from one tunnel mouth round to the other, is longer.
const LOST_FIX_DETOUR: f64 = 3.0;
/// A tunnel is followed through at most this many pieces of the map (`tunnel_way`).
const TUNNEL_PIECES: usize = 6;
/// Points of the finished line are at most this far apart...
const STEP_M: f64 = 5.0;
/// ...and it is smoothed so it bends like a road, no point moving further than this from where
/// it was put: corners round off, the route stays on its road.
const MAX_SHIFT_M: f64 = 2.5;
/// Pairs of smoothing passes; each pair shrinks and re-inflates the line (Taubin), which takes
/// out kinks and jitter but keeps long bends as they are.
const SMOOTHING_PAIRS: usize = 8;
/// Short excursions to one side and back are ridden straight. Where a street splits around a
/// traffic island the map draws a one-way branch either side, and the track follows one of
/// them out and back: a kink no rider rides. An excursion is at most this long...
const EXCURSION_MAX_M: f64 = 80.0;
/// ...leaves the line by this much at most (wider ones are real roads)...
const EXCURSION_OFFSET_M: f64 = 8.0;
/// ...leaves and rejoins it in its direction, within this angle (radians)...
const EXCURSION_ALIGNED: f64 = 0.06;
/// ...and turns away by at least this much on the way (gentle bends never do).
const EXCURSION_TURN: f64 = 0.15;
/// Spatial index cell size.
const CELL_M: f64 = 50.0;

const EARTH_RADIUS_M: f64 = 6_371_000.0;

/// Footpaths and tracks are taken only where the track clearly follows them: sidewalks and
/// cycle lanes are often mapped beside streets.
fn class_cost(class: RoadClass) -> f64 {
    match class {
        RoadClass::Path => 5.0,
        RoadClass::Track => 2.0,
        RoadClass::Major | RoadClass::Street | RoadClass::Service => 0.0,
    }
}

/// A track put on the roads it rides.
#[derive(Debug, Default)]
pub(crate) struct Snapped {
    pub(crate) track: Vec<RawPoint>,
    /// The bridges and tunnels among the roads it was put on.
    pub(crate) structures: Vec<Structure>,
}

/// A flat projection around the track, accurate to a fraction of a metre over a route.
struct Flat {
    lat0: f64,
    lon0: f64,
    cos_lat0: f64,
}

impl Flat {
    fn new(lat0: f64, lon0: f64) -> Self {
        Self {
            lat0,
            lon0,
            cos_lat0: lat0.to_radians().cos(),
        }
    }

    fn to_xy(&self, lat: f64, lon: f64) -> (f64, f64) {
        (
            (lon - self.lon0).to_radians() * self.cos_lat0 * EARTH_RADIUS_M,
            (lat - self.lat0).to_radians() * EARTH_RADIUS_M,
        )
    }

    fn to_lat_lon(&self, (x, y): (f64, f64)) -> (f64, f64) {
        (
            self.lat0 + (y / EARTH_RADIUS_M).to_degrees(),
            self.lon0 + (x / (EARTH_RADIUS_M * self.cos_lat0)).to_degrees(),
        )
    }
}

/// Where a point lies on a road: road, segment and position along that segment (0–1).
#[derive(Debug, Clone, Copy)]
struct Match {
    road: usize,
    segment: usize,
    along: f64,
    at: (f64, f64),
}

/// A point of the line being built, with where it was put (smoothing keeps it near there).
#[derive(Debug, Clone, Copy)]
struct Node {
    at: (f64, f64),
    anchor: (f64, f64),
    elevation: Option<f64>,
    time: Option<f64>,
    /// On a road, rather than where a point off road was recorded.
    on_road: bool,
}

/// The track on the roads it rides, smoothed, and the bridges and tunnels it uses.
pub(crate) fn to_roads(track: &[RawPoint], roads: &[Road]) -> Snapped {
    let Some(first) = track.first() else {
        return Snapped::default();
    };
    let flat = Flat::new(first.lat, first.lon);
    let positions: Vec<(f64, f64)> = track.iter().map(|p| flat.to_xy(p.lat, p.lon)).collect();
    let lines: Vec<Vec<(f64, f64)>> = roads
        .iter()
        .map(|r| {
            r.line
                .iter()
                .map(|&(lat, lon)| flat.to_xy(lat, lon))
                .collect()
        })
        .collect();
    let network = Network::new(lines, roads);
    let matches = network.choose(&positions);
    let kept = network.without_lost_fixes(&matches, &positions);
    let mut used: Vec<usize> = matches.iter().flatten().map(|m| m.road).collect();

    let mut nodes: Vec<Node> = Vec::with_capacity(track.len() * 2);
    let mut previous: Option<usize> = None;
    for &i in &kept {
        let point = &track[i];
        let at = matches[i].map_or(positions[i], |m| m.at);
        if let Some(p) = previous
            && let (Some(from), Some(to)) = (matches[p], matches[i])
        {
            network.connect((&mut nodes, &mut used), from, to, (&track[p], point));
        }
        previous = Some(i);
        nodes.push(Node {
            at,
            anchor: at,
            elevation: point.elevation,
            time: point.time,
            on_road: matches[i].is_some(),
        });
    }
    let mut nodes = densify(&nodes);
    straighten_excursions(&mut nodes);
    let nodes = smooth(&nodes);

    used.sort_unstable();
    used.dedup();
    Snapped {
        track: nodes
            .iter()
            .map(|n| {
                let (lat, lon) = flat.to_lat_lon(n.at);
                RawPoint {
                    lat,
                    lon,
                    elevation: n.elevation,
                    time: n.time,
                }
            })
            .collect(),
        structures: used
            .into_iter()
            .filter_map(|r| {
                roads[r].structure.map(|kind| Structure {
                    kind,
                    line: roads[r].line.clone(),
                })
            })
            .collect(),
    }
}

/// Where two roads meet, with the positions there on each.
type Meeting = ((f64, f64), Match, Match);
/// A way along roads as points, with the roads of the tunnel it goes through, if any.
type Way = (Vec<(f64, f64)>, Vec<usize>);

/// A way into a tunnel being followed through (`tunnel_way`): the piece of it reached, the
/// points up to it, where they enter it, and the pieces entered so far.
struct Entry {
    tunnel: usize,
    path: Vec<(f64, f64)>,
    into: Match,
    pieces: Vec<usize>,
}

/// The roads, projected and indexed.
struct Network<'a> {
    lines: Vec<Vec<(f64, f64)>>,
    roads: &'a [Road],
    index: HashMap<(i64, i64), Vec<(usize, usize)>>,
    /// The roads in tunnels (`tunnel_way`).
    tunnels: Vec<usize>,
}

impl<'a> Network<'a> {
    fn new(lines: Vec<Vec<(f64, f64)>>, roads: &'a [Road]) -> Self {
        let mut index: HashMap<(i64, i64), Vec<(usize, usize)>> = HashMap::new();
        for (road, line) in lines.iter().enumerate() {
            for segment in 0..line.len().saturating_sub(1) {
                let (a, b) = (line[segment], line[segment + 1]);
                let (x0, x1) = (cell(a.0.min(b.0)), cell(a.0.max(b.0)));
                let (y0, y1) = (cell(a.1.min(b.1)), cell(a.1.max(b.1)));
                for x in x0..=x1 {
                    for y in y0..=y1 {
                        index.entry((x, y)).or_default().push((road, segment));
                    }
                }
            }
        }
        let tunnels = roads
            .iter()
            .enumerate()
            .filter(|(_, road)| road.structure == Some(StructureKind::Tunnel))
            .map(|(index, _)| index)
            .collect();
        Self {
            lines,
            roads,
            index,
            tunnels,
        }
    }

    /// The closest point of every road within reach of `point`.
    fn candidates(&self, point: (f64, f64)) -> Vec<Match> {
        let (cx, cy) = (cell(point.0), cell(point.1));
        let mut best: HashMap<usize, (f64, Match)> = HashMap::new();
        for x in cx - 1..=cx + 1 {
            for y in cy - 1..=cy + 1 {
                for &(road, segment) in self.index.get(&(x, y)).into_iter().flatten() {
                    let line = &self.lines[road];
                    let (at, along) = project(point, line[segment], line[segment + 1]);
                    let distance = (at.0 - point.0).hypot(at.1 - point.1);
                    if distance > MAX_SNAP_M {
                        continue;
                    }
                    let candidate = Match {
                        road,
                        segment,
                        along,
                        at,
                    };
                    best.entry(road)
                        .and_modify(|b| {
                            if distance < b.0 {
                                *b = (distance, candidate);
                            }
                        })
                        .or_insert((distance, candidate));
                }
            }
        }
        let mut found: Vec<Match> = best.into_values().map(|(_, m)| m).collect();
        // A deterministic order, whatever the hash map did.
        found.sort_by_key(|m| (m.road, m.segment));
        found
    }

    /// The road of every point (or none), the cheapest for the whole track.
    fn choose(&self, positions: &[(f64, f64)]) -> Vec<Option<Match>> {
        let states: Vec<Vec<(Option<Match>, f64)>> = positions
            .iter()
            .enumerate()
            .map(|(i, &point)| {
                let heading = heading(positions, i);
                let candidates: Vec<(Match, (f64, f64))> = self
                    .candidates(point)
                    .into_iter()
                    .map(|m| (m, self.direction(m)))
                    .collect();
                let mut options = vec![(None, OFF_ROAD_COST)];
                for &(m, direction) in &candidates {
                    let across = heading.map_or(0.0, |h| {
                        CROSSING_COST * (1.0 - (h.0 * direction.0 + h.1 * direction.1).abs())
                    });
                    let class = self.roads[m.road].class;
                    let beside_bigger = candidates.iter().any(|&(other, way)| {
                        self.roads[other.road].class < class
                            && distance(other.at, m.at) < SIDE_REACH_M
                            && (way.0 * direction.0 + way.1 * direction.1).abs() > ALONGSIDE
                    });
                    let side = if beside_bigger { SIDE_ROAD_COST } else { 0.0 };
                    let cost = distance(m.at, point) + class_cost(class) + across + side;
                    options.push((Some(m), cost));
                }
                options
            })
            .collect();

        let mut meet_cache: HashMap<(usize, usize), bool> = HashMap::new();
        let mut cost: Vec<f64> = states[0].iter().map(|s| s.1).collect();
        let mut back: Vec<Vec<usize>> = vec![Vec::new()];
        for i in 1..states.len() {
            let mut next_cost = Vec::with_capacity(states[i].len());
            let mut next_back = Vec::with_capacity(states[i].len());
            for (state, emission) in &states[i] {
                let mut best = (0, f64::INFINITY);
                for (p, (previous, _)) in states[i - 1].iter().enumerate() {
                    let total = cost[p] + self.transition(*previous, *state, &mut meet_cache);
                    if total < best.1 {
                        best = (p, total);
                    }
                }
                next_back.push(best.0);
                next_cost.push(best.1 + emission);
            }
            cost = next_cost;
            back.push(next_back);
        }
        let mut chosen = vec![None; states.len()];
        let mut state = cost
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.total_cmp(b.1))
            .map_or(0, |(s, _)| s);
        for i in (0..states.len()).rev() {
            chosen[i] = states[i][state].0;
            if i > 0 {
                state = back[i][state];
            }
        }
        chosen
    }

    fn transition(
        &self,
        from: Option<Match>,
        to: Option<Match>,
        meet_cache: &mut HashMap<(usize, usize), bool>,
    ) -> f64 {
        match (from, to) {
            (Some(a), Some(b)) if a.road == b.road => 0.0,
            (Some(a), Some(b)) => {
                let key = (a.road.min(b.road), a.road.max(b.road));
                let meet = *meet_cache
                    .entry(key)
                    .or_insert_with(|| self.meet(a.road, b.road, None).is_some());
                if meet { TURN_COST } else { HOP_COST }
            }
            (None, None) => 0.0,
            _ => LEAVE_COST,
        }
    }

    /// Where roads `a` and `b` meet, closest to `near` if given: a vertex of one on the other
    /// (OpenStreetMap ways share nodes at junctions; pieces of a way from neighbouring tiles
    /// share theirs). With the positions there on `a` and on `b`.
    fn meet(&self, a: usize, b: usize, near: Option<(f64, f64)>) -> Option<Meeting> {
        let mut best: Option<(f64, Meeting)> = None;
        for (first, second) in [(a, b), (b, a)] {
            let line = &self.lines[first];
            for (vertex, &point) in line.iter().enumerate() {
                let Some(on_second) = self.closest_on(second, point) else {
                    continue;
                };
                if distance(on_second.at, point) > 1.0 {
                    continue;
                }
                let on_first = Match {
                    road: first,
                    segment: vertex.min(line.len().saturating_sub(2)),
                    along: if vertex + 1 == line.len() { 1.0 } else { 0.0 },
                    at: point,
                };
                let found = if first == a {
                    (point, on_first, on_second)
                } else {
                    (point, on_second, on_first)
                };
                let Some(near) = near else {
                    return Some(found);
                };
                let score = distance(point, near);
                if best.as_ref().is_none_or(|(s, _)| score < *s) {
                    best = Some((score, found));
                }
            }
        }
        best.map(|(_, found)| found)
    }

    /// The unit direction of the road's segment at `m`.
    fn direction(&self, m: Match) -> (f64, f64) {
        let line = &self.lines[m.road];
        let (a, b) = (line[m.segment], line[m.segment + 1]);
        let length = distance(a, b).max(1e-9);
        ((b.0 - a.0) / length, (b.1 - a.1) / length)
    }

    fn closest_on(&self, road: usize, point: (f64, f64)) -> Option<Match> {
        let line = &self.lines[road];
        (0..line.len().saturating_sub(1))
            .map(|segment| {
                let (at, along) = project(point, line[segment], line[segment + 1]);
                Match {
                    road,
                    segment,
                    along,
                    at,
                }
            })
            .min_by(|x, y| distance(x.at, point).total_cmp(&distance(y.at, point)))
    }

    /// The points of the track to keep: all but runs of points off every road between two
    /// points on roads that connect, where the road's way is about as long as the run
    /// (`LOST_FIX_DETOUR`): a receiver that lost its fix in a tunnel and found it again beyond
    /// (#166). The road between them is then followed (`connect`).
    fn without_lost_fixes(
        &self,
        matches: &[Option<Match>],
        positions: &[(f64, f64)],
    ) -> Vec<usize> {
        let mut kept = Vec::with_capacity(matches.len());
        let mut i = 0;
        while i < matches.len() {
            if matches[i].is_some() {
                kept.push(i);
                i += 1;
                continue;
            }
            let start = i;
            while i < matches.len() && matches[i].is_none() {
                i += 1;
            }
            if !self.lost_fix(matches, positions, start, i) {
                kept.extend(start..i);
            }
        }
        kept
    }

    /// Whether the points `start..end` off every road are a fix lost between the points on
    /// roads either side of them (see [`Network::without_lost_fixes`]).
    fn lost_fix(
        &self,
        matches: &[Option<Match>],
        positions: &[(f64, f64)],
        start: usize,
        end: usize,
    ) -> bool {
        if start == 0 || end >= matches.len() {
            return false;
        }
        let (Some(from), Some(to)) = (matches[start - 1], matches[end]) else {
            return false;
        };
        let Some((way, tunnel)) = self.way(from, to) else {
            return false;
        };
        // Only a tunnel takes the sky away; elsewhere the points off road were ridden.
        let in_tunnel = |road: usize| self.roads[road].structure == Some(StructureKind::Tunnel);
        if tunnel.is_empty() && !in_tunnel(from.road) && !in_tunnel(to.road) {
            return false;
        }
        let recorded = path_length(&positions[start - 1..=end]);
        recorded <= path_length(&way) * LOST_FIX_DETOUR
    }

    /// The way along roads from `from` to `to`: along their road's bends, round the junction
    /// where the track changes road, through a tunnel joining their roads (`tunnel_way`, with
    /// its roads), or straight where none of that holds. `None` where the way would be a
    /// detour (`MAX_DETOUR`): a loop of the road between the two is not what was ridden.
    fn way(&self, from: Match, to: Match) -> Option<Way> {
        let mut path = vec![from.at];
        let mut tunnel = Vec::new();
        if from.road == to.road {
            path.extend(self.bends(from, to));
        } else if let Some((junction, on_from, on_to)) = self.meet(from.road, to.road, Some(to.at))
            && distance(junction, from.at) <= JUNCTION_REACH_M
            && distance(junction, to.at) <= JUNCTION_REACH_M
        {
            path.extend(self.bends(from, on_from));
            path.push(junction);
            path.extend(self.bends(on_to, to));
        } else if let Some((through, pieces)) = self.tunnel_way(from, to) {
            path.extend(through);
            tunnel = pieces;
        }
        path.push(to.at);
        (path_length(&path) <= distance(from.at, to.at).max(1.0) * MAX_DETOUR)
            .then_some((path, tunnel))
    }

    /// The way from `from` to `to` through a tunnel joining their roads: the points between
    /// the two, and the tunnel's roads. The map splits a way where it enters a tunnel, and at
    /// tile borders, so the tunnel may be several roads meeting end to end (`TUNNEL_PIECES` at
    /// most); the shortest way where several lead through.
    fn tunnel_way(&self, from: Match, to: Match) -> Option<Way> {
        let mut best: Option<(f64, Way)> = None;
        // Ways into the tunnels so far: the tunnel, the points up to it and where they enter it.
        let mut frontier: Vec<Entry> = Vec::new();
        for &tunnel in &self.tunnels {
            if tunnel == from.road || tunnel == to.road {
                continue;
            }
            if let Some((entrance, on_from, into)) = self.meet(from.road, tunnel, Some(from.at)) {
                let mut path = self.bends(from, on_from);
                path.push(entrance);
                frontier.push(Entry {
                    tunnel,
                    path,
                    into,
                    pieces: vec![tunnel],
                });
            }
        }
        let mut visited: Vec<usize> = frontier.iter().map(|e| e.tunnel).collect();
        for _ in 0..TUNNEL_PIECES {
            let mut next = Vec::new();
            for entry in frontier {
                if let Some((exit, out_of, on_to)) = self.meet(entry.tunnel, to.road, Some(to.at)) {
                    let mut through = entry.path.clone();
                    through.extend(self.bends(entry.into, out_of));
                    through.push(exit);
                    through.extend(self.bends(on_to, to));
                    let length = path_length(&through);
                    if best.as_ref().is_none_or(|b| length < b.0) {
                        best = Some((length, (through, entry.pieces.clone())));
                    }
                    continue;
                }
                for &other in &self.tunnels {
                    if visited.contains(&other) || other == to.road {
                        continue;
                    }
                    if let Some((joint, out_of, into)) = self.meet(entry.tunnel, other, Some(to.at))
                    {
                        let mut path = entry.path.clone();
                        path.extend(self.bends(entry.into, out_of));
                        path.push(joint);
                        visited.push(other);
                        let mut pieces = entry.pieces.clone();
                        pieces.push(other);
                        next.push(Entry {
                            tunnel: other,
                            path,
                            into,
                            pieces,
                        });
                    }
                }
            }
            if next.is_empty() {
                break;
            }
            frontier = next;
        }
        best.map(|(_, way)| way)
    }

    /// The way from `from` to `to` (the positions on roads of two consecutive points `ends`)
    /// as [`Network::way`] gives it, as points of the line; a tunnel taken joins the roads
    /// `used`, so its tunnel comes with it.
    fn connect(
        &self,
        (nodes, used): (&mut Vec<Node>, &mut Vec<usize>),
        from: Match,
        to: Match,
        ends: (&RawPoint, &RawPoint),
    ) {
        let Some((path, tunnel)) = self.way(from, to) else {
            return;
        };
        used.extend(tunnel);
        let lengths: Vec<f64> = path.windows(2).map(|w| distance(w[0], w[1])).collect();
        let total: f64 = lengths.iter().sum();
        let mut done = 0.0;
        for (point, length) in path[1..path.len() - 1].iter().zip(&lengths) {
            done += length;
            if nodes.last().is_some_and(|n| distance(n.at, *point) < 0.05) {
                continue;
            }
            let share = if total > 0.0 { done / total } else { 0.0 };
            nodes.push(Node {
                at: *point,
                anchor: *point,
                elevation: lerp_option(ends.0.elevation, ends.1.elevation, share),
                time: lerp_option(ends.0.time, ends.1.time, share),
                on_road: true,
            });
        }
    }

    /// A road's vertices strictly between two positions on it, in the direction of travel.
    fn bends(&self, from: Match, to: Match) -> Vec<(f64, f64)> {
        let line = &self.lines[from.road];
        if (to.segment, to.along) >= (from.segment, from.along) {
            (from.segment + 1..=to.segment).map(|v| line[v]).collect()
        } else {
            (to.segment + 1..=from.segment)
                .rev()
                .map(|v| line[v])
                .collect()
        }
    }
}

fn path_length(path: &[(f64, f64)]) -> f64 {
    path.windows(2).map(|w| distance(w[0], w[1])).sum()
}

/// Direction of travel at point `i` (unit), from its neighbours; `None` where they coincide.
fn heading(positions: &[(f64, f64)], i: usize) -> Option<(f64, f64)> {
    let before = positions[i.saturating_sub(1)];
    let after = positions[(i + 1).min(positions.len() - 1)];
    let (dx, dy) = (after.0 - before.0, after.1 - before.1);
    let length = dx.hypot(dy);
    (length > 0.5).then(|| (dx / length, dy / length))
}

/// The line with points inserted so that, along roads, none are more than `STEP_M` apart.
/// Between points off road the line is not the road ridden (sparse files cut corners), so
/// nothing is added there: elevations along it are interpolated rather than taken from the
/// terrain.
fn densify(nodes: &[Node]) -> Vec<Node> {
    let mut out = Vec::with_capacity(nodes.len() * 2);
    for pair in nodes.windows(2) {
        let (from, to) = (pair[0], pair[1]);
        out.push(from);
        if !(from.on_road && to.on_road) {
            continue;
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // short segments
        let pieces = (distance(from.at, to.at) / STEP_M).ceil() as usize;
        for piece in 1..pieces {
            #[allow(clippy::cast_precision_loss)] // few pieces
            let share = piece as f64 / pieces as f64;
            out.push(Node {
                at: lerp(from.at, to.at, share),
                anchor: lerp(from.anchor, to.anchor, share),
                elevation: lerp_option(from.elevation, to.elevation, share),
                time: from
                    .time
                    .zip(to.time)
                    .map(|(start, end)| start + (end - start) * share),
                on_road: true,
            });
        }
    }
    out.extend(nodes.last());
    out
}

/// Puts short excursions to one side and back (see `EXCURSION_MAX_M`) onto the line they leave
/// and rejoin.
fn straighten_excursions(nodes: &mut [Node]) {
    // Points this far before and after an excursion give the line's direction there.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // small constants
    let (lead, reach) = (
        (15.0 / STEP_M).ceil() as usize,
        (EXCURSION_MAX_M / STEP_M).ceil() as usize,
    );
    let mut start = lead;
    while start + lead < nodes.len() {
        let last = (start + reach).min(nodes.len() - 1 - lead);
        // The longest excursion from here, so it is taken out as a whole.
        let end = (start + 2..=last)
            .rev()
            .find(|&end| is_excursion(nodes, start, end, lead));
        let Some(end) = end else {
            start += 1;
            continue;
        };
        let (a, b) = (nodes[start].at, nodes[end].at);
        for node in &mut nodes[start + 1..end] {
            let (on_line, _) = project(node.at, a, b);
            node.at = on_line;
            node.anchor = on_line;
        }
        start = end;
    }
}

/// Whether the line leaves the chord from point `start` to point `end` to one side and comes
/// back to it, as an excursion does.
fn is_excursion(nodes: &[Node], start: usize, end: usize, lead: usize) -> bool {
    let direction = |a: (f64, f64), b: (f64, f64)| (b.0 - a.0).atan2(b.1 - a.1);
    let apart = |x: f64, y: f64| {
        (x - y + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU) - std::f64::consts::PI
    };
    let (a, b) = (nodes[start].at, nodes[end].at);
    let length = distance(a, b);
    if !(15.0..=EXCURSION_MAX_M).contains(&length) {
        return false;
    }
    let chord = direction(a, b);
    let before = direction(nodes[start - lead].at, a);
    let after = direction(b, nodes[end + lead].at);
    if apart(before, chord).abs() > EXCURSION_ALIGNED
        || apart(after, chord).abs() > EXCURSION_ALIGNED
    {
        return false;
    }
    let inside = &nodes[start + 1..end];
    // Off-road stretches keep the course they were recorded on.
    if inside.iter().any(|n| !n.on_road) {
        return false;
    }
    let (ux, uy) = ((b.0 - a.0) / length, (b.1 - a.1) / length);
    let offsets: Vec<f64> = inside
        .iter()
        .map(|n| (n.at.0 - a.0) * uy - (n.at.1 - a.1) * ux)
        .collect();
    let widest = offsets.iter().fold(0.0_f64, |m, o| m.max(o.abs()));
    let one_side = offsets.iter().all(|&o| o >= -0.2) || offsets.iter().all(|&o| o <= 0.2);
    let turned = nodes[start..end]
        .windows(2)
        .map(|w| apart(direction(w[0].at, w[1].at), chord).abs())
        .fold(0.0, f64::max);
    one_side && (1.0..=EXCURSION_OFFSET_M).contains(&widest) && turned >= EXCURSION_TURN
}

/// Rounds corners and takes out jitter, keeping every point within `MAX_SHIFT_M` of where it
/// was put; the ends stay.
fn smooth(nodes: &[Node]) -> Vec<Node> {
    let mut out = nodes.to_vec();
    for pass in 0..2 * SMOOTHING_PAIRS {
        // Taubin's factors: shrink, then inflate a little more.
        let factor = if pass % 2 == 0 { 0.5 } else { -0.53 };
        let previous: Vec<(f64, f64)> = out.iter().map(|n| n.at).collect();
        for i in 1..out.len().saturating_sub(1) {
            let middle = lerp(previous[i - 1], previous[i + 1], 0.5);
            let moved = lerp(previous[i], middle, factor);
            let anchor = out[i].anchor;
            let (dx, dy) = (moved.0 - anchor.0, moved.1 - anchor.1);
            let shift = dx.hypot(dy);
            out[i].at = if shift > MAX_SHIFT_M {
                let scale = MAX_SHIFT_M / shift;
                (anchor.0 + dx * scale, anchor.1 + dy * scale)
            } else {
                moved
            };
        }
    }
    out
}

fn lerp(a: (f64, f64), b: (f64, f64), t: f64) -> (f64, f64) {
    (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
}

fn lerp_option(a: Option<f64>, b: Option<f64>, share: f64) -> Option<f64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a + (b - a) * share),
        (a, b) => a.or(b),
    }
}

fn distance(a: (f64, f64), b: (f64, f64)) -> f64 {
    (b.0 - a.0).hypot(b.1 - a.1)
}

#[allow(clippy::cast_possible_truncation)] // local metres over a route stay far below 2^63 cells
fn cell(metres: f64) -> i64 {
    (metres / CELL_M).floor() as i64
}

/// The point on segment `a`–`b` closest to `p`, and how far along the segment it is.
fn project(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> ((f64, f64), f64) {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let length_squared = dx * dx + dy * dy;
    let t = if length_squared > 0.0 {
        (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / length_squared).clamp(0.0, 1.0)
    } else {
        0.0
    };
    ((a.0 + dx * t, a.1 + dy * t), t)
}

#[cfg(test)]
mod tests {
    use torqa_osm::StructureKind;

    use super::*;

    const ORIGIN: (f64, f64) = (46.0, 7.0);

    fn flat() -> Flat {
        Flat::new(ORIGIN.0, ORIGIN.1)
    }

    fn point(x: f64, y: f64) -> RawPoint {
        let (lat, lon) = flat().to_lat_lon((x, y));
        RawPoint {
            lat,
            lon,
            elevation: Some(500.0),
            time: None,
        }
    }

    fn point2((x, y): (f64, f64)) -> RawPoint {
        point(x, y)
    }

    fn road(points: &[(f64, f64)]) -> Road {
        road_of(RoadClass::Street, points)
    }

    fn tunnel(points: &[(f64, f64)]) -> Road {
        Road {
            structure: Some(StructureKind::Tunnel),
            ..road(points)
        }
    }

    fn road_of(class: RoadClass, points: &[(f64, f64)]) -> Road {
        Road {
            class,
            line: points.iter().map(|&p| flat().to_lat_lon(p)).collect(),
            structure: None,
        }
    }

    fn xy(track: &[RawPoint]) -> Vec<(f64, f64)> {
        track.iter().map(|p| flat().to_xy(p.lat, p.lon)).collect()
    }

    fn length(points: &[(f64, f64)]) -> f64 {
        points.windows(2).map(|w| distance(w[0], w[1])).sum()
    }

    /// The largest change of direction between consecutive pieces of the line, in degrees.
    fn sharpest_turn(points: &[(f64, f64)]) -> f64 {
        points
            .windows(3)
            .map(|w| {
                let a = (w[1].0 - w[0].0).atan2(w[1].1 - w[0].1);
                let b = (w[2].0 - w[1].0).atan2(w[2].1 - w[1].1);
                let turn = (b - a).to_degrees().abs();
                if turn > 180.0 { 360.0 - turn } else { turn }
            })
            .fold(0.0, f64::max)
    }

    /// Distance from `p` to the polyline `line`.
    fn off(line: &[(f64, f64)], p: (f64, f64)) -> f64 {
        line.windows(2)
            .map(|w| distance(project(p, w[0], w[1]).0, p))
            .fold(f64::INFINITY, f64::min)
    }

    #[test]
    fn gps_wandering_beside_the_road_is_put_on_it() {
        let straight = road(&[(0.0, -10.0), (0.0, 510.0)]);
        // ±6 m either side, every 20 m.
        let track: Vec<RawPoint> = (0..=25)
            .map(|i| point(if i % 2 == 0 { 6.0 } else { -6.0 }, f64::from(i) * 20.0))
            .collect();

        let snapped = xy(&to_roads(&track, &[straight]).track);

        assert!(snapped.iter().all(|(x, _)| x.abs() < 0.01), "{snapped:?}");
        assert!((length(&snapped) - 500.0).abs() < 0.1);
    }

    #[test]
    fn sparse_points_follow_the_roads_bends_between_them() {
        // A half circle of 100 m radius drawn every 5°, ridden with a point every 60°.
        let arc = |degrees: f64| {
            let r = degrees.to_radians();
            (100.0 * r.cos(), 100.0 * r.sin())
        };
        let bend = road(
            &(0..=36)
                .map(|i| arc(f64::from(i) * 5.0))
                .collect::<Vec<_>>(),
        );
        let track: Vec<RawPoint> = (0..=3)
            .map(|i| {
                let (x, y) = arc(f64::from(i) * 60.0);
                point(x, y)
            })
            .collect();

        let snapped = xy(&to_roads(&track, &[bend]).track);

        // Along the curve (π × 100 m), not its chords (300 m), every point on it.
        assert!((length(&snapped) - std::f64::consts::PI * 100.0).abs() < 1.5);
        for (x, y) in &snapped {
            assert!((x.hypot(*y) - 100.0).abs() < 0.5, "off the road: {x}, {y}");
        }
    }

    #[test]
    fn a_fix_lost_in_a_tunnel_follows_the_road_through_it() {
        // #166: a road bending through a hill, a quarter circle of 300 m radius, in a tunnel
        // from 10° to 80° (the map splits the way at the tunnel's mouths); the receiver loses
        // its fix inside and records three points wandering 150 m off until it is out again.
        let arc = |degrees: f64| {
            let r = degrees.to_radians();
            (300.0 * r.cos(), 300.0 * r.sin())
        };
        let every_5 = |from: i32, to: i32| -> Vec<(f64, f64)> {
            (from..=to).map(|i| arc(f64::from(i) * 5.0)).collect()
        };
        // The tunnel in two pieces, as a tile border cuts it.
        let roads = [
            road(&every_5(0, 2)),
            tunnel(&every_5(2, 9)),
            tunnel(&every_5(9, 16)),
            road(&every_5(16, 18)),
        ];
        let mut track: Vec<RawPoint> = (0..=2).map(|i| point2(arc(f64::from(i) * 5.0))).collect();
        for degrees in [25.0, 45.0, 65.0] {
            let (x, y) = arc(degrees);
            let r = degrees.to_radians();
            track.push(point(x + 150.0 * r.cos(), y + 150.0 * r.sin()));
        }
        track.extend((16..=18).map(|i| point2(arc(f64::from(i) * 5.0))));

        let snapped = to_roads(&track, &roads);
        let line = xy(&snapped.track);

        // The wandering is gone, the line follows the road's bend through the hill...
        let furthest = line.iter().map(|(x, y)| x.hypot(*y)).fold(0.0, f64::max);
        assert!(furthest < 310.0, "the line wanders {furthest} m out");
        for (x, y) in &line {
            assert!((x.hypot(*y) - 300.0).abs() < 2.5, "off the road: {x}, {y}");
        }
        // ...as long as the road, not the chords between the wandering points...
        assert!((length(&line) - std::f64::consts::FRAC_PI_2 * 300.0).abs() < 5.0);
        // ...and the tunnel ridden comes with it, both pieces.
        assert_eq!(snapped.structures.len(), 2, "{:?}", snapped.structures);
    }

    #[test]
    fn a_real_detour_off_the_map_is_kept() {
        // Leaving the road at a tunnel's mouth for a loop off the map four times as long as
        // the tunnel, back to its other mouth: ridden, not a fix lost.
        let roads = [
            road(&[(0.0, -100.0), (0.0, 0.0)]),
            tunnel(&[(0.0, 0.0), (0.0, 400.0)]),
            road(&[(0.0, 400.0), (0.0, 500.0)]),
        ];
        let mut track: Vec<RawPoint> = vec![point(0.0, -50.0), point(0.0, 0.0)];
        for i in 1..=8 {
            let r = f64::from(i) * std::f64::consts::PI / 9.0;
            track.push(point(500.0 * r.sin(), 200.0 - 500.0 * r.cos()));
        }
        track.extend([point(0.0, 400.0), point(0.0, 450.0)]);

        let snapped = xy(&to_roads(&track, &roads).track);

        assert!(
            snapped.iter().any(|(x, _)| *x > 400.0),
            "the detour was cut: {snapped:?}"
        );
    }

    #[test]
    fn off_road_stretches_keep_their_course() {
        let far_road = road(&[(200.0, 0.0), (200.0, 500.0)]);
        let track: Vec<RawPoint> = (0..=10).map(|i| point(0.0, f64::from(i) * 50.0)).collect();

        let snapped = xy(&to_roads(&track, &[far_road]).track);

        assert!(snapped.iter().all(|(x, _)| x.abs() < 0.01));
        assert!((length(&snapped) - 500.0).abs() < 0.1);
    }

    #[test]
    fn drifting_towards_a_parallel_road_does_not_hop_onto_it() {
        let ridden = road(&[(0.0, -10.0), (0.0, 310.0)]);
        let parallel = road(&[(10.0, -10.0), (10.0, 310.0)]);
        // Closer to the ridden road at first, then a little closer to the other.
        let track: Vec<RawPoint> = (0..=15)
            .map(|i| point(if i < 5 { 3.0 } else { 5.8 }, f64::from(i) * 20.0))
            .collect();

        let snapped = xy(&to_roads(&track, &[ridden, parallel]).track);

        assert!(snapped.iter().all(|(x, _)| x.abs() < 0.01), "{snapped:?}");
    }

    #[test]
    fn a_sidewalk_beside_the_street_does_not_pull_the_route_over() {
        let street = road(&[(0.0, -10.0), (0.0, 410.0)]);
        let sidewalk = road_of(RoadClass::Path, &[(5.0, -10.0), (5.0, 410.0)]);
        // GPS mostly between them, sometimes right on the sidewalk.
        let track: Vec<RawPoint> = (0..=20)
            .map(|i| point(if i % 3 == 0 { 5.0 } else { 2.8 }, f64::from(i) * 20.0))
            .collect();

        let snapped = xy(&to_roads(&track, &[street, sidewalk]).track);

        assert!(snapped.iter().all(|(x, _)| x.abs() < 0.01), "{snapped:?}");
    }

    /// A main road north, and a service road branching off it at 100 m that runs 7.5 m beside
    /// it and joins it again at 320 m, as frontage roads and car park aisles are mapped.
    fn main_road_and_frontage() -> [Road; 2] {
        [
            road_of(RoadClass::Major, &[(0.0, -10.0), (0.0, 510.0)]),
            road_of(
                RoadClass::Service,
                &[(0.0, 100.0), (7.5, 120.0), (7.5, 300.0), (0.0, 320.0)],
            ),
        ]
    }

    #[test]
    fn a_service_road_alongside_does_not_pull_the_route_over() {
        // GPS between them along the service road, a little nearer to it (#74).
        let track: Vec<RawPoint> = (0..=50)
            .map(|i| {
                let y = f64::from(i) * 10.0;
                point(
                    if (130.0..=290.0).contains(&y) {
                        4.0
                    } else {
                        1.0
                    },
                    y,
                )
            })
            .collect();

        let snapped = xy(&to_roads(&track, &main_road_and_frontage()).track);

        assert!(snapped.iter().all(|(x, _)| x.abs() < 0.01), "{snapped:?}");
    }

    #[test]
    fn a_service_road_clearly_ridden_is_kept() {
        // GPS right on the service road along it.
        let track: Vec<RawPoint> = (0..=50)
            .map(|i| {
                let y = f64::from(i) * 10.0;
                point(
                    if (130.0..=290.0).contains(&y) {
                        7.2
                    } else {
                        0.5
                    },
                    y,
                )
            })
            .collect();

        let snapped = xy(&to_roads(&track, &main_road_and_frontage()).track);

        for &(x, y) in snapped.iter().filter(|(_, y)| (150.0..=270.0).contains(y)) {
            assert!((x - 7.5).abs() < 0.5, "{x} m east at {y} m");
        }
    }

    #[test]
    fn side_streets_passed_at_junctions_are_not_taken() {
        // A street north with side streets branching east and west at 200 m.
        let main = road(&[(0.0, -10.0), (0.0, 200.0), (0.0, 410.0)]);
        let east = road(&[(0.0, 200.0), (300.0, 200.0)]);
        let west = road(&[(0.0, 200.0), (-300.0, 200.0)]);
        // The track passes the junction drifting east, one point right on the side street.
        let track: Vec<RawPoint> = [
            (1.0, 0.0),
            (2.0, 100.0),
            (4.0, 190.0),
            (7.0, 200.0),
            (4.0, 210.0),
            (1.0, 300.0),
            (0.0, 400.0),
        ]
        .iter()
        .map(|&(x, y)| point(x, y))
        .collect();

        let snapped = xy(&to_roads(&track, &[main, east, west]).track);

        assert!(snapped.iter().all(|(x, _)| x.abs() < 0.01), "{snapped:?}");
    }

    #[test]
    fn turns_at_junctions_go_round_the_corner_not_across_it() {
        // North, then east at a junction 200 m along; one point each side of the corner.
        let north = road(&[(0.0, -10.0), (0.0, 200.0)]);
        let east = road(&[(0.0, 200.0), (300.0, 200.0)]);
        let track: Vec<RawPoint> = [(0.0, 0.0), (0.0, 120.0), (90.0, 200.0), (250.0, 200.0)]
            .iter()
            .map(|&(x, y)| point(x, y))
            .collect();
        let streets = [(0.0, -10.0), (0.0, 200.0), (300.0, 200.0)];

        let snapped = xy(&to_roads(&track, &[north, east]).track);

        // Round the corner, cutting it a little but never across the block, and turning over
        // several points rather than at one.
        let corner = snapped
            .iter()
            .map(|&p| distance(p, (0.0, 200.0)))
            .fold(f64::INFINITY, f64::min);
        assert!((0.3..4.0).contains(&corner), "{corner}");
        for &p in &snapped {
            assert!(off(&streets, p) <= MAX_SHIFT_M + 1e-6, "{p:?}");
        }
        assert!(
            sharpest_turn(&snapped) < 65.0,
            "{}",
            sharpest_turn(&snapped)
        );
    }

    #[test]
    fn excursions_round_traffic_islands_are_ridden_straight() {
        // A street north splitting round a traffic island 60 m long, its one-way branches 5 m
        // either side (as the map draws them), ridden straight through.
        let roads = [
            road(&[(0.0, 0.0), (0.0, 200.0)]),
            road(&[(0.0, 200.0), (5.0, 215.0), (5.0, 245.0), (0.0, 260.0)]),
            road(&[(0.0, 200.0), (-5.0, 215.0), (-5.0, 245.0), (0.0, 260.0)]),
            road(&[(0.0, 260.0), (0.0, 500.0)]),
        ];
        let track: Vec<RawPoint> = (0..=50).map(|i| point(0.5, f64::from(i) * 10.0)).collect();

        let snapped = xy(&to_roads(&track, &roads).track);

        for &(x, y) in &snapped {
            assert!(x.abs() < 0.5, "{x} m aside at {y} m");
        }
        assert!(sharpest_turn(&snapped) < 3.0, "{}", sharpest_turn(&snapped));
    }

    #[test]
    fn a_road_stepping_aside_keeps_its_course() {
        // A real dog-leg: the road moves 20 m east and goes on there; not an excursion.
        let dogleg = [(0.0, 0.0), (0.0, 200.0), (20.0, 240.0), (20.0, 500.0)];
        let track: Vec<RawPoint> = (0..=50)
            .map(|i| {
                let y = f64::from(i) * 10.0;
                point(
                    if y < 200.0 {
                        0.5
                    } else if y < 240.0 {
                        (y - 200.0) / 2.0
                    } else {
                        20.5
                    },
                    y,
                )
            })
            .collect();

        let snapped = xy(&to_roads(&track, &[road(&dogleg)]).track);

        for &p in &snapped {
            assert!(off(&dogleg, p) <= MAX_SHIFT_M + 1e-6, "{p:?}");
        }
    }

    #[test]
    fn the_route_bends_smoothly_without_kinks() {
        // A road of straight pieces with sharp corners, ridden densely beside it.
        let zigzag = [
            (0.0, 0.0),
            (0.0, 100.0),
            (40.0, 140.0),
            (40.0, 260.0),
            (0.0, 300.0),
        ];
        let track: Vec<RawPoint> = (0..=60)
            .map(|i| {
                let t = f64::from(i) / 60.0 * 300.0;
                let x = if t < 100.0 {
                    0.0
                } else if t < 140.0 {
                    t - 100.0
                } else if t < 260.0 {
                    40.0
                } else {
                    300.0 - t
                };
                point(x + 1.5, t)
            })
            .collect();

        let snapped = xy(&to_roads(&track, &[road(&zigzag)]).track);

        assert!(
            sharpest_turn(&snapped) < 30.0,
            "{}",
            sharpest_turn(&snapped)
        );
        assert!(
            snapped
                .windows(2)
                .all(|w| distance(w[0], w[1]) <= STEP_M * 1.5)
        );
        for &p in &snapped {
            assert!(off(&zigzag, p) <= MAX_SHIFT_M + 1e-6, "{p:?}");
        }
    }

    #[test]
    fn only_the_roads_ridden_bring_their_bridges_and_tunnels() {
        let ridden = Road {
            structure: Some(StructureKind::Bridge),
            ..road(&[(0.0, -10.0), (0.0, 310.0)])
        };
        // A tunnel of another road running alongside, 15 m away.
        let other = Road {
            structure: Some(StructureKind::Tunnel),
            ..road(&[(15.0, -10.0), (15.0, 310.0)])
        };
        let track: Vec<RawPoint> = (0..=15).map(|i| point(1.0, f64::from(i) * 20.0)).collect();

        let snapped = to_roads(&track, &[ridden, other]);

        assert_eq!(snapped.structures.len(), 1);
        assert_eq!(snapped.structures[0].kind, StructureKind::Bridge);
    }
}
