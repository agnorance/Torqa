//! 3D world geometry for Torqa (R16): terrain chunks in a corridor around the route, coloured
//! by land cover, with buildings and trees from OpenStreetMap, rivers, and the road.
//!
//! Coordinates follow Godot: metres with x east, y up and z south (−z is north), relative to
//! the route start for the road and water, and to each chunk's centre for chunk geometry.
//! Triangles wind clockwise seen from their front, Godot's front-face order.

mod buildings;
mod channels;
mod drape;
mod horizon;
mod junctions;
mod landcover;
mod minimap;
mod palette;
mod railways;
mod road;
mod roundabouts;
mod streets;
mod structures;
mod vegetation;
mod water;

use std::collections::{BTreeMap, BTreeSet, HashMap};

use torqa_osm::{LandCover, MapData, RoadClass};
use torqa_routes::{ElevationModel, LocalProjection, Route, Surface};
use tracing::{info, warn};

pub use horizon::{HORIZON, Horizon, horizon};
use landcover::LandIndex;
pub use minimap::{BACKGROUND as MINIMAP_BACKGROUND, FlatMap};
use road::RoadIndex;
pub use vegetation::Trees;

/// Edge length of a terrain chunk.
pub(crate) const CHUNK_SIZE: f64 = 480.0;
/// Distance between terrain vertices.
const GRID: f64 = 16.0;
/// Terrain (and map data) is used up to this far from the route.
pub const CORRIDOR: f64 = 1500.0;
/// Half the road width.
const ROAD_HALF_WIDTH: f64 = 3.0;
/// Terrain cells near the road are split into this many pieces a side, so the ground can follow
/// the road's verge, cuttings and embankments closely.
const SUB: usize = 6;
/// The size of those pieces.
#[allow(clippy::cast_precision_loss)] // a small constant
const FINE: f64 = GRID / SUB as f64;
/// Ground up to this distance from the road centre is level just below the road: the road and
/// its verge. Wide enough that no triangle of the fine ground touching the road can rise above
/// it (half the road plus the diagonal of a fine piece, and a little).
pub(crate) const VERGE: f64 = ROAD_HALF_WIDTH + FINE * std::f64::consts::SQRT_2 + 0.2;
/// Beyond the verge the ground climbs to the hillside at most this steeply (a cutting)...
const CUT_SLOPE: f64 = 1.2;
/// ...or falls to the valley at most this steeply (an embankment).
const FILL_SLOPE: f64 = 0.6;
/// Cuttings and embankments reach at most this far from the road; over the last
/// `REACH_FADE` metres the shaped ground blends into the natural.
pub(crate) const LEVEL_REACH: f64 = 45.0;
const REACH_FADE: f64 = 12.0;
/// Level ground sits this far below the road surface, so the two never flicker; the road's edge
/// bevels down to it (road.rs). Other streets lie between the two where they join the road.
const ROAD_SINK: f64 = 0.15;
/// Without terrain data, heights follow the road found within this distance.
const FALLBACK_RADIUS: f64 = 400.0;

/// Triangle geometry ready for a GPU mesh.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MeshData {
    /// Vertex positions.
    pub vertices: Vec<[f32; 3]>,
    /// Unit vertex normals.
    pub normals: Vec<[f32; 3]>,
    /// Texture coordinates; for the road `u` is 0–1 across and `v` the distance in metres.
    pub uvs: Vec<[f32; 2]>,
    /// Vertex colours (RGB tint, alpha 1 marks water); empty when the mesh has none.
    pub colors: Vec<[f32; 4]>,
    /// Three vertex indices per triangle.
    pub indices: Vec<u32>,
}

impl MeshData {
    /// Adds another mesh's triangles.
    ///
    /// # Panics
    /// If the combined mesh has more than `u32::MAX` vertices.
    pub fn append(&mut self, other: MeshData) {
        let offset = u32::try_from(self.vertices.len()).expect("mesh fits u32");
        self.vertices.extend(other.vertices);
        self.normals.extend(other.normals);
        self.uvs.extend(other.uvs);
        self.colors.extend(other.colors);
        self.indices
            .extend(other.indices.into_iter().map(|i| i + offset));
    }
}

/// The ways and water of the map around the route, ready for the chunks.
struct Ways {
    streets: Vec<streets::Street>,
    /// The railways, on their own lines (#85).
    network: railways::Network,
    streams: Vec<water::Stream>,
    pools: Vec<water::Pool>,
    /// The rounded corners where streets meet.
    corners: Vec<junctions::Corner>,
    /// Where plants keep off.
    clearance: streets::Clearance,
    /// Roundabouts' islands.
    islands: Vec<roundabouts::Island>,
    /// Where water cuts the ground.
    channels: channels::Channels,
}

