//! The climate a ride passes through, which sets what grows and how houses are built (#136):
//! palms, banana plants and flat-roofed or low red-tiled houses in the (sub)tropics, the
//! temperate world everywhere else.

/// Up to this latitude (north or south) the lowlands count as tropical: the tropics proper end
/// at 23.4°, but palms and houses built for the heat reach further, to Ishigaki (24.3° N),
/// Okinawa (26.5° N), Taiwan and southern Florida.
const TROPICAL_LATITUDE: f64 = 27.0;
/// Above this elevation even the tropics are temperate: highland forests, no palms.
const TROPICAL_CEILING: f64 = 1200.0;

/// The climate of a ride's region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Climate {
    /// Conifers and broadleaf trees, houses with pitched roofs, chalets in the mountains.
    Temperate,
    /// Palms, banana plants and tropical shrubs, houses built for the heat.
    Tropical,
}

impl Climate {
    /// The climate at `latitude` (degrees); a ride stays within one.
    pub(crate) fn at(latitude: f64) -> Self {
        if latitude.abs() < TROPICAL_LATITUDE {
            Self::Tropical
        } else {
            Self::Temperate
        }
    }

    /// Whether the ground at `elevation` metres is tropical: in the tropics and below their
    /// highlands.
    pub(crate) fn tropical_at(self, elevation: f64) -> bool {
        self == Self::Tropical && elevation < TROPICAL_CEILING
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_subtropical_lowlands_are_tropical() {
        // Ishigaki, Okinawa, Hawaii, Singapore, and Rio de Janeiro in the south.
        for latitude in [24.34, 26.2, 21.3, 1.35, -22.9] {
            assert!(Climate::at(latitude).tropical_at(10.0), "{latitude}");
        }
        // Bern, Brittany, Tokyo, Tasmania.
        for latitude in [46.95, 48.4, 35.7, -42.9] {
            assert_eq!(Climate::at(latitude), Climate::Temperate, "{latitude}");
        }
        // Highlands in the tropics are not.
        assert!(!Climate::at(4.6).tropical_at(2600.0));
    }
}
