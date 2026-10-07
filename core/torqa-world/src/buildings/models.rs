//! The building models made in Blender (`art/buildings`, ADR 0009): which there are, and which
//! one fits a building on the map, how stretched and which way round.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use serde::Deserialize;

use super::shape::Rect;
use super::{Dice, Kind};

/// The manifest the model build writes next to the models.
const MANIFEST: &str = include_str!("../../../../app/assets/models/buildings/models.json");

/// Models are stretched or squeezed at most this much to fit a footprint; beyond it windows
/// and roofs would look wrong, and the building keeps its shell.
pub(crate) const MAX_STRETCH: f64 = 1.25;

/// One model, as the manifest describes it.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Model {
    #[serde(skip)]
    pub(crate) name: String,
    /// `house`, `chalet`, `farmhouse`, `church`, `chapel`, `shed`, `office`, `public`,
    /// `hotel`, `castle`, `lighthouse` or `tropical` (houses of the subtropics).
    kind: String,
    /// `gable`, `hipped` or `flat`.
    pub(crate) roof: String,
    /// The footprint its walls stand on: along its x axis and across.
    pub(crate) length: f64,
    pub(crate) width: f64,
    /// Height of the eaves above the ground.
    pub(crate) eaves: f64,
    #[serde(default)]
    pub(crate) storeys: Option<u32>,
    #[serde(default)]
    pub(crate) pitch: Option<f64>,
}

#[derive(Deserialize)]
struct Manifest {
    models: BTreeMap<String, Model>,
}

/// All models, in name order.
pub(crate) static MODELS: LazyLock<Vec<Model>> = LazyLock::new(|| {
    let manifest: Manifest =
        serde_json::from_str(MANIFEST).expect("the committed models.json is valid");
    manifest
        .models
        .into_iter()
        .map(|(name, model)| Model { name, ..model })
        .collect()
});

/// What the building should look like, so that its model matches its shell.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Wanted {
    pub(crate) kind: Kind,
    /// Houses in the subtropics are built for the heat.
    pub(crate) tropical: bool,
    /// Small churches are chapels.
    pub(crate) chapel: bool,
    pub(crate) storeys: Option<u32>,
    pub(crate) hipped: bool,
}

/// A model fitted to a footprint.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Fit {
    pub(crate) model: &'static Model,
    /// Stretch along the model's length and across it.
    pub(crate) scale: (f64, f64),
}

