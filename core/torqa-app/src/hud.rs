//! The ride HUD's metrics (R23): what a rider can choose to show, and their live values.

use torqa_domain::profile::Profile;
use torqa_domain::recording::{RideSummary, Sample};
use torqa_routes::Route;
use torqa_session::RideState;

/// How a metric's value is shown; front ends convert units and format by kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricKind {
    /// A plain number with a fixed unit.
    Number,
    /// Kilometres per hour (miles per hour in imperial).
    Speed,
    /// Kilometres (miles in imperial).
    Distance,
    /// Metres (feet in imperial).
    Elevation,
    /// Seconds, shown as a clock time.
    Duration,
    /// A gradient in percent, shown with its sign.
    Grade,
    /// A training zone number.
    Zone,
}

/// A metric the HUD can show.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metric {
    /// Stable identifier, stored in the rider's layout.
    pub id: &'static str,
    /// Caption.
    pub caption: &'static str,
    /// Unit of [`MetricKind::Number`] values (others derive theirs from the kind).
    pub unit: &'static str,
    /// Decimals to show.
    pub decimals: u8,
    /// How to show it.
    pub kind: MetricKind,
    /// A typical value, in the units of [`values`], for previews of a layout.
    pub sample: f64,
}

const fn metric(
    id: &'static str,
    caption: &'static str,
    unit: &'static str,
    decimals: u8,
    kind: MetricKind,
    sample: f64,
) -> Metric {
    Metric {
        id,
        caption,
        unit,
        decimals,
        kind,
        sample,
    }
}

// i18n-begin: captions and units are translated by the front end.
/// Every metric, in the order offered to the rider.
pub const METRICS: &[Metric] = &[
    metric("power", "Power", "W", 0, MetricKind::Number, 245.0),
    metric("power_3s", "Power 3 s", "W", 0, MetricKind::Number, 252.0),
    metric("power_10s", "Power 10 s", "W", 0, MetricKind::Number, 238.0),
    metric("avg_power", "Avg power", "W", 0, MetricKind::Number, 221.0),
    metric(
        "normalized_power",
        "Normalized power",
        "W",
        0,
        MetricKind::Number,
        236.0,
    ),
    metric("watts_per_kg", "W/kg", "W/kg", 1, MetricKind::Number, 3.3),
    metric("power_zone", "Power zone", "", 0, MetricKind::Zone, 3.0),
    metric(
        "heart_rate",
        "Heart rate",
        "bpm",
        0,
        MetricKind::Number,
        148.0,
    ),
    metric(
        "heart_rate_zone",
        "Heart-rate zone",
        "",
        0,
        MetricKind::Zone,
        3.0,
    ),
    metric("cadence", "Cadence", "rpm", 0, MetricKind::Number, 88.0),
    metric("gear", "Gear", "", 0, MetricKind::Number, 12.0),
    metric("speed", "Speed", "", 1, MetricKind::Speed, 31.4),
    metric("avg_speed", "Avg speed", "", 1, MetricKind::Speed, 29.8),
    metric("distance", "Distance", "", 2, MetricKind::Distance, 12.4),
    metric("remaining", "To go", "", 2, MetricKind::Distance, 7.6),
    metric("elapsed", "Time", "", 0, MetricKind::Duration, 1543.0),
    metric(
        "elevation",
        "Elevation",
        "",
        0,
        MetricKind::Elevation,
        612.0,
    ),
    metric(
        "elevation_gain",
        "Climbed",
        "",
        0,
        MetricKind::Elevation,
        284.0,
    ),
    metric(
        "ascent_remaining",
        "Ascent to go",
        "",
        0,
        MetricKind::Elevation,
        356.0,
    ),
    metric("grade", "Grade", "", 1, MetricKind::Grade, 4.2),
    metric(
        "upcoming_grade",
        "Next 500 m",
        "",
        1,
        MetricKind::Grade,
        6.1,
    ),
    metric("intensity", "Intensity", "", 2, MetricKind::Number, 0.86),
    metric("training_stress", "TSS", "", 0, MetricKind::Number, 38.0),
    metric("work", "Work", "kJ", 0, MetricKind::Number, 412.0),
];

// i18n-end

/// The layout before a rider customises it: the first metric is shown large.
pub const DEFAULT_LAYOUT: &[&str] = &[
    "power",
    "speed",
    "grade",
    "heart_rate",
    "cadence",
    "distance",
    "elapsed",
];

/// Most metrics a layout may hold; more would cover the road.
pub const MAX_METRICS: usize = 13;

/// Distance over which the upcoming gradient is averaged.
const LOOK_AHEAD_M: f64 = 500.0;

/// A layout cleaned up: known metrics only, each once, at most [`MAX_METRICS`]; the default
/// if nothing is left.
#[must_use]
pub fn sanitize(layout: &[String]) -> Vec<String> {
    let mut clean: Vec<String> = Vec::new();
    for id in layout {
        if METRICS.iter().any(|m| m.id == id) && !clean.contains(id) {
            clean.push(id.clone());
        }
    }
    clean.truncate(MAX_METRICS);
    if clean.is_empty() {
        DEFAULT_LAYOUT.iter().map(|&id| id.to_owned()).collect()
    } else {
        clean
    }
}

