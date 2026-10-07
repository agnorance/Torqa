//! OpenStreetMap data for Torqa: buildings, land cover, water, roads, bridges and tunnels around
//! a route, from [`OpenFreeMap`](https://openfreemap.org) vector tiles (`OpenMapTiles` schema),
//! cached on disk for offline rides (R3).
//!
//! © `OpenFreeMap`, © `OpenMapTiles`, data © OpenStreetMap contributors (ODbL 1.0) — attribution
//! must be shown where the data is used.

mod mvt;

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;

use futures::{StreamExt, stream};
use torqa_domain::files::UsedFiles;
use tracing::{debug, info, warn};

/// A position as (latitude, longitude) in degrees.
pub type LatLon = (f64, f64);

/// Vector tile zoom: the most detailed level OpenFreeMap serves (~1.7 km tiles at 46° N).
const ZOOM: u8 = 14;
/// Tiles downloaded at the same time; they come from a CDN.
const PARALLEL_DOWNLOADS: usize = 6;
/// Bumped when the data source or schema changes, so stale cached tiles are not reused.
const CACHE_VERSION: &str = "openfreemap-omt-v1";
/// Describes the current tile set, including the URL of the latest planet snapshot.
const TILEJSON: &str = "https://tiles.openfreemap.org/planet";

/// Errors while getting map data.
#[derive(Debug, thiserror::Error)]
pub enum OsmError {
    /// A tile is neither cached nor downloadable.
    #[error("map data unavailable for tile {0} (offline and not cached?)")]
    Unavailable(String),
    /// A tile could not be decoded.
    #[error("invalid map data: {0}")]
    Invalid(String),
    /// Reading or writing the cache failed.
    #[error("map cache: {0}")]
    Cache(#[from] std::io::Error),
}

/// What covers an area of land.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LandCover {
    /// Forest or woodland.
    Forest,
    /// Meadow, grassland, parks.
    Meadow,
    /// Fields.
    Farmland,
    /// Vineyards and orchards.
    Orchard,
    /// Built-up areas.
    Residential,
    /// Industrial land: halls and warehouses.
    Industrial,
    /// Commercial and retail land: offices and stores.
    Commercial,
    /// Grounds of schools, colleges, universities and hospitals.
    Public,
    /// Lakes, rivers, ponds.
    Water,
    /// Rock, scree, glaciers, sand.
    Rock,
}

/// An area with a land cover; rings are closed (first point repeated at the end).
#[derive(Debug, Clone, PartialEq)]
pub struct Area {
    /// What covers the area.
    pub cover: LandCover,
    /// Outer rings.
    pub outer: Vec<Vec<LatLon>>,
    /// Holes.
    pub inner: Vec<Vec<LatLon>>,
}

/// A building footprint.
#[derive(Debug, Clone, PartialEq)]
pub struct Building {
    /// Stable id (used for deterministic variation).
    pub id: i64,
    /// Closed outline.
    pub outline: Vec<LatLon>,
    /// Height in metres, if known.
    pub height: Option<f64>,
    /// Number of floors, if known.
    pub levels: Option<f64>,
    /// Façade colour as mapped (sRGB, 0–1), if known.
    pub color: Option<[f32; 3]>,
}

/// A river, stream or canal centre line.
#[derive(Debug, Clone, PartialEq)]
pub struct Waterway {
    /// Width in metres (typical for the kind).
    pub width: f64,
    /// Centre line.
    pub line: Vec<LatLon>,
}

/// A road for the minimap.
#[derive(Debug, Clone, PartialEq)]
pub struct Road {
    /// What kind of way it is.
    pub class: RoadClass,
    /// Centre line.
    pub line: Vec<LatLon>,
    /// Carried over or under the ground here, if it is.
    pub structure: Option<StructureKind>,
}

impl Road {
    /// Through roads (primary, secondary, ...) rather than local streets, tracks and paths.
    #[must_use]
    pub fn major(&self) -> bool {
        self.class == RoadClass::Major
    }
}