impl Ways {
    async fn new<M: ElevationModel>(
        map: &MapData,
        projection: &LocalProjection,
        road: &RoadIndex,
        model: &mut M,
    ) -> Self {
        let streets = streets::lines(map, projection, model).await;
        let network = railways::network(map, projection, road, model).await;
        let streams = water::streams(&map.waterways, projection, road);
        let pools = water::pools(&map.areas, projection, road);
        let corners = junctions::corners(&streets, road);
        Self {
            clearance: streets::Clearance::new(&streets, &corners, &network.railways, &pools),
            corners,
            islands: roundabouts::islands(map, projection),
            channels: channels::Channels::new(&streams, &pools, &streets),
            streets,
            network,
            streams,
            pools,
        }
    }
}

/// A square piece of the world.
#[derive(Debug, Clone, PartialEq)]
pub struct TerrainChunk {
    /// Centre of the chunk at sea level; the chunk's geometry is relative to it.
    pub center: [f32; 3],
    /// Ground, coloured by land cover.
    pub mesh: MeshData,
    /// Buildings standing in the chunk that no model fits, built as shells.
    pub buildings: MeshData,
    /// Buildings drawn as Blender-made models up close, by cell.
    pub modelled: Vec<BuildingCell>,
    /// Paved streets of the map around the route (not the road ridden), on this chunk's ground.
    pub streets: MeshData,
    /// Unpaved tracks and paths of the map, likewise.
    pub tracks: MeshData,
    /// Lakes, ponds, rivers and streams of the map, likewise; they pass under streets and the
    /// road.
    pub water: MeshData,
    /// Trees standing in the chunk.
    pub trees: Trees,
}

/// Buildings drawn as Blender-made models (`app/assets/models/buildings`) up close, in one cell
/// of a chunk; each cell switches to the shells by its own distance.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BuildingCell {
    /// Instances by model name, as Godot `MultiMesh` buffers of 20 floats each: the transform
    /// relative to the chunk centre (row-major 3×4), the plaster colour (sRGB, alpha 1) as
    /// instance colour, and the roof colour (sRGB) with a variant in 0–1 as custom data.
    pub models: BTreeMap<String, Vec<f32>>,
    /// The same buildings as shells, for the distance.
    pub shells: MeshData,
}

/// The generated world.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct World {
    /// Terrain chunks around the route.
    pub chunks: Vec<TerrainChunk>,
    /// The road along the route.
    pub road: MeshData,
    /// Bridges and tunnels, of the road and the railways.
    pub structures: MeshData,
    /// The railways of the map near the route, in route coordinates (`u` across the bed of
    /// ballast, below 0 and above 1 on its shoulders, `v` metres along).
    pub railways: MeshData,
    /// The land beyond the corridor, from [`horizon`] (empty until it is made: it needs a coarse
    /// terrain model of its own).
    pub horizon: Horizon,
    /// Flat map of the corridor for the minimap.
    pub minimap: FlatMap,
    /// Terrain samples that had no elevation data and followed the road instead.
    pub fallback_samples: usize,
}

/// The parts of the world built whole rather than by chunk: the road ridden, the bridges and
/// tunnels, the railways and the map.
async fn whole<M: ElevationModel>(
    map: &MapData,
    projection: &LocalProjection,
    (shapers, ways, below): (&Shapers<'_>, &Ways, &structures::Below<'_>),
    model: &mut M,
) -> World {
    let (road, rails) = (shapers.road, shapers.rails);
    World {
        road: road.mesh(ROAD_HALF_WIDTH, &streets::mouths(&ways.streets, road)),
        structures: structures::build_all((road, rails), below, shapers.portals, projection, model)
            .await,
        railways: rails.mesh(railways::BED_M / 2.0, &[]),
        minimap: minimap::build(map, projection, road),
        ..World::default()
    }
}

