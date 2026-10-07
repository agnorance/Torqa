//! The Torqa application layer: devices, routes, the ride engine and storage behind a simple,
//! frame-driven API. Front ends (the Godot app, tests) call commands and [`App::update`] once per
//! frame; all asynchronous work runs on an internal runtime, so callers never block or await.

pub mod hud;
mod import;
pub mod media;
pub mod paths;
pub mod video;
pub mod view;

pub use import::{Imported, LoadStage, Progress, import_gpx, import_route};

use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::{Duration, SystemTime};

use torqa_devices::ble::{Bluetooth, DeviceKind, DiscoveredDevice};
use torqa_devices::fake::{self, FakeRider};
use torqa_devices::{DeviceEvent, DeviceHandle};
use torqa_domain::files::UsedFiles;
use torqa_domain::profile::{Drivetrain, Profile};
use torqa_domain::recording::{RideSummary, Sample};
use torqa_domain::shifting::{Shift, ShiftInput};
use torqa_domain::units::{Meters, Percent, Watts};
use torqa_physics::{DescentMode, RiderSetup, VirtualGears};
use torqa_routes::{Climb, ElevationSource, LocalProjection, Route};
use torqa_session::analysis::{
    effort, summarize, time_at, time_in_heart_rate_zones, time_in_power_zones,
};
use torqa_session::ghost::Ghost;
use torqa_session::workout::Workout;
use torqa_session::{Ride, RideConfig, RideState};
use torqa_storage::course::{self, Manifest};
use torqa_storage::profiles::{self, DeviceRole, StoredProfile};
pub use torqa_storage::profiles::{GraphicsQuality, OverlayWindow};
use torqa_storage::rides::{self, ClimbTime, RideRecord};
use torqa_terrain::{Terrain, TileSource};
pub use torqa_video::Frame;
use torqa_video::Video;
use torqa_world::World;
use tracing::warn;
pub use video::SyncMark;

/// Credits for the data a course bundles, stored in course files (ODbL, CC BY).
const ATTRIBUTION: [&str; 3] = [
    "© OpenFreeMap © OpenMapTiles · Data © OpenStreetMap contributors (ODbL)",
    "Terrain: Mapterhorn (CC BY 4.0)",
    "Terrain: AWS Terrain Tiles",
];

/// Points of the thinned track and profile stored with a course for its card.
const PREVIEW_POINTS: usize = 200;

/// How long the reconnect at start scans for the devices used last.
const RECONNECT_SCAN: Duration = Duration::from_secs(6);

/// How long [`App::shutdown`] waits for devices to disconnect.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(3);

/// Errors from commands that fail immediately.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// The async runtime could not be started.
    #[error("cannot start runtime: {0}")]
    Runtime(#[from] std::io::Error),
    /// A ride needs a loaded route.
    #[error("no route loaded")]
    NoRoute,
    /// A ride needs a trainer.
    #[error("no trainer connected")]
    NoTrainer,
    /// The device index does not refer to a discovered device of the right kind.
    #[error("unknown device")]
    UnknownDevice,
    /// Saving a course needs a route whose world has been built.
    #[error("the course is not ready yet")]
    CourseNotReady,
    /// No profile with that id.
    #[error("unknown profile")]
    UnknownProfile,
    /// Reading or writing a file in the data directory failed.
    #[error("{0}")]
    Storage(String),
    /// The chosen ghost cannot ride this route.
    #[error("{0}")]
    GhostUnavailable(String),
    /// The video of a video course cannot be played.
    #[error("{0}")]
    Video(String),
    /// A workout file cannot be read.
    #[error("{0}")]
    Workout(String),
}

/// Who to race against (R20).
#[derive(Debug, Clone, PartialEq)]
pub enum GhostChoice {
    /// Nobody.
    None,
    /// The rider's fastest earlier ride on this route.
    PersonalBest,
    /// A pacer holding constant power.
    Power(Watts),
    /// A pacer holding constant power per kilogram of the rider's body weight.
    WattsPerKg(f64),
    /// A recorded activity (GPX with times, or FIT) along this route.
    Activity(PathBuf),
}

/// Where the ghost is relative to the rider.
#[derive(Debug, Clone, PartialEq)]
pub struct GhostState {
    /// What the ghost is.
    pub name: String,
    /// Its distance from the route start.
    pub distance: Meters,
    /// Seconds the rider is behind it (negative: ahead); `None` once it is out of reach.
    pub gap: Option<f64>,
}

/// A device found by a scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    /// Index to pass to the connect commands.
    pub index: usize,
    /// Advertised name.
    pub name: String,
    /// Whether it is a trainer or a heart-rate sensor.
    pub kind: DeviceKind,
    /// Signal strength in dBm, if known.
    pub rssi: Option<i16>,
    /// Whether this is the trainer or sensor used last (R41).
    pub remembered: bool,
}

/// Key facts about a loaded route.
#[derive(Debug, Clone, PartialEq)]
pub struct RouteSummary {
    /// Name from the file, or the file name.
    pub name: String,
    /// Length in metres.
    pub length: f64,
    /// Total climbing in metres.
    pub elevation_gain: f64,
    /// Steepest climbing gradient in percent.
    pub max_grade: f64,
    /// Where the elevations come from.
    pub elevation_source: ElevationSource,
}

/// A course in the library.
#[derive(Debug, Clone, PartialEq)]
pub struct CourseEntry {
    /// The course file.
    pub path: PathBuf,
    /// What the course file says about itself.
    pub manifest: Manifest,
}

/// A ride in the history.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryEntry {
    /// The FIT activity file.
    pub fit: PathBuf,
    /// Route and summary.
    pub record: RideRecord,
    /// Whether this is the rider's fastest time over the whole route.
    pub route_record: bool,
    /// Per entry of [`RideRecord::climbs`]: whether it is the rider's fastest time there.
    pub climb_records: Vec<bool>,
}

/// The rider's best times on a route (R27).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RouteRecords {
    /// Fastest time over the whole route.
    pub route: Option<Duration>,
    /// Fastest time per climb, in the order of [`Route::climbs`].
    pub climbs: Vec<Option<Duration>>,
}

/// The climb the rider is on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClimbProgress {
    /// Index in [`Route::climbs`].
    pub index: usize,
    /// The climb.
    pub climb: Climb,
    /// Distance ridden on it so far.
    pub ridden: Meters,
    /// Time on it so far.
    pub elapsed: Duration,
    /// The rider's best time on it.
    pub best: Option<Duration>,
}

/// Everything recorded during one ride, for its analysis.
#[derive(Debug, Clone, PartialEq)]
pub struct RideDetail {
    /// The 1 Hz samples.
    pub samples: Vec<Sample>,
    /// Time in each power zone of the active rider.
    pub power_zones: [Duration; 7],
    /// Time in each heart-rate zone of the active rider.
    pub heart_rate_zones: [Duration; 5],
}

/// Something that happened since the last [`App::update`].
#[derive(Debug, Clone, PartialEq)]
pub enum AppEvent {
    /// A scan finished.
    DevicesFound(Vec<DeviceInfo>),
    /// Preparing a course advanced.
    LoadProgress {
        /// The current step.
        stage: LoadStage,
        /// Units done in this step.
        done: usize,
        /// Units in this step.
        total: usize,
    },
    /// A route was imported; its 3D world is being generated.
    RouteLoaded(RouteSummary),
    /// The 3D world for the loaded route is ready.
    WorldReady {
        /// Number of terrain chunks.
        chunks: usize,
        /// Terrain samples without elevation data (terrain follows the road there).
        fallback_samples: usize,
    },
    /// A device connected (also after a reconnect).
    Connected(String),
    /// Devices used last that a reconnect at start did not find (R41), by name: the rider
    /// should wake them and scan.
    RememberedMissing(Vec<String>),
    /// A device lost its connection; it reconnects automatically.
    Disconnected(String),
    /// The rider reached the finish.
    RideFinished,
    /// The ride was saved as a FIT file.
    RideSaved(PathBuf),
    /// A course was saved or imported into the library.
    CourseAdded(PathBuf),
    /// The rider reached the top of a climb.
    ClimbCompleted {
        /// Index in [`Route::climbs`].
        index: usize,
        /// Time from foot to top.
        elapsed: Duration,
        /// The best time before this ride, if any.
        previous_best: Option<Duration>,
    },
    /// The rider reached the finish.
    RouteCompleted {
        /// Time for the whole route.
        elapsed: Duration,
        /// The best time before this ride, if any.
        previous_best: Option<Duration>,
    },
    /// A background operation failed.
    Error(String),
}

/// Which trainer to connect.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TrainerChoice {
    /// The simulated trainer.
    Fake(FakeRider),
    /// A device from the last scan, by [`DeviceInfo::index`].
    Discovered(usize),
}

enum JobResult {
    Scan(Result<(Bluetooth, Vec<DiscoveredDevice>), String>),
    // Loading results carry the load they belong to; those of an abandoned load are dropped.
    Route(u64, Result<Box<Imported>, String>),
    Video(u64, Result<Box<(Imported, video::VideoSource)>, String>),
    World(u64, Box<World>),
    Progress(u64, LoadStage, usize, usize),
    CourseAdded(Result<PathBuf, String>),
    /// The loaded route was saved as this course.
    CourseSaved(Result<PathBuf, String>),
    Failed(String),
}

/// Forwards load progress to the frame loop, at most once per percent per stage.
struct Reporter {
    tx: mpsc::Sender<JobResult>,
    load: u64,
    last: Option<(LoadStage, usize)>,
}

impl Reporter {
    fn report(&mut self, stage: LoadStage, done: usize, total: usize) {
        let percent = done * 100 / total.max(1);
        if self.last != Some((stage, percent)) {
            self.last = Some((stage, percent));
            let _ = self
                .tx
                .send(JobResult::Progress(self.load, stage, done, total));
        }
    }
}

struct ActiveRide {
    ride: Ride,
    /// The name it goes into the history by, for a workout without a course; rides on a course
    /// are named after it.
    name: Option<String>,
    /// Wall-clock start, set when the trainer first connects.
    started: Option<SystemTime>,
    finished: bool,
    /// Best times before this ride, to compare against.
    records: RouteRecords,
    /// The next climb whose top has not been reached.
    next_climb: usize,
    /// Summary of the samples so far and how many it covers, refreshed once per new sample
    /// rather than every frame.
    summary: (usize, RideSummary),
    ghost: Option<Ghost>,
    /// Plays the video of a video course along the ride.
    player: Option<video::VideoPlayer>,
    /// And its sound (R26), if it has any.
    sound: Option<video::SoundPlayer>,
    /// Whether the video shows: a ride along a blank screen must say why.
    video_watch: VideoWatch,
    /// Ride time per real time, with the fake trainer (#53).
    time_scale: f64,
    /// Sped up or jumped: its times are not real, so it counts towards no records.
    simulated: bool,
}

/// The name the rider gave the course being imported.
#[derive(Debug)]
struct Naming {
    name: String,
    replace: bool,
}

/// How a ride is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    /// In the generated 3D world.
    World,
    /// Along the course's video.
    Video,
}

/// When the 3D world of a loaded route is built.
enum WorldPlan {
    /// Right after the route.
    Now,
    /// Once asked for (riding a course), with the route's map data when it is loaded.
    OnRequest(Option<Box<torqa_osm::MapData>>),
}

/// Frames of the video handed out during a ride, and whether a problem was reported.
#[derive(Debug)]
struct VideoWatch {
    since: std::time::Instant,
    frames: usize,
    reported: bool,
}

/// The longest step a ride advances by at once, also when sped up.
const MAX_RIDE_STEP: Duration = Duration::from_millis(50);
/// How far a simulated ride can be sped up (#53).
const MAX_TIME_SCALE: f64 = 20.0;

/// How long a video course may show no picture before the rider is told.
const NO_PICTURE_AFTER: Duration = Duration::from_secs(5);

/// The application state.
pub struct App {
    runtime: tokio::runtime::Runtime,
    jobs_tx: mpsc::Sender<JobResult>,
    jobs_rx: mpsc::Receiver<JobResult>,
    bluetooth: Option<Bluetooth>,
    discovered: Vec<DiscoveredDevice>,
    trainer: Option<DeviceHandle>,
    sensor: Option<DeviceHandle>,
    /// Identifiers of the connected Bluetooth trainer and sensor, to avoid reconnecting them.
    trainer_id: Option<String>,
    /// When the current trainer last reported itself connected; `None` while it is not. A
    /// ride started later must not wait for a report that has already come.
    trainer_connected: Option<SystemTime>,
    sensor_id: Option<String>,
    /// The running scan was started to reconnect the remembered devices.
    reconnecting: bool,
    /// The loaded route came from a course file (and is in the library already).
    from_course: bool,
    /// Counts loads; results of earlier ones are stale.
    load: u64,
    /// When to build the loaded route's world.
    world_plan: WorldPlan,
    /// The course file of the loaded route, once it is one.
    course: Option<PathBuf>,
    /// The route key of the course being written to the library, until it is there.
    saving: Option<String>,
    /// How the rider named the course being imported.
    naming: Option<Naming>,
    /// The loaded course's video, for video courses (R17).
    video: Option<video::VideoCourse>,
    /// How the next ride is shown.
    view: View,
    /// A video open for preview frames, e.g. while aligning it to a route.
    preview: Option<(PathBuf, Video)>,
    route: Option<Route>,
    world: Option<Arc<World>>,
    offline: bool,
    /// Name and GPX of the loaded route, for saving it as a course.
    loaded: Option<(String, String)>,
    /// Cached files the loaded route and its world were built from.
    used: UsedFiles,
    ride: Option<ActiveRide>,
    /// Inputs that shift the virtual gears (R7): the keyboard, …
    shift_inputs: Vec<Box<dyn ShiftInput>>,
    /// Whether each of them was connected at the last update, to report changes.
    shift_inputs_connected: Vec<bool>,
    /// The keyboard's keys, pressed through [`App::shift`].
    keys: torqa_devices::shift::KeyboardKeys,
    /// Identifier of the connected controller, to avoid reconnecting it.
    controller_id: Option<String>,
    /// Which D-Fly channels of a Di2 shifter shift up and down.
    shift_channels: Arc<torqa_devices::shift::Channels>,
    profile: StoredProfile,
    data_dir: PathBuf,
    cache_dir: PathBuf,
}

