//! Private, bounded schema-v1 coordination for the sidebar action and dashboard.

use std::ffi::OsStr;
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::atomic::{self, AtomicError, ReplaceError};
use crate::sidebar::layout::RebuildPlan;

pub(crate) const SIDEBAR_SCHEMA_VERSION: u64 = 1;
const MAX_DOCUMENT_BYTES: usize = 64 * 1024;
const MAX_IDENTIFIER_BYTES: usize = 256;
const MAX_TOKEN_BYTES: usize = 128;
const MAX_PANES: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SidebarPhase {
    Evacuating,
    Open,
}

/// The complete finite state that may cross the sidebar/dashboard process seam.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SidebarState {
    pub(crate) schema_version: u64,
    pub(crate) phase: SidebarPhase,
    pub(crate) token: String,
    pub(crate) workspace: String,
    pub(crate) tab: String,
    pub(crate) original_focus: String,
    pub(crate) plan: RebuildPlan,
    pub(crate) parked: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) parking_placeholder: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) sidebar_pane: Option<String>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(crate) enum SidebarStateError {
    #[error("sidebar state is corrupt")]
    Corrupt,
    #[error("sidebar state uses an incompatible schema")]
    Incompatible,
    #[error("sidebar state belongs to another action")]
    ForeignOwner,
    #[error("sidebar state path is unsafe")]
    Unsafe,
    #[error("sidebar state is unavailable")]
    Unavailable,
}

pub(crate) trait SidebarStore {
    fn load(&self) -> Result<Option<SidebarState>, SidebarStateError>;
    fn save(&self, state: &SidebarState) -> Result<(), SidebarStateError>;
    fn remove_owned(&self, token: &str) -> Result<bool, SidebarStateError>;
}

#[derive(Clone, Debug)]
pub(crate) struct FileSidebarStore {
    path: PathBuf,
}

impl FileSidebarStore {
    pub(crate) fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

impl SidebarStore for FileSidebarStore {
    fn load(&self) -> Result<Option<SidebarState>, SidebarStateError> {
        let Some(bytes) = atomic::read_regular(&self.path).map_err(map_atomic_error)? else {
            return Ok(None);
        };
        parse_state(&bytes).map(Some)
    }

    fn save(&self, state: &SidebarState) -> Result<(), SidebarStateError> {
        validate_state(state)?;
        let mut bytes = serde_json::to_vec(state).map_err(|_| SidebarStateError::Unavailable)?;
        bytes.push(b'\n');
        if bytes.len() > MAX_DOCUMENT_BYTES {
            return Err(SidebarStateError::Corrupt);
        }
        atomic::replace_atomically(&self.path, &bytes, |existing| {
            let Some(existing) = existing else {
                return Ok(());
            };
            let current = parse_state(existing)?;
            if current.token == state.token {
                Ok(())
            } else {
                Err(SidebarStateError::ForeignOwner)
            }
        })
        .map_err(map_replace_error)
    }

    fn remove_owned(&self, token: &str) -> Result<bool, SidebarStateError> {
        let Some(bytes) = atomic::read_regular(&self.path).map_err(map_atomic_error)? else {
            // The matching dashboard may have released its lease while the
            // action was waiting for the plugin pane to close.
            return Ok(true);
        };
        let state = parse_state(&bytes)?;
        if state.token != token {
            return Ok(false);
        }
        let Some(quarantined) =
            atomic::quarantine_if_unchanged(&self.path, &bytes).map_err(map_atomic_error)?
        else {
            return Ok(false);
        };
        fs::remove_file(quarantined).map_err(|_| SidebarStateError::Unavailable)?;
        remove_empty_runtime_directories(&self.path);
        Ok(true)
    }
}

pub(crate) fn release_matching_open_state(path: &Path, token: &str) -> bool {
    if !is_runtime_state_path(path) || !valid_token(token) {
        return false;
    }
    let store = FileSidebarStore::new(path);
    let Ok(Some(state)) = store.load() else {
        return false;
    };
    state.phase == SidebarPhase::Open
        && state.token == token
        && store.remove_owned(token).unwrap_or(false)
}

/// A best-effort dashboard-lifetime cleanup that never exposes or removes foreign state.
pub(crate) struct SidebarOwnershipGuard {
    path: Option<PathBuf>,
    token: Option<String>,
}

impl SidebarOwnershipGuard {
    pub(crate) fn from_environment() -> Self {
        Self {
            path: std::env::var_os("HERDR_QUOTA_STATE_FILE").map(PathBuf::from),
            token: std::env::var("HERDR_QUOTA_STATE_TOKEN").ok(),
        }
    }
}

impl Drop for SidebarOwnershipGuard {
    fn drop(&mut self) {
        if let (Some(path), Some(token)) = (self.path.as_deref(), self.token.as_deref()) {
            release_matching_open_state(path, token);
        }
    }
}

pub(crate) fn runtime_state_path(session: &str, tab: &str) -> PathBuf {
    std::env::temp_dir()
        .join("herdr-quota")
        .join(safe_part(session))
        .join(format!("{}.json", safe_part(tab)))
}

fn parse_state(bytes: &[u8]) -> Result<SidebarState, SidebarStateError> {
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return Err(SidebarStateError::Corrupt);
    }
    let value = serde_json::from_slice::<serde_json::Value>(bytes)
        .map_err(|_| SidebarStateError::Corrupt)?;
    let version = value
        .as_object()
        .and_then(|object| object.get("schemaVersion"))
        .and_then(serde_json::Value::as_u64)
        .ok_or(SidebarStateError::Corrupt)?;
    if version != SIDEBAR_SCHEMA_VERSION {
        return Err(SidebarStateError::Incompatible);
    }
    let state = serde_json::from_value(value).map_err(|_| SidebarStateError::Corrupt)?;
    validate_state(&state)?;
    Ok(state)
}