/// Builds the world for `route`, sampling heights from `model` (e.g. the terrain tiles) and
/// placing `map` features. Where the model has no data, the terrain follows the road.
/// `progress` is called with (chunks done, chunks total).
pub async fn generate<M: ElevationModel>(
    route: &Route,
    model: &mut M,
    map: &MapData,
    progress: &mut (dyn FnMut(usize, usize) + Send),
) -> World {
    let projection = LocalProjection::for_route(route);
    let mut road = RoadIndex::new(route, &projection);
    let cells = chunks_near_route(&road);
    let total = cells.len();
    // Announce the step before the slower preparation below.
    progress(0, total);
    let land = LandIndex::new(&map.areas, &projection);
    let buildings = buildings_by_chunk(map, &projection, &road, &land);
    let mut ways = Ways::new(map, &projection, &road, model).await;
    let portals =
        structures::open_portals(&mut road, &mut ways.network.index, &projection, model).await;
    let Ways {
        streets,
        streams,
        pools,
        ..
    } = &ways;
    let shapers = Shapers {
        road: &road,
        rails: &ways.network.index,
        portals: &portals,
    };
    let below = structures::Below::new(&road, &ways.network.index, streets);
    let mut world = whole(map, &projection, (&shapers, &ways, &below), model).await;

    for (done, (cx, cn)) in cells.into_iter().enumerate() {
        let heights = HeightGrid::sample(
            cx,
            cn,
            &projection,
            &shapers,
            (&ways.channels, &land),
            model,
            &mut world,
        )
        .await;
        let origin = [
            heights.origin.0 + CHUNK_SIZE / 2.0,
            0.0,
            -(heights.origin.1 + CHUNK_SIZE / 2.0),
        ];
        let mut chunk_buildings = buildings::ChunkBuildings::default();
        for plot in buildings.get(&(cx, cn)).into_iter().flatten() {
            buildings::add(&mut chunk_buildings, plot, &heights, origin);
        }
        #[allow(clippy::cast_possible_truncation)] // geometry is stored as f32 for the GPU
        let center = [origin[0] as f32, 0.0, origin[2] as f32];
        // Buildings of this chunk and its neighbours: plants near a border keep out of those
        // across it too.
        let footprints: Vec<vegetation::Footprint> = (-1..=1)
            .flat_map(|de| (-1..=1).map(move |dn| (cx + de, cn + dn)))
            .filter_map(|key| buildings.get(&key))
            .flatten()
            .map(|plot| vegetation::Footprint::around(&plot.footprint))
            .collect();
        let ground = vegetation::Ground {
            heights: &heights,
            land: &land,
            road: &road,
            streets: &ways.clearance,
            buildings: &footprints,
        };
        let mut trees = vegetation::place(heights.origin, CHUNK_SIZE, &ground, origin);
        vegetation::place_grass(&mut trees, heights.origin, CHUNK_SIZE, &ground, origin);
        let (paved, mut unpaved) = streets::meshes(
            (streets, &ways.corners),
            heights.origin,
            CHUNK_SIZE,
            &heights,
            &below,
            origin,
        );
        unpaved.append(water::shore_mesh(
            pools,
            heights.origin,
            CHUNK_SIZE,
            &heights,
            origin,
        ));
        world.chunks.push(TerrainChunk {
            center,
            mesh: heights.mesh(&land, origin, &shapers, &ways.islands),
            buildings: chunk_buildings.shells,
            modelled: chunk_buildings.cells.into_values().collect(),
            streets: paved,
            tracks: unpaved,
            water: water::mesh(streams, pools, heights.origin, CHUNK_SIZE, &heights, origin),
            trees,
        });
        progress(done + 1, total);
    }
    if world.fallback_samples > 0 {
        warn!(
            samples = world.fallback_samples,
            "terrain data missing in places; terrain follows the road there"
        );
    }
    info!(
        chunks = world.chunks.len(),
        trees = world.chunks.iter().map(|c| c.trees.len()).sum::<usize>(),
        "world generated"
    );
    world
}

/// Buildings near the route, grouped by the chunk containing their first corner.
fn buildings_by_chunk<'a>(
    map: &'a MapData,
    projection: &LocalProjection,
    road: &RoadIndex,
    land: &LandIndex,
) -> HashMap<(i32, i32), Vec<buildings::Plot<'a>>> {
    let project = |points: &[(f64, f64)]| -> Vec<(f64, f64)> {
        points
            .iter()
            .map(|&(lat, lon)| projection.project(lat, lon))
            .collect()
    };
    let streets = map
        .roads
        .iter()
        .filter(|r| {
            r.structure.is_none()
                && matches!(
                    r.class,
                    RoadClass::Major | RoadClass::Street | RoadClass::Service
                )
        })
        .flat_map(|r| drape::densify(&project(&r.line), 3.0))
        .chain(road.samples(3.0));
    let frontage = buildings::Frontage::new(streets);
    let mut plots = Vec::new();
    for building in &map.buildings {
        let footprint = buildings::footprint(building, projection);
        let Some(&(east, north)) = footprint.first() else {
            continue;
        };
        // Roads stay clear (#100): buildings mapped across them (e.g. bad data) are left out,
        // those reaching into the road ridden by a corner or a wall as drawn (#138), and those
        // a street runs through.
        let mut outline = buildings::drawn_outline(&footprint);
        outline.push(outline[0]);
        let on_road = drape::densify(&outline, 2.0)
            .iter()
            .any(|&(e, n)| road.nearest(e, n, ROAD_HALF_WIDTH + 1.0).is_some());
        if on_road
            || frontage.runs_through(&footprint)
            || road.nearest(east, north, CORRIDOR).is_none()
        {
            continue;
        }
        let (e, n) = buildings::centroid(&footprint);
        let setting = if land.has(e, n, LandCover::Industrial) {
            buildings::Setting::Industrial
        } else if land.has(e, n, LandCover::Commercial) {
            buildings::Setting::Commercial
        } else if land.has(e, n, LandCover::Public) {
            buildings::Setting::Public
        } else if land.has(e, n, LandCover::Residential) {
            buildings::Setting::Town
        } else {
            buildings::Setting::Countryside
        };
        plots.push(buildings::Plot {
            building,
            footprint,
            setting,
            church: false,
            shop: None,
            purpose: None,
        });
    }
    buildings::mark_churches(&mut plots, &project(&map.churches));
    for (points, purpose) in [
        (&map.offices, buildings::Purpose::Office),
        (&map.hotels, buildings::Purpose::Hotel),
        (&map.public, buildings::Purpose::Public),
    ] {
        buildings::mark_purpose(&mut plots, &project(points), purpose);
    }
    buildings::mark_shops(&mut plots, &project(&map.shops), &frontage);

    let mut by_chunk: HashMap<_, Vec<_>> = HashMap::new();
    for plot in plots {
        let (east, north) = plot.footprint[0];
        by_chunk
            .entry(chunk_of(east, north))
            .or_default()
            .push(plot);
    }
    by_chunk
}

