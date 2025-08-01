use merg::Merge;
use num_traits::FloatConst;
use serde::Deserialize;
use serde_with::{serde_as, DeserializeFromStr, DurationSeconds};
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;
use thiserror::Error;
use tracing::info;
use validator::Validate;
use video::aspect::AspectRatio;
use video::resolution::Resolution;

mod bootcore;
mod fb_size;
mod hdmi_limited;
mod hdr;
mod ini; // Internal module.
mod ntsc_mode;
mod osd_rotate;
mod reset_combo;
mod vga_mode;
pub mod video;
mod vrr_mode;
mod vscale_mode;
mod vsync_adjust;

pub use bootcore::*;
pub use fb_size::*;
pub use hdmi_limited::*;
pub use hdr::*;
pub use ntsc_mode::*;
pub use osd_rotate::*;
pub use reset_combo::*;
pub use vga_mode::*;
pub use video::*;
pub use vrr_mode::*;
pub use vscale_mode::*;
pub use vsync_adjust::*;

#[derive(Error, Debug)]
#[error(transparent)]
pub enum ConfigError {
    #[error("Invalid config file: {0}")]
    Io(#[from] io::Error),

    #[error("Could not read INI file: {0}")]
    IniError(#[from] ini::Error),

    #[error("Could not read JSON file: {0}")]
    JsonError(#[from] json5::Error),
}

/// A helper function to read minutes into durations from the config.
struct DurationMinutes;

impl<'de> serde_with::DeserializeAs<'de, Duration> for DurationMinutes {
    fn deserialize_as<D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let dur: Duration = DurationSeconds::<u64>::deserialize_as(deserializer)?;
        let secs = dur.as_secs();
        Ok(Duration::from_secs(secs * 60))
    }
}

/// A struct representing the `video=123x456` string for sections in the config.
#[derive(DeserializeFromStr, Debug, Clone, Hash, Ord, PartialOrd, Eq, PartialEq)]
pub struct VideoResolutionString(Resolution);

impl FromStr for VideoResolutionString {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(s) = s.strip_prefix("video=") {
            Ok(VideoResolutionString(Resolution::from_str(s)?))
        } else {
            Err("Invalid video section string.")
        }
    }
}

/// Serde helper for supporting mister booleans (can be 0 or 1) used in the current config.
mod mister_bool {
    use serde::{Deserialize, Deserializer};

    /// Transforms a mister string into a boolean.
    fn bool_from_str(s: impl AsRef<str>) -> bool {
        match s.as_ref() {
            "enabled" | "true" | "1" => true,
            "0" => false,
            _ => false,
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Option::deserialize(deserializer).map(|v: Option<String>| v.map(bool_from_str))
    }
}

/// Serde helper for supporting mister hexa values (can be 0x1234) used in the current config.
mod mister_hexa {
    use serde::{Deserialize, Deserializer};

    pub fn deserialize<'de, D, T: num_traits::Num>(deserializer: D) -> Result<Option<T>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Option::deserialize(deserializer).and_then(|v: Option<String>| {
            if let Some(s) = v {
                if let Some(hs) = s.strip_prefix("0x") {
                    T::from_str_radix(hs, 16)
                        .map_err(|_| {
                            serde::de::Error::custom(format!("Invalid hexadecimal value: {s}"))
                        })
                        .map(Some)
                } else if s == "0" {
                    Ok(Some(T::zero()))
                } else {
                    Err(serde::de::Error::custom(format!(
                        "Invalid hexadecimal value: {s}"
                    )))
                }
            } else {
                Ok(None)
            }
        })
    }
}

/// Serde helper for supporting mister hexa values (can be 0x1234) used in the current config.
mod mister_hexa_seq {
    use serde::{Deserialize, Deserializer};

    pub fn deserialize<'de, D, T: num_traits::Num>(deserializer: D) -> Result<Vec<T>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Vec::deserialize(deserializer).and_then(|v: Vec<String>| {
            v.into_iter()
                .map(|s| {
                    if let Some(hs) = s.strip_prefix("0x") {
                        T::from_str_radix(hs, 16).map_err(|_| {
                            serde::de::Error::custom(format!("Invalid hexadecimal value: {s}"))
                        })
                    } else if s == "0" {
                        Ok(T::zero())
                    } else {
                        Err(serde::de::Error::custom(format!(
                            "Invalid hexadecimal value: {s}"
                        )))
                    }
                })
                .collect()
        })
    }
}

