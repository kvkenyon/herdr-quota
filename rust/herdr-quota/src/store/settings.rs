//! The finite schema-v5 dashboard settings store.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Serialize, ser::SerializeStruct};
use serde_json::{Map, Value};

use super::atomic::{self, AtomicError, ReplaceError};

/// The current settings schema.
pub const SETTINGS_SCHEMA_VERSION: u64 = 5;

/// A provider that this product can show and persist.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SupportedProvider {
    Claude,
    Codex,
    Cursor,
    Kimi,
    Grok,
    Copilot,
}

impl Serialize for SupportedProvider {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.id())
    }
}

/// The finite provider catalog in its default display order.
pub const SUPPORTED_PROVIDERS: [SupportedProvider; 6] = [
    SupportedProvider::Claude,
    SupportedProvider::Codex,
    SupportedProvider::Cursor,
    SupportedProvider::Kimi,
    SupportedProvider::Grok,
    SupportedProvider::Copilot,
];

impl SupportedProvider {
    pub fn from_id(value: &str) -> Option<Self> {
        match value {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            "cursor" => Some(Self::Cursor),
            "kimi" => Some(Self::Kimi),
            "grok" => Some(Self::Grok),
            "copilot" => Some(Self::Copilot),
            _ => None,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Cursor => "cursor",
            Self::Kimi => "kimi",
            Self::Grok => "grok",
            Self::Copilot => "copilot",
        }
    }
}

/// The gauge value that the dashboard shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeterMode {
    Remaining,
    Used,
}

impl Serialize for MeterMode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(match self {
            Self::Remaining => "remaining",
            Self::Used => "used",
        })
    }
}

/// The provider identity treatment used by the terminal renderer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProviderIdentityMode {
    LogoOnly,
    #[default]
    LogoAndName,
    NameOnly,
}

impl Serialize for ProviderIdentityMode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(match self {
            Self::LogoOnly => "logo_only",
            Self::LogoAndName => "logo_and_name",
            Self::NameOnly => "name_only",
        })
    }
}

/// The screen shown when the dashboard starts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StartupView {
    #[default]
    Overview,
    Details,
}

impl Serialize for StartupView {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(match self {
            Self::Overview => "overview",
            Self::Details => "details",
        })
    }
}

/// The remaining-capacity cue policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemainingThreshold {
    Off,
    Percent25,
    Percent10,
    Percent5,
}

impl Serialize for RemainingThreshold {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Off => serializer.serialize_str("off"),
            Self::Percent25 => serializer.serialize_u8(25),
            Self::Percent10 => serializer.serialize_u8(10),
            Self::Percent5 => serializer.serialize_u8(5),
        }
    }
}

/// The complete settings that can enter the Rust runtime.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DashboardSettings {
    pub schema_version: u64,
    pub provider_order: Vec<SupportedProvider>,
    pub hidden_providers: Vec<SupportedProvider>,
    pub meter_mode: MeterMode,
    pub provider_identity: ProviderIdentityMode,
    pub remaining_threshold: RemainingThreshold,
    pub forecast_before_reset: bool,
    pub startup_view: StartupView,
}

impl Default for DashboardSettings {
    fn default() -> Self {
        Self {
            schema_version: SETTINGS_SCHEMA_VERSION,
            provider_order: SUPPORTED_PROVIDERS.to_vec(),
            hidden_providers: Vec::new(),
            meter_mode: MeterMode::Remaining,
            provider_identity: ProviderIdentityMode::LogoAndName,
            remaining_threshold: RemainingThreshold::Off,
            forecast_before_reset: false,
            startup_view: StartupView::Overview,
        }
    }
}

/// The result state for one settings load.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsAvailability {
    Ready,
    FirstRun,
    Recovered,
    Incompatible,
    Unavailable,
}

/// Settings plus their storage state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsLoadResult {
    pub settings: DashboardSettings,
    pub availability: SettingsAvailability,
}

/// A settings document or storage error.
#[derive(Debug)]
pub enum SettingsError {
    Corrupt,
    Incompatible,
    Unsafe,
    Unavailable(io::Error),
}

impl std::fmt::Display for SettingsError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Corrupt => formatter.write_str("settings_corrupt"),
            Self::Incompatible => formatter.write_str("settings_incompatible"),
            Self::Unsafe => formatter.write_str("settings_unsafe"),
            Self::Unavailable(error) => write!(formatter, "settings_unavailable: {error}"),
        }
    }
}

