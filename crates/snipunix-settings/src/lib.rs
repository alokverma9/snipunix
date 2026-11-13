//! Persistent, user-editable Snipunix settings.
//!
//! This crate is UI-independent. It owns the settings model, defaults, range
//! clamping, XDG path resolution, and atomic TOML storage. The GTK settings
//! pages and the capture flow are thin adapters over this module.

#![forbid(unsafe_code)]

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Current settings schema version.
pub const SCHEMA_VERSION: u32 = 1;
/// Default delay range, in seconds.
pub const MAX_DELAY_SECONDS: u32 = 10;
/// Default filename template, expanded with `glib::DateTime`.
pub const DEFAULT_FILENAME_TEMPLATE: &str = "Screenshot_%Y-%m-%d_%H-%M-%S";
/// Default OCR model directory name below the XDG data directory.
pub const MODEL_DIRECTORY_NAME: &str = "models";
/// Lowest recording frame rate offered in Settings.
pub const MIN_RECORDING_FPS: u32 = 10;
/// Highest recording frame rate offered in Settings.
pub const MAX_RECORDING_FPS: u32 = 60;
/// Lowest accepted recording bitrate, in kbit/s.
pub const MIN_RECORDING_BITRATE_KBPS: u32 = 500;
/// Highest accepted recording bitrate, in kbit/s.
pub const MAX_RECORDING_BITRATE_KBPS: u32 = 50_000;

/// All persisted settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub schema_version: u32,
    pub onboarding_complete: bool,
    pub capture: CaptureSettings,
    pub shortcuts: ShortcutSettings,
    pub files: FileSettings,
    pub ocr: OcrSettings,
    pub appearance: AppearanceSettings,
    pub recording: RecordingSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            onboarding_complete: false,
            capture: CaptureSettings::default(),
            shortcuts: ShortcutSettings::default(),
            files: FileSettings::default(),
            ocr: OcrSettings::default(),
            appearance: AppearanceSettings::default(),
            recording: RecordingSettings::default(),
        }
    }
}

impl Settings {
    /// Loads settings from `path`. A missing file is not an error and returns
    /// defaults, which is what a first run needs.
    pub fn load(path: &Path) -> Result<Self, SettingsError> {
        match fs::read_to_string(path) {
            Ok(contents) => Self::from_toml(&contents),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(SettingsError::Io {
                path: path.to_path_buf(),
                source: error,
            }),
        }
    }

    /// Parses and sanitises TOML.
    pub fn from_toml(contents: &str) -> Result<Self, SettingsError> {
        let mut settings: Self = toml::from_str(contents)?;
        settings.sanitize();
        Ok(settings)
    }

    /// Serialises the current settings.
    pub fn to_toml(&self) -> Result<String, SettingsError> {
        let mut clean = self.clone();
        clean.sanitize();
        Ok(toml::to_string_pretty(&clean)?)
    }

