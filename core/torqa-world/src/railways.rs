//! The railways of the map around the route (#75, #85). A railway runs on a smooth line of
//! its own, not over every bump of the ground: each line is sampled along the terrain, smoothed
//! and held to a railway's gentle grades; tunnels and bridges run straight between their ends;
//! where the hill rises far above the track it goes into a tunnel, where the ground falls far
//! below it onto a viaduct. The lines then shape the ground like the road ridden does — level
//! across the bed, cuttings into hillsides, embankments down to valleys — carry bridges and
//! tunnels like it (`structures`), and are drawn as a bed of ballast with sleepers and rails
//! (the app's rail shader) at their own height. Plants keep off the tracks
//! (`streets::Clearance`).

use torqa_osm::{MapData, StructureKind};
use torqa_routes::{ElevationModel, LocalProjection, Surface};

use crate::road::{Centre, RoadIndex};
use crate::{CORRIDOR, ROAD_HALF_WIDTH, drape};

/// Width of a track's bed of ballast.
pub(crate) const BED_M: f64 = 3.2;
/// Points of a line this far apart.
const STEP_M: f64 = 5.0;
/// The terrain is smoothed over this far either side of each point...
const SMOOTHING_M: f64 = 80.0;
/// ...and the line held to this grade (rise per metre), funiculars excepted.
const MAX_GRADE: f64 = 0.04;
/// Where the hill rises this far above the track it runs in a tunnel...
const TUNNEL_COVER: f64 = 8.0;
/// ...where the ground lies this far below it, on a viaduct...
const VIADUCT_HEIGHT: f64 = 12.0;
/// ...for this long at least.
const SHORTEST_STRUCTURE: f64 = 25.0;
/// Over the road ridden a railway bridge clears it by this much, under a road bridge the track
/// lies this far below the road; at a level crossing it lies just under the road's surface.
const OVER_ROAD: f64 = 6.0;
const UNDER_ROAD: f64 = 7.0;
const AT_ROAD: f64 = 0.03;
/// A track this close alongside one laid out before runs with it — at its height, on its
/// bridges and in its tunnels — as parallel tracks do (#99).
const PARALLEL_M: f64 = 7.0;

/// A railway near the route: its points a few metres apart, to keep plants off it.
pub(crate) struct Railway {
    pub(crate) points: Vec<(f64, f64)>,
}

/// The railways near the route: as centre lines to shape the ground and carry structures, and
/// as lines of points.
pub(crate) struct Network {
    pub(crate) index: RoadIndex,
    pub(crate) railways: Vec<Railway>,
}

/// A piece of a railway as the map has it: its line and what carries it.
type Piece = (Vec<(f64, f64)>, Surface);

/// A point of a railway being laid out.
#[derive(Debug, Clone, Copy)]
struct Point {
    position: (f64, f64),
    surface: Surface,
    /// The terrain's height here, if known.
    terrain: Option<f64>,
}

/// The map's railways near the route, laid out (see the module).
pub(crate) async fn network<M: ElevationModel>(
    map: &MapData,
    projection: &LocalProjection,
    road: &RoadIndex,
    model: &mut M,
) -> Network {
    let mut lines = Vec::new();
    let mut railways = Vec::new();
    for (pieces, funicular) in chains(map, projection, road) {
        let mut points = Vec::new();
        for (line, surface) in pieces {
            let dense = drape::densify(&line, STEP_M);
            // Joined pieces share their end points.
            let skip = usize::from(!points.is_empty());
            points.extend(dense.into_iter().skip(skip).map(|position| Point {
                position,
                surface,
                terrain: None,
            }));
        }
        if points.len() < 2 {
            continue;
        }
        for point in &mut points {
            let (lat, lon) = projection.unproject(point.position.0, point.position.1);
            point.terrain = model.elevation(lat, lon).await.ok();
        }
        let laid = RoadIndex::from_lines(&lines);
        let Some(line) = lay_out(&points, funicular, road, &laid) else {
            continue;
        };
        railways.push(Railway {
            points: line.iter().map(|c| c.position).collect(),
        });
        lines.push(line);
    }
    Network {
        index: RoadIndex::from_lines(&lines),
        railways,
    }
}

