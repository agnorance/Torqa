//! Trees, bushes and rocks (Blender models, `art/vegetation`), and grass and flowers along the
//! road (R45, ADR 0011); in the subtropics palms, banana plants and tropical shrubs (#136).

use std::collections::BTreeMap;
use std::sync::LazyLock;

use serde::Deserialize;
use torqa_osm::LandCover;

use crate::HeightGrid;
use crate::buildings::Point;
use crate::climate::Climate;
use crate::landcover::LandIndex;
use crate::palette;
use crate::road::RoadIndex;
use crate::streets::Clearance;

/// The manifest the vegetation build writes next to the models.
const MANIFEST: &str = include_str!("../../../app/assets/models/vegetation/models.json");

/// Plant spacing close to the road, where they are seen up close.
const NEAR_SPACING: f64 = 7.0;
/// Tree spacing further away, where the forest colour on the ground does most of the work.
const FAR_SPACING: f64 = 20.0;
/// Within this distance of the road trees use the near spacing, and other plants grow at all.
const NEAR_DISTANCE: f64 = 300.0;
/// Bushes grow under the trees within this distance of the road.
const UNDERSTOREY_DISTANCE: f64 = 60.0;
/// A slope steeper than this (rise over run) shows rocks.
const ROCKY_SLOPE: f64 = 0.7;
/// Grass grows within this distance of the road centre, where riders see it up close.
const GRASS_DISTANCE: f64 = 30.0;
/// Grass keeps off the road: half its width and a little more.
const GRASS_CLEARANCE: f64 = 3.6;
/// Spacing of grass tufts, jittered.
const GRASS_SPACING: f64 = 1.1;
/// Floats per plant in a `MultiMesh` buffer: transform (12) and colour (4).
pub(crate) const PLANT_FLOATS: usize = 16;

/// What grows: each kind has its models, its palette colours and its distance from the road.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Plant {
    Conifer,
    Broadleaf,
    Bush,
    Rock,
    /// Coconut and fan palms.
    Palm,
    Banana,
    /// Broadleaf shrubs of the tropics.
    TropicalBush,
}

impl Plant {
    fn kind(self) -> &'static str {
        match self {
            Self::Conifer => "conifer",
            Self::Broadleaf => "broadleaf",
            Self::Bush => "bush",
            Self::Rock => "rock",
            Self::Palm => "palm",
            Self::Banana => "banana",
            Self::TropicalBush => "tropical_bush",
        }
    }

    fn colours(self) -> &'static str {
        match self {
            Self::Conifer => "plants.conifers",
            Self::Broadleaf => "plants.broadleaves",
            Self::Bush => "plants.bushes",
            Self::Rock => "plants.rocks",
            Self::Palm => "plants.palms",
            Self::Banana | Self::TropicalBush => "plants.tropical",
        }
    }

    /// Distance kept from the road centre: the road's half width and the plant's reach.
    fn clearance(self) -> f64 {
        match self {
            Self::Conifer | Self::Broadleaf | Self::Palm => 8.0,
            Self::Bush | Self::Banana | Self::TropicalBush => 5.0,
            Self::Rock => 4.5,
        }
    }

    /// Random size: a factor on the model.
    fn scale(self, dice: f64) -> f64 {
        match self {
            Self::Conifer | Self::Broadleaf => 0.75 + dice * 0.6,
            Self::Palm | Self::Banana => 0.8 + dice * 0.45,
            Self::Bush | Self::TropicalBush => 0.7 + dice * 0.7,
            Self::Rock => 0.6 + dice * 1.4,
        }
    }
}

#[derive(Deserialize)]
struct Entry {
    kind: String,
}

#[derive(Deserialize)]
struct Manifest {
    models: BTreeMap<String, Entry>,
}

/// Model names by kind, in name order.
static MODELS: LazyLock<BTreeMap<String, Vec<String>>> = LazyLock::new(|| {
    let manifest: Manifest =
        serde_json::from_str(MANIFEST).expect("the committed vegetation models.json is valid");
    let mut by_kind: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, entry) in manifest.models {
        by_kind.entry(entry.kind).or_default().push(name);
    }
    by_kind
});

/// The kind (`conifer`, `broadleaf`, `bush`, `rock`, `palm`, …) of a vegetation model.
#[cfg(test)]
pub(crate) fn kind_of(model: &str) -> Option<&'static str> {
    MODELS
        .iter()
        .find(|(_, names)| names.iter().any(|name| name == model))
        .map(|(kind, _)| kind.as_str())
}

