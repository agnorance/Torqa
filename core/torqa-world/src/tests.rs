// Test geometry compares f32 GPU data with exact, small reference values, reads small
// non-negative style codes from colours, and turns small counts into densities.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::float_cmp
)]

use std::fmt::Write as _;

use torqa_osm::{Area, Building, LandCover, Structure, StructureKind, Waterway};

use super::*;

const METERS_PER_DEGREE: f64 = 111_195.0;

/// Terrain rising 10 % towards the east, 500 m at the route start.
struct EastwardSlope;

impl ElevationModel for EastwardSlope {
    fn elevation(
        &mut self,
        _lat: f64,
        lon: f64,
    ) -> impl std::future::Future<Output = Result<f64, String>> + Send {
        let east = (lon - 7.0) * METERS_PER_DEGREE * 46f64.to_radians().cos();
        std::future::ready(Ok(500.0 + 0.1 * east))
    }
}

/// No terrain data at all, as when offline without cached tiles.
struct NoData;

impl ElevationModel for NoData {
    fn elevation(
        &mut self,
        _lat: f64,
        _lon: f64,
    ) -> impl std::future::Future<Output = Result<f64, String>> + Send {
        std::future::ready(Err("offline".to_owned()))
    }
}

/// Latitude/longitude of a point `east`/`north` metres from the route start.
fn at(east: f64, north: f64) -> (f64, f64) {
    (
        46.0 + north / METERS_PER_DEGREE,
        7.0 + east / (METERS_PER_DEGREE * 46f64.to_radians().cos()),
    )
}

/// A closed square ring around a point.
fn square(east: f64, north: f64, half: f64) -> Vec<(f64, f64)> {
    rectangle(east, north, half, half)
}

/// A closed rectangular ring around a point, `half_east` by `half_north` from it.
fn rectangle(east: f64, north: f64, half_east: f64, half_north: f64) -> Vec<(f64, f64)> {
    vec![
        at(east - half_east, north - half_north),
        at(east + half_east, north - half_north),
        at(east + half_east, north + half_north),
        at(east - half_east, north + half_north),
        at(east - half_east, north - half_north),
    ]
}

/// A 1 km flat road due north at 500 m.
async fn route_north(structures: &[Structure]) -> Route {
    let mut xml = String::from("<gpx><trk><trkseg>");
    for i in 0..=100 {
        let (lat, lon) = at(0.0, f64::from(i) * 10.0);
        let _ = write!(
            xml,
            r#"<trkpt lat="{lat}" lon="{lon}"><ele>500</ele></trkpt>"#
        );
    }
    xml.push_str("</trkseg></trk></gpx>");
    // The road ridden, with the structures on it.
    let mut roads = Vec::new();
    let mut from = at(0.0, -20.0);
    for structure in structures {
        roads.push(torqa_osm::Road {
            class: torqa_osm::RoadClass::Street,
            line: vec![from, structure.line[0]],
            structure: None,
        });
        roads.push(torqa_osm::Road {
            class: torqa_osm::RoadClass::Street,
            line: structure.line.clone(),
            structure: Some(structure.kind),
        });
        from = structure.line[structure.line.len() - 1];
    }
    roads.push(torqa_osm::Road {
        class: torqa_osm::RoadClass::Street,
        line: vec![from, at(0.0, 1020.0)],
        structure: None,
    });
    let map = MapData {
        roads,
        ..MapData::default()
    };
    Route::from_gpx_with::<EastwardSlope>(&xml, None, &map)
        .await
        .unwrap()
}

async fn world(map: &MapData) -> World {
    generate(
        &route_north(&[]).await,
        &mut EastwardSlope,
        map,
        &mut |_, _| {},
    )
    .await
}

/// All terrain vertices in absolute coordinates.
fn terrain_vertices(world: &World) -> impl Iterator<Item = [f32; 3]> + '_ {
    world.chunks.iter().flat_map(|c| {
        c.mesh
            .vertices
            .iter()
            .map(move |v| [v[0] + c.center[0], v[1], v[2] + c.center[2]])
    })
}

fn triangles(mesh: &MeshData) -> impl Iterator<Item = [[f32; 3]; 3]> + '_ {
    mesh.indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|t| t.map(|k| mesh.vertices[k as usize]))
}

/// Normal of a triangle by the right-hand rule.
fn face_normal([first, second, third]: [[f32; 3]; 3]) -> [f32; 3] {
    let u = [0, 1, 2].map(|k| second[k] - first[k]);
    let v = [0, 1, 2].map(|k| third[k] - first[k]);
    [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ]
}

fn assert_valid(mesh: &MeshData) {
    assert_eq!(mesh.vertices.len(), mesh.normals.len());
    assert_eq!(mesh.vertices.len(), mesh.uvs.len());
    assert!(mesh.colors.is_empty() || mesh.colors.len() == mesh.vertices.len());
    assert_eq!(mesh.indices.len() % 3, 0);
    assert!(
        mesh.indices
            .iter()
            .all(|&i| (i as usize) < mesh.vertices.len())
    );
}

#[tokio::test]
async fn terrain_surrounds_the_route() {
    let world = world(&MapData::default()).await;

    assert_ne!(world.chunks.len(), 0);
    // Every chunk is within the corridor; the route spans x = 0 and z = 0..−1000.
    for chunk in &world.chunks {
        let [x, _, z] = chunk.center;
        assert!(
            x.abs() < 2000.0 && (-2700.0..1700.0).contains(&z),
            "{x}, {z}"
        );
    }
    assert_eq!(world.fallback_samples, 0);
}

#[tokio::test]
async fn ground_and_road_face_up() {
    let world = world(&MapData::default()).await;

    for mesh in world.chunks.iter().map(|c| &c.mesh).chain([&world.road]) {
        assert_valid(mesh);
        // Godot draws clockwise triangles; seen from above, clockwise means a downward normal
        // by the right-hand rule.
        assert!(triangles(mesh).all(|t| face_normal(t)[1] < 0.0));
        assert!(mesh.normals.iter().all(|n| n[1] > 0.0));
    }
}

#[tokio::test]
async fn terrain_follows_the_model_away_from_the_road() {
    let world = world(&MapData::default()).await;

    let vertex = terrain_vertices(&world)
        .find(|v| (v[0] - 1000.0).abs() < 9.0 && (v[2] + 500.0).abs() < 9.0)
        .expect("a vertex 1 km east of the route");
    let expected = 500.0 + 0.1 * vertex[0];
    assert!(
        (vertex[1] - expected).abs() < 0.5,
        "{} vs {expected}",
        vertex[1]
    );
}

#[tokio::test]
async fn streets_lie_on_the_ground_however_it_folds() {
    use torqa_osm::{Road, RoadClass};

    // A wide street crossing the route on the hillside: through the cutting into it, over the
    // cutting's sharp top edge and on up the natural slope.
    let world = world(&MapData {
        roads: vec![Road {
            class: RoadClass::Major,
            line: vec![at(-150.0, 300.0), at(150.0, 300.0)],
            structure: None,
        }],
        ..MapData::default()
    })
    .await;

    let mut checked = 0;
    for step in 0..1200 {
        #[allow(clippy::cast_precision_loss)] // small steps
        let x = -150.0 + step as f32 * 0.25;
        for offset in [-3.3_f32, -1.5, 0.0, 1.5, 3.3] {
            let z = -300.0 + offset;
            let (Some(ground), Some(street)) = (ground_at(&world, x, z), street_at(&world, x, z))
            else {
                continue;
            };
            assert!(
                street > ground,
                "the ground through the street at {x}, {z}: {ground} over {street}"
            );
            checked += 1;
        }
    }
    assert!(checked > 5000, "{checked} points checked");
}

#[tokio::test]
async fn where_streets_join_the_road_its_edge_is_road_not_shoulder() {
    use torqa_osm::{Road, RoadClass};

    // A street joining from the west at 300 m, none from the east.
    let world = world(&MapData {
        roads: vec![Road {
            class: RoadClass::Street,
            line: vec![at(-150.0, 300.0), at(0.0, 300.0)],
            structure: None,
        }],
        ..MapData::default()
    })
    .await;

    let road = &world.road;
    let rings = road.uvs.as_chunks::<6>().0;
    let bevels = |near: f32| -> Vec<(f32, f32)> {
        rings
            .iter()
            .filter(|ring| (ring[1][1] - near).abs() < 1.0)
            .map(|ring| (ring[1][0], ring[4][0]))
            .collect()
    };
    for (left, right) in bevels(300.0) {
        // Road (u within 0–1) on the left where the street joins, shoulder on the right.
        assert!(
            (0.0..=1.0).contains(&left),
            "left bevel {left} at the junction"
        );
        assert!(right > 1.0, "right bevel {right} at the junction");
    }
    for (left, right) in bevels(600.0) {
        assert!(
            left < 0.0 && right > 1.0,
            "bevels {left}, {right} away from junctions"
        );
    }
}

#[tokio::test]
async fn streets_meet_with_rounded_kerbs_and_end_round() {
    use torqa_osm::{Road, RoadClass};

    let street = |points: &[(f64, f64)]| Road {
        class: RoadClass::Street,
        line: points.iter().map(|&(e, n)| at(e, n)).collect(),
        structure: None,
    };
    // A street east–west at 300 m north, and one from the south ending on it at 160 m east
    // (sharing its point there, as the map's streets do); 5.5 m wide, away from the route.
    let world = world(&MapData {
        roads: vec![
            street(&[(60.0, 300.0), (160.0, 300.0), (260.0, 300.0)]),
            street(&[(160.0, 200.0), (160.0, 300.0)]),
        ],
        ..MapData::default()
    })
    .await;
    let paved = |east: f64, north: f64| street_at(&world, east as f32, -north as f32).is_some();

    // Both corners of the T have a kerb: the corner by the streets' edges (2.75 m out) is
    // paved, the grass beyond the kerb's curve is not.
    for side in [-1.0, 1.0] {
        let edge = 160.0 + side * 2.75;
        assert!(
            paved(edge + side * 0.5, 300.0 - 2.75 - 0.5),
            "corner {side}"
        );
        assert!(
            !paved(edge + side * 2.0, 300.0 - 2.75 - 2.0),
            "beyond the kerb {side}"
        );
    }
    // None across the street, where nothing joins.
    assert!(!paved(160.0 + 3.25, 300.0 + 3.25));
    // The dead end at the south is round, not square.
    assert!(paved(160.0, 200.0 - 1.5));
    assert!(!paved(160.0 + 2.5, 200.0 - 2.0));
}

#[tokio::test]
async fn streets_meeting_the_road_ridden_get_kerbs_at_its_edge() {
    use torqa_osm::{Road, RoadClass};

    let street = |points: &[(f64, f64)]| Road {
        class: RoadClass::Street,
        line: points.iter().map(|&(e, n)| at(e, n)).collect(),
        structure: None,
    };
    // The road ridden as the map has it, and a street from the west ending on it at 600 m.
    let world = world(&MapData {
        roads: vec![
            street(&[(0.0, -20.0), (0.0, 600.0), (0.0, 1020.0)]),
            street(&[(-150.0, 600.0), (0.0, 600.0)]),
        ],
        ..MapData::default()
    })
    .await;
    let paved = |east: f64, north: f64| street_at(&world, east as f32, -north as f32).is_some();

    // The road is 6 m wide: its corners with the street, off its edge, are paved.
    for side in [-1.0, 1.0] {
        assert!(
            paved(-3.0 - 0.6, 600.0 + side * (2.75 + 0.6)),
            "corner {side}"
        );
        assert!(
            !paved(-3.0 - 2.5, 600.0 + side * (2.75 + 2.5)),
            "beyond {side}"
        );
    }
    // The other side of the road, where nothing joins, keeps its plain edge.
    assert!(!paved(3.6, 600.0 + 3.35));
}

/// The railways' bed along its middle, as the mesh has it: (x, height, z) per point.
fn rail_bed(world: &World) -> Vec<[f32; 3]> {
    world
        .railways
        .vertices
        .as_chunks::<6>()
        .0
        .iter()
        .map(|ring| [0, 1, 2].map(|k| f32::midpoint(ring[2][k], ring[3][k])))
        .collect()
}

/// A railway of the map through `points` (metres east/north).
fn railway(points: &[(f64, f64)], structure: Option<StructureKind>) -> torqa_osm::Railway {
    torqa_osm::Railway {
        line: points.iter().map(|&(e, n)| at(e, n)).collect(),
        structure,
        funicular: false,
    }
}

