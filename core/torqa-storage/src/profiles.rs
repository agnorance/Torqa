//! Rider profiles on disk (R22): `profiles/<id>/profile.toml` in the data directory, next to the
//! rider's own rides in `profiles/<id>/rides/`. One directory per rider keeps riders from
//! overwriting each other's files when the data directory is synced (ADR 0002).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use torqa_domain::profile::{Avatar, Drivetrain, Profile, UnitSystem};
use torqa_domain::shifting::{ButtonAction, ButtonMap, Control, Press};
use torqa_domain::units::{BeatsPerMinute, Kilograms, Percent, Watts};

const PROFILES: &str = "profiles";
const PROFILE_FILE: &str = "profile.toml";
const SETTINGS_FILE: &str = "settings.toml";
const HUD_FILE: &str = "hud.toml";

/// Reading or writing a profile failed.
#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    /// File system error.
    #[error("{0}")]
    Io(#[from] std::io::Error),
    /// The file is not valid TOML for a profile.
    #[error("invalid profile: {0}")]
    Parse(#[from] toml::de::Error),
    /// Serialising failed (cannot happen for profiles, but the encoder is fallible).
    #[error("cannot write profile: {0}")]
    Serialize(#[from] toml::ser::Error),
}

/// A profile and the directory name that identifies it.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredProfile {
    /// Directory name under `profiles/`; stable when the rider is renamed.
    pub id: String,
    /// The profile.
    pub profile: Profile,
}

/// The file format. Missing fields take defaults, so older files keep working when fields are
/// added.
#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
struct ProfileFile {
    name: String,
    rider_mass_kg: f64,
    bike_mass_kg: f64,
    ftp_w: f64,
    max_heart_rate_bpm: f64,
    units: Units,
    language: String,
    avatar: AvatarFile,
    drivetrain: DrivetrainFile,
    chainring: u8,
    cog: u8,
    /// Upper bounds of the power and heart-rate zones in percent of FTP and of the maximum
    /// heart rate; the standard zones where missing or out of order.
    power_zones_pct: Vec<f64>,
    heart_rate_zones_pct: Vec<f64>,
    default_difficulty_pct: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DrivetrainFile {
    Cassette,
    SingleCog,
}

/// A common road chainring on the Zwift Cog: the gears a rider sets up first.
const DEFAULT_CHAINRING: u8 = 50;
const DEFAULT_COG: u8 = 14;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Units {
    Metric,
    Imperial,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum AvatarFile {
    Female,
    Male,
}

impl Default for ProfileFile {
    fn default() -> Self {
        Self::from(&Profile::default())
    }
}

impl From<&Profile> for ProfileFile {
    fn from(p: &Profile) -> Self {
        Self {
            name: p.name.clone(),
            rider_mass_kg: p.rider_mass.0,
            bike_mass_kg: p.bike_mass.0,
            ftp_w: p.ftp.0,
            max_heart_rate_bpm: p.max_heart_rate.0,
            units: match p.units {
                UnitSystem::Metric => Units::Metric,
                UnitSystem::Imperial => Units::Imperial,
            },
            language: p.language.clone(),
            avatar: match p.avatar {
                Avatar::Female => AvatarFile::Female,
                Avatar::Male => AvatarFile::Male,
            },
            drivetrain: match p.drivetrain {
                Drivetrain::Cassette => DrivetrainFile::Cassette,
                Drivetrain::SingleCog { .. } => DrivetrainFile::SingleCog,
            },
            // Kept while riding a cassette, for switching back.
            chainring: match p.drivetrain {
                Drivetrain::SingleCog { chainring, .. } => chainring,
                Drivetrain::Cassette => DEFAULT_CHAINRING,
            },
            cog: match p.drivetrain {
                Drivetrain::SingleCog { cog, .. } => cog,
                Drivetrain::Cassette => DEFAULT_COG,
            },
            power_zones_pct: p.power_zones.iter().map(|b| b * 100.0).collect(),
            heart_rate_zones_pct: p.heart_rate_zones.iter().map(|b| b * 100.0).collect(),
            default_difficulty_pct: p.default_difficulty.0,
        }
    }
}

/// Percentages as shares.
fn shares(percent: &[f64]) -> Vec<f64> {
    percent.iter().map(|p| p / 100.0).collect()
}

impl From<ProfileFile> for Profile {
    fn from(f: ProfileFile) -> Self {
        Self {
            name: f.name,
            rider_mass: Kilograms(f.rider_mass_kg),
            bike_mass: Kilograms(f.bike_mass_kg),
            ftp: Watts(f.ftp_w),
            max_heart_rate: BeatsPerMinute(f.max_heart_rate_bpm),
            units: match f.units {
                Units::Metric => UnitSystem::Metric,
                Units::Imperial => UnitSystem::Imperial,
            },
            language: f.language,
            avatar: match f.avatar {
                AvatarFile::Female => Avatar::Female,
                AvatarFile::Male => Avatar::Male,
            },
            drivetrain: match f.drivetrain {
                DrivetrainFile::Cassette => Drivetrain::Cassette,
                DrivetrainFile::SingleCog => Drivetrain::SingleCog {
                    chainring: f.chainring.max(1),
                    cog: f.cog.max(1),
                },
            },
            power_zones: Profile::sane_power_zones(&shares(&f.power_zones_pct)),
            heart_rate_zones: Profile::sane_heart_rate_zones(&shares(&f.heart_rate_zones_pct)),
            default_difficulty: Percent(f.default_difficulty_pct.clamp(0.0, 100.0)),
        }
    }
}

/// Per-installation settings shared by all riders.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    /// The profile chosen last.
    active_profile: Option<String>,
    /// The trainer connected last, reconnected at start (R41).
    trainer: Option<RememberedDevice>,
    /// The heart-rate sensor connected last.
    heart_rate: Option<RememberedDevice>,
    /// The controller (shifter) connected last.
    controller: Option<RememberedDevice>,
    /// What the Di2 shifter's buttons do (#139).
    buttons: Option<ButtonsFile>,
    /// The D-Fly channels that shifted up and down before buttons could be given other
    /// controls; they shift until [`Settings::buttons`] is saved.
    shift_up_channel: Option<u8>,
    shift_down_channel: Option<u8>,
    /// How detailed the 3D world is drawn on this computer (R43).
    graphics_quality: Option<GraphicsQuality>,
    /// Where the overlay was last on screen (R55).
    overlay: Option<OverlayWindow>,
}

/// What the Di2 shifter's buttons do (#139): for each D-Fly channel, the action of each kind
/// of press by name. A press left out, or with an unknown action, does nothing.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
#[allow(clippy::struct_field_names)] // the D-Fly channels, named as the rider reads them in the file
struct ButtonsFile {
    channel_1: PressesFile,
    channel_2: PressesFile,
    channel_3: PressesFile,
    channel_4: PressesFile,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct PressesFile {
    #[serde(skip_serializing_if = "Option::is_none")]
    press: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hold: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    double_press: Option<String>,
}

impl From<&ButtonMap> for ButtonsFile {
    fn from(map: &ButtonMap) -> Self {
        let channel = |button: u8| {
            let name = |press| {
                map.action(button, press)
                    .map(|action| button_action_name(action).to_owned())
            };
            PressesFile {
                press: name(Press::Short),
                hold: name(Press::Long),
                double_press: name(Press::Double),
            }
        };
        Self {
            channel_1: channel(1),
            channel_2: channel(2),
            channel_3: channel(3),
            channel_4: channel(4),
        }
    }
}

impl ButtonsFile {
    fn map(&self) -> ButtonMap {
        let mut map = ButtonMap::NONE;
        let channels = [
            &self.channel_1,
            &self.channel_2,
            &self.channel_3,
            &self.channel_4,
        ];
        for (button, presses) in (1..).zip(channels) {
            for (press, name) in [
                (Press::Short, &presses.press),
                (Press::Long, &presses.hold),
                (Press::Double, &presses.double_press),
            ] {
                map.assign(
                    button,
                    press,
                    name.as_deref().and_then(button_action_from_name),
                );
            }
        }
        map
    }
}

/// The name of a button's `action` (#139), in the settings file and for the front end.
#[must_use]
pub fn button_action_name(action: ButtonAction) -> &'static str {
    match action {
        ButtonAction::ShiftUp => "shift_up",
        ButtonAction::ShiftDown => "shift_down",
        ButtonAction::ShiftUpTwo => "shift_up_two",
        ButtonAction::ShiftDownTwo => "shift_down_two",
        ButtonAction::Control(Control::NextCamera) => "next_camera",
        ButtonAction::Control(Control::Overlay) => "overlay",
        ButtonAction::Control(Control::PlayPause) => "play_pause",
        ButtonAction::Control(Control::NextTrack) => "next_track",
        ButtonAction::Control(Control::PreviousTrack) => "previous_track",
    }
}

/// The button action called `name`, if any.
#[must_use]
pub fn button_action_from_name(name: &str) -> Option<ButtonAction> {
    ButtonAction::ALL
        .into_iter()
        .find(|&action| button_action_name(action) == name)
}

/// Where the overlay window is on screen (R55), in screen pixels, and how large it draws.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct OverlayWindow {
    /// Left edge.
    pub x: i32,
    /// Top edge.
    pub y: i32,
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
    /// How large the overlay draws its figures (#124): 1 is one interface unit per point.
    /// `None` in settings from before it could be chosen, which take the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<f64>,
}

/// How detailed the 3D world is drawn (R43): more detail needs a stronger GPU. Medium holds
/// 60 fps on a base M1.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GraphicsQuality {
    /// For weak integrated GPUs.
    Low,
    /// The default: 60 fps on a base M1.
    #[default]
    Medium,
    /// For stronger Apple GPUs and discrete GPUs.
    High,
    /// Everything on, including global illumination.
    Ultra,
}