/// The railways near the route joined into continuous lines where their pieces meet end to end
/// (not at switches, where three meet; `chains`): each line as its pieces with what carries
/// them, and whether it is a funicular.
fn chains(
    map: &MapData,
    projection: &LocalProjection,
    road: &RoadIndex,
) -> Vec<(Vec<Piece>, bool)> {
    let pieces: Vec<(Piece, bool)> = map
        .railways
        .iter()
        .map(|railway| {
            let line: Vec<(f64, f64)> = railway
                .line
                .iter()
                .map(|&(lat, lon)| projection.project(lat, lon))
                .collect();
            let surface = match railway.structure {
                Some(StructureKind::Bridge) => Surface::Bridge,
                Some(StructureKind::Tunnel) => Surface::Tunnel,
                None => Surface::Ground,
            };
            ((line, surface), railway.funicular)
        })
        .filter(|((line, _), _)| {
            line.len() >= 2
                && line
                    .iter()
                    .any(|&(e, n)| road.nearest(e, n, CORRIDOR).is_some())
        })
        .collect();
    let lines: Vec<&[(f64, f64)]> = pieces
        .iter()
        .map(|((line, _), _)| line.as_slice())
        .collect();
    crate::chains::chains(&lines, &|_, _| true)
        .into_iter()
        .map(|chain| {
            let funicular = chain.iter().any(|&(piece, _)| pieces[piece].1);
            let joined: Vec<Piece> = chain
                .iter()
                .map(|&(piece, reversed)| {
                    let (mut line, surface) = pieces[piece].0.clone();
                    if reversed {
                        line.reverse();
                    }
                    (line, surface)
                })
                .collect();
            (joined, funicular)
        })
        .collect()
}

/// A railway's centre line: heights from the terrain, smoothed and held to its grades, bridges
/// and tunnels straight between their ends, tunnels and viaducts where the ground lies far off,
/// its crossings with the road ridden, and alongside the tracks `laid` out before it, with them.
/// `None` without any terrain height.
fn lay_out(
    points: &[Point],
    funicular: bool,
    road: &RoadIndex,
    laid: &RoadIndex,
) -> Option<Vec<Centre>> {
    let mut along = vec![0.0];
    for pair in points.windows(2) {
        let (a, b) = (pair[0].position, pair[1].position);
        along.push(along[along.len() - 1] + (b.0 - a.0).hypot(b.1 - a.1));
    }
    // On the ground the terrain's heights; over and under structures nothing yet.
    let known: Vec<Option<f64>> = points
        .iter()
        .map(|p| p.terrain.filter(|_| p.surface == Surface::Ground))
        .collect();
    let mut heights = fill_between(&known, &along)?;
    heights = smooth(&heights, &along, SMOOTHING_M);
    let mut surfaces: Vec<Surface> = points.iter().map(|p| p.surface).collect();
    let mut pins = crossings(points, &surfaces, road);
    let beside: Vec<Option<(f64, Surface)>> = (0..points.len())
        .map(|k| {
            let (a, b) = (
                points[k.saturating_sub(1)].position,
                points[(k + 1).min(points.len() - 1)].position,
            );
            let length = (b.0 - a.0).hypot(b.1 - a.1).max(1e-9);
            let way = ((b.0 - a.0) / length, (b.1 - a.1) / length);
            let (east, north) = points[k].position;
            laid.alongside(east, north, way, PARALLEL_M)
        })
        .collect();
    pins.extend(
        beside
            .iter()
            .enumerate()
            .filter_map(|(k, b)| b.map(|(height, _)| (k, height, Pin::Exactly))),
    );
    hold_grades(&mut heights, &along, &pins, funicular);
    // Tunnels where the hill rises far above the track, viaducts where the ground falls far
    // below it.
    for (k, point) in points.iter().enumerate() {
        if surfaces[k] != Surface::Ground {
            continue;
        }
        if let Some(terrain) = point.terrain {
            if terrain - heights[k] > TUNNEL_COVER {
                surfaces[k] = Surface::Tunnel;
            } else if heights[k] - terrain > VIADUCT_HEIGHT {
                surfaces[k] = Surface::Bridge;
            }
        }
    }
    // Alongside, the structures the map has on this track stay; else it takes the other's.
    let kept: Vec<Surface> = points
        .iter()
        .zip(&beside)
        .map(|(p, b)| match b {
            Some((_, other)) if p.surface == Surface::Ground => *other,
            _ => p.surface,
        })
        .collect();
    for (k, &surface) in kept.iter().enumerate() {
        if beside[k].is_some() {
            surfaces[k] = surface;
        }
    }
    drop_short_structures(&mut surfaces, &along, &kept);
    Some(
        points
            .iter()
            .enumerate()
            .map(|(k, p)| Centre {
                position: p.position,
                elevation: heights[k],
                distance: along[k],
                surface: surfaces[k],
            })
            .collect(),
    )
}

