//! Routes for Torqa: GPX import (R11), elevation correction and smoothing (R12) and position,
//! elevation and gradient lookup along the route.

pub mod climbs;
mod curve;
mod gpx;
mod passes;
mod projection;
mod snap;
mod structures;
mod turns;

pub use climbs::{Climb, ClimbCategory};
pub use curve::catmull_rom;
pub use projection::LocalProjection;

use std::f64::consts::{PI, TAU};

use torqa_domain::units::{GradePercent, Meters};
use torqa_osm::MapData;
use torqa_terrain::Terrain;
use tracing::warn;

use gpx::RawPoint;

/// Distance between resampled route points.
const SPACING: f64 = 10.0;
/// Moving-average window for terrain-model elevations, which are already clean.
const TERRAIN_SMOOTHING: f64 = 40.0;
/// Moving-average window for recorded elevations, which are noisy (GPS, barometer drift).
const GPX_SMOOTHING: f64 = 100.0;
/// A rise or fall the terrain model has along the road that no road climbs (#172): a road on
/// a ledge or in a gallery the map does not know, where a 30 m terrain cell straddles the cliff
/// above. A stretch rising steeper than this and falling as steeply back to where it started
/// within `SPIKE_WIDTH` is a spike, and is cut straight across; a real climb, however steep,
/// goes on and is kept, and so is a hill the road rolls over gently on one side.
const SPIKE_GRADE: f64 = 0.18;
const SPIKE_WIDTH: f64 = 400.0;
/// Where the terrain model departs from the file's own elevations by more than this, beyond
/// the offset between the two along the stretch, the model is wrong there and the file wins:
/// a planner's profile is corrected already, and a recorder's barometer drifts by metres, never
/// by a cliff (#172)...
const FILE_DISAGREEMENT: f64 = 30.0;
/// ...the offset taken as the median of their difference over this far either side...
const FILE_OFFSET_REACH: f64 = 500.0;
/// ...and the model wrong out to where it agrees with the file again, within this.
const FILE_AGREEMENT: f64 = 5.0;
const EARTH_RADIUS: f64 = 6_371_000.0;
/// How far either side of a position the curve is looked at for its direction and bend: short
/// against the 10 m between points, long enough to be steady.
const CURVE_STEP_M: f64 = 2.0;

/// Errors while importing a route.
#[derive(Debug, thiserror::Error)]
pub enum RouteError {
    /// The file is not valid GPX.
    #[error("invalid GPX: {0}")]
    InvalidGpx(String),
    /// The file has fewer than two distinct points.
    #[error("route needs at least two distinct points")]
    TooShort,
    /// Neither the file nor the terrain model provided elevations.
    #[error("route has no elevation data and the terrain model is unavailable")]
    NoElevation,
}

/// A recorded position with its time, e.g. from an activity to follow as a ghost.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimedPoint {
    /// Latitude in degrees (WGS84).
    pub lat: f64,
    /// Longitude in degrees (WGS84).
    pub lon: f64,
    /// Seconds since the Unix epoch.
    pub time: f64,
}

/// The timed positions of a recorded GPX activity; points without a time are left out.
///
/// # Errors
/// [`RouteError::InvalidGpx`] if the file is not valid GPX.
pub fn timed_points(xml: &str) -> Result<Vec<TimedPoint>, RouteError> {
    Ok(gpx::parse(xml)?
        .points
        .into_iter()
        .filter_map(|p| {
            p.time.map(|time| TimedPoint {
                lat: p.lat,
                lon: p.lon,
                time,
            })
        })
        .collect())
}

/// The name a GPX file gives its route, if it is valid and has one; without building it.
#[must_use]
pub fn route_name(xml: &str) -> Option<String> {
    gpx::parse(xml).ok()?.name
}

/// The positions of a GPX file as (latitude, longitude), without building a route; for
/// fetching data along it before importing.
///
/// # Errors
/// [`RouteError::InvalidGpx`] if the file is not valid GPX.
pub fn track_points(xml: &str) -> Result<Vec<(f64, f64)>, RouteError> {
    Ok(gpx::parse(xml)?
        .points
        .iter()
        .map(|p| (p.lat, p.lon))
        .collect())
}

/// Where a route's elevations come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElevationSource {
    /// Terrain model (corrected, R12).
    Terrain,
    /// Elevations recorded in the file.
    File,
}

/// A point of the resampled route.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RoutePoint {
    /// Latitude in degrees (WGS84).
    pub lat: f64,
    /// Longitude in degrees (WGS84).
    pub lon: f64,
    /// Smoothed elevation.
    pub elevation: Meters,
    /// Distance from the start along the route.
    pub distance: Meters,
    /// What carries the road here.
    pub surface: Surface,
}

/// What carries the road at a point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Surface {
    /// The ground.
    #[default]
    Ground,
    /// A bridge: the road runs above the terrain.
    Bridge,
    /// A tunnel: the road runs below the terrain.
    Tunnel,
}

/// Where a rider is on the route.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RoutePosition {
    /// Latitude in degrees (WGS84).
    pub lat: f64,
    /// Longitude in degrees (WGS84).
    pub lon: f64,
    /// Elevation.
    pub elevation: Meters,
    /// Gradient of the road at this point.
    pub grade: GradePercent,
    /// Direction of travel in radians, clockwise from north: the road's own direction, which
    /// turns smoothly through bends.
    pub heading: f64,
    /// How sharply the road bends here, in radians per metre (1 / radius): positive in bends
    /// to the right, negative to the left, 0 on the straight.
    pub curvature: f64,
}

/// A route ready to ride: evenly resampled, with smoothed elevations.
#[derive(Debug, Clone)]
pub struct Route {
    /// Fingerprint of the track as recorded (see [`Route::key`]).
    key: String,
    name: Option<String>,
    points: Vec<RoutePoint>,
    elevation_source: ElevationSource,
    climbs: Vec<Climb>,
    /// Stretches taken out (see [`Route::without_turns_in_place`]).
    cuts: Vec<turns::Cut>,
}

impl Route {
    /// Imports a GPX route.
    ///
    /// With a `terrain`, elevations come from the terrain model; if it cannot provide them (e.g.
    /// offline without cached tiles), the file's elevations are used instead.
    ///
    /// # Errors
    /// [`RouteError`] if the file is invalid, too short, or no elevations are available.
    pub async fn from_gpx(xml: &str, terrain: Option<&mut Terrain>) -> Result<Self, RouteError> {
        Self::from_gpx_with(xml, terrain, &MapData::default()).await
    }

