//! Video courses (R17): a route ridden along a video — a GoPro recording with its own GPS, an
//! Incyclist route video (control file + GPX + video), or any video added to a GPX course by
//! hand (where the route starts and ends in it). The rider's distance decides the moment of the
//! video, through the same matching used for ghosts (R20).

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use torqa_domain::units::Meters;
use torqa_routes::Route;
use torqa_session::ghost::Ghost;
use torqa_video::audio::{Audio, Stereo, Stretcher};
use torqa_video::{Frame, Video, gps_track, gpx_from_track, incyclist, tacx};

/// File extensions of the videos Torqa reads directly (GoPro and similar).
pub const VIDEO_EXTENSIONS: [&str; 4] = ["mp4", "mov", "m4v", "mkv"];

/// What a video course is prepared from.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoSource {
    /// The video file.
    pub video: PathBuf,
    /// Display name.
    pub name: String,
    /// The route as GPX; its timestamps (from the first point) are the video's timeline.
    pub gpx: String,
    /// Where the route's first point sits in the video.
    pub offset: Duration,
    /// For videos aligned by hand: where route positions sit in the video, from the start to
    /// the end of the route; the video follows the distance evenly between neighbours. Empty
    /// when timestamps pair them.
    pub marks: Vec<SyncMark>,
    /// For a video with a record of its own pace (a Tacx RLV added to a GPX course): the
    /// distance as that record counts it, and the moment of the video there, at each change of
    /// speed. Between the marks the video follows this pace rather than going evenly. Empty
    /// for other videos.
    pub pace: Vec<SyncMark>,
    /// Whether the GPX is a real place; `false` for a Tacx RLV course drawn from its slopes.
    pub located: bool,
}

/// A position on the route and the moment of the video showing it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SyncMark {
    /// Distance along the route.
    pub distance: Meters,
    /// Moment in the video.
    pub time: Duration,
}

/// A Tacx Real Life Video (#42) from its `.rlv` file: the video and the `.pgmf` course next to
/// it (found by name, ignoring case). The course has no place, only distance and slope, so its
/// GPX is drawn from the slopes and the video is paired by the RLV's speed changes.
///
/// # Errors
/// A readable message if a file is missing or cannot be read.
fn tacx_source(path: &Path) -> Result<VideoSource, String> {
    let (video, marks) = rlv_pace(path)?;
    let dir = path.parent().unwrap_or(Path::new("."));
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let pgmf_name = format!("{stem}.pgmf");
    let pgmf_path =
        find_file(dir, |name| name.eq_ignore_ascii_case(&pgmf_name)).ok_or_else(|| {
            format!("{pgmf_name} not found — it holds the course's slopes, put it next to the RLV")
        })?;
    let pgmf_bytes = std::fs::read(&pgmf_path).map_err(|e| unreadable(&pgmf_path, &e))?;
    let pgmf = tacx::parse_pgmf(&pgmf_bytes).map_err(|e| unreadable(&pgmf_path, &e))?;
    let name = tacx_name(&pgmf, &stem);
    Ok(VideoSource {
        video,
        gpx: tacx::gpx_from_profile(&name, &pgmf),
        name,
        offset: Duration::ZERO,
        marks,
        pace: Vec::new(),
        located: false,
    })
}

fn unreadable(file: &Path, e: &dyn std::fmt::Display) -> String {
    format!("cannot read {}: {e}", file.display())
}

