//! Video decoding for the video ride mode (R17), built on FFmpeg (ADR 0010).
//!
//! A ride does not play a video at its own pace: the rider's position decides which moment of
//! the video is shown, so the speed changes all the time and the rider may even stop. [`Video`]
//! therefore hands out the frame for any moment, decoding forward when that is cheap and
//! seeking otherwise, scaled down to at most [`MAX_WIDTH`] for display.

pub mod audio;
mod av1;
pub mod gpmf;
pub mod incyclist;
pub mod tacx;
#[cfg(any(test, feature = "testing"))]
pub mod testing;

use std::path::Path;
use std::sync::Once;
use std::time::Duration;

use ffmpeg::format::Pixel;
use ffmpeg::software::scaling;
use ffmpeg_next as ffmpeg;

/// Frames are scaled down to at most this width (1080p); larger videos gain nothing on screen.
pub const MAX_WIDTH: u32 = 1920;
/// Further ahead than this, seeking to the next keyframe beats decoding every frame up to it.
const SEEK_AHEAD: Duration = Duration::from_secs(3);

/// Opening or decoding a video failed.
#[derive(Debug, thiserror::Error)]
pub enum VideoError {
    /// FFmpeg could not read the file or decode it.
    #[error("cannot read video: {0}")]
    Ffmpeg(#[from] ffmpeg::Error),
    /// The file has no video stream.
    #[error("the file contains no video")]
    NoVideo,
    /// No frame could be decoded at all.
    #[error("the video has no frames")]
    Empty,
    /// The AV1 decoder failed.
    #[error("{0}")]
    Av1(String),
}

/// What a video is like.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VideoInfo {
    /// Length.
    pub duration: Duration,
    /// Frames per second.
    pub frame_rate: f64,
    /// Width of the frames handed out (after scaling), in pixels.
    pub width: u32,
    /// Height of the frames handed out, in pixels.
    pub height: u32,
}

/// One decoded picture.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    /// When it is shown in the video.
    pub time: Duration,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Pixels row by row, four bytes (red, green, blue, alpha) each.
    pub rgba: Vec<u8>,
}

/// An open video, handing out frames for any moment.
pub struct Video {
    input: ffmpeg::format::context::Input,
    stream: usize,
    decoder: Decoder,
    /// Converts decoded pictures to RGBA at the display size; made for the first picture's
    /// format and remade if it changes.
    scaler: Option<Scaler>,
    /// Seconds per unit of the stream's timestamps.
    time_base: f64,
    info: VideoInfo,
    /// The frame handed out last; the next one requested is usually just after it.
    current: Option<Frame>,
    /// A picture decoded beyond the one requested, kept for the next request.
    ahead: Option<Decoded>,
    ended: bool,
    /// Pictures converted so far: one per frame handed out, none for those passed over.
    converted: u64,
}

/// A decoded picture before conversion, with its moment in the video. Pictures passed over
/// on the way to the one requested stay like this: converting them would cost more than
/// decoding them.
struct Decoded {
    time: Duration,
    picture: ffmpeg::frame::Video,
}

/// What decodes the stream: FFmpeg, or rav1d for AV1 (#39).
enum Decoder {
    Ffmpeg(ffmpeg::decoder::Video),
    Av1 {
        decoder: av1::Av1,
        /// The stream's sequence header, sent again after opening and seeking.
        config: Vec<u8>,
        send_config: bool,
        ready: std::collections::VecDeque<av1::Picture>,
    },
}

struct Scaler {
    from: (Pixel, u32, u32),
    context: scaling::Context,
}

