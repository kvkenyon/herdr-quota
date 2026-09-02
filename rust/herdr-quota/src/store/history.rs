//! Bounded, private, atomic quota-history storage.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use serde_json::{Map, Value};
use thiserror::Error;

use crate::domain::history_evidence::{
    HISTORY_MAX_FACTS_PER_PROVIDER, HISTORY_SCHEMA_VERSION, HistoryAvailability, HistoryDataHealth,
    HistoryDocument, HistoryFact, HistoryPaceFact, HistoryPaceState, HistoryProviderName,
    HistoryProviderSnapshot, HistoryRunwayFact, HistoryRunwayState, HistorySnapshot, HistoryView,
    history_view, normalize_history_snapshot, safe_identity, timestamp_millis,
};
use crate::domain::provider::{MarketedProvider, ProjectionConfidence};
use crate::domain::schema::QuotaReport;

/// Maximum snapshots retained in the one local document.
pub const HISTORY_MAX_SNAPSHOTS: usize = 512;
/// Maximum age of a retained snapshot.
pub const HISTORY_MAX_AGE_MILLIS: i128 = 30 * 24 * 60 * 60 * 1_000;
/// Minimum cadence for storing otherwise equivalent snapshots.
pub const HISTORY_EQUIVALENT_INTERVAL_MILLIS: i128 = 15 * 60 * 1_000;
/// Rollback beyond this tolerance starts a disconnected history segment.
pub const HISTORY_CLOCK_SKEW_MILLIS: i128 = 5 * 60 * 1_000;

const MAX_HISTORY_FILE_BYTES: u64 = 16 * 1024 * 1024;
static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// A finite parse failure that never carries file content into the UI.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum HistoryDocumentError {
    #[error("history_corrupt")]
    Corrupt,
    #[error("history_incompatible")]
    Incompatible,
}

/// Result of applying one normalized sample to retained history.
#[derive(Clone, Debug, PartialEq)]
pub struct HistoryUpdate {
    pub document: HistoryDocument,
    pub wrote: bool,
    pub clock_skew: bool,
}

fn object(value: &Value) -> Result<&Map<String, Value>, HistoryDocumentError> {
    value.as_object().ok_or(HistoryDocumentError::Corrupt)
}

fn exact_keys(raw: &Map<String, Value>, required: &[&str], optional: &[&str]) -> bool {
    raw.len() >= required.len()
        && raw.len() <= required.len() + optional.len()
        && required.iter().all(|key| raw.contains_key(*key))
        && raw
            .keys()
            .all(|key| required.contains(&key.as_str()) || optional.contains(&key.as_str()))
}

fn number(value: Option<&Value>, minimum: f64, maximum: f64) -> Option<f64> {
    value
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && *value >= minimum && *value <= maximum)
}

fn string(value: Option<&Value>) -> Result<&str, HistoryDocumentError> {
    value
        .and_then(Value::as_str)
        .ok_or(HistoryDocumentError::Corrupt)
}

fn parse_pace(value: &Value) -> Result<HistoryPaceFact, HistoryDocumentError> {
    let raw = object(value)?;
    if !exact_keys(raw, &["state"], &["reserve"]) {
        return Err(HistoryDocumentError::Corrupt);
    }
    let state = match string(raw.get("state"))? {
        "ahead" => HistoryPaceState::Ahead,
        "on_pace" => HistoryPaceState::OnPace,
        "behind" => HistoryPaceState::Behind,
        "mixed" => HistoryPaceState::Mixed,
        _ => return Err(HistoryDocumentError::Corrupt),
    };
    let reserve = match raw.get("reserve") {
        Some(value) => {
            Some(number(Some(value), -10_000.0, 10_000.0).ok_or(HistoryDocumentError::Corrupt)?)
        }
        None => None,
    };
    Ok(HistoryPaceFact { state, reserve })
}

fn parse_runway(value: &Value) -> Result<HistoryRunwayFact, HistoryDocumentError> {
    let raw = object(value)?;
    if !exact_keys(raw, &["state"], &["projectedAt", "confidence"]) {
        return Err(HistoryDocumentError::Corrupt);
    }
    let state = match string(raw.get("state"))? {
        "exhausted_now" => HistoryRunwayState::ExhaustedNow,
        "projected_exhaustion" => HistoryRunwayState::ProjectedExhaustion,
        "through_reset" => HistoryRunwayState::ThroughReset,
        _ => return Err(HistoryDocumentError::Corrupt),
    };
    let projected_at = raw
        .get("projectedAt")
        .map(|value| {
            let value = string(Some(value))?;
            timestamp_millis(value)
                .is_some()
                .then(|| value.to_owned())
                .ok_or(HistoryDocumentError::Corrupt)
        })
        .transpose()?;
    let confidence = raw
        .get("confidence")
        .map(|value| match string(Some(value))? {
            "early" => Ok(ProjectionConfidence::Early),
            "established" => Ok(ProjectionConfidence::Established),
            _ => Err(HistoryDocumentError::Corrupt),
        })
        .transpose()?;
    Ok(HistoryRunwayFact {
        state,
        projected_at,
        confidence,
    })
}