#[tokio::test]
async fn railways_run_smoothly_level_across_and_keep_the_forest_off() {
    // Bumpy land rising eastwards; a line due north across it, through a forest.
    struct Bumpy;
    impl ElevationModel for Bumpy {
        fn elevation(
            &mut self,
            lat: f64,
            lon: f64,
        ) -> impl std::future::Future<Output = Result<f64, String>> + Send {
            let east = (lon - 7.0) * METERS_PER_DEGREE * 46f64.to_radians().cos();
            let north = (lat - 46.0) * METERS_PER_DEGREE;
            std::future::ready(Ok(500.0 + 0.1 * east + 3.0 * (north / 25.0).sin()))
        }
    }
    let forest = Area {
        cover: LandCover::Forest,
        outer: vec![square(300.0, 500.0, 120.0)],
        inner: vec![],
    };
    let map = MapData {
        areas: vec![forest],
        railways: vec![railway(&[(300.0, 100.0), (300.0, 900.0)], None)],
        ..MapData::default()
    };
    let world = generate(&route_north(&[]).await, &mut Bumpy, &map, &mut |_, _| {}).await;

    let bed = rail_bed(&world);
    assert!(bed.len() > 100, "{} points of track", bed.len());
    // Gentle grades, no bumps: the terrain's 3 m waves are gone.
    for pair in bed.windows(2) {
        let run = (pair[1][2] - pair[0][2]).abs().max(1.0);
        let grade = (pair[1][1] - pair[0][1]).abs() / run;
        assert!(grade < 0.041, "a {grade} grade at {:?}", pair[0]);
    }
    // Level across, the ground shaped just below the bed beside it.
    for ring in world.railways.vertices.as_chunks::<6>().0 {
        assert!(
            (ring[2][1] - ring[3][1]).abs() < 0.01,
            "a tilted bed at {:?}",
            ring[2]
        );
    }
    for point in bed.iter().step_by(10) {
        for side in [-2.5_f32, 2.5] {
            let ground = ground_at(&world, point[0] + side, point[2]).expect("ground");
            assert!(
                ground < point[1] && point[1] - ground < 0.5,
                "ground at {ground} beside the bed at {point:?}"
            );
        }
    }
    // The forest grows beside the line, never on it.
    let trees = plants_of(&world, &["conifer", "broadleaf"]);
    assert!(
        trees
            .iter()
            .filter(|t| (t[0] - 300.0).abs() < 100.0)
            .count()
            > 50
    );
    for [x, _, z] in &trees {
        assert!((x - 300.0).abs() > 1.6, "a tree on the railway at {x}, {z}");
    }
}

#[tokio::test]
async fn the_land_reaches_from_the_detailed_ground_to_the_horizon() {
    // Land rising eastwards, and beyond 6 km east a lake whose surface the model reads at 600 m.
    struct Far;
    impl ElevationModel for Far {
        fn elevation(
            &mut self,
            _lat: f64,
            lon: f64,
        ) -> impl std::future::Future<Output = Result<f64, String>> + Send {
            let east = (lon - 7.0) * METERS_PER_DEGREE * 46f64.to_radians().cos();
            std::future::ready(Ok(if east > 6000.0 {
                600.0
            } else {
                500.0 + 0.01 * east.abs()
            }))
        }
    }
    let route = route_north(&[]).await;
    let world = generate(&route, &mut Far, &MapData::default(), &mut |_, _| {}).await;
    let land = horizon(&route, &mut Far).await;

    assert_valid(&land.ground);
    assert_valid(&land.water);
    let detailed = |x: f32, z: f32| {
        world.chunks.iter().any(|c| {
            (x - c.center[0]).abs() < CHUNK_SIZE as f32 / 2.0
                && (z - c.center[2]).abs() < CHUNK_SIZE as f32 / 2.0
        })
    };
    for cell in land.ground.vertices.as_chunks::<4>().0 {
        let middle = [0, 2].map(|k| cell.iter().map(|v| v[k]).sum::<f32>() / 4.0);
        assert!(
            !detailed(middle[0], middle[1]),
            "land over the detailed ground at {middle:?}"
        );
    }
    // Out to the horizon, west and east, at the land's height (a little lower).
    let reach = land
        .ground
        .vertices
        .iter()
        .map(|v| v[0])
        .fold(0.0_f32, f32::min);
    assert!(
        reach < -(HORIZON as f32) + 1000.0,
        "the land ends at {reach} m"
    );
    for v in &land.ground.vertices {
        let measured = 500.0 + 0.01 * v[0].abs();
        // On the shore the model may read the lake already.
        let shore = (v[0] - 6000.0).abs() < 1.0 && (v[1] - 598.0).abs() < 0.5;
        assert!(
            shore || (v[1] - (measured - 2.0)).abs() < 0.5,
            "land at {} at {}",
            v[1],
            v[0]
        );
    }
    // The lake far east is water, at its level.
    assert_ne!(land.water.vertices.len(), 0);
    assert!(
        land.water
            .vertices
            .iter()
            .all(|v| v[0] >= 6000.0 && (v[1] - 600.02).abs() < 0.01)
    );
    assert!(land.ground.vertices.iter().all(|v| v[0] <= 6240.0));
}

#[tokio::test]
async fn roundabouts_get_a_raised_island_with_a_kerb() {
    use torqa_osm::{Road, RoadClass};

    // A roundabout of 20 m radius east of the route; a long thin loop, a road round a square
    // block and a footpath round a pond (no roundabouts).
    let ring: Vec<(f64, f64)> = (0..=24)
        .map(|k| {
            let angle = std::f64::consts::TAU * f64::from(k) / 24.0;
            at(200.0 + 20.0 * angle.cos(), 500.0 + 20.0 * angle.sin())
        })
        .collect();
    let loop_ = vec![
        at(150.0, 800.0),
        at(250.0, 800.0),
        at(250.0, 806.0),
        at(200.0, 807.0),
        at(150.0, 806.0),
        at(150.0, 800.0),
    ];
    // The block's road has a point every 10 m, as mapped roads do.
    let block: Vec<(f64, f64)> = (0..=20)
        .map(|k| {
            let (side, step) = (k / 5, f64::from(k % 5) * 10.0);
            let (east, north) = match side {
                0 => (-25.0 + step, -25.0),
                1 => (25.0, -25.0 + step),
                2 => (25.0 - step, 25.0),
                3 => (-25.0, 25.0 - step),
                _ => (-25.0, -25.0),
            };
            at(200.0 + east, 300.0 + north)
        })
        .collect();
    let path: Vec<(f64, f64)> = (0..=24)
        .map(|k| {
            let angle = std::f64::consts::TAU * f64::from(k) / 24.0;
            at(200.0 + 15.0 * angle.cos(), 650.0 + 15.0 * angle.sin())
        })
        .collect();
    let world = world(&MapData {
        roads: [
            (RoadClass::Street, ring),
            (RoadClass::Street, loop_),
            (RoadClass::Service, block),
            (RoadClass::Path, path),
        ]
        .into_iter()
        .map(|(class, line)| Road {
            class,
            line,
            structure: None,
        })
        .collect(),
        ..MapData::default()
    })
    .await;
    // How far the topmost ground lies over the natural slope (0.1 m per metre east).
    let lift = |x: f32, z: f32| top_of_ground(&world, x, z).map(|top| top - (500.0 + 0.1 * x));

    // Inside the ring, the island stands 20 cm up; on the ring and outside, the ground is as
    // it was.
    for (x, z) in [
        (200.0, -500.0),
        (210.0, -505.0),
        (190.0, -492.0),
        (214.0, -500.0),
    ] {
        let up = lift(x, z).expect("ground");
        assert!((up - 0.2).abs() < 0.02, "island at {up} m at {x}, {z}");
    }
    for (x, z) in [
        (220.0, -500.0),
        (230.0, -500.0),
        (200.0, -525.0),
        (200.0, -803.0),
        (200.0, -300.0),
        (210.0, -290.0),
        (200.0, -650.0),
    ] {
        let up = lift(x, z).expect("ground");
        assert!(up.abs() < 0.02, "ground raised by {up} m at {x}, {z}");
    }
    // Its kerb: upright faces all round the island.
    let kerb = world
        .chunks
        .iter()
        .flat_map(|c| triangles(&c.mesh).map(move |t| (t, c.center)))
        .filter(|(t, centre)| {
            let normal = face_normal(*t);
            let middle = [0, 2].map(|k| t.iter().map(|v| v[k]).sum::<f32>() / 3.0);
            let (x, z) = (middle[0] + centre[0], middle[1] + centre[2]);
            normal[1].abs() < 0.01 && ((x - 200.0).hypot(z + 500.0) - 16.6).abs() < 1.0
        })
        .count();
    assert!(kerb >= 40, "{kerb} kerb faces");
}

#[tokio::test]
async fn railways_tunnel_through_hills_rather_than_climb_them() {
    // A hill 40 m high in the line's way.
    struct Hill;
    impl ElevationModel for Hill {
        fn elevation(
            &mut self,
            lat: f64,
            _lon: f64,
        ) -> impl std::future::Future<Output = Result<f64, String>> + Send {
            let north = (lat - 46.0) * METERS_PER_DEGREE;
            std::future::ready(Ok(500.0 + (40.0 - 0.3 * (north - 500.0).abs()).max(0.0)))
        }
    }
    let map = MapData {
        railways: vec![railway(&[(300.0, 100.0), (300.0, 900.0)], None)],
        ..MapData::default()
    };
    let world = generate(&route_north(&[]).await, &mut Hill, &map, &mut |_, _| {}).await;

    // The track stays low...
    let top = rail_bed(&world)
        .iter()
        .map(|p| p[1])
        .fold(f32::MIN, f32::max);
    assert!(top < 520.0, "the track climbs to {top}");
    // ...under the hill, which stays as it is, in a tunnel.
    let hill = ground_at(&world, 300.0, -500.0).expect("ground");
    assert!(hill > 535.0, "the hill cut down to {hill}");
    let tube = world
        .structures
        .vertices
        .iter()
        .filter(|v| (v[0] - 300.0).abs() < 6.0 && (v[2] + 500.0).abs() < 20.0)
        .count();
    assert!(tube > 10, "no tunnel under the hill");
    // It enters the hill through portals: a headwall at either end of the tube, facing out
    // along the line (#135).
    let faces: Vec<_> = world
        .structures
        .vertices
        .iter()
        .zip(&world.structures.normals)
        .filter(|(v, _)| (v[0] - 300.0).abs() < 6.0)
        .collect();
    let (south, north) = faces
        .iter()
        .filter(|(_, n)| n[2].abs() < 0.01)
        .fold((f32::MAX, f32::MIN), |(low, high), (v, _)| {
            (low.min(-v[2]), high.max(-v[2]))
        });
    for (end, facing) in [(south, 1.0_f32), (north, -1.0)] {
        let wall = faces
            .iter()
            .any(|(v, n)| n[2] * facing > 0.99 && (v[2] + end).abs() < 0.5);
        assert!(wall, "no headwall at the tunnel's end at {end} m");
    }
}

#[tokio::test]
async fn railway_tunnels_under_the_road_stay_below_it() {
    // Flat land 3 m below the road ridden (500 m), which crosses it on an embankment, and a line
    // passing under it at 500 m in a tunnel 240 m long: its 5 m arch would show through the
    // road and the embankment (#138).
    struct Low;
    impl ElevationModel for Low {
        fn elevation(
            &mut self,
            _lat: f64,
            _lon: f64,
        ) -> impl std::future::Future<Output = Result<f64, String>> + Send {
            std::future::ready(Ok(497.0))
        }
    }
    let map = MapData {
        railways: vec![
            railway(&[(-300.0, 500.0), (-120.0, 500.0)], None),
            railway(
                &[(-120.0, 500.0), (120.0, 500.0)],
                Some(StructureKind::Tunnel),
            ),
            railway(&[(120.0, 500.0), (300.0, 500.0)], None),
        ],
        ..MapData::default()
    };
    let world = generate(&route_north(&[]).await, &mut Low, &map, &mut |_, _| {}).await;

    let tube: Vec<_> = world
        .structures
        .vertices
        .iter()
        .filter(|v| (v[2] + 500.0).abs() < 8.0)
        .collect();
    // Nothing of it on or beside the road...
    let through = tube
        .iter()
        .filter(|v| v[0].abs() < 8.0)
        .map(|v| v[1])
        .fold(f32::MIN, f32::max);
    assert!(
        through < 500.0 - 0.1,
        "the tunnel reaches {through} at the road"
    );
    // ...while away from it the arch keeps its height.
    let arch = tube
        .iter()
        .filter(|v| v[0].abs() > 80.0)
        .map(|v| v[1])
        .fold(f32::MIN, f32::max);
    assert!(
        arch > 497.0 + 4.5,
        "the arch only reaches {arch} away from the road"
    );
}

#[tokio::test]
async fn railway_bridges_clear_the_road_and_meet_their_track() {
    // A line crossing the route at 500 m, on a bridge 60 m long over it.
    let map = MapData {
        railways: vec![
            railway(&[(-300.0, 500.0), (-30.0, 500.0)], None),
            railway(
                &[(-30.0, 500.0), (30.0, 500.0)],
                Some(StructureKind::Bridge),
            ),
            railway(&[(30.0, 500.0), (300.0, 500.0)], None),
        ],
        ..MapData::default()
    };
    let world = world(&map).await;

    let bed = rail_bed(&world);
    let at_x = |x: f32| {
        bed.iter()
            .min_by(|a, b| (a[0] - x).abs().total_cmp(&(b[0] - x).abs()))
            .map(|p| p[1])
            .expect("track")
    };
    // Over the road (500 m), high enough to pass under.
    assert!(
        at_x(0.0) > 500.0 + 5.5,
        "the bridge at {} over the road",
        at_x(0.0)
    );
    // One line: no step anywhere, the approaches ramping up gently.
    for pair in bed.windows(2) {
        let run = (pair[1][0] - pair[0][0]).abs().max(1.0);
        let grade = (pair[1][1] - pair[0][1]).abs() / run;
        assert!(grade < 0.041, "a {grade} step at {:?}", pair[0]);
    }
    // The bridge is a structure, standing on the ground beside the road.
    let deck = world
        .structures
        .vertices
        .iter()
        .filter(|v| v[0].abs() < 30.0 && (v[2] + 500.0).abs() < 5.0)
        .count();
    assert!(deck > 10, "no bridge over the road");
}

/// The triangles of a chunked mesh in absolute coordinates.
fn chunk_triangles<'a>(
    world: &'a World,
    mesh: impl Fn(&'a TerrainChunk) -> &'a MeshData + 'a,
) -> impl Iterator<Item = [[f32; 3]; 3]> + 'a {
    world.chunks.iter().flat_map(move |chunk| {
        triangles(mesh(chunk))
            .map(|t| t.map(|v| [v[0] + chunk.center[0], v[1], v[2] + chunk.center[2]]))
    })
}

