//! The ride engine: moves the virtual rider along a route from measured power, tells the
//! trainer which gradient to simulate — or, in a workout, which power to hold — and records
//! the ride.
//!
//! It is pure logic driven by [`Ride::tick`], independent of devices, threads and rendering, so
//! the same engine runs headless in the CLI, in tests and behind the 3D world.

pub mod analysis;
pub mod ghost;
pub mod workout;

use std::time::Duration;

use torqa_domain::recording::{Location, Sample};
use torqa_domain::shifting::Shift;
use torqa_domain::telemetry::{Telemetry, TrainerControl};
use torqa_domain::units::{GradePercent, Meters, MetersPerSecond, Percent, Watts};
use torqa_physics::{DescentMode, GEARS, Motion, RiderSetup, VirtualGears, trainer_grade};
use torqa_routes::{Route, RoutePosition};
use workout::{Workout, WorkoutControl, WorkoutState};

/// Trainers need time to change resistance; more frequent updates only add traffic.
const CONTROL_INTERVAL: Duration = Duration::from_secs(1);
/// Grade changes smaller than this are not worth a trainer update.
const GRADE_UPDATE_THRESHOLD: f64 = 0.1;
/// Nor are smaller changes of the target power (ERG targets are whole watts).
const POWER_UPDATE_THRESHOLD: f64 = 1.0;
const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);

/// Settings for one ride.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RideConfig {
    /// Rider and bike.
    pub setup: RiderSetup,
    /// Trainer difficulty (R14).
    pub difficulty: Percent,
    /// Descent behaviour (R15).
    pub descent: DescentMode,
    /// Virtual gears on a single cog (R9); `None` with a cassette.
    pub gears: Option<VirtualGears>,
}

impl Default for RideConfig {
    /// 50 % trainer difficulty, as popular platforms default to, coasting descents and a
    /// cassette.
    fn default() -> Self {
        Self {
            setup: RiderSetup::default(),
            difficulty: Percent(50.0),
            descent: DescentMode::Coast,
            gears: None,
        }
    }
}

/// The virtual gear ridden.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gear {
    /// Which, counted from 1.
    pub number: usize,
    /// How many there are.
    pub of: usize,
    /// Its ratio, as chainring over cog.
    pub ratio: f64,
}

/// A snapshot for display.
#[derive(Debug, Clone, PartialEq)]
pub struct RideState {
    /// Time since the start.
    pub elapsed: Duration,
    /// Distance covered.
    pub distance: Meters,
    /// Distance left to the finish; `None` without a route.
    pub remaining: Option<Meters>,
    /// Virtual speed.
    pub speed: MetersPerSecond,
    /// Where the rider is on the route; `None` without one.
    pub position: Option<RoutePosition>,
    /// Latest measurements.
    pub telemetry: Telemetry,
    /// What the workout asks for, in a workout.
    pub workout: Option<WorkoutState>,
    /// The virtual gear, with virtual gears.
    pub gear: Option<Gear>,
}

/// A ride along a route, or a workout on a flat road without one.
#[derive(Debug, Clone)]
pub struct Ride {
    route: Option<Route>,
    workout: Option<WorkoutControl>,
    config: RideConfig,
    motion: Motion,
    distance: Meters,
    elapsed: Duration,
    telemetry: Telemetry,
    samples: Vec<Sample>,
    next_sample: Duration,
    /// The control sent last and when; `None` sends the next one at once.
    last_control: Option<(TrainerControl, Duration)>,
    /// The virtual gear (index from 0), with virtual gears.
    gear: usize,
}

impl Ride {
    /// Starts a ride at the beginning of `route`; the trainer follows its gradient.
    #[must_use]
    pub fn new(route: Route, config: RideConfig) -> Self {
        Self::start(Some(route), None, config)
    }

    /// Starts a workout without a route (R56): the trainer holds the power the workout asks
    /// for, and the rider's speed and distance are those on a flat road. It goes on until the
    /// rider ends it, or a structured workout (R21) has run its last step.
    #[must_use]
    pub fn workout(workout: Workout, config: RideConfig) -> Self {
        Self::start(None, Some(WorkoutControl::new(workout)), config)
    }

    /// Makes the ride along a route a workout (R58): the trainer holds the workout's power
    /// instead of following the gradient, which still sets the rider's speed. It ends at the
    /// finish, as any ride along the route.
    #[must_use]
    pub fn with_workout(mut self, workout: Workout) -> Self {
        self.workout = Some(WorkoutControl::new(workout));
        self.last_control = None;
        self
    }

    fn start(route: Option<Route>, workout: Option<WorkoutControl>, config: RideConfig) -> Self {
        Self {
            route,
            workout,
            config,
            motion: Motion::default(),
            distance: Meters(0.0),
            elapsed: Duration::ZERO,
            telemetry: Telemetry::default(),
            samples: Vec::new(),
            next_sample: Duration::ZERO,
            last_control: None,
            gear: config.gears.map_or(0, |gears| gears.neutral()),
        }
    }