/// The model that fits `rect` best, if any fits within `MAX_STRETCH`: among those of the
/// wanted kind, the least stretched, with storeys and roof as wanted where possible.
pub(crate) fn fitting(rect: &Rect, wanted: Wanted, dice: &Dice) -> Option<Fit> {
    let kind = match wanted.kind {
        Kind::House if wanted.tropical => "tropical",
        Kind::House => "house",
        Kind::Chalet => "chalet",
        Kind::Farmhouse => "farmhouse",
        Kind::Shed => "shed",
        Kind::Church if wanted.chapel => "chapel",
        Kind::Church => "church",
        Kind::Office => "office",
        Kind::Public => "public",
        Kind::Hotel => "hotel",
        Kind::Castle => "castle",
        Kind::Lighthouse => "lighthouse",
        Kind::Block | Kind::Hall => return None,
    };
    let (length, width) = (2.0 * rect.half_length, 2.0 * rect.half_width);
    MODELS
        .iter()
        .filter(|m| m.kind == kind)
        .filter_map(|model| {
            let scale = (length / model.length, width / model.width);
            let fits = |s: f64| (1.0 / MAX_STRETCH..=MAX_STRETCH).contains(&s);
            if !fits(scale.0) || !fits(scale.1) {
                return None;
            }
            let storeys = match (wanted.storeys, model.storeys) {
                (Some(a), Some(b)) => f64::from(a.abs_diff(b)) * 0.5,
                _ => 0.0,
            };
            let roof = if (model.roof == "hipped") == wanted.hipped {
                0.0
            } else {
                0.3
            };
            // Equally good models take turns, so neighbours differ.
            let salt = model
                .name
                .bytes()
                .fold(7_i64, |h, b| h.wrapping_mul(31).wrapping_add(i64::from(b)));
            let variety = 0.08 * dice.roll(salt);
            let cost = scale.0.ln().abs() + scale.1.ln().abs() + storeys + roof + variety;
            Some((cost, Fit { model, scale }))
        })
        .min_by(|(a, _), (b, _)| a.total_cmp(b))
        .map(|(_, fit)| fit)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn every_model_in_the_manifest_has_its_file() {
        let directory =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../app/assets/models/buildings");

        assert!(MODELS.len() >= 10);
        for model in MODELS.iter() {
            assert!(
                directory.join(format!("{}.glb", model.name)).is_file(),
                "{} is missing",
                model.name
            );
            assert!(
                model.length >= model.width && model.width > 0.0,
                "{}",
                model.name
            );
            assert!(model.eaves > 2.0, "{}", model.name);
        }
    }

    #[test]
    fn houses_get_a_house_model_stretched_at_most_a_quarter() {
        let house = Rect::square((0.0, 0.0), (1.0, 0.0), 5.0);
        let long_house = Rect {
            half_length: 6.5,
            half_width: 4.6,
            ..house
        };
        let wanted = Wanted {
            kind: Kind::House,
            tropical: false,
            chapel: false,
            storeys: Some(2),
            hipped: false,
        };

        for rect in [house, long_house] {
            let fit = fitting(&rect, wanted, &Dice(3)).expect("a house model");
            assert_eq!(fit.model.kind, "house");
            for s in [fit.scale.0, fit.scale.1] {
                assert!((0.8..=1.25).contains(&s), "{s}");
            }
            assert!(
                (fit.model.length * fit.scale.0 - 2.0 * rect.half_length).abs() < 1e-9
                    && (fit.model.width * fit.scale.1 - 2.0 * rect.half_width).abs() < 1e-9
            );
        }
    }

    #[test]
    fn storeys_and_roofs_are_matched_where_a_model_has_them() {
        let rect = Rect {
            centre: (0.0, 0.0),
            axis: (1.0, 0.0),
            half_length: 6.0,
            half_width: 4.5,
        };
        for (storeys, hipped) in [(1, false), (2, false), (2, true), (3, false)] {
            let wanted = Wanted {
                kind: Kind::House,
                tropical: false,
                chapel: false,
                storeys: Some(storeys),
                hipped,
            };
            let fit = fitting(&rect, wanted, &Dice(11)).expect("a house model");

            assert_eq!(fit.model.storeys, Some(storeys));
            assert_eq!(fit.model.roof == "hipped", hipped);
        }
    }

    #[test]
    fn offices_hotels_and_public_buildings_get_models_of_their_kind() {
        for (kind, name, (half_length, half_width), storeys, hipped) in [
            (Kind::Office, "office", (15.0, 8.0), 4, false),
            (Kind::Hotel, "hotel", (12.0, 6.5), 4, false),
            (Kind::Public, "public", (13.0, 6.5), 2, true),
            (Kind::Public, "public", (19.0, 8.0), 2, false),
        ] {
            let rect = Rect {
                centre: (0.0, 0.0),
                axis: (1.0, 0.0),
                half_length,
                half_width,
            };
            let wanted = Wanted {
                kind,
                tropical: false,
                chapel: false,
                storeys: Some(storeys),
                hipped,
            };
            let fit = fitting(&rect, wanted, &Dice(5)).expect("a model");

            assert_eq!(fit.model.kind, name);
            assert_eq!(fit.model.storeys, Some(storeys));
            assert_eq!(fit.model.roof == "hipped", hipped);
        }
    }

    #[test]
    fn castles_and_lighthouses_get_their_models() {
        // A castle's keep, 24 × 17 m, and a lighthouse 6 m across (round, so a square).
        let keep = Rect {
            centre: (0.0, 0.0),
            axis: (1.0, 0.0),
            half_length: 12.0,
            half_width: 8.5,
        };
        let tower = Rect::square((0.0, 0.0), (1.0, 0.0), 3.0);
        for (rect, kind, name) in [
            (keep, Kind::Castle, "castle"),
            (tower, Kind::Lighthouse, "lighthouse"),
        ] {
            let wanted = Wanted {
                kind,
                tropical: false,
                chapel: false,
                storeys: None,
                hipped: false,
            };
            let fit = fitting(&rect, wanted, &Dice(9)).expect("a model");

            assert_eq!(fit.model.kind, name);
        }
    }

    #[test]
    fn houses_in_the_subtropics_get_flat_or_low_hipped_roofs() {
        let rect = Rect {
            centre: (0.0, 0.0),
            axis: (1.0, 0.0),
            half_length: 6.0,
            half_width: 4.75,
        };
        for (storeys, hipped) in [(1, false), (2, false), (1, true)] {
            let wanted = Wanted {
                kind: Kind::House,
                tropical: true,
                chapel: false,
                storeys: Some(storeys),
                hipped,
            };
            let fit = fitting(&rect, wanted, &Dice(13)).expect("a tropical model");

            assert_eq!(fit.model.kind, "tropical");
            assert_eq!(fit.model.storeys, Some(storeys));
            let roof = if hipped { "hipped" } else { "flat" };
            assert_eq!(fit.model.roof, roof);
        }
        // Elsewhere the same house gets a house of the temperate world.
        let temperate = Wanted {
            kind: Kind::House,
            tropical: false,
            chapel: false,
            storeys: Some(2),
            hipped: false,
        };
        let fit = fitting(&rect, temperate, &Dice(13)).expect("a house model");
        assert_eq!(fit.model.kind, "house");
    }

    #[test]
    fn nothing_fits_a_building_far_off_every_model() {
        let huge = Rect::square((0.0, 0.0), (1.0, 0.0), 40.0);
        let wanted = Wanted {
            kind: Kind::Chalet,
            tropical: false,
            chapel: false,
            storeys: None,
            hipped: false,
        };

        assert!(fitting(&huge, wanted, &Dice(1)).is_none());
        let block = Wanted {
            kind: Kind::Block,
            ..wanted
        };
        assert!(fitting(&Rect::square((0.0, 0.0), (1.0, 0.0), 8.0), block, &Dice(1)).is_none());
    }
}