    /// Writes settings atomically: the target is only replaced after a complete
    /// temporary file has been written and flushed.
    pub fn save(&self, path: &Path) -> Result<(), SettingsError> {
        let contents = self.to_toml()?;
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(|source| SettingsError::Io {
                    path: parent.to_path_buf(),
                    source,
                })?;
            }
        }
        let temporary = temporary_path(path);
        {
            let mut options = fs::OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options
                .open(&temporary)
                .map_err(|source| SettingsError::Io {
                    path: temporary.clone(),
                    source,
                })?;
            file.write_all(contents.as_bytes())
                .and_then(|()| file.sync_all())
                .map_err(|source| SettingsError::Io {
                    path: temporary.clone(),
                    source,
                })?;
        }
        fs::rename(&temporary, path).map_err(|source| SettingsError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(())
    }

    /// Clamps values into their supported ranges and fills empty required
    /// strings with defaults. Unknown TOML keys are ignored by serde.
    pub fn sanitize(&mut self) {
        self.schema_version = SCHEMA_VERSION;
        self.capture.delay = self.capture.delay.min(MAX_DELAY_SECONDS);
        self.files.jpeg_quality = self.files.jpeg_quality.clamp(1, 100);
        if self.files.filename_template.trim().is_empty() {
            self.files.filename_template = DEFAULT_FILENAME_TEMPLATE.to_string();
        }
        if self.shortcuts.region_trigger.trim().is_empty() {
            self.shortcuts.region_trigger = ShortcutSettings::default().region_trigger;
        }
        if self.shortcuts.screen_trigger.trim().is_empty() {
            self.shortcuts.screen_trigger = ShortcutSettings::default().screen_trigger;
        }
        self.appearance.dim_strength = self.appearance.dim_strength.clamp(0.10, 0.85);
        self.recording.fps = self
            .recording
            .fps
            .clamp(MIN_RECORDING_FPS, MAX_RECORDING_FPS);
        self.recording.bitrate_kbps = self
            .recording
            .bitrate_kbps
            .clamp(MIN_RECORDING_BITRATE_KBPS, MAX_RECORDING_BITRATE_KBPS);
        if self
            .recording
            .restore_token
            .as_deref()
            .is_some_and(|token| token.trim().is_empty())
        {
            self.recording.restore_token = None;
        }
    }

    /// Directory for finished recordings.
    #[must_use]
    pub fn recording_directory(&self) -> PathBuf {
        self.recording
            .directory
            .clone()
            .unwrap_or_else(|| videos_dir().join("Screen Recordings"))
    }

    /// Default config file: `$XDG_CONFIG_HOME/snipunix/settings.toml`.
    #[must_use]
    pub fn default_path() -> PathBuf {
        config_dir().join("snipunix").join("settings.toml")
    }

    /// Directory for downloaded/bundled OCR models.
    #[must_use]
    pub fn model_directory(&self) -> PathBuf {
        self.ocr
            .model_directory
            .clone()
            .unwrap_or_else(|| data_dir().join("snipunix").join(MODEL_DIRECTORY_NAME))
    }
}

/// Capture page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptureSettings {
    pub default_mode: CaptureMode,
    pub delay: u32,
    pub copy_after_capture: bool,
    pub remember_last_region: bool,
    /// Draw the pointer into native X11 captures.
    pub include_cursor: bool,
}

impl Default for CaptureSettings {
    fn default() -> Self {
        Self {
            default_mode: CaptureMode::Region,
            delay: 0,
            copy_after_capture: true,
            remember_last_region: true,
            include_cursor: false,
        }
    }
}

/// Default capture mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureMode {
    Region,
    Screen,
}

/// Shortcut page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShortcutSettings {
    pub register_on_startup: bool,
    pub region_trigger: String,
    pub screen_trigger: String,
    /// Last description returned by the portal, for display in Settings.
    pub bound_description: String,
}

impl Default for ShortcutSettings {
    fn default() -> Self {
        Self {
            register_on_startup: false,
            region_trigger: "CTRL+SHIFT+s".to_string(),
            screen_trigger: "CTRL+SHIFT+a".to_string(),
            bound_description: String::new(),
        }
    }
}

/// Files and saving page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FileSettings {
    pub auto_save: bool,
    pub directory: Option<PathBuf>,
    pub filename_template: String,
    pub format: SaveFormat,
    pub jpeg_quality: u8,
}

impl Default for FileSettings {
    fn default() -> Self {
        Self {
            auto_save: false,
            directory: None,
            filename_template: DEFAULT_FILENAME_TEMPLATE.to_string(),
            format: SaveFormat::Png,
            jpeg_quality: 90,
        }
    }
}

impl FileSettings {
    /// Effective save directory: the configured directory or
    /// `$XDG_PICTURES_DIR/Screenshots`, falling back to `~/Pictures/Screenshots`.
    #[must_use]
    pub fn directory_or_default(&self) -> PathBuf {
        if let Some(directory) = &self.directory {
            return directory.clone();
        }
        pictures_dir().join("Screenshots")
    }
}

/// Export format selected in settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SaveFormat {
    Png,
    Jpeg,
    Webp,
}

impl SaveFormat {
    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Webp => "webp",
        }
    }

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Png => "PNG",
            Self::Jpeg => "JPEG",
            Self::Webp => "WebP",
        }
    }
}