    /// Feeds new measurements from a device.
    pub fn on_telemetry(&mut self, telemetry: &Telemetry) {
        self.telemetry.merge(telemetry);
    }

    /// Forgets the power source's last values, e.g. when the trainer disconnects, so the rider
    /// does not keep riding on stale power.
    pub fn on_power_source_lost(&mut self) {
        self.telemetry.power = None;
        self.telemetry.cadence = None;
        self.telemetry.speed = None;
    }

    /// Advances the ride by `dt`. Returns a control for the trainer when the simulated gradient
    /// or the workout's power should change.
    pub fn tick(&mut self, dt: Duration) -> Option<TrainerControl> {
        if self.is_finished() {
            return None;
        }
        // Samples sit on a whole-second grid, so FIT timestamps are exact.
        if self.elapsed >= self.next_sample {
            self.record();
            self.next_sample += SAMPLE_INTERVAL;
        }

        let grade = self.road_grade();
        let power = self.telemetry.power.unwrap_or(Watts(0.0));
        let covered = self
            .motion
            .step(&self.config.setup, power, grade, MetersPerSecond(0.0), dt);
        self.distance = Meters(self.distance.0 + covered.0);
        if let Some(route) = &self.route {
            self.distance = Meters(self.distance.0.min(route.length().0));
        }
        self.elapsed += dt;
        if let Some(workout) = &mut self.workout {
            workout.update(&self.telemetry, dt);
        }

        if self.is_finished() {
            self.record();
            return None;
        }
        self.control_update()
    }

    /// Has the trainer told its gradient or power again on the next tick: after a pause it
    /// was freed of its resistance, so the last control sent no longer holds.
    pub fn resend_control(&mut self) {
        self.last_control = None;
    }

    /// Changes trainer difficulty and descent mode during the ride (R48); the trainer gets the
    /// new gradient on the next tick rather than at the next regular update.
    pub fn adjust(&mut self, difficulty: Percent, descent: DescentMode) {
        self.config.difficulty = difficulty;
        self.config.descent = descent;
        self.last_control = None;
    }

    /// Changes the workout during a workout, e.g. a new target power or heart rate; the
    /// trainer hears of it on the next tick. A ride along a route is left as it is.
    pub fn change_workout(&mut self, workout: Workout) {
        if let Some(control) = &mut self.workout {
            control.change(workout);
            self.last_control = None;
        }
    }

    /// Shifts the virtual gears (R9); the trainer feels the new gear at once. Without virtual
    /// gears, and in ERG (where the trainer holds the power in any gear), nothing changes.
    pub fn shift(&mut self, shift: Shift) {
        if self.config.gears.is_none() {
            return;
        }
        let gear = match shift {
            Shift::Up => self.gear + 1,
            Shift::Down => self.gear.saturating_sub(1),
            Shift::To(number) => number.saturating_sub(1),
        }
        .min(GEARS - 1);
        if gear != self.gear {
            self.gear = gear;
            self.last_control = None;
        }
    }

    /// Moves the rider to `distance` along the route, keeping their speed — for simulated
    /// rides (#53). The trainer gets the gradient there on the next tick.
    pub fn jump_to(&mut self, distance: Meters) {
        let end = self.route.as_ref().map_or(f64::INFINITY, |r| r.length().0);
        self.distance = Meters(distance.0.clamp(0.0, end));
        self.last_control = None;
    }

    /// Whether the rider has reached the end of the route; without one, whether a structured
    /// workout has run its last step (other workouts are never finished by themselves).
    #[must_use]
    pub fn is_finished(&self) -> bool {
        match &self.route {
            Some(route) => self.distance.0 >= route.length().0,
            None => self
                .workout
                .as_ref()
                .is_some_and(WorkoutControl::is_finished),
        }
    }

    /// Current state for display.
    #[must_use]
    pub fn state(&self) -> RideState {
        RideState {
            elapsed: self.elapsed,
            distance: self.distance,
            remaining: self
                .route
                .as_ref()
                .map(|route| Meters(route.length().0 - self.distance.0)),
            speed: self.motion.speed(),
            position: self.route.as_ref().map(|r| r.position(self.distance)),
            telemetry: self.telemetry,
            workout: self.workout.as_ref().map(WorkoutControl::state),
            gear: self.config.gears.map(|gears| Gear {
                number: self.gear + 1,
                of: GEARS,
                ratio: gears.ratio(self.gear),
            }),
        }
    }

    /// The FTP this ride shows if it is an FTP test (R22), from its samples so far; `None` for
    /// other rides and for a test left before its efforts were done.
    #[must_use]
    pub fn ftp_estimate(&self) -> Option<Watts> {
        self.workout.as_ref()?.ftp_estimate(&self.samples)
    }