    /// Like [`Route::from_gpx`], with any [`ElevationModel`] and the `map` around the route:
    /// the track is put onto the roads it rides (GPS wander and corners cut between sparse
    /// points removed), and on bridges and tunnels the elevation runs straight from one end to
    /// the other instead of following the ground (or water) below or the mountain above.
    ///
    /// # Errors
    /// [`RouteError`] if the file is invalid, too short, or no elevations are available.
    pub async fn from_gpx_with<M: ElevationModel>(
        xml: &str,
        model: Option<&mut M>,
        map: &MapData,
    ) -> Result<Self, RouteError> {
        let gpx = gpx::parse(xml)?;
        let recorded = dedup(gpx.points);
        // From the track as recorded: rides and records on a file keep matching as the map
        // (and the matching) changes.
        let key = key_of(&resample(&recorded)?);
        let snapped = snap::to_roads(&recorded, &map.roads);
        let mut track = dedup(snapped.track);
        // The file's own elevations, kept beside the model's to check it against (#172).
        let file_track = track.clone();

        // The model is sampled only at the file's points, which lie on the road. Between
        // sparse points a straight line can cut across a hillside, so elevations there are
        // interpolated rather than sampled.
        let mut source = None;
        if let Some(model) = model {
            match model_elevations(&track, model).await {
                Ok(elevations) => {
                    for (point, elevation) in track.iter_mut().zip(elevations) {
                        point.elevation = Some(elevation);
                    }
                    source = Some(ElevationSource::Terrain);
                }
                Err(error) => warn!(%error, "terrain model unavailable, using file elevations"),
            }
        }
        let source = match source {
            Some(source) => source,
            None if fill_gaps(&mut track) => ElevationSource::File,
            None if track.len() < 2 => return Err(RouteError::TooShort),
            None => return Err(RouteError::NoElevation),
        };
        let mut points = resample(&track)?;
        let mut surfaces = structures::surfaces(&points, &snapped.structures);
        structures::keep_real(&points, &mut surfaces, source == ElevationSource::Terrain);
        structures::bridge_elevations(&mut points, &surfaces);
        let file_elevations: Vec<Option<f64>> = if source == ElevationSource::Terrain {
            resample(&file_track)?.iter().map(|p| p.elevation).collect()
        } else {
            Vec::new()
        };

        let window = match source {
            ElevationSource::Terrain => TERRAIN_SMOOTHING,
            ElevationSource::File => GPX_SMOOTHING,
        };
        let mut raw: Vec<f64> = points
            .iter()
            .map(|p| p.elevation.unwrap_or_default())
            .collect();
        if source == ElevationSource::Terrain {
            trust_the_file_where_the_model_jumps(&mut raw, &file_elevations);
            cut_spikes(&mut raw);
        }
        let mut smoothed = smooth(&raw, window);
        passes::join(&points, &surfaces, &mut smoothed);

        let mut distance = 0.0;
        let points = points
            .iter()
            .zip(smoothed)
            .zip(surfaces)
            .enumerate()
            .map(|(i, ((p, elevation), surface))| {
                if i > 0 {
                    distance += haversine(&points[i - 1], p);
                }
                RoutePoint {
                    lat: p.lat,
                    lon: p.lon,
                    elevation: Meters(elevation),
                    distance: Meters(distance),
                    surface,
                }
            })
            .collect();

        let points: Vec<RoutePoint> = points;
        Ok(Self {
            key,
            name: gpx.name,
            climbs: climbs::detect(&points),
            points,
            elevation_source: source,
            cuts: Vec::new(),
        })
    }

    /// The route without its turns in place, for riding in 3D (#101): where the track runs a
    /// few dozen metres into a side road or past a junction and straight back, it rides on
    /// instead. Longer turns back stay as planned. Videos keep the track as recorded: they show
    /// what was ridden.
    #[must_use]
    pub fn without_turns_in_place(mut self) -> Self {
        let cuts = turns::straighten(&mut self.points);
        if !cuts.is_empty() {
            self.climbs = climbs::detect(&self.points);
        }
        self.cuts.extend(cuts);
        self
    }

    /// The distance along the track as recorded of a place `distance` along this route: further
    /// on by the turns in place taken out before it.
    #[must_use]
    pub fn recorded_distance(&self, distance: Meters) -> Meters {
        Meters(self.cuts.iter().rev().fold(
            distance.0,
            |d, cut| {
                if d > cut.at { d + cut.length } else { d }
            },
        ))
    }

    /// Name from the file, if any.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// Where the elevations come from.
    #[must_use]
    pub fn elevation_source(&self) -> ElevationSource {
        self.elevation_source
    }

    /// The resampled points.
    #[must_use]
    pub fn points(&self) -> &[RoutePoint] {
        &self.points
    }

    /// The climbs along the route, in order.
    #[must_use]
    pub fn climbs(&self) -> &[Climb] {
        &self.climbs
    }

    /// A fingerprint of the route's course: equal for the same track ridden again (e.g. the same
    /// GPX file or course), so rides on it can be compared. Ignores name and elevations.
    #[must_use]
    pub fn key(&self) -> String {
        self.key.clone()
    }

    /// Total length.
    #[must_use]
    pub fn length(&self) -> Meters {
        self.points.last().map_or(Meters(0.0), |p| p.distance)
    }

    /// Sum of all climbs.
    #[must_use]
    pub fn elevation_gain(&self) -> Meters {
        Meters(
            self.points
                .windows(2)
                .map(|w| (w[1].elevation.0 - w[0].elevation.0).max(0.0))
                .sum(),
        )
    }

    /// Total climbing still ahead from `distance` along the route: the rises between its
    /// points from there to the end, the first one from the height at `distance`.
    #[must_use]
    pub fn ascent_ahead(&self, distance: Meters) -> Meters {
        let here = self.position(distance).elevation.0;
        let mut ahead = 0.0;
        let mut last = here;
        for point in self.points.iter().filter(|p| p.distance.0 > distance.0) {
            ahead += (point.elevation.0 - last).max(0.0);
            last = point.elevation.0;
        }
        Meters(ahead)
    }

    /// The steepest climbing gradient anywhere on the route.
    #[must_use]
    pub fn max_grade(&self) -> GradePercent {
        let steepest = self
            .points
            .windows(2)
            .filter(|w| w[1].distance.0 > w[0].distance.0)
            .map(|w| (w[1].elevation.0 - w[0].elevation.0) / (w[1].distance.0 - w[0].distance.0))
            .fold(0.0, f64::max);
        GradePercent(steepest * 100.0)
    }