impl Video {
    /// Opens a video file.
    ///
    /// # Errors
    /// [`VideoError`] if the file cannot be read or holds no video.
    pub fn open(path: &Path) -> Result<Self, VideoError> {
        init();
        let input = ffmpeg::format::input(path)?;
        let stream = input
            .streams()
            .best(ffmpeg::media::Type::Video)
            .ok_or(VideoError::NoVideo)?;
        let index = stream.index();
        let time_base = f64::from(stream.time_base());
        // The stream's own rate; the average is frames over duration and comes out uneven.
        let rate = f64::from(stream.rate());
        let frame_rate = if rate > 0.0 {
            rate
        } else {
            f64::from(stream.avg_frame_rate()).max(1.0)
        };
        let duration = if stream.duration() > 0 {
            Duration::from_secs_f64(f64_from(stream.duration()) * time_base)
        } else {
            Duration::from_secs_f64(
                f64_from(input.duration().max(0)) / f64::from(ffmpeg::ffi::AV_TIME_BASE),
            )
        };
        let parameters = stream.parameters();
        let (coded_width, coded_height) = av1::size(&parameters);
        // FFmpeg's own AV1 decoder only drives hardware decoders, which most Macs lack.
        let decoder = if parameters.id() == ffmpeg::codec::Id::AV1 {
            Decoder::Av1 {
                decoder: av1::Av1::new()?,
                config: av1::config_obus(&parameters),
                send_config: true,
                ready: std::collections::VecDeque::new(),
            }
        } else {
            let mut context = ffmpeg::codec::context::Context::from_parameters(parameters)?;
            // FFmpeg decodes on one thread unless told otherwise; a camera's 4K stream
            // needs them all to keep up with a ride.
            context.set_threading(ffmpeg::codec::threading::Config {
                kind: ffmpeg::codec::threading::Type::Frame,
                count: 0,
            });
            Decoder::Ffmpeg(context.decoder().video()?)
        };
        let (width, height) = display_size(coded_width, coded_height);
        Ok(Self {
            input,
            stream: index,
            decoder,
            scaler: None,
            converted: 0,
            time_base,
            info: VideoInfo {
                duration,
                frame_rate,
                width,
                height,
            },
            current: None,
            ahead: None,
            ended: false,
        })
    }

    /// What the video is like.
    #[must_use]
    pub fn info(&self) -> VideoInfo {
        self.info
    }

    /// The frame shown at `time` (clamped to the video): the last frame starting at or before it.
    ///
    /// # Errors
    /// [`VideoError`] if decoding fails.
    pub fn frame_at(&mut self, time: Duration) -> Result<&Frame, VideoError> {
        let time = time.min(self.info.duration);
        let behind = self.current.as_ref().is_some_and(|f| time < f.time);
        let far_ahead = self
            .current
            .as_ref()
            .is_some_and(|f| time > f.time + SEEK_AHEAD);
        if behind || far_ahead || self.current.is_none() {
            self.seek(time)?;
        }
        // Decode forward until the next picture would be past `time`; only the last one
        // reached is converted, the ones passed over are not.
        let mut reached: Option<Decoded> = None;
        loop {
            if self.ahead.is_none() {
                self.ahead = self.decode_next()?;
            }
            match &self.ahead {
                Some(next)
                    if next.time <= time || (self.current.is_none() && reached.is_none()) =>
                {
                    reached = self.ahead.take();
                }
                _ => break,
            }
        }
        if let Some(decoded) = reached {
            self.current = Some(self.convert(&decoded)?);
        }
        self.current.as_ref().ok_or(VideoError::Empty)
    }

    /// Jumps to the keyframe at or before `time`.
    fn seek(&mut self, time: Duration) -> Result<(), VideoError> {
        #[allow(clippy::cast_possible_truncation)] // microseconds of a video fit i64
        let target = time.as_micros() as i64;
        // The keyframe at or before `target`: nothing later than it.
        self.input.seek(target, ..target + 1)?;
        match &mut self.decoder {
            Decoder::Ffmpeg(decoder) => decoder.flush(),
            Decoder::Av1 {
                decoder,
                send_config,
                ready,
                ..
            } => {
                decoder.flush();
                ready.clear();
                *send_config = true;
            }
        }
        self.current = None;
        self.ahead = None;
        self.ended = false;
        Ok(())
    }

    /// The next picture of the stream, or `None` at its end.
    fn decode_next(&mut self) -> Result<Option<Decoded>, VideoError> {
        if matches!(self.decoder, Decoder::Av1 { .. }) {
            return self.decode_next_av1();
        }
        let mut picture = ffmpeg::frame::Video::empty();
        loop {
            let Decoder::Ffmpeg(decoder) = &mut self.decoder else {
                unreachable!("checked above")
            };
            match decoder.receive_frame(&mut picture) {
                Ok(()) => return Ok(Some(self.decoded(picture))),
                Err(ffmpeg::Error::Eof) => return Ok(None),
                Err(ffmpeg::Error::Other {
                    errno: ffmpeg::error::EAGAIN,
                }) => {}
                Err(error) => return Err(error.into()),
            }
            if self.ended {
                return Ok(None);
            }
            // The decoder wants more data: feed it the next packet of our stream.
            let mut fed = false;
            for (stream, packet) in self.input.packets() {
                if stream.index() == self.stream {
                    decoder.send_packet(&packet)?;
                    fed = true;
                    break;
                }
            }
            if !fed {
                decoder.send_eof()?;
                self.ended = true;
            }
        }
    }