impl GraphicsQuality {
    /// All presets, from the lightest.
    pub const ALL: [Self; 4] = [Self::Low, Self::Medium, Self::High, Self::Ultra];

    /// The name stored and passed to the front end.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Ultra => "ultra",
        }
    }

    /// The preset called `name`, if any.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|q| q.name() == name)
    }
}

/// A Bluetooth device to reconnect at start (R41).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RememberedDevice {
    /// The system's identifier for it (stable per computer).
    pub id: String,
    /// Its advertised name, for messages and as a fallback when the identifier changed.
    pub name: String,
}

/// The devices to reconnect at start.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RememberedDevices {
    /// The trainer connected last.
    pub trainer: Option<RememberedDevice>,
    /// The heart-rate sensor connected last.
    pub heart_rate: Option<RememberedDevice>,
    /// The controller (shifter) connected last.
    pub controller: Option<RememberedDevice>,
}

/// What a remembered device is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceRole {
    /// The trainer.
    Trainer,
    /// The heart-rate sensor.
    HeartRate,
    /// A controller that shifts.
    Controller,
}

/// All readable profiles, by name. Unreadable ones are skipped.
#[must_use]
pub fn list(data_dir: &Path) -> Vec<StoredProfile> {
    let Ok(entries) = std::fs::read_dir(data_dir.join(PROFILES)) else {
        return Vec::new();
    };
    let mut profiles: Vec<StoredProfile> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let id = entry.file_name().to_str()?.to_owned();
            let profile = load(data_dir, &id).ok()?;
            Some(StoredProfile { id, profile })
        })
        .collect();
    profiles.sort_by(|a, b| a.profile.name.cmp(&b.profile.name).then(a.id.cmp(&b.id)));
    profiles
}