/// Kinds of ways, widest first (and ordered so: `Major < Street < … < Path`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RoadClass {
    /// Motorways, trunk, primary and secondary roads.
    Major,
    /// Tertiary roads and local streets.
    Street,
    /// Service roads: driveways, car parks, farm access.
    Service,
    /// Farm and forest tracks.
    Track,
    /// Footpaths and cycleways.
    Path,
}

/// Whether a road is carried over or under the ground.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StructureKind {
    /// Bridge, viaduct.
    Bridge,
    /// Tunnel, covered road.
    Tunnel,
}

/// A road bridge or tunnel: a road the route rides, where it is one.
#[derive(Debug, Clone, PartialEq)]
pub struct Structure {
    /// Bridge or tunnel.
    pub kind: StructureKind,
    /// Centre line of the road on the structure.
    pub line: Vec<LatLon>,
}

/// A railway line: main lines, narrow gauge, funiculars and light rail (not trams, which run
/// in the streets, nor subways, which run underground).
#[derive(Debug, Clone, PartialEq)]
pub struct Railway {
    /// Centre line.
    pub line: Vec<LatLon>,
    /// Carried over or under the ground here, if it is.
    pub structure: Option<StructureKind>,
    /// A funicular, which may climb far more steeply than other railways.
    pub funicular: bool,
}

/// Map features around a route.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MapData {
    /// Building footprints.
    pub buildings: Vec<Building>,
    /// Land cover areas.
    pub areas: Vec<Area>,
    /// Rivers and streams.
    pub waterways: Vec<Waterway>,
    /// Roads.
    pub roads: Vec<Road>,
    /// Railways.
    pub railways: Vec<Railway>,
    /// Churches and chapels, as points on or near their building.
    pub churches: Vec<LatLon>,
    /// Shops, cafés, restaurants and the like, as points in or by their building.
    pub shops: Vec<LatLon>,
    /// Hotels, guest houses and hostels, as points in or by their building.
    pub hotels: Vec<LatLon>,
    /// Offices, as points in or by their building.
    pub offices: Vec<LatLon>,
    /// Schools, hospitals, town halls, libraries, post offices, police and fire stations, as
    /// points in or by their building.
    pub public: Vec<LatLon>,
    /// Castles (not ruins), as points on or near their keep.
    pub castles: Vec<LatLon>,
    /// Lighthouses, as points on their tower or where it stands when it has no outline.
    pub lighthouses: Vec<LatLon>,
}

/// Downloads and caches map tiles.
pub struct Osm {
    cache_dir: PathBuf,
    client: reqwest::Client,
    online: bool,
    used: UsedFiles,
}

impl Osm {
    /// Creates a downloader caching under `cache_dir`.
    ///
    /// # Panics
    /// If the HTTP client cannot be initialised (no TLS backend), which is a build error.
    #[must_use]
    pub fn new(cache_dir: PathBuf) -> Self {
        let client = reqwest::Client::builder()
            .user_agent(concat!(
                "Torqa/",
                env!("CARGO_PKG_VERSION"),
                " (+https://github.com/bossm8/torqa)"
            ))
            .timeout(Duration::from_secs(30))
            .build()
            .expect("HTTP client with TLS");
        Self {
            cache_dir,
            client,
            online: true,
            used: UsedFiles::default(),
        }
    }

    /// Uses only cached tiles and never downloads.
    #[must_use]
    pub fn offline(mut self) -> Self {
        self.online = false;
        self
    }

    /// Records every cached tile file read or written in `used`.
    #[must_use]
    pub fn recording(mut self, used: UsedFiles) -> Self {
        self.used = used;
        self
    }