    /// Position, elevation, gradient, heading and curvature at a distance from the start
    /// (clamped to the route). The position lies on the smooth curve through the route's points
    /// that the road is drawn along ([`catmull_rom`]).
    #[must_use]
    pub fn position(&self, distance: Meters) -> RoutePosition {
        let along = distance.0.clamp(0.0, self.length().0);
        let (index, fraction) = self.segment_at(along);
        let (start, end) = (&self.points[index], &self.points[index + 1]);
        let span = end.distance.0 - start.distance.0;
        let rise = end.elevation.0 - start.elevation.0;
        let projection = LocalProjection::for_route(self);
        let here = self.curve_point(&projection, along);
        let (lat, lon) = projection.unproject(here.0, here.1);
        // The curve a little either side gives the direction and how it changes.
        let behind = self.curve_point(&projection, along - CURVE_STEP_M);
        let ahead = self.curve_point(&projection, along + CURVE_STEP_M);
        let direction = |a: (f64, f64), b: (f64, f64)| {
            ((b.0 - a.0).hypot(b.1 - a.1) > 1e-9).then(|| (b.0 - a.0).atan2(b.1 - a.1))
        };
        let heading = direction(behind, ahead).unwrap_or_else(|| heading(start, end));
        let curvature = match (direction(behind, here), direction(here, ahead)) {
            (Some(before), Some(after)) => {
                let turn = (after - before + PI).rem_euclid(TAU) - PI;
                turn / f64::midpoint(
                    distance_between(behind, here),
                    distance_between(here, ahead),
                )
            }
            _ => 0.0,
        };
        RoutePosition {
            lat,
            lon,
            elevation: Meters(start.elevation.0 + rise * fraction),
            grade: GradePercent(if span > 0.0 { rise / span * 100.0 } else { 0.0 }),
            heading,
            curvature,
        }
    }

    /// The segment containing `along` metres (clamped to the route) and how far along it.
    fn segment_at(&self, along: f64) -> (usize, f64) {
        let along = along.clamp(0.0, self.length().0);
        // The route always has at least two points.
        let index = self
            .points
            .partition_point(|p| p.distance.0 <= along)
            .clamp(1, self.points.len() - 1)
            - 1;
        let (start, end) = (&self.points[index], &self.points[index + 1]);
        let span = end.distance.0 - start.distance.0;
        let fraction = if span > 0.0 {
            (along - start.distance.0) / span
        } else {
            0.0
        };
        (index, fraction)
    }

    /// The point `along` metres from the start on the curve through the route's points, in
    /// `projection`'s metres.
    fn curve_point(&self, projection: &LocalProjection, along: f64) -> (f64, f64) {
        let (index, fraction) = self.segment_at(along);
        let last = self.points.len() - 1;
        let at = |i: usize| projection.project(self.points[i].lat, self.points[i].lon);
        catmull_rom(
            [
                at(index.saturating_sub(1)),
                at(index),
                at(index + 1),
                at((index + 2).min(last)),
            ],
            fraction,
        )
    }
}

fn distance_between(a: (f64, f64), b: (f64, f64)) -> f64 {
    (b.0 - a.0).hypot(b.1 - a.1)
}

/// Direction from `a` to `b` in radians, clockwise from north (flat-earth approximation,
/// exact enough over a 10 m segment).
fn heading(a: &RoutePoint, b: &RoutePoint) -> f64 {
    let east = (b.lon - a.lon) * a.lat.to_radians().cos();
    let north = b.lat - a.lat;
    east.atan2(north)
}

/// Removes consecutive points closer than 10 cm, which would create zero-length segments.
/// FNV-1a over the position every 100 m (of points every 10 m), rounded to about 10 m.
fn key_of(points: &[RawPoint]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut feed = |value: i64| {
        for byte in value.to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    };
    #[allow(clippy::cast_possible_truncation)] // degrees × 10⁴ fit easily
    for point in points.iter().step_by(10) {
        feed((point.lat * 1e4).round() as i64);
        feed((point.lon * 1e4).round() as i64);
    }
    feed(i64::try_from(points.len()).unwrap_or(i64::MAX));
    format!("{hash:016x}")
}

fn dedup(points: Vec<RawPoint>) -> Vec<RawPoint> {
    let mut result: Vec<RawPoint> = Vec::with_capacity(points.len());
    for point in points {
        match result.last_mut() {
            Some(last) if haversine(last, &point) < 0.1 => {
                if last.elevation.is_none() {
                    last.elevation = point.elevation;
                }
            }
            _ => result.push(point),
        }
    }
    result
}

/// Resamples the polyline every [`SPACING`] metres, keeping the exact end point. Elevations are
/// interpolated between known values; unknown ones stay `None`.
fn resample(points: &[RawPoint]) -> Result<Vec<RawPoint>, RouteError> {
    if points.len() < 2 {
        return Err(RouteError::TooShort);
    }
    let mut result = vec![points[0]];
    let mut next = SPACING; // distance of the next sample from the start
    let mut travelled = 0.0;
    for pair in points.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        let length = haversine(a, b);
        while next <= travelled + length {
            let t = (next - travelled) / length;
            result.push(RawPoint {
                lat: a.lat + (b.lat - a.lat) * t,
                lon: a.lon + (b.lon - a.lon) * t,
                elevation: a
                    .elevation
                    .zip(b.elevation)
                    .map(|(ea, eb)| ea + (eb - ea) * t),
                time: None,
            });
            next += SPACING;
        }
        travelled += length;
    }
    let end = points[points.len() - 1];
    if result
        .last()
        .is_some_and(|last| haversine(last, &end) >= 0.1)
    {
        result.push(end);
    }
    Ok(result)
}

/// A source of ground elevations, such as the terrain model.
pub trait ElevationModel {
    /// Ground elevation in metres at a WGS84 position.
    fn elevation(
        &mut self,
        lat: f64,
        lon: f64,
    ) -> impl std::future::Future<Output = Result<f64, String>> + Send;
}

impl ElevationModel for Terrain {
    async fn elevation(&mut self, lat: f64, lon: f64) -> Result<f64, String> {
        Terrain::elevation(self, lat, lon)
            .await
            .map_err(|e| e.to_string())
    }
}

async fn model_elevations<M: ElevationModel>(
    points: &[RawPoint],
    model: &mut M,
) -> Result<Vec<f64>, String> {
    let mut elevations = Vec::with_capacity(points.len());
    for point in points {
        elevations.push(model.elevation(point.lat, point.lon).await?);
    }
    Ok(elevations)
}