impl std::error::Error for SettingsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Unavailable(error) => Some(error),
            Self::Corrupt | Self::Incompatible | Self::Unsafe => None,
        }
    }
}

/// Return the settings path for a selected config home and user home.
pub fn settings_path(config_home: Option<&OsStr>, home: &Path) -> PathBuf {
    let root = config_home
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    root.join("herdr-quota").join("settings.json")
}

/// Return the settings path from the current process environment.
pub fn settings_path_from_environment() -> Result<PathBuf, SettingsError> {
    let config_home = std::env::var_os("XDG_CONFIG_HOME");
    if let Some(config_home) = config_home.as_deref().filter(|value| !value.is_empty()) {
        return Ok(settings_path(Some(config_home), Path::new("")));
    }
    let home = home_directory().ok_or_else(|| {
        SettingsError::Unavailable(io::Error::new(
            io::ErrorKind::NotFound,
            "user home directory is not set",
        ))
    })?;
    Ok(settings_path(None, &home))
}

/// Parse only the finite settings schemas.
pub fn parse_settings_document(value: &Value) -> Result<DashboardSettings, SettingsError> {
    let object = value.as_object().ok_or(SettingsError::Corrupt)?;
    let schema_version = parse_schema_version(object)?;
    let meter_mode = match object.get("meterMode").and_then(Value::as_str) {
        Some("remaining") => MeterMode::Remaining,
        Some("used") => MeterMode::Used,
        _ => return Err(SettingsError::Corrupt),
    };
    let provider_identity = if schema_version < 5 {
        ProviderIdentityMode::LogoAndName
    } else {
        match object.get("providerIdentity").and_then(Value::as_str) {
            Some("logo_only") => ProviderIdentityMode::LogoOnly,
            Some("logo_and_name") => ProviderIdentityMode::LogoAndName,
            Some("name_only") => ProviderIdentityMode::NameOnly,
            _ => return Err(SettingsError::Corrupt),
        }
    };
    let remaining_threshold = if schema_version == 1 {
        RemainingThreshold::Off
    } else {
        parse_remaining_threshold(object.get("remainingThreshold"))?
    };
    let forecast_before_reset = if schema_version == 1 {
        false
    } else {
        object
            .get("forecastBeforeReset")
            .and_then(Value::as_bool)
            .ok_or(SettingsError::Corrupt)?
    };
    let startup_view = if schema_version < 4 {
        StartupView::Overview
    } else {
        match object.get("startupView").and_then(Value::as_str) {
            Some("overview") => StartupView::Overview,
            Some("details") => StartupView::Details,
            _ => return Err(SettingsError::Corrupt),
        }
    };

    Ok(DashboardSettings {
        schema_version: SETTINGS_SCHEMA_VERSION,
        provider_order: provider_list(object.get("providerOrder"), true)?,
        hidden_providers: provider_list(object.get("hiddenProviders"), false)?,
        meter_mode,
        provider_identity,
        remaining_threshold,
        forecast_before_reset,
        startup_view,
    })
}

/// Normalize a typed settings value before a save.
pub fn normalize_settings(
    settings: &DashboardSettings,
) -> Result<DashboardSettings, SettingsError> {
    Ok(DashboardSettings {
        schema_version: SETTINGS_SCHEMA_VERSION,
        provider_order: normalize_provider_list(&settings.provider_order, true)?,
        hidden_providers: normalize_provider_list(&settings.hidden_providers, false)?,
        meter_mode: settings.meter_mode,
        provider_identity: settings.provider_identity,
        remaining_threshold: settings.remaining_threshold,
        forecast_before_reset: settings.forecast_before_reset,
        startup_view: settings.startup_view,
    })
}

/// Load and save one settings file.
#[derive(Clone, Debug)]
pub struct SettingsStore {
    path: PathBuf,
}

impl SettingsStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn from_environment() -> Result<Self, SettingsError> {
        Ok(Self::new(settings_path_from_environment()?))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> SettingsLoadResult {
        let defaults = DashboardSettings::default();
        let bytes = match atomic::read_regular(&self.path) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => {
                return SettingsLoadResult {
                    settings: defaults,
                    availability: SettingsAvailability::FirstRun,
                };
            }
            Err(_) => {
                return SettingsLoadResult {
                    settings: defaults,
                    availability: SettingsAvailability::Unavailable,
                };
            }
        };