impl App {
    /// Creates the application with rides saved under `data_dir` and downloads cached under
    /// `cache_dir` (see [`paths`] for platform defaults).
    ///
    /// # Errors
    /// [`AppError::Runtime`] if the async runtime cannot start.
    pub fn new(data_dir: PathBuf, cache_dir: PathBuf) -> Result<Self, AppError> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?;
        let (jobs_tx, jobs_rx) = mpsc::channel();
        let (keys, keyboard) = torqa_devices::shift::keyboard();
        Ok(Self {
            runtime,
            jobs_tx,
            jobs_rx,
            bluetooth: None,
            discovered: Vec::new(),
            trainer: None,
            sensor: None,
            trainer_id: None,
            trainer_connected: None,
            sensor_id: None,
            reconnecting: false,
            from_course: false,
            load: 0,
            world_plan: WorldPlan::Now,
            course: None,
            saving: None,
            naming: None,
            video: None,
            view: View::World,
            preview: None,
            route: None,
            world: None,
            offline: false,
            loaded: None,
            used: UsedFiles::default(),
            ride: None,
            shift_inputs: vec![Box::new(keyboard)],
            shift_inputs_connected: vec![true],
            keys,
            controller_id: None,
            shift_channels: Arc::new({
                let (up, down) = profiles::shift_channels(&data_dir);
                torqa_devices::shift::Channels::new(up, down)
            }),
            profile: initial_profile(&data_dir),
            data_dir,
            cache_dir,
        })
    }

    /// All rider profiles, by name.
    #[must_use]
    pub fn profiles(&self) -> Vec<StoredProfile> {
        let listed = profiles::list(&self.data_dir);
        if listed.is_empty() {
            // Not saved yet, e.g. a read-only data directory: still offer the rider in use.
            vec![self.profile.clone()]
        } else {
            listed
        }
    }

    /// The rider riding now.
    #[must_use]
    pub fn profile(&self) -> &StoredProfile {
        &self.profile
    }

    /// Switches to another rider, remembered for the next start.
    ///
    /// # Errors
    /// [`AppError::UnknownProfile`] if there is no such profile.
    pub fn select_profile(&mut self, id: &str) -> Result<(), AppError> {
        let profile = profiles::load(&self.data_dir, id).map_err(|_| AppError::UnknownProfile)?;
        self.profile = StoredProfile {
            id: id.to_owned(),
            profile,
        };
        if let Err(error) = profiles::set_active(&self.data_dir, id) {
            warn!(%error, "cannot remember the active profile");
        }
        Ok(())
    }

    /// Saves a profile (a new one if `id` is `None`) and makes it the active one; returns its id.
    ///
    /// # Errors
    /// [`AppError::Storage`] if it cannot be written.
    pub fn save_profile(&mut self, id: Option<&str>, profile: Profile) -> Result<String, AppError> {
        let id = id.map_or_else(
            || profiles::new_id(&self.data_dir, &profile.name),
            ToOwned::to_owned,
        );
        profiles::save(&self.data_dir, &id, &profile)
            .map_err(|e| AppError::Storage(format!("cannot save profile: {e}")))?;
        if let Err(error) = profiles::set_active(&self.data_dir, &id) {
            warn!(%error, "cannot remember the active profile");
        }
        self.profile = StoredProfile {
            id: id.clone(),
            profile,
        };
        Ok(id)
    }

    /// Scans for trainers and heart-rate sensors; reports [`AppEvent::DevicesFound`].
    pub fn scan(&mut self, duration: Duration) {
        let tx = self.jobs_tx.clone();
        let existing = self.bluetooth.clone();
        self.runtime.spawn(async move {
            let result = async {
                let bluetooth = match existing {
                    Some(bluetooth) => bluetooth,
                    None => Bluetooth::new().await?,
                };
                let devices = bluetooth.scan(duration).await?;
                Ok((bluetooth, devices))
            }
            .await
            .map_err(|e: torqa_devices::DeviceError| e.to_string());
            let _ = tx.send(JobResult::Scan(result));
        });
    }

    /// Imports a GPX route with terrain-corrected elevations; reports [`AppEvent::RouteLoaded`].
    pub fn load_route(&mut self, path: PathBuf, offline: bool) {
        let used = self.start_loading(offline);
        let load = self.load;
        let tx = self.jobs_tx.clone();
        let cache = self.cache_dir.clone();
        self.runtime.spawn(async move {
            let mut reporter = Reporter {
                tx: tx.clone(),
                load,
                last: None,
            };
            let result = import_route(&path, &cache, offline, &used, &mut |stage, done, total| {
                reporter.report(stage, done, total);
            })
            .await
            .map(Box::new);
            let _ = tx.send(JobResult::Route(load, result));
        });
    }

    /// Opens a course file: its data goes back into the cache and its route is loaded offline;
    /// reports [`AppEvent::RouteLoaded`] like [`App::load_route`]. Its 3D world is built only
    /// when asked for with [`App::build_world`].
    pub fn open_course(&mut self, path: PathBuf) {
        let used = self.start_loading(true);
        let load = self.load;
        self.naming = None;
        self.from_course = true;
        self.world_plan = WorldPlan::OnRequest(None);
        self.course = Some(path.clone());
        let tx = self.jobs_tx.clone();
        let cache = self.cache_dir.clone();
        self.runtime.spawn(async move {
            let mut reporter = Reporter {
                tx: tx.clone(),
                load,
                last: None,
            };
            reporter.report(LoadStage::Route, 0, 1);
            let unpack_from = path.clone();
            let unpack_to = cache.clone();
            let unpacked =
                tokio::task::spawn_blocking(move || course::unpack(&unpack_from, &unpack_to))
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(|r| r.map_err(|e| format!("cannot open {}: {e}", path.display())));
            let unpacked = match unpacked {
                Ok(unpacked) => unpacked,
                Err(message) => {
                    let _ = tx.send(JobResult::Route(load, Err(message)));
                    return;
                }
            };
            // A video course needs its video, referenced rather than stored (R35).
            let mut video = None;
            if let Some(reference) = &unpacked.manifest.video {
                let Some(file) = reference.locate(&path) else {
                    let message = format!(
                        "video {} not found — put it next to the course file",
                        reference.file_name
                    );
                    let _ = tx.send(JobResult::Route(load, Err(message)));
                    return;
                };
                video = Some(video::VideoSource {
                    video: file,
                    name: unpacked.manifest.name.clone(),
                    gpx: unpacked.gpx.clone(),
                    offset: Duration::from_secs_f64(reference.offset_s.max(0.0)),
                    marks: video_marks(reference),
                    located: reference.located,
                });
            }
            let imported = import_gpx(
                unpacked.gpx,
                &unpacked.manifest.name,
                (&cache, true),
                video.is_some(),
                &used,
                &mut |stage, done, total| reporter.report(stage, done, total),
            )
            .await
            .map(|mut imported| {
                imported.name = unpacked.manifest.name;
                imported
            });
            let _ = tx.send(match (imported, video) {
                (Ok(imported), Some(source)) => {
                    JobResult::Video(load, Ok(Box::new((imported, source))))
                }
                (Ok(imported), None) => JobResult::Route(load, Ok(Box::new(imported))),
                (Err(message), _) => JobResult::Route(load, Err(message)),
            });
        });
    }

    /// Prepares a video course (R17) from a GoPro video with GPS or an Incyclist route video
    /// (its `.xml` control file): the route as usual, without a 3D world, paired with the
    /// video; reports [`AppEvent::RouteLoaded`] and [`AppEvent::WorldReady`] like
    /// [`App::load_route`], and adds it to the library. `offline` as for [`App::load_route`].
    pub fn load_video(&mut self, path: PathBuf, offline: bool) {
        self.load_video_from(offline, move || video::source(&path));
    }

    fn load_video_from(
        &mut self,
        offline: bool,
        source: impl FnOnce() -> Result<video::VideoSource, String> + Send + 'static,
    ) {
        let used = self.start_loading(offline);
        let load = self.load;
        let tx = self.jobs_tx.clone();
        let cache = self.cache_dir.clone();
        self.runtime.spawn(async move {
            let mut reporter = Reporter {
                tx: tx.clone(),
                load,
                last: None,
            };
            reporter.report(LoadStage::Route, 0, 1);
            let source = tokio::task::spawn_blocking(source)
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r);
            let result = match source {
                Ok(source) => import_gpx(
                    source.gpx.clone(),
                    &source.name,
                    // A course without a place keeps its own elevations: terrain data from
                    // wherever its drawn line lies would only spoil them.
                    (&cache, offline || !source.located),
                    true,
                    &used,
                    &mut |stage, done, total| reporter.report(stage, done, total),
                )
                .await
                .map(|imported| Box::new((imported, source))),
                Err(message) => Err(message),
            };
            let _ = tx.send(JobResult::Video(load, result));
        });
    }

    /// The loaded video course's video, if it is one.
    #[must_use]
    pub fn video(&self) -> Option<&video::VideoCourse> {
        self.video.as_ref()
    }

    /// Adds a video to the loaded course (a GPX course from the library), placed on its route
    /// by hand: `marks` say where route positions sit in the video, from the route's start to
    /// its end; the video follows the rider's distance evenly between neighbouring marks. The course becomes a video course; its file refers
    /// to the video.
    ///
    /// # Errors
    /// [`AppError::Video`] if no course is loaded, it has a video already, or the video does
    /// not fit the marks; [`AppError::Storage`] if the course file cannot be updated.
    pub fn add_video(&mut self, video: &Path, marks: &[SyncMark]) -> Result<(), AppError> {
        let (Some(route), Some(path), Some((name, gpx))) =
            (&self.route, &self.course, &self.loaded)
        else {
            return Err(AppError::Video("open a course first".to_owned()));
        };
        if self.video.is_some() {
            return Err(AppError::Video(
                "this course has a video already".to_owned(),
            ));
        }
        let source = video::VideoSource {
            video: video.to_owned(),
            name: name.clone(),
            gpx: gpx.clone(),
            offset: Duration::ZERO,
            marks: marks.to_vec(),
            located: true,
        };
        let added = video::VideoCourse::new(route, &source).map_err(AppError::Video)?;
        let reference = video_reference(&added, route);
        update_manifest(path, |manifest| manifest.video = Some(reference))?;
        self.video = Some(added);
        self.view = View::Video;
        Ok(())
    }

    /// Takes the video off the loaded course, which is ridden in 3D only from then on; the
    /// route stays as it is.
    ///
    /// # Errors
    /// [`AppError::Video`] if the loaded course has no video, [`AppError::Storage`] if the
    /// course file cannot be updated.
    pub fn remove_video(&mut self) -> Result<(), AppError> {
        let (Some(video), Some(path)) = (&self.video, &self.course) else {
            return Err(AppError::Video("this course has no video".to_owned()));
        };
        if !video.located {
            return Err(AppError::Video(
                "this course is known only along its video and keeps it".to_owned(),
            ));
        }
        update_manifest(path, |manifest| manifest.video = None)?;
        self.video = None;
        self.view = View::World;
        Ok(())
    }

    /// How the next ride on a video course is shown: along its video, or in 3D (#44).
    /// Courses without a video are always ridden in 3D, courses without a place (Tacx RLV)
    /// always along their video.
    pub fn ride_along_video(&mut self, along: bool) {
        let unlocated = self.video.as_ref().is_some_and(|v| !v.located);
        self.view = if (along || unlocated) && self.video.is_some() {
            View::Video
        } else {
            View::World
        };
    }

    /// Whether the current ride plays the course's video.
    #[must_use]
    pub fn riding_along_video(&self) -> bool {
        self.ride.as_ref().is_some_and(|ride| ride.player.is_some())
    }

    /// Replaces the marks of the loaded video course's video (see [`App::add_video`]), for
    /// videos placed on the route by hand; the course file keeps the new marks.
    ///
    /// # Errors
    /// [`AppError::Video`] if the loaded course is not aligned by hand or the marks do not fit
    /// the video, [`AppError::Storage`] if the course file cannot be updated.
    pub fn align_video(&mut self, marks: &[SyncMark]) -> Result<(), AppError> {
        let (Some(route), Some(current), Some((name, gpx))) =
            (&self.route, &self.video, &self.loaded)
        else {
            return Err(AppError::Video("no video course loaded".to_owned()));
        };
        if !current.aligned_by_hand() {
            return Err(AppError::Video(
                "this video follows its GPS; there is nothing to align".to_owned(),
            ));
        }
        let source = video::VideoSource {
            video: current.video.clone(),
            name: name.clone(),
            gpx: gpx.clone(),
            offset: Duration::ZERO,
            marks: marks.to_vec(),
            located: current.located,
        };
        let aligned = video::VideoCourse::new(route, &source).map_err(AppError::Video)?;
        if let Some(path) = &self.course {
            let reference = video_reference(&aligned, route);
            update_manifest(path, |manifest| manifest.video = Some(reference))?;
        }
        self.video = Some(aligned);
        Ok(())
    }

    /// The frame of the video at `path` shown at `time`, e.g. to see where a route starts in
    /// it. The video stays open for the next call.
    ///
    /// # Errors
    /// [`AppError::Video`] if the video cannot be opened or decoded.
    pub fn video_preview(&mut self, path: &Path, time: Duration) -> Result<Frame, AppError> {
        let opened = match &mut self.preview {
            Some((open, video)) if open == path => video,
            preview => {
                let video = Video::open(path).map_err(|e| AppError::Video(e.to_string()))?;
                &mut preview.insert((path.to_owned(), video)).1
            }
        };
        opened
            .frame_at(time)
            .cloned()
            .map_err(|e| AppError::Video(e.to_string()))
    }

    /// Done with [`App::video_preview`]: closes the video.
    pub fn close_video_preview(&mut self) {
        self.preview = None;
    }

    /// The moment of the video to show now, during a ride on a video course.
    #[must_use]
    pub fn video_time(&self) -> Option<Duration> {
        let video = self.video.as_ref()?;
        Some(video.time_at(self.ride_state()?.distance))
    }

    /// The newest video frame decoded for the ride on a video course, once; it follows the
    /// moment of [`App::video_time`], so the view can blend from the frame before.
    pub fn video_frame(&mut self) -> Option<torqa_video::Frame> {
        let active = self.ride.as_mut()?;
        let frame = active.player.as_ref()?.frame()?;
        active.video_watch.frames += 1;
        Some(frame)
    }

    /// Reports once if the ride's video cannot be decoded or shows no picture.
    fn watch_video(&mut self, events: &mut Vec<AppEvent>) {
        let Some(active) = &mut self.ride else { return };
        let Some(player) = &active.player else { return };
        if active.video_watch.reported {
            return;
        }
        let problem = player
            .error()
            .map(|error| format!("cannot play the video: {error}"))
            .or_else(|| {
                (active.video_watch.frames == 0
                    && active.video_watch.since.elapsed() > NO_PICTURE_AFTER)
                    .then(|| "the video shows no picture yet — see the log for why".to_owned())
            });
        if let Some(message) = problem {
            warn!(%message, "video course");
            active.video_watch.reported = true;
            events.push(AppEvent::Error(message));
        }
    }

    /// The next stereo samples of the video's sound during a ride on a video course, at most
    /// `max`; at [`App::video_sound_rate`] samples per second. Empty without sound.
    pub fn video_sound(&mut self, max: usize) -> Vec<torqa_video::audio::Stereo> {
        self.ride
            .as_ref()
            .and_then(|active| active.sound.as_ref())
            .map_or_else(Vec::new, |sound| sound.pull(max))
    }

    /// Samples per second of [`App::video_sound`]; `None` while riding without the video's
    /// sound.
    #[must_use]
    pub fn video_sound_rate(&self) -> Option<u32> {
        Some(self.ride.as_ref()?.sound.as_ref()?.rate())
    }

    /// Plays the video's sound during this ride or not (R26, a ride option).
    pub fn set_video_sound(&mut self, on: bool) {
        if let Some(sound) = self.ride.as_ref().and_then(|a| a.sound.as_ref()) {
            sound.set_on(on);
        }
    }

    /// Saves the loaded route with everything needed to ride it offline as a course in the
    /// library; reports [`AppEvent::CourseAdded`].
    ///
    /// # Errors
    /// [`AppError::CourseNotReady`] until the route's world has been built.
    pub fn save_course(&mut self) -> Result<(), AppError> {
        // A GPX course is complete with the data of its world; a video course rides along
        // its video and builds the world on demand.
        let (Some(route), Some((name, gpx))) = (&self.route, &self.loaded) else {
            return Err(AppError::CourseNotReady);
        };
        if self.world.is_none() && self.video.is_none() {
            return Err(AppError::CourseNotReady);
        }
        let naming = self.naming.take();
        let name = naming
            .as_ref()
            .map_or_else(|| name.clone(), |n| n.name.clone());
        let manifest = Manifest {
            format: course::FORMAT_VERSION,
            generator: format!("Torqa {}", torqa_domain::version()),
            name: name.clone(),
            length_m: route.length().0,
            elevation_gain_m: route.elevation_gain().0,
            max_grade_percent: route.max_grade().0,
            created_unix_s: SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
            attribution: ATTRIBUTION.map(ToOwned::to_owned).to_vec(),
            route_key: Some(route.key()),
            track: thin(&view::track(route, PREVIEW_POINTS)),
            profile: thin(&view::elevation_profile(route, PREVIEW_POINTS)),
            video: self.video.as_ref().map(|v| video_reference(v, route)),
        };
        // Preparing the same course again must not fill the library with copies; one still
        // being written reports itself when done. A course named on import is the rider's
        // choice: saved as asked. A video course is not the GPX course of its route (#40).
        if naming.is_none() {
            if manifest.route_key.is_some() && manifest.route_key == self.saving {
                return Ok(());
            }
            let video_file = |m: &Manifest| m.video.as_ref().map(|v| v.file_name.clone());
            if let Some(existing) = self.courses().into_iter().find(|c| {
                c.manifest.route_key == manifest.route_key
                    && video_file(&c.manifest) == video_file(&manifest)
            }) {
                let _ = self.jobs_tx.send(JobResult::CourseSaved(Ok(existing.path)));
                return Ok(());
            }
        }
        let replacing = self.import_target(naming.as_ref());
        let gpx = gpx.clone();
        if let Some((loaded, _)) = &mut self.loaded {
            loaded.clone_from(&name);
        }
        self.saving.clone_from(&manifest.route_key);
        let data = self.used.paths();
        let cache = self.cache_dir.clone();
        let library = self.courses_dir();
        let tx = self.jobs_tx.clone();
        self.runtime.spawn_blocking(move || {
            let result = std::fs::create_dir_all(&library)
                .map_err(course::CourseError::from)
                .and_then(|()| {
                    let path =
                        replacing.unwrap_or_else(|| unique_course_path(&library, &manifest.name));
                    // Written aside first: a replaced course stays whole if writing fails.
                    let partial = path.with_extension("part");
                    course::write(&partial, &manifest, &gpx, &cache, &data)?;
                    std::fs::rename(&partial, &path)?;
                    Ok(path)
                })
                .map_err(|e| format!("cannot save course: {e}"));
            let _ = tx.send(JobResult::CourseSaved(result));
        });
        Ok(())
    }

    /// Builds the 3D world of a course opened with [`App::open_course`], now or as soon as its
    /// route is loaded; reports [`AppEvent::WorldReady`]. True if the world is ready already.
    pub fn build_world(&mut self) -> bool {
        if self.world.is_some() {
            return true;
        }
        // A video course's file holds the data of its route only, as no world was built when
        // it was saved: the world fetches the rest when online.
        if self.video.is_some() {
            self.offline = false;
        }
        if let WorldPlan::OnRequest(map) = std::mem::replace(&mut self.world_plan, WorldPlan::Now)
            && let (Some(route), Some(map)) = (self.route.clone(), map)
        {
            self.generate_world(route, *map);
        }
        false
    }

    /// The course file of the loaded route: the opened course, or the one a GPX import was
    /// saved as.
    #[must_use]
    pub fn loaded_course(&self) -> Option<&Path> {
        self.course.as_deref()
    }

    /// Renames the course at `path`; blank names are refused.
    ///
    /// # Errors
    /// [`AppError::Storage`] if the name is blank or the file cannot be rewritten.
    pub fn rename_course(&mut self, path: &Path, name: &str) -> Result<(), AppError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(AppError::Storage("a course needs a name".to_owned()));
        }
        update_manifest(path, |manifest| name.clone_into(&mut manifest.name))?;
        if self.course.as_deref() == Some(path)
            && let Some((loaded, _)) = &mut self.loaded
        {
            name.clone_into(loaded);
        }
        Ok(())
    }

    /// Deletes the course at `path` from the library; rides on it stay in the history.
    ///
    /// # Errors
    /// [`AppError::Storage`] if the file cannot be deleted.
    pub fn delete_course(&mut self, path: &Path) -> Result<(), AppError> {
        std::fs::remove_file(path).map_err(|e| AppError::Storage(e.to_string()))?;
        if self.course.as_deref() == Some(path) {
            self.course = None;
        }
        Ok(())
    }

    /// Names the course the next import adds to the library ([`App::load_route`],
    /// [`App::load_video`] or [`App::import_course`]): saved as `name`, replacing a course of
    /// that name with `replace`, else next to it (#40).
    pub fn name_next_import(&mut self, name: &str, replace: bool) {
        self.naming = Some(Naming {
            name: name.trim().to_owned(),
            replace,
        });
    }

    /// The library's course named `name` (ignoring case and surrounding spaces), if any.
    #[must_use]
    pub fn course_named(&self, name: &str) -> Option<PathBuf> {
        let name = name.trim();
        self.courses()
            .into_iter()
            .find(|c| c.manifest.name.trim().eq_ignore_ascii_case(name))
            .map(|c| c.path)
    }

    /// Where the course from the next import goes: next to others, or over the one of that name.
    fn import_target(&self, naming: Option<&Naming>) -> Option<PathBuf> {
        naming
            .filter(|n| n.replace)
            .and_then(|n| self.course_named(&n.name))
    }

    /// Copies a course file into the library; reports [`AppEvent::CourseAdded`].
    pub fn import_course(&mut self, path: PathBuf) {
        let library = self.courses_dir();
        let tx = self.jobs_tx.clone();
        let naming = self.naming.take();
        let replacing = self.import_target(naming.as_ref());
        self.runtime.spawn_blocking(move || {
            let result = course::read_manifest(&path)
                .and_then(|mut manifest| {
                    std::fs::create_dir_all(&library)?;
                    if let Some(naming) = &naming {
                        manifest.name.clone_from(&naming.name);
                    }
                    let target =
                        replacing.unwrap_or_else(|| unique_course_path(&library, &manifest.name));
                    // Not `.part`: renaming rewrites the copy through a `.part` file itself.
                    let partial = target.with_extension("import");
                    std::fs::copy(&path, &partial)?;
                    if naming.is_some() {
                        course::rewrite_manifest(&partial, &manifest)?;
                    }
                    std::fs::rename(&partial, &target)?;
                    Ok(target)
                })
                .map_err(|e| format!("cannot import {}: {e}", path.display()));
            let _ = tx.send(JobResult::CourseAdded(result));
        });
    }

    /// The courses in the library, by name. Unreadable files are skipped.
    #[must_use]
    pub fn courses(&self) -> Vec<CourseEntry> {
        let Ok(entries) = std::fs::read_dir(self.courses_dir()) else {
            return Vec::new();
        };
        let mut courses: Vec<CourseEntry> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|e| e == course::EXTENSION))
            .filter_map(|path| match course::read_manifest(&path) {
                Ok(manifest) => Some(CourseEntry { path, manifest }),
                Err(error) => {
                    warn!(path = %path.display(), %error, "skipping course");
                    None
                }
            })
            .collect();
        courses.sort_by(|a, b| a.manifest.name.cmp(&b.manifest.name));
        courses
    }

    /// A name for the course imported from `path`, for the rider to confirm: a GPX file's
    /// route name, an Incyclist video's title, a course file's name, else the file name.
    #[must_use]
    pub fn suggested_course_name(path: &Path) -> String {
        let extension = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let read = || std::fs::read_to_string(path).ok();
        let named = match extension.as_str() {
            "gpx" => read().and_then(|xml| torqa_routes::route_name(&xml)),
            "xml" => read()
                .and_then(|xml| torqa_video::incyclist::parse(&xml).ok())
                .map(|route| route.title),
            "tqc" => course::read_manifest(path).ok().map(|m| m.name),
            "rlv" => video::tacx_course_name(path),
            _ => None,
        };
        named.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| {
            path.file_stem()
                .map_or_else(|| "Course".to_owned(), |s| s.to_string_lossy().into_owned())
        })
    }

    /// The course library: `courses/` in the data directory (R34).
    #[must_use]
    pub fn courses_dir(&self) -> PathBuf {
        self.data_dir.join("courses")
    }

    /// Forgets the current route and starts recording the files a new one uses.
    fn start_loading(&mut self, offline: bool) -> UsedFiles {
        self.load += 1;
        self.offline = offline;
        self.world_plan = WorldPlan::Now;
        self.course = None;
        // Opening a course sets this; a GPX import goes into the library when ready.
        self.from_course = false;
        self.video = None;
        self.view = View::World;
        self.world = None;
        self.route = None;
        self.loaded = None;
        self.used = UsedFiles::default();
        self.used.clone()
    }

    /// Generates the 3D world for `route` in the background; reports [`AppEvent::WorldReady`].
    fn generate_world(&mut self, route: Route, map: torqa_osm::MapData) {
        let tx = self.jobs_tx.clone();
        let load = self.load;
        let terrain = |sources| {
            let terrain =
                Terrain::new(sources, self.cache_dir.join("terrain")).recording(self.used.clone());
            if self.offline {
                terrain.offline()
            } else {
                terrain
            }
        };
        let mut detailed = terrain(TileSource::defaults());
        // The land beyond the corridor needs only coarse tiles, a few for the whole view.
        let mut distant = terrain(TileSource::distant());
        self.runtime.spawn(async move {
            let mut reporter = Reporter {
                tx: tx.clone(),
                load,
                last: None,
            };
            let mut world =
                torqa_world::generate(&route, &mut detailed, &map, &mut |done, total| {
                    reporter.report(LoadStage::World, done, total);
                })
                .await;
            world.horizon = torqa_world::horizon(&route, &mut distant).await;
            let _ = tx.send(JobResult::World(load, Box::new(world)));
        });
    }

    /// How detailed the 3D world is drawn on this computer (R43).
    #[must_use]
    pub fn graphics_quality(&self) -> profiles::GraphicsQuality {
        profiles::graphics_quality(&self.data_dir)
    }

    /// Chooses how detailed the 3D world is drawn on this computer (R43).
    ///
    /// # Errors
    /// [`AppError::Storage`] if the choice cannot be saved.
    pub fn set_graphics_quality(
        &mut self,
        quality: profiles::GraphicsQuality,
    ) -> Result<(), AppError> {
        profiles::set_graphics_quality(&self.data_dir, quality)
            .map_err(|e| AppError::Storage(e.to_string()))
    }

    /// Where the overlay was last on screen (R55); `None` before it was first used.
    #[must_use]
    pub fn overlay_window(&self) -> Option<profiles::OverlayWindow> {
        profiles::overlay_window(&self.data_dir)
    }

    /// Remembers where the overlay is on screen.
    ///
    /// # Errors
    /// [`AppError::Storage`] if it cannot be saved.
    pub fn set_overlay_window(&mut self, window: profiles::OverlayWindow) -> Result<(), AppError> {
        profiles::set_overlay_window(&self.data_dir, window)
            .map_err(|e| AppError::Storage(e.to_string()))
    }

    /// Whether rides are simulated: with the fake trainer, which can be sped up and jumped
    /// along the route (#53).
    #[must_use]
    pub fn simulating(&self) -> bool {
        self.trainer.is_some() && self.trainer_id.is_none()
    }

    /// Speeds the simulated ride up (or back down to 1): ride time per real time, from 1 to
    /// 20; returns the speed in effect. Without a simulated ride, 1.
    pub fn set_time_scale(&mut self, scale: f64) -> f64 {
        let simulating = self.simulating();
        let Some(active) = self.ride.as_mut().filter(|_| simulating) else {
            return 1.0;
        };
        active.time_scale = if scale.is_finite() {
            scale.clamp(1.0, MAX_TIME_SCALE)
        } else {
            1.0
        };
        if active.time_scale > 1.0 {
            active.simulated = true;
        }
        active.time_scale
    }

    /// Moves the simulated ride's rider to `distance` along the route; false without one.
    pub fn jump_to(&mut self, distance: Meters) -> bool {
        let simulating = self.simulating();
        let Some(active) = self.ride.as_mut().filter(|_| simulating) else {
            return false;
        };
        active.ride.jump_to(distance);
        active.simulated = true;
        // Climbs passed by jumping are not timed; the next one counts from where it starts.
        let distance = active.ride.state().distance;
        let climbs = active.ride.route().map_or(&[][..], Route::climbs);
        active.next_climb = climbs
            .iter()
            .position(|c| c.start.0 >= distance.0)
            .unwrap_or(climbs.len());
        true
    }

    /// Moves the simulated ride's rider to the route's point nearest to `x`/`y` (metres east
    /// and north of the start, as on the map); false without a simulated ride.
    pub fn jump_near(&mut self, x: f64, y: f64) -> bool {
        let Some(route) = self.ride.as_ref().and_then(|a| a.ride.route()) else {
            return false;
        };
        let projection = LocalProjection::for_route(route);
        let nearest = route
            .points()
            .iter()
            .map(|p| {
                let (px, py) = projection.project(p.lat, p.lon);
                ((px - x).hypot(py - y), p.distance)
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, distance)| distance);
        nearest.is_some_and(|distance| self.jump_to(distance))
    }

    /// Whether the trainer is connected now.
    #[must_use]
    pub fn trainer_connected(&self) -> bool {
        self.trainer_connected.is_some()
    }

    /// Connects the trainer, replacing any previous one.
    ///
    /// # Errors
    /// [`AppError::UnknownDevice`] if the index is not a trainer from the last scan.
    pub fn connect_trainer(&mut self, choice: TrainerChoice) -> Result<(), AppError> {
        let _runtime = self.runtime.enter();
        let handle = match choice {
            TrainerChoice::Fake(mut rider) => {
                self.trainer_id = None;
                // A real strap's heart rate is the one to use.
                if self.sensor.is_some() {
                    rider.heart = None;
                }
                fake::spawn(rider, Duration::from_millis(250))
            }
            TrainerChoice::Discovered(index) => {
                let device = self.discovered_device(index, DeviceKind::Trainer)?;
                let id = device.id();
                // Already connected (e.g. reconnected at start): keep the link.
                if self.trainer.is_some() && self.trainer_id.as_deref() == Some(id.as_str()) {
                    return Ok(());
                }
                self.remember(DeviceRole::Trainer, &device);
                self.trainer_id = Some(id);
                self.bluetooth()?.connect(device)
            }
        };
        self.trainer = Some(handle);
        self.trainer_connected = None;
        Ok(())
    }

    /// Reconnects the trainer and heart-rate sensor used last (R41): scans in the background
    /// and connects them when found, reporting [`AppEvent::DevicesFound`] (with the remembered
    /// devices marked) and [`AppEvent::RememberedMissing`] for those not found. Returns false,
    /// without touching Bluetooth, if no device is remembered.
    pub fn reconnect_remembered(&mut self) -> bool {
        let remembered = profiles::remembered_devices(&self.data_dir);
        if remembered.trainer.is_none()
            && remembered.heart_rate.is_none()
            && remembered.controller.is_none()
        {
            return false;
        }
        self.reconnecting = true;
        self.scan(RECONNECT_SCAN);
        true
    }

    fn remember(&self, role: DeviceRole, device: &DiscoveredDevice) {
        let remembered = profiles::RememberedDevice {
            id: device.id(),
            name: device.name.clone(),
        };
        if let Err(error) = profiles::remember_device(&self.data_dir, role, remembered) {
            warn!(%error, "cannot remember the device");
        }
    }

    /// Connects the remembered devices found by a reconnect scan; returns the names of those
    /// not found.
    fn connect_remembered(&mut self) -> Vec<String> {
        let remembered = profiles::remembered_devices(&self.data_dir);
        let mut missing = Vec::new();
        for (wanted, kind) in [
            (remembered.trainer, DeviceKind::Trainer),
            (remembered.heart_rate, DeviceKind::HeartRateSensor),
            (remembered.controller, DeviceKind::Controller),
        ] {
            let Some(wanted) = wanted else { continue };
            let found = self
                .discovered
                .iter()
                .position(|d| d.kind == kind && d.id() == wanted.id)
                .or_else(|| {
                    self.discovered
                        .iter()
                        .position(|d| d.kind == kind && d.name == wanted.name)
                });
            let connected = match (found, kind) {
                (Some(index), DeviceKind::Trainer) => {
                    self.connect_trainer(TrainerChoice::Discovered(index))
                }
                (Some(index), DeviceKind::HeartRateSensor) => self.connect_heart_rate(index),
                (Some(index), DeviceKind::Controller) => self.connect_controller(index),
                (None, _) => Err(AppError::UnknownDevice),
            };
            if connected.is_err() {
                missing.push(wanted.name);
            }
        }
        missing
    }

    /// Whether a scanned device is one of the remembered ones.
    fn is_remembered(&self, device: &DiscoveredDevice) -> bool {
        let remembered = profiles::remembered_devices(&self.data_dir);
        let wanted = match device.kind {
            DeviceKind::Trainer => remembered.trainer,
            DeviceKind::HeartRateSensor => remembered.heart_rate,
            DeviceKind::Controller => remembered.controller,
        };
        wanted.is_some_and(|w| w.id == device.id() || w.name == device.name)
    }

    /// Connects a heart-rate sensor, replacing any previous one.
    ///
    /// # Errors
    /// [`AppError::UnknownDevice`] if the index is not a heart-rate sensor from the last scan.
    pub fn connect_heart_rate(&mut self, index: usize) -> Result<(), AppError> {
        let device = self.discovered_device(index, DeviceKind::HeartRateSensor)?;
        let id = device.id();
        if self.sensor.is_some() && self.sensor_id.as_deref() == Some(id.as_str()) {
            return Ok(());
        }
        let _runtime = self.runtime.enter();
        self.remember(DeviceRole::HeartRate, &device);
        self.sensor_id = Some(id);
        self.sensor = Some(self.bluetooth()?.connect(device));
        Ok(())
    }

    /// Connects a Shimano Di2 shifter whose D-Fly buttons shift (R7), replacing any previous
    /// one; the keyboard keeps shifting too.
    ///
    /// # Errors
    /// [`AppError::UnknownDevice`] if the index is not a controller from the last scan.
    pub fn connect_controller(&mut self, index: usize) -> Result<(), AppError> {
        let device = self.discovered_device(index, DeviceKind::Controller)?;
        let id = device.id();
        if self.controller_id.as_deref() == Some(id.as_str()) {
            return Ok(());
        }
        let _runtime = self.runtime.enter();
        self.remember(DeviceRole::Controller, &device);
        let handle = self.bluetooth()?.connect(device);
        self.controller_id = Some(id);
        let channels = Arc::clone(&self.shift_channels);
        self.add_shift_input(Box::new(torqa_devices::shift::Controller::new(
            handle, channels,
        )));
        Ok(())
    }

    /// The D-Fly channels (1–4) whose buttons shift up and down.
    #[must_use]
    pub fn shift_channels(&self) -> (u8, u8) {
        self.shift_channels.get()
    }

    /// Chooses the D-Fly channels that shift up and down, at once and for next time.
    ///
    /// # Errors
    /// [`AppError::Storage`] if the choice cannot be saved.
    pub fn set_shift_channels(&mut self, up: u8, down: u8) -> Result<(), AppError> {
        self.shift_channels.set(up, down);
        profiles::set_shift_channels(&self.data_dir, up, down)
            .map_err(|e| AppError::Storage(e.to_string()))
    }

    /// Shifts from `input` from now on, instead of the shifter connected before (R7).
    pub fn add_shift_input(&mut self, input: Box<dyn ShiftInput>) {
        // The keyboard stays first; one controller at a time after it.
        self.shift_inputs.truncate(1);
        self.shift_inputs_connected.truncate(1);
        self.shift_inputs_connected.push(false);
        self.shift_inputs.push(input);
    }

    /// Starts riding the loaded route as the active rider, whose profile sets the mass. The
    /// clock starts once the trainer is connected.
    ///
    /// # Errors
    /// [`AppError::NoRoute`] or [`AppError::NoTrainer`] if either is missing.
    pub fn start_ride(
        &mut self,
        difficulty: Percent,
        descent: DescentMode,
        ghost: &GhostChoice,
    ) -> Result<(), AppError> {
        let route = self.route.clone().ok_or(AppError::NoRoute)?;
        if self.trainer.is_none() {
            return Err(AppError::NoTrainer);
        }
        let ghost = self.ghost_for(&route, descent, ghost)?;
        let along = self.video.as_ref().filter(|_| self.view == View::Video);
        let player = along
            .map(|v| video::VideoPlayer::open(&v.video))
            .transpose()
            .map_err(AppError::Video)?;
        // A ride without the video's sound is still a ride.
        let sound = along.and_then(|v| {
            video::SoundPlayer::open(&v.video)
                .inspect_err(|error| warn!(%error, "no sound for the video"))
                .ok()
                .flatten()
        });
        let records = self.records_for(&route);
        let ride = Ride::new(route, self.ride_config(difficulty, descent));
        self.begin(ride, None, records, ghost, (player, sound));
        Ok(())
    }

    /// Starts `workout` as the active rider (R56, R58): on the loaded course in its 3D world
    /// if `on_course` (the gradient then sets only the speed), otherwise on its own on a flat
    /// road, going into the history as `name`. The clock starts once the trainer is connected.
    ///
    /// # Errors
    /// [`AppError::NoTrainer`] without a trainer, [`AppError::NoRoute`] if `on_course` but no
    /// course is loaded.
    pub fn start_workout(
        &mut self,
        workout: Workout,
        on_course: bool,
        descent: DescentMode,
        name: &str,
    ) -> Result<(), AppError> {
        if self.trainer.is_none() {
            return Err(AppError::NoTrainer);
        }
        let config = self.ride_config(Percent(100.0), descent);
        if on_course {
            let route = self.route.clone().ok_or(AppError::NoRoute)?;
            self.view = View::World;
            let records = self.records_for(&route);
            let ride = Ride::new(route, config).with_workout(workout);
            self.begin(ride, None, records, None, (None, None));
        } else {
            let ride = Ride::workout(workout, config);
            let records = RouteRecords::default();
            self.begin(ride, Some(name.to_owned()), records, None, (None, None));
        }
        Ok(())
    }

    /// The workouts to choose from (R21): the built-in ones, then the library's files.
    #[must_use]
    pub fn workouts(&self) -> Vec<torqa_workouts::Entry> {
        torqa_workouts::library(&torqa_workouts::library_dir(&self.data_dir))
    }

    /// Adds a workout file (ZWO, ERG, MRC or FIT) to the library; returns its id.
    ///
    /// # Errors
    /// [`AppError::Workout`] if it is no workout Torqa understands or cannot be copied.
    pub fn import_workout(&mut self, file: &Path) -> Result<String, AppError> {
        torqa_workouts::import(&torqa_workouts::library_dir(&self.data_dir), file)
            .map(|path| path.display().to_string())
            .map_err(|e| AppError::Workout(e.to_string()))
    }

    /// Saves a workout made in the editor into the library (over `replace` if that is the
    /// library's ZWO file being edited); returns its id.
    ///
    /// # Errors
    /// [`AppError::Workout`] if it has no steps or cannot be written.
    pub fn save_workout(
        &mut self,
        plan: &torqa_domain::workout::Plan,
        replace: Option<&str>,
    ) -> Result<String, AppError> {
        let dir = torqa_workouts::library_dir(&self.data_dir);
        torqa_workouts::save(&dir, plan, self.profile.profile.ftp, replace)
            .map_err(|e| AppError::Workout(e.to_string()))
    }

    /// Deletes a workout file from the library.
    ///
    /// # Errors
    /// [`AppError::Workout`] for built-ins and files that cannot be deleted.
    pub fn delete_workout(&mut self, id: &str) -> Result<(), AppError> {
        torqa_workouts::delete(&torqa_workouts::library_dir(&self.data_dir), id)
            .map_err(|e| AppError::Workout(e.to_string()))
    }

    /// A structured workout by its id (see [`App::workouts`]), for the active rider's FTP.
    ///
    /// # Errors
    /// [`AppError::Workout`] if it cannot be read.
    pub fn structured_workout(&self, id: &str) -> Result<Workout, AppError> {
        let plan = torqa_workouts::load(id).map_err(|e| AppError::Workout(e.to_string()))?;
        Ok(Workout::Structured {
            plan: Arc::new(plan),
            ftp: self.profile.profile.ftp,
        })
    }

    /// Makes `ftp` the active rider's FTP, e.g. from an FTP test (R22).
    ///
    /// # Errors
    /// [`AppError::Storage`] if the profile cannot be saved.
    pub fn set_ftp(&mut self, ftp: Watts) -> Result<(), AppError> {
        let mut profile = self.profile.profile.clone();
        profile.ftp = ftp;
        let id = self.profile.id.clone();
        self.save_profile(Some(&id), profile).map(|_| ())
    }

    /// Changes the workout of the current workout, e.g. its target power or heart rate.
    pub fn change_workout(&mut self, workout: Workout) {
        if let Some(active) = &mut self.ride {
            active.ride.change_workout(workout);
        }
    }

    fn ride_config(&self, difficulty: Percent, descent: DescentMode) -> RideConfig {
        RideConfig {
            setup: RiderSetup {
                mass: self.profile.profile.system_mass(),
                ..RiderSetup::default()
            },
            difficulty,
            descent,
            gears: match self.profile.profile.drivetrain {
                Drivetrain::SingleCog { chainring, cog } => Some(VirtualGears::new(chainring, cog)),
                Drivetrain::Cassette => None,
            },
        }
    }

    /// A shift key was pressed (R7); the ride shifts on the next update.
    pub fn shift(&mut self, shift: Shift) {
        self.keys.press(shift);
    }

    fn begin(
        &mut self,
        ride: Ride,
        name: Option<String>,
        records: RouteRecords,
        ghost: Option<Ghost>,
        (player, sound): (Option<video::VideoPlayer>, Option<video::SoundPlayer>),
    ) {
        self.ride = Some(ActiveRide {
            ride,
            name,
            // With the trainer connected already (e.g. while the world was built), the ride
            // starts now; otherwise when it connects.
            started: self.trainer_connected.map(|_| SystemTime::now()),
            finished: false,
            records,
            next_climb: 0,
            summary: (0, RideSummary::default()),
            ghost,
            player,
            sound,
            video_watch: VideoWatch {
                since: std::time::Instant::now(),
                frames: 0,
                reported: false,
            },
            time_scale: 1.0,
            simulated: false,
        });
    }

    /// Changes trainer difficulty and descent mode of the current ride (R48).
    pub fn adjust_ride(&mut self, difficulty: Percent, descent: DescentMode) {
        if let Some(active) = &mut self.ride {
            active.ride.adjust(difficulty, descent);
        }
    }

    /// Ends the ride without saving anything (R49).
    pub fn abort_ride(&mut self) {
        self.ride = None;
    }

    /// Ends the ride and saves it; reports [`AppEvent::RideSaved`] on the next update.
    pub fn finish_ride(&mut self) -> Vec<AppEvent> {
        let Some(active) = self.ride.take() else {
            return Vec::new();
        };
        let Some(start) = active.started else {
            return Vec::new();
        };
        if active.ride.samples().is_empty() {
            return Vec::new();
        }
        let path = profiles::rides_dir(&self.data_dir, &self.profile.id)
            .join(paths::activity_file_name(start));
        let samples = active.ride.samples();
        let saved = torqa_storage::encode_fit(start, samples)
            .map_err(|e| e.to_string())
            .and_then(|fit| {
                std::fs::create_dir_all(path.parent().unwrap_or(&self.data_dir))
                    .and_then(|()| std::fs::write(&path, fit))
                    .map_err(|e| format!("cannot save {}: {e}", path.display()))
            });
        if saved.is_ok() {
            let route = active.ride.route();
            let finished = samples
                .last()
                .zip(route)
                .is_some_and(|(s, route)| s.distance.0 >= route.length().0 - 1.0);
            // A simulated ride's times are not real: like a ride known only from its FIT file,
            // it counts towards no records; neither does a workout without a route.
            let real = !active.simulated;
            let record = RideRecord {
                route: active
                    .name
                    .clone()
                    .or_else(|| self.loaded.as_ref().map(|(name, _)| name.clone()))
                    .unwrap_or_else(|| "Ride".to_owned()),
                start,
                summary: summarize(samples, self.profile.profile.ftp),
                route_key: route.filter(|_| real).map(Route::key),
                route_time: route
                    .filter(|_| finished && real)
                    .and_then(|route| effort(samples, Meters(0.0), route.length()))
                    .map(|e| e.elapsed),
                climbs: route
                    .map_or(&[][..], Route::climbs)
                    .iter()
                    .filter(|_| real)
                    .filter_map(|c| {
                        effort(samples, c.start, c.end).map(|e| ClimbTime {
                            start: c.start,
                            end: c.end,
                            elapsed: e.elapsed,
                            avg_power: e.avg_power,
                        })
                    })
                    .collect(),
                name: None,
                ftp_estimate: active.ride.ftp_estimate(),
            };
            // The FIT file is what counts; the history rebuilds missing metadata from it.
            if let Err(error) = rides::save(&path, &record) {
                warn!(%error, "cannot save ride metadata");
            }
        }
        vec![match saved {
            Ok(()) => AppEvent::RideSaved(path),
            Err(message) => AppEvent::Error(message),
        }]
    }

    /// The active rider's rides, newest first (R31). Rides without metadata, e.g. FIT files
    /// copied in by hand, are analysed once and get it written.
    #[must_use]
    pub fn history(&self) -> Vec<HistoryEntry> {
        let records: Vec<(PathBuf, RideRecord)> = rides::fit_files(&self.rides_dir())
            .into_iter()
            .filter_map(|fit| {
                let record = rides::load(&fit).or_else(|_| self.rebuild_metadata(&fit));
                match record {
                    Ok(record) => Some((fit, record)),
                    Err(error) => {
                        warn!(path = %fit.display(), %error, "skipping ride");
                        None
                    }
                }
            })
            .collect();
        let all: Vec<&RideRecord> = records.iter().map(|(_, r)| r).collect();
        records
            .iter()
            .map(|(fit, record)| {
                let same_route: Vec<&RideRecord> = all
                    .iter()
                    .copied()
                    .filter(|other| {
                        other.route_key.is_some() && other.route_key == record.route_key
                    })
                    .collect();
                let best_route = same_route.iter().filter_map(|r| r.route_time).min();
                HistoryEntry {
                    fit: fit.clone(),
                    route_record: record.route_time.is_some() && record.route_time == best_route,
                    climb_records: record
                        .climbs
                        .iter()
                        .map(|climb| Some(climb.elapsed) == best_climb_time(&same_route, climb))
                        .collect(),
                    record: record.clone(),
                }
            })
            .collect()
    }

    /// The active rider's best times on `route`, from earlier rides on the same course.
    #[must_use]
    pub fn records_for(&self, route: &Route) -> RouteRecords {
        let key = route.key();
        let history = self.history();
        let same_route: Vec<&RideRecord> = history
            .iter()
            .map(|entry| &entry.record)
            .filter(|record| record.route_key.as_deref() == Some(key.as_str()))
            .collect();
        RouteRecords {
            route: same_route.iter().filter_map(|r| r.route_time).min(),
            climbs: route
                .climbs()
                .iter()
                .map(|c| {
                    let probe = ClimbTime {
                        start: c.start,
                        end: c.end,
                        elapsed: Duration::ZERO,
                        avg_power: None,
                    };
                    best_climb_time(&same_route, &probe)
                })
                .collect(),
        }
    }

    /// The active rider's HUD metrics, in order (R23); the first is shown large.
    #[must_use]
    pub fn hud_layout(&self) -> Vec<String> {
        hud::sanitize(&profiles::load_hud(&self.data_dir, &self.profile.id).unwrap_or_default())
    }

    /// Saves the active rider's HUD metrics; unknown or repeated ones are dropped. Returns the
    /// layout as saved.
    ///
    /// # Errors
    /// [`AppError::Storage`] if it cannot be written.
    pub fn set_hud_layout(&mut self, layout: &[String]) -> Result<Vec<String>, AppError> {
        let layout = hud::sanitize(layout);
        profiles::save_hud(&self.data_dir, &self.profile.id, &layout)
            .map_err(|e| AppError::Storage(format!("cannot save HUD layout: {e}")))?;
        Ok(layout)
    }

    /// Live values of all HUD metrics while riding (see [`hud::values`]).
    #[must_use]
    pub fn hud_values(&self) -> Vec<(&'static str, Option<f64>)> {
        let Some(active) = &self.ride else {
            return Vec::new();
        };
        hud::values(
            &active.ride.state(),
            active.ride.samples(),
            &active.summary.1,
            active.ride.route(),
            &self.profile.profile,
        )
    }

    /// Tells the music app to play/pause or skip (R26), in the background; failures are
    /// reported as [`AppEvent::Error`].
    pub fn control_music(&mut self, command: media::MediaCommand) {
        let tx = self.jobs_tx.clone();
        self.runtime.spawn_blocking(move || {
            if let Err(message) = media::send(command) {
                let _ = tx.send(JobResult::Failed(message));
            }
        });
    }

    /// The ghost of the current ride, if any.
    #[must_use]
    pub fn ghost_state(&self) -> Option<GhostState> {
        let active = self.ride.as_ref()?;
        let ghost = active.ghost.as_ref()?;
        let state = active.ride.state();
        Some(GhostState {
            name: ghost.name.clone(),
            distance: ghost.distance_at(state.elapsed),
            gap: ghost.gap(state.distance, state.elapsed),
        })
    }

    fn ghost_for(
        &self,
        route: &Route,
        descent: DescentMode,
        choice: &GhostChoice,
    ) -> Result<Option<Ghost>, AppError> {
        let profile = &self.profile.profile;
        let setup = RiderSetup {
            mass: profile.system_mass(),
            ..RiderSetup::default()
        };
        let unavailable = |why: String| AppError::GhostUnavailable(why);
        let ghost = match choice {
            GhostChoice::None => return Ok(None),
            GhostChoice::PersonalBest => {
                let key = route.key();
                let best = self
                    .history()
                    .into_iter()
                    .filter(|h| h.record.route_key.as_deref() == Some(key.as_str()))
                    .filter_map(|h| h.record.route_time.map(|t| (t, h.fit)))
                    .min_by_key(|(t, _)| *t)
                    .ok_or_else(|| unavailable("no finished ride on this route yet".to_owned()))?;
                let (_, samples) = read_fit(&best.1).map_err(unavailable)?;
                Ghost::from_samples("Your best", &samples)
            }
            GhostChoice::Power(watts) => Some(Ghost::pacer(
                &format!("Pacer {:.0} W", watts.0),
                route,
                &setup,
                descent,
                *watts,
            )),
            GhostChoice::WattsPerKg(ratio) => Some(Ghost::pacer(
                &format!("Pacer {ratio:.1} W/kg"),
                route,
                &setup,
                descent,
                Watts(ratio * profile.rider_mass.0),
            )),
            GhostChoice::Activity(path) => {
                let points = activity_points(path).map_err(unavailable)?;
                let name = path
                    .file_stem()
                    .map_or_else(|| "Ghost".to_owned(), |s| s.to_string_lossy().into_owned());
                Some(Ghost::from_activity(&name, route, &points).ok_or_else(|| {
                    unavailable(format!("{} does not follow this route", path.display()))
                })?)
            }
        };
        Ok(ghost)
    }

    /// The climb the rider is on now, if any.
    #[must_use]
    pub fn current_climb(&self) -> Option<ClimbProgress> {
        let active = self.ride.as_ref()?;
        let state = active.ride.state();
        let (index, climb) = active
            .ride
            .route()?
            .climbs()
            .iter()
            .enumerate()
            .find(|(_, c)| c.start.0 <= state.distance.0 && state.distance.0 < c.end.0)?;
        let started = time_at(active.ride.samples(), climb.start).unwrap_or(state.elapsed);
        Some(ClimbProgress {
            index,
            climb: *climb,
            ridden: Meters(state.distance.0 - climb.start.0),
            elapsed: state.elapsed.saturating_sub(started),
            best: active.records.climbs.get(index).copied().flatten(),
        })
    }

    /// The recorded data of one ride, with zones of the active rider.
    ///
    /// # Errors
    /// [`AppError::Storage`] if the FIT file cannot be read.
    pub fn ride_detail(&self, fit: &Path) -> Result<RideDetail, AppError> {
        let (_, samples) = read_fit(fit).map_err(AppError::Storage)?;
        let profile = &self.profile.profile;
        Ok(RideDetail {
            power_zones: time_in_power_zones(&samples, profile),
            heart_rate_zones: time_in_heart_rate_zones(&samples, profile),
            samples,
        })
    }

    /// Names a ride (R50); an empty name goes back to the route and date. Only the metadata
    /// changes, so file names stay stable for syncing.
    ///
    /// # Errors
    /// [`AppError::Storage`] if the metadata cannot be read or written.
    pub fn rename_ride(&self, fit: &Path, name: &str) -> Result<(), AppError> {
        let storage = |e: rides::RideError| AppError::Storage(format!("cannot rename ride: {e}"));
        let mut record = rides::load(fit)
            .or_else(|_| self.rebuild_metadata(fit).map_err(AppError::Storage))
            .map_err(|e| AppError::Storage(e.to_string()))?;
        let name = name.trim();
        record.name = (!name.is_empty()).then(|| name.to_owned());
        rides::save(fit, &record).map_err(storage)
    }

    /// Deletes a ride: its FIT file and metadata.
    ///
    /// # Errors
    /// [`AppError::Storage`] if the FIT file cannot be removed.
    pub fn delete_ride(&self, fit: &Path) -> Result<(), AppError> {
        std::fs::remove_file(fit)
            .map_err(|e| AppError::Storage(format!("cannot delete {}: {e}", fit.display())))?;
        let _ = std::fs::remove_file(rides::metadata_path(fit));
        Ok(())
    }

    fn rides_dir(&self) -> PathBuf {
        profiles::rides_dir(&self.data_dir, &self.profile.id)
    }

    fn rebuild_metadata(&self, fit: &Path) -> Result<RideRecord, String> {
        let (start, samples) = read_fit(fit)?;
        let record = RideRecord {
            route: fit
                .file_stem()
                .map_or_else(|| "Ride".to_owned(), |s| s.to_string_lossy().into_owned()),
            start,
            summary: summarize(&samples, self.profile.profile.ftp),
            // The route is unknown, so this ride counts towards no records.
            route_key: None,
            route_time: None,
            climbs: Vec::new(),
            name: None,
            ftp_estimate: None,
        };
        if let Err(error) = rides::save(fit, &record) {
            warn!(%error, "cannot save rebuilt ride metadata");
        }
        Ok(record)
    }

    /// Processes everything that happened since the last call and advances the ride by `dt`.
    pub fn update(&mut self, dt: Duration) -> Vec<AppEvent> {
        let mut events = Vec::new();
        self.poll_jobs(&mut events);
        self.poll_trainer(&mut events);
        self.poll_sensor(&mut events);
        for (input, was_connected) in self
            .shift_inputs
            .iter_mut()
            .zip(&mut self.shift_inputs_connected)
        {
            for shift in input.poll() {
                if let Some(active) = &mut self.ride {
                    active.ride.shift(shift);
                }
            }
            let connected = input.connected();
            if connected != *was_connected {
                *was_connected = connected;
                let name = input.name().to_owned();
                events.push(if connected {
                    AppEvent::Connected(name)
                } else {
                    AppEvent::Disconnected(name)
                });
            }
        }

        if let Some(active) = &mut self.ride
            && active.started.is_some()
            && !active.finished
        {
            // Sped up, the ride advances in small steps all the same, so its physics and
            // one-second samples stay as exact as at real speed.
            let mut left = dt.mul_f64(active.time_scale);
            let mut control = None;
            while !left.is_zero() {
                let step = left.min(MAX_RIDE_STEP);
                left -= step;
                control = active.ride.tick(step).or(control);
            }
            if let Some(control) = control
                && let Some(trainer) = &self.trainer
                && let Err(error) = trainer.try_control(control)
            {
                warn!(%error, "cannot control trainer");
            }
            let samples = active.ride.samples();
            if samples.len() != active.summary.0 {
                active.summary = (samples.len(), summarize(samples, self.profile.profile.ftp));
            }
            // Samples arrive once a second, so a climb is timed once a sample lies past its top.
            let climbs = active.ride.route().map_or(&[][..], Route::climbs);
            while let Some(climb) = climbs.get(active.next_climb)
                && let Some(done) = effort(active.ride.samples(), climb.start, climb.end)
            {
                events.push(AppEvent::ClimbCompleted {
                    index: active.next_climb,
                    elapsed: done.elapsed,
                    previous_best: active
                        .records
                        .climbs
                        .get(active.next_climb)
                        .copied()
                        .flatten(),
                });
                active.next_climb += 1;
            }
            if active.ride.is_finished() {
                active.finished = true;
                events.push(AppEvent::RouteCompleted {
                    elapsed: active.ride.state().elapsed,
                    previous_best: active.records.route,
                });
                events.push(AppEvent::RideFinished);
            }
        }
        if let (Some(active), Some(video)) = (&self.ride, &self.video) {
            let time = video.time_at(active.ride.state().distance);
            if let Some(player) = &active.player {
                player.show(time);
            }
            if let Some(sound) = &active.sound {
                sound.follow(time);
            }
        }
        self.watch_video(&mut events);
        events
    }

    /// The current ride's samples so far, one per second, e.g. for a live chart.
    #[must_use]
    pub fn ride_samples(&self) -> &[Sample] {
        self.ride
            .as_ref()
            .map_or(&[], |active| active.ride.samples())
    }

    /// The current ride's state, if riding.
    #[must_use]
    pub fn ride_state(&self) -> Option<RideState> {
        self.ride.as_ref().map(|active| active.ride.state())
    }

    /// The loaded route.
    #[must_use]
    pub fn route(&self) -> Option<&Route> {
        self.route.as_ref()
    }

    /// The 3D world of the loaded route, once generated.
    #[must_use]
    pub fn world(&self) -> Option<&World> {
        self.world.as_deref()
    }

    /// Disconnects all devices, waiting at most a few seconds.
    pub fn shutdown(&mut self) {
        let trainer = self.trainer.take();
        let sensor = self.sensor.take();
        self.runtime.block_on(async {
            if let Some(trainer) = trainer {
                trainer.close(CLOSE_TIMEOUT).await;
            }
            if let Some(sensor) = sensor {
                sensor.close(CLOSE_TIMEOUT).await;
            }
        });
    }

    fn bluetooth(&self) -> Result<&Bluetooth, AppError> {
        self.bluetooth.as_ref().ok_or(AppError::UnknownDevice)
    }

    fn discovered_device(
        &self,
        index: usize,
        kind: DeviceKind,
    ) -> Result<DiscoveredDevice, AppError> {
        self.discovered
            .get(index)
            .filter(|d| d.kind == kind)
            .cloned()
            .ok_or(AppError::UnknownDevice)
    }

    fn poll_jobs(&mut self, events: &mut Vec<AppEvent>) {
        while let Ok(result) = self.jobs_rx.try_recv() {
            match result {
                JobResult::Scan(Ok((bluetooth, devices))) => {
                    let infos = devices
                        .iter()
                        .enumerate()
                        .map(|(index, d)| DeviceInfo {
                            index,
                            name: d.name.clone(),
                            kind: d.kind,
                            rssi: d.rssi,
                            remembered: self.is_remembered(d),
                        })
                        .collect();
                    self.bluetooth = Some(bluetooth);
                    self.discovered = devices;
                    events.push(AppEvent::DevicesFound(infos));
                    if std::mem::take(&mut self.reconnecting) {
                        let missing = self.connect_remembered();
                        if !missing.is_empty() {
                            events.push(AppEvent::RememberedMissing(missing));
                        }
                    }
                }
                JobResult::Route(load, _)
                | JobResult::Video(load, _)
                | JobResult::World(load, _)
                | JobResult::Progress(load, ..)
                    if load != self.load => {}
                JobResult::Route(_, Ok(imported)) => {
                    let map = imported.map.clone();
                    self.route_loaded(*imported, events);
                    if let WorldPlan::OnRequest(waiting) = &mut self.world_plan {
                        *waiting = Some(Box::new(map));
                    } else if let Some(route) = self.route.clone() {
                        self.generate_world(route, map);
                    }
                }
                JobResult::Video(_, Ok(loaded)) => {
                    let (imported, source) = *loaded;
                    match video::VideoCourse::new(&imported.route, &source) {
                        Ok(course) => {
                            self.video = Some(course);
                            self.view = View::Video;
                            let map = imported.map.clone();
                            self.route_loaded(imported, events);
                            // Ridden along the video, or in 3D once asked for (#44).
                            self.world_plan = WorldPlan::OnRequest(Some(Box::new(map)));
                            if !self.from_course
                                && let Err(error) = self.save_course()
                            {
                                warn!(%error, "cannot add the video course to the library");
                            }
                        }
                        Err(message) => events.push(AppEvent::Error(message)),
                    }
                }
                JobResult::Progress(_, stage, done, total) => {
                    events.push(AppEvent::LoadProgress { stage, done, total });
                }
                JobResult::World(_, world) => {
                    events.push(AppEvent::WorldReady {
                        chunks: world.chunks.len(),
                        fallback_samples: world.fallback_samples,
                    });
                    self.world = Some(Arc::from(world));
                    // A prepared GPX goes into the course library (R39).
                    if !self.from_course
                        && let Err(error) = self.save_course()
                    {
                        warn!(%error, "cannot add the course to the library");
                    }
                }
                JobResult::CourseAdded(Ok(path)) => events.push(AppEvent::CourseAdded(path)),
                JobResult::CourseSaved(Ok(path)) => {
                    self.saving = None;
                    self.course = Some(path.clone());
                    events.push(AppEvent::CourseAdded(path));
                }
                JobResult::CourseSaved(Err(message)) => {
                    self.saving = None;
                    events.push(AppEvent::Error(message));
                }
                JobResult::Scan(Err(message)) if std::mem::take(&mut self.reconnecting) => {
                    // No Bluetooth (or no permission): the rider sees it when scanning.
                    warn!(%message, "cannot reconnect the devices used last");
                }
                JobResult::Scan(Err(message))
                | JobResult::Route(_, Err(message))
                | JobResult::Video(_, Err(message))
                | JobResult::CourseAdded(Err(message))
                | JobResult::Failed(message) => {
                    events.push(AppEvent::Error(message));
                }
            }
        }
    }

    /// Takes a loaded route as the current one and reports it.
    fn route_loaded(&mut self, imported: Imported, events: &mut Vec<AppEvent>) {
        let Imported {
            route, name, gpx, ..
        } = imported;
        self.loaded = Some((name.clone(), gpx));
        events.push(AppEvent::RouteLoaded(RouteSummary {
            name,
            length: route.length().0,
            elevation_gain: route.elevation_gain().0,
            max_grade: route.max_grade().0,
            elevation_source: route.elevation_source(),
        }));
        self.route = Some(route);
    }

    fn poll_trainer(&mut self, events: &mut Vec<AppEvent>) {
        let Some(trainer) = &mut self.trainer else {
            return;
        };
        loop {
            match trainer.try_next_event() {
                Ok(Some(DeviceEvent::Connected)) => {
                    self.trainer_connected = Some(SystemTime::now());
                    if let Some(active) = &mut self.ride
                        && active.started.is_none()
                    {
                        active.started = Some(SystemTime::now());
                    }
                    events.push(AppEvent::Connected(trainer.name().to_owned()));
                }
                Ok(Some(DeviceEvent::Disconnected)) => {
                    self.trainer_connected = None;
                    if let Some(active) = &mut self.ride {
                        active.ride.on_power_source_lost();
                    }
                    events.push(AppEvent::Disconnected(trainer.name().to_owned()));
                }
                Ok(Some(DeviceEvent::Telemetry(telemetry))) => {
                    if let Some(active) = &mut self.ride {
                        active.ride.on_telemetry(&telemetry);
                    }
                }
                Ok(Some(DeviceEvent::Buttons(_))) => {}
                Ok(None) => break,
                Err(error) => {
                    events.push(AppEvent::Error(format!("trainer: {error}")));
                    self.trainer = None;
                    break;
                }
            }
        }
    }

    fn poll_sensor(&mut self, events: &mut Vec<AppEvent>) {
        let Some(sensor) = &mut self.sensor else {
            return;
        };
        loop {
            match sensor.try_next_event() {
                Ok(Some(DeviceEvent::Connected)) => {
                    events.push(AppEvent::Connected(sensor.name().to_owned()));
                }
                Ok(Some(DeviceEvent::Disconnected)) => {
                    events.push(AppEvent::Disconnected(sensor.name().to_owned()));
                }
                Ok(Some(DeviceEvent::Telemetry(telemetry))) => {
                    if let Some(active) = &mut self.ride {
                        active.ride.on_telemetry(&telemetry);
                    }
                }
                Ok(Some(DeviceEvent::Buttons(_))) => {}
                Ok(None) => break,
                Err(error) => {
                    events.push(AppEvent::Error(format!("heart rate: {error}")));
                    self.sensor = None;
                    break;
                }
            }
        }
    }
}