#[tokio::test]
async fn bridges_over_the_road_keep_it_clear() {
    // The road in a valley, shallow but for a deep stretch at 800 m; crossing it, a short, low
    // railway bridge at 300 m whose arches' pier would stand on it, a long one at 600 m whose
    // pier would, and at 800 m a street bridge whose pier would (#98).
    struct Valley;
    impl ElevationModel for Valley {
        fn elevation(
            &mut self,
            lat: f64,
            lon: f64,
        ) -> impl std::future::Future<Output = Result<f64, String>> + Send {
            let east = (lon - 7.0) * METERS_PER_DEGREE * 46f64.to_radians().cos();
            let north = (lat - 46.0) * METERS_PER_DEGREE;
            let slope = if (north - 800.0).abs() < 60.0 {
                0.5
            } else {
                0.15
            };
            std::future::ready(Ok(500.0 + slope * east.abs()))
        }
    }
    let line = |from: f64, to: f64, north: f64| {
        vec![
            railway(&[(-300.0, north), (from, north)], None),
            railway(&[(from, north), (to, north)], Some(StructureKind::Bridge)),
            railway(&[(to, north), (300.0, north)], None),
        ]
    };
    let map = MapData {
        railways: [line(-13.0, 27.0, 300.0), line(-75.0, 45.0, 600.0)].concat(),
        roads: vec![torqa_osm::Road {
            class: torqa_osm::RoadClass::Street,
            line: vec![at(-30.0, 800.0), at(20.0, 800.0)],
            structure: Some(StructureKind::Bridge),
        }],
        ..MapData::default()
    };
    let world = generate(&route_north(&[]).await, &mut Valley, &map, &mut |_, _| {}).await;

    // Nothing reaches down onto the road: no pier, wall or arch.
    let on_road = |t: &[[f32; 3]; 3]| {
        let middle = (t[0][0] + t[1][0] + t[2][0]) / 3.0;
        middle.abs() < ROAD_HALF_WIDTH as f32 && t.iter().any(|v| v[1] < 502.0)
    };
    let structures: Vec<_> = triangles(&world.structures).collect();
    for north in [300.0_f32, 600.0] {
        let bridge: Vec<_> = structures
            .iter()
            .filter(|t| (t[0][2] + north).abs() < 10.0)
            .collect();
        assert!(bridge.len() > 20, "no bridge at {north} m");
        assert!(
            !bridge.iter().any(|t| on_road(t)),
            "the bridge at {north} m stands on the road"
        );
        // It still stands on piers either side.
        assert!(
            bridge.iter().any(|t| t.iter().any(|v| v[1] < 502.0)),
            "the bridge at {north} m has no piers"
        );
    }
    let street_piers: Vec<_> = chunk_triangles(&world, |c| &c.streets)
        .filter(|t| (t[0][2] + 800.0).abs() < 10.0 && t.iter().any(|v| v[1] < 502.0))
        .collect();
    assert!(!street_piers.is_empty(), "the street bridge has no piers");
    assert!(
        !street_piers.iter().any(on_road),
        "the street bridge stands on the road"
    );
}

#[tokio::test]
async fn parallel_tracks_share_one_tunnel_and_one_bridge() {
    // Two tracks 4.5 m apart, mapped each on its own, through a hill at 300 m and over a
    // bridge across a valley at 700 m (#99).
    struct HillAndValley;
    impl ElevationModel for HillAndValley {
        fn elevation(
            &mut self,
            lat: f64,
            _lon: f64,
        ) -> impl std::future::Future<Output = Result<f64, String>> + Send {
            let north = (lat - 46.0) * METERS_PER_DEGREE;
            let hill = (40.0 - 0.3 * (north - 300.0).abs()).max(0.0);
            let valley = (15.0 - 0.3 * (north - 700.0).abs()).max(0.0);
            std::future::ready(Ok(500.0 + hill - valley))
        }
    }
    let track = |east: f64| {
        vec![
            railway(&[(east, 0.0), (east, 650.0)], None),
            railway(&[(east, 650.0), (east, 750.0)], Some(StructureKind::Bridge)),
            railway(&[(east, 750.0), (east, 1000.0)], None),
        ]
    };
    let map = MapData {
        railways: [track(300.0), track(304.5)].concat(),
        ..MapData::default()
    };
    let world = generate(
        &route_north(&[]).await,
        &mut HillAndValley,
        &map,
        &mut |_, _| {},
    )
    .await;

    // One height: the tracks lie level with each other.
    let bed = rail_bed(&world);
    let height = |east: f32, north: f32| {
        bed.iter()
            .filter(|p| (p[0] - east).abs() < 1.0)
            .min_by(|a, b| (a[2] + north).abs().total_cmp(&(b[2] + north).abs()))
            .map(|p| p[1])
            .expect("track")
    };
    for k in 1..20 {
        let north = k as f32 * 50.0;
        let (a, b) = (height(300.0, north), height(304.5, north));
        assert!(
            (a - b).abs() < 0.05,
            "at {north} m the tracks lie at {a} and {b}"
        );
    }
    // A tunnel and a bridge, neither standing on a track: no wall or parapet between them.
    let structures: Vec<_> = triangles(&world.structures).collect();
    for north in [300.0_f32, 700.0] {
        let here: Vec<_> = structures
            .iter()
            .filter(|t| (t[0][2] + north).abs() < 20.0)
            .collect();
        assert!(here.len() > 20, "no structure at {north} m");
        let track = height(300.0, north);
        let on_a_track = here.iter().any(|t| {
            let middle = [0, 1, 2].map(|k| (t[0][k] + t[1][k] + t[2][k]) / 3.0);
            let beside = |east: f32| (middle[0] - east).abs() < railways::BED_M as f32 / 2.0 + 0.3;
            (beside(300.0) || beside(304.5)) && (track - 0.5..track + 3.0).contains(&middle[1])
        });
        assert!(!on_a_track, "the structure at {north} m stands on a track");
    }
}

#[tokio::test]
async fn the_road_bevels_gently_down_to_a_level_verge() {
    let world = world(&MapData::default()).await;

    let near_road: Vec<_> = terrain_vertices(&world)
        .filter(|v| v[0].abs() <= 6.5 && (-1000.0..0.0).contains(&v[2]))
        .collect();
    assert!(near_road.len() > 100);
    for v in near_road {
        assert!((v[1] - (500.0 - ROAD_SINK as f32)).abs() < 0.01, "{v:?}");
        // Just below the road: a low step, never a wall.
        assert!(500.0 - v[1] < 0.16, "{v:?}");
    }
    // The road's surface is at the route's 500 m; from its edges a bevel runs down, gentler
    // than 45°, to below the verge.
    for ring in world.road.vertices.as_chunks::<6>().0 {
        let (bevel, edge) = (ring[1], ring[2]);
        assert!((ring[2][1] - 500.0).abs() < 0.01 && (ring[3][1] - 500.0).abs() < 0.01);
        let drop = edge[1] - bevel[1];
        let out = (edge[0] - bevel[0]).hypot(edge[2] - bevel[2]);
        assert!(
            drop > ROAD_SINK as f32,
            "the bevel ends above the verge: {drop}"
        );
        assert!(
            drop < out,
            "a bevel steeper than 45°: {drop} m over {out} m"
        );
    }
}

/// The height of the topmost ground at (`x`, `z`): roundabouts' islands lie over the plain
/// ground.
fn top_of_ground(world: &World, x: f32, z: f32) -> Option<f32> {
    let chunk = world.chunks.iter().find(|c| {
        (x - c.center[0]).abs() <= CHUNK_SIZE as f32 / 2.0
            && (z - c.center[2]).abs() <= CHUNK_SIZE as f32 / 2.0
    })?;
    let (px, pz) = (x - chunk.center[0], z - chunk.center[2]);
    triangles(&chunk.mesh)
        .filter_map(|[a, b, c]| {
            let det = (b[2] - c[2]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[2] - c[2]);
            if det.abs() < 1e-9 {
                return None;
            }
            let wa = ((b[2] - c[2]) * (px - c[0]) + (c[0] - b[0]) * (pz - c[2])) / det;
            let wb = ((c[2] - a[2]) * (px - c[0]) + (a[0] - c[0]) * (pz - c[2])) / det;
            let wc = 1.0 - wa - wb;
            (wa >= -1e-4 && wb >= -1e-4 && wc >= -1e-4).then(|| wa * a[1] + wb * b[1] + wc * c[1])
        })
        .reduce(f32::max)
}

/// The terrain's height at (`x`, `z`) as its mesh has it.
fn ground_at(world: &World, x: f32, z: f32) -> Option<f32> {
    height_on(world, |c| &c.mesh, x, z)
}

/// The height at (`x`, `z`) of the other streets, as their meshes have it.
fn street_at(world: &World, x: f32, z: f32) -> Option<f32> {
    height_on(world, |c| &c.streets, x, z)
}

/// The height at (`x`, `z`) of a chunk mesh `pick` chooses.
fn height_on(
    world: &World,
    pick: fn(&crate::TerrainChunk) -> &MeshData,
    x: f32,
    z: f32,
) -> Option<f32> {
    let chunk = world.chunks.iter().find(|c| {
        (x - c.center[0]).abs() <= CHUNK_SIZE as f32 / 2.0
            && (z - c.center[2]).abs() <= CHUNK_SIZE as f32 / 2.0
    })?;
    let (px, pz) = (x - chunk.center[0], z - chunk.center[2]);
    triangles(pick(chunk)).find_map(|[a, b, c]| {
        let det = (b[2] - c[2]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[2] - c[2]);
        if det.abs() < 1e-9 {
            return None;
        }
        let wa = ((b[2] - c[2]) * (px - c[0]) + (c[0] - b[0]) * (pz - c[2])) / det;
        let wb = ((c[2] - a[2]) * (px - c[0]) + (a[0] - c[0]) * (pz - c[2])) / det;
        let wc = 1.0 - wa - wb;
        (wa >= -1e-4 && wb >= -1e-4 && wc >= -1e-4).then(|| wa * a[1] + wb * b[1] + wc * c[1])
    })
}

#[tokio::test]
async fn riders_ride_on_the_road_drawn_through_bends() {
    // North, a hairpin of 15 m radius, back south: the road is a smooth curve there.
    let mut xml = String::from("<gpx><trk><trkseg>");
    let mut points: Vec<(f64, f64)> = (0..=10).map(|i| (0.0, f64::from(i) * 10.0)).collect();
    points.extend((1..12).map(|k| {
        let angle = std::f64::consts::PI * f64::from(k) / 12.0;
        (15.0 - 15.0 * angle.cos(), 100.0 + 15.0 * angle.sin())
    }));
    points.extend((0..=10).rev().map(|i| (30.0, f64::from(i) * 10.0)));
    for &(east, north) in &points {
        let (lat, lon) = at(east, north);
        let _ = write!(
            xml,
            r#"<trkpt lat="{lat}" lon="{lon}"><ele>500</ele></trkpt>"#
        );
    }
    xml.push_str("</trkseg></trk></gpx>");
    let route = Route::from_gpx(&xml, None).await.unwrap();
    let projection = LocalProjection::for_route(&route);
    let road = road::RoadIndex::new(&route, &projection);

    let length = route.length().0;
    let mut along = route.length();
    along.0 = 0.0;
    while along.0 < length {
        let rider = route.position(along);
        let (east, north) = projection.project(rider.lat, rider.lon);
        let (off, _, _) = road.nearest(east, north, 5.0).expect("the road nearby");
        // The road is drawn in straight pieces of a few metres along the curve.
        assert!(off < 0.05, "{off} m off the road's middle at {} m", along.0);
        along.0 += 0.5;
    }
}

#[tokio::test]
async fn turning_back_the_same_way_the_road_ends_round() {
    // 100 m north and straight back (#101).
    let mut xml = String::from("<gpx><trk><trkseg>");
    for i in (0..=10).chain((0..10).rev()) {
        let (lat, lon) = at(0.0, f64::from(i) * 10.0);
        let _ = write!(
            xml,
            r#"<trkpt lat="{lat}" lon="{lon}"><ele>500</ele></trkpt>"#
        );
    }
    xml.push_str("</trkseg></trk></gpx>");
    let route = Route::from_gpx(&xml, None).await.unwrap();
    let projection = LocalProjection::for_route(&route);
    let mesh = road::RoadIndex::new(&route, &projection).mesh(ROAD_HALF_WIDTH, &[]);
    assert_valid(&mesh);

    // The road's surface reaches round the turning point as far as its half width, rather than
    // to a point straight ahead.
    let surface: Vec<[[f32; 3]; 3]> = triangles(&mesh)
        .filter(|t| t.iter().all(|v| (v[1] - 500.0).abs() < 0.01))
        .collect();
    let layers = |east: f32, north: f32| {
        surface
            .iter()
            .filter(|t| {
                let side = |a: [f32; 3], b: [f32; 3]| {
                    (b[0] - a[0]) * (-north - a[2]) - (b[2] - a[2]) * (east - a[0])
                };
                let sides = [side(t[0], t[1]), side(t[1], t[2]), side(t[2], t[0])];
                sides.iter().all(|&s| s >= 0.0) || sides.iter().all(|&s| s <= 0.0)
            })
            .count()
    };
    let reach = 0.8 * ROAD_HALF_WIDTH as f32;
    for degrees in (-80..=80).step_by(10) {
        let angle = (degrees as f32).to_radians();
        let (east, north) = (reach * angle.sin(), 100.0 + reach * angle.cos());
        assert!(layers(east, north) > 0, "no road at {east:.1}, {north:.1}");
    }
    // Ridden twice, the road is drawn once: its markings are not doubled.
    assert_eq!(layers(1.1, 50.3), 1);
}