        let parsed = serde_json::from_slice::<Value>(&bytes)
            .map_err(|_| SettingsError::Corrupt)
            .and_then(|value| parse_settings_document(&value));
        match parsed {
            Ok(settings) => SettingsLoadResult {
                settings,
                availability: SettingsAvailability::Ready,
            },
            Err(SettingsError::Incompatible) => SettingsLoadResult {
                settings: defaults,
                availability: SettingsAvailability::Incompatible,
            },
            Err(SettingsError::Corrupt) => {
                match atomic::quarantine_if_unchanged(&self.path, &bytes) {
                    Ok(_) => SettingsLoadResult {
                        settings: defaults,
                        availability: SettingsAvailability::Recovered,
                    },
                    Err(_) => SettingsLoadResult {
                        settings: defaults,
                        availability: SettingsAvailability::Unavailable,
                    },
                }
            }
            Err(SettingsError::Unsafe | SettingsError::Unavailable(_)) => SettingsLoadResult {
                settings: defaults,
                availability: SettingsAvailability::Unavailable,
            },
        }
    }

    pub fn save(&self, settings: &DashboardSettings) -> Result<(), SettingsError> {
        let normalized = normalize_settings(settings)?;
        let bytes = serialize_settings(&normalized)?;
        atomic::replace_atomically(&self.path, &bytes, preserve_future_version).map_err(|error| {
            match error {
                ReplaceError::Storage(error) => map_atomic_error(error),
                ReplaceError::Validation(error) => error,
            }
        })
    }
}

struct SettingsDocument<'a> {
    schema_version: u64,
    provider_order: &'a [SupportedProvider],
    hidden_providers: &'a [SupportedProvider],
    meter_mode: MeterMode,
    provider_identity: ProviderIdentityMode,
    remaining_threshold: RemainingThreshold,
    forecast_before_reset: bool,
    startup_view: StartupView,
}

impl Serialize for SettingsDocument<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut document = serializer.serialize_struct("SettingsDocument", 8)?;
        document.serialize_field("schemaVersion", &self.schema_version)?;
        document.serialize_field("providerOrder", self.provider_order)?;
        document.serialize_field("hiddenProviders", self.hidden_providers)?;
        document.serialize_field("meterMode", &self.meter_mode)?;
        document.serialize_field("providerIdentity", &self.provider_identity)?;
        document.serialize_field("remainingThreshold", &self.remaining_threshold)?;
        document.serialize_field("forecastBeforeReset", &self.forecast_before_reset)?;
        document.serialize_field("startupView", &self.startup_view)?;
        document.end()
    }
}

fn serialize_settings(settings: &DashboardSettings) -> Result<Vec<u8>, SettingsError> {
    let document = SettingsDocument {
        schema_version: SETTINGS_SCHEMA_VERSION,
        provider_order: &settings.provider_order,
        hidden_providers: &settings.hidden_providers,
        meter_mode: settings.meter_mode,
        provider_identity: settings.provider_identity,
        remaining_threshold: settings.remaining_threshold,
        forecast_before_reset: settings.forecast_before_reset,
        startup_view: settings.startup_view,
    };
    let mut bytes = serde_json::to_vec(&document).map_err(|error| {
        SettingsError::Unavailable(io::Error::new(io::ErrorKind::InvalidData, error))
    })?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn preserve_future_version(bytes: Option<&[u8]>) -> Result<(), SettingsError> {
    let Some(bytes) = bytes else {
        return Ok(());
    };
    let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
        return Ok(());
    };
    match parse_settings_document(&value) {
        Err(SettingsError::Incompatible) => Err(SettingsError::Incompatible),
        _ => Ok(()),
    }
}

fn parse_schema_version(object: &Map<String, Value>) -> Result<u64, SettingsError> {
    match object.get("schemaVersion") {
        Some(Value::Number(number)) => match number.as_u64() {
            Some(1..=SETTINGS_SCHEMA_VERSION) => Ok(number.as_u64().expect("number checked")),
            _ => Err(SettingsError::Incompatible),
        },
        _ => Err(SettingsError::Corrupt),
    }
}