fn parse_fact(value: &Value) -> Result<HistoryFact, HistoryDocumentError> {
    let raw = object(value)?;
    if !exact_keys(
        raw,
        &["scope", "remaining"],
        &["limit", "resetAt", "pace", "runway"],
    ) {
        return Err(HistoryDocumentError::Corrupt);
    }
    let scope = string(raw.get("scope"))?;
    if !safe_identity(scope) {
        return Err(HistoryDocumentError::Corrupt);
    }
    let limit = raw
        .get("limit")
        .map(|value| {
            let value = string(Some(value))?;
            safe_identity(value)
                .then(|| value.to_owned())
                .ok_or(HistoryDocumentError::Corrupt)
        })
        .transpose()?;
    let remaining =
        number(raw.get("remaining"), 0.0, 100.0).ok_or(HistoryDocumentError::Corrupt)?;
    let reset_at = raw
        .get("resetAt")
        .map(|value| {
            let value = string(Some(value))?;
            timestamp_millis(value)
                .is_some()
                .then(|| value.to_owned())
                .ok_or(HistoryDocumentError::Corrupt)
        })
        .transpose()?;
    let pace = raw.get("pace").map(parse_pace).transpose()?;
    let runway = raw.get("runway").map(parse_runway).transpose()?;
    Ok(HistoryFact {
        scope: scope.to_owned(),
        limit,
        remaining,
        reset_at,
        pace,
        runway,
    })
}

fn parse_health(value: &str) -> Result<HistoryDataHealth, HistoryDocumentError> {
    match value {
        "current" => Ok(HistoryDataHealth::Current),
        "stale" => Ok(HistoryDataHealth::Stale),
        "unavailable" => Ok(HistoryDataHealth::Unavailable),
        "error" => Ok(HistoryDataHealth::Error),
        "unknown" => Ok(HistoryDataHealth::Unknown),
        _ => Err(HistoryDocumentError::Corrupt),
    }
}

fn parse_provider(value: &Value) -> Result<HistoryProviderSnapshot, HistoryDocumentError> {
    let raw = object(value)?;
    if !exact_keys(
        raw,
        &["provider", "dataHealth", "authEligible", "facts"],
        &[],
    ) {
        return Err(HistoryDocumentError::Corrupt);
    }
    let provider = HistoryProviderName::from_label(string(raw.get("provider"))?)
        .ok_or(HistoryDocumentError::Corrupt)?;
    let data_health = parse_health(string(raw.get("dataHealth"))?)?;
    let auth_eligible = raw
        .get("authEligible")
        .and_then(Value::as_bool)
        .ok_or(HistoryDocumentError::Corrupt)?;
    let values = raw
        .get("facts")
        .and_then(Value::as_array)
        .ok_or(HistoryDocumentError::Corrupt)?;
    if values.len() > HISTORY_MAX_FACTS_PER_PROVIDER {
        return Err(HistoryDocumentError::Corrupt);
    }
    let facts = values
        .iter()
        .map(parse_fact)
        .collect::<Result<Vec<_>, _>>()?;
    if (data_health != HistoryDataHealth::Current || !auth_eligible) && !facts.is_empty() {
        return Err(HistoryDocumentError::Corrupt);
    }
    Ok(HistoryProviderSnapshot {
        provider,
        data_health,
        auth_eligible,
        facts,
    })
}

fn parse_snapshot(value: &Value) -> Result<HistorySnapshot, HistoryDocumentError> {
    let raw = object(value)?;
    if !exact_keys(raw, &["capturedAt", "providers"], &[]) {
        return Err(HistoryDocumentError::Corrupt);
    }
    let captured_at = string(raw.get("capturedAt"))?;
    if timestamp_millis(captured_at).is_none() {
        return Err(HistoryDocumentError::Corrupt);
    }
    let values = raw
        .get("providers")
        .and_then(Value::as_array)
        .ok_or(HistoryDocumentError::Corrupt)?;
    if values.len() > MarketedProvider::ALL.len() {
        return Err(HistoryDocumentError::Corrupt);
    }
    let providers = values
        .iter()
        .map(parse_provider)
        .collect::<Result<Vec<_>, _>>()?;
    if providers.iter().enumerate().any(|(index, provider)| {
        providers[..index]
            .iter()
            .any(|previous| previous.provider == provider.provider)
    }) {
        return Err(HistoryDocumentError::Corrupt);
    }
    Ok(HistorySnapshot {
        captured_at: captured_at.to_owned(),
        providers,
    })
}

