//! Rider profiles (R22): body and fitness figures, and the training zones derived from them.

use crate::units::{BeatsPerMinute, Kilograms, Percent, Watts};

/// How values are shown to this rider (R24). Everything is stored in SI units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UnitSystem {
    /// Kilometres, metres, kilograms.
    #[default]
    Metric,
    /// Miles, feet, pounds.
    Imperial,
}

/// The rider shown on the bike in the 3D world (R46).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Avatar {
    /// The female rider.
    #[default]
    Female,
    /// The male rider.
    Male,
}

/// What the rider shifts with on the trainer (R9, ADR 0003).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Drivetrain {
    /// A cassette: the rider shifts on the bike and the trainer follows the road.
    #[default]
    Cassette,
    /// A single cog (e.g. the Zwift Cog): Torqa shifts virtual gears.
    SingleCog {
        /// Teeth of the chainring ridden.
        chainring: u8,
        /// Teeth of the cog on the trainer.
        cog: u8,
    },
}

/// One rider of an installation; several riders can share it.
#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    /// Display name.
    pub name: String,
    /// Body mass.
    pub rider_mass: Kilograms,
    /// Bike mass, added to the rider for climbing and rolling resistance.
    pub bike_mass: Kilograms,
    /// Functional threshold power: the base of the power zones.
    pub ftp: Watts,
    /// Maximum heart rate: the base of the heart-rate zones.
    pub max_heart_rate: BeatsPerMinute,
    /// Display units.
    pub units: UnitSystem,
    /// User interface language as a locale code (e.g. `de`); empty for the system language.
    pub language: String,
    /// The rider shown on the bike.
    pub avatar: Avatar,
    /// What the rider shifts with.
    pub drivetrain: Drivetrain,
    /// Trainer difficulty a ride starts with: how much of the road gradient the rider feels.
    pub default_difficulty: Percent,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            name: "Rider".to_owned(),
            rider_mass: Kilograms(75.0),
            bike_mass: Kilograms(8.0),
            ftp: Watts(200.0),
            max_heart_rate: BeatsPerMinute(185.0),
            units: UnitSystem::Metric,
            language: String::new(),
            avatar: Avatar::Female,
            drivetrain: Drivetrain::Cassette,
            default_difficulty: Percent(50.0),
        }
    }
}

/// Upper bounds of power zones 1–6 as a share of FTP (Coggan's seven zones; zone 7 is open).
const POWER_ZONES: [f64; 6] = [0.55, 0.75, 0.90, 1.05, 1.20, 1.50];
/// Upper bounds of heart-rate zones 1–4 as a share of the maximum (five zones; zone 5 is open).
const HEART_RATE_ZONES: [f64; 4] = [0.60, 0.70, 0.80, 0.90];
/// Where zone 1 starts as a share of the maximum: lower heart rates are rest, not training.
const HEART_RATE_ZONE_1_FLOOR: f64 = 0.50;

impl Profile {
    /// Rider plus bike.
    #[must_use]
    pub fn system_mass(&self) -> Kilograms {
        Kilograms(self.rider_mass.0 + self.bike_mass.0)
    }

    /// Power relative to body mass, the usual measure of climbing ability.
    #[must_use]
    pub fn watts_per_kg(&self, power: Watts) -> f64 {
        if self.rider_mass.0 > 0.0 {
            power.0 / self.rider_mass.0
        } else {
            0.0
        }
    }

    /// Power zone 1–7 relative to FTP.
    #[must_use]
    pub fn power_zone(&self, power: Watts) -> u8 {
        zone(power.0, self.ftp.0, &POWER_ZONES)
    }

    /// Heart-rate zone 1–5 relative to the maximum heart rate.
    #[must_use]
    pub fn heart_rate_zone(&self, heart_rate: BeatsPerMinute) -> u8 {
        zone(heart_rate.0, self.max_heart_rate.0, &HEART_RATE_ZONES)
    }

    /// The lowest and highest heart rate of zone 1–5 (other zones are clamped to these): zone
    /// 1 starts at half the maximum, zone 5 ends at the maximum.
    #[must_use]
    pub fn heart_rate_zone_range(&self, zone: u8) -> (BeatsPerMinute, BeatsPerMinute) {
        let index = usize::from(zone.clamp(1, 5)) - 1;
        let low = index
            .checked_sub(1)
            .map_or(HEART_RATE_ZONE_1_FLOOR, |below| HEART_RATE_ZONES[below]);
        let high = HEART_RATE_ZONES.get(index).copied().unwrap_or(1.0);
        let max = self.max_heart_rate.0;
        (BeatsPerMinute(low * max), BeatsPerMinute(high * max))
    }
}

/// The 1-based zone of `value`: zone n ends at `bounds[n - 1] × base`, inclusive.
fn zone(value: f64, base: f64, bounds: &[f64]) -> u8 {
    let below = bounds
        .iter()
        .take_while(|&&bound| value > bound * base)
        .count();
    u8::try_from(below + 1).unwrap_or(u8::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rider() -> Profile {
        Profile {
            ftp: Watts(250.0),
            max_heart_rate: BeatsPerMinute(190.0),
            rider_mass: Kilograms(70.0),
            ..Profile::default()
        }
    }

    #[test]
    fn power_zones_follow_coggan() {
        let rider = rider();
        // 55 / 75 / 90 / 105 / 120 / 150 % of 250 W.
        for (watts, zone) in [
            (0.0, 1),
            (137.5, 1),
            (140.0, 2),
            (187.5, 2),
            (200.0, 3),
            (230.0, 4),
            (262.5, 4),
            (280.0, 5),
            (310.0, 6),
            (375.0, 6),
            (900.0, 7),
        ] {
            assert_eq!(rider.power_zone(Watts(watts)), zone, "{watts} W");
        }
    }

    #[test]
    fn heart_rate_zones_are_shares_of_the_maximum() {
        let rider = rider();
        for (bpm, zone) in [
            (100.0, 1),
            (114.0, 1),
            (120.0, 2),
            (150.0, 3),
            (165.0, 4),
            (180.0, 5),
        ] {
            assert_eq!(
                rider.heart_rate_zone(BeatsPerMinute(bpm)),
                zone,
                "{bpm} bpm"
            );
        }
    }

    #[test]
    fn heart_rate_zone_ranges_meet_where_the_zones_change() {
        let rider = rider();
        for zone in 1..=5 {
            let (low, high) = rider.heart_rate_zone_range(zone);
            assert!(low.0 < high.0, "zone {zone}");
            // Just inside each end the rider is in this zone.
            assert_eq!(rider.heart_rate_zone(BeatsPerMinute(low.0 + 0.5)), zone);
            assert_eq!(rider.heart_rate_zone(BeatsPerMinute(high.0 - 0.5)), zone);
        }
        // Zone 3 of 190 bpm: 70–80 %.
        let (low, high) = rider.heart_rate_zone_range(3);
        assert!((low.0 - 133.0).abs() < 1e-9 && (high.0 - 152.0).abs() < 1e-9);
        assert_eq!(
            rider.heart_rate_zone_range(9),
            rider.heart_rate_zone_range(5)
        );
    }

    #[test]
    fn watts_per_kg_uses_body_mass_and_physics_uses_bike_too() {
        let rider = rider();

        assert!((rider.watts_per_kg(Watts(280.0)) - 4.0).abs() < 1e-9);
        assert!((rider.system_mass().0 - 78.0).abs() < 1e-9);
    }
}