fn chunk_of(east: f64, north: f64) -> (i32, i32) {
    #[allow(clippy::cast_possible_truncation)] // world coordinates are far below 2^31 chunks
    (
        (east / CHUNK_SIZE).floor() as i32,
        (north / CHUNK_SIZE).floor() as i32,
    )
}

/// Chunk grid cells (east, north) within the corridor of any part of the route.
pub(crate) fn chunks_near_route(road: &RoadIndex) -> BTreeSet<(i32, i32)> {
    let mut cells = BTreeSet::new();
    let reach = CORRIDOR + CHUNK_SIZE / 2.0 * std::f64::consts::SQRT_2;
    // Sampling the road every half chunk is enough to touch every chunk in reach.
    for (east, north) in road.samples(CHUNK_SIZE / 2.0) {
        let (low, high) = (
            chunk_of(east - reach, north - reach),
            chunk_of(east + reach, north + reach),
        );
        for cx in low.0..=high.0 {
            for cn in low.1..=high.1 {
                let center_e = (f64::from(cx) + 0.5) * CHUNK_SIZE;
                let center_n = (f64::from(cn) + 0.5) * CHUNK_SIZE;
                if (center_e - east).hypot(center_n - north) <= reach {
                    cells.insert((cx, cn));
                }
            }
        }
    }
    cells
}

/// Terrain heights of one chunk: a vertex grid with a one-vertex border, and finer cells near
/// the road where the ground is shaped around it.
pub(crate) struct HeightGrid {
    /// South-west corner in metres east/north.
    origin: (f64, f64),
    /// Vertices per side, without the border.
    side: usize,
    /// Shaped heights of the grid's vertices, border included.
    heights: Vec<f64>,
    /// Natural heights of the grid's vertices, border included.
    natural: Vec<f64>,
    /// Cells split into `SUB` × `SUB` pieces: the shaped heights of their vertices, row by row
    /// from the south-west.
    fine: HashMap<(usize, usize), Vec<f64>>,
    /// How far channels cut the ground at each vertex (`channels`), coarse and fine as above;
    /// the heights include it.
    carve: Vec<f64>,
    fine_carve: HashMap<(usize, usize), Vec<f64>>,
}

/// What a strip or area is laid on (`drape`): the ground, or the water's surface in its channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum On {
    Ground,
    Water,
}

impl HeightGrid {
    #[allow(clippy::too_many_arguments)]
    async fn sample<M: ElevationModel>(
        cx: i32,
        cn: i32,
        projection: &LocalProjection,
        shapers: &Shapers<'_>,
        water: (&channels::Channels, &LandIndex),
        model: &mut M,
        world: &mut World,
    ) -> Self {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let side = (CHUNK_SIZE / GRID).round() as usize + 1;
        let bordered = side + 2;
        let origin = (f64::from(cx) * CHUNK_SIZE, f64::from(cn) * CHUNK_SIZE);
        let chunk_road_elevation = shapers
            .road
            .nearest(
                origin.0 + CHUNK_SIZE / 2.0,
                origin.1 + CHUNK_SIZE / 2.0,
                f64::INFINITY,
            )
            .map_or(0.0, |(_, elevation, _)| elevation);

        let mut natural = vec![0.0; bordered * bordered];
        let mut heights = vec![0.0; bordered * bordered];
        let mut carve = vec![0.0; bordered * bordered];
        for j in 0..bordered {
            for i in 0..bordered {
                #[allow(clippy::cast_precision_loss)] // small grid indices
                let (east, north) = (
                    origin.0 + (i as f64 - 1.0) * GRID,
                    origin.1 + (j as f64 - 1.0) * GRID,
                );
                let (lat, lon) = projection.unproject(east, north);
                let height = if let Ok(height) = model.elevation(lat, lon).await {
                    height
                } else {
                    world.fallback_samples += 1;
                    shapers
                        .road
                        .nearest(east, north, FALLBACK_RADIUS)
                        .map_or(chunk_road_elevation, |(_, elevation, _)| elevation)
                };
                let cut = carve_at(water, east, north, shapers);
                natural[j * bordered + i] = height;
                carve[j * bordered + i] = cut;
                heights[j * bordered + i] =
                    shape(height, &shapers.near(east, north, LEVEL_REACH)) - cut;
            }
        }
        let mut grid = Self {
            origin,
            side,
            heights,
            natural,
            fine: HashMap::new(),
            carve,
            fine_carve: HashMap::new(),
        };
        let half_diagonal = GRID * std::f64::consts::FRAC_1_SQRT_2;
        for j in 0..side - 1 {
            for i in 0..side - 1 {
                let (east, north) = grid.position(i, j, 0.5, 0.5);
                if grid.needs_detail(i, j, shapers) || water.0.near(east, north, half_diagonal) {
                    let (fine, cuts) = grid.detail(i, j, shapers, water);
                    grid.fine.insert((i, j), fine);
                    grid.fine_carve.insert((i, j), cuts);
                }
            }
        }
        grid
    }