#[tokio::test]
async fn no_ground_covers_the_road_on_a_hillside_with_a_hairpin() {
    // Terrain rising 30 % to the east; the road climbs north along it, turns in a hairpin of
    // 15 m radius and comes back 30 m further up the slope, cut into the hillside.
    struct Hillside;
    impl ElevationModel for Hillside {
        fn elevation(
            &mut self,
            _lat: f64,
            lon: f64,
        ) -> impl std::future::Future<Output = Result<f64, String>> + Send {
            let east = (lon - 7.0) * METERS_PER_DEGREE * 46f64.to_radians().cos();
            std::future::ready(Ok(500.0 + 0.3 * east))
        }
    }
    let mut points = Vec::new();
    for i in 0..=30 {
        points.push((0.0, f64::from(i) * 10.0));
    }
    for k in 1..12 {
        let angle = std::f64::consts::PI * f64::from(k) / 12.0;
        points.push((15.0 - 15.0 * angle.cos(), 300.0 + 15.0 * angle.sin()));
    }
    for i in (0..=30).rev() {
        points.push((30.0, f64::from(i) * 10.0));
    }
    let mut xml = String::from("<gpx><trk><trkseg>");
    for &(east, north) in &points {
        let (lat, lon) = at(east, north);
        let elevation = 500.0 + 0.3 * east;
        let _ = write!(
            xml,
            r#"<trkpt lat="{lat}" lon="{lon}"><ele>{elevation}</ele></trkpt>"#
        );
    }
    xml.push_str("</trkseg></trk></gpx>");
    let route = Route::from_gpx_with::<Hillside>(&xml, None, &MapData::default())
        .await
        .unwrap();
    let world = generate(&route, &mut Hillside, &MapData::default(), &mut |_, _| {}).await;

    // Every edge of the road, and its middle, lies above the ground there.
    let road = &world.road.vertices;
    let mut checked = 0;
    for section in road.as_chunks::<6>().0 {
        let (left, right) = (section[2], section[3]);
        let middle = [0, 1, 2].map(|k| f32::midpoint(left[k], right[k]));
        for point in [left, middle, right] {
            let ground = ground_at(&world, point[0], point[2]).expect("ground under the road");
            assert!(
                ground <= point[1] + 0.01,
                "ground {ground} over the road at {point:?}"
            );
            checked += 1;
        }
    }
    assert!(checked > 300);
    // The hillside above the upper leg is natural again further up.
    let up = ground_at(&world, 90.0, -150.0).unwrap();
    assert!((up - (500.0 + 0.3 * 90.0)).abs() < 0.5, "{up}");
}

#[tokio::test]
async fn ground_under_a_bridge_is_left_alone() {
    // A valley 30 m below the deck.
    struct Valley;
    impl ElevationModel for Valley {
        fn elevation(
            &mut self,
            lat: f64,
            _lon: f64,
        ) -> impl std::future::Future<Output = Result<f64, String>> + Send {
            let north = (lat - 46.0) * METERS_PER_DEGREE;
            std::future::ready(Ok(if (320.0..680.0).contains(&north) {
                470.0
            } else {
                500.0
            }))
        }
    }
    let bridge = Structure {
        kind: StructureKind::Bridge,
        line: vec![at(0.0, 300.0), at(0.0, 700.0)],
    };
    let route = route_north(&[bridge]).await;
    let world = generate(&route, &mut Valley, &MapData::default(), &mut |_, _| {}).await;

    let under_bridge = terrain_vertices(&world)
        .find(|v| v[0].abs() < 9.0 && (v[2] + 500.0).abs() < 9.0)
        .unwrap();
    assert!((under_bridge[1] - 470.0).abs() < 0.01, "{under_bridge:?}");
}

#[tokio::test]
async fn without_terrain_data_the_world_follows_the_road() {
    let world = generate(
        &route_north(&[]).await,
        &mut NoData,
        &MapData::default(),
        &mut |_, _| {},
    )
    .await;

    assert!(world.fallback_samples > 0);
    assert!(terrain_vertices(&world).all(|v| (v[1] - 500.0).abs() < 0.3));
}

#[tokio::test]
async fn land_cover_colours_the_ground() {
    let forest = Area {
        cover: LandCover::Forest,
        outer: vec![square(300.0, 500.0, 100.0)],
        inner: vec![],
    };
    let world = world(&MapData {
        areas: vec![forest],
        ..MapData::default()
    })
    .await;

    let color_at = |east: f32, north: f32| {
        world
            .chunks
            .iter()
            .find_map(|c| {
                c.mesh
                    .vertices
                    .iter()
                    .zip(&c.mesh.colors)
                    .find_map(|(v, color)| {
                        let (x, z) = (v[0] + c.center[0], v[2] + c.center[2]);
                        ((x - east).abs() < 9.0 && (z + north).abs() < 9.0).then_some(*color)
                    })
            })
            .unwrap()
    };
    assert_eq!(
        color_at(300.0, 500.0),
        landcover::color(Some(LandCover::Forest))
    );
    assert_eq!(color_at(800.0, 500.0), landcover::color(None));
}

#[tokio::test]
async fn forests_get_trees_but_not_on_the_road() {
    // A forest across the road; open meadow around it.
    let forest = Area {
        cover: LandCover::Forest,
        outer: vec![square(0.0, 500.0, 100.0)],
        inner: vec![],
    };
    let world = world(&MapData {
        areas: vec![forest],
        ..MapData::default()
    })
    .await;

    let trees = plants_of(&world, &["conifer", "broadleaf"]);
    let in_forest = |[x, _, z]: &[f32; 3]| (-610.0..=-390.0).contains(z) && x.abs() <= 110.0;
    let forest_trees = trees.iter().filter(|t| in_forest(t)).count();
    assert!(forest_trees > 200, "{forest_trees} trees in the forest");
    // Outside it, in the meadows within 300 m of the road, only now and then a tree on its own.
    let lone = (trees.len() - forest_trees) as f64;
    let forest_density = forest_trees as f64 / (200.0 * 200.0);
    let meadow_density = lone / (1600.0 * 600.0 - 200.0 * 200.0);
    assert!(
        meadow_density < forest_density / 20.0,
        "{lone} trees outside the forest"
    );
    for [x, y, z] in &trees {
        // Past the road's ends the open ground is free.
        let beside = (0.0..=1000.0).contains(&-z);
        assert!(!beside || x.abs() >= 8.0, "tree on the road at {x}");
        // Beyond the ground levelled for the road, trees stand on the natural terrain.
        assert!(
            x.abs() < LEVEL_REACH as f32 || (y - (500.0 + 0.1 * x)).abs() < 1.0,
            "tree not on the ground: {y}"
        );
    }
    let colours: BTreeSet<[u32; 3]> = world
        .chunks
        .iter()
        .flat_map(|c| c.trees.models.values())
        .flat_map(|buffer| buffer.as_chunks::<16>().0.iter())
        .map(|plant| [plant[12], plant[13], plant[14]].map(f32::to_bits))
        .collect();
    assert!(colours.len() > 3, "trees vary in colour: {colours:?}");
}

#[tokio::test]
async fn rocks_lie_on_rocky_ground_off_the_road_and_out_of_buildings() {
    // Scree beside the road, a house on it; the gentle slope elsewhere has no rocks.
    let scree = Area {
        cover: LandCover::Rock,
        outer: vec![square(60.0, 400.0, 50.0)],
        inner: vec![],
    };
    let house = Building {
        id: 7,
        outline: rectangle(60.0, 400.0, 6.0, 5.0),
        height: None,
        levels: Some(2.0),
        color: None,
    };
    let world = world(&MapData {
        areas: vec![scree],
        buildings: vec![house],
        ..MapData::default()
    })
    .await;

    let rocks = plants_of(&world, &["rock"]);
    assert!(rocks.len() > 10, "{} rocks", rocks.len());
    for [x, _, z] in &rocks {
        assert!(
            (5.0..=115.0).contains(x) && (-455.0..=-345.0).contains(z),
            "rock off the scree at {x}, {z}"
        );
        assert!(
            (x - 60.0).hypot(z + 400.0) > 7.0,
            "rock in the house at {x}, {z}"
        );
    }
    for [x, _, z] in plants_of(&world, &["bush", "broadleaf", "conifer"]) {
        let beside = (0.0..=1000.0).contains(&-z);
        assert!(!beside || x.abs() >= 5.0, "plant on the road at {x}, {z}");
    }
}

/// Positions (absolute) of the plants of the given model kinds.
fn plants_of(world: &World, kinds: &[&str]) -> Vec<[f32; 3]> {
    world
        .chunks
        .iter()
        .flat_map(|c| {
            c.trees
                .models
                .iter()
                .filter(|(name, _)| {
                    vegetation::kind_of(name).is_some_and(|kind| kinds.contains(&kind))
                })
                .flat_map(move |(_, buffer)| {
                    buffer
                        .as_chunks::<16>()
                        .0
                        .iter()
                        .map(move |t| [t[3] + c.center[0], t[7], t[11] + c.center[2]])
                })
        })
        .collect()
}

#[tokio::test]
async fn grass_and_flowers_line_the_road_but_not_lakes_or_the_road_itself() {
    // A pond beside the road at 300 m, a forest beside it at 700 m; open ground elsewhere.
    let pond = Area {
        cover: LandCover::Water,
        outer: vec![square(15.0, 300.0, 8.0)],
        inner: vec![],
    };
    let forest = Area {
        cover: LandCover::Forest,
        outer: vec![square(15.0, 700.0, 60.0)],
        inner: vec![],
    };
    let world = world(&MapData {
        areas: vec![pond, forest],
        ..MapData::default()
    })
    .await;
    let positions = |pick: fn(&crate::Trees) -> &Vec<f32>| -> Vec<[f32; 2]> {
        world
            .chunks
            .iter()
            .flat_map(|c| {
                pick(&c.trees)
                    .as_chunks::<12>()
                    .0
                    .iter()
                    .map(move |t| [t[3] + c.center[0], t[11] + c.center[2]])
                    .collect::<Vec<_>>()
            })
            .collect()
    };
    let grass = positions(|t| &t.grass);
    let flowers = positions(|t| &t.flowers);

    assert!(grass.len() > 1000, "{} tufts", grass.len());
    assert_ne!(flowers.len(), 0);
    for [x, z] in grass.iter().chain(&flowers) {
        // Beside the road (it runs north for 1 km); past its ends the distance is to its end.
        let beside = (0.0..=1000.0).contains(&-z);
        assert!(x.abs() <= 30.0, "plant {x} m from the road");
        assert!(!beside || x.abs() >= 3.6, "plant on the road at {x}, {z}");
        let in_pond = (x - 15.0).abs() < 8.0 && (z + 300.0).abs() < 8.0;
        assert!(!in_pond, "grass in the pond at {x}, {z}");
    }
    // The forest floor is sparse next to open ground of the same size.
    let count = |north: f32| {
        grass
            .iter()
            .filter(|[x, z]| *x > 3.6 && *x < 30.0 && (z + north).abs() < 50.0)
            .count()
    };
    assert!(
        count(700.0) * 2 < count(500.0),
        "{} vs {}",
        count(700.0),
        count(500.0)
    );
}

#[tokio::test]
async fn other_streets_join_the_road_ridden_and_lie_on_the_ground() {
    use torqa_osm::{Road, RoadClass, StructureKind};

    let way = |class, from: (f64, f64), to: (f64, f64), structure| Road {
        class,
        line: vec![at(from.0, from.1), at(to.0, to.1)],
        structure,
    };
    let world = world(&MapData {
        roads: vec![
            // The road ridden itself, as the map has it.
            way(RoadClass::Street, (0.0, 0.0), (0.0, 1000.0), None),
            // A side street crossing the route, a farm track beside it, a tunnel, a bridge.
            way(RoadClass::Street, (-200.0, 300.0), (200.0, 300.0), None),
            way(RoadClass::Track, (60.0, 100.0), (60.0, 600.0), None),
            way(
                RoadClass::Major,
                (-300.0, 800.0),
                (300.0, 800.0),
                Some(StructureKind::Tunnel),
            ),
            way(
                RoadClass::Street,
                (100.0, 900.0),
                (300.0, 900.0),
                Some(StructureKind::Bridge),
            ),
        ],
        ..MapData::default()
    })
    .await;
    let vertices = |pick: fn(&crate::TerrainChunk) -> &MeshData| -> Vec<[f32; 3]> {
        world
            .chunks
            .iter()
            .flat_map(|c| {
                pick(c)
                    .vertices
                    .iter()
                    .map(move |v| [v[0] + c.center[0], v[1] + c.center[1], v[2] + c.center[2]])
                    .collect::<Vec<_>>()
            })
            .collect()
    };
    let streets = vertices(|c| &c.streets);
    let tracks = vertices(|c| &c.tracks);

    assert!(streets.len() > 100, "{} street vertices", streets.len());
    assert!(tracks.len() > 100, "{} track vertices", tracks.len());
    let mut under_the_road = 0;
    for [x, y, z] in &streets {
        // The crossing street and the bridge only: not the road ridden again, not the tunnel.
        let crossing = (z + 300.0).abs() < 3.0;
        let bridge = (z + 900.0).abs() < 3.0 && (99.0..=301.0).contains(x);
        assert!(crossing || bridge, "street vertex at {x}, {z}");
        if crossing && x.abs() < 3.0 {
            // It runs on under the road, below its surface, so the junction is joined.
            under_the_road += 1;
            assert!(
                *y < 500.0 - 0.05,
                "street over the road ridden: {y} at {x}, {z}"
            );
        }
        // On the ground, which rises 0.1 m per metre east beyond the shaped ground; the
        // bridge's deck runs straight between its ends, here on the same slope.
        if x.abs() > LEVEL_REACH as f32 {
            assert!(
                (y - (500.0 + 0.1 * x)).abs() < 0.8,
                "street off the ground: {y} at {x}"
            );
        }
    }
    assert!(
        under_the_road > 0,
        "the crossing street stops short of the road"
    );
    for [x, _, _] in &tracks {
        assert!((x - 60.0).abs() < 2.0);
    }
    // Nothing grows on them.
    for chunk in &world.chunks {
        for tuft in chunk.trees.grass.as_chunks::<12>().0 {
            let (x, z) = (tuft[3] + chunk.center[0], tuft[11] + chunk.center[2]);
            assert!(
                (z + 300.0).abs() > 2.75 || x.abs() < 5.5,
                "grass on the street at {x}"
            );
            assert!(
                (x - 60.0).abs() > 1.4 || !(100.0..=600.0).contains(&-z),
                "grass on the track"
            );
        }
    }
}

