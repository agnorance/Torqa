//! Headless command-line interface to the Torqa core.

mod devices;
mod free_ride;
mod gear_check;
mod recorded_ride;

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;
use clap::{Args, Parser, Subcommand, ValueEnum};
use torqa_app::paths;
use torqa_devices::ble::Bluetooth;
use torqa_domain::profile::Profile;
use torqa_domain::units::{BeatsPerMinute, Kilograms, Percent, Watts};
use torqa_domain::workout::Target;
use torqa_physics::{DescentMode, RiderSetup, VirtualGears};
use torqa_routes::{ElevationSource, Route};
use torqa_session::workout::{HeartRateHold, Workout};
use torqa_session::{Ride, RideConfig};
use tracing_subscriber::EnvFilter;

use devices::DeviceArgs;

#[derive(Parser)]
#[command(
    name = "torqa-cli",
    version,
    about = "Headless Torqa: find and ride smart trainers"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List nearby trainers and heart-rate sensors.
    Scan {
        /// Scan duration in seconds.
        #[arg(long, default_value_t = 5)]
        seconds: u64,
    },
    /// Ride a GPX route or a workout, or control the trainer from the keyboard.
    Ride(Box<RideArgs>),
    /// Check virtual gears on the trainer: what it brakes in each gear against what it should.
    GearCheck(Box<gear_check::GearCheckArgs>),
    /// Show length, climbing and elevation source of a GPX route.
    Route(RouteArgs),
    /// Check how a video keeps up on this machine, played against the clock as a ride is.
    Video {
        /// The video file.
        file: PathBuf,
        /// How long to play it, in seconds.
        #[arg(long, default_value_t = 8)]
        seconds: u64,
    },
    /// Show the steps of a workout file (ZWO, ERG, MRC, FIT) or built-in workout.
    Workout {
        /// Workout file, or `builtin:<name>`; `builtins` lists the built-in workouts.
        workout: String,
        /// Your FTP in watts, for steps given as a share of it.
        #[arg(long, default_value_t = 200.0)]
        ftp: f64,
    },
}

#[derive(Args)]
struct RideArgs {
    #[command(flatten)]
    pub(crate) devices: DeviceArgs,
    /// GPX route to ride; without it or a workout, resistance is set from the keyboard.
    #[arg(long)]
    route: Option<PathBuf>,
    #[command(flatten)]
    workout: WorkoutArgs,
    /// Trainer difficulty in percent: how much of the road gradient you feel.
    #[arg(long, default_value_t = 50.0)]
    difficulty: f64,
    /// How descents behave.
    #[arg(long, value_enum, default_value_t = Descent::Coast)]
    descent: Descent,
    /// Rider plus bike mass in kg.
    #[arg(long, default_value_t = 83.0)]
    mass: f64,
    /// Only use cached terrain data.
    #[arg(long)]
    offline: bool,
    /// Where to save the FIT activity (default: torqa-<date>-<time>.fit).
    #[arg(long)]
    output: Option<PathBuf>,
    /// Virtual gears for a single cog, as chainring x cog teeth (e.g. 50x14): type u / d and
    /// Enter to shift up and down. Without it you shift on the bike.
    #[arg(long, value_parser = parse_gears)]
    gears: Option<(u8, u8)>,
    /// Runs simulated time faster, for testing routes quickly with the fake trainer. Not for
    /// heart-rate workouts: the simulated heart beats in real time.
    #[arg(long, default_value_t = 1.0, requires = "fake", conflicts_with_all = ["hr_zone", "hr_target"])]
    time_scale: f64,
}

/// Workouts without a route (R56): the trainer holds a power (ERG).
#[derive(Args)]
struct WorkoutArgs {
    /// Constant-power workout: the trainer holds this many watts.
    #[arg(long, conflicts_with_all = ["route", "hr_zone", "hr_target"])]
    power: Option<f64>,
    /// Heart-rate workout holding the middle of this heart-rate zone (1–5).
    #[arg(long, value_parser = clap::value_parser!(u8).range(1..=5),
          conflicts_with_all = ["route", "hr_target"])]
    hr_zone: Option<u8>,
    /// Heart-rate workout holding this heart rate, in bpm.
    #[arg(long, conflicts_with = "route")]
    hr_target: Option<f64>,
    /// Structured workout: a ZWO, ERG, MRC or FIT workout file, or `builtin:<name>`.
    #[arg(long, conflicts_with_all = ["route", "power", "hr_zone", "hr_target"])]
    workout: Option<String>,
    /// The least power a heart-rate workout asks for, in watts; it starts there.
    #[arg(long, default_value_t = 100.0)]
    min_power: f64,
    /// The most power a heart-rate workout asks for, in watts.
    #[arg(long, default_value_t = 250.0)]
    max_power: f64,
    /// Your FTP in watts: how much power a beat off target is worth.
    #[arg(long, default_value_t = 200.0)]
    ftp: f64,
    /// Your maximum heart rate in bpm: the base of the heart-rate zones.
    #[arg(long, default_value_t = 185.0)]
    max_hr: f64,
}