fn parse_history_value(value: &Value) -> Result<HistoryDocument, HistoryDocumentError> {
    let raw = object(value)?;
    let version = raw
        .get("schemaVersion")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            if raw.get("schemaVersion").is_some_and(Value::is_number) {
                HistoryDocumentError::Incompatible
            } else {
                HistoryDocumentError::Corrupt
            }
        })?;
    if version != 1 && version != u64::from(HISTORY_SCHEMA_VERSION) {
        return Err(HistoryDocumentError::Incompatible);
    }
    if !exact_keys(raw, &["schemaVersion", "snapshots"], &[]) {
        return Err(HistoryDocumentError::Corrupt);
    }
    let values = raw
        .get("snapshots")
        .and_then(Value::as_array)
        .ok_or(HistoryDocumentError::Corrupt)?;
    if values.len() > HISTORY_MAX_SNAPSHOTS {
        return Err(HistoryDocumentError::Corrupt);
    }
    let snapshots = values
        .iter()
        .map(parse_snapshot)
        .collect::<Result<Vec<_>, _>>()?;
    if snapshots.windows(2).any(|pair| {
        timestamp_millis(&pair[0].captured_at) >= timestamp_millis(&pair[1].captured_at)
    }) {
        return Err(HistoryDocumentError::Corrupt);
    }
    Ok(HistoryDocument {
        schema_version: HISTORY_SCHEMA_VERSION,
        snapshots,
    })
}

/// Parse only schema v1/v2 fields, migrating v1 in memory without writing.
pub fn parse_history_document(input: &str) -> Result<HistoryDocument, HistoryDocumentError> {
    let value = serde_json::from_str(input).map_err(|_| HistoryDocumentError::Corrupt)?;
    parse_history_value(&value)
}

fn serialized_document(document: &HistoryDocument) -> io::Result<Vec<u8>> {
    let value = serde_json::to_value(document)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "history_corrupt"))?;
    let document = parse_history_value(&value)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
    let mut bytes = serde_json::to_vec(&document)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "history_corrupt"))?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn snapshot_fingerprint(snapshot: &HistorySnapshot) -> Vec<u8> {
    serde_json::to_vec(&snapshot.providers).expect("validated history providers serialize")
}

/// Apply count, age, cadence, change, and clock-segment bounds.
pub fn update_history_document(
    document: &HistoryDocument,
    snapshot: HistorySnapshot,
) -> HistoryUpdate {
    let current_at = timestamp_millis(&snapshot.captured_at)
        .expect("a normalized history snapshot has a canonical timestamp");
    let previous = document.snapshots.last();
    let previous_at = previous.and_then(|snapshot| timestamp_millis(&snapshot.captured_at));
    if previous_at.is_some_and(|previous_at| current_at < previous_at - HISTORY_CLOCK_SKEW_MILLIS) {
        return HistoryUpdate {
            document: HistoryDocument {
                schema_version: HISTORY_SCHEMA_VERSION,
                snapshots: vec![snapshot],
            },
            wrote: true,
            clock_skew: true,
        };
    }
    if previous_at.is_some_and(|previous_at| current_at <= previous_at) {
        return HistoryUpdate {
            document: document.clone(),
            wrote: false,
            clock_skew: false,
        };
    }

    let cutoff = current_at - HISTORY_MAX_AGE_MILLIS;
    let mut snapshots = document
        .snapshots
        .iter()
        .filter(|item| timestamp_millis(&item.captured_at).is_some_and(|at| at >= cutoff))
        .cloned()
        .collect::<Vec<_>>();
    let equivalent = previous
        .is_some_and(|previous| snapshot_fingerprint(previous) == snapshot_fingerprint(&snapshot));
    let should_append = !equivalent
        || previous_at.is_none()
        || previous_at.is_some_and(|previous_at| {
            current_at - previous_at >= HISTORY_EQUIVALENT_INTERVAL_MILLIS
        });
    if should_append {
        snapshots.push(snapshot);
    }
    if snapshots.len() > HISTORY_MAX_SNAPSHOTS {
        snapshots.drain(..snapshots.len() - HISTORY_MAX_SNAPSHOTS);
    }
    let retained_changed = snapshots.len() != document.snapshots.len();
    HistoryUpdate {
        document: HistoryDocument {
            schema_version: HISTORY_SCHEMA_VERSION,
            snapshots,
        },
        wrote: should_append || retained_changed,
        clock_skew: false,
    }
}

fn nonempty_environment(name: &str) -> Option<OsString> {
    std::env::var_os(name).filter(|value| !value.is_empty())
}

/// Return the fixed quota-history path, honoring its explicit/XDG overrides.
pub fn history_path_from_environment() -> PathBuf {
    if let Some(path) = nonempty_environment("HERDR_QUOTA_HISTORY_FILE") {
        return PathBuf::from(path);
    }
    let state_root = nonempty_environment("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            nonempty_environment("HOME").map(|home| PathBuf::from(home).join(".local/state"))
        })
        .unwrap_or_else(|| PathBuf::from(".local/state"));
    state_root.join("herdr-quota/history-v1.json")
}