mod validate {
    use std::time::Duration;
    use validator::ValidationError;

    pub fn video_info(video_info: &Duration) -> Result<(), ValidationError> {
        if video_info.as_secs() > 10 || video_info.as_secs() < 1 {
            return Err(ValidationError::new("video_info must be between 1 and 10."));
        }

        Ok(())
    }

    pub fn controller_info(controller_info: &Duration) -> Result<(), ValidationError> {
        if controller_info.as_secs() > 10 {
            return Err(ValidationError::new(
                "controller_info must be between 0 and 10.",
            ));
        }

        Ok(())
    }

    pub fn osd_timeout(osd_timeout: &Duration) -> Result<(), ValidationError> {
        if osd_timeout.as_secs() > 3600 || osd_timeout.as_secs() < 5 {
            return Err(ValidationError::new(
                "osd_timeout must be between 5 and 3600.",
            ));
        }

        Ok(())
    }

    pub fn bootcore_timeout(bootcore_timeout: &Duration) -> Result<(), ValidationError> {
        if bootcore_timeout.as_secs() > 30 {
            return Err(ValidationError::new("bootcore_timeout must be 30 or less."));
        }

        Ok(())
    }

    pub fn video_off(video_off: &Duration) -> Result<(), ValidationError> {
        if video_off.as_secs() > 3600 {
            return Err(ValidationError::new("video_off must be 3600 or less."));
        }

        Ok(())
    }
}

/// The `[MiSTer]` section of the configuration file. This represents a single MiSTer section,
/// which contains most configurations.
///
/// The `Mister.ini` file specifies that one can override the default values by specifying the
/// `[MiSTer]` section with resolution specific configurations, e.g. `[video=320x240]`.
/// Because of this, we pretty much set everything as optional and defines the default in the
/// getters on the [`Config`] struct.
///
/// This allows us to overwrite only options which are defined in the subsections.
#[serde_as]
#[derive(Default, Debug, Clone, Deserialize, Merge, Validate)]
#[serde(default)]
pub struct MisterConfig {
    #[merge(strategy = merg::option::overwrite_some)]
    pub bootcore: Option<BootCoreConfig>,

    #[serde(alias = "ypbpr")]
    #[merge(strategy = merg::option::overwrite_some)]
    pub vga_mode: Option<VgaMode>,

    #[merge(strategy = merg::option::overwrite_some)]
    pub ntsc_mode: Option<NtscModeConfig>,

    #[merge(strategy = merg::option::overwrite_some)]
    pub reset_combo: Option<ResetComboConfig>,

    #[merge(strategy = merg::option::overwrite_some)]
    pub hdmi_limited: Option<HdmiLimitedConfig>,

    #[merge(strategy = merg::option::overwrite_some)]
    #[validate(range(max = 100))]
    pub mouse_throttle: Option<u8>,

    #[serde(with = "mister_hexa")]
    #[merge(strategy = merg::option::overwrite_some)]
    pub keyrah_mode: Option<u32>,

    /// Specify a custom aspect ratio in the format `a:b`. This can be repeated.
    /// They are applied in order, so the first one matching will be the one used.
    #[merge(strategy = merg::vec::append)]
    custom_aspect_ratio: Vec<AspectRatio>,

    /// Specify a custom aspect ratio, allowing for backward compatibility with older
    /// MiSTer config files. We only need 2 as that's what the previous version supported.
    #[merge(strategy = merg::option::overwrite_some)]
    pub custom_aspect_ratio_1: Option<AspectRatio>,

    /// Specify a custom aspect ratio, allowing for backward compatibility with older
    /// MiSTer config files. We only need 2 as that's what the previous version supported.
    #[merge(strategy = merg::option::overwrite_some)]
    pub custom_aspect_ratio_2: Option<AspectRatio>,

    /// Set to 1 to run scandoubler on VGA output always (depends on core).
    #[serde(with = "mister_bool")]
    #[merge(strategy = merg::option::overwrite_some)]
    pub forced_scandoubler: Option<bool>,

    /// Set to true to make the MENU key map to RGUI in Minimig (e.g. for Right Amiga).
    #[serde(with = "mister_bool")]
    #[merge(strategy = merg::option::overwrite_some)]
    pub key_menu_as_rgui: Option<bool>,