/// Plants of one chunk, relative to the chunk origin: trees, bushes and rocks as Godot
/// `MultiMesh` buffers per model (transform and colour, `PLANT_FLOATS` per plant), grass and
/// flowers as transform buffers (12 floats each).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Trees {
    /// Trees, bushes and rocks by vegetation model name.
    pub models: BTreeMap<String, Vec<f32>>,
    /// Grass tufts along the road.
    pub grass: Vec<f32>,
    /// Flower clumps in meadows along the road.
    pub flowers: Vec<f32>,
}

impl Trees {
    /// Number of trees, bushes and rocks (grass and flowers not counted).
    #[must_use]
    pub fn len(&self) -> usize {
        self.models.values().map(Vec::len).sum::<usize>() / PLANT_FLOATS
    }

    /// Whether there are no trees, bushes or rocks.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Adds a plant of `kind` standing at `position`: one of its models and palette colours,
    /// size and heading chosen by `seed`.
    fn add(&mut self, kind: Plant, position: [f64; 3], seed: i64, chunk_origin: [f64; 3]) {
        let Some(models) = MODELS.get(kind.kind()).filter(|m| !m.is_empty()) else {
            return;
        };
        let pick = |salt: i64, count: usize| {
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                clippy::cast_precision_loss
            )] // a small count; the product lies in [0, count)
            let index = (crate::hash(seed ^ salt) * count as f64) as usize;
            index.min(count - 1)
        };
        let model = &models[pick(0x2b7e, models.len())];
        let colour = palette::pick(kind.colours(), pick(0x6c8f, 64), 1.0);
        let scale = kind.scale(crate::hash(seed ^ 0x1234));
        let yaw = crate::hash(seed ^ 0x4321) * std::f64::consts::TAU;
        let buffer = self.models.entry(model.clone()).or_default();
        push_transform(buffer, position, scale, yaw, chunk_origin);
        buffer.extend(colour);
    }
}

/// A building's footprint as a circle around it, which plants keep out of.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Footprint {
    east: f64,
    north: f64,
    radius: f64,
}

impl Footprint {
    pub(crate) fn around(outline: &[Point]) -> Self {
        #[allow(clippy::cast_precision_loss)] // few corners
        let count = outline.len().max(1) as f64;
        let east = outline.iter().map(|p| p.0).sum::<f64>() / count;
        let north = outline.iter().map(|p| p.1).sum::<f64>() / count;
        let radius = outline
            .iter()
            .map(|p| (p.0 - east).hypot(p.1 - north))
            .fold(0.0, f64::max);
        Self {
            east,
            north,
            radius,
        }
    }
}

/// What plants are placed on: the chunk's ground, its land cover, the road ridden, the map's
/// other streets and the buildings around, in the region's climate.
#[derive(Clone, Copy)]
pub(crate) struct Ground<'a> {
    pub(crate) heights: &'a HeightGrid,
    pub(crate) land: &'a LandIndex,
    pub(crate) road: &'a RoadIndex,
    pub(crate) streets: &'a Clearance,
    pub(crate) buildings: &'a [Footprint],
    pub(crate) climate: Climate,
}

impl Ground<'_> {
    /// Whether something of `reach` metres standing at a point would touch a building.
    fn by_building(&self, east: f64, north: f64, reach: f64) -> bool {
        self.buildings
            .iter()
            .any(|b| (b.east - east).hypot(b.north - north) < b.radius + reach)
    }

    /// The ground's steepest rise over run at a point.
    fn slope(&self, east: f64, north: f64) -> f64 {
        let step = 2.0;
        let east_slope = (self.heights.at(east + step, north)
            - self.heights.at(east - step, north))
            / (2.0 * step);
        let north_slope = (self.heights.at(east, north + step)
            - self.heights.at(east, north - step))
            / (2.0 * step);
        east_slope.hypot(north_slope)
    }
}

/// Places trees, bushes and rocks within the square `[origin, origin + size]` (metres
/// east/north): forests of conifers and broadleaf trees (conifers higher up) with bushes under
/// them near the road; near the road also a few solitary trees and bushes in meadows and
/// gardens, and rocks on rocky ground and steep slopes. In the subtropical lowlands palms
/// take the conifers' place, and banana plants and tropical shrubs the bushes'. Nothing on the
/// road or streets, in water or in buildings.
pub(crate) fn place(
    origin: (f64, f64),
    size: f64,
    ground: &Ground,
    chunk_origin: [f64; 3],
) -> Trees {
    let Ground {
        heights,
        land,
        road,
        streets,
        ..
    } = *ground;
    let mut trees = Trees::default();
    let mut north = origin.1;
    // Rows use the near spacing and thin out far from the road, keeping placement
    // deterministic for a position regardless of chunk boundaries.
    while north < origin.1 + size {
        let mut east = origin.0;
        while east < origin.0 + size {
            let seed = cell_seed(east, north);
            let (e, n) = (
                east + (crate::hash(seed) - 0.5) * NEAR_SPACING,
                north + (crate::hash(seed ^ 0x5bd1) - 0.5) * NEAR_SPACING,
            );
            east += NEAR_SPACING;
            let cover = land.cover_at(e, n);
            let road_distance = road.nearest(e, n, NEAR_DISTANCE).map(|(d, _, _)| d);
            let Some(kind) = choose(cover, road_distance, heights.at(e, n), seed, ground, e, n)
            else {
                continue;
            };
            let too_close = road_distance.is_some_and(|d| d < kind.clearance());
            if too_close || streets.blocked(e, n, 2.0) || ground.by_building(e, n, 2.0) {
                continue;
            }
            trees.add(kind, [e, heights.at(e, n), n], seed, chunk_origin);
        }
        north += NEAR_SPACING;
    }
    trees
}