/// Heights everywhere from those known: straight between known ones, level beyond the first
/// and last. `None` if none is known.
fn fill_between(known: &[Option<f64>], along: &[f64]) -> Option<Vec<f64>> {
    let first = known.iter().position(Option::is_some)?;
    let last = known.iter().rposition(Option::is_some)?;
    let mut heights = vec![0.0; known.len()];
    let mut previous = first;
    for k in 0..known.len() {
        heights[k] = if k <= first {
            known[first].unwrap_or_default()
        } else if k >= last {
            known[last].unwrap_or_default()
        } else if let Some(h) = known[k] {
            previous = k;
            h
        } else {
            let next = (k..=last).find(|&j| known[j].is_some()).unwrap_or(last);
            let (a, b) = (
                known[previous].unwrap_or_default(),
                known[next].unwrap_or_default(),
            );
            let share = (along[k] - along[previous]) / (along[next] - along[previous]).max(1e-9);
            a + (b - a) * share
        };
    }
    Some(heights)
}

/// Each height the mean of those within `reach` metres along the line.
fn smooth(heights: &[f64], along: &[f64], reach: f64) -> Vec<f64> {
    let (mut from, mut to) = (0, 0);
    let mut sum = 0.0;
    let mut smoothed = Vec::with_capacity(heights.len());
    for k in 0..heights.len() {
        while to < heights.len() && along[to] <= along[k] + reach {
            sum += heights[to];
            to += 1;
        }
        while along[from] < along[k] - reach {
            sum -= heights[from];
            from += 1;
        }
        #[allow(clippy::cast_precision_loss)] // a few dozen points
        smoothed.push(sum / (to - from) as f64);
    }
    smoothed
}

/// Where the line crosses the road ridden: the heights it must keep there (point, height and
/// whether that is a least, a most or an exact height).
fn crossings(points: &[Point], surfaces: &[Surface], road: &RoadIndex) -> Vec<(usize, f64, Pin)> {
    let reach = ROAD_HALF_WIDTH + BED_M / 2.0 + 1.0;
    let mut pins = Vec::new();
    for (k, point) in points.iter().enumerate() {
        let (e, n) = point.position;
        let Some((_, road_height, road_surface)) = road.nearest(e, n, reach) else {
            continue;
        };
        let pin = match (surfaces[k], road_surface) {
            (Surface::Tunnel, _) | (_, Surface::Tunnel) => continue,
            (Surface::Bridge, _) => (k, road_height + OVER_ROAD, Pin::AtLeast),
            (_, Surface::Bridge) => (k, road_height - UNDER_ROAD, Pin::AtMost),
            _ => (k, road_height - AT_ROAD, Pin::Exactly),
        };
        pins.push(pin);
    }
    pins
}

/// How a crossing holds the line's height.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pin {
    AtLeast,
    AtMost,
    Exactly,
}