fn validate_state(state: &SidebarState) -> Result<(), SidebarStateError> {
    if state.schema_version != SIDEBAR_SCHEMA_VERSION {
        return Err(SidebarStateError::Incompatible);
    }
    if !valid_token(&state.token)
        || !valid_identifier(&state.workspace)
        || !valid_identifier(&state.tab)
        || !valid_identifier(&state.original_focus)
        || !valid_identifier(&state.plan.anchor)
        || state.plan.steps.len() >= MAX_PANES
        || state.parked.len() >= MAX_PANES
        || state
            .parking_placeholder
            .as_deref()
            .is_some_and(|value| !valid_identifier(value))
        || state
            .sidebar_pane
            .as_deref()
            .is_some_and(|value| !valid_identifier(value))
        || (state.phase == SidebarPhase::Open
            && (state.sidebar_pane.is_none()
                || state.parking_placeholder.is_some()
                || !state.parked.is_empty()))
    {
        return Err(SidebarStateError::Corrupt);
    }
    let mut introduced = std::collections::HashSet::from([state.plan.anchor.as_str()]);
    for step in &state.plan.steps {
        if !valid_identifier(&step.pane)
            || !valid_identifier(&step.target)
            || !step.ratio.is_finite()
            || step.ratio <= 0.0
            || step.ratio >= 1.0
            || !introduced.contains(step.target.as_str())
            || !introduced.insert(step.pane.as_str())
        {
            return Err(SidebarStateError::Corrupt);
        }
    }
    let mut parked = std::collections::HashSet::new();
    if state.parked.iter().any(|pane| {
        !valid_identifier(pane)
            || !introduced.contains(pane.as_str())
            || pane == &state.plan.anchor
            || !parked.insert(pane.as_str())
    }) {
        return Err(SidebarStateError::Corrupt);
    }
    Ok(())
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_IDENTIFIER_BYTES && !value.chars().any(char::is_control)
}

fn valid_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_TOKEN_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn safe_part(value: &str) -> String {
    let part = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .take(80)
        .collect::<String>();
    if part.is_empty() {
        "_".to_owned()
    } else {
        part
    }
}

fn is_runtime_state_path(path: &Path) -> bool {
    let root = std::env::temp_dir().join("herdr-quota");
    let Ok(relative) = path.strip_prefix(root) else {
        return false;
    };
    let components = relative.components().collect::<Vec<_>>();
    if components.len() != 2
        || components
            .iter()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return false;
    }
    let session = components[0].as_os_str();
    let Some(file) = components[1].as_os_str().to_str() else {
        return false;
    };
    safe_os_part(session)
        && file.ends_with(".json")
        && safe_os_part(OsStr::new(&file[..file.len() - 5]))
}

fn safe_os_part(value: &OsStr) -> bool {
    value.to_str().is_some_and(|value| {
        !value.is_empty()
            && value.len() <= 80
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    })
}

fn remove_empty_runtime_directories(path: &Path) {
    let Some(session) = path.parent() else {
        return;
    };
    fs::remove_dir(session).ok();
    let Some(root) = session.parent() else {
        return;
    };
    if root.file_name() == Some(OsStr::new("herdr-quota")) {
        fs::remove_dir(root).ok();
    }
}

fn map_atomic_error(error: AtomicError) -> SidebarStateError {
    match error {
        AtomicError::UnsafeTarget => SidebarStateError::Unsafe,
        AtomicError::InvalidPath | AtomicError::Io(_) => SidebarStateError::Unavailable,
    }
}