/// What grows at a spot, if anything: by its land cover, its distance from the road (`None`
/// beyond `NEAR_DISTANCE`), its height and slope, and dice from `seed`.
fn choose(
    cover: Option<LandCover>,
    road_distance: Option<f64>,
    height: f64,
    seed: i64,
    ground: &Ground,
    east: f64,
    north: f64,
) -> Option<Plant> {
    let dice = |salt: i64| crate::hash(seed ^ salt);
    if cover == Some(LandCover::Water) {
        return None;
    }
    let tropical = ground.climate.tropical_at(height);
    let near = road_distance.is_some();
    if near && (cover == Some(LandCover::Rock) || ground.slope(east, north) > ROCKY_SLOPE) {
        let share = if cover == Some(LandCover::Rock) {
            0.2
        } else {
            0.1
        };
        return (dice(0x0dc5) < share).then_some(Plant::Rock);
    }
    match cover {
        Some(LandCover::Forest) => {
            if road_distance.is_some_and(|d| d < UNDERSTOREY_DISTANCE) && dice(0x3b9a) < 0.12 {
                return Some(if tropical {
                    tropical_shrub(dice(0x2d4f), 0.3)
                } else {
                    Plant::Bush
                });
            }
            let keep = (NEAR_SPACING / FAR_SPACING).powi(2);
            if !near && dice(0x9e37) > keep {
                return None;
            }
            if tropical {
                // Evergreen broadleaf forest, palms among it.
                return Some(if dice(0x7777) < TROPICAL_FOREST_PALMS {
                    Plant::Palm
                } else {
                    Plant::Broadleaf
                });
            }
            // Conifers dominate higher up.
            let conifer_share = ((height - 600.0) / 800.0).clamp(0.3, 0.9);
            Some(if dice(0x7777) < conifer_share {
                Plant::Conifer
            } else {
                Plant::Broadleaf
            })
        }
        // Open land near the road: now and then a tree on its own, or a bush.
        Some(LandCover::Meadow | LandCover::Public | LandCover::Residential) | None if near => {
            let (tree, shrub) = if cover == Some(LandCover::Residential) {
                (0.03, 0.04)
            } else {
                (0.006, 0.015)
            };
            let roll = dice(0x51a3);
            if roll < tree {
                // In the subtropics most trees standing alone are palms.
                Some(if tropical && dice(0x6a09) < TROPICAL_OPEN_PALMS {
                    Plant::Palm
                } else {
                    Plant::Broadleaf
                })
            } else if roll < tree + shrub {
                Some(if tropical {
                    // Banana plants grow in gardens more than in the open.
                    let bananas = if cover == Some(LandCover::Residential) {
                        0.4
                    } else {
                        0.15
                    };
                    tropical_shrub(dice(0x2d4f), bananas)
                } else {
                    Plant::Bush
                })
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Share of palms among the trees of tropical forests, and among trees standing alone there.
const TROPICAL_FOREST_PALMS: f64 = 0.25;
const TROPICAL_OPEN_PALMS: f64 = 0.7;

/// A shrub of the tropics: a banana plant for a `roll` below `bananas`, else a broadleaf shrub.
fn tropical_shrub(roll: f64, bananas: f64) -> Plant {
    if roll < bananas {
        Plant::Banana
    } else {
        Plant::TropicalBush
    }
}

/// Places grass tufts and flower clumps along the road within the square `[origin, origin +
/// size]` into `plants`: on open ground (meadows, farmland verges, orchards, lawns), sparse on
/// the forest floor, never on the road or other streets, in water or on rock.
pub(crate) fn place_grass(
    plants: &mut Trees,
    origin: (f64, f64),
    size: f64,
    ground: &Ground,
    chunk_origin: [f64; 3],
) {
    let Ground {
        heights,
        land,
        road,
        streets,
        ..
    } = *ground;
    let mut north = (origin.1 / GRASS_SPACING).floor() * GRASS_SPACING;
    while north < origin.1 + size {
        let mut east = (origin.0 / GRASS_SPACING).floor() * GRASS_SPACING;
        while east < origin.0 + size {
            let seed = grass_seed(east, north);
            let (e, n) = (
                east + (crate::hash(seed) - 0.5) * GRASS_SPACING,
                north + (crate::hash(seed ^ 0x2c1b) - 0.5) * GRASS_SPACING,
            );
            east += GRASS_SPACING;
            let inside =
                e >= origin.0 && e < origin.0 + size && n >= origin.1 && n < origin.1 + size;
            let Some((distance, _, _)) = road.nearest(e, n, GRASS_DISTANCE) else {
                continue;
            };
            if !inside || distance < GRASS_CLEARANCE || streets.blocked(e, n, 0.2) {
                continue;
            }
            let cover = land.cover_at(e, n);
            let (density, flowery) = match cover {
                Some(LandCover::Water | LandCover::Rock) => (0.0, false),
                // Yards and car parks: a little grass at the edges.
                Some(LandCover::Industrial | LandCover::Commercial) => (0.2, false),
                Some(LandCover::Forest) => (0.25, false),
                Some(LandCover::Meadow | LandCover::Public) | None => (1.0, true),
                Some(LandCover::Farmland | LandCover::Orchard | LandCover::Residential) => {
                    (0.8, false)
                }
            };
            // Thinner towards the edge of the strip, so it does not end in a hard line.
            let fade = 1.0 - ((distance - GRASS_DISTANCE * 0.6) / (GRASS_DISTANCE * 0.4)).max(0.0);
            if crate::hash(seed ^ 0x51ed) > density * fade {
                continue;
            }
            let height = heights.at(e, n);
            let scale = 0.7 + crate::hash(seed ^ 0x0dd5) * 0.7;
            let yaw = crate::hash(seed ^ 0x3a7c) * std::f64::consts::TAU;
            let target = if flowery && crate::hash(seed ^ 0x6b43) < FLOWER_SHARE {
                &mut plants.flowers
            } else {
                &mut plants.grass
            };
            push_transform(target, [e, height, n], scale, yaw, chunk_origin);
        }
        north += GRASS_SPACING;
    }
}

/// Share of meadow grass spots that are flower clumps instead.
const FLOWER_SHARE: f64 = 0.12;

/// A stable seed per grass cell.
fn grass_seed(east: f64, north: f64) -> i64 {
    #[allow(clippy::cast_possible_truncation)] // cell indices are small
    let (e, n) = (
        (east / GRASS_SPACING).round() as i64,
        (north / GRASS_SPACING).round() as i64,
    );
    e.wrapping_mul(83_492_791) ^ n.wrapping_mul(2_654_435_761)
}

/// A stable seed per grid cell.
fn cell_seed(east: f64, north: f64) -> i64 {
    #[allow(clippy::cast_possible_truncation)] // cell indices are small
    let (e, n) = (
        (east / NEAR_SPACING).round() as i64,
        (north / NEAR_SPACING).round() as i64,
    );
    e.wrapping_mul(73_856_093) ^ n.wrapping_mul(19_349_663)
}

/// Appends a Godot `Transform3D` in `MultiMesh` buffer layout (row-major 3×4).
#[allow(clippy::cast_possible_truncation)] // geometry is stored as f32 for the GPU
fn push_transform(
    buffer: &mut Vec<f32>,
    [east, height, north]: [f64; 3],
    scale: f64,
    yaw: f64,
    origin: [f64; 3],
) {
    let (sin, cos) = yaw.sin_cos();
    let (x, y, z) = (east - origin[0], height - origin[1], -north - origin[2]);
    buffer.extend(
        [
            cos * scale,
            0.0,
            sin * scale,
            x,
            0.0,
            scale,
            0.0,
            y,
            -sin * scale,
            0.0,
            cos * scale,
            z,
        ]
        .map(|v| v as f32),
    );
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn every_plant_has_models_with_files_and_palette_colours() {
        let directory =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../app/assets/models/vegetation");
        for plant in [
            Plant::Conifer,
            Plant::Broadleaf,
            Plant::Bush,
            Plant::Rock,
            Plant::Palm,
            Plant::Banana,
            Plant::TropicalBush,
        ] {
            let models = MODELS.get(plant.kind()).map_or(&[][..], Vec::as_slice);
            assert!(!models.is_empty(), "no {} models", plant.kind());
            for name in models {
                assert!(
                    directory.join(format!("{name}.glb")).is_file(),
                    "{name} is missing"
                );
            }
            // Panics if the palette lacks the plant's colours.
            palette::pick(plant.colours(), 0, 1.0);
        }
    }
}