/// Live value of every metric, in metric display units (km/h, km, m, s); `None` where unknown,
/// e.g. heart rate without a strap, or the road without a route. `summary` summarises the
/// samples so far.
#[must_use]
pub fn values(
    state: &RideState,
    samples: &[Sample],
    summary: &RideSummary,
    route: Option<&Route>,
    profile: &Profile,
) -> Vec<(&'static str, Option<f64>)> {
    let t = state.telemetry;
    let power = t.power;
    let upcoming = route.and_then(|route| {
        let ahead = (state.distance.0 + LOOK_AHEAD_M).min(route.length().0);
        (ahead > state.distance.0).then(|| {
            let here = route.position(state.distance).elevation.0;
            let there = route
                .position(torqa_domain::units::Meters(ahead))
                .elevation
                .0;
            (there - here) / (ahead - state.distance.0) * 100.0
        })
    });
    METRICS
        .iter()
        .map(|metric| {
            let value = match metric.id {
                "power" => power.map(|p| p.0),
                "power_3s" => recent_power(samples, 3),
                "power_10s" => recent_power(samples, 10),
                "avg_power" => summary.avg_power.map(|p| p.0),
                "normalized_power" => summary.normalized_power.map(|p| p.0),
                "watts_per_kg" => power.map(|p| profile.watts_per_kg(p)),
                "power_zone" => power.map(|p| f64::from(profile.power_zone(p))),
                "heart_rate" => t.heart_rate.map(|h| h.0),
                "heart_rate_zone" => t.heart_rate.map(|h| f64::from(profile.heart_rate_zone(h))),
                "cadence" => t.cadence.map(|c| c.0),
                #[allow(clippy::cast_precision_loss)] // a gear number
                "gear" => state.gear.map(|g| g.number as f64),
                "speed" => Some(state.speed.as_kilometers_per_hour()),
                "avg_speed" => Some(summary.avg_speed.as_kilometers_per_hour()),
                "distance" => Some(state.distance.0 / 1000.0),
                "remaining" => state.remaining.map(|r| r.0 / 1000.0),
                "elapsed" => Some(state.elapsed.as_secs_f64()),
                "elevation" => state.position.map(|p| p.elevation.0),
                "elevation_gain" => route.map(|_| summary.elevation_gain.0),
                "ascent_remaining" => route.map(|r| r.ascent_ahead(state.distance).0),
                "grade" => state.position.map(|p| p.grade.0),
                "upcoming_grade" => upcoming,
                "intensity" => summary.intensity_factor,
                "training_stress" => summary.training_stress,
                "work" => summary.work.map(|w| w.0 / 1000.0),
                _ => None,
            };
            (metric.id, value)
        })
        .collect()
}

/// Average power over the last `seconds` samples that have power.
fn recent_power(samples: &[Sample], seconds: usize) -> Option<f64> {
    let recent: Vec<f64> = samples
        .iter()
        .rev()
        .take(seconds)
        .filter_map(|s| s.power.map(|p| p.0))
        .collect();
    #[allow(clippy::cast_precision_loss)] // at most a few samples
    let count = recent.len() as f64;
    (!recent.is_empty()).then(|| recent.iter().sum::<f64>() / count)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use torqa_domain::units::{Meters, MetersPerSecond, Watts};

    use super::*;

    #[test]
    fn layouts_keep_known_metrics_once_in_order() {
        let layout: Vec<String> = ["speed", "bogus", "power_3s", "speed"]
            .iter()
            .map(|&s| s.to_owned())
            .collect();

        assert_eq!(sanitize(&layout), ["speed", "power_3s"]);
        assert_eq!(sanitize(&[]), DEFAULT_LAYOUT);
        let everything: Vec<String> = METRICS.iter().map(|m| m.id.to_owned()).collect();
        assert_eq!(sanitize(&everything).len(), MAX_METRICS);
    }

    #[test]
    fn every_metric_has_a_unique_id_and_the_default_layout_is_valid() {
        for (i, metric) in METRICS.iter().enumerate() {
            assert!(
                METRICS[..i].iter().all(|m| m.id != metric.id),
                "{}",
                metric.id
            );
        }
        let default: Vec<String> = DEFAULT_LAYOUT.iter().map(|&s| s.to_owned()).collect();
        assert_eq!(sanitize(&default), default);
    }

    #[test]
    fn short_term_power_averages_the_latest_seconds() {
        let samples: Vec<Sample> = [100.0, 100.0, 200.0, 300.0, 400.0]
            .iter()
            .enumerate()
            .map(|(i, &watts)| Sample {
                elapsed: Duration::from_secs(i as u64),
                location: None,
                distance: Meters(0.0),
                speed: MetersPerSecond(0.0),
                power: Some(Watts(watts)),
                cadence: None,
                heart_rate: None,
            })
            .collect();

        assert_eq!(recent_power(&samples, 3), Some(300.0));
        assert_eq!(recent_power(&samples, 10), Some(220.0));
        assert_eq!(recent_power(&[], 3), None);
    }
}