/// The video of the Tacx `.rlv` at `path`, found next to it, and its pace: the distance the
/// camera has moved and the moment of the video, at each change of speed.
///
/// # Errors
/// A readable message if the RLV or its video is missing or cannot be read.
fn rlv_pace(path: &Path) -> Result<(PathBuf, Vec<SyncMark>), String> {
    let bytes = std::fs::read(path).map_err(|e| unreadable(path, &e))?;
    let rlv = tacx::parse_rlv(&bytes).map_err(|e| unreadable(path, &e))?;
    let dir = path.parent().unwrap_or(Path::new("."));
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let video = find_file(dir, |name| name.eq_ignore_ascii_case(rlv.video_file_name()))
        .or_else(|| {
            // Courses copied about often keep the video next to the RLV under its own name.
            find_file(dir, |name| {
                let file = Path::new(name);
                file.file_stem()
                    .is_some_and(|s| s.to_string_lossy().eq_ignore_ascii_case(&stem))
                    && !file.extension().is_some_and(|e| {
                        ["rlv", "pgmf"].contains(&e.to_string_lossy().to_lowercase().as_str())
                    })
            })
        })
        .ok_or_else(|| {
            format!(
                "video {} not found — put it next to {}",
                rlv.video_file_name(),
                path.display()
            )
        })?;
    let duration = Video::open(&video)
        .map_err(|e| unreadable(&video, &e))?
        .info()
        .duration;
    // The RLV's end may lie a few frames past the video's; the video ends where it ends.
    let mut pace: Vec<SyncMark> = Vec::new();
    for (distance, time) in rlv.sync_points() {
        let time = time.min(duration);
        if pace.last().is_none_or(|m| time > m.time) {
            pace.push(SyncMark {
                distance: Meters(distance),
                time,
            });
        }
    }
    Ok((video, pace))
}

/// The video to add to a GPX course from the file chosen, and its pace (see
/// [`VideoSource::pace`]): for a Tacx `.rlv`, the video next to it, ridden by the RLV's
/// speeds; any other file is the video itself, without a pace.
///
/// # Errors
/// A readable message if an RLV or its video is missing or cannot be read.
pub fn paced_video(path: &Path) -> Result<(PathBuf, Vec<SyncMark>), String> {
    if is_rlv(path) {
        rlv_pace(path)
    } else {
        Ok((path.to_owned(), Vec::new()))
    }
}

fn is_rlv(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("rlv"))
}

/// `(route distance m, video s)` for each mark, with the moments of `pace` in between: the
/// video follows the pace from one mark to the next, stretched to the route's distance there.
/// Evenly between marks where the pace does not move.
fn paced_trace(marks: &[SyncMark], pace: &[SyncMark]) -> Vec<(f64, f64)> {
    let along = |time: Duration| -> f64 {
        let after = pace.partition_point(|p| p.time <= time);
        match (after.checked_sub(1).map(|i| pace[i]), pace.get(after)) {
            (Some(a), Some(b)) => {
                let f = time.saturating_sub(a.time).as_secs_f64()
                    / b.time.saturating_sub(a.time).as_secs_f64();
                a.distance.0 + (b.distance.0 - a.distance.0) * f
            }
            (Some(a), None) => a.distance.0,
            (None, Some(b)) => b.distance.0,
            (None, None) => 0.0,
        }
    };
    let mut trace = Vec::new();
    for pair in marks.windows(2) {
        let (from, to) = (pair[0], pair[1]);
        trace.push((from.distance.0, from.time.as_secs_f64()));
        let (start, end) = (along(from.time), along(to.time));
        if end <= start {
            continue;
        }
        let scale = (to.distance.0 - from.distance.0) / (end - start);
        for p in pace
            .iter()
            .filter(|p| p.time > from.time && p.time < to.time)
        {
            trace.push((
                from.distance.0 + (p.distance.0 - start) * scale,
                p.time.as_secs_f64(),
            ));
        }
    }
    if let Some(last) = marks.last() {
        trace.push((last.distance.0, last.time.as_secs_f64()));
    }
    trace
}

/// The name of a Tacx course: its PGMF's, unless missing or cut short by the field's size,
/// then the RLV's file name (`stem`).
fn tacx_name(pgmf: &tacx::Pgmf, stem: &str) -> String {
    let name = pgmf.name.trim();
    if name.is_empty() || name.chars().count() >= PGMF_NAME_CHARS {
        stem.to_owned()
    } else {
        name.to_owned()
    }
}

/// The name a Tacx course from the `.rlv` at `path` gets, if its PGMF can be read.
#[must_use]
pub fn tacx_course_name(path: &Path) -> Option<String> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let stem = path.file_stem()?.to_string_lossy().into_owned();
    let pgmf_name = format!("{stem}.pgmf");
    let pgmf = find_file(dir, |name| name.eq_ignore_ascii_case(&pgmf_name))?;
    let pgmf = tacx::parse_pgmf(&std::fs::read(pgmf).ok()?).ok()?;
    Some(tacx_name(&pgmf, &stem))
}