/// Single-precision points for course previews, which need no more precision.
#[allow(clippy::cast_possible_truncation)] // metres in a course fit f32 easily
fn thin(points: &[(f64, f64)]) -> Vec<[f32; 2]> {
    points.iter().map(|&(a, b)| [a as f32, b as f32]).collect()
}

/// The fastest time on `climb` among `records`.
fn best_climb_time(records: &[&RideRecord], climb: &ClimbTime) -> Option<Duration> {
    records
        .iter()
        .flat_map(|r| r.climbs.iter())
        .filter(|other| other.same_climb(climb))
        .map(|other| other.elapsed)
        .min()
}

/// The timed positions of a recorded activity: a GPX file with times, or a FIT file.
fn activity_points(path: &Path) -> Result<Vec<torqa_routes::TimedPoint>, String> {
    let is_fit = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("fit"));
    if is_fit {
        let (start, samples) = read_fit(path)?;
        let start = start
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0.0, |d| d.as_secs_f64());
        return Ok(samples
            .iter()
            .filter_map(|s| {
                let location = s.location?;
                Some(torqa_routes::TimedPoint {
                    lat: location.lat,
                    lon: location.lon,
                    time: start + s.elapsed.as_secs_f64(),
                })
            })
            .collect());
    }
    let xml = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let points = torqa_routes::timed_points(&xml).map_err(|e| e.to_string())?;
    if points.is_empty() {
        return Err(format!("{} has no times to race against", path.display()));
    }
    Ok(points)
}