/// Recording page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RecordingSettings {
    pub fps: u32,
    pub bitrate_kbps: u32,
    /// Ask the compositor to draw the pointer into the stream.
    pub include_cursor: bool,
    pub microphone: bool,
    pub system_audio: bool,
    /// Output directory override; defaults to `$XDG_VIDEOS_DIR/Screen Recordings`.
    pub directory: Option<PathBuf>,
    /// Opaque ScreenCast portal token that skips source selection next time.
    pub restore_token: Option<String>,
}

impl Default for RecordingSettings {
    fn default() -> Self {
        Self {
            fps: 30,
            bitrate_kbps: 6000,
            include_cursor: true,
            microphone: false,
            system_audio: false,
            directory: None,
            restore_token: None,
        }
    }
}

/// OCR page.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OcrSettings {
    pub auto_copy: bool,
    pub remove_line_breaks: bool,
    pub model_directory: Option<PathBuf>,
}

/// Appearance page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppearanceSettings {
    pub show_magnifier: bool,
    pub dim_strength: f64,
    pub reduced_motion: bool,
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            show_magnifier: true,
            dim_strength: 0.45,
            reduced_motion: false,
        }
    }
}

/// Settings storage errors.
#[derive(Debug, Error)]
pub enum SettingsError {
    #[error("could not read or write {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("settings TOML is invalid: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("could not serialise settings: {0}")]
    Serialize(#[from] toml::ser::Error),
}

fn temporary_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.tmp", std::process::id()));
    path.with_file_name(name)
}

fn config_dir() -> PathBuf {
    xdg_dir("XDG_CONFIG_HOME", ".config")
}

fn data_dir() -> PathBuf {
    xdg_dir("XDG_DATA_HOME", ".local/share")
}

fn xdg_dir(variable: &str, fallback: &str) -> PathBuf {
    if let Some(value) = std::env::var_os(variable) {
        let path = PathBuf::from(value);
        if path.is_absolute() {
            return path;
        }
    }
    home_dir().join(fallback)
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("."), PathBuf::from)
}

fn pictures_dir() -> PathBuf {
    if let Some(value) = user_dir("XDG_PICTURES_DIR") {
        return value;
    }
    home_dir().join("Pictures")
}

fn videos_dir() -> PathBuf {
    if let Some(value) = user_dir("XDG_VIDEOS_DIR") {
        return value;
    }
    home_dir().join("Videos")
}