fn parse_remaining_threshold(value: Option<&Value>) -> Result<RemainingThreshold, SettingsError> {
    match value {
        Some(Value::String(value)) if value == "off" => Ok(RemainingThreshold::Off),
        Some(Value::Number(value)) if value.as_u64() == Some(25) => {
            Ok(RemainingThreshold::Percent25)
        }
        Some(Value::Number(value)) if value.as_u64() == Some(10) => {
            Ok(RemainingThreshold::Percent10)
        }
        Some(Value::Number(value)) if value.as_u64() == Some(5) => Ok(RemainingThreshold::Percent5),
        _ => Err(SettingsError::Corrupt),
    }
}

fn provider_list(
    value: Option<&Value>,
    append_missing: bool,
) -> Result<Vec<SupportedProvider>, SettingsError> {
    let values = value
        .and_then(Value::as_array)
        .ok_or(SettingsError::Corrupt)?;
    let mut providers = Vec::new();
    for value in values {
        let id = value.as_str().ok_or(SettingsError::Corrupt)?;
        if let Some(provider) = SupportedProvider::from_id(id) {
            providers.push(provider);
        }
    }
    normalize_provider_list(&providers, append_missing)
}

fn normalize_provider_list(
    values: &[SupportedProvider],
    append_missing: bool,
) -> Result<Vec<SupportedProvider>, SettingsError> {
    let mut seen = HashSet::new();
    let mut providers = Vec::with_capacity(SUPPORTED_PROVIDERS.len());
    for provider in values {
        if !seen.insert(*provider) {
            return Err(SettingsError::Corrupt);
        }
        providers.push(*provider);
    }
    if append_missing {
        for provider in SUPPORTED_PROVIDERS {
            if seen.insert(provider) {
                providers.push(provider);
            }
        }
    }
    Ok(providers)
}

fn map_atomic_error(error: AtomicError) -> SettingsError {
    match error {
        AtomicError::UnsafeTarget => SettingsError::Unsafe,
        AtomicError::InvalidPath => SettingsError::Unavailable(io::Error::new(
            io::ErrorKind::InvalidInput,
            "settings path has no file name",
        )),
        AtomicError::Io(error) => SettingsError::Unavailable(error),
    }
}

#[cfg(unix)]
fn home_directory() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

#[cfg(windows)]
fn home_directory() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            let drive = std::env::var_os("HOMEDRIVE")?;
            let path = std::env::var_os("HOMEPATH")?;
            if drive.is_empty() || path.is_empty() {
                return None;
            }
            let mut home = PathBuf::from(drive);
            home.push(path);
            Some(home)
        })
}