    /// The route being ridden; `None` in a workout without one.
    #[must_use]
    pub fn route(&self) -> Option<&Route> {
        self.route.as_ref()
    }

    /// The recorded samples, one per second.
    #[must_use]
    pub fn samples(&self) -> &[Sample] {
        &self.samples
    }

    /// The gradient used for physics: the road, adjusted for the descent mode; flat without a
    /// route.
    fn road_grade(&self) -> GradePercent {
        self.route.as_ref().map_or(GradePercent(0.0), |route| {
            self.config
                .descent
                .apply(route.position(self.distance).grade)
        })
    }

    fn control_update(&mut self) -> Option<TrainerControl> {
        // A free step of a structured workout simulates the road like a ride without one.
        let control = if let Some(power) = self.workout.as_ref().and_then(WorkoutControl::power) {
            TrainerControl::TargetPower(power)
        } else {
            let grade = trainer_grade(self.road_grade(), self.config.difficulty);
            let road = self.config.setup.simulation_parameters(grade);
            TrainerControl::Simulation(match self.config.gears {
                Some(gears) => gears.in_gear(road, self.gear),
                None => road,
            })
        };
        let due = match &self.last_control {
            None => true,
            Some((sent, at)) => {
                self.elapsed.saturating_sub(*at) >= CONTROL_INTERVAL
                    && worth_sending(sent, &control)
            }
        };
        if !due {
            return None;
        }
        self.last_control = Some((control, self.elapsed));
        Some(control)
    }

    fn record(&mut self) {
        let location = self.route.as_ref().map(|route| {
            let position = route.position(self.distance);
            Location {
                lat: position.lat,
                lon: position.lon,
                elevation: position.elevation,
                grade: position.grade,
            }
        });
        self.samples.push(Sample {
            elapsed: self.elapsed,
            location,
            distance: self.distance,
            speed: self.motion.speed(),
            power: self.telemetry.power,
            cadence: self.telemetry.cadence,
            heart_rate: self.telemetry.heart_rate,
        });
    }
}