    /// Map features within `corridor` metres of the polyline `points`. `progress` is called
    /// with (tiles done, tiles total) as tiles arrive.
    ///
    /// Tiles that cannot be loaded are skipped with a warning, so partial data is still
    /// returned; the error is only reported if no tile could be loaded at all.
    ///
    /// # Errors
    /// [`OsmError`] if none of the needed tiles is available.
    pub async fn around(
        &self,
        points: &[LatLon],
        corridor: f64,
        progress: &mut (dyn FnMut(usize, usize) + Send),
    ) -> Result<MapData, OsmError> {
        let tiles = tiles_near(points, corridor);
        let total = tiles.len();
        info!(tiles = total, "loading map data");
        progress(0, total);
        let template = self.template().await;
        let template = template.as_deref();
        let mut downloads = stream::iter(tiles)
            .map(|tile| async move { (tile, self.tile(tile, template).await) })
            .buffer_unordered(PARALLEL_DOWNLOADS);
        // Download in parallel, but merge in tile order so the data does not depend on
        // download timing.
        let mut results = Vec::with_capacity(total);
        while let Some(result) = downloads.next().await {
            results.push(result);
            progress(results.len(), total);
        }
        results.sort_by_key(|(tile, _)| *tile);
        let mut data = MapData::default();
        let mut loaded = 0;
        let mut last_error = None;
        for (tile, result) in results {
            match result.and_then(|bytes| mvt::merge(tile, bytes, &mut data)) {
                Ok(()) => loaded += 1,
                Err(error) => {
                    warn!(tile = %tile_name(tile), %error, "map tile unavailable");
                    last_error = Some(error);
                }
            }
        }
        match last_error {
            Some(error) if loaded == 0 => Err(error),
            _ => Ok(data),
        }
    }

    /// The tile URL template of the latest snapshot, if online.
    async fn template(&self) -> Option<String> {
        if !self.online {
            return None;
        }
        let response = self.client.get(TILEJSON).send().await.ok()?;
        let tilejson: serde_json::Value = response.json().await.ok()?;
        let template = tilejson["tiles"][0].as_str().map(ToOwned::to_owned);
        if template.is_none() {
            warn!("map tile index without tile URL");
        }
        template
    }

    async fn tile(&self, tile: (u32, u32), template: Option<&str>) -> Result<Vec<u8>, OsmError> {
        let path = self
            .cache_dir
            .join(CACHE_VERSION)
            .join(ZOOM.to_string())
            .join(tile.0.to_string())
            .join(format!("{}.pbf", tile.1));
        match tokio::fs::read(&path).await {
            Ok(bytes) => {
                self.used.record(&path);
                return Ok(bytes);
            }
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.into()),
            Err(_) => {}
        }
        let Some(template) = template else {
            return Err(OsmError::Unavailable(tile_name(tile)));
        };
        let url = template
            .replace("{z}", &ZOOM.to_string())
            .replace("{x}", &tile.0.to_string())
            .replace("{y}", &tile.1.to_string());
        debug!(%url, "downloading map tile");
        let bytes = self
            .download(&url)
            .await
            .ok_or_else(|| OsmError::Unavailable(tile_name(tile)))?;
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        // Write atomically, so an interrupted download never leaves a broken cache file.
        let partial = path.with_extension("part");
        tokio::fs::write(&partial, &bytes).await?;
        tokio::fs::rename(&partial, &path).await?;
        self.used.record(&path);
        Ok(bytes)
    }

    /// Downloads with one retry, as CDNs occasionally drop a request.
    async fn download(&self, url: &str) -> Option<Vec<u8>> {
        for attempt in 1..=2 {
            match self.client.get(url).send().await {
                Ok(response) if response.status().is_success() => match response.bytes().await {
                    Ok(bytes) => return Some(bytes.to_vec()),
                    Err(error) => warn!(%url, %error, attempt, "map download failed"),
                },
                Ok(response) => {
                    warn!(%url, status = %response.status(), attempt, "map server refused");
                }
                Err(error) => warn!(%url, %error, attempt, "map download failed"),
            }
        }
        None
    }
}

/// Tiles (x, y at [`ZOOM`]) within `corridor` metres of the polyline.
fn tiles_near(points: &[LatLon], corridor: f64) -> BTreeSet<(u32, u32)> {
    const METERS_PER_DEGREE: f64 = 111_195.0;
    let mut tiles = BTreeSet::new();
    for &(lat, lon) in points {
        let d_lat = corridor / METERS_PER_DEGREE;
        let d_lon = corridor / (METERS_PER_DEGREE * lat.to_radians().cos().max(0.01));
        let (west, north) = tile_of(lat + d_lat, lon - d_lon);
        let (east, south) = tile_of(lat - d_lat, lon + d_lon);
        for x in west..=east {
            for y in north..=south {
                tiles.insert((x, y));
            }
        }
    }
    tiles
}