    /// Whether the ground anywhere in cell (`i`, `j`) is shaped around the road. Elsewhere it
    /// is natural, and the plain cell's flat triangles match the neighbours' edges exactly.
    fn needs_detail(&self, i: usize, j: usize, shapers: &Shapers<'_>) -> bool {
        let half_diagonal = GRID * std::f64::consts::FRAC_1_SQRT_2;
        let (east, north) = self.position(i, j, 0.5, 0.5);
        // At a tunnel portal the cutting ends: the ground steps up to the hill there.
        if shapers
            .portals
            .iter()
            .any(|p| p.near(east, north, half_diagonal + FINE))
        {
            return true;
        }
        let roads: Vec<_> = shapers
            .near(east, north, LEVEL_REACH + half_diagonal)
            .into_iter()
            .filter(|r| r.2 != Surface::Tunnel)
            .collect();
        if roads.is_empty() {
            return false;
        }
        let nearest = roads.iter().map(|r| r.0).fold(f64::INFINITY, f64::min);
        let closest = (nearest - half_diagonal).max(0.0);
        if closest <= VERGE {
            return true;
        }
        // Natural ground within every road's cutting and embankment slopes is left as it is.
        let (low_road, high_road) = roads
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |a, r| {
                (a.0.min(r.1), a.1.max(r.1))
            });
        let corners = [(0, 0), (1, 0), (0, 1), (1, 1)].map(|(di, dj)| {
            #[allow(clippy::cast_possible_wrap)] // small grid indices
            self.natural_vertex(i as isize + di, j as isize + dj)
        });
        let (low, high) = corners
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |a, &h| {
                (a.0.min(h), a.1.max(h))
            });
        let room = closest - VERGE;
        high > low_road - ROAD_SINK + room * CUT_SLOPE
            || low < high_road - ROAD_SINK - room * FILL_SLOPE
    }

    /// The shaped heights of cell (`i`, `j`) split into `SUB` × `SUB` pieces.
    /// The shaped and carved heights of a cell's fine pieces' vertices, and the carve.
    fn detail(
        &self,
        i: usize,
        j: usize,
        shapers: &Shapers<'_>,
        water: (&channels::Channels, &LandIndex),
    ) -> (Vec<f64>, Vec<f64>) {
        let mut fine = Vec::with_capacity((SUB + 1) * (SUB + 1));
        let mut cuts = Vec::with_capacity((SUB + 1) * (SUB + 1));
        for b in 0..=SUB {
            for a in 0..=SUB {
                #[allow(clippy::cast_precision_loss)] // small counts
                let (u, v) = (a as f64 / SUB as f64, b as f64 / SUB as f64);
                let (east, north) = self.position(i, j, u, v);
                let cut = carve_at(water, east, north, shapers);
                fine.push(
                    shape(
                        self.natural_at(east, north),
                        &shapers.near(east, north, LEVEL_REACH),
                    ) - cut,
                );
                cuts.push(cut);
            }
        }
        (fine, cuts)
    }

    /// Metres east/north of the point (`u`, `v`) (0–1) of cell (`i`, `j`).
    fn position(&self, i: usize, j: usize, u: f64, v: f64) -> (f64, f64) {
        #[allow(clippy::cast_precision_loss)] // small grid indices
        (
            self.origin.0 + (i as f64 + u) * GRID,
            self.origin.1 + (j as f64 + v) * GRID,
        )
    }

    /// Shaped height at a grid vertex; `i`/`j` may be −1 or `side` (the border).
    /// The ground's triangles (corners in metres east/north) overlapping the rectangle
    /// `low`–`high`, as the mesh draws them: cells, or their fine pieces near the road, split
    /// along the south-west to north-east diagonal, clockwise seen from above.
    pub(crate) fn triangles(&self, low: (f64, f64), high: (f64, f64)) -> Vec<[(f64, f64); 3]> {
        // Cells (or pieces) of `size` from `start` that `from`–`to` touches, of `count`.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // clamped, small grid
        let touched = |from: f64, to: f64, start: f64, size: f64, count: usize| {
            let first = (((from - start) / size).floor().max(0.0) as usize).min(count - 1);
            let last = (((to - start) / size).floor().max(0.0) as usize).min(count - 1);
            first..=last
        };
        let cells = self.side - 1;
        let mut triangles = Vec::new();
        for j in touched(low.1, high.1, self.origin.1, GRID, cells) {
            for i in touched(low.0, high.0, self.origin.0, GRID, cells) {
                #[allow(clippy::cast_precision_loss)] // small grid
                let corner = (
                    self.origin.0 + i as f64 * GRID,
                    self.origin.1 + j as f64 * GRID,
                );
                let (size, count) = if self.fine.contains_key(&(i, j)) {
                    (FINE, SUB)
                } else {
                    (GRID, 1)
                };
                for row in touched(low.1, high.1, corner.1, size, count) {
                    for column in touched(low.0, high.0, corner.0, size, count) {
                        #[allow(clippy::cast_precision_loss)] // a few pieces
                        let sw = (
                            corner.0 + column as f64 * size,
                            corner.1 + row as f64 * size,
                        );
                        let (se, nw, ne) = (
                            (sw.0 + size, sw.1),
                            (sw.0, sw.1 + size),
                            (sw.0 + size, sw.1 + size),
                        );
                        triangles.push([sw, nw, ne]);
                        triangles.push([sw, ne, se]);
                    }
                }
            }
        }
        triangles
    }

    fn vertex(&self, i: isize, j: isize) -> f64 {
        self.heights[self.slot(i, j)]
    }

    fn natural_vertex(&self, i: isize, j: isize) -> f64 {
        self.natural[self.slot(i, j)]
    }

    fn slot(&self, i: isize, j: isize) -> usize {
        let bordered = self.side + 2;
        let clamp = |k: isize| usize::try_from(k + 1).unwrap_or(0).min(bordered - 1);
        clamp(j) * bordered + clamp(i)
    }

    /// Natural height anywhere in or around the chunk, between the grid's samples.
    fn natural_at(&self, east: f64, north: f64) -> f64 {
        let u = (east - self.origin.0) / GRID;
        let v = (north - self.origin.1) / GRID;
        let (i, j) = (u.floor(), v.floor());
        let (fu, fv) = (u - i, v - j);
        #[allow(clippy::cast_possible_truncation)] // clamped to the small grid by `slot`
        let (i, j) = (i as isize, j as isize);
        let bottom = self.natural_vertex(i, j) * (1.0 - fu) + self.natural_vertex(i + 1, j) * fu;
        let top =
            self.natural_vertex(i, j + 1) * (1.0 - fu) + self.natural_vertex(i + 1, j + 1) * fu;
        bottom * (1.0 - fv) + top * fv
    }

    /// The ground's height at any point of (or slightly around) the chunk, exactly as the mesh
    /// has it.
    /// The ground's height at a point, channels cut into it.
    pub(crate) fn at(&self, east: f64, north: f64) -> f64 {
        self.interpolate(east, north, &self.heights, &self.fine)
    }

    /// The height of what lies `on` the ground at a point: the ground, or the water's surface
    /// in its channel (the ground without the channel, less the channel's drop).
    pub(crate) fn level(&self, on: On, east: f64, north: f64) -> f64 {
        match on {
            On::Ground => self.at(east, north),
            On::Water => {
                self.at(east, north) + self.interpolate(east, north, &self.carve, &self.fine_carve)
                    - channels::DROP
            }
        }
    }

    /// A value given at the grid's vertices (`coarse`, and `fine` in detailed cells) at a point,
    /// on the triangles the mesh draws.
    fn interpolate(
        &self,
        east: f64,
        north: f64,
        coarse: &[f64],
        fine: &HashMap<(usize, usize), Vec<f64>>,
    ) -> f64 {
        let u = (east - self.origin.0) / GRID;
        let v = (north - self.origin.1) / GRID;
        // Points on the chunk's far edges belong to its last cells, not to the border.
        #[allow(clippy::cast_precision_loss)] // small grid
        let last = (self.side - 2) as f64;
        let cell = |w: f64| {
            let floor = w.floor();
            if (last + 1.0..=last + 1.0 + 1e-9).contains(&w) {
                last
            } else {
                floor
            }
        };
        let (i, j) = (cell(u), cell(v));
        let (fu, fv) = (u - i, v - j);
        #[allow(clippy::cast_possible_truncation)] // clamped to the small grid by `slot`
        let (i, j) = (i as isize, j as isize);
        if let (Ok(ci), Ok(cj)) = (usize::try_from(i), usize::try_from(j))
            && let Some(fine) = fine.get(&(ci, cj))
        {
            #[allow(clippy::cast_precision_loss)]
            let (su, sv) = (fu * SUB as f64, fv * SUB as f64);
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let (column, row) = (
                (su.floor() as usize).min(SUB - 1),
                (sv.floor() as usize).min(SUB - 1),
            );
            #[allow(clippy::cast_precision_loss)]
            let (pu, pv) = (su - column as f64, sv - row as f64);
            let piece = |column: usize, row: usize| fine[row * (SUB + 1) + column];
            return on_triangles(
                piece(column, row),
                piece(column + 1, row),
                piece(column, row + 1),
                piece(column + 1, row + 1),
                pu,
                pv,
            );
        }
        let vertex = |i: isize, j: isize| coarse[self.slot(i, j)];
        on_triangles(
            vertex(i, j),
            vertex(i + 1, j),
            vertex(i, j + 1),
            vertex(i + 1, j + 1),
            fu,
            fv,
        )
    }

    /// The ground's normal at a point, from the shaped surface itself, so it is the same on
    /// either side of chunk and cell edges.
    fn normal_at(&self, east: f64, north: f64, shapers: &Shapers<'_>) -> [f32; 3] {
        let height =
            |e: f64, n: f64| shape(self.natural_at(e, n), &shapers.near(e, n, LEVEL_REACH));
        let step = 1.0;
        let slope_east = (height(east + step, north) - height(east - step, north)) / (2.0 * step);
        let slope_north = (height(east, north + step) - height(east, north - step)) / (2.0 * step);
        unit([-slope_east, 1.0, slope_north])
    }

    /// The ground mesh relative to `origin`, coloured by land cover, with the roundabouts'
    /// `islands` raised on it.
    fn mesh(
        &self,
        land: &LandIndex,
        origin: [f64; 3],
        shapers: &Shapers<'_>,
        islands: &[roundabouts::Island],
    ) -> MeshData {
        let mut mesh = self.plain_mesh(land, origin, shapers);
        mesh.append(roundabouts::mesh(
            islands,
            self.origin,
            CHUNK_SIZE,
            self,
            origin,
        ));
        mesh
    }

    /// The ground mesh relative to `origin`, coloured by land cover.
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)] // f32 GPU data; small grid
    fn plain_mesh(&self, land: &LandIndex, origin: [f64; 3], shapers: &Shapers<'_>) -> MeshData {
        let side = self.side;
        let mut mesh = MeshData::default();
        let push = |mesh: &mut MeshData, east: f64, north: f64, height: f64| {
            mesh.vertices.push([
                (east - origin[0]) as f32,
                (height - origin[1]) as f32,
                (-north - origin[2]) as f32,
            ]);
            mesh.normals.push(self.normal_at(east, north, shapers));
            mesh.uvs.push([(east / GRID) as f32, (north / GRID) as f32]);
            mesh.colors
                .push(landcover::color(land.cover_at(east, north)));
            u32::try_from(mesh.vertices.len() - 1).expect("chunk fits u32")
        };
        // The plain grid's vertices, shared by its cells.
        let mut grid = Vec::with_capacity(side * side);
        for j in 0..side {
            for i in 0..side {
                let (east, north) = self.position(i, j, 0.0, 0.0);
                grid.push(push(
                    &mut mesh,
                    east,
                    north,
                    self.vertex(i as isize, j as isize),
                ));
            }
        }
        for j in 0..side - 1 {
            for i in 0..side - 1 {
                let (east, north) = self.position(i, j, 0.5, 0.5);
                let portals: Vec<&structures::Portal> = shapers
                    .portals
                    .iter()
                    .filter(|p| p.near(east, north, GRID))
                    .collect();
                // A piece's two triangles from its corners (index, [east, north, height]), but
                // none reaching into a tunnel's opening.
                let add = |mesh: &mut MeshData, corners: [(u32, [f64; 3]); 4]| {
                    let [sw, se, nw, ne] = corners;
                    for triangle in [[sw, nw, ne], [sw, ne, se]] {
                        if !in_opening(&portals, triangle.map(|c| c.1)) {
                            mesh.indices.extend(triangle.map(|c| c.0));
                        }
                    }
                };
                if let Some(fine) = self.fine.get(&(i, j)) {
                    let mut corners = Vec::with_capacity((SUB + 1) * (SUB + 1));
                    for b in 0..=SUB {
                        for a in 0..=SUB {
                            #[allow(clippy::cast_precision_loss)]
                            let (east, north) =
                                self.position(i, j, a as f64 / SUB as f64, b as f64 / SUB as f64);
                            let height = fine[b * (SUB + 1) + a];
                            corners.push((
                                push(&mut mesh, east, north, height),
                                [east, north, height],
                            ));
                        }
                    }
                    for b in 0..SUB {
                        for a in 0..SUB {
                            let at = |a: usize, b: usize| corners[b * (SUB + 1) + a];
                            add(
                                &mut mesh,
                                [at(a, b), at(a + 1, b), at(a, b + 1), at(a + 1, b + 1)],
                            );
                        }
                    }
                } else {
                    let at = |i: usize, j: usize| {
                        let (east, north) = self.position(i, j, 0.0, 0.0);
                        (
                            grid[j * side + i],
                            [east, north, self.vertex(i as isize, j as isize)],
                        )
                    };
                    add(
                        &mut mesh,
                        [at(i, j), at(i + 1, j), at(i, j + 1), at(i + 1, j + 1)],
                    );
                }
            }
        }
        mesh
    }
}