    /// Set to true for composite sync on HSync signal of VGA output.
    #[serde(with = "mister_bool", alias = "csync")]
    #[merge(strategy = merg::option::overwrite_some)]
    pub composite_sync: Option<bool>,

    /// Set to true to connect VGA to scaler output.
    #[serde(with = "mister_bool")]
    #[merge(strategy = merg::option::overwrite_some)]
    pub vga_scaler: Option<bool>,

    /// Set to true to enable sync on green (needs analog I/O board v6.0 or newer).
    #[serde(with = "mister_bool")]
    #[merge(strategy = merg::option::overwrite_some)]
    pub vga_sog: Option<bool>,

    /// Set to true for 96khz/16bit HDMI audio (48khz/16bit otherwise)
    #[serde(with = "mister_bool")]
    #[merge(strategy = merg::option::overwrite_some)]
    pub hdmi_audio_96k: Option<bool>,

    /// Set to true for DVI mode. Audio won't be transmitted through HDMI in DVI mode.
    #[serde(with = "mister_bool")]
    #[merge(strategy = merg::option::overwrite_some)]
    pub dvi_mode: Option<bool>,

    /// Set to true to enable core video timing over HDMI, use only with VGA converters.
    #[serde(with = "mister_bool")]
    #[merge(strategy = merg::option::overwrite_some)]
    pub direct_video: Option<bool>,

    /// Set to 0-10 (seconds) to display video info on startup/change
    #[serde_as(as = "Option<DurationSeconds<u64>>")]
    #[merge(strategy = merg::option::overwrite_some)]
    #[validate(custom(function = validate::video_info))]
    pub video_info: Option<Duration>,

    /// 1-10 (seconds) to display controller's button map upon first time key press
    /// 0 - disable
    #[serde_as(as = "Option<DurationSeconds<u64>>")]
    #[validate(custom(function = validate::controller_info))]
    #[merge(strategy = merg::option::overwrite_some)]
    pub controller_info: Option<Duration>,

    /// If you monitor doesn't support either very low (NTSC monitors may not support PAL) or
    /// very high (PAL monitors may not support NTSC) then you can set refresh_min and/or refresh_max
    /// parameters, so vsync_adjust won't be applied for refreshes outside specified.
    /// These parameters are valid only when vsync_adjust is non-zero.
    #[validate(range(min = 0.0, max = 150.0))]
    #[merge(strategy = merg::option::overwrite_some)]
    pub refresh_min: Option<f32>,

    /// If you monitor doesn't support either very low (NTSC monitors may not support PAL) or
    /// very high (PAL monitors may not support NTSC) then you can set refresh_min and/or refresh_max
    /// parameters, so vsync_adjust won't be applied for refreshes outside specified.
    /// These parameters are valid only when vsync_adjust is non-zero.
    #[validate(range(min = 0.0, max = 150.0))]
    #[merge(strategy = merg::option::overwrite_some)]
    refresh_max: Option<f32>,

    /// Set to 1 for automatic HDMI VSync rate adjust to match original VSync.
    /// Set to 2 for low latency mode (single buffer).
    /// This option makes video butter smooth like on original emulated system.
    /// Adjusting is done by changing pixel clock. Not every display supports variable pixel clock.
    /// For proper adjusting and to reduce possible out of range pixel clock, use 60Hz HDMI video
    /// modes as a base even for 50Hz systems.
    #[merge(strategy = merg::option::overwrite_some)]
    vsync_adjust: Option<VsyncAdjustConfig>,

    // TODO: figure this out.
    #[serde(with = "mister_bool")]
    #[merge(strategy = merg::option::overwrite_some)]
    kbd_nomouse: Option<bool>,

    #[serde(with = "mister_bool")]
    #[merge(strategy = merg::option::overwrite_some)]
    bootscreen: Option<bool>,

    /// 0 - scale to fit the screen height.
    /// 1 - use integer scale only.
    /// 2 - use 0.5 steps of scale.
    /// 3 - use 0.25 steps of scale.
    /// 4 - integer resolution scaling, use core aspect ratio
    /// 5 - integer resolution scaling, maintain display aspect ratio
    #[merge(strategy = merg::option::overwrite_some)]
    vscale_mode: Option<VideoScaleModeConfig>,

    /// Set vertical border for TVs cutting the upper/bottom parts of screen (1-399)
    #[validate(range(min = 0, max = 399))]
    #[merge(strategy = merg::option::overwrite_some)]
    pub vscale_border: Option<u16>,