impl WorkoutArgs {
    fn workout(&self) -> Result<Option<Workout>> {
        let rider = Profile {
            ftp: Watts(self.ftp),
            max_heart_rate: BeatsPerMinute(self.max_hr),
            ..Profile::default()
        };
        let (min, max) = (Watts(self.min_power), Watts(self.max_power));
        Ok(if let Some(power) = self.power {
            Some(Workout::ConstantPower(Watts(power)))
        } else if let Some(zone) = self.hr_zone {
            Some(Workout::HeartRate(HeartRateHold::zone(
                &rider, zone, min, max,
            )))
        } else if let Some(bpm) = self.hr_target {
            Some(Workout::HeartRate(HeartRateHold::bpm(
                &rider,
                BeatsPerMinute(bpm),
                min,
                max,
            )))
        } else if let Some(id) = &self.workout {
            let plan = torqa_workouts::load(id)?;
            println!("{}: {}", plan.name, recorded_ride::clock(plan.duration()));
            Some(Workout::Structured {
                plan: std::sync::Arc::new(plan),
                ftp: rider.ftp,
            })
        } else {
            None
        })
    }
}

#[derive(Args)]
struct RouteArgs {
    /// GPX file.
    file: PathBuf,
    /// Only use cached terrain data.
    #[arg(long)]
    offline: bool,
    /// Also generate the 3D world and report its size and timings.
    #[arg(long)]
    world: bool,
}

#[derive(Clone, Copy, ValueEnum)]
enum Descent {
    /// Gravity builds speed and the trainer goes light.
    Coast,
    /// Descents are ridden like flat roads.
    Flat,
}