fn map_replace_error(error: ReplaceError<SidebarStateError>) -> SidebarStateError {
    match error {
        ReplaceError::Storage(error) => map_atomic_error(error),
        ReplaceError::Validation(error) => error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sidebar::layout::{MoveStep, SplitDirection};
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    fn state(token: &str) -> SidebarState {
        SidebarState {
            schema_version: SIDEBAR_SCHEMA_VERSION,
            phase: SidebarPhase::Open,
            token: token.to_owned(),
            workspace: "workspace-1".to_owned(),
            tab: "tab-1".to_owned(),
            original_focus: "pane-2".to_owned(),
            plan: RebuildPlan {
                anchor: "pane-1".to_owned(),
                steps: vec![MoveStep {
                    pane: "pane-2".to_owned(),
                    direction: SplitDirection::Right,
                    target: "pane-1".to_owned(),
                    ratio: 0.4,
                }],
            },
            parked: Vec::new(),
            parking_placeholder: None,
            sidebar_pane: Some("sidebar".to_owned()),
        }
    }

    #[test]
    fn schema_v1_is_finite_private_and_contains_no_provider_or_credential_payload() {
        let directory = TempDir::new().expect("temporary directory");
        let path = directory.path().join("session").join("tab.json");
        let store = FileSidebarStore::new(&path);
        store.save(&state("token-1")).expect("save state");

        let text = fs::read_to_string(&path).expect("read state");
        let value: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
        assert_eq!(value["schemaVersion"], SIDEBAR_SCHEMA_VERSION);
        assert_eq!(
            value
                .as_object()
                .expect("object")
                .keys()
                .collect::<Vec<_>>(),
            [
                "originalFocus",
                "parked",
                "phase",
                "plan",
                "schemaVersion",
                "sidebarPane",
                "tab",
                "token",
                "workspace",
            ]
        );
        for forbidden in ["credential", "account", "provider", "payload", "stateFile"] {
            assert!(!text.contains(forbidden));
        }
        #[cfg(unix)]
        {
            assert_eq!(
                fs::metadata(&path).expect("file mode").permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(path.parent().expect("parent"))
                    .expect("directory mode")
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
    }

    #[test]
    fn corrupt_and_future_documents_fail_closed_without_changing_bytes() {
        let directory = TempDir::new().expect("temporary directory");
        let path = directory.path().join("state.json");
        let store = FileSidebarStore::new(&path);
        for bytes in [
            b"{truncated".as_slice(),
            br#"{"schemaVersion":2,"future":"preserved"}"#.as_slice(),
            br#"{"schemaVersion":1,"phase":"open","token":"x","unknown":"raw"}"#.as_slice(),
        ] {
            fs::write(&path, bytes).expect("write document");
            assert!(store.load().is_err());
            assert_eq!(fs::read(&path).expect("preserved document"), bytes);
        }

        let oversized = vec![b'x'; MAX_DOCUMENT_BYTES + 1];
        fs::write(&path, &oversized).expect("write oversized document");
        assert_eq!(store.load(), Err(SidebarStateError::Corrupt));
        assert_eq!(
            fs::read(&path).expect("preserved oversized document"),
            oversized
        );
    }

    #[test]
    fn matching_owned_cleanup_requires_the_open_phase_and_exact_token() {
        let path = runtime_state_path(&format!("test-{}", std::process::id()), "matching-cleanup");
        fs::remove_file(&path).ok();
        let store = FileSidebarStore::new(&path);
        let mut owned = state("new-token");
        owned.phase = SidebarPhase::Evacuating;
        store.save(&owned).expect("save evacuating state");
        assert!(!release_matching_open_state(&path, "new-token"));
        assert!(path.exists());

        owned.phase = SidebarPhase::Open;
        store.save(&owned).expect("save open state");
        assert!(!release_matching_open_state(&path, "old-token"));
        assert!(path.exists());
        assert!(release_matching_open_state(&path, "new-token"));
        assert!(!path.exists());
    }

    #[test]
    fn bounds_and_foreign_ownership_prevent_untrusted_replacement() {
        let directory = TempDir::new().expect("temporary directory");
        let path = directory.path().join("state.json");
        let store = FileSidebarStore::new(&path);
        store.save(&state("owner-a")).expect("save owner A");
        assert_eq!(
            store.save(&state("owner-b")),
            Err(SidebarStateError::ForeignOwner)
        );

        let mut oversized = state("owner-a");
        oversized.workspace = "x".repeat(MAX_IDENTIFIER_BYTES + 1);
        assert_eq!(store.save(&oversized), Err(SidebarStateError::Corrupt));
        assert_eq!(
            store.load().expect("load original").expect("state").token,
            "owner-a"
        );

        let mut unsafe_plan = state("owner-a");
        unsafe_plan.plan.steps[0].target = "not-introduced".to_owned();
        assert_eq!(store.save(&unsafe_plan), Err(SidebarStateError::Corrupt));

        let mut too_many = state("owner-a");
        too_many.phase = SidebarPhase::Evacuating;
        too_many.sidebar_pane = None;
        too_many.plan.steps = (0..MAX_PANES)
            .map(|index| MoveStep {
                pane: format!("pane-{index}"),
                direction: SplitDirection::Right,
                target: if index == 0 {
                    too_many.plan.anchor.clone()
                } else {
                    format!("pane-{}", index - 1)
                },
                ratio: 0.5,
            })
            .collect();
        assert_eq!(store.save(&too_many), Err(SidebarStateError::Corrupt));
    }

    #[test]
    fn cleanup_rejects_paths_outside_the_private_runtime_tree() {
        let directory = TempDir::new().expect("temporary directory");
        let path = directory.path().join("credential.json");
        fs::write(
            &path,
            serde_json::to_vec(&state("token-1")).expect("serialize"),
        )
        .expect("write decoy");

        assert!(!release_matching_open_state(&path, "token-1"));
        assert!(path.exists());
    }
}