    /// true - hides datecodes from rbf file names. Press F2 for quick temporary toggle
    #[serde(with = "mister_bool")]
    #[merge(strategy = merg::option::overwrite_some)]
    rbf_hide_datecode: Option<bool>,

    /// 1 - PAL mode for menu core
    #[serde(with = "mister_bool")]
    #[merge(strategy = merg::option::overwrite_some)]
    menu_pal: Option<bool>,

    /// 10-30 timeout before autoboot, comment for autoboot without timeout.
    #[serde_as(as = "Option<DurationSeconds<u64>>")]
    #[validate(custom(function = validate::bootcore_timeout))]
    #[merge(strategy = merg::option::overwrite_some)]
    bootcore_timeout: Option<Duration>,

    /// 0 - automatic, 1 - full size, 2 - 1/2 of resolution, 4 - 1/4 of resolution.
    #[merge(strategy = merg::option::overwrite_some)]
    pub fb_size: Option<FramebufferSizeConfig>,

    /// TODO: figure this out.
    #[serde(with = "mister_bool")]
    #[merge(strategy = merg::option::overwrite_some)]
    fb_terminal: Option<bool>,

    /// Display OSD menu rotated,  0 - no rotation, 1 - rotate right (+90°), 2 - rotate left (-90°)
    #[merge(strategy = merg::option::overwrite_some)]
    osd_rotate: Option<OsdRotateConfig>,

    /// 5-3600 timeout (in seconds) for OSD to disappear in Menu core. 0 - never timeout.
    /// Background picture will get darker after double timeout.
    #[serde_as(as = "Option<DurationSeconds<u64>>")]
    #[validate(custom(function = validate::osd_timeout))]
    #[merge(strategy = merg::option::overwrite_some)]
    osd_timeout: Option<Duration>,

    /// Defines internal joypad mapping from virtual SNES mapping in main to core mapping
    /// Set to 0 for name mapping (jn) (e.g. A button in SNES core = A button on controller regardless of position on pad)
    /// Set to 1 for positional mapping (jp) (e.g. A button in SNES core = East button on controller regardless of button name)
    #[serde(with = "mister_bool")]
    #[merge(strategy = merg::option::overwrite_some)]
    gamepad_defaults: Option<bool>,

    /// 1 - enables the recent file loaded/mounted.
    /// WARNING: This option will enable write to SD card on every load/mount which may wear the SD card after many writes to the same place
    ///          There is also higher chance to corrupt the File System if MiSTer will be reset or powered off while writing.
    #[serde(with = "mister_bool")]
    #[merge(strategy = merg::option::overwrite_some)]
    recents: Option<bool>,

    /// JammaSD/J-PAC/I-PAC keys to joysticks translation
    /// You have to provide correct VID and PID of your input device
    /// Examples: Legacy J-PAC with Mini-USB or USB capable I-PAC with PS/2 connectors VID=0xD209/PID=0x0301
    /// USB Capable J-PAC with only PS/2 connectors VID=0x04B4/PID=0x0101
    /// JammaSD: VID=0x04D8/PID=0xF3AD
    #[serde(with = "mister_hexa")]
    #[merge(strategy = merg::option::overwrite_some)]
    jamma_vid: Option<u16>,

    #[serde(with = "mister_hexa")]
    #[merge(strategy = merg::option::overwrite_some)]
    jamma_pid: Option<u16>,

    /// Disable merging input devices. Use if only Player1 works.
    /// Leave no_merge_pid empty to apply this to all devices with the same VID.
    #[serde(with = "mister_hexa")]
    #[merge(strategy = merg::option::overwrite_some)]
    no_merge_vid: Option<u16>,

    #[serde(with = "mister_hexa")]
    #[merge(strategy = merg::option::overwrite_some)]
    no_merge_pid: Option<u16>,

    #[serde(with = "mister_hexa_seq")]
    #[merge(strategy = merg::vec::append)]
    no_merge_vidpid: Vec<u32>,

    /// use specific (VID/PID) mouse X movement as a spinner and paddle. Use VID=0xFFFF/PID=0xFFFF to use all mice as spinners.
    #[serde(with = "mister_hexa")]
    #[merge(strategy = merg::option::overwrite_some)]
    spinner_vid: Option<u16>,