/// One building surface: corners in absolute coordinates (x east, y up, z south), the stored
/// normal and the vertex colour, whose alpha is the shader style.
struct Face {
    corners: [[f32; 3]; 3],
    normal: [f32; 3],
    color: [f32; 4],
}

impl Face {
    fn middle(&self) -> [f32; 3] {
        [0, 1, 2].map(|k| self.corners.iter().map(|c| c[k]).sum::<f32>() / 3.0)
    }

    fn style(&self) -> u8 {
        buildings::Style::code(self.color[3])
    }
}

/// The building shells of a world within `radius` metres of (east, north), modelled buildings'
/// included (their shells are drawn in the distance), checking on the way that every mesh is
/// valid and faces its normals.
fn building_faces(world: &World, (east, north): (f32, f32), radius: f32) -> Vec<Face> {
    let mut faces = Vec::new();
    for chunk in &world.chunks {
        let shells =
            std::iter::once(&chunk.buildings).chain(chunk.modelled.iter().map(|c| &c.shells));
        for mesh in shells {
            faces.extend(shell_faces(chunk, mesh, (east, north), radius));
        }
    }
    faces
}

fn shell_faces(
    chunk: &TerrainChunk,
    mesh: &MeshData,
    (east, north): (f32, f32),
    radius: f32,
) -> Vec<Face> {
    assert_valid(mesh);
    assert_faces_follow_normals(mesh);
    let mut faces = Vec::new();
    for t in mesh.indices.as_chunks::<3>().0 {
        let corners = t.map(|k| {
            let v = mesh.vertices[k as usize];
            [v[0] + chunk.center[0], v[1], v[2] + chunk.center[2]]
        });
        let face = Face {
            corners,
            normal: mesh.normals[t[0] as usize],
            color: mesh.colors[t[0] as usize],
        };
        let middle = face.middle();
        if (middle[0] - east).hypot(-middle[2] - north) < radius {
            faces.push(face);
        }
    }
    faces
}

/// A model instance read back from a `MultiMesh` buffer, in absolute coordinates.
struct Placed {
    model: String,
    origin: [f32; 3],
    /// The model's x axis (its length, scaled) and z axis (its width, scaled).
    x: [f32; 3],
    z: [f32; 3],
    plaster: [f32; 4],
}

fn placed(world: &World) -> Vec<Placed> {
    let mut all = Vec::new();
    for chunk in &world.chunks {
        for cell in &chunk.modelled {
            assert_valid(&cell.shells);
            for (model, buffer) in &cell.models {
                assert_eq!(buffer.len() % 20, 0);
                for b in buffer.as_chunks::<20>().0 {
                    all.push(Placed {
                        model: model.clone(),
                        origin: [
                            b[3] + chunk.center[0],
                            b[7] + chunk.center[1],
                            b[11] + chunk.center[2],
                        ],
                        x: [b[0], b[4], b[8]],
                        z: [b[2], b[6], b[10]],
                        plaster: [b[12], b[13], b[14], b[15]],
                    });
                }
            }
        }
    }
    all
}

fn length(v: [f32; 3]) -> f32 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

fn highest(faces: &[Face]) -> f32 {
    faces
        .iter()
        .flat_map(|f| f.corners)
        .map(|c| c[1])
        .fold(f32::MIN, f32::max)
}

fn lowest(faces: &[Face]) -> f32 {
    faces
        .iter()
        .flat_map(|f| f.corners)
        .map(|c| c[1])
        .fold(f32::MAX, f32::min)
}

/// Ground of the eastward slope at a point `east` metres from the road.
fn slope_at(east: f32) -> f32 {
    500.0 + 0.1 * east
}

#[tokio::test]
async fn buildings_drawn_into_the_road_are_left_out() {
    // A square house turned 45° to the road, its corner towards it clipped in the map: the
    // footprint keeps 4.2 m off the centre line, but the house is drawn on the whole square,
    // its corner 2.5 m off, on the road (#138). The same house further off stays.
    let diamond = |east: f64, clip: f64| {
        let reach = clip - east + 6.0;
        let points = [
            (clip, 500.0 - reach),
            (clip, 500.0 + reach),
            (east, 506.0),
            (east + 6.0, 500.0),
            (east, 494.0),
            (clip, 500.0 - reach),
        ];
        points.iter().map(|&(e, n)| at(e, n)).collect::<Vec<_>>()
    };
    let house = |id, outline| Building {
        id,
        outline,
        height: None,
        levels: Some(2.0),
        color: None,
    };
    let world = world(&MapData {
        buildings: vec![house(1, diamond(8.5, 4.2)), house(2, diamond(30.0, 25.7))],
        ..MapData::default()
    })
    .await;

    assert!(
        building_faces(&world, (8.5, 500.0), 7.0).is_empty(),
        "a house drawn on the road"
    );
    assert!(!building_faces(&world, (30.0, 500.0), 7.0).is_empty());
}

#[tokio::test]
async fn buildings_stand_on_the_ground_with_walls_facing_out() {
    // A 10 × 10 m house of two storeys on ground rising eastwards.
    let house = Building {
        id: 42,
        outline: square(60.0, 500.0, 5.0),
        height: None,
        levels: Some(2.0),
        color: None,
    };
    let world = world(&MapData {
        buildings: vec![house],
        ..MapData::default()
    })
    .await;

    let faces = building_faces(&world, (60.0, 500.0), 30.0);
    // From below the lowest corner, 55 m east, to a pitched roof (perhaps with a chimney)
    // over two storeys of walls from the highest corner, 65 m east.
    let eaves = slope_at(65.0) + 2.0 * 3.0 + 0.4;
    let top = highest(&faces);
    assert!((eaves + 3.0..eaves + 6.0).contains(&top), "top {top}");
    assert!((lowest(&faces) - (slope_at(55.0) - 1.0)).abs() < 0.01);

    // Upright faces along the outline (walls, gables, windows, roof edges) face out; those
    // inside it belong to the chimney.
    let mut outside = 0;
    for face in faces.iter().filter(|f| f.normal[1].abs() < 0.1) {
        let middle = face.middle();
        let offset = [middle[0] - 60.0, middle[2] + 500.0];
        if offset[0].abs().max(offset[1].abs()) < 4.9 {
            continue;
        }
        outside += 1;
        let facing = face.normal[0] * offset[0] + face.normal[2] * offset[1];
        assert!(facing > 0.0, "face at {middle:?} faces in");
    }
    assert!(outside >= 8);
}

#[tokio::test]
async fn churches_get_a_tower_and_chapels_a_turret() {
    // A 14 × 30 m church and a 7 × 12 m chapel, with their church points inside.
    let church = Building {
        id: 3,
        outline: rectangle(80.0, 300.0, 7.0, 15.0),
        height: None,
        levels: None,
        color: None,
    };
    let chapel = Building {
        id: 4,
        outline: rectangle(80.0, 700.0, 3.5, 6.0),
        ..church.clone()
    };
    let world = world(&MapData {
        buildings: vec![church, chapel],
        churches: vec![at(80.0, 302.0), at(80.0, 701.0)],
        ..MapData::default()
    })
    .await;

    // The tower rises far above the nave; its top is no wider than a tower.
    let church = building_faces(&world, (80.0, 300.0), 40.0);
    let top = highest(&church);
    assert!(top > slope_at(73.0) + 25.0, "top {top}");
    let tower_top: Vec<[f32; 3]> = church
        .iter()
        .flat_map(|f| f.corners)
        .filter(|c| c[1] > slope_at(73.0) + 20.0)
        .collect();
    let width = |k: usize| {
        let values = tower_top.iter().map(|c| c[k]);
        values.clone().fold(f32::MIN, f32::max) - values.fold(f32::MAX, f32::min)
    };
    assert!(
        width(0) < 9.0 && width(2) < 9.0,
        "{} × {}",
        width(0),
        width(2)
    );
    assert!(church.iter().any(|f| f.style() == 4), "church windows");

    // The chapel's highest 3 m are a turret on its ridge.
    let chapel = building_faces(&world, (80.0, 700.0), 20.0);
    let top = highest(&chapel);
    let turret: Vec<[f32; 3]> = chapel
        .iter()
        .flat_map(|f| f.corners)
        .filter(|c| c[1] > top - 3.0)
        .collect();
    assert!(turret.iter().all(|c| (c[0] - 80.0).abs() < 1.0));
    assert!(top < slope_at(76.5) + 20.0, "top {top}");
}

#[tokio::test]
async fn walls_show_whole_windows_only() {
    // A block of odd size: no wall is a whole number of window spacings long.
    let block = Building {
        id: 7,
        outline: rectangle(80.0, 500.0, 11.85, 5.65),
        height: Some(15.0),
        levels: None,
        color: None,
    };
    let world = world(&MapData {
        buildings: vec![block],
        ..MapData::default()
    })
    .await;

    let mut walls = 0;
    for chunk in &world.chunks {
        let shells =
            std::iter::once(&chunk.buildings).chain(chunk.modelled.iter().map(|c| &c.shells));
        for mesh in shells {
            for (k, color) in mesh.colors.iter().enumerate() {
                // Windowed walls: plaster and timber every 3.2 m, churches every 4.5 m.
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // style codes
                let spacing = match buildings::Style::code(color[3]) {
                    0 | 2 => 3.2,
                    4 => 4.5,
                    _ => continue,
                };
                if mesh.normals[k][1].abs() > 0.01 {
                    continue;
                }
                // Each wall spans whole windows: its ends fall on the grid.
                let windows = mesh.uvs[k][0] / spacing;
                assert!(
                    (windows - windows.round()).abs() < 1e-3,
                    "a wall ending {windows} windows along"
                );
                walls += 1;
            }
        }
    }
    assert!(walls > 8, "{walls} wall corners checked");
}

#[tokio::test]
async fn blocks_have_a_cornice_and_stacked_balconies() {
    // Six blocks of six storeys, 30 m long east-west and 12 m deep.
    let middles: Vec<(f64, f64)> = (0..6)
        .map(|k| {
            (
                80.0 + 70.0 * f64::from(k % 3),
                300.0 + 200.0 * f64::from(k / 3),
            )
        })
        .collect();
    let blocks = middles
        .iter()
        .zip(20..)
        .map(|(&(east, north), id)| Building {
            id,
            outline: rectangle(east, north, 15.0, 6.0),
            height: Some(18.0),
            levels: None,
            color: None,
        })
        .collect();
    let world = world(&MapData {
        buildings: blocks,
        ..MapData::default()
    })
    .await;

    let mut with_balconies = 0;
    for &(east, north) in &middles {
        #[allow(clippy::cast_possible_truncation)] // small coordinates
        let (east, north) = (east as f32, north as f32);
        let faces = building_faces(&world, (east, north), 25.0);
        let off_north = |f: &Face| (-f.middle()[2] - north).abs() - 6.0;
        let off_east = |f: &Face| (f.middle()[0] - east).abs() - 15.0;
        // A cornice stands a quarter metre out of every wall (string courses and balconies
        // stand out less and more).
        let cornice = faces.iter().filter(|f| {
            f.normal[1].abs() < 0.01 && (off_north(f).max(off_east(f)) - 0.25).abs() < 0.05
        });
        assert!(cornice.count() >= 8, "no cornice round the block at {east}");
        // Balconies, if any: level tops 1.2 m out of the long walls, one per storey above the
        // ground floor, 3 m apart.
        let mut tops: Vec<f32> = faces
            .iter()
            .filter(|f| f.normal[1] > 0.99 && off_north(f) > 0.5 && off_east(f) < 0.0)
            .map(|f| f.middle()[1])
            .collect();
        tops.sort_by(f32::total_cmp);
        tops.dedup_by(|a, b| (*a - *b).abs() < 0.1);
        if tops.is_empty() {
            continue;
        }
        with_balconies += 1;
        assert_eq!(tops.len(), 5, "balconies at {tops:?}");
        assert!(
            tops.windows(2).all(|w| (w[1] - w[0] - 3.0).abs() < 0.01),
            "{tops:?}"
        );
    }
    assert!(
        with_balconies >= 2,
        "{with_balconies} of six blocks have balconies"
    );
}

#[tokio::test]
async fn tall_blocks_have_flat_roofs_behind_a_parapet() {
    let block = Building {
        id: 5,
        outline: rectangle(80.0, 500.0, 10.0, 15.0),
        height: Some(18.0),
        levels: None,
        color: None,
    };
    let world = world(&MapData {
        buildings: vec![block],
        ..MapData::default()
    })
    .await;

    let faces = building_faces(&world, (80.0, 500.0), 30.0);
    // Six storeys of 3 m make the mapped 18 m above the highest corner; a parapet capped by
    // its cornice, and perhaps a stair housing, top them.
    let eaves = slope_at(90.0) + 6.0 * 3.0 + 0.4;
    let top = highest(&faces);
    assert!(
        (top - (eaves + 0.88)).abs() < 0.01 || (top - (eaves + 2.6)).abs() < 0.01,
        "top {top}"
    );
    // Nothing slopes: faces are upright or level.
    assert!(
        faces
            .iter()
            .all(|f| f.normal[1].abs() < 0.01 || f.normal[1].abs() > 0.99)
    );
}