/// Reads one profile.
///
/// # Errors
/// If the file is missing or not a valid profile.
pub fn load(data_dir: &Path, id: &str) -> Result<Profile, ProfileError> {
    let text = std::fs::read_to_string(profile_dir(data_dir, id).join(PROFILE_FILE))?;
    Ok(toml::from_str::<ProfileFile>(&text)?.into())
}

/// Writes a profile atomically, creating its directory.
///
/// # Errors
/// On file system errors.
pub fn save(data_dir: &Path, id: &str, profile: &Profile) -> Result<(), ProfileError> {
    let text = toml::to_string_pretty(&ProfileFile::from(profile))?;
    write_atomically(&profile_dir(data_dir, id).join(PROFILE_FILE), &text)
}

/// A new, unused profile id derived from `name`.
#[must_use]
pub fn new_id(data_dir: &Path, name: &str) -> String {
    let slug = crate::slug(name, "rider");
    let mut id = slug.clone();
    let mut n = 1;
    while profile_dir(data_dir, &id).exists() {
        n += 1;
        id = format!("{slug}-{n}");
    }
    id
}

/// The rider's HUD layout: which metrics to show, in order (R23).
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct HudFile {
    metrics: Vec<String>,
}

/// The rider's chosen HUD metrics, in order, if the rider chose any.
#[must_use]
pub fn load_hud(data_dir: &Path, id: &str) -> Option<Vec<String>> {
    let text = std::fs::read_to_string(profile_dir(data_dir, id).join(HUD_FILE)).ok()?;
    Some(toml::from_str::<HudFile>(&text).ok()?.metrics)
}