impl From<Descent> for DescentMode {
    fn from(descent: Descent) -> Self {
        match descent {
            Descent::Coast => Self::Coast,
            Descent::Flat => Self::Flat,
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    match Cli::parse().command {
        Command::Scan { seconds } => scan(seconds).await,
        Command::Ride(args) => ride(*args).await,
        Command::GearCheck(args) => gear_check(&args).await,
        Command::Route(args) => route_info(&args).await,
        Command::Video { file, seconds } => video_check(&file, seconds),
        Command::Workout { workout, ftp } => workout_info(&workout, Watts(ftp)),
    }
}

async fn scan(seconds: u64) -> Result<()> {
    let bluetooth = Bluetooth::new().await?;
    println!("Scanning for {seconds} s…");
    let devices = bluetooth.scan(Duration::from_secs(seconds)).await?;
    if devices.is_empty() {
        println!("No trainers or heart-rate sensors found.");
    }
    for device in &devices {
        let rssi = device
            .rssi
            .map_or_else(|| "?".to_owned(), |rssi| format!("{rssi} dBm"));
        println!(
            "{:<10} {:>8}  {}",
            devices::kind_label(device.kind),
            rssi,
            device.name
        );
    }
    Ok(())
}

async fn ride(args: RideArgs) -> Result<()> {
    let route = match &args.route {
        Some(path) => {
            let route = load_route(path, args.offline).await?;
            print_route(&route);
            Some(route)
        }
        None => None,
    };

    let workout = args.workout.workout()?;
    if matches!(workout, Some(Workout::HeartRate(_))) && !args.devices.heart_rate() {
        println!("A heart-rate workout needs a heart rate: add --hr for your strap.");
    }

    let mut devices = devices::connect(&args.devices).await?;
    let config = RideConfig {
        setup: RiderSetup {
            mass: Kilograms(args.mass),
            ..RiderSetup::default()
        },
        difficulty: Percent(args.difficulty),
        descent: args.descent.into(),
        gears: args
            .gears
            .map(|(chainring, cog)| VirtualGears::new(chainring, cog)),
    };
    let result = match (route, workout) {
        (Some(route), _) => recorded_ride::run(Ride::new(route, config), &args, &mut devices).await,
        (None, Some(workout)) => {
            recorded_ride::run(Ride::workout(workout, config), &args, &mut devices).await
        }
        (None, None) => free_ride::run(&mut devices).await,
    };
    devices.close().await;
    result
}

async fn gear_check(args: &gear_check::GearCheckArgs) -> Result<()> {
    let mut devices = devices::connect(&args.devices).await?;
    let result = gear_check::run(args, &mut devices).await;
    devices.close().await;
    result
}

/// `50x14`: chainring and cog teeth.
fn parse_gears(text: &str) -> Result<(u8, u8), String> {
    let (chainring, cog) = text
        .split_once(['x', 'X', '/'])
        .ok_or_else(|| "chainring x cog, e.g. 50x14".to_owned())?;
    let teeth = |t: &str| {
        t.trim()
            .parse::<u8>()
            .ok()
            .filter(|&n| n > 0)
            .ok_or_else(|| format!("{t:?} is not a number of teeth"))
    };
    Ok((teeth(chainring)?, teeth(cog)?))
}

fn workout_info(id: &str, ftp: Watts) -> Result<()> {
    if id == "builtins" {
        for entry in torqa_workouts::builtins() {
            println!(
                "{:<28} {:>6}  {}",
                entry.id,
                recorded_ride::clock(entry.plan.duration()),
                entry.plan.name
            );
        }
        return Ok(());
    }
    let plan = torqa_workouts::load(id)?;
    println!("{} ({})", plan.name, recorded_ride::clock(plan.duration()));
    if !plan.description.is_empty() {
        println!("{}", plan.description);
    }
    let mut start = std::time::Duration::ZERO;
    for step in &plan.steps {
        let target = match step.target {
            Target::Power { from, to } if from == to => format!("{:.0} W", from.watts(ftp).0),
            Target::Power { from, to } => {
                format!("{:.0} → {:.0} W", from.watts(ftp).0, to.watts(ftp).0)
            }
            Target::Free => "free".to_owned(),
        };
        let cadence = step
            .cadence
            .map(|c| format!(" at {:.0} rpm", c.0))
            .unwrap_or_default();
        println!(
            "{:>6}  {:>6}  {target}{cadence}",
            recorded_ride::clock(start),
            recorded_ride::clock(step.duration)
        );
        start += step.duration;
    }
    for cue in &plan.cues {
        println!("{:>6}  “{}”", recorded_ride::clock(cue.at), cue.text);
    }
    Ok(())
}

fn video_check(file: &Path, seconds: u64) -> Result<()> {
    let report = torqa_video::benchmark(file, seconds)?;
    let info = report.info;
    let length = info.duration.as_secs();
    println!(
        "{}×{}, {:.1} frames/s, {}:{:02}",
        info.width,
        info.height,
        info.frame_rate,
        length / 60,
        length % 60
    );
    println!(
        "played {:.0} s against the clock: {} frames ({:.1}/s), longest wait {:.2} s, median {:.3} s",
        report.played.as_secs_f64(),
        report.delivered,
        report.delivered_per_second(),
        report.longest_wait.as_secs_f64(),
        report.median_wait.as_secs_f64()
    );
    println!(
        "straight decoding: {:.1} frames/s",
        report.straight_per_second
    );
    println!(
        "{}",
        if report.keeps_up() {
            "keeps up: a ride along this video runs smoothly here"
        } else {
            "cannot keep up: a ride along this video stutters here (see docs/video.md)"
        }
    );
    Ok(())
}

async fn route_info(args: &RouteArgs) -> Result<()> {
    let started = std::time::Instant::now();
    let imported = torqa_app::import_route(
        &args.file,
        &paths::cache_dir(),
        args.offline,
        &torqa_domain::files::UsedFiles::default(),
        &mut |_, _, _| {},
    )
    .await
    .map_err(anyhow::Error::msg)?;
    print_route(&imported.route);
    let route = &imported.route;
    let (low, high) = route
        .points()
        .iter()
        .fold((f64::MAX, f64::MIN), |(lo, hi), p| {
            (lo.min(p.elevation.0), hi.max(p.elevation.0))
        });
    let steepest = route
        .points()
        .windows(2)
        .max_by(|a, b| {
            let grade = |w: &[torqa_routes::RoutePoint]| {
                (w[1].elevation.0 - w[0].elevation.0)
                    / (w[1].distance.0 - w[0].distance.0).max(0.01)
            };
            grade(a).total_cmp(&grade(b))
        })
        .map_or(0.0, |w| w[0].distance.0);
    println!(
        "Elevation {low:.0}–{high:.0} m, steepest at {:.2} km",
        steepest / 1000.0
    );
    let map = &imported.map;
    println!(
        "Map: {} buildings, {} areas, {} waterways, {} roads (import {:.1} s)",
        map.buildings.len(),
        map.areas.len(),
        map.waterways.len(),
        map.roads.len(),
        started.elapsed().as_secs_f64()
    );
    if args.world {
        let started = std::time::Instant::now();
        let mut terrain = torqa_terrain::Terrain::new(
            torqa_terrain::TileSource::defaults(),
            paths::cache_dir().join("terrain"),
        );
        if args.offline {
            terrain = terrain.offline();
        }
        let world = torqa_world::generate(&imported.route, &mut terrain, map, &mut |_, _| {}).await;
        let triangles = |m: &torqa_world::MeshData| m.indices.len() / 3;
        println!(
            "World: {} chunks, {} terrain / {} building triangles, {} trees ({:.1} s)",
            world.chunks.len(),
            world
                .chunks
                .iter()
                .map(|c| triangles(&c.mesh))
                .sum::<usize>(),
            world
                .chunks
                .iter()
                .map(|c| triangles(&c.buildings))
                .sum::<usize>(),
            world.chunks.iter().map(|c| c.trees.len()).sum::<usize>(),
            started.elapsed().as_secs_f64()
        );
    }
    Ok(())
}

async fn load_route(path: &std::path::Path, offline: bool) -> Result<Route> {
    let imported = torqa_app::import_route(
        path,
        &paths::cache_dir(),
        offline,
        &torqa_domain::files::UsedFiles::default(),
        &mut |_, _, _| {},
    )
    .await
    .map_err(anyhow::Error::msg)?;
    Ok(imported.route)
}

fn print_route(route: &Route) {
    let source = match route.elevation_source() {
        ElevationSource::Terrain => "terrain model (Mapterhorn / AWS Terrain Tiles)",
        ElevationSource::File => "GPX file",
    };
    println!(
        "{}: {:.2} km, {:.0} m climbing, steepest {:.1} %, elevation from {source}",
        route.name().unwrap_or("Route"),
        route.length().0 / 1000.0,
        route.elevation_gain().0,
        route.max_grade().0
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_definition_is_valid() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }

    #[test]
    fn workouts_ride_without_a_route_one_at_a_time() {
        let parse = |args: &[&str]| Cli::try_parse_from([&["torqa-cli", "ride"], args].concat());

        assert!(parse(&["--power", "200"]).is_ok());
        assert!(parse(&["--hr-zone", "3", "--min-power", "120", "--max-power", "220"]).is_ok());
        assert!(parse(&["--hr-target", "140", "--max-hr", "190"]).is_ok());
        assert!(parse(&["--hr-zone", "6"]).is_err());
        assert!(parse(&["--power", "200", "--hr-zone", "3"]).is_err());
        assert!(parse(&["--power", "200", "--route", "a.gpx"]).is_err());
        assert!(parse(&["--fake", "--hr-zone", "3", "--time-scale", "10"]).is_err());
        assert!(parse(&["--fake", "--power", "200", "--time-scale", "10"]).is_ok());
        assert!(parse(&["--workout", "builtin:vo2max-5x3"]).is_ok());
        assert!(parse(&["--workout", "a.zwo", "--power", "200"]).is_err());
        assert!(parse(&["--gears", "50x14"]).is_ok());
        assert!(parse(&["--gears", "50"]).is_err());
        assert_eq!(parse_gears("34/14"), Ok((34, 14)));
        assert!(
            Cli::try_parse_from(["torqa-cli", "gear-check", "--fake", "--gears", "34x14"]).is_ok()
        );
    }

    #[test]
    fn a_zone_workout_holds_the_middle_of_the_zone() {
        let Ok(Cli {
            command: Command::Ride(args),
        }) = Cli::try_parse_from(["torqa-cli", "ride", "--hr-zone", "3", "--max-hr", "200"])
        else {
            panic!("valid arguments");
        };

        let Ok(Some(Workout::HeartRate(hold))) = args.workout.workout() else {
            panic!("a heart-rate workout");
        };
        assert!((hold.target.0 - 150.0).abs() < 1e-9);
        assert_eq!(
            (hold.min_power, hold.max_power),
            (Watts(100.0), Watts(250.0))
        );
    }

    #[test]
    fn time_scale_requires_the_fake_trainer() {
        assert!(Cli::try_parse_from(["torqa-cli", "ride", "--time-scale", "10"]).is_err());
        assert!(Cli::try_parse_from(["torqa-cli", "ride", "--fake", "--time-scale", "10"]).is_ok());
    }
}