#[tokio::test]
async fn halls_on_industrial_land_are_low_and_clad_in_metal() {
    let estate = Area {
        cover: LandCover::Industrial,
        outer: vec![square(150.0, 500.0, 100.0)],
        inner: vec![],
    };
    let hall = Building {
        id: 6,
        outline: rectangle(150.0, 500.0, 30.0, 15.0),
        height: None,
        levels: None,
        color: None,
    };
    let world = world(&MapData {
        buildings: vec![hall],
        areas: vec![estate],
        ..MapData::default()
    })
    .await;

    let faces = building_faces(&world, (150.0, 500.0), 50.0);
    let height = highest(&faces) - slope_at(180.0);
    assert!((6.0..=12.5).contains(&height), "{height} m");
    let walls = faces.iter().filter(|f| f.normal[1].abs() < 0.1);
    assert!(walls.clone().any(|f| f.style() == 5), "metal cladding");
    assert!(walls.clone().all(|f| f.style() != 0), "no house windows");
}

#[tokio::test]
async fn offices_hotels_and_public_buildings_come_from_the_map() {
    // Long sides north–south, so the eastward slope falls little across them.
    let building = |id: i64, east: f64, north: f64, half_east: f64, half_north: f64| Building {
        id,
        outline: rectangle(east, north, half_east, half_north),
        height: None,
        levels: None,
        color: None,
    };
    let land = |cover: LandCover, east: f64, north: f64| Area {
        cover,
        outer: vec![square(east, north, 45.0)],
        inner: vec![],
    };
    let world = world(&MapData {
        buildings: vec![
            // On commercial land.
            building(1, 200.0, 300.0, 8.0, 15.0),
            // A hotel and a town hall marked by their points.
            building(2, -150.0, 450.0, 6.5, 12.0),
            building(3, -150.0, 600.0, 7.0, 13.0),
            // On a school's grounds.
            building(4, 200.0, 700.0, 8.0, 19.0),
            // A guest house in a family house stays a house.
            building(5, -150.0, 800.0, 5.0, 6.0),
        ],
        areas: vec![
            land(LandCover::Commercial, 200.0, 300.0),
            land(LandCover::Public, 200.0, 700.0),
        ],
        hotels: vec![at(-150.0, 450.0), at(-151.0, 805.0)],
        public: vec![at(-152.0, 600.0)],
        ..MapData::default()
    })
    .await;

    let placed = placed(&world);
    let model_at = |east: f32, north: f32| {
        placed
            .iter()
            .find(|p| (p.origin[0] - east).hypot(-p.origin[2] - north) < 3.0)
            .map(|p| p.model.clone())
            .unwrap_or_default()
    };
    for (east, north, kind) in [
        (200.0, 300.0, "office"),
        (-150.0, 450.0, "hotel"),
        (-150.0, 600.0, "public"),
        (200.0, 700.0, "public"),
        (-150.0, 800.0, "house"),
    ] {
        let model = model_at(east, north);
        assert!(
            model.starts_with(kind),
            "{model} at {east}, {north}, not {kind}"
        );
    }
}

#[tokio::test]
async fn chalets_in_the_mountains_have_timber_walls_and_deep_eaves() {
    struct Mountains;
    impl ElevationModel for Mountains {
        fn elevation(
            &mut self,
            _lat: f64,
            _lon: f64,
        ) -> impl std::future::Future<Output = Result<f64, String>> + Send {
            std::future::ready(Ok(1300.0))
        }
    }
    // A 12 × 10 m house at 1300 m.
    let house = Building {
        id: 9,
        outline: rectangle(80.0, 500.0, 6.0, 5.0),
        height: None,
        levels: None,
        color: None,
    };
    let map = MapData {
        buildings: vec![house],
        ..MapData::default()
    };
    let world = generate(
        &route_north(&[]).await,
        &mut Mountains,
        &map,
        &mut |_, _| {},
    )
    .await;

    let faces = building_faces(&world, (80.0, 500.0), 30.0);
    // Timber above a plastered ground floor.
    assert!(faces.iter().any(|f| f.style() == 2), "timber");
    assert!(faces.iter().any(|f| f.style() == 0), "plaster");
    // A shallow roof reaching well beyond the walls all round.
    let sloped = faces
        .iter()
        .filter(|f| (0.1..0.99).contains(&f.normal[1].abs()));
    assert!(sloped.clone().count() >= 4);
    assert!(sloped.clone().all(|f| f.normal[1].abs() > 0.85));
    let corners: Vec<[f32; 3]> = faces.iter().flat_map(|f| f.corners).collect();
    let reach = |k: usize, centre: f32| {
        corners
            .iter()
            .map(|c| (c[k] - centre).abs())
            .fold(f32::MIN, f32::max)
    };
    assert!(reach(0, 80.0) > 6.0 + 1.1 && reach(2, -500.0) > 5.0 + 1.1);
}

#[tokio::test]
async fn roofs_of_irregular_outlines_stay_over_them() {
    // An L-shaped farmhouse: 20 m arms, 10 m wide.
    let corners = [
        (60.0, 400.0),
        (80.0, 400.0),
        (80.0, 410.0),
        (70.0, 410.0),
        (70.0, 420.0),
        (60.0, 420.0),
        (60.0, 400.0),
    ];
    let house = Building {
        id: 11,
        outline: corners.iter().map(|&(e, n)| at(e, n)).collect(),
        height: None,
        levels: None,
        color: None,
    };
    let world = world(&MapData {
        buildings: vec![house],
        ..MapData::default()
    })
    .await;

    let faces = building_faces(&world, (70.0, 410.0), 30.0);
    assert!(
        faces.iter().any(|f| (0.1..0.99).contains(&f.normal[1])),
        "a pitched roof"
    );
    // Nothing reaches beyond the walls by more than the eaves' overhang (at most 1.2 m), so
    // nothing covers the notch.
    for c in faces.iter().flat_map(|f| f.corners) {
        let (east, north) = (c[0], -c[2]);
        let beyond = |value: f32, low: f32, high: f32| (low - value).max(value - high).max(0.0);
        let to_arm =
            |e: (f32, f32), n: (f32, f32)| beyond(east, e.0, e.1).max(beyond(north, n.0, n.1));
        let outside =
            to_arm((60.0, 80.0), (400.0, 410.0)).min(to_arm((60.0, 70.0), (400.0, 420.0)));
        assert!(outside < 1.25, "{c:?} is {outside} m out");
    }
}

#[tokio::test]
async fn shops_get_a_front_and_an_awning_onto_their_street() {
    use torqa_osm::{Road, RoadClass};

    // Two 12 × 9 m houses along a street to their south; a bakery in the western one.
    let house = |id: i64, east: f64| Building {
        id,
        outline: rectangle(east, 500.0, 6.0, 4.5),
        height: None,
        levels: Some(2.0),
        color: None,
    };
    let world = world(&MapData {
        buildings: vec![house(31, 80.0), house(32, 120.0)],
        roads: vec![Road {
            class: RoadClass::Street,
            line: vec![at(40.0, 489.0), at(160.0, 489.0)],
            structure: None,
        }],
        shops: vec![at(81.0, 501.0)],
        ..MapData::default()
    })
    .await;

    // Drawn by the world itself, so the front shows up close too.
    assert!(
        placed(&world)
            .iter()
            .all(|m| (m.origin[0] - 80.0).abs() > 1.0),
        "the shop became a model"
    );
    let shop = building_faces(&world, (80.0, 500.0), 9.0);
    let fronts: Vec<&Face> = shop.iter().filter(|f| f.style() == 10).collect();
    assert!(!fronts.is_empty(), "no shop front");
    let ground = slope_at(86.0);
    for front in &fronts {
        // On the ground floor of the south wall, facing the street (Godot's +z).
        assert!(front.normal[2] > 0.99, "a front facing {:?}", front.normal);
        assert!(
            (-front.middle()[2] - 495.5).abs() < 0.2,
            "a front at {:?}",
            front.middle()
        );
        assert!(front.corners.iter().all(|c| c[1] < ground + 3.0));
    }
    // The awning reaches out over the pavement, above the front.
    let awnings = palette::list("buildings.awnings");
    let awning: Vec<&Face> = shop
        .iter()
        .filter(|f| awnings.contains(&[f.color[0], f.color[1], f.color[2]]))
        .collect();
    assert!(awning.len() >= 4, "{} awning faces", awning.len());
    let reach = awning
        .iter()
        .flat_map(|f| f.corners)
        .map(|c| -c[2])
        .fold(f32::MAX, f32::min);
    assert!(
        (495.5 - reach - 1.4).abs() < 0.15,
        "the awning reaches to {reach}"
    );
    // The neighbour has no shop.
    let neighbour = building_faces(&world, (120.0, 500.0), 9.0);
    assert!(neighbour.iter().all(|f| f.style() != 10));
}

#[tokio::test]
async fn roads_stay_clear_of_buildings() {
    use torqa_osm::{Road, RoadClass};

    // A 60 × 20 m block across the road ridden (its corners far from it), a 40 × 12 m block
    // a street runs through, and a house beside each.
    let building = |id: i64, outline| Building {
        id,
        outline,
        height: None,
        levels: Some(3.0),
        color: None,
    };
    let world = world(&MapData {
        buildings: vec![
            building(41, rectangle(0.0, 300.0, 30.0, 10.0)),
            building(42, rectangle(20.0, 400.0, 6.0, 4.5)),
            building(43, rectangle(100.0, 600.0, 20.0, 6.0)),
            building(44, rectangle(100.0, 615.0, 6.0, 4.0)),
        ],
        roads: vec![Road {
            class: RoadClass::Street,
            line: vec![at(40.0, 600.0), at(200.0, 600.0)],
            structure: None,
        }],
        ..MapData::default()
    })
    .await;

    let models = placed(&world);
    let stands = |(east, north): (f32, f32), radius: f32| {
        !building_faces(&world, (east, north), radius).is_empty()
            || models
                .iter()
                .any(|m| (m.origin[0] - east).hypot(-m.origin[2] - north) < radius)
    };
    assert!(!stands((0.0, 300.0), 25.0), "a block stands on the road");
    assert!(!stands((100.0, 600.0), 8.0), "a block stands on the street");
    assert!(stands((20.0, 400.0), 5.0), "the house by the road is gone");
    assert!(
        stands((100.0, 615.0), 3.0),
        "the house by the street is gone"
    );
}

#[tokio::test]
async fn rectangular_houses_become_models_with_shells_for_the_distance() {
    // A 12 × 9 m house with its long side north–south, and an L-shaped farmhouse.
    let house = Building {
        id: 21,
        outline: rectangle(60.0, 500.0, 4.5, 6.0),
        height: None,
        levels: Some(2.0),
        color: None,
    };
    let l_shape = [
        (60.0, 700.0),
        (80.0, 700.0),
        (80.0, 710.0),
        (70.0, 710.0),
        (70.0, 720.0),
        (60.0, 720.0),
        (60.0, 700.0),
    ];
    let farmhouse = Building {
        id: 22,
        outline: l_shape.iter().map(|&(e, n)| at(e, n)).collect(),
        ..house.clone()
    };
    let world = world(&MapData {
        buildings: vec![house, farmhouse],
        ..MapData::default()
    })
    .await;

    let models = placed(&world);
    assert_eq!(models.len(), 1, "only the rectangular house");
    let house = &models[0];
    assert!(house.model.starts_with("house_"), "{}", house.model);
    // Standing on its footprint's centre, at the highest ground (65 m east), stretched by no
    // more than a quarter, its length along the outline's.
    assert!((house.origin[0] - 60.0).abs() < 0.01 && (house.origin[2] + 500.0).abs() < 0.01);
    assert!(
        (house.origin[1] - slope_at(64.5)).abs() < 0.01,
        "{}",
        house.origin[1]
    );
    assert!(
        house.x[0].abs() < 0.01 && house.z[2].abs() < 0.01,
        "length runs north"
    );
    let (along, across) = (length(house.x), length(house.z));
    assert!((0.8..=1.25).contains(&along) && (0.8..=1.25).contains(&across));
    assert!(house.plaster[3] == 1.0 && house.plaster[..3].iter().all(|&c| c > 0.3));
    // Both still have shells: the house's for the distance, the farmhouse's always.
    assert!(!building_faces(&world, (60.0, 500.0), 12.0).is_empty());
    assert!(!building_faces(&world, (70.0, 710.0), 15.0).is_empty());
    let unmodelled: usize = world
        .chunks
        .iter()
        .map(|c| c.buildings.vertices.len())
        .sum();
    assert!(unmodelled > 0, "the farmhouse is drawn as its shell");
}

/// Terrain sloping down eastwards by `grade`, 1300 m at the route start: mountains.
struct Valley {
    grade: f64,
}

impl ElevationModel for Valley {
    fn elevation(
        &mut self,
        _lat: f64,
        lon: f64,
    ) -> impl std::future::Future<Output = Result<f64, String>> + Send {
        let east = (lon - 7.0) * METERS_PER_DEGREE * 46f64.to_radians().cos();
        std::future::ready(Ok(1300.0 - self.grade * east))
    }
}