#[cfg(not(any(unix, windows)))]
fn home_directory() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::{
        DashboardSettings, MeterMode, ProviderIdentityMode, RemainingThreshold,
        SETTINGS_SCHEMA_VERSION, SUPPORTED_PROVIDERS, SettingsAvailability, SettingsError,
        SettingsStore, StartupView, SupportedProvider, parse_settings_document, settings_path,
    };
    use serde_json::{Value, json};
    use std::ffi::OsStr;
    use std::fs::{self, File};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    #[cfg(unix)]
    use std::os::fd::AsRawFd;
    #[cfg(unix)]
    use std::os::unix::fs::{PermissionsExt, symlink};

    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "herdr-quota-settings-test-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path).expect("create test directory");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            #[cfg(unix)]
            fs::set_permissions(&self.0, fs::Permissions::from_mode(0o700)).ok();
            fs::remove_dir_all(&self.0).ok();
        }
    }

    fn parse_fixture(text: &str) -> DashboardSettings {
        let value: Value = serde_json::from_str(text).expect("parse fixture JSON");
        parse_settings_document(&value).expect("parse settings fixture")
    }

    #[test]
    fn defaults_and_path_use_the_v5_contract() {
        assert_eq!(SETTINGS_SCHEMA_VERSION, 5);
        assert_eq!(
            SUPPORTED_PROVIDERS,
            [
                SupportedProvider::Claude,
                SupportedProvider::Codex,
                SupportedProvider::Cursor,
                SupportedProvider::Kimi,
                SupportedProvider::Grok,
                SupportedProvider::Copilot,
            ]
        );
        assert_eq!(
            DashboardSettings::default(),
            DashboardSettings {
                schema_version: 5,
                provider_order: SUPPORTED_PROVIDERS.to_vec(),
                hidden_providers: Vec::new(),
                meter_mode: MeterMode::Remaining,
                provider_identity: ProviderIdentityMode::LogoAndName,
                remaining_threshold: RemainingThreshold::Off,
                forecast_before_reset: false,
                startup_view: StartupView::Overview,
            }
        );
        assert_eq!(
            settings_path(Some(OsStr::new("config-root")), Path::new("unused")),
            Path::new("config-root")
                .join("herdr-quota")
                .join("settings.json")
        );
        assert_eq!(
            settings_path(None, Path::new("home-root")),
            Path::new("home-root")
                .join(".config")
                .join("herdr-quota")
                .join("settings.json")
        );
    }

    #[test]
    fn prior_schemas_migrate_in_memory_without_a_write() {
        let v1 = include_str!("../../tests/fixtures/settings-v1.json");
        let v2 = include_str!("../../tests/fixtures/settings-v2.json");
        let directory = TestDirectory::new();
        let path = directory.path().join("settings.json");

        for (text, threshold, forecast) in [
            (v1, RemainingThreshold::Off, false),
            (v2, RemainingThreshold::Percent25, true),
        ] {
            fs::write(&path, text).expect("write fixture");
            let loaded = SettingsStore::new(&path).load();
            assert_eq!(loaded.availability, SettingsAvailability::Ready);
            assert_eq!(loaded.settings.schema_version, SETTINGS_SCHEMA_VERSION);
            assert_eq!(loaded.settings.remaining_threshold, threshold);
            assert_eq!(loaded.settings.forecast_before_reset, forecast);
            assert_eq!(loaded.settings.startup_view, StartupView::Overview);
            assert_eq!(
                loaded.settings.provider_identity,
                ProviderIdentityMode::LogoAndName
            );
            assert_eq!(fs::read_to_string(&path).expect("read fixture"), text);
        }

        let migrated = parse_fixture(v2);
        assert_eq!(
            migrated.provider_order,
            vec![
                SupportedProvider::Cursor,
                SupportedProvider::Claude,
                SupportedProvider::Codex,
                SupportedProvider::Kimi,
                SupportedProvider::Grok,
                SupportedProvider::Copilot,
            ]
        );

        let v3 = r#"{"schemaVersion":3,"providerOrder":["claude"],"hiddenProviders":[],"meterMode":"remaining","remainingThreshold":"off","forecastBeforeReset":false}"#;
        fs::write(&path, v3).expect("write v3 fixture");
        let loaded = SettingsStore::new(&path).load();
        assert_eq!(loaded.availability, SettingsAvailability::Ready);
        assert_eq!(loaded.settings.startup_view, StartupView::Overview);
        assert_eq!(
            loaded.settings.provider_identity,
            ProviderIdentityMode::LogoAndName
        );
        assert_eq!(fs::read_to_string(&path).expect("read v3 fixture"), v3);
    }

    #[test]
    fn startup_view_load_save_and_default_are_finite() {
        assert_eq!(
            DashboardSettings::default().startup_view,
            StartupView::Overview
        );
        for (text, expected) in [
            ("overview", StartupView::Overview),
            ("details", StartupView::Details),
        ] {
            let parsed = parse_settings_document(&json!({
                "schemaVersion": 4,
                "providerOrder": ["claude"],
                "hiddenProviders": [],
                "meterMode": "remaining",
                "remainingThreshold": "off",
                "forecastBeforeReset": false,
                "startupView": text
            }))
            .expect("parse startup view");
            assert_eq!(parsed.startup_view, expected);
        }
    }

    #[test]
    fn provider_identity_has_exactly_three_persisted_modes() {
        assert_eq!(
            DashboardSettings::default().provider_identity,
            ProviderIdentityMode::LogoAndName
        );
        for (text, expected) in [
            ("logo_only", ProviderIdentityMode::LogoOnly),
            ("logo_and_name", ProviderIdentityMode::LogoAndName),
            ("name_only", ProviderIdentityMode::NameOnly),
        ] {
            let parsed = parse_settings_document(&json!({
                "schemaVersion": 5,
                "providerOrder": ["claude"],
                "hiddenProviders": [],
                "meterMode": "remaining",
                "providerIdentity": text,
                "remainingThreshold": "off",
                "forecastBeforeReset": false,
                "startupView": "overview"
            }))
            .expect("parse provider identity");
            assert_eq!(parsed.provider_identity, expected);
        }
    }

    #[test]
    fn save_writes_only_the_finite_schema_and_a_private_file() {
        let directory = TestDirectory::new();
        let path = directory.path().join("config/herdr-quota/settings.json");
        let settings = DashboardSettings {
            provider_order: vec![
                SupportedProvider::Cursor,
                SupportedProvider::Claude,
                SupportedProvider::Kimi,
                SupportedProvider::Codex,
                SupportedProvider::Grok,
                SupportedProvider::Copilot,
            ],
            hidden_providers: vec![SupportedProvider::Kimi],
            meter_mode: MeterMode::Used,
            startup_view: StartupView::Details,
            ..DashboardSettings::default()
        };

        SettingsStore::new(&path)
            .save(&settings)
            .expect("save settings");
        assert_eq!(
            fs::read_to_string(&path).expect("read settings"),
            "{\"schemaVersion\":5,\"providerOrder\":[\"cursor\",\"claude\",\"kimi\",\"codex\",\"grok\",\"copilot\"],\"hiddenProviders\":[\"kimi\"],\"meterMode\":\"used\",\"providerIdentity\":\"logo_and_name\",\"remainingThreshold\":\"off\",\"forecastBeforeReset\":false,\"startupView\":\"details\"}\n"
        );
        assert_eq!(
            fs::read_dir(path.parent().expect("settings parent"))
                .expect("read settings directory")
                .filter_map(Result::ok)
                .count(),
            1
        );
        #[cfg(unix)]
        {
            assert_eq!(
                fs::metadata(&path)
                    .expect("settings metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(path.parent().expect("settings parent"))
                    .expect("settings directory metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
    }

    #[test]
    fn corrupt_content_is_quarantined_with_its_original_bytes() {
        let directory = TestDirectory::new();
        let path = directory.path().join("settings.json");
        fs::write(&path, b"{truncated").expect("write corrupt settings");

        let loaded = SettingsStore::new(&path).load();
        assert_eq!(loaded.availability, SettingsAvailability::Recovered);
        assert_eq!(loaded.settings, DashboardSettings::default());
        assert!(!path.exists());
        let quarantine = fs::read_dir(directory.path())
            .expect("read directory")
            .map(|entry| entry.expect("read entry").path())
            .find(|entry| {
                entry
                    .file_name()
                    .expect("quarantine name")
                    .to_string_lossy()
                    .starts_with("settings.json.invalid-")
            })
            .expect("quarantine file");
        assert_eq!(
            fs::read(&quarantine).expect("read quarantine"),
            b"{truncated"
        );
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(quarantine)
                .expect("quarantine metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[cfg(unix)]
    #[test]
    fn quarantine_lock_timeout_reports_unavailable() {
        let directory = TestDirectory::new();
        let path = directory.path().join("settings.json");
        fs::write(&path, b"{truncated").expect("write corrupt settings");
        let parent = File::open(directory.path()).expect("open settings directory");
        assert_eq!(
            unsafe { libc::flock(parent.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0
        );

        let loaded = SettingsStore::new(&path).load();

        assert_eq!(loaded.availability, SettingsAvailability::Unavailable);
        assert_eq!(loaded.settings, DashboardSettings::default());
        assert_eq!(
            fs::read(path).expect("read corrupt settings"),
            b"{truncated"
        );
    }

    #[test]
    fn future_versions_stay_in_place_and_cannot_be_replaced() {
        let directory = TestDirectory::new();
        let path = directory.path().join("settings.json");
        let future = b"{\"schemaVersion\":6,\"future\":\"keep\"}\n";
        fs::write(&path, future).expect("write future settings");
        let store = SettingsStore::new(&path);

        assert_eq!(
            store.load().availability,
            SettingsAvailability::Incompatible
        );
        assert!(matches!(
            store.save(&DashboardSettings::default()),
            Err(SettingsError::Incompatible)
        ));
        assert_eq!(fs::read(&path).expect("read future settings"), future);
        assert_eq!(
            fs::read_dir(directory.path())
                .expect("read directory")
                .filter_map(Result::ok)
                .count(),
            1
        );
    }

    #[test]
    fn unknown_fields_and_provider_ids_do_not_enter_the_runtime() {
        let parsed = parse_settings_document(&json!({
            "schemaVersion": 4,
            "providerOrder": ["future-provider", "cursor", "claude"],
            "hiddenProviders": ["future-provider", "cursor"],
            "meterMode": "remaining",
            "remainingThreshold": 10,
            "forecastBeforeReset": false,
            "startupView": "details",
            "accountId": "secret",
            "rawPayload": {"token": "credential"}
        }))
        .expect("parse current settings");

        assert_eq!(
            parsed.provider_order,
            vec![
                SupportedProvider::Cursor,
                SupportedProvider::Claude,
                SupportedProvider::Codex,
                SupportedProvider::Kimi,
                SupportedProvider::Grok,
                SupportedProvider::Copilot,
            ]
        );
        assert_eq!(parsed.hidden_providers, vec![SupportedProvider::Cursor]);
        assert_eq!(parsed.remaining_threshold, RemainingThreshold::Percent10);
        assert_eq!(parsed.startup_view, StartupView::Details);
        assert_eq!(parsed.provider_identity, ProviderIdentityMode::LogoAndName);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_directory_and_read_only_targets_are_refused() {
        let directory = TestDirectory::new();
        let path = directory.path().join("settings.json");
        let destination = directory.path().join("elsewhere.json");
        fs::write(&destination, b"do not touch").expect("write destination");
        symlink(&destination, &path).expect("create symlink");
        let store = SettingsStore::new(&path);

        assert_eq!(store.load().availability, SettingsAvailability::Unavailable);
        assert!(matches!(
            store.save(&DashboardSettings::default()),
            Err(SettingsError::Unsafe)
        ));
        assert_eq!(
            fs::read(&destination).expect("read destination"),
            b"do not touch"
        );

        fs::remove_file(&path).expect("remove symlink");
        fs::create_dir(&path).expect("create target directory");
        assert!(matches!(
            store.save(&DashboardSettings::default()),
            Err(SettingsError::Unsafe)
        ));
        fs::remove_dir(&path).expect("remove target directory");

        fs::write(&path, b"previous\n").expect("write target");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o400))
            .expect("make target read only");
        assert!(matches!(
            store.save(&DashboardSettings::default()),
            Err(SettingsError::Unsafe)
        ));
        assert_eq!(fs::read(&path).expect("read target"), b"previous\n");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("restore target mode");
    }

    #[cfg(unix)]
    #[test]
    fn save_failure_does_not_change_settings_or_create_a_partial_file() {
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let directory = TestDirectory::new();
        let locked = directory.path().join("locked");
        let path = locked.join("settings.json");
        fs::create_dir(&locked).expect("create locked directory");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o500)).expect("lock directory");

        let settings = DashboardSettings {
            meter_mode: MeterMode::Used,
            ..DashboardSettings::default()
        };
        let store = SettingsStore::new(&path);
        let result = store.save(&settings);

        fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).expect("unlock directory");
        assert!(matches!(result, Err(SettingsError::Unavailable(_))));
        assert_eq!(settings.meter_mode, MeterMode::Used);
        assert!(!path.exists());
        assert_eq!(store.load().availability, SettingsAvailability::FirstRun);
    }

    #[test]
    fn invalid_finite_values_are_corrupt() {
        for value in [
            json!({
                "schemaVersion": 5,
                "providerOrder": ["claude"],
                "hiddenProviders": [],
                "meterMode": "remaining",
                "providerIdentity": "icon_and_name",
                "remainingThreshold": "off",
                "forecastBeforeReset": false,
                "startupView": "overview"
            }),
            json!({
                "schemaVersion": 4,
                "providerOrder": ["claude", "claude"],
                "hiddenProviders": [],
                "meterMode": "remaining",
                "remainingThreshold": "off",
                "forecastBeforeReset": false,
                "startupView": "overview"
            }),
            json!({
                "schemaVersion": 4,
                "providerOrder": ["claude"],
                "hiddenProviders": [],
                "meterMode": "remaining",
                "remainingThreshold": 13,
                "forecastBeforeReset": false,
                "startupView": "overview"
            }),
            json!({
                "schemaVersion": 4,
                "providerOrder": ["claude"],
                "hiddenProviders": [],
                "meterMode": "remaining",
                "remainingThreshold": "off",
                "forecastBeforeReset": "yes",
                "startupView": "overview"
            }),
            json!({
                "schemaVersion": 4,
                "providerOrder": ["claude"],
                "hiddenProviders": [],
                "meterMode": "remaining",
                "remainingThreshold": "off",
                "forecastBeforeReset": false,
                "startupView": "grid"
            }),
        ] {
            assert!(matches!(
                parse_settings_document(&value),
                Err(SettingsError::Corrupt)
            ));
        }
    }
}