fn read_fit(fit: &Path) -> Result<(SystemTime, Vec<Sample>), String> {
    let bytes = std::fs::read(fit).map_err(|e| format!("cannot read {}: {e}", fit.display()))?;
    torqa_storage::decode_fit(&bytes).map_err(|e| format!("{}: {e}", fit.display()))
}

/// The profile chosen last, else the first one, else a new default profile (saved, so it shows
/// up in the data directory to be edited).
fn initial_profile(data_dir: &Path) -> StoredProfile {
    let listed = profiles::list(data_dir);
    if let Some(found) = profiles::active(data_dir)
        .and_then(|id| listed.iter().find(|p| p.id == id).cloned())
        .or_else(|| listed.into_iter().next())
    {
        return found;
    }
    let profile = Profile::default();
    let id = profiles::new_id(data_dir, &profile.name);
    if let Err(error) = profiles::save(data_dir, &id, &profile) {
        warn!(%error, "cannot save the default profile");
    }
    StoredProfile { id, profile }
}

/// A free file name in `library` for a course called `name`.
/// How a course file refers to its video.
/// How the course file refers to its video. Its marks are kept along the track as recorded,
/// which the course is loaded along as a video course: placed on a route ridden in 3D, they
/// move on by its turns in place taken out.
fn video_reference(video: &video::VideoCourse, route: &Route) -> course::VideoReference {
    course::VideoReference {
        path: video.video.display().to_string(),
        file_name: video
            .video
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        size: std::fs::metadata(&video.video).map_or(0, |m| m.len()),
        offset_s: video.offset.as_secs_f64(),
        end_s: video.marks.last().map(|m| m.time.as_secs_f64()),
        marks: video
            .marks
            .iter()
            .map(|m| [route.recorded_distance(m.distance).0, m.time.as_secs_f64()])
            .collect(),
        located: video.located,
    }
}