    #[serde(with = "mister_hexa")]
    #[merge(strategy = merg::option::overwrite_some)]
    spinner_pid: Option<u16>,

    // I WAS HERE.
    #[validate(range(min = -10000, max = 10000))]
    #[merge(strategy = merg::option::overwrite_some)]
    spinner_throttle: Option<i32>,

    #[merge(strategy = merg::option::overwrite_some)]
    spinner_axis: Option<u8>,

    #[serde(with = "mister_bool")]
    #[merge(strategy = merg::option::overwrite_some)]
    sniper_mode: Option<bool>,

    #[serde(with = "mister_bool")]
    #[merge(strategy = merg::option::overwrite_some)]
    browse_expand: Option<bool>,

    /// 0 - disable MiSTer logo in Menu core
    #[serde(with = "mister_bool")]
    #[merge(strategy = merg::option::overwrite_some)]
    logo: Option<bool>,

    #[serde(with = "mister_bool")]
    #[merge(strategy = merg::option::overwrite_some)]
    log_file_entry: Option<bool>,

    #[merge(strategy = merg::option::overwrite_some)]
    shmask_mode_default: Option<u8>,

    /// Automatically disconnect (and shutdown) Bluetooth input device if not use specified amount of time.
    /// Some controllers have no automatic shutdown built in and will keep connection till battery dry out.
    /// 0 - don't disconnect automatically, otherwise it's amount of minutes.
    #[serde_as(as = "Option<DurationMinutes>")]
    #[merge(strategy = merg::option::overwrite_some)]
    bt_auto_disconnect: Option<Duration>,

    #[serde(with = "mister_bool")]
    #[merge(strategy = merg::option::overwrite_some)]
    bt_reset_before_pair: Option<bool>,

    #[serde(alias = "video_mode")]
    #[merge(strategy = merg::option::overwrite_some)]
    pub video_conf: Option<String>,

    #[serde(alias = "video_mode_pal")]
    #[merge(strategy = merg::option::overwrite_some)]
    pub video_conf_pal: Option<String>,

    #[serde(alias = "video_mode_ntsc")]
    #[merge(strategy = merg::option::overwrite_some)]
    pub video_conf_ntsc: Option<String>,

    #[merge(strategy = merg::option::overwrite_some)]
    font: Option<String>,

    #[merge(strategy = merg::option::overwrite_some)]
    shared_folder: Option<String>,

    #[merge(strategy = merg::option::overwrite_some)]
    waitmount: Option<String>,

    #[merge(strategy = merg::option::overwrite_some)]
    afilter_default: Option<String>,

    #[merge(strategy = merg::option::overwrite_some)]
    vfilter_default: Option<String>,

    #[merge(strategy = merg::option::overwrite_some)]
    vfilter_vertical_default: Option<String>,

    #[merge(strategy = merg::option::overwrite_some)]
    vfilter_scanlines_default: Option<String>,

    #[merge(strategy = merg::option::overwrite_some)]
    shmask_default: Option<String>,

    #[merge(strategy = merg::option::overwrite_some)]
    preset_default: Option<String>,

    #[serde(default)]
    #[merge(strategy = merg::vec::append)]
    player_controller: Vec<Vec<String>>,

    #[serde(default)]
    #[merge(strategy = merg::vec::append)]
    player_1_controller: Vec<String>,
    #[serde(default)]
    #[merge(strategy = merg::vec::append)]
    player_2_controller: Vec<String>,
    #[serde(default)]
    #[merge(strategy = merg::vec::append)]
    player_3_controller: Vec<String>,
    #[serde(default)]
    #[merge(strategy = merg::vec::append)]
    player_4_controller: Vec<String>,
    #[serde(default)]
    #[merge(strategy = merg::vec::append)]
    player_5_controller: Vec<String>,
    #[serde(default)]
    #[merge(strategy = merg::vec::append)]
    player_6_controller: Vec<String>,

    #[serde(with = "mister_bool")]
    #[merge(strategy = merg::option::overwrite_some)]
    rumble: Option<bool>,

    #[merge(strategy = merg::option::overwrite_some)]
    #[validate(range(min = 0, max = 100))]
    wheel_force: Option<u8>,

    #[merge(strategy = merg::option::overwrite_some)]
    #[validate(range(min = 0, max = 1000))]
    wheel_range: Option<u16>,