/// Characters a PGMF course name can hold (34 bytes of UTF-16).
const PGMF_NAME_CHARS: usize = 17;

/// The file in `dir` whose name `matches`, if any.
fn find_file(dir: &Path, matches: impl Fn(&str) -> bool) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.is_file()
                && path
                    .file_name()
                    .is_some_and(|name| matches(&name.to_string_lossy()))
        })
}

/// What the import needs to know about a video first: its length, and whether it carries GPS
/// (then it pairs itself with its route) or must be placed on a GPX by hand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    /// The video itself: the file looked at, or the video of a Tacx `.rlv`.
    pub video: PathBuf,
    /// The video's length.
    pub duration: Duration,
    /// Whether it records GPS.
    pub has_gps: bool,
    /// Where the ride starts and ends in the video, as far as known: the whole video, or
    /// the stretch a Tacx RLV's pace covers.
    pub span: (Duration, Duration),
}

/// Looks at a video, or the video of a Tacx `.rlv`, before importing it or adding it to a
/// course.
///
/// # Errors
/// A readable message if it cannot be opened as a video.
pub fn probe(path: &Path) -> Result<Probe, String> {
    let (video, pace) = paced_video(path)?;
    let duration = Video::open(&video)
        .map_err(|e| unreadable(&video, &e))?
        .info()
        .duration;
    let has_gps = torqa_video::has_gps(&video).map_err(|e| unreadable(&video, &e))?;
    let span = match (pace.first(), pace.last()) {
        (Some(first), Some(last)) if last.time > first.time => (first.time, last.time),
        _ => (Duration::ZERO, duration),
    };
    Ok(Probe {
        video,
        duration,
        has_gps,
        span,
    })
}

/// Reads what a video course needs from an Incyclist control file (`.xml`) or a video with
/// GPS (GoPro).
///
/// # Errors
/// A readable message if the files cannot be read, or the video has no GPS.
pub fn source(path: &Path) -> Result<VideoSource, String> {
    let extension = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let unreadable = |e: &dyn std::fmt::Display| format!("cannot read {}: {e}", path.display());
    if extension == "rlv" {
        return tacx_source(path);
    }
    if extension == "xml" {
        let xml = std::fs::read_to_string(path).map_err(|e| unreadable(&e))?;
        let route = incyclist::parse(&xml).map_err(|e| unreadable(&e))?;
        let dir = path.parent().unwrap_or(Path::new("."));
        let gpx_path = dir.join(&route.gpx_file);
        let gpx = std::fs::read_to_string(&gpx_path)
            .map_err(|e| format!("cannot read {}: {e}", gpx_path.display()))?;
        return Ok(VideoSource {
            video: dir.join(&route.video_file),
            name: route.title.clone(),
            gpx,
            offset: route.video_offset(),
            marks: Vec::new(),
            pace: Vec::new(),
            located: true,
        });
    }
    let track = gps_track(path).map_err(|e| unreadable(&e))?;
    if track.len() < 2 {
        return Err(format!(
            "{} has no GPS track — import its GPX route, then add the video on the course page",
            path.display()
        ));
    }
    let name = path
        .file_stem()
        .map_or_else(|| "Video".to_owned(), |s| s.to_string_lossy().into_owned());
    Ok(VideoSource {
        video: path.to_owned(),
        gpx: gpx_from_track(&name, &track),
        name,
        offset: Duration::ZERO,
        marks: Vec::new(),
        pace: Vec::new(),
        located: true,
    })
}

/// A route paired with its video.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoCourse {
    /// The video file.
    pub video: PathBuf,
    /// Where the route's first point sits in the video.
    pub offset: Duration,
    /// The marks of a video aligned by hand, from the route's start to its end; empty
    /// otherwise.
    pub marks: Vec<SyncMark>,
    /// The video's own pace between the marks (see [`VideoSource::pace`]); empty when it goes
    /// evenly.
    pub pace: Vec<SyncMark>,
    /// Whether the route is a real place (see [`VideoSource::located`]).
    pub located: bool,
    /// The video's length.
    pub duration: Duration,
    /// Video time at each distance along the route.
    sync: Ghost,
}