/// The marks of a video course from its file, older files with a start and end only included.
fn video_marks(reference: &course::VideoReference) -> Vec<SyncMark> {
    let mark = |distance: f64, time: f64| SyncMark {
        distance: Meters(distance),
        time: Duration::try_from_secs_f64(time.max(0.0)).unwrap_or_default(),
    };
    if !reference.marks.is_empty() {
        return reference.marks.iter().map(|&[d, t]| mark(d, t)).collect();
    }
    // The route's end is placed when the course is paired with its route.
    reference.end_s.map_or_else(Vec::new, |end| {
        vec![mark(0.0, reference.offset_s), mark(0.0, end)]
    })
}

/// Changes the manifest of the course file at `path`.
fn update_manifest(path: &Path, change: impl FnOnce(&mut Manifest)) -> Result<(), AppError> {
    let storage = |e: course::CourseError| AppError::Storage(e.to_string());
    let mut manifest = course::read_manifest(path).map_err(storage)?;
    change(&mut manifest);
    course::rewrite_manifest(path, &manifest).map_err(storage)
}

fn unique_course_path(library: &Path, name: &str) -> PathBuf {
    let slug = torqa_storage::slug(name, "course");
    let mut path = library.join(format!("{slug}.{}", course::EXTENSION));
    let mut n = 1;
    while path.exists() {
        n += 1;
        path = library.join(format!("{slug}-{n}.{}", course::EXTENSION));
    }
    path
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use torqa_domain::units::{Rpm, Watts};
    use torqa_session::workout::RampTest;

    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("torqa-app-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A flat 400 m route with elevations, so no terrain download is needed.
    fn write_route(dir: &std::path::Path) -> PathBuf {
        let mut xml = String::from("<gpx><trk><name>Test loop</name><trkseg>");
        for i in 0..=40 {
            let lat = 46.0 + f64::from(i) * 10.0 / 111_195.0;
            let _ = write!(xml, r#"<trkpt lat="{lat}" lon="7"><ele>500</ele></trkpt>"#);
        }
        xml.push_str("</trkseg></trk></gpx>");
        let path = dir.join("route.gpx");
        std::fs::write(&path, xml).unwrap();
        path
    }

    /// Calls `update` like a 60 fps frame loop until `done` returns true (or 120 s pass).
    fn run_until(app: &mut App, mut done: impl FnMut(&AppEvent) -> bool) -> Vec<AppEvent> {
        // Generous wall-clock limit: an unoptimized world build under a parallel test run takes
        // well over 30 s on some machines; the limit only has to catch a hang.
        let deadline = std::time::Instant::now() + Duration::from_secs(120);
        let mut seen = Vec::new();
        while std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(16));
            for event in app.update(Duration::from_millis(16)) {
                let stop = done(&event);
                seen.push(event);
                if stop {
                    return seen;
                }
            }
        }
        panic!("timed out; events so far: {seen:?}");
    }

    #[tokio::test]
    async fn courses_ridden_in_3d_lose_their_turns_in_place_videos_keep_them() {
        // North 300 m, 30 m back and north again to 500 m: steps back and forth (#101).
        let dir = temp_dir("turns");
        let mut xml = String::from("<gpx><trk><trkseg>");
        let norths = (0..=30).chain((27..30).rev()).chain(28..=50);
        for north in norths {
            let lat = 46.0 + f64::from(north) * 10.0 / 111_195.0;
            let _ = write!(xml, r#"<trkpt lat="{lat}" lon="7"><ele>500</ele></trkpt>"#);
        }
        xml.push_str("</trkseg></trk></gpx>");
        let length = async |video| {
            import_gpx(
                xml.clone(),
                "zig-zag",
                (&dir, true),
                video,
                &UsedFiles::default(),
                &mut |_, _, _| {},
            )
            .await
            .unwrap()
            .route
            .length()
            .0
        };

        let (ridden, recorded) = (length(false).await, length(true).await);
        assert!((ridden - 500.0).abs() < 10.0, "ridden in 3D: {ridden} m");
        assert!(
            (recorded - 560.0).abs() < 10.0,
            "along a video: {recorded} m"
        );
    }

    #[test]
    fn loads_a_route_in_the_background() {
        let dir = temp_dir("load");
        let mut app = App::new(dir.join("data"), dir.join("cache")).unwrap();

        app.load_route(write_route(&dir), true);
        let events = run_until(&mut app, |e| matches!(e, AppEvent::RouteLoaded(_)));

        let Some(AppEvent::RouteLoaded(summary)) = events.last() else {
            unreachable!()
        };
        assert_eq!(summary.name, "Test loop");
        assert!((summary.length - 400.0).abs() < 1.0);
        assert!(app.route().is_some());
    }

    #[test]
    fn a_ride_starts_with_a_trainer_connected_before_it() {
        // As on the course page: the trainer connects, the world is built, then the ride
        // starts — the trainer reported itself long before.
        let dir = temp_dir("connected-first");
        let mut app = App::new(dir.join("data"), dir.join("cache")).unwrap();
        app.load_route(write_route(&dir), true);
        run_until(&mut app, |e| matches!(e, AppEvent::RouteLoaded(_)));
        app.connect_trainer(TrainerChoice::Fake(FakeRider {
            power: Watts(250.0),
            cadence: Rpm(90.0),
            heart: None,
        }))
        .unwrap();
        run_until(&mut app, |e| matches!(e, AppEvent::Connected(_)));

        app.start_ride(Percent(50.0), DescentMode::Coast, &GhostChoice::None)
            .unwrap();
        for _ in 0..120 {
            std::thread::sleep(Duration::from_millis(16));
            app.update(Duration::from_millis(16));
        }

        let state = app.ride_state().unwrap();
        assert!(state.distance.0 > 1.0, "rider should be moving: {state:?}");
        assert!(app.trainer_connected());
        app.shutdown();
    }

    #[test]
    fn a_workout_on_its_own_rides_a_flat_road_and_lands_in_the_history_by_its_name() {
        let dir = temp_dir("workout");
        let mut app = App::new(dir.join("data"), dir.join("cache")).unwrap();
        app.connect_trainer(TrainerChoice::Fake(FakeRider {
            power: Watts(150.0),
            cadence: Rpm(90.0),
            heart: None,
        }))
        .unwrap();
        run_until(&mut app, |e| matches!(e, AppEvent::Connected(_)));

        let workout = Workout::ConstantPower(Watts(220.0));
        app.start_workout(workout, false, DescentMode::Coast, "Constant power 220 W")
            .unwrap();
        for _ in 0..150 {
            std::thread::sleep(Duration::from_millis(16));
            app.update(Duration::from_millis(16));
        }

        let state = app.ride_state().unwrap();
        assert!(state.distance.0 > 1.0, "{state:?}");
        assert_eq!(state.position, None);
        assert_eq!(
            state.telemetry.power,
            Some(Watts(220.0)),
            "ERG holds the target"
        );
        app.change_workout(Workout::ConstantPower(Watts(240.0)));
        assert_eq!(
            app.ride_state()
                .unwrap()
                .workout
                .and_then(|w| w.target_power),
            Some(Watts(240.0))
        );
        let saved = app.finish_ride();
        assert!(
            matches!(saved.as_slice(), [AppEvent::RideSaved(_)]),
            "{saved:?}"
        );
        let history = app.history();
        assert_eq!(history[0].record.route, "Constant power 220 W");
        assert_eq!(
            history[0].record.route_key, None,
            "no records without a course"
        );
        app.shutdown();
    }

    #[test]
    fn a_workout_on_a_course_rides_along_it() {
        let dir = temp_dir("workout-course");
        let mut app = App::new(dir.join("data"), dir.join("cache")).unwrap();
        assert!(matches!(
            app.start_workout(
                Workout::ConstantPower(Watts(200.0)),
                true,
                DescentMode::Coast,
                "x"
            ),
            Err(AppError::NoTrainer)
        ));
        app.connect_trainer(TrainerChoice::Fake(FakeRider {
            power: Watts(150.0),
            cadence: Rpm(90.0),
            heart: None,
        }))
        .unwrap();
        assert!(matches!(
            app.start_workout(
                Workout::ConstantPower(Watts(200.0)),
                true,
                DescentMode::Coast,
                "x"
            ),
            Err(AppError::NoRoute)
        ));
        app.load_route(write_route(&dir), true);
        run_until(&mut app, |e| matches!(e, AppEvent::RouteLoaded(_)));

        app.start_workout(
            Workout::ConstantPower(Watts(200.0)),
            true,
            DescentMode::Coast,
            "x",
        )
        .unwrap();
        for _ in 0..150 {
            std::thread::sleep(Duration::from_millis(16));
            app.update(Duration::from_millis(16));
        }

        let state = app.ride_state().unwrap();
        assert!(state.position.is_some() && state.remaining.is_some());
        assert!(state.workout.is_some());
        assert!(!app.riding_along_video());
        app.finish_ride();
        assert_eq!(
            app.history()[0].record.route,
            "Test loop",
            "named after the course"
        );
        app.shutdown();
    }

    #[test]
    fn workouts_come_built_in_and_from_imported_files() {
        let dir = temp_dir("workout-library");
        let mut app = App::new(dir.join("data"), dir.join("cache")).unwrap();
        let file = dir.join("Tempo.erg");
        std::fs::write(
            &file,
            "[COURSE HEADER]\nMINUTES WATTS\n[END COURSE HEADER]\n\
             [COURSE DATA]\n0\t180\n20\t180\n[END COURSE DATA]\n",
        )
        .unwrap();
        let builtin = app.workouts().len();

        let id = app.import_workout(&file).unwrap();

        let library = app.workouts();
        assert_eq!(library.len(), builtin + 1);
        assert_eq!(library.last().unwrap().id, id);
        let Workout::Structured { plan, ftp } = app.structured_workout(&id).unwrap() else {
            panic!("a structured workout");
        };
        assert_eq!(
            (plan.name.as_str(), ftp),
            ("Tempo", app.profile().profile.ftp)
        );
        assert!(matches!(
            app.import_workout(&dir.join("missing.zwo")),
            Err(AppError::Workout(_))
        ));
    }

    #[test]
    fn an_ftp_test_ends_when_the_rider_gives_way_and_its_ftp_can_be_kept() {
        let dir = temp_dir("ftp-test");
        let mut app = App::new(dir.join("data"), dir.join("cache")).unwrap();
        app.connect_trainer(TrainerChoice::Fake(FakeRider {
            power: Watts(150.0),
            cadence: Rpm(90.0),
            heart: None,
        }))
        .unwrap();
        run_until(&mut app, |e| matches!(e, AppEvent::Connected(_)));
        let test = RampTest::for_ftp(app.profile().profile.ftp);
        app.start_workout(
            Workout::RampTest(test),
            false,
            DescentMode::Coast,
            "FTP test",
        )
        .unwrap();
        // Through the warm-up and a few steps at once; the fake rider holds every step.
        let active = app.ride.as_mut().unwrap();
        for _ in 0..(10 * 60 * 20) {
            active
                .ride
                .on_telemetry(&torqa_domain::telemetry::Telemetry {
                    power: Some(Watts(200.0)),
                    cadence: Some(Rpm(90.0)),
                    ..Default::default()
                });
            active.ride.tick(Duration::from_millis(50));
        }
        // Then the legs give way.
        for _ in 0..(15 * 20) {
            active
                .ride
                .on_telemetry(&torqa_domain::telemetry::Telemetry {
                    cadence: Some(Rpm(20.0)),
                    ..Default::default()
                });
            active.ride.tick(Duration::from_millis(50));
        }
        assert!(active.ride.is_finished(), "over once the rider gives way");

        let saved = app.finish_ride();

        assert!(
            matches!(saved.as_slice(), [AppEvent::RideSaved(_)]),
            "{saved:?}"
        );
        let test = &app.history()[0].record;
        assert_eq!(
            test.ftp_estimate,
            Some(Watts(150.0)),
            "75 % of the best minute at 200 W"
        );
        app.set_ftp(Watts(150.0)).unwrap();
        assert_eq!(app.profile().profile.ftp, Watts(150.0));
        app.shutdown();
    }

    #[test]
    fn a_single_cog_rides_virtual_gears_shifted_from_the_keyboard() {
        let dir = temp_dir("gears");
        let mut app = App::new(dir.join("data"), dir.join("cache")).unwrap();
        let mut profile = app.profile().profile.clone();
        profile.drivetrain = Drivetrain::SingleCog {
            chainring: 50,
            cog: 14,
        };
        let id = app.profile().id.clone();
        app.save_profile(Some(&id), profile).unwrap();
        app.load_route(write_route(&dir), true);
        run_until(&mut app, |e| matches!(e, AppEvent::RouteLoaded(_)));
        app.connect_trainer(TrainerChoice::Fake(FakeRider {
            power: Watts(200.0),
            cadence: Rpm(90.0),
            heart: None,
        }))
        .unwrap();
        app.start_ride(Percent(50.0), DescentMode::Coast, &GhostChoice::None)
            .unwrap();
        let before = app.ride_state().unwrap().gear.unwrap().number;

        app.shift(Shift::Up);
        app.update(Duration::from_millis(16));

        assert_eq!(app.ride_state().unwrap().gear.unwrap().number, before + 1);
        app.shutdown();
    }

    /// A shifter that shifts as told and is connected or not.
    struct Shifter {
        shifts: Vec<Shift>,
        connected: bool,
    }

    impl ShiftInput for Shifter {
        fn name(&self) -> &'static str {
            "RDR9250"
        }

        fn poll(&mut self) -> Vec<Shift> {
            std::mem::take(&mut self.shifts)
        }

        fn connected(&self) -> bool {
            self.connected
        }
    }

    #[test]
    fn a_shifters_shifts_reach_the_ride_and_its_connection_is_told() {
        let dir = temp_dir("shifter");
        let mut app = App::new(dir.join("data"), dir.join("cache")).unwrap();
        let mut profile = app.profile().profile.clone();
        profile.drivetrain = Drivetrain::SingleCog {
            chainring: 50,
            cog: 14,
        };
        let id = app.profile().id.clone();
        app.save_profile(Some(&id), profile).unwrap();
        app.load_route(write_route(&dir), true);
        run_until(&mut app, |e| matches!(e, AppEvent::RouteLoaded(_)));
        app.connect_trainer(TrainerChoice::Fake(FakeRider {
            power: Watts(200.0),
            cadence: Rpm(90.0),
            heart: None,
        }))
        .unwrap();
        app.start_ride(Percent(50.0), DescentMode::Coast, &GhostChoice::None)
            .unwrap();
        let before = app.ride_state().unwrap().gear.unwrap().number;

        app.add_shift_input(Box::new(Shifter {
            shifts: vec![Shift::Up, Shift::Up, Shift::Down, Shift::Up],
            connected: true,
        }));
        let events = app.update(Duration::from_millis(16));

        assert!(
            events.contains(&AppEvent::Connected("RDR9250".to_owned())),
            "{events:?}"
        );
        assert_eq!(app.ride_state().unwrap().gear.unwrap().number, before + 2);
        app.set_shift_channels(3, 4).unwrap();
        assert_eq!(app.shift_channels(), (3, 4));
        app.shutdown();
    }

    #[test]
    fn simulated_rides_speed_up_and_jump_but_set_no_records() {
        let dir = temp_dir("simulation");
        let mut app = App::new(dir.join("data"), dir.join("cache")).unwrap();
        app.load_route(write_climb_route(&dir), true);
        run_until(&mut app, |e| matches!(e, AppEvent::RouteLoaded(_)));
        assert!(!app.simulating());
        assert_eq!(app.set_time_scale(10.0), 1.0, "nothing to speed up");
        app.connect_trainer(TrainerChoice::Fake(FakeRider {
            power: Watts(300.0),
            cadence: Rpm(90.0),
            heart: None,
        }))
        .unwrap();
        assert!(app.simulating());
        app.start_ride(Percent(50.0), DescentMode::Coast, &GhostChoice::None)
            .unwrap();
        run_until(&mut app, |e| matches!(e, AppEvent::Connected(_)));
        assert_eq!(app.set_time_scale(100.0), 20.0);
        assert_eq!(app.set_time_scale(10.0), 10.0);

        // One real second at 10×: ten seconds of riding, sampled every ride second.
        for _ in 0..60 {
            std::thread::sleep(Duration::from_millis(16));
            app.update(Duration::from_millis(16));
        }
        let state = app.ride_state().unwrap();
        assert!(state.elapsed > Duration::from_secs(8), "{state:?}");
        let samples = app.ride.as_ref().unwrap().ride.samples().len();
        let seconds = usize::try_from(state.elapsed.as_secs()).unwrap();
        assert!(
            samples.abs_diff(seconds) <= 1,
            "{samples} samples in {seconds} s"
        );

        // Jumps: by distance, and to the route's point nearest a spot on the map.
        assert!(app.jump_to(Meters(150.0)));
        assert!((app.ride_state().unwrap().distance.0 - 150.0).abs() < 1.0);
        let route = app.route().unwrap().clone();
        let target = route.position(Meters(80.0));
        let (x, y) = LocalProjection::for_route(&route).project(target.lat, target.lon);
        assert!(app.jump_near(x + 3.0, y - 2.0));
        assert!((app.ride_state().unwrap().distance.0 - 80.0).abs() < 12.0);

        let saved = app.finish_ride();
        let Some(AppEvent::RideSaved(fit)) = saved.first() else {
            panic!("{saved:?}")
        };
        let record = &app.history()[0].record;
        assert_eq!(app.history()[0].fit, *fit);
        assert_eq!(record.route_key, None, "a simulated ride sets no records");
        assert_eq!(record.climbs.len(), 0);
        app.shutdown();
    }

    #[test]
    fn rides_with_the_fake_trainer_and_saves_a_fit_file() {
        let dir = temp_dir("ride");
        let mut app = App::new(dir.join("data"), dir.join("cache")).unwrap();
        app.load_route(write_route(&dir), true);
        run_until(&mut app, |e| matches!(e, AppEvent::RouteLoaded(_)));

        app.connect_trainer(TrainerChoice::Fake(FakeRider {
            power: Watts(250.0),
            cadence: Rpm(90.0),
            heart: None,
        }))
        .unwrap();
        app.start_ride(Percent(50.0), DescentMode::Coast, &GhostChoice::None)
            .unwrap();
        run_until(&mut app, |e| matches!(e, AppEvent::Connected(_)));
        // Ride three seconds of frames.
        for _ in 0..180 {
            std::thread::sleep(Duration::from_millis(16));
            app.update(Duration::from_millis(16));
        }
        let state = app.ride_state().unwrap();
        assert!(state.distance.0 > 1.0, "rider should be moving: {state:?}");
        assert_eq!(state.telemetry.power, Some(Watts(250.0)));
        let values = app.hud_values();
        assert_eq!(values.len(), hud::METRICS.len());
        let value = |id: &str| values.iter().find(|(m, _)| *m == id).and_then(|(_, v)| *v);
        assert_eq!(value("power_3s"), Some(250.0));
        assert_eq!(value("heart_rate"), None);
        assert!(value("distance").is_some_and(|km| km > 0.0));

        app.adjust_ride(Percent(80.0), DescentMode::Flat);
        let events = app.finish_ride();
        let Some(AppEvent::RideSaved(path)) = events.first() else {
            panic!("not saved: {events:?}");
        };
        assert!(path.starts_with(dir.join("data/profiles/rider/rides")));
        assert!(std::fs::metadata(path).unwrap().len() > 100);

        let history = app.history();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].fit, *path);
        assert_eq!(history[0].record.route, "Test loop");
        assert!(history[0].record.summary.distance.0 > 1.0);
        let detail = app.ride_detail(path).unwrap();
        assert!(detail.samples.len() > 1);
        assert!(detail.power_zones.iter().sum::<Duration>() > Duration::ZERO);

        // Without metadata, e.g. a FIT file copied in by hand, the history rebuilds it.
        std::fs::remove_file(rides::metadata_path(path)).unwrap();
        assert_eq!(app.history().len(), 1);
        assert!(rides::metadata_path(path).exists());

        app.rename_ride(path, "  Lunch spin ").unwrap();
        assert_eq!(app.history()[0].record.name.as_deref(), Some("Lunch spin"));
        app.rename_ride(path, "").unwrap();
        assert_eq!(app.history()[0].record.name, None);

        app.delete_ride(path).unwrap();
        assert_eq!(app.history().len(), 0);
        app.shutdown();
    }

    #[test]
    fn a_saved_course_rides_on_another_machine() {
        let dir = temp_dir("course");
        let mut prepared = App::new(dir.join("a/data"), dir.join("a/cache")).unwrap();
        prepared.load_route(write_route(&dir), true);
        assert!(matches!(
            prepared.save_course(),
            Err(AppError::CourseNotReady)
        ));
        run_until(&mut prepared, |e| matches!(e, AppEvent::WorldReady { .. }));

        prepared.save_course().unwrap();
        let events = run_until(&mut prepared, |e| matches!(e, AppEvent::CourseAdded(_)));
        let Some(AppEvent::CourseAdded(file)) = events.last() else {
            unreachable!()
        };
        assert_eq!(*file, dir.join("a/data/courses/test-loop.tqc"));
        let listed = prepared.courses();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].manifest.name, "Test loop");
        assert!((listed[0].manifest.length_m - 400.0).abs() < 1.0);
        // Prepared once, listed once — the import added it, saving again found it — and with a
        // preview for its card.
        assert!(listed[0].manifest.track.len() > 2);
        assert!(listed[0].manifest.profile.len() > 2);

        let mut other = App::new(dir.join("b/data"), dir.join("b/cache")).unwrap();
        other.import_course(file.clone());
        let events = run_until(&mut other, |e| matches!(e, AppEvent::CourseAdded(_)));
        let Some(AppEvent::CourseAdded(imported)) = events.last() else {
            unreachable!()
        };
        assert!(imported.starts_with(dir.join("b/data/courses")));
        other.open_course(imported.clone());
        let events = run_until(&mut other, |e| matches!(e, AppEvent::RouteLoaded(_)));

        let Some(AppEvent::RouteLoaded(summary)) = events.last() else {
            unreachable!()
        };
        assert!(summary.name == "Test loop" && (summary.length - 400.0).abs() < 1.0);
        assert_eq!(other.loaded_course(), Some(imported.as_path()));
        // Looking at a course does not build its world; riding it does.
        for _ in 0..30 {
            std::thread::sleep(Duration::from_millis(16));
            other.update(Duration::from_millis(16));
        }
        assert!(other.world().is_none());
        assert!(!other.build_world());
        run_until(&mut other, |e| matches!(e, AppEvent::WorldReady { .. }));
        assert!(other.world().is_some());
        assert!(other.build_world());
    }

    #[test]
    fn opening_another_course_drops_the_one_still_loading() {
        let dir = temp_dir("switch");
        let mut app = App::new(dir.join("data"), dir.join("cache")).unwrap();
        app.load_route(write_route(&dir), true);
        let events = run_until(&mut app, |e| matches!(e, AppEvent::CourseAdded(_)));
        let Some(AppEvent::CourseAdded(course)) = events.last() else {
            unreachable!()
        };
        assert_eq!(app.loaded_course(), Some(course.as_path()));

        // Back and forth quickly, as when leaving and re-entering a course page.
        app.open_course(course.clone());
        app.build_world();
        app.open_course(course.clone());
        let events = run_until(&mut app, |e| matches!(e, AppEvent::RouteLoaded(_)));
        for _ in 0..60 {
            std::thread::sleep(Duration::from_millis(16));
            app.update(Duration::from_millis(16));
        }

        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, AppEvent::RouteLoaded(_)))
                .count(),
            1,
            "{events:?}"
        );
        assert!(app.world().is_none(), "the abandoned load built its world");
    }

    #[test]
    fn courses_can_be_renamed_and_deleted() {
        let dir = temp_dir("rename-course");
        let mut app = App::new(dir.join("data"), dir.join("cache")).unwrap();
        app.load_route(write_route(&dir), true);
        let events = run_until(&mut app, |e| matches!(e, AppEvent::CourseAdded(_)));
        let Some(AppEvent::CourseAdded(course)) = events.last() else {
            unreachable!()
        };

        app.rename_course(course, "  Evening loop ").unwrap();
        assert!(app.rename_course(course, "  ").is_err());
        assert_eq!(app.courses()[0].manifest.name, "Evening loop");
        app.open_course(course.clone());
        let events = run_until(&mut app, |e| matches!(e, AppEvent::RouteLoaded(_)));
        assert!(
            matches!(events.last(), Some(AppEvent::RouteLoaded(s)) if s.name == "Evening loop")
        );

        app.delete_course(course).unwrap();
        assert_eq!(app.courses().len(), 0);
        assert_eq!(app.loaded_course(), None);
    }

    /// A copy of a generated test video in `dir`, so each test owns its files.
    fn video_in(dir: &Path, generated: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::rename(generated, &path)
            .or_else(|_| std::fs::copy(generated, &path).map(|_| ()))
            .unwrap();
        path
    }

    fn loaded_video(app: &mut App, path: PathBuf) -> Vec<AppEvent> {
        app.load_video(path, true);
        run_until(app, |e| {
            matches!(e, AppEvent::CourseAdded(_) | AppEvent::Error(_))
        })
    }

    #[test]
    fn a_gopro_video_with_gps_becomes_a_course_that_follows_the_rider() {
        let dir = temp_dir("gopro");
        let video = video_in(&dir, &torqa_video::testing::gopro_video("app"), "Ride.MOV");
        let mut app = App::new(dir.join("data"), dir.join("cache")).unwrap();

        let events = loaded_video(&mut app, video.clone());

        assert!(
            events
                .iter()
                .any(|e| matches!(e, AppEvent::RouteLoaded(s) if s.name == "Ride")),
            "{events:?}"
        );
        // Ridden along the video unless the rider asks for 3D: no world built for it.
        assert!(app.world().is_none());
        let course = app.video().unwrap();
        assert_eq!(course.video, video);
        // 10 m every quarter second: the video is 2.5 s in after 100 m.
        let at = |m: f64| course.time_at(Meters(m)).as_secs_f64();
        assert!(at(0.0) < 0.05, "{}", at(0.0));
        assert!((at(100.0) - 2.5).abs() < 0.1, "{}", at(100.0));
        assert!(at(10_000.0) <= course.duration.as_secs_f64());
        let listed = app.courses();
        assert_eq!(listed.len(), 1);
        let reference = listed[0].manifest.video.as_ref().unwrap();
        assert_eq!(reference.file_name, "Ride.MOV");
    }

    #[test]
    fn the_video_plays_as_far_as_the_rider_has_come() {
        let dir = temp_dir("video-ride");
        let video = video_in(&dir, &torqa_video::testing::gopro_video("ride"), "Ride.MOV");
        let mut app = App::new(dir.join("data"), dir.join("cache")).unwrap();
        loaded_video(&mut app, video);
        app.connect_trainer(TrainerChoice::Fake(FakeRider {
            power: Watts(300.0),
            cadence: Rpm(90.0),
            heart: None,
        }))
        .unwrap();
        app.start_ride(Percent(50.0), DescentMode::Coast, &GhostChoice::None)
            .unwrap();

        let mut frames: Vec<(Duration, Duration)> = Vec::new();
        for _ in 0..240 {
            std::thread::sleep(Duration::from_millis(16));
            app.update(Duration::from_millis(16));
            let shown = app.video_time().unwrap();
            if let Some(frame) = app.video_frame() {
                assert_eq!((frame.width, frame.height), (64, 48));
                frames.push((shown, frame.time));
            }
        }

        let distance = app.ride_state().unwrap().distance;
        assert!(distance.0 > 5.0, "rider should be moving: {distance:?}");
        assert!(frames.len() >= 2, "{frames:?}");
        assert!(frames.windows(2).all(|w| w[1].1 > w[0].1), "{frames:?}");
        // Each frame is the one coming up next, to blend towards (decoding may lag a little).
        let (shown, last) = frames[frames.len() - 1];
        assert!(last + Duration::from_millis(150) > shown, "{frames:?}");
        app.abort_ride();
        assert!(app.video_frame().is_none());
    }

    #[test]
    fn video_courses_find_their_video_on_another_machine() {
        let dir = temp_dir("video-course");
        let video = video_in(
            &dir,
            &torqa_video::testing::gopro_video("moved"),
            "Gurten.mov",
        );
        let mut prepared = App::new(dir.join("a/data"), dir.join("a/cache")).unwrap();
        let events = loaded_video(&mut prepared, video.clone());
        let Some(AppEvent::CourseAdded(file)) = events.last() else {
            panic!("{events:?}")
        };

        // Another machine: the course elsewhere, the video not where it was recorded.
        let shared = dir.join("shared");
        let aside = dir.join("aside");
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::create_dir_all(&aside).unwrap();
        std::fs::copy(file, shared.join("gurten.tqc")).unwrap();
        std::fs::rename(&video, aside.join("Gurten.mov")).unwrap();
        let mut other = App::new(dir.join("b/data"), dir.join("b/cache")).unwrap();
        other.open_course(shared.join("gurten.tqc"));
        let events = run_until(&mut other, |e| matches!(e, AppEvent::Error(_)));
        assert!(
            matches!(events.last(), Some(AppEvent::Error(m)) if m.contains("Gurten.mov")),
            "{events:?}"
        );

        std::fs::rename(aside.join("Gurten.mov"), shared.join("Gurten.mov")).unwrap();
        other.open_course(shared.join("gurten.tqc"));
        run_until(&mut other, |e| matches!(e, AppEvent::RouteLoaded(_)));
        assert_eq!(other.video().unwrap().video, shared.join("Gurten.mov"));
        // Opened, not prepared: nothing is added to that machine's library.
        assert_eq!(other.courses().len(), 0);
    }

    #[test]
    fn an_incyclist_route_video_starts_at_its_start_frame() {
        let dir = temp_dir("incyclist");
        video_in(
            &dir,
            &torqa_video::testing::test_video("incyclist", 64, 48),
            "climb.mp4",
        );
        // 200 m north, 10 m every 0.1 s, from frame 11 at 10 fps: 1 s into the video.
        let mut gpx = String::from("<gpx><trk><trkseg>");
        for i in 0..=20 {
            let lat = 46.0 + f64::from(i) * 10.0 / 111_195.0;
            let _ = write!(
                gpx,
                r#"<trkpt lat="{lat}" lon="7"><ele>500</ele><time>2024-06-01T08:00:{:06.3}Z</time></trkpt>"#,
                f64::from(i) / 10.0
            );
        }
        gpx.push_str("</trkseg></trk></gpx>");
        std::fs::write(dir.join("climb.gpx"), gpx).unwrap();
        std::fs::write(
            dir.join("climb.xml"),
            "<gpx-import><title>Col &amp; Climb</title><video-file-path>climb.mp4</video-file-path>\
             <gpx-file-path>climb.gpx</gpx-file-path><framerate>10</framerate>\
             <start-frame>11</start-frame></gpx-import>",
        )
        .unwrap();
        let mut app = App::new(dir.join("data"), dir.join("cache")).unwrap();

        let events = loaded_video(&mut app, dir.join("climb.xml"));

        assert!(
            events
                .iter()
                .any(|e| matches!(e, AppEvent::RouteLoaded(s) if s.name == "Col & Climb")),
            "{events:?}"
        );
        let course = app.video().unwrap();
        assert_eq!(course.offset, Duration::from_secs(1));
        assert!((course.time_at(Meters(0.0)).as_secs_f64() - 1.0).abs() < 0.05);
        assert!((course.time_at(Meters(40.0)).as_secs_f64() - 1.4).abs() < 0.1);
    }

    /// Sync marks from `(distance m, video s)` pairs.
    fn marks(pairs: &[(f64, f64)]) -> Vec<SyncMark> {
        pairs
            .iter()
            .map(|&(d, t)| SyncMark {
                distance: Meters(d),
                time: Duration::from_secs_f64(t),
            })
            .collect()
    }

    /// 200 m north, 10 m per point, with timestamps of another day's recording.
    fn write_timed_route(dir: &Path) -> PathBuf {
        let mut gpx = String::from("<gpx><trk><trkseg>");
        for i in 0..=20 {
            let lat = 46.0 + f64::from(i) * 10.0 / 111_195.0;
            let _ = write!(
                gpx,
                r#"<trkpt lat="{lat}" lon="7"><ele>500</ele><time>2024-06-01T08:{i:02}:00Z</time></trkpt>"#
            );
        }
        gpx.push_str("</trkseg></trk></gpx>");
        let path = dir.join("commute.gpx");
        std::fs::write(&path, gpx).unwrap();
        path
    }

    #[test]
    fn a_video_without_gps_is_added_to_a_gpx_course_by_its_start_and_end() {
        let dir = temp_dir("aligned");
        let video = video_in(
            &dir,
            &torqa_video::testing::test_video("aligned", 64, 48),
            "Commute.mp4",
        );
        let probe = video::probe(&video).unwrap();
        assert!(!probe.has_gps);
        assert!((probe.duration.as_secs_f64() - 4.0).abs() < 0.15);
        let mut app = App::new(dir.join("a/data"), dir.join("a/cache")).unwrap();
        // Only a course can take a video.
        assert!(
            app.add_video(&video, &marks(&[(0.0, 1.0), (1.0, 3.0)]))
                .is_err()
        );
        // The GPX course, as imported on the Courses tab and opened on its page.
        app.load_route(write_timed_route(&dir), true);
        let events = run_until(&mut app, |e| matches!(e, AppEvent::CourseAdded(_)));
        let Some(AppEvent::CourseAdded(file)) = events.last() else {
            unreachable!()
        };
        app.open_course(file.clone());
        run_until(&mut app, |e| matches!(e, AppEvent::RouteLoaded(_)));

        // Start and end only; the end mark is put at the route's end whatever its distance.
        app.add_video(&video, &marks(&[(0.0, 1.0), (0.0, 3.0)]))
            .unwrap();

        let length = app.route().unwrap().length().0;
        let at = |app: &App, m: f64| app.video().unwrap().time_at(Meters(m)).as_secs_f64();
        // Evenly between the marks — the GPX's own minutes apart are ignored.
        assert!((at(&app, 0.0) - 1.0).abs() < 0.01);
        assert!((at(&app, length / 2.0) - 2.0).abs() < 0.01);
        assert!((at(&app, length) - 3.0).abs() < 0.01);
        assert!(app.video().unwrap().aligned_by_hand());
        assert!(
            app.add_video(&video, &marks(&[(0.0, 0.0), (0.0, 1.0)]))
                .is_err()
        );

        // Moved later, with a point in between where the footage slows down: a quarter of the
        // route takes two seconds of video, the rest one.
        let quarter = length / 4.0;
        app.align_video(&marks(&[(0.0, 0.5), (quarter, 2.5), (length, 3.5)]))
            .unwrap();
        assert!((at(&app, 0.0) - 0.5).abs() < 0.01);
        assert!((at(&app, quarter / 2.0) - 1.5).abs() < 0.01);
        assert!((at(&app, quarter) - 2.5).abs() < 0.01);
        assert!((at(&app, quarter + (length - quarter) / 2.0) - 3.0).abs() < 0.01);
        // Points must follow each other on the route and in the video, within it.
        for wrong in [
            vec![(0.0, 3.0), (quarter, 2.5), (length, 3.5)],
            vec![
                (0.0, 0.5),
                (quarter, 2.0),
                (quarter / 2.0, 2.5),
                (length, 3.5),
            ],
            vec![(0.0, 0.5), (length + 10.0, 2.5), (length, 3.5)],
            vec![(0.0, 1.0), (quarter, 2.0), (length, 60.0)],
        ] {
            assert!(app.align_video(&marks(&wrong)).is_err(), "{wrong:?}");
        }
        let reference = course::read_manifest(file).unwrap().video.unwrap();
        assert!((reference.offset_s - 0.5).abs() < 1e-9);
        assert_eq!(reference.end_s, Some(3.5));
        assert_eq!(reference.marks.len(), 3);

        let mut other = App::new(dir.join("b/data"), dir.join("b/cache")).unwrap();
        other.open_course(file.clone());
        run_until(&mut other, |e| matches!(e, AppEvent::RouteLoaded(_)));
        assert!((at(&other, 0.0) - 0.5).abs() < 0.01);
        assert!((at(&other, quarter) - 2.5).abs() < 0.01);
        assert!((at(&other, length) - 3.5).abs() < 0.01);

        // Course files with only a start and an end (before points in between) still open.
        update_manifest(file, |m| {
            let video = m.video.as_mut().unwrap();
            video.marks.clear();
            video.end_s = Some(3.0);
        })
        .unwrap();
        other.open_course(file.clone());
        run_until(&mut other, |e| matches!(e, AppEvent::RouteLoaded(_)));
        assert!((at(&other, length / 2.0) - 1.75).abs() < 0.01);

        // Removed again: a 3D course, as before.
        app.remove_video().unwrap();
        assert!(app.video().is_none());
        assert!(course::read_manifest(file).unwrap().video.is_none());
        assert!(!app.build_world());
        run_until(&mut app, |e| matches!(e, AppEvent::WorldReady { .. }));
    }

    #[test]
    fn the_videos_sound_plays_along_at_its_pitch_and_can_be_switched_off() {
        let dir = temp_dir("sound");
        let video = video_in(&dir, &torqa_video::testing::sound_video("app"), "Ride.mov");
        let mut app = App::new(dir.join("data"), dir.join("cache")).unwrap();
        app.load_route(write_route(&dir), true);
        let events = run_until(&mut app, |e| matches!(e, AppEvent::CourseAdded(_)));
        let Some(AppEvent::CourseAdded(file)) = events.last() else {
            unreachable!()
        };
        app.open_course(file.clone());
        run_until(&mut app, |e| matches!(e, AppEvent::RouteLoaded(_)));
        // Most of the video on the first 40 m: at 6 m/s it plays at about half its speed.
        app.add_video(&video, &marks(&[(0.0, 0.0), (40.0, 3.6), (400.0, 3.95)]))
            .unwrap();
        app.connect_trainer(TrainerChoice::Fake(FakeRider {
            power: Watts(400.0),
            cadence: Rpm(90.0),
            heart: None,
        }))
        .unwrap();
        app.start_ride(Percent(0.0), DescentMode::Coast, &GhostChoice::None)
            .unwrap();
        assert_eq!(app.video_sound_rate(), Some(48_000));

        let ride = |app: &mut App, frames: usize| {
            let mut heard = Vec::new();
            for _ in 0..frames {
                std::thread::sleep(Duration::from_millis(16));
                app.update(Duration::from_millis(16));
                heard.extend(app.video_sound(4_096));
            }
            heard
        };
        let heard = ride(&mut app, 240);

        // Sound keeps up with real time once it runs (≈ 48 000 samples a second).
        assert!(heard.len() > 48_000 * 3, "{} samples", heard.len());
        let last: Vec<f32> = heard[heard.len() - 9_600..].iter().map(|s| s[0]).collect();
        let loudness = last.iter().map(|v| v.abs()).fold(0.0_f32, f32::max);
        assert!(loudness > 0.05, "silent while riding: {loudness}");
        #[allow(clippy::cast_precision_loss)]
        let hertz = last
            .windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count() as f32
            / 2.0
            / 0.2;
        assert!((hertz - 440.0).abs() < 30.0, "pitch {hertz} Hz");

        app.set_video_sound(false);
        let heard = ride(&mut app, 60);
        let tail = &heard[heard.len().saturating_sub(4_800)..];
        assert!(
            tail.iter().flatten().all(|v| v.abs() < 1e-4),
            "still audible"
        );
        app.abort_ride();
        assert_eq!(app.video_sound_rate(), None);
    }

    #[test]
    fn a_video_showing_no_picture_is_reported_once() {
        let dir = temp_dir("no-picture");
        let video = video_in(
            &dir,
            &torqa_video::testing::gopro_video("no-picture"),
            "Ride.MOV",
        );
        let mut app = App::new(dir.join("data"), dir.join("cache")).unwrap();
        loaded_video(&mut app, video);
        app.connect_trainer(TrainerChoice::Fake(FakeRider {
            power: Watts(250.0),
            cadence: Rpm(90.0),
            heart: None,
        }))
        .unwrap();
        let ride = |app: &mut App, take_frames: bool| {
            app.start_ride(Percent(50.0), DescentMode::Coast, &GhostChoice::None)
                .unwrap();
            let mut problems = Vec::new();
            for _ in 0..360 {
                std::thread::sleep(Duration::from_millis(16));
                for event in app.update(Duration::from_millis(16)) {
                    if let AppEvent::Error(message) = event {
                        problems.push(message);
                    }
                }
                if take_frames {
                    let _ = app.video_frame();
                }
            }
            app.abort_ride();
            problems
        };

        // Shown as it should: nothing to report.
        assert_eq!(ride(&mut app, true), Vec::<String>::new());
        // Never shown (as on a front end that cannot display it): said once, not every frame.
        let problems = ride(&mut app, false);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("no picture"), "{problems:?}");
    }

    #[test]
    fn a_video_course_is_ridden_along_its_video_or_in_3d() {
        let dir = temp_dir("video-or-3d");
        let video = video_in(
            &dir,
            &torqa_video::testing::gopro_video("or-3d"),
            "Ride.MOV",
        );
        let mut app = App::new(dir.join("data"), dir.join("cache")).unwrap();
        loaded_video(&mut app, video);
        app.connect_trainer(TrainerChoice::Fake(FakeRider {
            power: Watts(250.0),
            cadence: Rpm(90.0),
            heart: None,
        }))
        .unwrap();
        let start = |app: &mut App| {
            app.start_ride(Percent(50.0), DescentMode::Coast, &GhostChoice::None)
                .unwrap();
        };

        // In 3D: the route's world is built, the video stays off.
        app.ride_along_video(false);
        assert!(!app.build_world());
        run_until(&mut app, |e| matches!(e, AppEvent::WorldReady { .. }));
        start(&mut app);
        assert!(!app.riding_along_video());
        assert!(app.video_frame().is_none());
        app.abort_ride();

        // Along the video, as before.
        app.ride_along_video(true);
        start(&mut app);
        assert!(app.riding_along_video());
        app.abort_ride();

        // Without its video, a course is ridden in 3D only.
        app.remove_video().unwrap();
        app.ride_along_video(true);
        start(&mut app);
        assert!(!app.riding_along_video());
    }

    #[test]
    fn a_tacx_real_life_video_rides_along_its_video_by_distance_and_slope() {
        use torqa_video::testing::{pgmf_bytes, rlv_bytes, test_video};

        let dir = temp_dir("rlv");
        video_in(&dir, &test_video("rlv", 64, 48), "stelvio.mp4");
        std::fs::write(
            dir.join("Stelvio.rlv"),
            rlv_bytes(r"C:\Tacx\Videos\STELVIO.MP4"),
        )
        .unwrap();
        std::fs::write(dir.join("Stelvio.PGMF"), pgmf_bytes("Passo dello Stelvio")).unwrap();
        assert_eq!(
            App::suggested_course_name(&dir.join("Stelvio.rlv")),
            "Stelvio"
        );
        let mut app = App::new(dir.join("data"), dir.join("cache")).unwrap();

        app.load_video(dir.join("Stelvio.rlv"), false);
        let file = added(&mut app);

        let route = app.route().unwrap();
        assert!(
            (route.length().0 - 30.0).abs() < 0.5,
            "{}",
            route.length().0
        );
        // The elevations are the course's own (from 500 m, up then down), not terrain data's
        // from wherever its drawn line happens to lie.
        assert_eq!(route.elevation_source(), ElevationSource::File);
        assert!(route.elevation_gain().0 > 0.3);
        let course = app.video().unwrap();
        assert!(!course.located);
        let at = |m: f64| course.time_at(Meters(m)).as_secs_f64();
        // 1 m per frame for 20 m, then 0.5 m: 2 s at 20 m, 3 s at 25 m.
        assert!((at(20.0) - 2.0).abs() < 0.05, "{}", at(20.0));
        assert!((at(25.0) - 3.0).abs() < 0.05, "{}", at(25.0));
        // The PGMF's 17-character name field cut "Passo dello Stelvio" short: the RLV's
        // file name is used instead.
        assert_eq!(course::read_manifest(&file).unwrap().name, "Stelvio");

        // Only along its video: it has no place for a 3D world, and keeps its video.
        app.ride_along_video(false);
        app.connect_trainer(TrainerChoice::Fake(FakeRider {
            power: Watts(200.0),
            cadence: Rpm(90.0),
            heart: None,
        }))
        .unwrap();
        app.start_ride(Percent(50.0), DescentMode::Coast, &GhostChoice::None)
            .unwrap();
        assert!(app.riding_along_video());
        app.abort_ride();
        assert!(app.remove_video().is_err());

        // Opened again from its file: still a course without a place.
        app.open_course(file);
        run_until(&mut app, |e| matches!(e, AppEvent::RouteLoaded(_)));
        assert!(!app.video().unwrap().located);
    }

    #[test]
    fn videos_with_gps_follow_it_and_cannot_be_moved() {
        let dir = temp_dir("gps-align");
        let video = video_in(
            &dir,
            &torqa_video::testing::gopro_video("align"),
            "Ride.MOV",
        );
        assert!(video::probe(&video).unwrap().has_gps);
        let mut app = App::new(dir.join("data"), dir.join("cache")).unwrap();
        loaded_video(&mut app, video);

        assert!(!app.video().unwrap().aligned_by_hand());
        assert!(app.align_video(&marks(&[(0.0, 0.0), (0.0, 2.0)])).is_err());
    }

    #[test]
    fn previews_show_the_frame_at_a_moment() {
        let dir = temp_dir("preview");
        let video = video_in(
            &dir,
            &torqa_video::testing::test_video("preview", 64, 48),
            "a.mp4",
        );
        let mut app = App::new(dir.join("data"), dir.join("cache")).unwrap();

        let frame = app
            .video_preview(&video, Duration::from_millis(2050))
            .unwrap();
        assert_eq!(frame.time, Duration::from_secs(2));
        let earlier = app
            .video_preview(&video, Duration::from_millis(500))
            .unwrap();
        assert_eq!(earlier.time, Duration::from_millis(500));
        assert!(
            app.video_preview(&dir.join("missing.mp4"), Duration::ZERO)
                .is_err()
        );
    }

    #[test]
    fn videos_without_gps_are_refused_with_a_reason() {
        let dir = temp_dir("nogps");
        let video = video_in(
            &dir,
            &torqa_video::testing::test_video("app-nogps", 64, 48),
            "a.mp4",
        );
        let mut app = App::new(dir.join("data"), dir.join("cache")).unwrap();

        let events = loaded_video(&mut app, video);

        assert!(
            matches!(events.last(), Some(AppEvent::Error(m)) if m.contains("no GPS")),
            "{events:?}"
        );
        assert!(app.video().is_none());
        assert_eq!(app.courses().len(), 0);
    }

    /// An Incyclist route video in `dir` for `video` (copied there), on a 200 m GPX.
    fn write_incyclist(dir: &Path, video: &Path) {
        std::fs::copy(video, dir.join("climb.mp4")).unwrap();
        let mut gpx = String::from("<gpx><trk><trkseg>");
        for i in 0..=20 {
            let lat = 46.0 + f64::from(i) * 10.0 / 111_195.0;
            let _ = write!(
                gpx,
                r#"<trkpt lat="{lat}" lon="7"><ele>500</ele><time>2024-06-01T08:00:{:06.3}Z</time></trkpt>"#,
                f64::from(i) / 10.0
            );
        }
        gpx.push_str("</trkseg></trk></gpx>");
        std::fs::write(dir.join("climb.gpx"), gpx).unwrap();
        std::fs::write(
            dir.join("climb.xml"),
            "<gpx-import><title>Col &amp; Climb</title><video-file-path>climb.mp4</video-file-path>\
             <gpx-file-path>climb.gpx</gpx-file-path><framerate>10</framerate>\
             <start-frame>1</start-frame></gpx-import>",
        )
        .unwrap();
    }

    fn added(app: &mut App) -> PathBuf {
        let events = run_until(app, |e| {
            matches!(e, AppEvent::CourseAdded(_) | AppEvent::Error(_))
        });
        match events.last() {
            Some(AppEvent::CourseAdded(path)) => path.clone(),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_route_video_of_an_imported_gpx_is_a_course_of_its_own() {
        let dir = temp_dir("same-gpx");
        write_incyclist(&dir, &torqa_video::testing::test_video("same-gpx", 64, 48));
        let mut app = App::new(dir.join("data"), dir.join("cache")).unwrap();

        app.load_route(dir.join("climb.gpx"), true);
        let gpx_course = added(&mut app);
        app.load_video(dir.join("climb.xml"), true);
        let video_course = added(&mut app);

        assert_ne!(gpx_course, video_course);
        let courses = app.courses();
        assert_eq!(courses.len(), 2);
        assert_eq!(
            courses
                .iter()
                .filter(|c| c.manifest.video.is_some())
                .count(),
            1
        );
    }

    #[test]
    fn imported_courses_get_the_name_given_and_replace_only_when_asked() {
        let dir = temp_dir("naming");
        let mut app = App::new(dir.join("data"), dir.join("cache")).unwrap();
        let route = write_route(&dir);

        app.name_next_import("  Evening loop ", false);
        let first = added_after(&mut app, |app| app.load_route(route.clone(), true));
        assert_eq!(course::read_manifest(&first).unwrap().name, "Evening loop");
        assert_eq!(app.course_named("evening LOOP"), Some(first.clone()));
        // Same name, kept next to it.
        app.name_next_import("Evening loop", false);
        let second = added_after(&mut app, |app| app.load_route(route.clone(), true));
        assert_ne!(first, second);
        assert_eq!(app.courses().len(), 2);
        // Replacing: the course of that name is the new one, nothing added.
        app.name_next_import("Evening loop", true);
        write_incyclist(&dir, &torqa_video::testing::test_video("naming", 64, 48));
        let replaced = added_after(&mut app, |app| app.load_video(dir.join("climb.xml"), true));
        assert_eq!(app.courses().len(), 2);
        assert!(replaced == first || replaced == second);
        assert!(course::read_manifest(&replaced).unwrap().video.is_some());
        // Course files too.
        app.name_next_import("Shared", false);
        let copied = added_after(&mut app, |app| app.import_course(replaced.clone()));
        assert_eq!(course::read_manifest(&copied).unwrap().name, "Shared");
        assert_eq!(app.courses().len(), 3);
    }

    fn added_after(app: &mut App, import: impl FnOnce(&mut App)) -> PathBuf {
        import(app);
        added(app)
    }

    #[test]
    fn imports_suggest_the_name_the_files_give() {
        let dir = temp_dir("suggest");
        write_incyclist(&dir, &torqa_video::testing::test_video("suggest", 64, 48));
        let named = dir.join("named.gpx");
        std::fs::write(&named, r#"<gpx><trk><name>Gurten</name><trkseg><trkpt lat="46" lon="7"/></trkseg></trk></gpx>"#).unwrap();

        assert_eq!(App::suggested_course_name(&named), "Gurten");
        assert_eq!(App::suggested_course_name(&dir.join("climb.gpx")), "climb");
        assert_eq!(
            App::suggested_course_name(&dir.join("climb.xml")),
            "Col & Climb"
        );
        assert_eq!(App::suggested_course_name(&dir.join("Ride.MOV")), "Ride");
    }

    #[test]
    fn saving_a_course_twice_keeps_both() {
        let library = temp_dir("unique");
        std::fs::write(library.join("lake-biel.tqc"), b"").unwrap();

        assert_eq!(
            unique_course_path(&library, "Lake Biel"),
            library.join("lake-biel-2.tqc")
        );
        assert_eq!(
            unique_course_path(&library, "Gurten / Bern!"),
            library.join("gurten-bern.tqc")
        );
        assert_eq!(
            unique_course_path(&library, "??"),
            library.join("course.tqc")
        );
    }

    #[test]
    fn generates_the_world_after_loading_a_route() {
        let dir = temp_dir("world");
        let mut app = App::new(dir.join("data"), dir.join("cache")).unwrap();

        app.load_route(write_route(&dir), true);
        let events = run_until(&mut app, |e| matches!(e, AppEvent::WorldReady { .. }));

        assert!(matches!(events.last(), Some(AppEvent::WorldReady { chunks, .. }) if *chunks > 0));
        assert!(app.world().is_some_and(|w| !w.road.vertices.is_empty()));
    }

    #[test]
    fn reports_progress_through_all_stages() {
        let dir = temp_dir("progress");
        let mut app = App::new(dir.join("data"), dir.join("cache")).unwrap();

        app.load_route(write_route(&dir), true);
        let events = run_until(&mut app, |e| matches!(e, AppEvent::WorldReady { .. }));

        for stage in [
            LoadStage::Route,
            LoadStage::Map,
            LoadStage::Elevation,
            LoadStage::World,
        ] {
            let finished = events.iter().any(|e| {
                matches!(e, AppEvent::LoadProgress { stage: s, done, total } if *s == stage && done == total)
            });
            assert!(finished, "{stage:?} not completed: {events:?}");
        }
    }

    /// 300 m flat, 600 m at 6 %, 300 m flat, due north.
    fn write_climb_route(dir: &std::path::Path) -> PathBuf {
        let mut xml = String::from("<gpx><trk><name>Hill</name><trkseg>");
        for i in 0..=120 {
            let lat = 46.0 + f64::from(i) * 10.0 / 111_195.0;
            let ele = 500.0 + f64::from((i - 30).clamp(0, 60)) * 0.6;
            let _ = write!(
                xml,
                r#"<trkpt lat="{lat}" lon="7"><ele>{ele}</ele></trkpt>"#
            );
        }
        xml.push_str("</trkseg></trk></gpx>");
        let path = dir.join("hill.gpx");
        std::fs::write(&path, xml).unwrap();
        path
    }

    /// Rides the loaded route with the fake trainer in fast-forward and saves it.
    fn ride_to_the_finish(app: &mut App, ghost: &GhostChoice) -> Vec<AppEvent> {
        app.connect_trainer(TrainerChoice::Fake(FakeRider {
            power: Watts(400.0),
            cadence: Rpm(90.0),
            heart: None,
        }))
        .unwrap();
        app.start_ride(Percent(50.0), DescentMode::Coast, ghost)
            .unwrap();
        run_until(app, |e| matches!(e, AppEvent::Connected(_)));
        let mut events = Vec::new();
        for _ in 0..2000 {
            std::thread::sleep(Duration::from_millis(2));
            events.extend(app.update(Duration::from_millis(500)));
            if events.iter().any(|e| matches!(e, AppEvent::RideFinished)) {
                app.finish_ride();
                return events;
            }
        }
        panic!("did not finish: {events:?}");
    }

    #[test]
    fn climbs_and_routes_are_timed_against_the_riders_records() {
        let dir = temp_dir("records");
        let mut app = App::new(dir.join("data"), dir.join("cache")).unwrap();
        app.load_route(write_climb_route(&dir), true);
        run_until(&mut app, |e| matches!(e, AppEvent::RouteLoaded(_)));
        assert_eq!(app.route().unwrap().climbs().len(), 1);

        // Nothing to race on a first ride.
        assert!(matches!(
            app.ghost_for(
                app.route().unwrap(),
                DescentMode::Coast,
                &GhostChoice::PersonalBest
            ),
            Err(AppError::GhostUnavailable(_))
        ));
        let first = ride_to_the_finish(&mut app, &GhostChoice::None);
        let climb = first.iter().find_map(|e| match e {
            AppEvent::ClimbCompleted {
                index: 0,
                elapsed,
                previous_best: None,
            } => Some(*elapsed),
            _ => None,
        });
        assert!(climb.is_some_and(|t| t > Duration::ZERO), "{first:?}");
        assert!(first.iter().any(|e| matches!(
            e,
            AppEvent::RouteCompleted {
                previous_best: None,
                ..
            }
        )));

        let records = app.records_for(app.route().unwrap());
        assert_eq!(records.climbs, [climb]);
        assert!(records.route.is_some());

        // Riding it again compares with the first ride.
        std::thread::sleep(Duration::from_millis(1100)); // a new FIT file name
        let second = ride_to_the_finish(&mut app, &GhostChoice::PersonalBest);
        assert!(second.iter().any(|e| matches!(
            e,
            AppEvent::ClimbCompleted {
                previous_best: Some(best),
                ..
            } if Some(*best) == climb
        )));
        let history = app.history();
        assert_eq!(history.len(), 2);
        // Racing the first ride at the same power: the gap stays about zero.
        app.start_ride(
            Percent(50.0),
            DescentMode::Coast,
            &GhostChoice::PersonalBest,
        )
        .unwrap();
        let ghost = app.ghost_state().unwrap();
        assert_eq!(ghost.name, "Your best");
        assert_eq!(ghost.distance, Meters(0.0));
        app.finish_ride();
        app.start_ride(
            Percent(50.0),
            DescentMode::Coast,
            &GhostChoice::WattsPerKg(3.0),
        )
        .unwrap();
        assert_eq!(app.ghost_state().unwrap().name, "Pacer 3.0 W/kg");
        app.finish_ride();
        // Each record belongs to a ride; equal times would count for both.
        assert!(history.iter().any(|h| h.route_record));
        assert!(history.iter().any(|h| h.climb_records == [true]));
        app.shutdown();
    }

    #[test]
    fn riders_keep_their_own_hud_layout() {
        let dir = temp_dir("hud");
        let mut app = App::new(dir.join("data"), dir.join("cache")).unwrap();
        assert_eq!(app.hud_layout(), hud::DEFAULT_LAYOUT);

        let saved = app
            .set_hud_layout(&["power_3s".to_owned(), "nonsense".to_owned()])
            .unwrap();

        assert_eq!(saved, ["power_3s"]);
        assert_eq!(app.hud_layout(), ["power_3s"]);
        app.save_profile(None, Profile::default()).unwrap();
        assert_eq!(app.hud_layout(), hud::DEFAULT_LAYOUT);
    }

    #[test]
    fn riders_have_their_own_profiles_and_rides() {
        let dir = temp_dir("profiles");
        let mut app = App::new(dir.join("data"), dir.join("cache")).unwrap();
        assert_eq!(app.profile().id, "rider");

        let anna = Profile {
            name: "Anna".to_owned(),
            ftp: Watts(280.0),
            ..Profile::default()
        };
        let id = app.save_profile(None, anna.clone()).unwrap();

        assert_eq!(app.profile().profile, anna);
        let names: Vec<String> = app.profiles().into_iter().map(|p| p.profile.name).collect();
        assert_eq!(names, ["Anna", "Rider"]);
        // The choice survives a restart.
        let restarted = App::new(dir.join("data"), dir.join("cache")).unwrap();
        assert_eq!(restarted.profile().id, id);
        assert!(matches!(
            app.select_profile("nobody"),
            Err(AppError::UnknownProfile)
        ));
    }

    #[test]
    fn aborted_rides_leave_no_trace() {
        let dir = temp_dir("abort");
        let mut app = App::new(dir.join("data"), dir.join("cache")).unwrap();
        app.load_route(write_route(&dir), true);
        run_until(&mut app, |e| matches!(e, AppEvent::RouteLoaded(_)));
        app.connect_trainer(TrainerChoice::Fake(FakeRider {
            power: Watts(250.0),
            cadence: Rpm(90.0),
            heart: None,
        }))
        .unwrap();
        app.start_ride(Percent(50.0), DescentMode::Coast, &GhostChoice::None)
            .unwrap();
        run_until(&mut app, |e| matches!(e, AppEvent::Connected(_)));
        for _ in 0..60 {
            std::thread::sleep(Duration::from_millis(16));
            app.update(Duration::from_millis(50));
        }

        app.abort_ride();

        assert!(app.ride_state().is_none());
        assert_eq!(app.finish_ride(), []);
        assert_eq!(app.history().len(), 0);
        app.shutdown();
    }

    #[test]
    fn without_remembered_devices_nothing_is_scanned() {
        let dir = temp_dir("reconnect");
        let mut app = App::new(dir.join("data"), dir.join("cache")).unwrap();

        // No Bluetooth is touched (the container has none): nothing to reconnect.
        assert!(!app.reconnect_remembered());
        assert_eq!(app.update(Duration::ZERO), []);
    }

    #[test]
    fn ride_needs_route_and_trainer() {
        let dir = temp_dir("needs");
        let mut app = App::new(dir.join("data"), dir.join("cache")).unwrap();

        assert!(matches!(
            app.start_ride(Percent(50.0), DescentMode::Coast, &GhostChoice::None),
            Err(AppError::NoRoute)
        ));
        assert!(matches!(
            app.connect_heart_rate(0),
            Err(AppError::UnknownDevice)
        ));
    }
}