    #[serde(with = "mister_bool")]
    #[merge(strategy = merg::option::overwrite_some)]
    hdmi_game_mode: Option<bool>,

    /// Variable Refresh Rate control
    /// 0 - Do not enable VRR (send no VRR control frames)
    /// 1 - Auto Detect VRR from display EDID.
    /// 2 - Force Enable Freesync
    /// 3 - Force Enable Vesa HDMI Forum VRR
    #[merge(strategy = merg::option::overwrite_some)]
    vrr_mode: Option<VrrModeConfig>,

    #[merge(strategy = merg::option::overwrite_some)]
    vrr_min_framerate: Option<u8>,

    #[merge(strategy = merg::option::overwrite_some)]
    vrr_max_framerate: Option<u8>,

    #[merge(strategy = merg::option::overwrite_some)]
    vrr_vesa_framerate: Option<u8>,

    /// output black frame in Menu core after timeout (is seconds). Valid only if osd_timout is non-zero.
    #[serde_as(as = "Option<DurationSeconds<u64>>")]
    #[merge(strategy = merg::option::overwrite_some)]
    #[validate(custom(function = validate::video_off))]
    video_off: Option<Duration>,

    #[serde(with = "mister_bool")]
    #[merge(strategy = merg::option::overwrite_some)]
    disable_autofire: Option<bool>,

    #[merge(strategy = merg::option::overwrite_some)]
    #[validate(range(min = 0, max = 100))]
    video_brightness: Option<u8>,

    #[merge(strategy = merg::option::overwrite_some)]
    #[validate(range(min = 0, max = 100))]
    video_contrast: Option<u8>,

    #[merge(strategy = merg::option::overwrite_some)]
    #[validate(range(min = 0, max = 100))]
    video_saturation: Option<u8>,

    #[merge(strategy = merg::option::overwrite_some)]
    #[validate(range(min = 0, max = 360))]
    video_hue: Option<u16>,

    #[merge(strategy = merg::option::overwrite_some)]
    video_gain_offset: Option<VideoGainOffsets>,

    #[merge(strategy = merg::option::overwrite_some)]
    hdr: Option<HdrConfig>,

    #[merge(strategy = merg::option::overwrite_some)]
    #[validate(range(min = 100, max = 10000))]
    hdr_max_nits: Option<u16>,

    #[merge(strategy = merg::option::overwrite_some)]
    #[validate(range(min = 100, max = 10000))]
    hdr_avg_nits: Option<u16>,

    #[serde(with = "mister_hexa_seq")]
    #[merge(strategy = merg::vec::append)]
    controller_unique_mapping: Vec<u32>,
}

impl MisterConfig {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn new_defaults() -> Self {
        let mut config = Self::new();
        config.set_defaults();
        config
    }

    /// Set the default values for this config.
    pub fn set_defaults(&mut self) {
        self.bootscreen.get_or_insert(true);
        self.fb_terminal.get_or_insert(true);
        self.controller_info.get_or_insert(Duration::from_secs(6));
        self.browse_expand.get_or_insert(true);
        self.logo.get_or_insert(true);
        self.rumble.get_or_insert(true);
        self.wheel_force.get_or_insert(50);
        self.hdr.get_or_insert(HdrConfig::default());
        self.hdr_avg_nits.get_or_insert(250);
        self.hdr_max_nits.get_or_insert(1000);
        self.video_brightness.get_or_insert(50);
        self.video_contrast.get_or_insert(50);
        self.video_saturation.get_or_insert(100);
        self.video_hue.get_or_insert(0);
        self.video_gain_offset
            .get_or_insert("1, 0, 1, 0, 1, 0".parse().unwrap());
        self.video_conf.get_or_insert("6".to_string());
    }

    pub fn custom_aspect_ratio(&self) -> Vec<AspectRatio> {
        if self.custom_aspect_ratio.is_empty() {
            self.custom_aspect_ratio_1
                .iter()
                .chain(self.custom_aspect_ratio_2.iter())
                .copied()
                .collect()
        } else {
            self.custom_aspect_ratio.clone()
        }
    }