#[cfg(unix)]
fn current_uid() -> u32 {
    // SAFETY: geteuid has no preconditions and does not dereference memory.
    unsafe { libc::geteuid() }
}

#[cfg(unix)]
fn validate_attributes(
    is_regular: bool,
    is_symlink: bool,
    uid: u32,
    mode: u32,
    require_writable: bool,
) -> io::Result<()> {
    if is_symlink || !is_regular || uid != current_uid() || (require_writable && mode & 0o200 == 0)
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "history_unsafe",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn read_regular_target(path: &Path, require_writable: bool) -> io::Result<Option<String>> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    validate_attributes(
        metadata.is_file(),
        metadata.file_type().is_symlink(),
        metadata.uid(),
        metadata.mode(),
        require_writable,
    )?;
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let opened = file.metadata()?;
    validate_attributes(
        opened.is_file(),
        false,
        opened.uid(),
        opened.mode(),
        require_writable,
    )?;
    if opened.len() > MAX_HISTORY_FILE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "history_too_large",
        ));
    }
    let mut bytes = Vec::with_capacity(opened.len() as usize);
    Read::by_ref(&mut file)
        .take(MAX_HISTORY_FILE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_HISTORY_FILE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "history_too_large",
        ));
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "history_corrupt"))
}

#[cfg(not(unix))]
fn read_regular_target(path: &Path, require_writable: bool) -> io::Result<Option<String>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || (require_writable && metadata.permissions().readonly())
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "history_unsafe",
        ));
    }
    let bytes = fs::read(path)?;
    if bytes.len() as u64 > MAX_HISTORY_FILE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "history_too_large",
        ));
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "history_corrupt"))
}

trait HistoryFileOperations {
    type Writer: Write;

    fn read_target(&self, path: &Path, require_writable: bool) -> io::Result<Option<String>>;
    fn create_parent(&self, parent: &Path) -> io::Result<()>;
    fn create_temporary(&self, path: &Path) -> io::Result<Self::Writer>;
    fn sync_file(&self, writer: &mut Self::Writer) -> io::Result<()>;
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()>;
    fn sync_parent(&self, parent: &Path) -> io::Result<()>;
    fn remove_temporary(&self, path: &Path) -> io::Result<()>;
}

#[derive(Clone, Copy, Debug, Default)]
struct RealFileOperations;

impl HistoryFileOperations for RealFileOperations {
    type Writer = File;

    fn read_target(&self, path: &Path, require_writable: bool) -> io::Result<Option<String>> {
        read_regular_target(path, require_writable)
    }

    fn create_parent(&self, parent: &Path) -> io::Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::{DirBuilderExt, MetadataExt};

            let mut builder = fs::DirBuilder::new();
            builder.recursive(true).mode(0o700).create(parent)?;
            let metadata = fs::symlink_metadata(parent)?;
            if metadata.file_type().is_symlink()
                || !metadata.is_dir()
                || metadata.uid() != current_uid()
                || metadata.mode() & 0o022 != 0
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "history_unsafe",
                ));
            }
        }
        #[cfg(not(unix))]
        fs::create_dir_all(parent)?;
        Ok(())
    }

    fn create_temporary(&self, path: &Path) -> io::Result<Self::Writer> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        options.open(path)
    }

    fn sync_file(&self, writer: &mut Self::Writer) -> io::Result<()> {
        writer.sync_all()
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        fs::rename(from, to)
    }

    fn sync_parent(&self, parent: &Path) -> io::Result<()> {
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW);
        }
        let directory = options.open(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let metadata = directory.metadata()?;
            if !metadata.is_dir() || metadata.uid() != current_uid() || metadata.mode() & 0o022 != 0
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "history_unsafe",
                ));
            }
        }
        directory.sync_all()
    }

    fn remove_temporary(&self, path: &Path) -> io::Result<()> {
        match fs::remove_file(path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            result => result,
        }
    }
}

fn temporary_path(path: &Path) -> PathBuf {
    let sequence = TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    path.with_file_name(format!("{name}.{}.{}.tmp", std::process::id(), sequence))
}

fn target_may_be_replaced<F: HistoryFileOperations>(operations: &F, path: &Path) -> io::Result<()> {
    if let Some(text) = operations.read_target(path, true)? {
        if parse_history_document(&text) == Err(HistoryDocumentError::Incompatible) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "history_incompatible",
            ));
        }
    }
    Ok(())
}