/// Whether a triangle of the ground (corners east, north, height) reaches into the opening of a
/// tunnel at one of `portals`: it is left out, so nothing closes the opening (#135).
fn in_opening(portals: &[&structures::Portal], [a, b, c]: [[f64; 3]; 3]) -> bool {
    if portals.is_empty() {
        return false;
    }
    let between = |p: [f64; 3], q: [f64; 3]| [0, 1, 2].map(|k| f64::midpoint(p[k], q[k]));
    let centre = [0, 1, 2].map(|k| (a[k] + b[k] + c[k]) / 3.0);
    let probes = [a, b, c, between(a, b), between(b, c), between(c, a), centre];
    portals
        .iter()
        .any(|portal| probes.iter().any(|&probe| portal.hollow(probe)))
}

/// How far channels cut the ground at a point (`channels::Channels::depth`).
fn carve_at(
    (channels, land): (&channels::Channels, &LandIndex),
    east: f64,
    north: f64,
    shapers: &Shapers<'_>,
) -> f64 {
    let inside = land.cover_at(east, north) == Some(LandCover::Water);
    if !inside && !channels.near(east, north, 0.0) {
        return 0.0;
    }
    channels.depth(east, north, inside, shapers)
}

/// Height at (`u`, `v`) (0–1) of a cell with corner heights south-west, south-east,
/// north-west and north-east, split into the two triangles the mesh draws (along the
/// south-west to north-east diagonal).
fn on_triangles(sw: f64, se: f64, nw: f64, ne: f64, u: f64, v: f64) -> f64 {
    if v >= u {
        sw + (ne - nw) * u + (nw - sw) * v
    } else {
        sw + (se - sw) * u + (ne - se) * v
    }
}