    pub fn player_controller(&self) -> Vec<Vec<String>> {
        if self.player_controller.is_empty() {
            let mut vec = Vec::new();
            if !self.player_1_controller.is_empty() {
                vec.push(self.player_1_controller.clone());
            }
            if !self.player_2_controller.is_empty() {
                vec.push(self.player_2_controller.clone());
            }
            if !self.player_3_controller.is_empty() {
                vec.push(self.player_3_controller.clone());
            }
            if !self.player_4_controller.is_empty() {
                vec.push(self.player_4_controller.clone());
            }
            if !self.player_5_controller.is_empty() {
                vec.push(self.player_5_controller.clone());
            }
            if !self.player_6_controller.is_empty() {
                vec.push(self.player_6_controller.clone());
            }

            vec
        } else {
            self.player_controller.clone()
        }
    }

    #[inline]
    pub fn hdmi_limited(&self) -> HdmiLimitedConfig {
        self.hdmi_limited.unwrap_or_default()
    }

    #[inline]
    pub fn hdmi_game_mode(&self) -> bool {
        self.hdmi_game_mode.unwrap_or_default()
    }

    #[inline]
    pub fn hdr(&self) -> HdrConfig {
        self.hdr.unwrap_or_default()
    }

    #[inline]
    pub fn hdr_max_nits(&self) -> u16 {
        self.hdr_max_nits.unwrap_or(1000)
    }

    #[inline]
    pub fn hdr_avg_nits(&self) -> u16 {
        self.hdr_avg_nits.unwrap_or(250)
    }

    #[inline]
    pub fn dvi_mode(&self) -> bool {
        self.dvi_mode.unwrap_or_default()
    }

    #[inline]
    pub fn dvi_mode_raw(&self) -> Option<bool> {
        self.dvi_mode
    }

    #[inline]
    pub fn hdmi_audio_96k(&self) -> bool {
        self.hdmi_audio_96k.unwrap_or_default()
    }

    /// The video brightness, between [-0.5..0.5]
    #[inline]
    pub fn video_brightness(&self) -> f32 {
        (self.video_brightness.unwrap_or(50).clamp(0, 100) as f32 / 100.0) - 0.5
    }

    /// The video contrast, between [0..2]
    #[inline]
    pub fn video_contrast(&self) -> f32 {
        ((self.video_contrast.unwrap_or(50).clamp(0, 100) as f32 / 100.0) - 0.5) * 2. + 1.
    }

    /// The video saturation, between [0..1]
    #[inline]
    pub fn video_saturation(&self) -> f32 {
        self.video_saturation.unwrap_or(100).clamp(0, 100) as f32 / 100.
    }

    /// The video hue.
    #[inline]
    pub fn video_hue_radian(&self) -> f32 {
        (self.video_hue.unwrap_or_default() as f32) * f32::PI() / 180.
    }

    /// The video gains and offets.
    #[inline]
    pub fn video_gain_offset(&self) -> VideoGainOffsets {
        self.video_gain_offset.unwrap_or_default()
    }

    /// The VGA mode.
    #[inline]
    pub fn vga_mode(&self) -> VgaMode {
        self.vga_mode.unwrap_or_default()
    }

    /// Direct Video?
    #[inline]
    pub fn direct_video(&self) -> bool {
        self.direct_video.unwrap_or_default()
    }

    /// Whether to use vsync adjust.
    #[inline]
    pub fn vsync_adjust(&self) -> VsyncAdjustConfig {
        if self.direct_video() {
            VsyncAdjustConfig::Disabled
        } else {
            self.vsync_adjust.unwrap_or_default()
        }
    }

    /// Whether to use PAL in the menu.
    #[inline]
    pub fn menu_pal(&self) -> bool {
        self.menu_pal.unwrap_or_default()
    }

    /// Whether to force the scan doubler.
    #[inline]
    pub fn forced_scandoubler(&self) -> bool {
        self.forced_scandoubler.unwrap_or_default()
    }
}

#[cfg(test)]
pub mod testing {
    use std::path::PathBuf;

    pub(super) static mut ROOT: Option<PathBuf> = None;

    #[cfg(test)]
    pub fn set_config_root(root: impl Into<PathBuf>) {
        unsafe {
            ROOT = Some(root.into());
        }
    }
}

#[derive(Default, Debug, Clone, Deserialize, Merge)]
#[serde(default)]
pub struct Config {
    /// The MiSTer section which should be 99% of config files.
    #[serde(rename = "MiSTer")]
    mister: MisterConfig,

    /// The `[video=123x456@78]` sections, or core section.
    #[serde(flatten)]
    #[merge(strategy = merg::hashmap::recurse)]
    overrides: HashMap<String, MisterConfig>,
}