    /// [`Video::decode_next`] for AV1: packets go to rav1d, pictures through FFmpeg's scaler.
    fn decode_next_av1(&mut self) -> Result<Option<Decoded>, VideoError> {
        loop {
            let Decoder::Av1 {
                decoder,
                config,
                send_config,
                ready,
            } = &mut self.decoder
            else {
                unreachable!("only called for AV1")
            };
            if let Some(picture) = ready.pop_front() {
                let frame = picture_frame(&picture)?;
                return Ok(Some(self.decoded(frame)));
            }
            if self.ended {
                return Ok(None);
            }
            let mut pictures = Vec::new();
            let mut fed = false;
            for (stream, packet) in self.input.packets() {
                if stream.index() != self.stream {
                    continue;
                }
                let data = packet.data().unwrap_or_default();
                let timestamp = packet.pts().or(packet.dts()).unwrap_or(0);
                if std::mem::take(send_config) && !config.is_empty() {
                    let mut with_config = config.clone();
                    with_config.extend_from_slice(data);
                    decoder.decode(&with_config, timestamp, &mut pictures)?;
                } else {
                    decoder.decode(data, timestamp, &mut pictures)?;
                }
                fed = true;
                break;
            }
            if !fed {
                decoder.pictures(&mut pictures)?;
                self.ended = true;
            }
            ready.extend(pictures);
        }
    }

    /// A decoded picture with its moment in the video, from its timestamp.
    fn decoded(&self, picture: ffmpeg::frame::Video) -> Decoded {
        let timestamp = picture.timestamp().or(picture.pts()).unwrap_or(0).max(0);
        Decoded {
            time: Duration::from_secs_f64(f64_from(timestamp) * self.time_base),
            picture,
        }
    }

    /// The picture scaled to the display size, as RGBA rows.
    fn convert(&mut self, decoded: &Decoded) -> Result<Frame, VideoError> {
        let (width, height) = (self.info.width, self.info.height);
        let picture = &decoded.picture;
        let from = (picture.format(), picture.width(), picture.height());
        if self.scaler.as_ref().is_none_or(|s| s.from != from) {
            self.scaler = Some(Scaler {
                from,
                context: scaling::Context::get(
                    from.0,
                    from.1,
                    from.2,
                    Pixel::RGBA,
                    width,
                    height,
                    scaling::Flags::BILINEAR,
                )?,
            });
        }
        let mut rgba = ffmpeg::frame::Video::empty();
        if let Some(scaler) = &mut self.scaler {
            scaler.context.run(picture, &mut rgba)?;
        }
        self.converted += 1;
        let row = width as usize * 4;
        let stride = rgba.stride(0);
        let data = rgba.data(0);
        // Rows in FFmpeg's buffer may be padded; hand out tightly packed rows.
        let mut pixels = Vec::with_capacity(row * height as usize);
        for y in 0..height as usize {
            pixels.extend_from_slice(&data[y * stride..y * stride + row]);
        }
        Ok(Frame {
            time: decoded.time,
            width,
            height,
            rgba: pixels,
        })
    }
}

/// A picture from rav1d as an FFmpeg frame, for the scaler: its planes copied row by row.
fn picture_frame(picture: &av1::Picture) -> Result<ffmpeg::frame::Video, VideoError> {
    use av1::Layout;

    let (width, height) = (picture.width(), picture.height());
    let deep = picture.bits() > 8;
    let format = match (picture.layout(), picture.bits()) {
        (Layout::I400, 8) => Pixel::GRAY8,
        (Layout::I400, 10) => Pixel::GRAY10LE,
        (Layout::I400, _) => Pixel::GRAY12LE,
        (Layout::I420, 8) => Pixel::YUV420P,
        (Layout::I420, 10) => Pixel::YUV420P10LE,
        (Layout::I420, _) => Pixel::YUV420P12LE,
        (Layout::I422, 8) => Pixel::YUV422P,
        (Layout::I422, 10) => Pixel::YUV422P10LE,
        (Layout::I422, _) => Pixel::YUV422P12LE,
        (Layout::I444, 8) => Pixel::YUV444P,
        (Layout::I444, 10) => Pixel::YUV444P10LE,
        (Layout::I444, _) => Pixel::YUV444P12LE,
    };
    let mut frame = ffmpeg::frame::Video::new(format, width, height);
    let (chroma_width, chroma_height) = match picture.layout() {
        Layout::I400 => (0, 0),
        Layout::I420 => (width.div_ceil(2), height.div_ceil(2)),
        Layout::I422 => (width.div_ceil(2), height),
        Layout::I444 => (width, height),
    };
    let planes = if picture.layout() == Layout::I400 {
        1
    } else {
        3
    };
    for plane in 0..planes {
        let (columns, rows) = if plane == 0 {
            (width, height)
        } else {
            (chroma_width, chroma_height)
        };
        let row_bytes = columns as usize * if deep { 2 } else { 1 };
        let (source, source_stride) = picture
            .plane(plane, rows as usize, row_bytes)
            .ok_or_else(|| VideoError::Av1("an AV1 picture lacks a plane".to_owned()))?;
        let target_stride = frame.stride(plane);
        let target = frame.data_mut(plane);
        for row in 0..rows as usize {
            target[row * target_stride..row * target_stride + row_bytes]
                .copy_from_slice(&source[row * source_stride..row * source_stride + row_bytes]);
        }
    }
    frame.set_pts(Some(picture.timestamp()));
    Ok(frame)
}