/// What shapes the ground: the road ridden and the railways (`railways`), each level across just
/// below it, with cuttings and embankments, and the portals of their tunnels, where those
/// cuttings end and the ground keeps out of the openings (#135).
pub(crate) struct Shapers<'a> {
    pub(crate) road: &'a RoadIndex,
    pub(crate) rails: &'a RoadIndex,
    pub(crate) portals: &'a [structures::Portal],
}

impl Shapers<'_> {
    /// Every piece of road or railway within `reach`, as [`RoadIndex::near`] gives them.
    pub(crate) fn near(&self, east: f64, north: f64, reach: f64) -> Vec<(f64, f64, Surface)> {
        let mut found = self.road.near(east, north, reach);
        found.extend(self.rails.near(east, north, reach));
        found
    }
}

/// The ground at a point with natural height `natural`, shaped around the pieces of road near
/// it (distance, road elevation, surface): level just below the road out to the verge, then
/// cut into the hillside or banked down to the valley at most as steeply as cuttings and
/// embankments are, natural again further away. Under bridges the ground is only lowered,
/// above tunnels never touched. Where the road passes more than once (hairpins), the ground
/// stays below every pass.
pub(crate) fn shape(natural: f64, roads: &[(f64, f64, Surface)]) -> f64 {
    let Some(&(distance, elevation, surface)) = roads
        .iter()
        .filter(|r| r.2 != Surface::Tunnel)
        .min_by(|a, b| a.0.total_cmp(&b.0))
    else {
        return natural;
    };
    let level = elevation - ROAD_SINK;
    let room = (distance - VERGE).max(0.0);
    let (floor, ceiling) = (level - room * FILL_SLOPE, level + room * CUT_SLOPE);
    let shaped = match surface {
        // The valley under a bridge stays open: only ground above the deck is cut away.
        Surface::Bridge => natural.min(ceiling),
        Surface::Ground | Surface::Tunnel => natural.clamp(floor, ceiling),
    };
    let mut height = shaped + (natural - shaped) * fade(distance);
    for &(distance, elevation, surface) in roads {
        if surface == Surface::Tunnel {
            continue;
        }
        let ceiling = elevation - ROAD_SINK + (distance - VERGE).max(0.0) * CUT_SLOPE;
        let allowed = ceiling + (natural - ceiling).max(0.0) * fade(distance);
        height = height.min(allowed);
    }
    height
}

/// 0 where the ground is fully shaped around the road, rising to 1 at `LEVEL_REACH`.
fn fade(distance: f64) -> f64 {
    let t = ((distance - (LEVEL_REACH - REACH_FADE)) / REACH_FADE).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[allow(clippy::cast_possible_truncation)]
fn unit(v: [f64; 3]) -> [f32; 3] {
    let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    [
        (v[0] / length) as f32,
        (v[1] / length) as f32,
        (v[2] / length) as f32,
    ]
}

/// A deterministic pseudo-random number in `[0, 1)` from a seed (`SplitMix64`), so the world
/// looks the same every time a route is loaded.
pub(crate) fn hash(seed: i64) -> f64 {
    #[allow(clippy::cast_sign_loss)] // bit reinterpretation
    let mut z = (seed as u64).wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    #[allow(clippy::cast_precision_loss)] // 53 significant bits are plenty
    let value = (z >> 11) as f64 / (1u64 << 53) as f64;
    value
}

#[cfg(test)]
mod tests;