/// Puts the first mark at the route's start and the last at its end, then checks that the
/// marks follow each other along the route and in the video, within its length.
///
/// # Errors
/// A readable message if they do not.
pub fn fit_marks(
    marks: &[SyncMark],
    length: Meters,
    duration: Duration,
) -> Result<Vec<SyncMark>, String> {
    // Container durations are rounded; a mark on the last frame may lie a little past them.
    let slack = Duration::from_millis(500);
    if marks.len() < 2 {
        return Err("a video needs a start and an end on the route".to_owned());
    }
    let mut fitted = marks.to_vec();
    fitted[0].distance = Meters(0.0);
    let last = fitted.len() - 1;
    fitted[last].distance = length;
    let ordered = fitted
        .windows(2)
        .all(|w| w[1].distance.0 > w[0].distance.0 && w[1].time > w[0].time);
    if !ordered {
        return Err(
            "each point must come after the one before it, on the route and in the video"
                .to_owned(),
        );
    }
    if fitted[last].time > duration + slack {
        return Err("the route must end within the video".to_owned());
    }
    Ok(fitted)
}

impl VideoCourse {
    /// Pairs `route` (imported from `source.gpx`) with the video: by the GPX timestamps, or
    /// by the marks of a video aligned by hand, in between at the video's own pace if it has
    /// one, else evenly. Without either, the video is spread evenly from its offset to its
    /// end.
    ///
    /// # Errors
    /// A readable message if the video cannot be opened or the marks do not fit it.
    pub fn new(route: &Route, source: &VideoSource) -> Result<Self, String> {
        let duration = Video::open(&source.video)
            .map_err(|e| format!("cannot open {}: {e}", source.video.display()))?
            .info()
            .duration;
        let marks = if source.marks.is_empty() {
            Vec::new()
        } else {
            fit_marks(&source.marks, route.length(), duration)?
        };
        let offset = marks.first().map_or(source.offset, |m| m.time);
        let sync = if marks.is_empty() {
            // Matching counts from the route start; the offset places that in the video.
            torqa_routes::timed_points(&source.gpx)
                .ok()
                .and_then(|points| Ghost::from_activity("video", route, &points))
                .or_else(|| {
                    let span = duration.saturating_sub(offset).as_secs_f64();
                    Ghost::from_trace("video", [(0.0, 0.0), (route.length().0, span)].into_iter())
                })
        } else {
            let start = offset.as_secs_f64();
            Ghost::from_trace(
                "video",
                paced_trace(&marks, &source.pace)
                    .into_iter()
                    .map(|(distance, time)| (distance, (time - start).max(0.0))),
            )
        }
        .ok_or_else(|| "the video and the route do not match".to_owned())?;
        Ok(Self {
            video: source.video.clone(),
            offset,
            marks,
            pace: source.pace.clone(),
            located: source.located,
            duration,
            sync,
        })
    }

    /// Whether the video was placed on the route by hand (marks), so the marks can be moved.
    #[must_use]
    pub fn aligned_by_hand(&self) -> bool {
        !self.marks.is_empty()
    }

    /// The moment of the video at `distance` along the route (the end beyond it).
    #[must_use]
    pub fn time_at(&self, distance: Meters) -> Duration {
        let along = self
            .sync
            .time_at(distance)
            .unwrap_or_else(|| self.sync.total_time());
        (self.offset + along).min(self.duration)
    }
}

/// Decodes a video course's frames on its own thread, so riding never waits for the decoder:
/// [`VideoPlayer::show`] asks for a moment, [`VideoPlayer::frame`] hands out the newest frame
/// once decoded.
pub struct VideoPlayer {
    shared: Arc<(Mutex<PlayerState>, Condvar)>,
    thread: Option<JoinHandle<()>>,
}

#[derive(Default)]
struct PlayerState {
    wanted: Option<Duration>,
    ready: Option<Frame>,
    error: Option<String>,
    stop: bool,
}