impl Config {
    fn root() -> PathBuf {
        #[cfg(test)]
        #[allow(static_mut_refs)]
        {
            unsafe { testing::ROOT.clone().unwrap() }
        }

        #[cfg(not(test))]
        PathBuf::from("/media/fat")
    }

    pub fn into_inner(self) -> MisterConfig {
        self.mister
    }

    pub fn base() -> Self {
        let path = Self::root().join("MiSTer.ini");
        Self::load(&path).map_or_else(
            |error| {
                info!(?path, ?error, "Failed to load MiSTer.ini, using defaults.");
                let mut c = Self::default();
                c.mister.set_defaults();
                c
            },
            |base| {
                info!(?path, "Successfully loaded MiSTer.ini");
                base
            },
        )
    }

    pub fn cores_root() -> PathBuf {
        Self::root()
    }

    pub fn config_root() -> PathBuf {
        Self::root().join("config")
    }

    pub fn last_core_data() -> Option<String> {
        std::fs::read_to_string(Self::config_root().join("lastcore.dat")).ok()
    }

    pub fn merge_core_override(&mut self, corename: &str) {
        if let Some(o) = self.overrides.get(corename) {
            self.mister.merge(o.clone());
        }
    }

    pub fn merge_video_override(&mut self, resolution: Resolution) {
        // Try to get `123x456@78` format first.
        let video_str = format!("video={resolution}");
        if let Some(o) = self.overrides.get(&video_str) {
            self.mister.merge(o.clone());
        }
    }

    /// Read INI config using our custom parser, then output the JSON, then parse that into
    /// the config struct. This is surprisingly fast, solid and byte compatible with the
    /// original CPP code (checked manually on various files).
    pub fn from_ini<R: io::Read>(mut content: R) -> Result<Self, ConfigError> {
        let mut s = String::new();
        content.read_to_string(&mut s)?;

        if s.is_empty() {
            return Ok(Default::default());
        }

        let json = ini::parse(&s)?.to_json_string(
            |name, value: &str| match name {
                "mouse_throttle"
                | "video_info"
                | "controller_info"
                | "refresh_min"
                | "refresh_max"
                | "vscale_border"
                | "bootcore_timeout"
                | "osd_timeout"
                | "spinner_throttle"
                | "spinner_axis"
                | "shmask_mode_default"
                | "bt_auto_disconnect"
                | "wheel_force"
                | "wheel_range"
                | "vrr_min_framerate"
                | "vrr_max_framerate"
                | "vrr_vesa_framerate"
                | "video_off"
                | "video_brightness"
                | "video_contrast"
                | "video_saturation"
                | "video_hue"
                | "hdr_max_nits"
                | "hdr_avg_nits" => Some(value.to_string()),
                _ => None,
            },
            |name| {
                [
                    "custom_aspect_ratio",
                    "no_merge_vidpid",
                    "player_controller",
                    "player_1_controller",
                    "player_2_controller",
                    "player_3_controller",
                    "player_4_controller",
                    "player_5_controller",
                    "player_6_controller",
                    "controller_unique_mapping",
                ]
                .contains(&name)
            },
            |name: &str| -> Option<&str> {
                if name == "ypbpr" {
                    Some("vga_mode")
                } else {
                    None
                }
            },
        );

        Config::from_json(json.as_bytes())
    }

    pub fn from_json<R: io::Read>(mut content: R) -> Result<Self, ConfigError> {
        let mut c = String::new();
        content.read_to_string(&mut c)?;
        Ok(json5::from_str(&c)?)
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        match path.extension().and_then(|ext| ext.to_str()) {
            Some("ini") => Self::from_ini(std::fs::File::open(path)?),
            Some("json") => Self::from_json(std::fs::File::open(path)?),
            _ => Err(
                io::Error::new(io::ErrorKind::InvalidInput, "Invalid config file extension").into(),
            ),
        }
    }

    /// Merge a configuration file with another.
    pub fn merge(&mut self, other: Config) {
        Merge::merge(self, other);
    }
}

#[test]
fn works_with_empty_file() {
    Config::from_ini(io::empty()).unwrap();
}

#[cfg(test)]
mod examples {
    use super::*;

    #[rstest::rstest]
    fn works_with_example(#[files("tests/assets/config/*.ini")] p: PathBuf) {
        Config::load(p).unwrap();
    }
}