/// Saves the rider's HUD metrics.
///
/// # Errors
/// On file system errors.
pub fn save_hud(data_dir: &Path, id: &str, metrics: &[String]) -> Result<(), ProfileError> {
    let file = HudFile {
        metrics: metrics.to_vec(),
    };
    write_atomically(
        &profile_dir(data_dir, id).join(HUD_FILE),
        &toml::to_string_pretty(&file)?,
    )
}

/// Where a rider's activities are saved.
#[must_use]
pub fn rides_dir(data_dir: &Path, id: &str) -> PathBuf {
    profile_dir(data_dir, id).join("rides")
}

/// The id of the profile chosen last, if any.
#[must_use]
pub fn active(data_dir: &Path) -> Option<String> {
    settings(data_dir).active_profile
}

/// The devices connected last.
#[must_use]
pub fn remembered_devices(data_dir: &Path) -> RememberedDevices {
    let settings = settings(data_dir);
    RememberedDevices {
        trainer: settings.trainer,
        heart_rate: settings.heart_rate,
        controller: settings.controller,
    }
}

/// Remembers a device in its `role` to reconnect next time.
///
/// # Errors
/// On file system errors.
pub fn remember_device(
    data_dir: &Path,
    role: DeviceRole,
    device: RememberedDevice,
) -> Result<(), ProfileError> {
    let mut settings = settings(data_dir);
    let slot = match role {
        DeviceRole::Trainer => &mut settings.trainer,
        DeviceRole::HeartRate => &mut settings.heart_rate,
        DeviceRole::Controller => &mut settings.controller,
    };
    *slot = Some(device);
    save_settings(data_dir, &settings)
}

/// What the Di2 shifter's buttons do (#139); until that is saved, the channels chosen before
/// shift up and down (1 and 2 if none were).
#[must_use]
pub fn button_map(data_dir: &Path) -> ButtonMap {
    let settings = settings(data_dir);
    settings.buttons.map_or_else(
        || {
            ButtonMap::shifting(
                settings.shift_up_channel.unwrap_or(1),
                settings.shift_down_channel.unwrap_or(2),
            )
        },
        |buttons| buttons.map(),
    )
}

/// Remembers what the Di2 shifter's buttons do.
///
/// # Errors
/// On file system errors.
pub fn set_button_map(data_dir: &Path, map: &ButtonMap) -> Result<(), ProfileError> {
    let mut settings = settings(data_dir);
    settings.buttons = Some(ButtonsFile::from(map));
    settings.shift_up_channel = None;
    settings.shift_down_channel = None;
    save_settings(data_dir, &settings)
}

/// The graphics quality chosen on this installation; Medium until one is chosen.
#[must_use]
pub fn graphics_quality(data_dir: &Path) -> GraphicsQuality {
    settings(data_dir).graphics_quality.unwrap_or_default()
}

/// Remembers the graphics quality for this installation.
///
/// # Errors
/// On file system errors.
pub fn set_graphics_quality(data_dir: &Path, quality: GraphicsQuality) -> Result<(), ProfileError> {
    let mut settings = settings(data_dir);
    settings.graphics_quality = Some(quality);
    save_settings(data_dir, &settings)
}

/// Where the overlay was last on screen; `None` before it was first used.
#[must_use]
pub fn overlay_window(data_dir: &Path) -> Option<OverlayWindow> {
    settings(data_dir).overlay
}

/// Remembers where the overlay is on screen, for the next time (R55).
///
/// # Errors
/// On file system errors.
pub fn set_overlay_window(data_dir: &Path, window: OverlayWindow) -> Result<(), ProfileError> {
    let mut settings = settings(data_dir);
    settings.overlay = Some(window);
    save_settings(data_dir, &settings)
}

fn settings(data_dir: &Path) -> Settings {
    std::fs::read_to_string(data_dir.join(SETTINGS_FILE))
        .ok()
        .and_then(|text| toml::from_str(&text).ok())
        .unwrap_or_default()
}

fn save_settings(data_dir: &Path, settings: &Settings) -> Result<(), ProfileError> {
    write_atomically(
        &data_dir.join(SETTINGS_FILE),
        &toml::to_string_pretty(settings)?,
    )
}