/// A camera position with the moment of the video it belongs to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimedGps {
    /// When in the video.
    pub time: Duration,
    /// Where.
    pub point: gpmf::GpsPoint,
}

/// The GPS track a GoPro recorded into the video (its `gpmd` metadata track), each position
/// with its video time; empty for videos without GPS.
///
/// # Errors
/// [`VideoError`] if the file cannot be read.
pub fn gps_track(path: &Path) -> Result<Vec<TimedGps>, VideoError> {
    init();
    let mut input = ffmpeg::format::input(path)?;
    // GoPro writes GPMF into a data track ("GoPro MET"); the track whose packets hold GPS is it.
    let data_streams: Vec<(usize, f64)> = input
        .streams()
        .filter(|s| s.parameters().medium() == ffmpeg::media::Type::Data)
        .map(|s| (s.index(), f64::from(s.time_base())))
        .collect();
    let mut tracks: Vec<Vec<TimedGps>> = vec![Vec::new(); data_streams.len()];
    for (stream, packet) in input.packets() {
        let Some(slot) = data_streams.iter().position(|&(i, _)| i == stream.index()) else {
            continue;
        };
        let time_base = data_streams[slot].1;
        let Some(data) = packet.data() else { continue };
        let points = gpmf::gps_points(data);
        // A payload covers its packet's time span (about a second); its samples are spread
        // evenly over it.
        let start = f64_from(packet.pts().or(packet.dts()).unwrap_or(0).max(0)) * time_base;
        let span = f64_from(packet.duration().max(0)) * time_base;
        #[allow(clippy::cast_precision_loss)] // a handful of samples per packet
        let step = if points.is_empty() {
            0.0
        } else {
            span / points.len() as f64
        };
        #[allow(clippy::cast_precision_loss)]
        tracks[slot].extend(points.into_iter().enumerate().map(|(i, point)| TimedGps {
            time: Duration::from_secs_f64(start + step * i as f64),
            point,
        }));
    }
    let track = tracks.into_iter().max_by_key(Vec::len).unwrap_or_default();
    Ok(track)
}

/// Whether the video records GPS (GoPro GPMF), found from its first data packets rather than
/// by reading the whole file.
///
/// # Errors
/// [`VideoError`] if the file cannot be opened.
pub fn has_gps(path: &Path) -> Result<bool, VideoError> {
    // GoPro writes a payload about once a second; a few without positions mean no GPS (or no
    // fix in the first seconds, when a track would start late anyway).
    const PAYLOADS_TO_CHECK: usize = 10;
    init();
    let mut input = ffmpeg::format::input(path)?;
    let data_streams: Vec<usize> = input
        .streams()
        .filter(|s| s.parameters().medium() == ffmpeg::media::Type::Data)
        .map(|s| s.index())
        .collect();
    if data_streams.is_empty() {
        return Ok(false);
    }
    let mut checked = 0;
    for (stream, packet) in input.packets() {
        if !data_streams.contains(&stream.index()) {
            continue;
        }
        if packet
            .data()
            .is_some_and(|d| !gpmf::gps_points(d).is_empty())
        {
            return Ok(true);
        }
        checked += 1;
        if checked >= PAYLOADS_TO_CHECK * data_streams.len() {
            break;
        }
    }
    Ok(false)
}

/// A GPX track of a video's GPS (e.g. from [`gps_track`]) named `name`, so the usual import
/// prepares a course from the footage. Each point's `<time>` is its moment in the video,
/// counted from the Unix epoch, so the video time can be read back from the track.
#[must_use]
pub fn gpx_from_track(name: &str, track: &[TimedGps]) -> String {
    use std::fmt::Write as _;

    let escaped = name
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let mut gpx = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<gpx version=\"1.1\" creator=\"Torqa\" \
         xmlns=\"http://www.topografix.com/GPX/1/1\">\n<trk><name>{escaped}</name><trkseg>\n"
    );
    for sample in track {
        let seconds = sample.time.as_secs_f64();
        let _ = writeln!(
            gpx,
            "<trkpt lat=\"{:.7}\" lon=\"{:.7}\"><ele>{:.2}</ele><time>{}</time></trkpt>",
            sample.point.lat,
            sample.point.lon,
            sample.point.altitude,
            iso_time(seconds)
        );
    }
    gpx.push_str("</trkseg></trk>\n</gpx>\n");
    gpx
}