#[tokio::test]
async fn chalets_show_their_front_to_the_valley_and_churches_their_choir_to_the_east() {
    // A chalet with its long side east–west on ground falling eastwards, and a church
    // likewise, each the other way round in the map.
    let chalet = Building {
        id: 31,
        outline: rectangle(80.0, 400.0, 6.0, 4.75),
        height: None,
        levels: None,
        color: None,
    };
    let church = Building {
        id: 32,
        outline: [
            (-95.0, 600.0),
            (-95.0, 610.5),
            (-67.0, 610.5),
            (-67.0, 600.0),
            (-95.0, 600.0),
        ]
        .iter()
        .map(|&(e, n)| at(e, n))
        .collect(),
        ..chalet.clone()
    };
    let map = MapData {
        buildings: vec![chalet, church],
        churches: vec![at(-80.0, 605.0)],
        ..MapData::default()
    };
    let route = route_north(&[]).await;
    let world = generate(&route, &mut Valley { grade: 0.05 }, &map, &mut |_, _| {}).await;

    let models = placed(&world);
    let chalet = models
        .iter()
        .find(|p| p.model.starts_with("chalet"))
        .expect("a chalet model");
    assert!(
        chalet.x[0] > 0.0,
        "the balconies face east, down the valley"
    );
    let church = models
        .iter()
        .find(|p| p.model.starts_with("church"))
        .expect("a church model");
    assert!(church.x[0] > 0.0, "the choir is in the east");
}

#[tokio::test]
async fn houses_on_steep_slopes_keep_their_shells() {
    // A 30 % slope: the downhill side of a 12 m house lies 3.6 m lower, more than a model's
    // basement walls reach.
    let house = Building {
        id: 41,
        outline: rectangle(80.0, 400.0, 6.0, 4.5),
        height: None,
        levels: Some(2.0),
        color: None,
    };
    let map = MapData {
        buildings: vec![house],
        ..MapData::default()
    };
    let route = route_north(&[]).await;
    let world = generate(&route, &mut Valley { grade: 0.3 }, &map, &mut |_, _| {}).await;

    assert!(placed(&world).is_empty());
    assert!(!building_faces(&world, (80.0, 400.0), 12.0).is_empty());
}

#[tokio::test]
async fn streams_run_in_channels_and_pass_under_roads_and_streets() {
    use torqa_osm::{Road, RoadClass};

    // A river crossing the route at 500 m, and a street crossing the river east of the route.
    let world = world(&MapData {
        waterways: vec![Waterway {
            width: 12.0,
            line: vec![at(-3000.0, 500.0), at(3000.0, 500.0)],
        }],
        roads: vec![Road {
            class: RoadClass::Street,
            line: vec![at(100.0, 300.0), at(100.0, 700.0)],
            structure: None,
        }],
        ..MapData::default()
    })
    .await;
    let water_at = |x: f32, z: f32| height_on(&world, |c| &c.water, x, z);

    for chunk in &world.chunks {
        assert_valid(&chunk.water);
        // Only within the corridor.
        assert!(
            chunk
                .water
                .vertices
                .iter()
                .all(|v| (v[0] + chunk.center[0]).abs() <= CORRIDOR as f32 + 10.0)
        );
    }
    let mut checked = 0;
    for step in -1400..=1400 {
        #[allow(clippy::cast_precision_loss)] // small steps
        let x = step as f32;
        for z in [-505.0, -500.0, -495.0] {
            let (Some(water), Some(ground)) = (water_at(x, z), ground_at(&world, x, z)) else {
                continue;
            };
            // In a channel (#93): over its bed, half a metre below the land beside it (rising
            // 0.1 m per metre east), never floating over the land.
            let land = 500.0 + 0.1 * x;
            // Where the road and the street cross on the ground, the stream runs through a
            // culvert, under the ground.
            let culvert = x.abs() < 12.0 || (x - 100.0).abs() < 9.0;
            assert!(
                culvert || water > ground,
                "water {water} under ground {ground} at {x}, {z}"
            );
            if x.abs() > LEVEL_REACH as f32 {
                assert!(
                    (water - (land - 0.48)).abs() < 0.05,
                    "water {water} by land at {land} at {x}, {z}"
                );
            }
            if x.abs() < ROAD_HALF_WIDTH as f32 {
                assert!(
                    water < 500.0 - 0.05,
                    "water over the road ridden: {water} at {x}, {z}"
                );
            }
            if let Some(street) = street_at(&world, x, z) {
                assert!(
                    water < street,
                    "water over the street: {water} over {street} at {x}, {z}"
                );
            }
            checked += 1;
        }
    }
    assert!(checked > 6000, "{checked} points checked");
}

/// Every triangle is clockwise seen from the side its normal points to (Godot's front face):
/// the right-hand normal points against the stored vertex normal.
fn assert_faces_follow_normals(mesh: &MeshData) {
    for t in mesh.indices.as_chunks::<3>().0 {
        let triangle = t.map(|k| mesh.vertices[k as usize]);
        let normal = mesh.normals[t[0] as usize];
        let face = face_normal(triangle);
        let dot = face[0] * normal[0] + face[1] * normal[1] + face[2] * normal[2];
        assert!(
            dot < 0.0,
            "triangle {triangle:?} faces away from {normal:?}"
        );
    }
}

#[tokio::test]
async fn bridges_of_other_streets_stand_on_piers() {
    use torqa_osm::{Road, RoadClass};

    // A street bridge east of the route over a valley 20 m deep.
    struct Valley;
    impl ElevationModel for Valley {
        fn elevation(
            &mut self,
            _lat: f64,
            lon: f64,
        ) -> impl std::future::Future<Output = Result<f64, String>> + Send {
            let east = (lon - 7.0) * METERS_PER_DEGREE * 46f64.to_radians().cos();
            std::future::ready(Ok(if (150.0..250.0).contains(&east) {
                480.0
            } else {
                500.0
            }))
        }
    }
    let route = route_north(&[]).await;
    let map = MapData {
        roads: vec![Road {
            class: RoadClass::Street,
            line: vec![at(100.0, 600.0), at(300.0, 600.0)],
            structure: Some(StructureKind::Bridge),
        }],
        ..MapData::default()
    };
    let world = generate(&route, &mut Valley, &map, &mut |_, _| {}).await;

    let mut piers = 0;
    for chunk in &world.chunks {
        assert_valid(&chunk.streets);
        assert_faces_follow_normals(&chunk.streets);
        for v in &chunk.streets.vertices {
            let (x, y) = (v[0] + chunk.center[0], v[1]);
            if y < 495.0 {
                // Only in the valley (its sides slope across a cell of the ground), down to
                // its floor.
                assert!(
                    (134.0..=266.0).contains(&x),
                    "a pier outside the valley at {x}"
                );
                assert!(y > 479.0, "a pier under the ground: {y}");
                if y < 480.0 {
                    piers += 1;
                }
            }
        }
    }
    // Five piers, 20 m apart, four corners each at the bottom.
    assert!(piers >= 16, "{piers} pier corners on the valley floor");
}

#[tokio::test]
async fn short_low_bridges_are_stone_arches() {
    // A 40 m bridge over a gully 8 m deep.
    struct Gully;
    impl ElevationModel for Gully {
        fn elevation(
            &mut self,
            lat: f64,
            _lon: f64,
        ) -> impl std::future::Future<Output = Result<f64, String>> + Send {
            let north = (lat - 46.0) * METERS_PER_DEGREE;
            std::future::ready(Ok(if (305.0..335.0).contains(&north) {
                492.0
            } else {
                500.0
            }))
        }
    }
    let bridge = Structure {
        kind: StructureKind::Bridge,
        line: vec![at(0.0, 300.0), at(0.0, 340.0)],
    };
    let route = route_north(&[bridge]).await;
    let world = generate(&route, &mut Gully, &MapData::default(), &mut |_, _| {}).await;

    let mesh = &world.structures;
    assert_valid(mesh);
    assert_faces_follow_normals(mesh);
    let stone = palette::srgb("structure.stone", 0.0);
    assert!(mesh.colors.iter().all(|c| *c == stone), "built of stone");
    // Vaults: faces turned down into the openings, curving from their springing up to just
    // under the deck.
    let vault: Vec<f32> = mesh
        .indices
        .as_chunks::<3>()
        .0
        .iter()
        .filter(|t| mesh.normals[t[0] as usize][1] < -0.1)
        .map(|t| t.iter().map(|&k| mesh.vertices[k as usize][1]).sum::<f32>() / 3.0)
        .filter(|&y| y < 500.0 - 1.2 - 0.1)
        .collect();
    assert!(vault.len() > 10, "{} vault faces", vault.len());
    let (low, high) = vault
        .iter()
        .fold((f32::MAX, f32::MIN), |(l, h), &y| (l.min(y), h.max(y)));
    assert!(high - low > 2.0, "the vault curves: {low} to {high}");
    assert!(high < 500.0 - 1.2 - 0.3, "crowns under the deck: {high}");
    // The piers and walls reach down into the gully.
    let lowest = mesh.vertices.iter().map(|v| v[1]).fold(f32::MAX, f32::min);
    assert!(
        (lowest - 489.5).abs() < 0.1,
        "below the gully's floor: {lowest}"
    );
}

#[tokio::test]
async fn bridges_have_a_deck_and_pillars_down_to_the_valley() {
    // A 400 m bridge over a valley: the model is 30 m lower under the bridge.
    struct Valley;
    impl ElevationModel for Valley {
        fn elevation(
            &mut self,
            lat: f64,
            _lon: f64,
        ) -> impl std::future::Future<Output = Result<f64, String>> + Send {
            let north = (lat - 46.0) * METERS_PER_DEGREE;
            std::future::ready(Ok(if (320.0..680.0).contains(&north) {
                470.0
            } else {
                500.0
            }))
        }
    }
    let bridge = Structure {
        kind: StructureKind::Bridge,
        line: vec![at(0.0, 300.0), at(0.0, 700.0)],
    };
    let route = route_north(&[bridge]).await;
    let world = generate(&route, &mut Valley, &MapData::default(), &mut |_, _| {}).await;

    let mesh = &world.structures;
    assert_valid(mesh);
    assert_faces_follow_normals(mesh);
    let lowest = mesh.vertices.iter().map(|v| v[1]).fold(f32::MAX, f32::min);
    assert!(
        (lowest - 467.5).abs() < 0.1,
        "pillars reach below the valley floor: {lowest}"
    );
    let highest = mesh.vertices.iter().map(|v| v[1]).fold(f32::MIN, f32::max);
    assert!(
        (highest - 501.0).abs() < 0.1,
        "parapets 1 m above the road: {highest}"
    );
    // Nothing outside the bridge.
    assert!(
        mesh.vertices
            .iter()
            .all(|v| (-720.0..=-280.0).contains(&v[2]))
    );
}

#[tokio::test]
async fn tunnels_are_tubes_visible_from_inside() {
    let tunnel = Structure {
        kind: StructureKind::Tunnel,
        line: vec![at(0.0, 300.0), at(0.0, 700.0)],
    };
    let route = route_north(&[tunnel]).await;
    let world = generate(
        &route,
        &mut EastwardSlope,
        &MapData::default(),
        &mut |_, _| {},
    )
    .await;

    let mesh = &world.structures;
    assert_valid(mesh);
    assert_faces_follow_normals(mesh);
    // Inner faces point towards the tunnel axis (x = 0, 500 m + half the radius up).
    let inward = mesh
        .vertices
        .iter()
        .zip(&mesh.normals)
        .filter(|(v, n)| n[0] * v[0] < 0.0 || (n[1] < 0.0 && v[1] > 500.0))
        .count();
    assert!(inward > 0);
    let top = mesh.vertices.iter().map(|v| v[1]).fold(f32::MIN, f32::max);
    assert!((top - 505.0).abs() < 0.1, "arch 5 m high: {top}");
}