fn write_history_document_atomic_with<F: HistoryFileOperations>(
    operations: &F,
    path: &Path,
    document: &HistoryDocument,
) -> io::Result<()> {
    let bytes = serialized_document(document)?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    operations.create_parent(parent)?;
    target_may_be_replaced(operations, path)?;

    let (temporary, mut writer) = loop {
        let temporary = temporary_path(path);
        match operations.create_temporary(&temporary) {
            Ok(writer) => break (temporary, writer),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    };
    let result = (|| {
        writer.write_all(&bytes)?;
        operations.sync_file(&mut writer)?;
        drop(writer);
        target_may_be_replaced(operations, path)?;
        operations.rename(&temporary, path)?;
        operations.sync_parent(parent)
    })();
    if result.is_err() {
        let _ = operations.remove_temporary(&temporary);
    }
    result
}

/// Persist a validated document through a private, durable sibling replacement.
pub fn write_history_document_atomic(path: &Path, document: &HistoryDocument) -> io::Result<()> {
    write_history_document_atomic_with(&RealFileOperations, path, document)
}

enum LoadResult {
    Ready(HistoryDocument),
    Missing(HistoryDocument),
    Corrupt(HistoryDocument),
    Incompatible,
    Unavailable,
}

fn load_history<F: HistoryFileOperations>(operations: &F, path: &Path) -> LoadResult {
    let text = match operations.read_target(path, false) {
        Ok(Some(text)) => text,
        Ok(None) => return LoadResult::Missing(HistoryDocument::default()),
        Err(_) => return LoadResult::Unavailable,
    };
    match parse_history_document(&text) {
        Ok(document) => LoadResult::Ready(document),
        Err(HistoryDocumentError::Incompatible) => LoadResult::Incompatible,
        Err(HistoryDocumentError::Corrupt) => LoadResult::Corrupt(HistoryDocument::default()),
    }
}

struct HistoryStore<F> {
    path: PathBuf,
    operations: F,
    document: Option<HistoryDocument>,
}

impl<F: HistoryFileOperations> HistoryStore<F> {
    fn retained(&mut self) -> Option<&HistoryDocument> {
        self.document = match load_history(&self.operations, &self.path) {
            LoadResult::Ready(document)
            | LoadResult::Missing(document)
            | LoadResult::Corrupt(document) => Some(document),
            LoadResult::Incompatible | LoadResult::Unavailable => None,
        };
        self.document.as_ref()
    }

    fn record_at(&mut self, report: &QuotaReport, now: SystemTime) -> HistoryView {
        let Some(snapshot) = normalize_history_snapshot(report, now) else {
            return HistoryView {
                availability: HistoryAvailability::NoUsableData,
                evidence: None,
            };
        };
        let (document, loaded_availability) = match load_history(&self.operations, &self.path) {
            LoadResult::Ready(document) => (document, HistoryAvailability::Ready),
            LoadResult::Missing(document) => (document, HistoryAvailability::Ready),
            LoadResult::Corrupt(document) => (document, HistoryAvailability::Recovered),
            LoadResult::Incompatible => {
                self.document = None;
                return HistoryView {
                    availability: HistoryAvailability::Incompatible,
                    evidence: None,
                };
            }
            LoadResult::Unavailable => {
                self.document = None;
                return HistoryView {
                    availability: HistoryAvailability::Unavailable,
                    evidence: None,
                };
            }
        };
        let update = update_history_document(&document, snapshot);
        if update.wrote
            && write_history_document_atomic_with(&self.operations, &self.path, &update.document)
                .is_err()
        {
            self.document = None;
            return HistoryView {
                availability: HistoryAvailability::Unavailable,
                evidence: None,
            };
        }
        self.document = Some(update.document);
        history_view(
            self.document.as_ref().expect("history was assigned"),
            if update.clock_skew {
                HistoryAvailability::ClockSkew
            } else {
                loaded_availability
            },
        )
    }
}

/// Stateful local quota history used by the dashboard refresh path.
pub struct LocalHistory {
    inner: HistoryStore<RealFileOperations>,
}

impl Default for LocalHistory {
    fn default() -> Self {
        Self::new(history_path_from_environment())
    }
}

impl LocalHistory {
    /// Use one explicit history file.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            inner: HistoryStore {
                path: path.into(),
                operations: RealFileOperations,
                document: None,
            },
        }
    }

    /// Load retained history without rewriting migrations or corrupt bytes.
    pub fn retained(&mut self) -> Option<&HistoryDocument> {
        self.inner.retained()
    }

    /// Return the last document loaded or recorded by this instance.
    pub fn current(&self) -> Option<&HistoryDocument> {
        self.inner.document.as_ref()
    }

    /// Record a usable report at the current wall-clock time.
    pub fn record(&mut self, report: &QuotaReport) -> HistoryView {
        self.record_at(report, SystemTime::now())
    }

    /// Record a usable report at an explicit time for deterministic callers/tests.
    pub fn record_at(&mut self, report: &QuotaReport, now: SystemTime) -> HistoryView {
        self.inner.record_at(report, now)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::{Duration, UNIX_EPOCH};

    use serde_json::json;

    use super::*;
    use crate::domain::history_evidence::{
        HistoryPaceState, HistoryProviderName, HistoryRunwayState,
    };
    use crate::domain::provider::{
        EffectiveAvailability, EffectiveStatus, ProviderQuota, ProviderState, ProviderStatus,
        SemanticsStatus,
    };

    fn time(value: &str) -> SystemTime {
        UNIX_EPOCH + Duration::from_millis(timestamp_millis(value).unwrap() as u64)
    }

    fn fact(remaining: f64) -> HistoryFact {
        HistoryFact {
            scope: "All models".to_owned(),
            limit: Some("Week".to_owned()),
            remaining,
            reset_at: Some("2026-09-08T12:00:00.000Z".to_owned()),
            pace: Some(HistoryPaceFact {
                state: HistoryPaceState::OnPace,
                reserve: Some(4.0),
            }),
            runway: Some(HistoryRunwayFact {
                state: HistoryRunwayState::ThroughReset,
                projected_at: None,
                confidence: None,
            }),
        }
    }

    fn snapshot(at: &str, remaining: f64) -> HistorySnapshot {
        HistorySnapshot {
            captured_at: at.to_owned(),
            providers: vec![HistoryProviderSnapshot {
                provider: HistoryProviderName::new(MarketedProvider::Claude),
                data_health: HistoryDataHealth::Current,
                auth_eligible: true,
                facts: vec![fact(remaining)],
            }],
        }
    }

    fn document(snapshots: Vec<HistorySnapshot>) -> HistoryDocument {
        HistoryDocument {
            schema_version: HISTORY_SCHEMA_VERSION,
            snapshots,
        }
    }

    fn report(remaining: f64) -> QuotaReport {
        QuotaReport {
            generated_at: "2026-09-02T12:00:00Z".to_owned(),
            schema_version: 5,
            providers: vec![ProviderQuota {
                provider: "claude".to_owned(),
                label: None,
                source: None,
                plan: None,
                windows: Vec::new(),
                effective: vec![EffectiveAvailability {
                    scope: "all_models".to_owned(),
                    status: EffectiveStatus::Known,
                    effective_percent_remaining: Some(remaining),
                    bounded_by: Vec::new(),
                    limiting_window_ids: Vec::new(),
                    pace: None,
                    runway: None,
                }],
                semantics_status: Some(SemanticsStatus::Known),
                credits: None,
                state: ProviderState {
                    status: ProviderStatus::Fresh,
                    stale: false,
                    refreshed_at: None,
                    auth_status: Some("usable".to_owned()),
                    reason: None,
                    remedy_command: None,
                    error_code: None,
                },
            }],
            adaptation_warnings: Vec::new(),
        }
    }

    #[test]
    fn parser_accepts_only_the_closed_six_provider_schema() {
        for provider in MarketedProvider::ALL {
            let value = document(vec![HistorySnapshot {
                captured_at: "2026-09-02T12:00:00.000Z".to_owned(),
                providers: vec![HistoryProviderSnapshot {
                    provider: HistoryProviderName::new(provider),
                    data_health: HistoryDataHealth::Current,
                    auth_eligible: true,
                    facts: vec![fact(50.0)],
                }],
            }]);
            let text = serde_json::to_string(&value).unwrap();
            assert!(
                parse_history_document(&text).is_ok(),
                "{}",
                provider.label()
            );
        }
        let extra = json!({
            "schemaVersion": 2,
            "snapshots": [{
                "capturedAt": "2026-09-02T12:00:00.000Z",
                "providers": [{
                    "provider": "Future Provider",
                    "dataHealth": "current",
                    "authEligible": true,
                    "facts": []
                }],
                "token": "secret"
            }]
        });
        assert_eq!(
            parse_history_document(&extra.to_string()),
            Err(HistoryDocumentError::Corrupt)
        );
    }

    #[test]
    fn parser_migrates_v1_in_memory_and_rejects_future_schema() {
        let mut value = serde_json::to_value(document(Vec::new())).unwrap();
        value["schemaVersion"] = json!(1);
        let migrated = parse_history_document(&value.to_string()).unwrap();
        assert_eq!(migrated.schema_version, 2);
        value["schemaVersion"] = json!(3);
        assert_eq!(
            parse_history_document(&value.to_string()),
            Err(HistoryDocumentError::Incompatible)
        );
    }

    #[test]
    fn cadence_age_and_count_bounds_are_independent() {
        let start = timestamp_millis("2026-01-01T00:00:00.000Z").unwrap();
        let timestamp = |millis: i128| {
            let at = UNIX_EPOCH + Duration::from_millis(millis as u64);
            crate::domain::history_evidence::canonical_history_timestamp(at).unwrap()
        };
        let first = update_history_document(
            &HistoryDocument::default(),
            snapshot(&timestamp(start), 50.0),
        );
        let duplicate = update_history_document(
            &first.document,
            snapshot(
                &timestamp(start + HISTORY_EQUIVALENT_INTERVAL_MILLIS - 1),
                50.0,
            ),
        );
        assert!(!duplicate.wrote);
        let cadence = update_history_document(
            &duplicate.document,
            snapshot(&timestamp(start + HISTORY_EQUIVALENT_INTERVAL_MILLIS), 50.0),
        );
        assert_eq!(cadence.document.snapshots.len(), 2);

        let mut many = HistoryDocument::default();
        for index in 0..HISTORY_MAX_SNAPSHOTS + 20 {
            many = update_history_document(
                &many,
                snapshot(
                    &timestamp(start + index as i128 * 60_000),
                    (index % 2) as f64,
                ),
            )
            .document;
        }
        assert_eq!(many.snapshots.len(), HISTORY_MAX_SNAPSHOTS);

        let now = start + 40 * 24 * 60 * 60 * 1_000;
        let aged = document(vec![
            snapshot(&timestamp(now - HISTORY_MAX_AGE_MILLIS - 1), 10.0),
            snapshot(&timestamp(now - HISTORY_MAX_AGE_MILLIS + 1), 20.0),
        ]);
        let retained = update_history_document(&aged, snapshot(&timestamp(now), 30.0));
        assert_eq!(retained.document.snapshots.len(), 2);
        assert_eq!(retained.document.snapshots[0].facts_remaining(), 20.0);
    }

    trait SnapshotTestExt {
        fn facts_remaining(&self) -> f64;
    }

    impl SnapshotTestExt for HistorySnapshot {
        fn facts_remaining(&self) -> f64 {
            self.providers[0].facts[0].remaining
        }
    }

    #[derive(Default)]
    struct FakeState {
        events: Vec<&'static str>,
        target: Option<String>,
        pending: Vec<u8>,
        fail_rename: bool,
    }

    #[derive(Clone, Default)]
    struct FakeOperations(Rc<RefCell<FakeState>>);

    struct FakeWriter(Rc<RefCell<FakeState>>);

    impl Write for FakeWriter {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.0.borrow_mut().pending.extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl HistoryFileOperations for FakeOperations {
        type Writer = FakeWriter;

        fn read_target(&self, _path: &Path, require_writable: bool) -> io::Result<Option<String>> {
            self.0
                .borrow_mut()
                .events
                .push(if require_writable { "validate" } else { "read" });
            Ok(self.0.borrow().target.clone())
        }

        fn create_parent(&self, _parent: &Path) -> io::Result<()> {
            self.0.borrow_mut().events.push("mkdir-0700");
            Ok(())
        }

        fn create_temporary(&self, _path: &Path) -> io::Result<Self::Writer> {
            let mut state = self.0.borrow_mut();
            state.events.push("open-exclusive-0600-nofollow");
            state.pending.clear();
            Ok(FakeWriter(self.0.clone()))
        }

        fn sync_file(&self, _writer: &mut Self::Writer) -> io::Result<()> {
            self.0.borrow_mut().events.push("fsync-file");
            Ok(())
        }

        fn rename(&self, _from: &Path, _to: &Path) -> io::Result<()> {
            let mut state = self.0.borrow_mut();
            state.events.push("rename");
            if state.fail_rename {
                return Err(io::Error::other("interrupted rename"));
            }
            state.target = Some(String::from_utf8(state.pending.clone()).unwrap());
            Ok(())
        }

        fn sync_parent(&self, _parent: &Path) -> io::Result<()> {
            self.0.borrow_mut().events.push("fsync-directory");
            Ok(())
        }

        fn remove_temporary(&self, _path: &Path) -> io::Result<()> {
            self.0.borrow_mut().events.push("unlink-temp");
            Ok(())
        }
    }

    #[test]
    fn atomic_write_fsyncs_private_exclusive_file_and_directory() {
        let operations = FakeOperations::default();
        write_history_document_atomic_with(
            &operations,
            Path::new("state/history-v1.json"),
            &document(vec![snapshot("2026-09-02T12:00:00.000Z", 50.0)]),
        )
        .unwrap();
        assert_eq!(
            operations.0.borrow().events,
            [
                "mkdir-0700",
                "validate",
                "open-exclusive-0600-nofollow",
                "fsync-file",
                "validate",
                "rename",
                "fsync-directory"
            ]
        );
    }

    #[test]
    fn interrupted_replacement_preserves_prior_bytes() {
        let original =
            serde_json::to_string(&document(vec![snapshot("2026-09-02T11:00:00.000Z", 60.0)]))
                .unwrap();
        let operations = FakeOperations::default();
        {
            let mut state = operations.0.borrow_mut();
            state.target = Some(original.clone());
            state.fail_rename = true;
        }
        assert!(
            write_history_document_atomic_with(
                &operations,
                Path::new("state/history-v1.json"),
                &document(vec![snapshot("2026-09-02T12:00:00.000Z", 50.0)]),
            )
            .is_err()
        );
        let state = operations.0.borrow();
        assert_eq!(state.target.as_deref(), Some(original.as_str()));
        assert_eq!(state.events.last(), Some(&"unlink-temp"));
    }

    #[test]
    fn interrupted_record_is_unavailable_and_does_not_publish_memory_state() {
        let original =
            serde_json::to_string(&document(vec![snapshot("2026-09-02T11:00:00.000Z", 60.0)]))
                .unwrap();
        let operations = FakeOperations::default();
        {
            let mut state = operations.0.borrow_mut();
            state.target = Some(original.clone());
            state.fail_rename = true;
        }
        let mut store = HistoryStore {
            path: PathBuf::from("state/history-v1.json"),
            operations: operations.clone(),
            document: None,
        };
        let view = store.record_at(&report(40.0), time("2026-09-02T12:00:00.000Z"));

        assert_eq!(view.availability, HistoryAvailability::Unavailable);
        assert!(store.document.is_none());
        assert_eq!(
            operations.0.borrow().target.as_deref(),
            Some(original.as_str())
        );
    }

    #[test]
    fn corrupt_future_and_clock_behavior_stays_finite() {
        let corrupt = FakeOperations::default();
        corrupt.0.borrow_mut().target = Some("{truncated".to_owned());
        let mut store = HistoryStore {
            path: PathBuf::from("state/history-v1.json"),
            operations: corrupt.clone(),
            document: None,
        };
        let view = store.record_at(&report(50.0), time("2026-09-02T12:00:00.000Z"));
        assert_eq!(view.availability, HistoryAvailability::Recovered);

        let future = FakeOperations::default();
        future.0.borrow_mut().target = Some(r#"{"schemaVersion":3,"snapshots":[]}"#.to_owned());
        let original = future.0.borrow().target.clone();
        let mut store = HistoryStore {
            path: PathBuf::from("state/history-v1.json"),
            operations: future.clone(),
            document: None,
        };
        let view = store.record_at(&report(50.0), time("2026-09-02T12:00:00.000Z"));
        assert_eq!(view.availability, HistoryAvailability::Incompatible);
        assert_eq!(future.0.borrow().target, original);

        let clock = FakeOperations::default();
        clock.0.borrow_mut().target = Some(
            serde_json::to_string(&document(vec![snapshot("2026-09-02T13:00:00.000Z", 60.0)]))
                .unwrap(),
        );
        let mut store = HistoryStore {
            path: PathBuf::from("state/history-v1.json"),
            operations: clock,
            document: None,
        };
        let view = store.record_at(&report(50.0), time("2026-09-02T12:00:00.000Z"));
        assert_eq!(view.availability, HistoryAvailability::ClockSkew);
        assert_eq!(store.document.unwrap().snapshots.len(), 1);
    }

    #[cfg(unix)]
    struct WorkspaceTemp {
        path: PathBuf,
    }

    #[cfg(unix)]
    impl WorkspaceTemp {
        fn new() -> Self {
            let root = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/history-store-tests")
                .join(format!(
                    "{}-{}",
                    std::process::id(),
                    TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed)
                ));
            fs::create_dir_all(&root).unwrap();
            Self { path: root }
        }
    }

    #[cfg(unix)]
    impl Drop for WorkspaceTemp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[cfg(unix)]
    #[test]
    fn real_storage_is_private_and_refuses_symlink_targets() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

        let temporary = WorkspaceTemp::new();
        let path = temporary.path.join("private/history-v1.json");
        let value = document(vec![snapshot("2026-09-02T12:00:00.000Z", 50.0)]);
        write_history_document_atomic(&path, &value).unwrap();
        assert_eq!(
            fs::metadata(path.parent().unwrap()).unwrap().mode() & 0o777,
            0o700
        );
        assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);

        let destination = temporary.path.join("destination.json");
        fs::write(&destination, "do not replace").unwrap();
        let link = temporary.path.join("link.json");
        symlink(&destination, &link).unwrap();
        assert!(write_history_document_atomic(&link, &value).is_err());
        assert_eq!(fs::read_to_string(&destination).unwrap(), "do not replace");

        let readonly = temporary.path.join("readonly.json");
        fs::write(&readonly, serde_json::to_vec(&value).unwrap()).unwrap();
        fs::set_permissions(&readonly, fs::Permissions::from_mode(0o400)).unwrap();
        assert!(write_history_document_atomic(&readonly, &value).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn ownership_check_rejects_foreign_files() {
        assert!(validate_attributes(true, false, current_uid(), 0o600, true).is_ok());
        assert!(
            validate_attributes(true, false, current_uid().wrapping_add(1), 0o600, true).is_err()
        );
    }
}