/// Fills missing elevations by linear interpolation (nearest value at the ends).
/// Returns `false` if there is no elevation at all.
fn fill_gaps(points: &mut [RawPoint]) -> bool {
    let known: Vec<usize> = (0..points.len())
        .filter(|&i| points[i].elevation.is_some())
        .collect();
    let (Some(&first), Some(&last)) = (known.first(), known.last()) else {
        return false;
    };
    let value = |points: &[RawPoint], i: usize| points[i].elevation.unwrap_or_default();
    let (start, end) = (value(points, first), value(points, last));
    for point in &mut points[..first] {
        point.elevation = Some(start);
    }
    for point in &mut points[last + 1..] {
        point.elevation = Some(end);
    }
    for pair in known.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let (ea, eb) = (value(points, a), value(points, b));
        for (offset, point) in points[a + 1..b].iter_mut().enumerate() {
            #[allow(clippy::cast_precision_loss)] // indices are far below 2^52
            let t = (offset + 1) as f64 / (b - a) as f64;
            point.elevation = Some(ea + (eb - ea) * t);
        }
    }
    true
}

/// Cuts the terrain model's spikes along the road (#172): where the profile rises steeper than
/// `SPIKE_GRADE` and falls back as steeply to its starting level within `SPIKE_WIDTH`, or dips
/// and climbs back the same way, the stretch is drawn straight across from where it left the
/// level to where it returned. A climb that goes on is reachable at road grades and stays,
/// however steep; so does a knoll the road leaves gently on one side.
fn cut_spikes(values: &mut [f64]) {
    let room = SPIKE_GRADE * SPACING;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // a positive count
    let width = (SPIKE_WIDTH / SPACING).round() as usize;
    let mut i = 0;
    while i + 1 < values.len() {
        let step = values[i + 1] - values[i];
        if step.abs() <= room {
            i += 1;
            continue;
        }
        // Up or down a flank too steep for a road: look for the way back to this level, as
        // steep somewhere on the way, within the width of a spike.
        let sign = step.signum();
        let mut steep_back = false;
        let mut returned = None;
        for j in i + 2..=(i + width).min(values.len() - 1) {
            if sign * (values[j - 1] - values[j]) > room {
                steep_back = true;
            }
            if sign * (values[j] - values[i]) <= room {
                returned = steep_back.then_some(j);
                break;
            }
        }
        let Some(j) = returned else {
            i += 1;
            continue;
        };
        let (from, to) = (values[i], values[j]);
        #[allow(clippy::cast_precision_loss)] // a few dozen samples
        let span = (j - i) as f64;
        for (k, value) in values[i..=j].iter_mut().enumerate() {
            #[allow(clippy::cast_precision_loss)]
            let t = k as f64 / span;
            *value = from + (to - from) * t;
        }
        i = j;
    }
}

/// Where the model's `values` depart from the `file`'s elevations by more than
/// `FILE_DISAGREEMENT`, beyond the offset between the two along the stretch (the median of
/// their difference over `FILE_OFFSET_REACH` either side), the model is wrong there: the whole
/// departure, out to where the two agree again within `FILE_AGREEMENT`, takes the file's
/// elevation plus that offset instead (#172). Nothing changes without file elevations.
fn trust_the_file_where_the_model_jumps(values: &mut [f64], file: &[Option<f64>]) {
    if file.len() != values.len() || file.iter().filter(|e| e.is_some()).count() < 2 {
        return;
    }
    let differences: Vec<Option<f64>> = values
        .iter()
        .zip(file)
        .map(|(model, file)| file.map(|f| model - f))
        .collect();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // a positive count
    let reach = (FILE_OFFSET_REACH / SPACING).round() as usize;
    // Each sample's departure from the offset along its stretch; none without a file value.
    let departures: Vec<Option<f64>> = (0..values.len())
        .map(|i| {
            let own = differences[i]?;
            let mut nearby: Vec<f64> = differences
                [i.saturating_sub(reach)..(i + reach + 1).min(differences.len())]
                .iter()
                .flatten()
                .copied()
                .collect();
            nearby.sort_by(f64::total_cmp);
            Some(own - nearby[nearby.len() / 2])
        })
        .collect();
    let departs = |i: usize, by: f64| departures[i].is_some_and(|d| d.abs() > by);
    let mut wrong = vec![false; values.len()];
    for i in 0..values.len() {
        if !departs(i, FILE_DISAGREEMENT) {
            continue;
        }
        wrong[i] = true;
        // Out to where the model and the file agree again.
        let mut k = i;
        while k > 0 && departs(k - 1, FILE_AGREEMENT) {
            k -= 1;
            wrong[k] = true;
        }
        let mut k = i;
        while k + 1 < values.len() && departs(k + 1, FILE_AGREEMENT) {
            k += 1;
            wrong[k] = true;
        }
    }
    for i in 0..values.len() {
        if wrong[i]
            && let Some(departure) = departures[i]
        {
            // The file's elevation at the offset: the model less its departure.
            values[i] -= departure;
        }
    }
}

/// Smooths evenly spaced samples with two passes of a centred moving average over `window`
/// metres, which approximates a Gaussian and leaves far less ripple than a single pass.
fn smooth(values: &[f64], window: f64) -> Vec<f64> {
    moving_average(&moving_average(values, window), window)
}

/// Centred moving average. Near the ends the window shrinks symmetrically, so a constant
/// gradient is preserved exactly and the end points keep their elevation.
fn moving_average(values: &[f64], window: f64) -> Vec<f64> {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // small positive count
    let half = ((window / SPACING / 2.0).round() as usize).max(1);
    let last = values.len().saturating_sub(1);
    (0..values.len())
        .map(|i| {
            let h = half.min(i).min(last - i);
            let range = &values[i - h..=i + h];
            #[allow(clippy::cast_precision_loss)]
            let count = range.len() as f64;
            range.iter().sum::<f64>() / count
        })
        .collect()
}