/// The Web Mercator tile containing a position.
fn tile_of(lat: f64, lon: f64) -> (u32, u32) {
    let n = f64::from(1u32 << ZOOM);
    let lat = lat.clamp(-85.05, 85.05).to_radians();
    let x = (lon + 180.0) / 360.0 * n;
    let y = (1.0 - (lat.tan() + 1.0 / lat.cos()).ln() / std::f64::consts::PI) / 2.0 * n;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // clamped to the map
    let clamp = |v: f64| v.floor().clamp(0.0, n - 1.0) as u32;
    (clamp(x), clamp(y))
}

fn tile_name((x, y): (u32, u32)) -> String {
    format!("{ZOOM}/{x}/{y}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The test tile's coordinates (see `tests/data`).
    const TEST_TILE: (u32, u32) = (8531, 5767);

    fn cache_with_test_tile(name: &str) -> PathBuf {
        let cache = std::env::temp_dir().join(format!("torqa-osm-{name}-{}", std::process::id()));
        let dir = cache.join(CACHE_VERSION).join("14").join("8531");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("5767.pbf"),
            include_bytes!("../tests/data/14_8531_5767.pbf"),
        )
        .unwrap();
        cache
    }

    #[test]
    fn covers_the_corridor_with_tiles() {
        // Wabern, near the Gurten: inside one tile with a small corridor.
        assert_eq!(
            tiles_near(&[(46.9296, 7.4512)], 50.0),
            BTreeSet::from([TEST_TILE])
        );
        // A 1.5 km corridor reaches into the neighbouring tiles.
        let wide = tiles_near(&[(46.9296, 7.4512)], 1500.0);
        assert!(wide.contains(&TEST_TILE) && wide.len() >= 4);
    }

    #[tokio::test]
    async fn offline_without_cache_is_unavailable() {
        let cache = std::env::temp_dir().join(format!("torqa-osm-empty-{}", std::process::id()));
        let osm = Osm::new(cache).offline();

        let result = osm.around(&[(46.9296, 7.4512)], 50.0, &mut |_, _| {}).await;

        assert!(matches!(result, Err(OsmError::Unavailable(_))));
    }

    #[tokio::test]
    async fn reads_cached_tiles_offline_with_progress() {
        let cache = cache_with_test_tile("offline");
        let osm = Osm::new(cache.clone()).offline();
        let mut reports = Vec::new();

        let data = osm
            .around(&[(46.9296, 7.4512)], 50.0, &mut |done, total| {
                reports.push((done, total));
            })
            .await
            .unwrap();

        assert!(
            data.buildings.len() > 50,
            "{} buildings",
            data.buildings.len()
        );
        assert!(data.roads.len() > 10);
        assert!(data.areas.iter().any(|a| a.cover == LandCover::Forest));
        assert_eq!(reports, [(0, 1), (1, 1)]);
        std::fs::remove_dir_all(cache).unwrap();
    }

    /// Downloads real data; run with `cargo test -p torqa-osm -- --ignored`.
    #[tokio::test]
    #[ignore = "needs network"]
    async fn live_download_around_the_gurten() {
        let cache = std::env::temp_dir().join(format!("torqa-osm-live-{}", std::process::id()));
        let osm = Osm::new(cache);

        let started = std::time::Instant::now();
        let data = osm
            .around(&[(46.925, 7.445), (46.918, 7.44)], 1500.0, &mut |_, _| {})
            .await
            .unwrap();

        let structures = data.roads.iter().filter(|r| r.structure.is_some()).count();
        println!(
            "{:?}: {} buildings, {} areas, {} waterways, {} roads ({} bridges or tunnels)",
            started.elapsed(),
            data.buildings.len(),
            data.areas.len(),
            data.waterways.len(),
            data.roads.len(),
            structures
        );
        assert!(!data.buildings.is_empty() && structures > 0);
    }
}