#[tokio::test]
async fn tunnels_enter_the_hill_through_a_portal_left_open() {
    // A ridge 60 m high across the road, its flanks rising 0.8 m a metre, and a tunnel mapped
    // from 300 m to 700 m: the ground lies over the 5 m tube from about 432 m to 568 m only.
    // The tube must not stand in the open in front of the hill, nor the hill close its mouth
    // (#135).
    struct Ridge;
    impl ElevationModel for Ridge {
        fn elevation(
            &mut self,
            lat: f64,
            _lon: f64,
        ) -> impl std::future::Future<Output = Result<f64, String>> + Send {
            std::future::ready(Ok(ridge((lat - 46.0) * METERS_PER_DEGREE)))
        }
    }
    fn ridge(north: f64) -> f64 {
        500.0 + (60.0 - 0.8 * (north - 500.0).abs()).max(0.0)
    }
    let tunnel = Structure {
        kind: StructureKind::Tunnel,
        line: vec![at(0.0, 300.0), at(0.0, 700.0)],
    };
    let route = route_north(&[tunnel]).await;
    let world = generate(&route, &mut Ridge, &MapData::default(), &mut |_, _| {}).await;

    let mesh = &world.structures;
    assert_valid(mesh);
    assert_faces_follow_normals(mesh);
    let inside = palette::srgb("structure.tunnel", 0.0);
    let tube: Vec<[f32; 3]> = mesh
        .vertices
        .iter()
        .zip(&mesh.colors)
        .filter(|(_, colour)| **colour == inside)
        .map(|(v, _)| *v)
        .collect();
    // The tube runs through the hill...
    let under_the_top = tube.iter().filter(|v| (v[2] + 500.0).abs() < 20.0).count();
    assert!(under_the_top > 10, "no tube under the hill");
    // ...from where the ground first lies over its crown to where it last does, nowhere in
    // the open.
    let (entrance, exit) = tube.iter().fold((f32::MAX, f32::MIN), |(low, high), v| {
        (low.min(-v[2]), high.max(-v[2]))
    });
    assert!(
        (420.0..445.0).contains(&entrance),
        "the tube begins at {entrance} m"
    );
    assert!((555.0..580.0).contains(&exit), "the tube ends at {exit} m");
    for v in &tube {
        let hill = ridge(f64::from(-v[2]));
        assert!(
            hill > 504.5,
            "the tube stands in the open at {v:?}, the hill at {hill}"
        );
    }

    // At either end a headwall faces out along the road, over the opening and beside it.
    for (portal, facing) in [(entrance, 1.0_f32), (exit, -1.0)] {
        let face: Vec<[f32; 3]> = mesh
            .vertices
            .iter()
            .zip(&mesh.normals)
            .filter(|(v, n)| n[2] * facing > 0.99 && (v[2] + portal).abs() < 0.5)
            .map(|(v, _)| *v)
            .collect();
        assert!(
            face.iter().any(|v| v[0].abs() < 1.0 && v[1] > 505.5),
            "no wall over the opening at {portal} m"
        );
        assert!(
            face.iter().any(|v| v[0] < -5.5) && face.iter().any(|v| v[0] > 5.5),
            "no wall beside the opening at {portal} m"
        );
    }

    // The opening is open: before it the cutting lies at the road, and no ground reaches into
    // it just before the headwall or behind it.
    for (portal, inward) in [(entrance, 1.0_f32), (exit, -1.0)] {
        for x in [-3.0_f32, 0.0, 3.0] {
            let before =
                ground_at(&world, x, -(portal - inward * 3.0)).expect("ground before the portal");
            assert!(
                before < 500.5,
                "the ground at {before} before the portal at {portal} m"
            );
            for step in -12..=12 {
                let north = portal + inward * step as f32 * 0.5;
                if let Some(ground) = ground_at(&world, x, -north) {
                    assert!(
                        !(500.3..504.0).contains(&ground),
                        "ground at {ground} in the opening at {x}, {north} m"
                    );
                }
            }
        }
    }

    // The hill over the tunnel stays as it is, right behind the headwalls too.
    for north in [entrance + 8.0, 470.0, exit - 8.0] {
        let ground = ground_at(&world, 0.0, -north).expect("ground over the tunnel");
        let hill = ridge(f64::from(north)) as f32;
        assert!(
            (ground - hill).abs() < 0.5,
            "the hill at {north} m cut to {ground}, not {hill}"
        );
    }
}

#[tokio::test]
async fn all_world_meshes_face_their_normals() {
    let house = Building {
        id: 7,
        outline: square(60.0, 500.0, 5.0),
        height: Some(9.0),
        levels: None,
        color: None,
    };
    let world = world(&MapData {
        buildings: vec![house],
        ..MapData::default()
    })
    .await;

    assert_faces_follow_normals(&world.road);
    for chunk in &world.chunks {
        assert_faces_follow_normals(&chunk.buildings);
    }
}

#[tokio::test]
async fn ground_never_covers_a_bridge_deck() {
    // Terrain 10 m above the road where the bridge starts (an abutment in a slope).
    struct Bank;
    impl ElevationModel for Bank {
        fn elevation(
            &mut self,
            lat: f64,
            _lon: f64,
        ) -> impl std::future::Future<Output = Result<f64, String>> + Send {
            let north = (lat - 46.0) * METERS_PER_DEGREE;
            std::future::ready(Ok(if (300.0..400.0).contains(&north) {
                510.0
            } else {
                500.0
            }))
        }
    }
    let bridge = Structure {
        kind: StructureKind::Bridge,
        line: vec![at(0.0, 300.0), at(0.0, 700.0)],
    };
    // The route keeps 500 m (file elevations), the bridge spans the bank.
    let route = route_north(&[bridge]).await;
    let world = generate(&route, &mut Bank, &MapData::default(), &mut |_, _| {}).await;

    // Under the deck and its verge the ground stays below it; the bank rises only beside it.
    let under: Vec<_> = terrain_vertices(&world)
        .filter(|v| v[0].abs() <= 6.3 && (-700.0..-300.0).contains(&v[2]))
        .collect();
    assert!(under.len() > 50);
    for v in under {
        assert!(
            v[1] <= 500.0 - ROAD_SINK as f32 + 0.01,
            "terrain above the deck: {v:?}"
        );
    }
}

#[tokio::test]
async fn minimap_draws_map_features_near_the_route_only() {
    let forest = Area {
        cover: LandCover::Forest,
        outer: vec![square(300.0, 500.0, 100.0)],
        inner: vec![],
    };
    let far_lake = Area {
        cover: LandCover::Water,
        outer: vec![square(9000.0, 500.0, 100.0)],
        inner: vec![],
    };
    let house = Building {
        id: 1,
        outline: square(60.0, 500.0, 5.0),
        height: None,
        levels: None,
        color: None,
    };
    let world = world(&MapData {
        areas: vec![forest, far_lake],
        buildings: vec![house],
        ..MapData::default()
    })
    .await;

    let flat = &world.minimap;
    assert_eq!(flat.vertices.len() % 3, 0);
    assert_eq!(flat.vertices.len(), flat.colors.len());
    // Forest square (2 triangles) and house (2 triangles); the lake is 9 km away.
    assert_eq!(flat.vertices.len(), 12);
    assert!(flat.vertices.iter().all(|v| v[0] < 500.0));
}

#[tokio::test]
async fn lakes_are_edged_by_a_band_of_gravel_the_forest_keeps_off() {
    // A pond 100 m square east of the route, in a forest.
    let pond = Area {
        cover: LandCover::Water,
        outer: vec![square(300.0, 500.0, 50.0)],
        inner: vec![],
    };
    let forest = Area {
        cover: LandCover::Forest,
        outer: vec![square(300.0, 500.0, 120.0)],
        inner: vec![],
    };
    let world = world(&MapData {
        areas: vec![forest, pond],
        ..MapData::default()
    })
    .await;
    let band_at = |x: f32, z: f32| height_on(&world, |c| &c.tracks, x, z);

    // All round, just outside the shore, on the ground; none out in the water.
    let mut checked = 0;
    for step in 0..=40 {
        #[allow(clippy::cast_precision_loss)] // small steps
        let along = 255.0 + step as f32 * 2.25;
        for (x, z) in [
            (along, -448.5),
            (along, -551.5),
            (248.5, -(along + 200.0)),
            (351.5, -(along + 200.0)),
        ] {
            let band = band_at(x, z).unwrap_or_else(|| panic!("no shore at {x}, {z}"));
            let ground = ground_at(&world, x, z).expect("ground");
            assert!(
                band > ground && band - ground < 0.03,
                "{band} on {ground} at {x}, {z}"
            );
            checked += 1;
        }
    }
    assert!(checked > 150);
    assert!(band_at(300.0, -500.0).is_none() && band_at(300.0, -453.0).is_none());
    // Trees grow up to the band, not on it.
    let trees = plants_of(&world, &["conifer", "broadleaf"]);
    assert!(trees.len() > 50);
    for [x, _, z] in &trees {
        let outside = (x - 300.0).abs().max((z + 500.0).abs()) - 50.0;
        assert!(
            outside > 3.0 || outside < 0.0,
            "a tree on the shore at {x}, {z}"
        );
    }
}

#[tokio::test]
async fn streams_cut_natural_channels_and_run_on_under_bridges() {
    // A stream 4 m wide crossing the route at 500 m, where the route crosses on a bridge.
    let bridge = Structure {
        kind: StructureKind::Bridge,
        line: vec![at(0.0, 480.0), at(0.0, 520.0)],
    };
    let route = route_north(&[bridge]).await;
    let map = MapData {
        waterways: vec![Waterway {
            width: 4.0,
            line: vec![at(-400.0, 500.0), at(400.0, 500.0)],
        }],
        ..MapData::default()
    };
    let world = generate(&route, &mut EastwardSlope, &map, &mut |_, _| {}).await;

    // Across the stream, 200 m east of the route: the bed deepest in the middle, the banks
    // rising to the land without a wall, the land untouched a few metres out.
    let x = 200.0_f32;
    let land = 500.0 + 0.1 * x;
    let water = height_on(&world, |c| &c.water, x, -500.0).expect("water");
    let profile: Vec<(f32, f32)> = (0..=24)
        .map(|k| {
            #[allow(clippy::cast_precision_loss)] // small steps
            let off = k as f32 * 0.5;
            (
                off,
                ground_at(&world, x, -500.0 - off).expect("ground") - land,
            )
        })
        .collect();
    assert!(
        water - land < -0.4,
        "the water {} m below the land",
        water - land
    );
    assert!(
        profile[0].1 < water - land - 0.2,
        "the bed {} m below the land, the water {}",
        profile[0].1,
        water - land
    );
    for pair in profile.windows(2) {
        let ((a, low), (b, high)) = (pair[0], pair[1]);
        assert!(
            high >= low - 0.01,
            "the bank falls again at {b} m: {profile:?}"
        );
        assert!((high - low) / (b - a) < 1.0, "a wall at {b} m: {profile:?}");
    }
    let (out, top) = profile[profile.len() - 1];
    assert!(top.abs() < 0.02, "the land {top} m off at {out} m");
    // Under the bridge the channel runs on: the ground there lies below the ground under the
    // bridge away from the stream.
    let under = ground_at(&world, 1.0, -500.0).expect("ground under the bridge");
    let beside = ground_at(&world, 1.0, -488.0).expect("ground under the bridge");
    assert!(
        under < beside - 0.5,
        "no channel under the bridge: {under} by {beside}"
    );
}

#[tokio::test]
async fn lakes_lie_level_in_the_land() {
    // A lake east of the road whose surface the terrain model reports at 429 m.
    struct Lake;
    impl ElevationModel for Lake {
        fn elevation(
            &mut self,
            _lat: f64,
            lon: f64,
        ) -> impl std::future::Future<Output = Result<f64, String>> + Send {
            let east = (lon - 7.0) * METERS_PER_DEGREE * 46f64.to_radians().cos();
            std::future::ready(Ok(if east > 50.0 { 429.0 } else { 440.0 }))
        }
    }
    let lake = Area {
        cover: LandCover::Water,
        outer: vec![square(400.0, 500.0, 300.0)],
        inner: vec![],
    };
    let route = route_north(&[]).await;
    let map = MapData {
        areas: vec![lake],
        ..MapData::default()
    };
    let world = generate(&route, &mut Lake, &map, &mut |_, _| {}).await;

    for chunk in &world.chunks {
        assert_valid(&chunk.water);
        assert_faces_follow_normals(&chunk.water);
    }
    // All over the lake, level half a metre below its shore (#93), over its bed.
    for x in (110..=690).step_by(20) {
        for north in (210..=790).step_by(20) {
            #[allow(clippy::cast_precision_loss)] // small numbers
            let (x, z) = (x as f32, -(north as f32));
            let water = height_on(&world, |c| &c.water, x, z).expect("water over the lake");
            assert!(
                (water - 428.52).abs() < 0.01,
                "water at {water} at {x}, {z}"
            );
            let bed = ground_at(&world, x, z).expect("a bed");
            assert!(bed < water - 0.2, "the bed at {bed} under water at {water}");
        }
    }
}

#[tokio::test]
async fn rivers_slope_with_their_course_in_their_channel() {
    // A river 40 m wide running east, where the land rises 0.1 m per metre: its surface,
    // which the terrain model measures, falls 130 m along it.
    let river = Area {
        cover: LandCover::Water,
        outer: vec![vec![
            at(100.0, 480.0),
            at(1400.0, 480.0),
            at(1400.0, 520.0),
            at(100.0, 520.0),
            at(100.0, 480.0),
        ]],
        inner: vec![],
    };
    let world = world(&MapData {
        areas: vec![river],
        ..MapData::default()
    })
    .await;

    let mut checked = 0;
    for x in (110..=1390).step_by(5) {
        #[allow(clippy::cast_precision_loss)] // small numbers
        let x = x as f32;
        for z in [-485.0, -500.0, -515.0] {
            let (Some(water), Some(ground)) = (
                height_on(&world, |c| &c.water, x, z),
                ground_at(&world, x, z),
            ) else {
                continue;
            };
            // In its channel (#93): over its bed, half a metre below the land beside it.
            let land = 500.0 + 0.1 * x;
            assert!(
                water > ground,
                "water at {water} under ground {ground} at {x}, {z}"
            );
            assert!(
                (water - (land - 0.48)).abs() < 0.05,
                "water at {water} by land at {land} at {x}, {z}"
            );
            checked += 1;
        }
    }
    assert!(checked > 500, "{checked} points checked");
    let (low, high) = (
        height_on(&world, |c| &c.water, 150.0, -500.0).expect("water"),
        height_on(&world, |c| &c.water, 1350.0, -500.0).expect("water"),
    );
    assert!(high - low > 100.0, "the river runs level: {low} to {high}");
}

#[tokio::test]
async fn mapped_building_colours_turn_into_palette_colours() {
    // A house mapped in dark grey: it keeps a wall colour of the light pastel palette.
    let house = Building {
        id: 5,
        outline: square(60.0, 500.0, 5.0),
        height: None,
        levels: Some(2.0),
        color: Some([0.15, 0.15, 0.17]),
    };
    let world = world(&MapData {
        buildings: vec![house],
        ..MapData::default()
    })
    .await;

    // Plastered walls carry style code 0 in their alpha.
    let walls: Vec<[f32; 4]> = world
        .chunks
        .iter()
        .flat_map(|c| {
            c.modelled
                .iter()
                .flat_map(|cell| cell.shells.colors.iter())
                .chain(c.buildings.colors.iter())
        })
        .filter(|colour| colour[3] == 0.0)
        .copied()
        .collect();
    assert_ne!(walls.len(), 0, "no plastered walls");
    for [r, g, b, _] in walls {
        let luminance = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        assert!(luminance > 0.6, "dark wall {r} {g} {b}");
    }
}