/// Great-circle distance in metres.
fn haversine(a: &RawPoint, b: &RawPoint) -> f64 {
    let (lat1, lat2) = (a.lat.to_radians(), b.lat.to_radians());
    let dlat = lat2 - lat1;
    let dlon = (b.lon - a.lon).to_radians();
    let h = (dlat / 2.0).sin().powi(2) + lat1.cos() * lat2.cos() * (dlon / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS * h.sqrt().asin()
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use super::*;

    #[tokio::test]
    async fn snapping_to_the_map_keeps_the_routes_key() {
        // GPS wandering a few metres east of a road due north.
        let mut xml = String::from("<gpx><trk><trkseg>");
        for i in 0..=60 {
            let lat = 46.0 + f64::from(i) * 10.0 / 111_195.0;
            let lon = 7.0 + f64::from(i % 3) * 4.0 / 77_000.0;
            let _ = write!(
                xml,
                r#"<trkpt lat="{lat}" lon="{lon}"><ele>500</ele></trkpt>"#
            );
        }
        xml.push_str("</trkseg></trk></gpx>");
        let road = torqa_osm::Road {
            class: torqa_osm::RoadClass::Street,
            line: vec![(45.999, 7.0), (46.01, 7.0)],
            structure: None,
        };
        let map = MapData {
            roads: vec![road],
            ..MapData::default()
        };

        let plain = Route::from_gpx(&xml, None).await.unwrap();
        let snapped = Route::from_gpx_with::<Terrain>(&xml, None, &map)
            .await
            .unwrap();

        // On the road the zig-zag is gone, so the route is shorter; it is the same route.
        assert!(snapped.length().0 < plain.length().0 - 1.0);
        assert!(snapped.points().iter().all(|p| (p.lon - 7.0).abs() < 1e-6));
        assert_eq!(snapped.key(), plain.key());
    }

    /// The road the dense track rides north along 7°E, with `structure` on it (approach roads
    /// either side), and maybe other roads.
    fn map_with(structure: torqa_osm::Structure, others: Vec<torqa_osm::Road>) -> MapData {
        let road = |line: Vec<(f64, f64)>, structure| torqa_osm::Road {
            class: torqa_osm::RoadClass::Street,
            line,
            structure,
        };
        let (start, end) = (structure.line[0], structure.line[structure.line.len() - 1]);
        let mut roads = vec![
            road(vec![(45.999, 7.0), start], None),
            road(structure.line, Some(structure.kind)),
            road(vec![end, (46.011, 7.0)], None),
        ];
        roads.extend(others);
        MapData {
            roads,
            ..MapData::default()
        }
    }

    /// A straight track due north with the given elevations, `step` metres apart.
    fn gpx_north(elevations: &[Option<f64>], step: f64) -> String {
        let degrees_per_meter = 1.0 / (EARTH_RADIUS.to_radians());
        let mut points = String::new();
        for (i, elevation) in elevations.iter().enumerate() {
            #[allow(clippy::cast_precision_loss)]
            let lat = 46.0 + i as f64 * step * degrees_per_meter;
            let ele = elevation
                .map(|e| format!("<ele>{e}</ele>"))
                .unwrap_or_default();
            let _ = write!(points, r#"<trkpt lat="{lat}" lon="7.0">{ele}</trkpt>"#);
        }
        format!("<gpx><trk><trkseg>{points}</trkseg></trk></gpx>")
    }

    async fn import(xml: &str) -> Route {
        Route::from_gpx(xml, None).await.unwrap()
    }

    #[tokio::test]
    async fn routes_know_their_climbs() {
        // 1 km flat, 2 km at 5 %, 1 km flat.
        let elevations: Vec<_> = (0..=40)
            .map(|i| Some(f64::from(i.clamp(10, 30) - 10) * 5.0))
            .collect();
        let route = import(&gpx_north(&elevations, 100.0)).await;

        let climbs = route.climbs();
        assert_eq!(climbs.len(), 1, "{climbs:?}");
        assert!((climbs[0].gain.0 - 100.0).abs() < 5.0, "{climbs:?}");
        // Smoothing the file's elevations rounds both ends of the climb by up to a window.
        assert!(
            (climbs[0].length().0 - 2000.0).abs() <= GPX_SMOOTHING * 2.0 + 1.0,
            "{climbs:?}"
        );
    }

    #[tokio::test]
    async fn the_same_track_has_the_same_key() {
        let track = gpx_north(&[Some(0.0); 11], 100.0);
        let renamed = track.replace("<trk>", "<trk><name>Other name</name>");
        let other = gpx_north(&[Some(0.0); 12], 100.0);

        let key = import(&track).await.key();

        assert_eq!(key, import(&renamed).await.key());
        assert_ne!(key, import(&other).await.key());
    }

    #[tokio::test]
    async fn measures_length_along_the_track() {
        let route = import(&gpx_north(&[Some(0.0); 11], 100.0)).await;

        assert!(
            (route.length().0 - 1000.0).abs() < 0.5,
            "{:?}",
            route.length()
        );
        assert_eq!(route.elevation_source(), ElevationSource::File);
    }

    #[tokio::test]
    async fn ascent_ahead_counts_the_rises_still_to_come() {
        // Up 50 m over 500 m, down 20 m over 200 m, up 30 m over 300 m.
        let mut xml = String::from("<gpx><trk><trkseg>");
        for i in 0..=100 {
            let metres = f64::from(i) * 10.0;
            let elevation = if metres <= 500.0 {
                500.0 + metres / 10.0
            } else if metres <= 700.0 {
                550.0 - (metres - 500.0) / 10.0
            } else {
                530.0 + (metres - 700.0) / 10.0
            };
            let lat = 46.0 + metres / 111_195.0;
            let _ = write!(
                xml,
                r#"<trkpt lat="{lat}" lon="7.0"><ele>{elevation}</ele></trkpt>"#
            );
        }
        xml.push_str("</trkseg></trk></gpx>");
        let route = Route::from_gpx(&xml, None).await.unwrap();

        // Smoothing rounds the top and the dip off, so a little under 80 m in all.
        let total = route.elevation_gain().0;
        assert!((65.0..80.0).contains(&total), "gain {total}");
        // At the start all of it; halfway up the first climb, the rest of it and the last
        // climb; in the dip, the last climb only; at the end nothing. Never more than before.
        let ahead = |metres: f64| route.ascent_ahead(Meters(metres)).0;
        assert!((ahead(0.0) - total).abs() < 0.5, "{}", ahead(0.0));
        assert!(
            (0.6 * total..0.75 * total).contains(&ahead(250.0)),
            "{}",
            ahead(250.0)
        );
        assert!(
            (0.3 * total..0.45 * total).contains(&ahead(650.0)),
            "{}",
            ahead(650.0)
        );
        assert!(
            ahead(route.length().0) < 0.01,
            "{}",
            ahead(route.length().0)
        );
        let along: Vec<f64> = (0..=20).map(|k| ahead(f64::from(k) * 50.0)).collect();
        assert!(along.windows(2).all(|w| w[1] <= w[0] + 1e-9), "{along:?}");
    }

    #[tokio::test]
    async fn the_reference_route_reads_as_its_planner_wrote_it() {
        // #167: the Oberalp fixture by the planner's own elevations, without tiles.
        let xml = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../fixtures/oberalp.gpx"
        ))
        .unwrap();
        let route = Route::from_gpx(&xml, None).await.unwrap();

        assert_eq!(route.name(), Some("Oberalp"));
        assert!(
            (33_400.0..33_900.0).contains(&route.length().0),
            "{}",
            route.length().0
        );
        // Andermatt 1435 m to the pass at 2044 m and down to Disentis at 1130 m.
        let gain = route.elevation_gain().0;
        assert!((580.0..760.0).contains(&gain), "gain {gain}");
        assert_eq!(route.elevation_source(), ElevationSource::File);
        // Without the map the planner's chords through the tunnels stay, and the grades
        // along them say nothing: `the_reference_route_follows_the_map_at_road_grades` in
        // torqa-app checks those against the cached tiles.
    }

    #[tokio::test]
    async fn reports_grade_of_a_steady_climb() {
        // 5 % for 2 km: smoothing must not change a constant gradient.
        let elevations: Vec<_> = (0..=20).map(|i| Some(f64::from(i) * 5.0)).collect();
        let route = import(&gpx_north(&elevations, 100.0)).await;

        let middle = route.position(Meters(1000.0));
        assert!((middle.grade.0 - 5.0).abs() < 0.05, "{:?}", middle.grade);
        assert!(
            (middle.elevation.0 - 50.0).abs() < 0.5,
            "{:?}",
            middle.elevation
        );
        assert!((route.elevation_gain().0 - 100.0).abs() < 2.0);
    }

    #[tokio::test]
    async fn turns_in_place_are_taken_out_for_riding_in_3d() {
        // North, 40 m into a side road and straight back out; on north, up 300 m, back down and
        // up again; on, 30 m back and forth on the spot (#101); then 400 m up a dead end to a
        // summit and back, and on north.
        let track = gpx_through(&[
            (0.0, 0.0),
            (0.0, 500.0),
            (40.0, 500.0),
            (0.0, 500.0),
            (0.0, 1300.0),
            (0.0, 1000.0),
            (0.0, 1800.0),
            (0.0, 1770.0),
            (0.0, 2000.0),
            (400.0, 2000.0),
            (0.0, 2000.0),
            (0.0, 2400.0),
        ]);
        let recorded = import(&track).await;
        let route = recorded.clone().without_turns_in_place();

        // The side road and the steps back and forth are gone: 80 m and 60 m shorter...
        assert!((recorded.length().0 - 3940.0).abs() < 5.0);
        assert!(
            (route.length().0 - 3800.0).abs() < 10.0,
            "{} m long",
            route.length().0
        );
        let metres = |p: &RoutePoint| {
            let scale = EARTH_RADIUS.to_radians();
            (
                (p.lon - 7.0) * scale * 46f64.to_radians().cos(),
                (p.lat - 46.0) * scale,
            )
        };
        assert!(
            route
                .points()
                .iter()
                .map(metres)
                .all(|(east, north)| east < 5.0 || north > 1900.0)
        );
        // ...the long way down and up again and the summit stay (planned so)...
        assert!(
            route
                .points()
                .iter()
                .map(metres)
                .any(|(east, _)| east > 390.0)
        );
        // ...and places on it are where they were on the track: 1500 m on, the track had come
        // 80 m further, 3000 m on 140 m.
        for (along, on_track) in [(1500.0, 1580.0), (3000.0, 3140.0)] {
            let found = route.recorded_distance(Meters(along)).0;
            assert!((found - on_track).abs() < 10.0, "{along} m: {found} m");
        }
        assert_eq!(route.recorded_distance(Meters(400.0)).0, 400.0);
    }

    #[tokio::test]
    async fn the_same_road_ridden_twice_lies_at_one_height() {
        // 600 m north at 2 % and back the same way, the file's heights 6 m higher on the way
        // back (#101).
        let degrees_per_meter = 1.0 / EARTH_RADIUS.to_radians();
        let mut points = String::new();
        for i in 0..=120 {
            let k = if i <= 60 { i } else { 120 - i };
            let lat = 46.0 + f64::from(k) * 10.0 * degrees_per_meter;
            let ele = 500.0 + f64::from(k) * 0.2 + if i > 60 { 6.0 } else { 0.0 };
            let _ = write!(
                points,
                r#"<trkpt lat="{lat}" lon="7.0"><ele>{ele}</ele></trkpt>"#
            );
        }
        let route = import(&format!("<gpx><trk><trkseg>{points}</trkseg></trk></gpx>")).await;

        // Wherever the way back passes, it lies at the height of the way out...
        for k in 1..=11 {
            let at = f64::from(k) * 50.0;
            let out = route.position(Meters(at)).elevation.0;
            let back = route.position(Meters(1200.0 - at)).elevation.0;
            assert!(
                (out - back).abs() < 0.05,
                "{at} m out at {out}, back at {back}"
            );
        }
        // ...with no step anywhere.
        for k in 0..240 {
            let grade = route.position(Meters(f64::from(k) * 5.0)).grade.0;
            assert!(grade.abs() < 5.0, "{grade} % at {} m", k * 5);
        }
    }

    #[tokio::test]
    async fn smoothing_removes_elevation_noise() {
        // Flat road with ±3 m GPS noise every 10 m would read as ±60 % spikes unsmoothed.
        let elevations: Vec<_> = (0..200)
            .map(|i| Some(if i % 2 == 0 { 503.0 } else { 497.0 }))
            .collect();
        let route = import(&gpx_north(&elevations, 10.0)).await;

        let max_grade = (0..19)
            .map(|k| {
                route
                    .position(Meters(100.0 + f64::from(k) * 90.0))
                    .grade
                    .0
                    .abs()
            })
            .fold(0.0, f64::max);
        assert!(max_grade < 1.0, "max grade {max_grade}");
    }

    #[tokio::test]
    async fn interpolates_missing_elevations() {
        let route = import(&gpx_north(&[Some(100.0), None, None, Some(130.0)], 100.0)).await;

        assert!((route.position(Meters(150.0)).elevation.0 - 115.0).abs() < 1.0);
    }

    #[tokio::test]
    async fn position_is_clamped_to_the_route() {
        let route = import(&gpx_north(&[Some(10.0), Some(20.0)], 100.0)).await;

        assert_eq!(route.position(Meters(-5.0)).lat, route.points()[0].lat);
        let end = route.position(Meters(1e6));
        assert!((end.lat - route.points().last().unwrap().lat).abs() < 1e-12);
    }

    #[tokio::test]
    async fn needs_elevation_and_two_points() {
        assert!(matches!(
            Route::from_gpx(&gpx_north(&[None, None], 100.0), None).await,
            Err(RouteError::NoElevation)
        ));
        assert!(matches!(
            Route::from_gpx(&gpx_north(&[Some(1.0)], 100.0), None).await,
            Err(RouteError::TooShort)
        ));
    }

    /// Terrain with a 200 m ridge between latitudes 46.000 and 46.009, everywhere else 500 m.
    struct Ridge;

    impl ElevationModel for Ridge {
        fn elevation(
            &mut self,
            lat: f64,
            _lon: f64,
        ) -> impl std::future::Future<Output = Result<f64, String>> + Send {
            let t = ((lat - 46.0) / 0.009).clamp(0.0, 1.0);
            std::future::ready(Ok(500.0 + 200.0 * (t * std::f64::consts::PI).sin()))
        }
    }

    /// Flat land at 500 m with a 100 m spike over 200 m of it, from 400 m to 600 m north, as a
    /// terrain cell straddling a cliff above a road on a ledge gives (#172).
    struct Spike;

    impl ElevationModel for Spike {
        fn elevation(
            &mut self,
            lat: f64,
            _lon: f64,
        ) -> impl std::future::Future<Output = Result<f64, String>> + Send {
            let north = (lat - 46.0) * EARTH_RADIUS.to_radians();
            let bump = (1.0 - (north - 500.0).abs() / 100.0).max(0.0);
            std::future::ready(Ok(500.0 + 100.0 * bump))
        }
    }

    /// A real hill: 80 m up over 1 km and down again, 8 % either side.
    struct Hill;

    impl ElevationModel for Hill {
        fn elevation(
            &mut self,
            lat: f64,
            _lon: f64,
        ) -> impl std::future::Future<Output = Result<f64, String>> + Send {
            let north = (lat - 46.0) * EARTH_RADIUS.to_radians();
            let rise = (1.0 - (north - 1000.0).abs() / 1000.0).max(0.0);
            std::future::ready(Ok(500.0 + 80.0 * rise))
        }
    }

    #[test]
    fn cut_spikes_leaves_a_tent_at_the_road_grade() {
        // Flat at 500 m, a spike 100 m high over ten samples either side (100 % grades).
        let mut values: Vec<f64> = (0..41)
            .map(|i| 500.0 + 100.0 * (1.0 - f64::from((i - 20i32).abs()) / 10.0).max(0.0))
            .collect();
        cut_spikes(&mut values);
        let top = values.iter().copied().fold(f64::MIN, f64::max);
        assert!((top - 500.0).abs() < 0.01, "top {top}: {values:?}");
        // A climb of 50 % going on is kept.
        let mut climb: Vec<f64> = (0..41).map(|i| 500.0 + 5.0 * f64::from(i)).collect();
        let before = climb.clone();
        cut_spikes(&mut climb);
        assert_eq!(climb, before);
    }

    #[tokio::test]
    async fn a_spike_in_the_terrain_is_cut_to_a_road_grade() {
        // No elevations in the file: the spike's shape alone gives it away.
        let elevations: Vec<Option<f64>> = vec![None; 101];
        let route = Route::from_gpx_with(
            &gpx_north(&elevations, 10.0),
            Some(&mut Spike),
            &MapData::default(),
        )
        .await
        .unwrap();

        let top = route
            .points()
            .iter()
            .map(|p| p.elevation.0)
            .fold(f64::MIN, f64::max);
        assert!(top < 505.0, "the spike still stands {top} m high");
        assert!(
            route.max_grade().0 < 5.0,
            "steepest {} %",
            route.max_grade().0
        );
        assert!(
            route.elevation_gain().0 < 5.0,
            "{}",
            route.elevation_gain().0
        );
    }

    #[tokio::test]
    async fn the_file_corrects_the_model_where_it_jumps() {
        // The planner's file has the road flat, 40 m under the model's level: an offset, not a
        // spike. Where the model jumps, the file wins, offset and all.
        let elevations: Vec<Option<f64>> = vec![Some(460.0); 101];
        let route = Route::from_gpx_with(
            &gpx_north(&elevations, 10.0),
            Some(&mut Spike),
            &MapData::default(),
        )
        .await
        .unwrap();

        for point in route.points() {
            assert!((point.elevation.0 - 500.0).abs() < 6.0, "{point:?}");
        }
        assert!(
            route.elevation_gain().0 < 6.0,
            "{}",
            route.elevation_gain().0
        );
    }

    #[tokio::test]
    async fn a_real_hill_is_kept_whole() {
        let elevations: Vec<Option<f64>> = vec![None; 201];
        let route = Route::from_gpx_with(
            &gpx_north(&elevations, 10.0),
            Some(&mut Hill),
            &MapData::default(),
        )
        .await
        .unwrap();

        let gain = route.elevation_gain().0;
        assert!((76.0..81.0).contains(&gain), "gain {gain}");
        let steepest = route.max_grade().0;
        assert!((7.0..8.5).contains(&steepest), "steepest {steepest}");
    }

    #[tokio::test]
    async fn sparse_points_do_not_cut_across_the_terrain() {
        // Two points 1 km apart on a road that goes around the ridge, not over it.
        let xml = r#"<gpx><trk><trkseg>
            <trkpt lat="46.0" lon="7.0"/><trkpt lat="46.009" lon="7.0"/>
        </trkseg></trk></gpx>"#;

        let route = Route::from_gpx_with(xml, Some(&mut Ridge), &MapData::default())
            .await
            .unwrap();

        assert_eq!(route.elevation_source(), ElevationSource::Terrain);
        assert!(route.max_grade().0 < 0.5, "{:?}", route.max_grade());
        assert!(route.elevation_gain().0 < 1.0);
    }

    #[tokio::test]
    async fn reports_the_steepest_gradient() {
        let elevations: Vec<_> = [0.0, 0.0, 3.0, 13.0, 13.0, 13.0, 13.0]
            .into_iter()
            .map(Some)
            .collect();
        let route = import(&gpx_north(&elevations, 100.0)).await;

        // 10 m over 100 m, softened a little by smoothing.
        let max = route.max_grade().0;
        assert!((7.0..10.5).contains(&max), "{max}");
    }

    #[tokio::test]
    async fn heading_points_along_the_road() {
        let north = import(&gpx_north(&[Some(0.0), Some(0.0)], 100.0)).await;

        assert!(north.position(Meters(50.0)).heading.abs() < 1e-6);
    }

    /// A track through points given in metres east and north of 46° N 7° E.
    fn gpx_through(points: &[(f64, f64)]) -> String {
        let degrees_per_meter = 1.0 / (EARTH_RADIUS.to_radians());
        let mut xml = String::new();
        for &(east, north) in points {
            let lat = 46.0 + north * degrees_per_meter;
            let lon = 7.0 + east * degrees_per_meter / 46f64.to_radians().cos();
            let _ = write!(
                xml,
                r#"<trkpt lat="{lat}" lon="{lon}"><ele>500</ele></trkpt>"#
            );
        }
        format!("<gpx><trk><trkseg>{xml}</trkseg></trk></gpx>")
    }

    /// Half a circle of `radius` metres from west of the centre over the north to the east
    /// (a bend to the right), or the other way round.
    fn half_circle(radius: f64, to_the_right: bool) -> Vec<(f64, f64)> {
        (0..=90)
            .map(|k| {
                let angle = PI * f64::from(k) / 90.0;
                let east = -radius * angle.cos();
                (
                    if to_the_right { east } else { -east },
                    radius * angle.sin(),
                )
            })
            .collect()
    }

    #[tokio::test]
    async fn the_heading_turns_smoothly_through_a_corner() {
        // 200 m north, then a right-angled corner and 200 m east.
        let mut corner: Vec<(f64, f64)> = (0..=20).map(|i| (0.0, f64::from(i) * 10.0)).collect();
        corner.extend((1..=20).map(|i| (f64::from(i) * 10.0, 200.0)));
        let route = import(&gpx_through(&corner)).await;

        // From well before the corner to well after it (import smooths tracks over 100 m).
        let headings: Vec<f64> = (0..=1200)
            .map(|step| {
                route
                    .position(Meters(50.0 + f64::from(step) * 0.25))
                    .heading
            })
            .collect();
        let biggest_step = headings
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f64::max);
        // On the polygon the heading would jump by 90° at the corner; on the curve it turns
        // over metres (no tighter than a 2.5 m radius: 0.1 rad per 0.25 m).
        assert!(biggest_step < 0.1, "heading jumps by {biggest_step} rad");
        assert!(headings[0].abs() < 1e-3, "north before: {}", headings[0]);
        let after = headings[1200];
        assert!(
            (after - std::f64::consts::FRAC_PI_2).abs() < 1e-3,
            "east after: {after}"
        );
    }

    #[tokio::test]
    async fn curvature_is_one_over_the_radius_and_signed_by_the_side() {
        let right = import(&gpx_through(&half_circle(50.0, true))).await;
        let left = import(&gpx_through(&half_circle(50.0, false))).await;
        let straight = import(&gpx_north(&[Some(0.0), Some(0.0)], 100.0)).await;
        let middle = Meters(right.length().0 / 2.0);

        let bend = right.position(middle).curvature;
        assert!((bend - 1.0 / 50.0).abs() < 0.002, "right bend: {bend}");
        let bend = left.position(middle).curvature;
        assert!((bend + 1.0 / 50.0).abs() < 0.002, "left bend: {bend}");
        assert!(straight.position(Meters(50.0)).curvature.abs() < 1e-6);
    }

    /// A valley 60 m deep in the middle of a 1 km route due north at 500 m.
    struct Valley;

    impl ElevationModel for Valley {
        fn elevation(
            &mut self,
            lat: f64,
            _lon: f64,
        ) -> impl std::future::Future<Output = Result<f64, String>> + Send {
            let north = (lat - 46.0) * 111_195.0;
            let depth = if (300.0..700.0).contains(&north) {
                60.0
            } else {
                0.0
            };
            std::future::ready(Ok(500.0 - depth))
        }
    }

    /// A densely recorded track due north (a point every 10 m), as from a bike computer.
    fn dense_track_north() -> String {
        gpx_north(&[None; 101], 10.0)
    }

    fn line_north(from_m: f64, to_m: f64, lon: f64) -> Vec<(f64, f64)> {
        vec![
            (46.0 + from_m / 111_195.0, lon),
            (46.0 + to_m / 111_195.0, lon),
        ]
    }

    #[tokio::test]
    async fn bridges_carry_the_road_straight_across_valleys() {
        let bridge = torqa_osm::Structure {
            kind: torqa_osm::StructureKind::Bridge,
            line: line_north(280.0, 720.0, 7.0),
        };

        let route = Route::from_gpx_with(
            &dense_track_north(),
            Some(&mut Valley),
            &map_with(bridge, Vec::new()),
        )
        .await
        .unwrap();

        assert!(route.max_grade().0 < 1.0, "{:?}", route.max_grade());
        assert_eq!(route.position(Meters(500.0)).elevation, Meters(500.0));
        let middle = route
            .points()
            .iter()
            .find(|p| p.distance.0 >= 500.0)
            .unwrap();
        assert_eq!(middle.surface, Surface::Bridge);
        assert_eq!(route.points()[0].surface, Surface::Ground);
    }

    #[tokio::test]
    async fn without_the_bridge_the_route_dips_into_the_valley() {
        let route =
            Route::from_gpx_with(&dense_track_north(), Some(&mut Valley), &MapData::default())
                .await
                .unwrap();

        assert!(route.position(Meters(500.0)).elevation.0 < 450.0);
    }

    #[tokio::test]
    async fn roads_crossing_above_are_not_the_route() {
        // The route's own road is plain; a bridge runs east-west over it.
        let plain = torqa_osm::Structure {
            kind: torqa_osm::StructureKind::Bridge,
            line: line_north(280.0, 720.0, 7.0),
        };
        let crossing = torqa_osm::Road {
            class: torqa_osm::RoadClass::Major,
            line: vec![
                (46.0 + 500.0 / 111_195.0, 6.999),
                (46.0 + 500.0 / 111_195.0, 7.001),
            ],
            structure: Some(torqa_osm::StructureKind::Bridge),
        };
        let mut map = map_with(plain, vec![crossing]);
        map.roads[1].structure = None;

        let route = Route::from_gpx_with(&dense_track_north(), Some(&mut Valley), &map)
            .await
            .unwrap();

        assert!(route.points().iter().all(|p| p.surface == Surface::Ground));
    }

    #[tokio::test]
    async fn tunnels_and_bridges_the_terrain_does_not_bear_out_stay_on_the_ground() {
        // The map says tunnel where the route crosses the valley: there is no hill above it.
        let tunnel = torqa_osm::Structure {
            kind: torqa_osm::StructureKind::Tunnel,
            line: line_north(280.0, 720.0, 7.0),
        };

        let route = Route::from_gpx_with(
            &dense_track_north(),
            Some(&mut Valley),
            &map_with(tunnel, Vec::new()),
        )
        .await
        .unwrap();

        assert!(route.points().iter().all(|p| p.surface == Surface::Ground));
    }
}
