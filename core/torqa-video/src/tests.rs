use super::*;
use crate::testing::{FRAMES, gopro_video, test_video};

fn brightness(frame: &Frame) -> f64 {
    let sum: u64 = frame.rgba.chunks(4).map(|p| u64::from(p[0])).sum();
    #[allow(clippy::cast_precision_loss)]
    let mean = sum as f64 / (frame.rgba.len() / 4) as f64;
    mean
}

#[test]
fn knows_length_rate_and_size() {
    let path = test_video("info", 320, 180);

    let info = Video::open(&path).unwrap().info();

    assert!((info.duration.as_secs_f64() - 4.0).abs() < 0.15, "{info:?}");
    assert!((info.frame_rate - 10.0).abs() < 1e-6, "{info:?}");
    assert_eq!((info.width, info.height), (320, 180));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn hands_out_the_frame_for_any_moment_forwards_and_backwards() {
    let path = test_video("frames", 160, 96);
    let mut video = Video::open(&path).unwrap();
    // Each frame's brightness, decoded in order.
    let in_order: Vec<f64> = (0..FRAMES)
        .map(|i| {
            let t = Duration::from_millis(u64::try_from(i).unwrap() * 100 + 50);
            brightness(video.frame_at(t).unwrap())
        })
        .collect();
    assert!(in_order.windows(2).all(|w| w[1] > w[0]), "{in_order:?}");

    // Jumping back and far ahead lands on the same frames as decoding in order.
    for (seconds, index) in [(3.25, 32), (0.0, 0), (1.04, 10), (3.99, 39), (2.5, 25)] {
        let frame = video.frame_at(Duration::from_secs_f64(seconds)).unwrap();
        assert!(
            (brightness(frame) - in_order[index]).abs() < 1.0,
            "at {seconds} s: {} vs frame {index} {}",
            brightness(frame),
            in_order[index]
        );
        assert!(
            (frame.time.as_secs_f64() - f64::from(u8::try_from(index).unwrap()) / 10.0).abs()
                < 1e-6
        );
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
fn large_videos_are_scaled_to_1080p() {
    assert_eq!(display_size(3840, 2160), (1920, 1080));
    assert_eq!(display_size(2704, 1520), (1920, 1078));
    assert_eq!(display_size(1280, 720), (1280, 720));
}

#[test]
fn files_without_video_are_refused() {
    let path = std::env::temp_dir().join(format!("torqa-video-none-{}.txt", std::process::id()));
    std::fs::write(&path, b"not a video").unwrap();

    assert!(Video::open(&path).is_err());
    std::fs::remove_file(path).unwrap();
}

#[test]
fn reads_the_gps_track_with_video_times() {
    let path = gopro_video("gps");

    let track = gps_track(&path).unwrap();

    assert_eq!(track.len(), 12, "{track:?}");
    // The second payload's third position: 1.5 s into the video, 60 m north.
    let sample = track[6];
    assert!((sample.time.as_secs_f64() - 1.5).abs() < 1e-3, "{sample:?}");
    assert!((sample.point.lat - (46.0 + 60.0 / 111_195.0)).abs() < 1e-7);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn tells_videos_with_gps_from_plain_ones() {
    let gopro = gopro_video("probe");
    let plain = test_video("probe", 64, 48);

    assert!(has_gps(&gopro).unwrap());
    assert!(!has_gps(&plain).unwrap());
    std::fs::remove_file(gopro).unwrap();
    std::fs::remove_file(plain).unwrap();
}

#[test]
fn videos_without_gps_have_no_track() {
    let path = test_video("nogps", 64, 48);

    assert_eq!(gps_track(&path).unwrap(), []);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn the_track_becomes_a_gpx_with_video_times() {
    let point = |lat: f64| gpmf::GpsPoint {
        lat,
        lon: 7.4,
        altitude: 540.25,
        speed: 8.0,
    };
    let track = [
        TimedGps {
            time: Duration::ZERO,
            point: point(46.9),
        },
        TimedGps {
            time: Duration::from_millis(3_725_500),
            point: point(46.9001),
        },
    ];

    let gpx = gpx_from_track("Gurten <climb>", &track);

    assert!(gpx.contains("<name>Gurten &lt;climb&gt;</name>"), "{gpx}");
    assert!(gpx.contains(r#"<trkpt lat="46.9001000" lon="7.4000000"><ele>540.25</ele>"#));
    assert!(
        gpx.contains("<time>1970-01-01T01:02:05.500Z</time>"),
        "{gpx}"
    );
}

/// Loudness (peak-like: √2 × RMS) and zero crossings per second of one channel.
fn tone(samples: &[audio::Stereo], channel: usize, rate: f64) -> (f64, f64) {
    let values: Vec<f64> = samples.iter().map(|s| f64::from(s[channel])).collect();
    #[allow(clippy::cast_precision_loss)]
    let count = values.len() as f64;
    let rms = (values.iter().map(|v| v * v).sum::<f64>() / count).sqrt();
    #[allow(clippy::cast_precision_loss)]
    let crossings = values
        .windows(2)
        .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
        .count() as f64;
    (
        rms * std::f64::consts::SQRT_2,
        crossings / 2.0 / (count / rate),
    )
}

#[test]
fn hands_out_the_sound_of_any_moment_with_its_pitch() {
    use crate::testing::{SOUND_RATE, TONES, sound_video};

    let path = sound_video("audio");
    let mut sound = audio::Audio::open(&path).unwrap().unwrap();
    let rate = f64::from(sound.rate());
    assert_eq!(sound.rate(), u32::try_from(SOUND_RATE).unwrap());
    let mut read_at = |seconds: f64| {
        let mut out = vec![[0.0; 2]; 4_800];
        #[allow(clippy::cast_possible_truncation)]
        sound.read((seconds * rate) as i64, &mut out).unwrap();
        out
    };

    // In order, back, and far ahead: loudness grows 0.2 per second, pitch stays.
    for seconds in [1.0, 1.1, 0.5, 3.5, 2.0] {
        let samples = read_at(seconds);
        for (channel, wanted) in TONES.iter().enumerate() {
            let (loudness, hertz) = tone(&samples, channel, rate);
            let expected = 0.8 * (seconds + 0.05) / 4.0;
            assert!(
                (loudness - expected).abs() < 0.02,
                "{seconds} s: {loudness} vs {expected}"
            );
            assert!(
                (hertz - wanted).abs() < 15.0,
                "{seconds} s: {hertz} Hz vs {wanted}"
            );
        }
    }
    // Past the end: silence.
    assert!(
        read_at(5.0)
            .iter()
            .flatten()
            .all(|v| v.abs() < f32::EPSILON)
    );
    std::fs::remove_file(path).unwrap();
}

#[test]
fn videos_without_sound_have_no_audio() {
    let path = test_video("silent", 64, 48);

    assert!(audio::Audio::open(&path).unwrap().is_none());
    std::fs::remove_file(path).unwrap();
}

#[test]
fn slower_and_faster_sound_keeps_its_pitch() {
    use crate::testing::{TONES, sound_video};

    let path = sound_video("stretch");
    let mut sound = audio::Audio::open(&path).unwrap().unwrap();
    let rate = f64::from(sound.rate());
    for speed in [0.5, 1.0, 1.7] {
        let mut stretcher = audio::Stretcher::new(sound.rate());
        let hop = stretcher.hop();
        let mut out = Vec::new();
        #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
        let mut position = rate * 1.0;
        // One second of output, from 1 s into the track.
        while out.len() < 48_000 {
            #[allow(clippy::cast_possible_truncation)]
            out.extend(stretcher.step(&mut sound, position as i64).unwrap());
            #[allow(clippy::cast_precision_loss)]
            let advance = hop as f64 * speed;
            position += advance;
        }
        // Past the first grain, which fades in.
        let steady = &out[4_800..];
        for (channel, wanted) in TONES.iter().enumerate() {
            let (_, hertz) = tone(steady, channel, rate);
            assert!(
                (hertz - wanted).abs() < 20.0,
                "×{speed}: {hertz} Hz vs {wanted}"
            );
        }
        // The track moved on by the speed: louder the faster (loudness grows with time).
        let (end_loudness, _) = tone(&out[out.len() - 4_800..], 0, rate);
        let expected = 0.8 * (1.0 + speed * 0.95) / 4.0;
        assert!(
            (end_loudness - expected).abs() < 0.03,
            "×{speed}: {end_loudness} vs {expected}"
        );
    }
    std::fs::remove_file(path).unwrap();
}

fn fixture(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures")
        .join(name)
}

/// Mean of each colour over a frame, to compare frames that should be the same picture.
fn colour(frame: &Frame) -> [f64; 3] {
    let mut sum = [0.0; 3];
    for pixel in frame.rgba.chunks(4) {
        for (total, value) in sum.iter_mut().zip(pixel) {
            *total += f64::from(*value);
        }
    }
    #[allow(clippy::cast_precision_loss)]
    let count = (frame.rgba.len() / 4) as f64;
    sum.map(|s| s / count)
}

#[test]
fn av1_videos_play_forwards_backwards_and_after_jumps() {
    // A 2 s, 10 fps test pattern in AV1 (8-bit 4:2:0), as Van Gestel's route videos are.
    let mut video = Video::open(&fixture("av1-8bit.mp4")).unwrap();
    let info = video.info();
    assert_eq!((info.width, info.height), (64, 48));
    assert!((info.duration.as_secs_f64() - 2.0).abs() < 0.15, "{info:?}");
    assert!((info.frame_rate - 10.0).abs() < 1e-6, "{info:?}");

    let in_order: Vec<[f64; 3]> = (0..20)
        .map(|i| {
            let frame = video.frame_at(Duration::from_millis(i * 100 + 50)).unwrap();
            assert_eq!(frame.time, Duration::from_millis(i * 100), "frame {i}");
            colour(frame)
        })
        .collect();
    // A moving pattern, not one colour throughout.
    assert!(in_order.windows(2).any(|w| (w[0][0] - w[1][0]).abs() > 0.5));
    for (seconds, index) in [(1.55, 15), (0.0, 0), (1.95, 19), (0.75, 7)] {
        let frame = video.frame_at(Duration::from_secs_f64(seconds)).unwrap();
        let (got, wanted) = (colour(frame), in_order[index]);
        assert!(
            got.iter().zip(wanted).all(|(a, b)| (a - b).abs() < 0.5),
            "at {seconds} s: {got:?} vs frame {index} {wanted:?}"
        );
    }
}

#[test]
fn deeper_av1_videos_play_too() {
    let mut video = Video::open(&fixture("av1-10bit.mp4")).unwrap();
    let frame = video.frame_at(Duration::from_millis(500)).unwrap();
    assert_eq!(frame.time, Duration::from_millis(500));
    assert!(
        colour(frame).iter().any(|c| *c > 20.0),
        "{:?}",
        colour(frame)
    );
}

#[test]
fn pictures_passed_over_on_the_way_to_a_frame_are_not_converted() {
    // Playback behind the clock skips ahead: decoding the pictures between is unavoidable,
    // scaling them to the screen is not.
    let path = test_video("skip", 160, 96);
    let mut video = Video::open(&path).unwrap();
    let first = video.frame_at(Duration::from_millis(50)).unwrap().time;
    let later = video.frame_at(Duration::from_millis(2050)).unwrap().time;
    assert!(
        later > first + Duration::from_secs(1),
        "{first:?} -> {later:?}"
    );
    assert_eq!(video.converted, 2);
    std::fs::remove_file(path).unwrap();
}

/// Plays a real video as a ride at 1× would, asking for the frame at the wall clock's time,
/// and reports how it keeps up: `TORQA_VIDEO=<file> cargo test -p torqa-video --release --
/// --ignored real_video --nocapture`.
#[test]
#[ignore = "needs a real video in TORQA_VIDEO"]
fn a_real_video_keeps_up_with_the_clock() {
    let path = std::env::var("TORQA_VIDEO").expect("TORQA_VIDEO");
    let mut video = Video::open(Path::new(&path)).unwrap();
    let info = video.info();
    println!("{info:?}");
    let play = Duration::from_secs(8).min(info.duration);
    let start = std::time::Instant::now();
    let (mut delivered, mut last) = (0, None);
    let mut gaps: Vec<f64> = Vec::new();
    let mut previous = start;
    while start.elapsed() < play {
        let frame = video.frame_at(start.elapsed()).unwrap();
        if last != Some(frame.time) {
            last = Some(frame.time);
            delivered += 1;
            gaps.push(previous.elapsed().as_secs_f64());
            previous = std::time::Instant::now();
        }
    }
    gaps.sort_by(f64::total_cmp);
    println!(
        "{delivered} frames in {:.1} s ({:.1}/s), longest wait {:.2} s, median {:.3} s",
        play.as_secs_f64(),
        f64::from(delivered) / play.as_secs_f64(),
        gaps.last().copied().unwrap_or(0.0),
        gaps.get(gaps.len() / 2).copied().unwrap_or(0.0)
    );
    // Straight decoding, no clock: the raw throughput.
    video.seek(Duration::ZERO).unwrap();
    let start = std::time::Instant::now();
    let mut frames = 0;
    while let Ok(Some(_)) = video.decode_next() {
        frames += 1;
        if start.elapsed() > Duration::from_secs(5) {
            break;
        }
    }
    println!(
        "straight decoding: {:.1} frames/s",
        f64::from(frames) / start.elapsed().as_secs_f64()
    );
}