/// Remembers the chosen profile for the next start.
///
/// # Errors
/// On file system errors.
pub fn set_active(data_dir: &Path, id: &str) -> Result<(), ProfileError> {
    // Read first: the file holds other settings too.
    let mut settings = settings(data_dir);
    settings.active_profile = Some(id.to_owned());
    save_settings(data_dir, &settings)
}

fn profile_dir(data_dir: &Path, id: &str) -> PathBuf {
    data_dir.join(PROFILES).join(id)
}

fn write_atomically(path: &Path, text: &str) -> Result<(), ProfileError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let partial = path.with_extension("toml.part");
    std::fs::write(&partial, text)?;
    std::fs::rename(&partial, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("torqa-profiles-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn graphics_quality_is_kept_with_the_other_settings() {
        let dir = temp_dir("quality");
        assert_eq!(graphics_quality(&dir), GraphicsQuality::Medium);
        remember_device(
            &dir,
            DeviceRole::Trainer,
            RememberedDevice {
                id: "kickr".to_owned(),
                name: "KICKR".to_owned(),
            },
        )
        .unwrap();

        set_graphics_quality(&dir, GraphicsQuality::Ultra).unwrap();

        assert_eq!(graphics_quality(&dir), GraphicsQuality::Ultra);
        assert!(remembered_devices(&dir).trainer.is_some());
        assert!(
            std::fs::read_to_string(dir.join(SETTINGS_FILE))
                .unwrap()
                .contains(r#"graphics_quality = "ultra""#)
        );
        assert_eq!(
            GraphicsQuality::from_name("high"),
            Some(GraphicsQuality::High)
        );
    }

    #[test]
    fn buttons_shift_on_the_channels_chosen_before_they_could_be_assigned() {
        let dir = temp_dir("buttons-before");
        assert_eq!(button_map(&dir), ButtonMap::shifting(1, 2));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(SETTINGS_FILE),
            "shift_up_channel = 2\nshift_down_channel = 1\n",
        )
        .unwrap();

        assert_eq!(button_map(&dir), ButtonMap::shifting(2, 1));
    }

    #[test]
    fn assigned_buttons_are_kept_by_name() {
        let dir = temp_dir("buttons");
        set_graphics_quality(&dir, GraphicsQuality::High).unwrap();
        let mut map = ButtonMap::shifting(2, 1);
        map.assign(
            3,
            Press::Long,
            Some(ButtonAction::Control(Control::NextCamera)),
        );
        map.assign(1, Press::Double, None);

        set_button_map(&dir, &map).unwrap();

        assert_eq!(button_map(&dir), map);
        assert_eq!(graphics_quality(&dir), GraphicsQuality::High);
        let text = std::fs::read_to_string(dir.join(SETTINGS_FILE)).unwrap();
        assert!(text.contains(r#"hold = "next_camera""#), "{text}");
        assert!(!text.contains("shift_up_channel"), "{text}");
    }

    #[test]
    fn every_button_action_has_its_own_name_and_unknown_names_do_nothing() {
        for action in ButtonAction::ALL {
            assert_eq!(
                button_action_from_name(button_action_name(action)),
                Some(action)
            );
        }
        let dir = temp_dir("buttons-unknown");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(SETTINGS_FILE),
            "[buttons.channel_1]\npress = \"launch\"\nhold = \"shift_up\"\n",
        )
        .unwrap();

        let map = button_map(&dir);

        assert_eq!(map.action(1, Press::Short), None);
        assert_eq!(map.action(1, Press::Long), Some(ButtonAction::ShiftUp));
        assert_eq!(map.action(2, Press::Short), None, "not shifting any more");
    }

    #[test]
    fn the_overlay_window_is_remembered_with_the_other_settings() {
        let dir = temp_dir("overlay");
        assert_eq!(overlay_window(&dir), None);
        set_graphics_quality(&dir, GraphicsQuality::High).unwrap();
        let window = OverlayWindow {
            x: -1200,
            y: 40,
            width: 320,
            height: 480,
            scale: Some(1.75),
        };

        set_overlay_window(&dir, window).unwrap();

        assert_eq!(overlay_window(&dir), Some(window));
        assert_eq!(graphics_quality(&dir), GraphicsQuality::High);
        assert_eq!(GraphicsQuality::from_name("epic"), None);
    }

    #[test]
    fn an_overlay_remembered_before_its_size_could_be_chosen_keeps_its_place() {
        let dir = temp_dir("overlay-before-scale");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(SETTINGS_FILE),
            "[overlay]\nx = 10\ny = 20\nwidth = 300\nheight = 400\n",
        )
        .unwrap();

        assert_eq!(
            overlay_window(&dir),
            Some(OverlayWindow {
                x: 10,
                y: 20,
                width: 300,
                height: 400,
                scale: None,
            })
        );
    }

    #[test]
    fn profiles_round_trip_and_list_by_name() {
        let data = temp_dir("roundtrip");
        let zoe = Profile {
            name: "Zoë".to_owned(),
            ftp: Watts(310.0),
            units: UnitSystem::Imperial,
            language: "de".to_owned(),
            avatar: Avatar::Male,
            drivetrain: Drivetrain::SingleCog {
                chainring: 46,
                cog: 14,
            },
            power_zones: [0.5, 0.7, 0.85, 1.0, 1.15, 1.4],
            heart_rate_zones: [0.55, 0.65, 0.75, 0.85],
            default_difficulty: Percent(65.0),
            ..Profile::default()
        };
        let anna = Profile {
            name: "Anna".to_owned(),
            ..Profile::default()
        };

        save(&data, "zoe", &zoe).unwrap();
        save(&data, "anna", &anna).unwrap();

        let listed = list(&data);
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].profile, anna);
        assert_eq!(
            listed[1],
            StoredProfile {
                id: "zoe".to_owned(),
                profile: zoe
            }
        );
        std::fs::remove_dir_all(data).unwrap();
    }

    #[test]
    fn files_missing_newer_fields_still_load() {
        let data = temp_dir("partial");
        std::fs::create_dir_all(data.join("profiles/old")).unwrap();
        std::fs::write(
            data.join("profiles/old/profile.toml"),
            "name = \"Old\"\nftp_w = 180.0\n",
        )
        .unwrap();

        let profile = load(&data, "old").unwrap();

        assert_eq!(profile.name, "Old");
        assert_eq!(profile.ftp, Watts(180.0));
        assert_eq!(profile.rider_mass, Profile::default().rider_mass);
        assert_eq!(profile.avatar, Profile::default().avatar);
        assert_eq!(
            profile.drivetrain,
            Drivetrain::Cassette,
            "riders shift on the bike"
        );
        std::fs::remove_dir_all(data).unwrap();
    }

    #[test]
    fn new_ids_never_reuse_a_directory() {
        let data = temp_dir("ids");
        save(&data, "marco", &Profile::default()).unwrap();

        assert_eq!(new_id(&data, "Marco"), "marco-2");
        assert_eq!(new_id(&data, "Anna B."), "anna-b");
        assert_eq!(new_id(&data, "!!"), "rider");
        std::fs::remove_dir_all(data).unwrap();
    }

    #[test]
    fn hud_layouts_are_per_rider() {
        let data = temp_dir("hud");
        assert_eq!(load_hud(&data, "anna"), None);

        save_hud(&data, "anna", &["power_3s".to_owned(), "speed".to_owned()]).unwrap();

        assert_eq!(
            load_hud(&data, "anna"),
            Some(vec!["power_3s".to_owned(), "speed".to_owned()])
        );
        assert_eq!(load_hud(&data, "zoe"), None);
        std::fs::remove_dir_all(data).unwrap();
    }

    #[test]
    fn remembers_devices_alongside_the_active_profile() {
        let data = temp_dir("devices");
        assert_eq!(remembered_devices(&data), RememberedDevices::default());
        let kickr = RememberedDevice {
            id: "hci0/dev_AA".to_owned(),
            name: "KICKR CORE".to_owned(),
        };

        set_active(&data, "anna").unwrap();
        remember_device(&data, DeviceRole::Trainer, kickr.clone()).unwrap();
        set_active(&data, "zoe").unwrap();

        assert_eq!(remembered_devices(&data).trainer, Some(kickr));
        assert_eq!(remembered_devices(&data).heart_rate, None);
        assert_eq!(active(&data), Some("zoe".to_owned()));
        std::fs::remove_dir_all(data).unwrap();
    }

    #[test]
    fn remembers_the_active_profile() {
        let data = temp_dir("active");
        assert_eq!(active(&data), None);

        set_active(&data, "anna").unwrap();

        assert_eq!(active(&data), Some("anna".to_owned()));
        std::fs::remove_dir_all(data).unwrap();
    }
}