/// Whether `next` differs enough from the control `sent` last to be sent.
fn worth_sending(sent: &TrainerControl, next: &TrainerControl) -> bool {
    match (sent, next) {
        (TrainerControl::Simulation(a), TrainerControl::Simulation(b)) => {
            // A shift changes the resistance coefficients too, not only the grade.
            (a.grade.0 - b.grade.0).abs() >= GRADE_UPDATE_THRESHOLD
                || (a.crr - b.crr).abs() > 1e-9
                || (a.cw.0 - b.cw.0).abs() > 1e-9
        }
        (TrainerControl::TargetPower(a), TrainerControl::TargetPower(b)) => {
            (a.0 - b.0).abs() >= POWER_UPDATE_THRESHOLD
        }
        _ => sent != next,
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use std::sync::Arc;
    use torqa_devices::DeviceEvent;
    use torqa_devices::fake::{self, FakeHeart, FakeRider, SimulatedHeart};
    use torqa_domain::profile::Profile;

    use torqa_domain::units::{BeatsPerMinute, Rpm};
    use torqa_domain::workout::{Cue, Intensity, Plan, Step, Target};
    use workout::HeartRateHold;

    use super::*;

    /// A straight route due north: one segment per grade, each `length` metres.
    async fn route(grades: &[f64], length: f64) -> Route {
        let degrees_per_meter = 180.0 / (std::f64::consts::PI * 6_371_000.0);
        let mut xml = String::from("<gpx><trk><trkseg>");
        let mut elevation = 500.0;
        let mut add = |i: usize, elevation: f64| {
            #[allow(clippy::cast_precision_loss)]
            let lat = 46.0 + i as f64 * 10.0 * degrees_per_meter;
            let _ = write!(
                xml,
                r#"<trkpt lat="{lat}" lon="7"><ele>{elevation}</ele></trkpt>"#
            );
        };
        let mut i = 0;
        add(i, elevation);
        for grade in grades {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            for _ in 0..(length / 10.0) as usize {
                i += 1;
                elevation += grade / 100.0 * 10.0;
                add(i, elevation);
            }
        }
        xml.push_str("</trkseg></trk></gpx>");
        Route::from_gpx(&xml, None).await.unwrap()
    }

    fn pedal(ride: &mut Ride, power: f64, seconds: u32) -> Vec<TrainerControl> {
        ride.on_telemetry(&Telemetry {
            power: Some(Watts(power)),
            cadence: Some(Rpm(90.0)),
            ..Telemetry::default()
        });
        (0..seconds * 4)
            .filter_map(|_| ride.tick(Duration::from_millis(250)))
            .collect()
    }

    fn grade_of(control: &TrainerControl) -> f64 {
        match control {
            TrainerControl::Simulation(p) => p.grade.0,
            other => panic!("unexpected control {other:?}"),
        }
    }

    #[tokio::test]
    async fn rides_along_the_route_at_physics_speed() {
        let mut ride = Ride::new(route(&[0.0], 5000.0).await, RideConfig::default());

        pedal(&mut ride, 250.0, 120);

        // Accelerating from standstill to ~37 km/h, about 1.2 km in two minutes.
        let distance = ride.state().distance.0;
        assert!((1100.0..1300.0).contains(&distance), "{distance} m");
    }

    #[tokio::test]
    async fn trainer_feels_the_scaled_gradient() {
        let mut ride = Ride::new(route(&[6.0], 2000.0).await, RideConfig::default());

        let controls = pedal(&mut ride, 250.0, 5);

        // 6 % at the default 50 % difficulty.
        assert!((grade_of(&controls[0]) - 3.0).abs() < 0.1, "{controls:?}");
    }

    #[tokio::test]
    async fn grade_updates_are_throttled_and_skip_unchanged_grades() {
        let mut ride = Ride::new(route(&[4.0], 3000.0).await, RideConfig::default());

        let controls = pedal(&mut ride, 250.0, 30);

        assert_eq!(
            controls.len(),
            1,
            "steady climb needs one update: {controls:?}"
        );
    }

    #[tokio::test]
    async fn grade_changes_reach_the_trainer() {
        let config = RideConfig {
            difficulty: Percent(100.0),
            ..RideConfig::default()
        };
        let mut ride = Ride::new(route(&[0.0, 8.0], 300.0).await, config);

        let controls = pedal(&mut ride, 300.0, 120);

        let last = controls.last().map(grade_of).unwrap();
        assert!((last - 8.0).abs() < 0.5, "{controls:?}");
    }

    #[tokio::test]
    async fn a_jump_moves_the_rider_and_the_trainer_feels_the_road_there() {
        let config = RideConfig {
            difficulty: Percent(100.0),
            ..RideConfig::default()
        };
        let mut ride = Ride::new(route(&[0.0, 8.0], 1000.0).await, config);
        pedal(&mut ride, 250.0, 10);
        let speed = ride.state().speed;

        ride.jump_to(Meters(1500.0));
        let controls = pedal(&mut ride, 250.0, 1);

        assert!((ride.state().distance.0 - 1500.0).abs() < 15.0);
        // Not braked by the jump, and the trainer gets the 8 % climb at once.
        assert!(ride.state().speed.0 > speed.0 * 0.5);
        assert!((grade_of(&controls[0]) - 8.0).abs() < 0.5, "{controls:?}");
        // Beyond the end means the end.
        ride.jump_to(Meters(5000.0));
        assert!(ride.is_finished());
    }

    #[tokio::test]
    async fn difficulty_changes_reach_the_trainer_at_once() {
        let config = RideConfig {
            difficulty: Percent(100.0),
            ..RideConfig::default()
        };
        let mut ride = Ride::new(route(&[8.0], 5000.0).await, config);
        pedal(&mut ride, 300.0, 30);

        ride.adjust(Percent(50.0), DescentMode::Coast);
        let controls = pedal(&mut ride, 300.0, 1);

        let first = controls.first().map(grade_of).unwrap();
        assert!((first - 4.0).abs() < 0.3, "{controls:?}");
    }

    #[tokio::test]
    async fn records_one_sample_per_second() {
        let mut ride = Ride::new(route(&[0.0], 5000.0).await, RideConfig::default());

        pedal(&mut ride, 200.0, 10);

        let samples = ride.samples();
        assert_eq!(samples.len(), 10);
        assert_eq!(samples[3].elapsed, Duration::from_secs(3));
        assert_eq!(samples[3].power, Some(Watts(200.0)));
    }

    #[tokio::test]
    async fn stops_at_the_finish() {
        let mut ride = Ride::new(route(&[0.0], 200.0).await, RideConfig::default());

        pedal(&mut ride, 300.0, 120);

        assert!(ride.is_finished());
        assert!((ride.state().distance.0 - ride.route().unwrap().length().0).abs() < 1e-9);
        let recorded = ride.samples().len();
        pedal(&mut ride, 300.0, 5);
        assert_eq!(
            ride.samples().len(),
            recorded,
            "no recording after the finish"
        );
    }

    #[tokio::test]
    async fn lost_power_source_stops_propulsion() {
        let mut ride = Ride::new(route(&[0.0], 5000.0).await, RideConfig::default());
        pedal(&mut ride, 250.0, 60);

        ride.on_power_source_lost();
        for _ in 0..1200 {
            ride.tick(Duration::from_millis(250));
        }

        assert!(ride.state().speed.0 < 1.0, "{:?}", ride.state().speed);
    }

    #[test]
    fn a_constant_power_workout_holds_its_power_on_a_flat_road() {
        let mut ride = Ride::workout(Workout::ConstantPower(Watts(250.0)), RideConfig::default());

        // The rider's own power is what the trainer holds in ERG.
        let controls = pedal(&mut ride, 250.0, 120);

        assert_eq!(controls, [TrainerControl::TargetPower(Watts(250.0))]);
        // As far as 250 W carry on the flat: about 1.2 km in two minutes.
        let state = ride.state();
        assert!((1100.0..1300.0).contains(&state.distance.0), "{state:?}");
        assert_eq!(state.remaining, None);
        assert_eq!(state.position, None);
        assert_eq!(
            state.workout.and_then(|w| w.target_power),
            Some(Watts(250.0))
        );
        assert!(!ride.is_finished(), "goes on until the rider ends it");
        assert!(ride.samples().iter().all(|s| s.location.is_none()));
    }

    #[tokio::test]
    async fn a_workout_on_a_route_holds_its_power_while_the_road_sets_the_speed() {
        let route = route(&[0.0, 8.0], 1000.0).await;
        let workout = Workout::ConstantPower(Watts(250.0));
        let mut flat = Ride::workout(workout.clone(), RideConfig::default());
        let mut hilly = Ride::new(route, RideConfig::default()).with_workout(workout);

        pedal(&mut flat, 250.0, 300);
        let controls = pedal(&mut hilly, 250.0, 300);

        assert_eq!(controls, [TrainerControl::TargetPower(Watts(250.0))]);
        // The same power climbs the 8 % half slower than the flat road goes.
        assert!(hilly.state().distance.0 < flat.state().distance.0 * 0.9);
        assert!(hilly.state().position.is_some() && hilly.state().workout.is_some());
        pedal(&mut hilly, 250.0, 600);
        assert!(hilly.is_finished(), "ends at the finish");
    }

    /// A minute at half of FTP, half a minute from 100 W to 160 W, a free minute.
    fn plan() -> Arc<Plan> {
        Arc::new(Plan {
            steps: vec![
                Step {
                    duration: Duration::from_mins(1),
                    target: Target::steady(Intensity::Ftp(0.5)),
                    cadence: Some(Rpm(85.0)),
                },
                Step {
                    duration: Duration::from_secs(30),
                    target: Target::Power {
                        from: Intensity::Watts(Watts(100.0)),
                        to: Intensity::Watts(Watts(160.0)),
                    },
                    cadence: None,
                },
                Step {
                    duration: Duration::from_mins(1),
                    target: Target::Free,
                    cadence: None,
                },
            ],
            cues: vec![Cue {
                at: Duration::from_secs(65),
                text: "Ramp".to_owned(),
            }],
            ..Plan::default()
        })
    }

    #[test]
    fn a_structured_workout_follows_its_steps_and_ends_after_the_last() {
        let workout = Workout::Structured {
            plan: plan(),
            ftp: Watts(240.0),
        };
        let mut ride = Ride::workout(workout, RideConfig::default());

        let first = pedal(&mut ride, 150.0, 70);
        let progress = ride.state().workout.and_then(|w| w.progress).unwrap();
        let rest = pedal(&mut ride, 150.0, 120);

        assert_eq!(
            first[0],
            TrainerControl::TargetPower(Watts(120.0)),
            "half of 240 W"
        );
        // Ten seconds into the ramp: a third of the way from 100 to 160 W, rising each second.
        let power = ride_power(first.last().unwrap());
        assert!((117.0..=121.0).contains(&power), "{first:?}");
        assert_eq!((progress.step, progress.steps), (1, 3));
        assert_eq!(progress.step_left, Duration::from_secs(20));
        assert_eq!(progress.left, Duration::from_secs(80));
        assert_eq!(
            progress.next.map(|n| n.power),
            Some(None),
            "a free step next"
        );
        assert_eq!(progress.cue.as_deref(), Some("Ramp"));
        // The free step lets the trainer simulate the flat road.
        assert!(
            matches!(rest.last(), Some(TrainerControl::Simulation(p)) if p.grade.0 == 0.0),
            "{rest:?}"
        );
        assert!(ride.is_finished(), "over after its last step");
        let recorded = ride.samples().last().unwrap().elapsed;
        assert!((Duration::from_secs(150)..Duration::from_secs(152)).contains(&recorded));
    }

    #[tokio::test]
    async fn a_structured_workout_on_a_route_rides_on_to_the_finish() {
        let workout = Workout::Structured {
            plan: plan(),
            ftp: Watts(240.0),
        };
        let mut ride = Ride::new(route(&[0.0, 6.0], 1000.0).await, RideConfig::default())
            .with_workout(workout);

        let controls = pedal(&mut ride, 200.0, 200);

        assert!(!ride.is_finished(), "2 km take longer than the workout");
        assert!(ride.state().workout.and_then(|w| w.target_power).is_none());
        // Back to the road: the 6 % climb at the default 50 % difficulty.
        assert!(
            matches!(controls.last(), Some(TrainerControl::Simulation(p)) if p.grade.0 > 1.0),
            "{controls:?}"
        );
    }

    fn ride_power(control: &TrainerControl) -> f64 {
        match control {
            TrainerControl::TargetPower(p) => p.0,
            other => panic!("not ERG: {other:?}"),
        }
    }

    #[test]
    fn a_ramp_test_rises_every_minute_until_the_cadence_gives_way() {
        let test = workout::RampTest::for_ftp(Watts(250.0));
        assert_eq!(
            (test.warm_up_power, test.start, test.step),
            (Watts(100.0), Watts(125.0), Watts(15.0))
        );
        let mut ride = Ride::workout(Workout::RampTest(test), RideConfig::default());

        // Five minutes of warm-up and nine and a half steps, pedalling at 90 rpm.
        let controls = pedal(&mut ride, 200.0, 14 * 60 + 30);
        let state = ride.state().workout.unwrap();
        // A few seconds below 50 rpm is no failure yet…
        spin(&mut ride, 30.0, 5);
        assert!(!ride.is_finished());
        // …ten seconds is.
        spin(&mut ride, 30.0, 11);

        assert_eq!(controls[0], TrainerControl::TargetPower(Watts(100.0)));
        assert_eq!(
            controls.last(),
            Some(&TrainerControl::TargetPower(Watts(125.0 + 9.0 * 15.0))),
            "the tenth step"
        );
        let progress = state.progress.unwrap();
        assert_eq!((progress.step, progress.steps), (10, 0), "open-ended");
        assert_eq!(progress.step_left, Duration::from_secs(30));
        assert_eq!(progress.next.and_then(|n| n.power), Some(Watts(275.0)));
        assert!(ride.is_finished(), "over once the rider gives way");
    }

    #[test]
    fn the_twenty_minute_test_leaves_its_effort_to_the_rider_and_takes_95_percent() {
        let test = workout::EffortTest::twenty_minutes(Watts(250.0));
        assert_eq!(
            test.plan.duration(),
            Duration::from_mins(59),
            "an hour in all"
        );
        assert_eq!(
            test.effort_times(),
            vec![Duration::from_mins(29)..Duration::from_mins(49)]
        );
        let mut ride = Ride::workout(Workout::EffortTest(test), RideConfig::default());

        let warm_up = pedal(&mut ride, 150.0, 15 * 60 + 30);
        let activation = ride.state().workout.unwrap().target_power;
        pedal(&mut ride, 150.0, 8 * 60);
        let max_minute = ride.state().workout.unwrap();
        // From a second before the test to a second before its end: a tick's control is for
        // where it ends.
        pedal(&mut ride, 150.0, 5 * 60 + 29);
        let test_part = pedal(&mut ride, 260.0, 20 * 60);
        pedal(&mut ride, 260.0, 1);
        pedal(&mut ride, 100.0, 10 * 60 + 5);

        assert!(
            matches!(warm_up[0], TrainerControl::TargetPower(w) if (w.0 - 125.0).abs() < 1.0),
            "from half of FTP: {:?}",
            warm_up[0]
        );
        assert_eq!(activation, Some(Watts(225.0)), "the first minute at 90 %");
        assert!(
            max_minute.target_power.is_none() && max_minute.progress.as_ref().unwrap().all_out,
            "a minute all out: {max_minute:?}"
        );
        assert!(
            !test_part.is_empty()
                && test_part
                    .iter()
                    .all(|c| matches!(c, TrainerControl::Simulation(_))),
            "the rider sets the power in the test: {:?}",
            test_part.first()
        );
        assert!(ride.is_finished(), "over after the cool-down");
        assert_eq!(ride.ftp_estimate(), Some(Watts(247.0)), "95 % of 260 W");
    }

    #[test]
    fn the_two_by_eight_minute_test_takes_90_percent_of_both_efforts() {
        let test = workout::EffortTest::two_by_eight(Watts(250.0));
        assert_eq!(test.plan.duration(), Duration::from_mins(51));
        assert_eq!(
            test.effort_times(),
            vec![
                Duration::from_mins(15)..Duration::from_mins(23),
                Duration::from_mins(33)..Duration::from_mins(41)
            ]
        );
        let mut ride = Ride::workout(Workout::EffortTest(test), RideConfig::default());

        pedal(&mut ride, 150.0, 15 * 60);
        pedal(&mut ride, 300.0, 8 * 60);
        pedal(&mut ride, 120.0, 9 * 60);
        let resting = ride.state().workout.unwrap().progress.unwrap();
        let before_the_second = ride.ftp_estimate();
        pedal(&mut ride, 120.0, 60);
        pedal(&mut ride, 280.0, 8 * 60);
        pedal(&mut ride, 100.0, 10 * 60 + 5);

        assert!(
            !resting.all_out && resting.next.is_some_and(|n| n.all_out),
            "resting before the second effort: {resting:?}"
        );
        assert_eq!(before_the_second, None, "no FTP from half a test");
        assert!(ride.is_finished());
        assert_eq!(ride.ftp_estimate(), Some(Watts(261.0)), "90 % of 290 W");
    }

    /// Rides on at the power held so far, but at `cadence`.
    fn spin(ride: &mut Ride, cadence: f64, seconds: u32) {
        ride.on_telemetry(&Telemetry {
            cadence: Some(Rpm(cadence)),
            ..Telemetry::default()
        });
        for _ in 0..seconds * 4 {
            ride.tick(Duration::from_millis(250));
        }
    }

    #[tokio::test]
    async fn virtual_gears_make_the_road_harder_or_easier_at_once() {
        let config = RideConfig {
            difficulty: Percent(100.0),
            gears: Some(VirtualGears::new(50, 14)),
            ..RideConfig::default()
        };
        let mut ride = Ride::new(route(&[4.0], 3000.0).await, config);
        let first = pedal(&mut ride, 200.0, 5);
        let start = ride.state().gear.unwrap();

        ride.shift(Shift::Up);
        ride.shift(Shift::Up);
        let harder = pedal(&mut ride, 200.0, 1);
        ride.shift(Shift::To(1));
        let easiest = pedal(&mut ride, 200.0, 1);
        ride.shift(Shift::To(99));

        assert_eq!(first.len(), 1, "one control while the road stays the same");
        assert!(
            (start.ratio - 50.0 / 14.0).abs() < 0.2,
            "starts as on the bike: {start:?}"
        );
        assert_eq!(harder.len(), 1, "a shift reaches the trainer at once");
        assert!(
            grade_of(&harder[0]) > grade_of(&first[0]) * 1.1,
            "{harder:?} {first:?}"
        );
        assert!(
            grade_of(&easiest[0]) < grade_of(&first[0]) * 0.4,
            "{easiest:?}"
        );
        assert_eq!(
            ride.state().gear.unwrap().number,
            GEARS,
            "no gear beyond the last"
        );
    }

    #[tokio::test]
    async fn with_a_cassette_the_rider_shifts_on_the_bike() {
        let mut ride = Ride::new(route(&[4.0], 3000.0).await, RideConfig::default());
        pedal(&mut ride, 200.0, 5);

        ride.shift(Shift::Up);

        assert!(pedal(&mut ride, 200.0, 1).is_empty(), "no new control");
        assert_eq!(ride.state().gear, None);
    }

    #[test]
    fn a_changed_workout_reaches_the_trainer_at_once() {
        let mut ride = Ride::workout(Workout::ConstantPower(Watts(200.0)), RideConfig::default());
        pedal(&mut ride, 200.0, 10);

        ride.change_workout(Workout::ConstantPower(Watts(230.0)));
        let controls = pedal(&mut ride, 230.0, 1);

        assert_eq!(controls, [TrainerControl::TargetPower(Watts(230.0))]);
    }

    /// A rider whose heart rate the controller estimates from a 200 W FTP and 185 bpm maximum.
    fn rider() -> Profile {
        Profile {
            ftp: Watts(200.0),
            max_heart_rate: BeatsPerMinute(185.0),
            ..Profile::default()
        }
    }

    /// Rides a heart-rate hold for `minutes` against a simulated heart that beats to the
    /// power the trainer is asked for, as in ERG. Its rate arrives 10 s late, as straps
    /// average over several beats. Returns heart rate and power each second.
    fn hold_against(heart: FakeHeart, hold: HeartRateHold, minutes: u64) -> Vec<(f64, f64)> {
        let mut ride = Ride::workout(Workout::HeartRate(hold), RideConfig::default());
        let mut simulated = SimulatedHeart::new(heart);
        let mut power = Watts(0.0);
        let mut beats = std::collections::VecDeque::new();
        let mut trace = Vec::new();
        for _ in 0..minutes * 60 {
            beats.push_back(simulated.step(power, Duration::from_secs(1)));
            let heart_rate = if beats.len() > 10 {
                beats.pop_front().unwrap()
            } else {
                beats[0]
            };
            ride.on_telemetry(&Telemetry {
                power: Some(power),
                cadence: Some(Rpm(90.0)),
                heart_rate: Some(heart_rate),
                ..Telemetry::default()
            });
            for _ in 0..4 {
                if let Some(TrainerControl::TargetPower(target)) =
                    ride.tick(Duration::from_millis(250))
                {
                    power = target;
                }
            }
            trace.push((heart_rate.0, power.0));
        }
        trace
    }

    /// Checks what R56 asks of a heart-rate hold: power within the limits and changing
    /// gently, heart rate settled at the target within `settle` minutes and not swinging past
    /// it on the way.
    fn assert_holds(trace: &[(f64, f64)], hold: &HeartRateHold, settle: usize) {
        let target = hold.target.0;
        for (second, &(heart_rate, power)) in trace.iter().enumerate() {
            assert!(
                (hold.min_power.0..=hold.max_power.0).contains(&power),
                "{power} W at {second} s"
            );
            assert!(
                heart_rate <= target + 2.0,
                "overshoot: {heart_rate} bpm at {second} s"
            );
            if second >= settle * 60 {
                assert!(
                    (heart_rate - target).abs() <= 3.0,
                    "{heart_rate} bpm at {second} s, target {target}"
                );
            }
        }
        for minute in trace.windows(61) {
            let change = (minute[60].1 - minute[0].1).abs();
            assert!(change <= 31.0, "{change} W in a minute");
        }
    }

    #[test]
    fn a_heart_rate_hold_settles_in_the_middle_of_the_zone() {
        let hold = HeartRateHold::zone(&rider(), 3, Watts(100.0), Watts(250.0));

        let trace = hold_against(FakeHeart::default(), hold, 30);

        assert_holds(&trace, &hold, 8);
    }

    #[test]
    fn a_heart_rate_hold_settles_for_hearts_unlike_the_estimate() {
        let hold = HeartRateHold::zone(&rider(), 3, Watts(50.0), Watts(350.0));
        let hearts = [
            // Reacting more strongly and faster than estimated…
            (0.8, 30),
            // …or weaker and slower.
            (0.3, 60),
        ];
        for (per_watt, lag) in hearts {
            let heart = FakeHeart {
                per_watt,
                lag: Duration::from_secs(lag),
                ..FakeHeart::default()
            };

            let trace = hold_against(heart, hold, 40);

            assert_holds(&trace, &hold, 15);
        }
    }

    #[test]
    fn a_heart_rate_hold_follows_cardiac_drift_by_easing_off() {
        let hold = HeartRateHold::zone(&rider(), 2, Watts(50.0), Watts(250.0));
        let heart = FakeHeart {
            drift_per_hour: BeatsPerMinute(12.0),
            ..FakeHeart::default()
        };

        let trace = hold_against(heart, hold, 90);

        assert_holds(&trace, &hold, 10);
        let eased = trace[15 * 60].1 - trace[89 * 60].1;
        // 12 bpm an hour at 0.5 bpm per watt: about 30 W less over 74 minutes.
        assert!((26.0..34.0).contains(&eased), "{eased} W");
    }

    #[test]
    fn a_heart_rate_hold_stays_within_limits_it_cannot_reach_the_target_in() {
        let hold = HeartRateHold::zone(&rider(), 4, Watts(100.0), Watts(140.0));

        let trace = hold_against(FakeHeart::default(), hold, 20);

        assert!(trace.iter().all(|&(_, power)| power <= 140.0));
        // Held at the top: targets go out in whole watts.
        assert!(trace.last().unwrap().1 > 139.0, "{:?}", trace.last());
    }

    #[tokio::test(start_paused = true)]
    async fn a_heart_rate_hold_settles_with_the_fake_trainer() {
        let tick = Duration::from_millis(250);
        let mut trainer = fake::spawn(
            FakeRider {
                power: Watts(150.0),
                cadence: Rpm(90.0),
                heart: Some(FakeHeart::default()),
            },
            tick,
        );
        let hold = HeartRateHold::zone(&rider(), 3, Watts(100.0), Watts(250.0));
        let mut ride = Ride::workout(Workout::HeartRate(hold), RideConfig::default());
        let mut heart_rates = Vec::new();

        for _ in 0..20 * 60 * 4 {
            tokio::time::sleep(tick).await;
            while let Ok(Some(event)) = trainer.try_next_event() {
                if let DeviceEvent::Telemetry(telemetry) = event {
                    ride.on_telemetry(&telemetry);
                }
            }
            if let Some(control) = ride.tick(tick) {
                trainer.control(control).await.unwrap();
            }
            heart_rates.extend(ride.state().telemetry.heart_rate);
        }

        let last_ten_minutes = &heart_rates[heart_rates.len() - 10 * 60 * 4..];
        assert!(
            last_ten_minutes
                .iter()
                .all(|h| (h.0 - hold.target.0).abs() <= 3.0),
            "{last_ten_minutes:?}"
        );
        // Recorded like any ride, from the trainer's power and heart rate.
        let last = ride.samples().last().unwrap();
        assert!(last.power.is_some() && last.heart_rate.is_some());
    }

    #[tokio::test]
    async fn flat_descent_mode_gives_no_free_speed() {
        let descent = route(&[-6.0], 3000.0).await;
        let mut coasting = Ride::new(descent.clone(), RideConfig::default());
        let flat_config = RideConfig {
            descent: DescentMode::Flat,
            ..RideConfig::default()
        };
        let mut flat = Ride::new(descent, flat_config);

        let flat_controls = pedal(&mut flat, 100.0, 60);
        pedal(&mut coasting, 100.0, 60);

        assert!(flat.state().distance.0 < coasting.state().distance.0 * 0.8);
        assert!(flat_controls.iter().all(|c| grade_of(c) >= 0.0));
    }
}