/// Keeps the line's grades within `MAX_GRADE` (funiculars excepted) while it keeps its heights
/// at the crossings.
fn hold_grades(heights: &mut [f64], along: &[f64], pins: &[(usize, f64, Pin)], funicular: bool) {
    let apply = |heights: &mut [f64]| {
        for &(k, height, pin) in pins {
            heights[k] = match pin {
                Pin::AtLeast => heights[k].max(height),
                Pin::AtMost => heights[k].min(height),
                Pin::Exactly => height,
            };
        }
    };
    apply(heights);
    if funicular {
        return;
    }
    let pinned: Vec<bool> = (0..heights.len())
        .map(|k| pins.iter().any(|p| p.0 == k))
        .collect();
    for _ in 0..4 {
        for k in 1..heights.len() {
            let room = MAX_GRADE * (along[k] - along[k - 1]);
            if !pinned[k] {
                heights[k] = heights[k].clamp(heights[k - 1] - room, heights[k - 1] + room);
            }
        }
        for k in (0..heights.len() - 1).rev() {
            let room = MAX_GRADE * (along[k + 1] - along[k]);
            if !pinned[k] {
                heights[k] = heights[k].clamp(heights[k + 1] - room, heights[k + 1] + room);
            }
        }
        apply(heights);
    }
}

/// Turns tunnels and viaducts found from the ground's shape back into track where they would be
/// shorter than `SHORTEST_STRUCTURE`; those the map has stay.
fn drop_short_structures(surfaces: &mut [Surface], along: &[f64], mapped: &[Surface]) {
    let mut k = 0;
    while k < surfaces.len() {
        let kind = surfaces[k];
        let start = k;
        while k < surfaces.len() && surfaces[k] == kind {
            k += 1;
        }
        let length = along[k - 1] - along[start];
        if kind != Surface::Ground && length < SHORTEST_STRUCTURE {
            for j in start..k {
                if mapped[j] == Surface::Ground {
                    surfaces[j] = Surface::Ground;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grade_at(heights: &[f64], along: &[f64]) -> f64 {
        heights
            .windows(2)
            .zip(along.windows(2))
            .map(|(h, a)| (h[1] - h[0]).abs() / (a[1] - a[0]))
            .fold(0.0, f64::max)
    }

    #[test]
    fn lines_keep_gentle_grades_and_their_crossings() {
        // A steep, bumpy hillside 1 km long; a level crossing at 500 m.
        let along: Vec<f64> = (0..=200).map(|k| f64::from(k) * 5.0).collect();
        let mut heights: Vec<f64> = along
            .iter()
            .map(|a| 500.0 + 0.12 * a + 3.0 * (a / 15.0).sin())
            .collect();
        let pins = [(100, 520.0, Pin::Exactly)];
        hold_grades(&mut heights, &along, &pins, false);

        assert!(grade_at(&heights, &along) <= MAX_GRADE + 1e-9);
        assert!((heights[100] - 520.0).abs() < 1e-9);
        // A funicular keeps its slope.
        let mut steep: Vec<f64> = along.iter().map(|a| 500.0 + 0.3 * a).collect();
        hold_grades(&mut steep, &along, &[], true);
        assert!(grade_at(&steep, &along) > 0.29);
    }

    #[test]
    fn short_structures_from_the_ground_s_shape_are_dropped() {
        let along: Vec<f64> = (0..10).map(|k| f64::from(k) * 5.0).collect();
        let mapped = [Surface::Ground; 10];
        let mut found = mapped;
        found[3] = Surface::Tunnel;
        found[4] = Surface::Tunnel;
        drop_short_structures(&mut found, &along, &mapped);
        assert_eq!(found, mapped);
        // A mapped bridge stays, however short.
        let mut mapped_bridge = mapped;
        mapped_bridge[5] = Surface::Bridge;
        let mut kept = mapped_bridge;
        drop_short_structures(&mut kept, &along, &mapped_bridge);
        assert_eq!(kept, mapped_bridge);
    }
}