/// `1970-01-01T00:00:12.345Z` for 12.345 seconds after the epoch (videos are shorter than a day).
fn iso_time(seconds: f64) -> String {
    let whole = seconds.max(0.0);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // under a day
    let total = whole.floor() as u64;
    format!(
        "1970-01-01T{:02}:{:02}:{:06.3}Z",
        total / 3600,
        total / 60 % 60,
        whole - f64::from(u32::try_from(total - total % 60).unwrap_or(0))
    )
}

/// Sets up FFmpeg once per process.
pub(crate) fn init() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let _ = ffmpeg::init();
        ffmpeg::log::set_level(ffmpeg::log::Level::Error);
    });
}

/// The size frames are handed out in: at most [`MAX_WIDTH`] wide, same aspect, even sides.
fn display_size(width: u32, height: u32) -> (u32, u32) {
    if width <= MAX_WIDTH {
        return (width, height);
    }
    let scaled = u64::from(height) * u64::from(MAX_WIDTH) / u64::from(width);
    let even = u32::try_from(scaled).unwrap_or(u32::MAX) & !1;
    (MAX_WIDTH, even.max(2))
}

#[allow(clippy::cast_precision_loss)] // timestamps far below 2^52
pub(crate) fn f64_from(value: i64) -> f64 {
    value as f64
}

/// How a video keeps up on this machine, played against the wall clock as a ride at 1×
/// does; from [`benchmark`], for `torqa-cli video`.
#[derive(Debug, Clone, PartialEq)]
pub struct Benchmark {
    /// The video.
    pub info: VideoInfo,
    /// How long the clock ran.
    pub played: Duration,
    /// Frames handed out meanwhile.
    pub delivered: u32,
    /// The longest wait between two frames.
    pub longest_wait: Duration,
    /// The median wait between two frames.
    pub median_wait: Duration,
    /// Frames decoded per second with no clock to wait for.
    pub straight_per_second: f64,
}

impl Benchmark {
    /// Frames handed out per second of the clock.
    #[must_use]
    pub fn delivered_per_second(&self) -> f64 {
        f64::from(self.delivered) / self.played.as_secs_f64().max(1e-9)
    }

    /// Whether a ride along this video would run smoothly here: frames at the video's own
    /// rate, or 24 a second at least, and never a wait of half a second.
    #[must_use]
    pub fn keeps_up(&self) -> bool {
        let wanted = self.info.frame_rate.min(24.0) * 0.9;
        self.delivered_per_second() >= wanted && self.longest_wait < Duration::from_millis(500)
    }
}

/// Plays the video at `path` against the wall clock for `seconds`, asking for the frame at
/// the clock's time as a ride at 1× does, then decodes straight for up to five seconds.
///
/// # Errors
/// [`VideoError`] if the file cannot be opened or decoded.
pub fn benchmark(path: &Path, seconds: u64) -> Result<Benchmark, VideoError> {
    let mut video = Video::open(path)?;
    let info = video.info();
    let played = Duration::from_secs(seconds).min(info.duration);
    let start = std::time::Instant::now();
    let (mut delivered, mut last) = (0, None);
    let mut waits: Vec<Duration> = Vec::new();
    let mut previous = start;
    while start.elapsed() < played {
        let time = video.frame_at(start.elapsed())?.time;
        if last != Some(time) {
            last = Some(time);
            delivered += 1;
            waits.push(previous.elapsed());
            previous = std::time::Instant::now();
        }
    }
    waits.sort();
    video.seek(Duration::ZERO)?;
    let start = std::time::Instant::now();
    let mut frames = 0;
    while video.decode_next()?.is_some() {
        frames += 1;
        if start.elapsed() > Duration::from_secs(5) {
            break;
        }
    }
    Ok(Benchmark {
        info,
        played,
        delivered,
        longest_wait: waits.last().copied().unwrap_or_default(),
        median_wait: waits.get(waits.len() / 2).copied().unwrap_or_default(),
        straight_per_second: f64::from(frames) / start.elapsed().as_secs_f64().max(1e-9),
    })
}

#[cfg(test)]
mod tests;