impl VideoPlayer {
    /// Opens `video` for playback.
    ///
    /// # Errors
    /// A readable message if the video cannot be opened.
    pub fn open(video: &Path) -> Result<Self, String> {
        let shared = Arc::new((Mutex::new(PlayerState::default()), Condvar::new()));
        let state = Arc::clone(&shared);
        let path = video.to_owned();
        let (opened_tx, opened_rx) = std::sync::mpsc::channel();
        // The decoder stays on its thread: FFmpeg's scaler may not move between threads.
        let thread = std::thread::Builder::new()
            .name("video".to_owned())
            .spawn(move || {
                let mut video = match Video::open(&path) {
                    Ok(video) => {
                        let _ = opened_tx.send(Ok(()));
                        video
                    }
                    Err(error) => {
                        let _ =
                            opened_tx.send(Err(format!("cannot open {}: {error}", path.display())));
                        return;
                    }
                };
                // The frame after the moment shown, so the view can blend towards it (R17).
                let step = Duration::from_secs_f64(1.0 / video.info().frame_rate.max(1.0));
                let (lock, wake) = &*state;
                let mut shown: Option<Duration> = None;
                loop {
                    let wanted = {
                        let mut s = lock.lock().unwrap_or_else(PoisonError::into_inner);
                        while s.wanted.is_none() && !s.stop {
                            s = wake.wait(s).unwrap_or_else(PoisonError::into_inner);
                        }
                        if s.stop {
                            return;
                        }
                        s.wanted.take()
                    };
                    let Some(wanted) = wanted else { continue };
                    let decoded = video.frame_at(wanted + step);
                    let mut s = lock.lock().unwrap_or_else(PoisonError::into_inner);
                    match decoded {
                        Ok(frame) if shown != Some(frame.time) => {
                            shown = Some(frame.time);
                            s.ready = Some(frame.clone());
                        }
                        Ok(_) => {}
                        Err(error) => s.error = Some(error.to_string()),
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        opened_rx
            .recv()
            .map_err(|_| "the video player stopped".to_owned())??;
        Ok(Self {
            shared,
            thread: Some(thread),
        })
    }

    /// Asks for the frames around `time`; a newer request replaces one not yet started.
    pub fn show(&self, time: Duration) {
        let (lock, wake) = &*self.shared;
        lock.lock().unwrap_or_else(PoisonError::into_inner).wanted = Some(time);
        wake.notify_one();
    }

    /// The newest decoded frame not handed out yet: the one following the moment asked for.
    #[must_use]
    pub fn frame(&self) -> Option<Frame> {
        let (lock, _) = &*self.shared;
        lock.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .ready
            .take()
    }

    /// Why the video could not be decoded, once, if it could not.
    #[must_use]
    pub fn error(&self) -> Option<String> {
        let (lock, _) = &*self.shared;
        lock.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .error
            .take()
    }
}

impl Drop for VideoPlayer {
    fn drop(&mut self) {
        let (lock, wake) = &*self.shared;
        lock.lock().unwrap_or_else(PoisonError::into_inner).stop = true;
        wake.notify_one();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl std::fmt::Debug for VideoPlayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VideoPlayer").finish_non_exhaustive()
    }
}

/// Sound queued ahead of playback, in seconds: enough to ride out a slow frame, little enough
/// to follow the rider's speed closely.
const SOUND_AHEAD_S: f64 = 0.12;
/// Below this speed of the video the sound fades out (standing still is silent); it is fully
/// there from `SOUND_FULL_SPEED` on.
const SOUND_SILENT_SPEED: f64 = 0.2;
const SOUND_FULL_SPEED: f64 = 0.5;
/// How far the sound may drift from the picture before it jumps rather than catching up.
const SOUND_MAX_DRIFT_S: f64 = 0.5;

/// The sound of a video course during a ride (R26), on its own thread: it follows the moment
/// of the video ([`SoundPlayer::follow`]) at the speed the rider makes it play, stretched to
/// keep its pitch, and is pulled by the front end ([`SoundPlayer::pull`]).
pub struct SoundPlayer {
    shared: Arc<(Mutex<SoundState>, Condvar)>,
    thread: Option<JoinHandle<()>>,
    rate: u32,
}

struct SoundState {
    /// The moment of the video shown last, and when.
    target: Option<(Duration, Instant)>,
    /// Video seconds played per second, estimated from the targets.
    speed: f64,
    on: bool,
    queue: VecDeque<Stereo>,
    stop: bool,
}

impl SoundPlayer {
    /// Opens the sound of `video`; `None` if it has none.
    ///
    /// # Errors
    /// A readable message if the video cannot be read.
    pub fn open(video: &Path) -> Result<Option<Self>, String> {
        let shared = Arc::new((
            Mutex::new(SoundState {
                target: None,
                speed: 0.0,
                on: true,
                queue: VecDeque::new(),
                stop: false,
            }),
            Condvar::new(),
        ));
        let state = Arc::clone(&shared);
        let path = video.to_owned();
        let (opened_tx, opened_rx) = std::sync::mpsc::channel();
        // Like the picture, the decoder stays on its thread.
        let thread = std::thread::Builder::new()
            .name("video sound".to_owned())
            .spawn(move || {
                let audio = match Audio::open(&path) {
                    Ok(Some(audio)) => {
                        let _ = opened_tx.send(Ok(Some(audio.rate())));
                        audio
                    }
                    Ok(None) => {
                        let _ = opened_tx.send(Ok(None));
                        return;
                    }
                    Err(error) => {
                        let _ =
                            opened_tx.send(Err(format!("cannot open {}: {error}", path.display())));
                        return;
                    }
                };
                play(audio, &state);
            })
            .map_err(|e| e.to_string())?;
        let rate = opened_rx
            .recv()
            .map_err(|_| "the sound player stopped".to_owned())??;
        Ok(rate.map(|rate| Self {
            shared,
            thread: Some(thread),
            rate,
        }))
    }

    /// Samples per second of [`SoundPlayer::pull`].
    #[must_use]
    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// The moment of the video shown now; called every frame.
    pub fn follow(&self, time: Duration) {
        let now = Instant::now();
        let (lock, _) = &*self.shared;
        let mut s = lock.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((before, at)) = s.target {
            let elapsed = now.duration_since(at).as_secs_f64();
            if elapsed > 0.0 {
                let speed = (time.as_secs_f64() - before.as_secs_f64()) / elapsed;
                // Frames come unevenly; smooth over a few of them.
                s.speed += (speed.clamp(0.0, 8.0) - s.speed) * 0.2;
            }
        }
        s.target = Some((time, now));
    }

    /// Switches the sound on or off (it fades).
    pub fn set_on(&self, on: bool) {
        let (lock, _) = &*self.shared;
        lock.lock().unwrap_or_else(PoisonError::into_inner).on = on;
    }

    /// Up to `max` stereo samples to play next.
    #[must_use]
    pub fn pull(&self, max: usize) -> Vec<Stereo> {
        let (lock, wake) = &*self.shared;
        let mut s = lock.lock().unwrap_or_else(PoisonError::into_inner);
        let count = max.min(s.queue.len());
        let samples = s.queue.drain(..count).collect();
        wake.notify_one();
        samples
    }
}

/// The sound thread: keeps the queue filled with the stretched sound around the moment shown.
fn play(mut audio: Audio, state: &Arc<(Mutex<SoundState>, Condvar)>) {
    let rate = f64::from(audio.rate());
    let mut stretcher = Stretcher::new(audio.rate());
    let hop = stretcher.hop();
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_sign_loss,
        clippy::cast_possible_truncation
    )]
    let ahead = (SOUND_AHEAD_S * rate) as usize;
    let mut position: Option<f64> = None;
    let mut gain = 0.0_f32;
    let (lock, wake) = &**state;
    loop {
        let (target, speed, on, queued) = {
            let mut s = lock.lock().unwrap_or_else(PoisonError::into_inner);
            while !s.stop && (s.target.is_none() || s.queue.len() >= ahead) {
                s = wake
                    .wait_timeout(s, Duration::from_millis(10))
                    .unwrap_or_else(PoisonError::into_inner)
                    .0;
            }
            if s.stop {
                return;
            }
            let Some(target) = s.target else { continue };
            (target, s.speed, s.on, s.queue.len())
        };
        // Where the picture will be when this hop is heard, after what is queued already.
        #[allow(clippy::cast_precision_loss)]
        let lead = target.1.elapsed().as_secs_f64() + queued as f64 / rate;
        let expected = (target.0.as_secs_f64() + lead * speed) * rate;
        #[allow(clippy::cast_precision_loss)]
        let step = hop as f64 * speed;
        let at = match position {
            Some(at) if (expected - at).abs() < SOUND_MAX_DRIFT_S * rate => {
                // Catch up gently rather than jump, which would be heard.
                at + step + (expected - at) * 0.05
            }
            _ => {
                stretcher.reset();
                expected
            }
        };
        position = Some(at);
        #[allow(clippy::cast_possible_truncation)]
        let wanted = if on {
            ((speed - SOUND_SILENT_SPEED) / (SOUND_FULL_SPEED - SOUND_SILENT_SPEED)).clamp(0.0, 1.0)
                as f32
        } else {
            0.0
        };
        let samples = if gain <= 0.0 && wanted <= 0.0 {
            vec![[0.0; 2]; hop]
        } else {
            #[allow(clippy::cast_possible_truncation)]
            let mut samples = stretcher
                .step(&mut audio, at as i64)
                .unwrap_or_else(|error| {
                    tracing::warn!(%error, "cannot decode the video's sound");
                    vec![[0.0; 2]; hop]
                });
            // Fade over a few hops so that switching or stopping does not click.
            #[allow(clippy::cast_precision_loss)]
            let change = (wanted - gain).clamp(-0.1, 0.1) / hop as f32;
            for sample in &mut samples {
                gain = (gain + change).clamp(0.0, 1.0);
                sample[0] *= gain;
                sample[1] *= gain;
            }
            samples
        };
        lock.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .queue
            .extend(samples);
    }
}

impl Drop for SoundPlayer {
    fn drop(&mut self) {
        let (lock, wake) = &*self.shared;
        lock.lock().unwrap_or_else(PoisonError::into_inner).stop = true;
        wake.notify_one();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl std::fmt::Debug for SoundPlayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SoundPlayer")
            .field("rate", &self.rate)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marks(pairs: &[(f64, f64)]) -> Vec<SyncMark> {
        pairs
            .iter()
            .map(|&(d, t)| SyncMark {
                distance: Meters(d),
                time: Duration::from_secs_f64(t),
            })
            .collect()
    }

    #[test]
    fn between_marks_a_video_follows_its_own_pace_stretched_to_the_route() {
        // The camera covers 20 m in the first 2 s, then only 10 m in the next 2 s (a climb).
        let pace = marks(&[(0.0, 0.0), (20.0, 2.0), (30.0, 4.0)]);
        // The route is 60 m long: twice the RLV's distance.
        let trace = paced_trace(&marks(&[(0.0, 0.0), (60.0, 4.0)]), &pace);
        assert_eq!(trace, [(0.0, 0.0), (40.0, 2.0), (60.0, 4.0)]);

        // A mark in between, off from where the pace would put it: each stretch is fitted
        // to its own marks. The pace's 20 m lies between 1 s (10 m) and 3 s (25 m).
        let trace = paced_trace(&marks(&[(0.0, 1.0), (30.0, 3.0), (40.0, 4.0)]), &pace);
        assert_eq!(trace.len(), 4);
        assert_eq!(trace[0], (0.0, 1.0));
        assert!((trace[1].0 - 20.0).abs() < 1e-9 && (trace[1].1 - 2.0).abs() < 1e-9);
        assert_eq!(&trace[2..], [(30.0, 3.0), (40.0, 4.0)]);
    }

    #[test]
    fn without_a_pace_the_video_goes_evenly_from_mark_to_mark() {
        let given = marks(&[(0.0, 0.5), (10.0, 2.5), (40.0, 3.5)]);
        assert_eq!(
            paced_trace(&given, &[]),
            [(0.0, 0.5), (10.0, 2.5), (40.0, 3.5)]
        );
        // Where the pace stands still (the camera stopped), evenly too.
        let still = marks(&[(0.0, 0.0), (5.0, 1.0), (5.0, 3.0), (10.0, 4.0)]);
        assert_eq!(
            paced_trace(&marks(&[(0.0, 1.5), (20.0, 2.5)]), &still),
            [(0.0, 1.5), (20.0, 2.5)]
        );
    }
}