/// Reads a single `XDG_PICTURES_DIR` entry from `user-dirs.dirs`, which is how
/// desktop environments record localised folder names.
fn user_dir(name: &str) -> Option<PathBuf> {
    let path = config_dir().join("user-dirs.dirs");
    let contents = fs::read_to_string(path).ok()?;
    for line in contents.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key != name {
            continue;
        }
        let value = value.trim().trim_matches('"');
        let expanded = value
            .strip_prefix("$HOME")
            .map(|rest| home_dir().join(rest.trim_start_matches('/')))
            .unwrap_or_else(|| PathBuf::from(value));
        if expanded.is_absolute() {
            return Some(expanded);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_cover_every_page() {
        let settings = Settings::default();
        assert_eq!(settings.schema_version, SCHEMA_VERSION);
        assert!(!settings.onboarding_complete);
        assert_eq!(settings.capture.default_mode, CaptureMode::Region);
        assert!(settings.capture.copy_after_capture);
        assert_eq!(settings.capture.delay, 0);
        assert!(!settings.shortcuts.register_on_startup);
        assert_eq!(settings.shortcuts.region_trigger, "CTRL+SHIFT+s");
        assert_eq!(settings.files.format, SaveFormat::Png);
        assert_eq!(settings.files.jpeg_quality, 90);
        assert!(!settings.ocr.auto_copy);
        assert!(settings.appearance.show_magnifier);
        assert_eq!(settings.recording.fps, 30);
        assert_eq!(settings.recording.bitrate_kbps, 6000);
        assert!(settings.recording.include_cursor);
        assert!(!settings.recording.microphone);
        assert!(!settings.recording.system_audio);
        assert!(settings.recording.restore_token.is_none());
    }

    #[test]
    fn toml_round_trip_preserves_values() {
        let mut settings = Settings {
            onboarding_complete: true,
            ..Settings::default()
        };
        settings.capture.delay = 4;
        settings.capture.default_mode = CaptureMode::Screen;
        settings.files.auto_save = true;
        settings.files.directory = Some(PathBuf::from("/tmp/shots"));
        settings.files.format = SaveFormat::Webp;
        settings.ocr.auto_copy = true;
        settings.ocr.remove_line_breaks = true;
        settings.shortcuts.register_on_startup = true;
        settings.shortcuts.bound_description = "Ctrl+Shift+S — region".to_string();
        settings.appearance.dim_strength = 0.6;
        settings.recording.fps = 60;
        settings.recording.bitrate_kbps = 12_000;
        settings.recording.microphone = true;
        settings.recording.system_audio = true;
        settings.recording.restore_token = Some("token-123".to_string());

        let text = settings.to_toml().unwrap();
        let parsed = Settings::from_toml(&text).unwrap();
        assert_eq!(parsed, settings);
    }

    #[test]
    fn missing_keys_and_unknown_keys_use_defaults() {
        let parsed = Settings::from_toml(
            r#"
            unknown_future_key = 42
            [capture]
            delay = 3
            "#,
        )
        .unwrap();
        assert_eq!(parsed.capture.delay, 3);
        assert_eq!(parsed.files.format, SaveFormat::Png);
        assert_eq!(parsed.shortcuts.region_trigger, "CTRL+SHIFT+s");
    }

    #[test]
    fn invalid_toml_is_a_typed_error() {
        let error = Settings::from_toml("capture = [").unwrap_err();
        assert!(matches!(error, SettingsError::Parse(_)));
    }

    #[test]
    fn save_is_atomic_and_creates_parents() {
        let unique = format!(
            "snipunix-settings-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let directory = std::env::temp_dir().join(unique);
        let path = directory.join("nested/settings.toml");
        let mut settings = Settings::default();
        settings.capture.delay = 7;
        settings.save(&path).unwrap();
        assert_eq!(Settings::load(&path).unwrap(), settings);
        let leftover: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp"))
            .collect();
        assert!(leftover.is_empty());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn missing_file_returns_defaults() {
        let path = std::env::temp_dir().join("snipunix-settings-definitely-missing.toml");
        let _ = fs::remove_file(&path);
        assert_eq!(Settings::load(&path).unwrap(), Settings::default());
    }

    #[test]
    fn sanitize_clamps_ranges_and_restores_empty_strings() {
        let mut settings = Settings::default();
        settings.capture.delay = 99;
        settings.files.jpeg_quality = 0;
        settings.files.filename_template = "   ".to_string();
        settings.shortcuts.region_trigger.clear();
        settings.shortcuts.screen_trigger.clear();
        settings.appearance.dim_strength = 2.0;
        settings.recording.fps = 1;
        settings.recording.bitrate_kbps = 1;
        settings.recording.restore_token = Some("   ".to_string());
        settings.sanitize();
        assert_eq!(settings.capture.delay, MAX_DELAY_SECONDS);
        assert_eq!(settings.files.jpeg_quality, 1);
        assert_eq!(settings.files.filename_template, DEFAULT_FILENAME_TEMPLATE);
        assert_eq!(settings.shortcuts.region_trigger, "CTRL+SHIFT+s");
        assert_eq!(settings.shortcuts.screen_trigger, "CTRL+SHIFT+a");
        assert_eq!(settings.appearance.dim_strength, 0.85);
        assert_eq!(settings.recording.fps, MIN_RECORDING_FPS);
        assert_eq!(settings.recording.bitrate_kbps, MIN_RECORDING_BITRATE_KBPS);
        assert!(settings.recording.restore_token.is_none());
    }

    #[test]
    fn recording_directory_honours_override() {
        let mut settings = Settings::default();
        settings.recording.directory = Some(PathBuf::from("/tmp/recordings"));
        assert_eq!(
            settings.recording_directory(),
            PathBuf::from("/tmp/recordings")
        );
        settings.recording.directory = None;
        assert!(
            settings
                .recording_directory()
                .ends_with("Screen Recordings")
        );
    }

    #[test]
    fn model_directory_honours_override() {
        let mut settings = Settings::default();
        settings.ocr.model_directory = Some(PathBuf::from("/opt/models"));
        assert_eq!(settings.model_directory(), PathBuf::from("/opt/models"));
        settings.ocr.model_directory = None;
        assert!(settings.model_directory().ends_with("snipunix/models"));
    }
}
