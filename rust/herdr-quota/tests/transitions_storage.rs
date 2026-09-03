use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{DateTime, Utc};
use herdr_quota::domain::provider::MarketedProvider;
use herdr_quota::store::transitions::{
    HistoryDataHealth, HistoryFact, HistoryProviderSnapshot, HistoryRunway, HistoryRunwayState,
    HistorySnapshot, PersistedTransitionKind, RemainingThreshold, TRANSITION_MAX_AGE_MILLIS,
    TRANSITION_MAX_EVENTS, TRANSITION_SCHEMA_VERSION, TransitionAvailability, TransitionDocument,
    TransitionHistory, TransitionPolicy, TransitionSettings, TransitionStore,
    append_transition_events, parse_transition_document, transition_path,
    write_transition_document_atomic,
};
use serde_json::{Value, json};

#[cfg(unix)]
use std::os::unix::fs::{PermissionsExt, symlink};

static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const BASE: i64 = 1_787_227_200_000;
const RESET: &str = "2026-08-27T12:00:00.000Z";

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "herdr-quota-transition-test-{}-{sequence}",
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

fn timestamp(minute: i64) -> String {
    DateTime::<Utc>::from_timestamp_millis(BASE + minute * 60_000)
        .expect("test timestamp")
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn settings() -> TransitionSettings {
    TransitionSettings {
        hidden_providers: Vec::new(),
        remaining_threshold: RemainingThreshold::Percent25,
        forecast_before_reset: false,
    }
}

fn snapshot(minute: i64, remaining: f64) -> HistorySnapshot {
    HistorySnapshot {
        captured_at: timestamp(minute),
        providers: vec![HistoryProviderSnapshot {
            provider: MarketedProvider::Codex,
            data_health: HistoryDataHealth::Current,
            auth_eligible: true,
            facts: vec![HistoryFact {
                scope: "All models".to_owned(),
                limit: Some("Week".to_owned()),
                remaining,
                reset_at: Some(RESET.to_owned()),
                runway: Some(HistoryRunway {
                    state: HistoryRunwayState::ThroughReset,
                    confidence: None,
                }),
            }],
        }],
    }
}

fn history(snapshots: Vec<HistorySnapshot>) -> TransitionHistory {
    TransitionHistory { snapshots }
}

fn event(minute: i64) -> herdr_quota::store::transitions::TransitionEvent {
    herdr_quota::store::transitions::TransitionEvent {
        provider: MarketedProvider::Codex,
        scope: "All models".to_owned(),
        limit: Some("Week".to_owned()),
        cycle: RESET.to_owned(),
        policy: TransitionPolicy {
            remaining_threshold: RemainingThreshold::Percent25,
            forecast_before_reset: false,
        },
        kind: PersistedTransitionKind::ThresholdBaseline,
        occurred_at: timestamp(minute),
        acknowledged_at: None,
    }
}

fn document(events: Vec<herdr_quota::store::transitions::TransitionEvent>) -> TransitionDocument {
    TransitionDocument {
        schema_version: TRANSITION_SCHEMA_VERSION,
        events,
    }
}

#[test]
fn path_and_v1_migration_keep_the_existing_filename_and_bytes_until_save() {
    assert_eq!(
        transition_path(Some(OsStr::new("state-root")), Path::new("unused")),
        Path::new("state-root")
            .join("herdr-quota")
            .join("transitions-v1.json")
    );
    assert_eq!(
        transition_path(None, Path::new("home-root")),
        Path::new("home-root").join(".local/state/herdr-quota/transitions-v1.json")
    );

    let directory = TestDirectory::new();
    let path = directory.path().join("transitions-v1.json");
    let v1 = include_str!("../../../test/fixtures/transitions-v1.json");
    fs::write(&path, v1).expect("write v1 fixture");
    let store = TransitionStore::new(&path);
    assert_eq!(
        store
            .load_view(&history(vec![snapshot(0, 40.0)]), &settings())
            .availability,
        TransitionAvailability::Ready
    );
    assert_eq!(fs::read_to_string(&path).expect("read v1 fixture"), v1);

    let value: Value = serde_json::from_str(v1).expect("parse v1 JSON");
    let migrated = parse_transition_document(&value).expect("migrate v1");
    assert_eq!(migrated.schema_version, TRANSITION_SCHEMA_VERSION);
    write_transition_document_atomic(&path, &migrated).expect("save migrated state");
    let saved: Value = serde_json::from_slice(&fs::read(&path).expect("read migrated state"))
        .expect("parse migrated state");
    assert_eq!(saved["schemaVersion"], TRANSITION_SCHEMA_VERSION);
}

#[test]
fn parser_and_writer_enforce_the_finite_allow_list() {
    for provider in MarketedProvider::ALL {
        let mut provider_event = event(0);
        provider_event.provider = provider;
        let value = serde_json::to_value(document(vec![provider_event])).expect("serialize event");
        parse_transition_document(&value).expect("parse marketed provider");
    }

    for invalid in [
        json!({
            "schemaVersion": 2,
            "events": [{
                "provider": "Future provider",
                "scope": "All models",
                "limit": "Week",
                "cycle": RESET,
                "policy": {"remainingThreshold": 25, "forecastBeforeReset": false},
                "kind": "threshold_baseline",
                "occurredAt": timestamp(0)
            }]
        }),
        json!({
            "schemaVersion": 2,
            "events": [{
                "provider": "OpenAI Codex",
                "scope": "All models",
                "limit": "Week",
                "cycle": RESET,
                "policy": {"remainingThreshold": 25, "forecastBeforeReset": false},
                "kind": "threshold_baseline",
                "occurredAt": timestamp(0),
                "accountId": "secret",
                "rawPayload": {"token": "credential"}
            }]
        }),
    ] {
        assert!(parse_transition_document(&invalid).is_err());
    }

    let directory = TestDirectory::new();
    let path = directory.path().join("events.json");
    let mut unsafe_document = document(vec![event(0)]);
    unsafe_document.events[0].scope = "token secret".to_owned();
    assert!(write_transition_document_atomic(&path, &unsafe_document).is_err());
    assert!(!path.exists());
}

#[test]
fn private_atomic_persistence_and_acknowledgement_survive_reopen() {
    let directory = TestDirectory::new();
    let path = directory
        .path()
        .join("state/herdr-quota/transitions-v1.json");
    let store = TransitionStore::new(&path);
    let first = history(vec![snapshot(0, 40.0)]);
    assert!(store.evaluate(&first, &settings()).events.is_empty());

    let crossed = history(vec![snapshot(0, 40.0), snapshot(5, 20.0)]);
    let view = store.evaluate(&crossed, &settings());
    assert_eq!(view.events.len(), 1);
    assert_eq!(view.events[0].remaining, Some(20.0));
    #[cfg(unix)]
    {
        assert_eq!(
            fs::metadata(&path)
                .expect("transition metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(path.parent().expect("transition parent"))
                .expect("transition parent metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }

    let reopened = TransitionStore::new(&path);
    assert_eq!(reopened.load_view(&crossed, &settings()).events.len(), 1);
    let acknowledged = reopened.acknowledge(
        &crossed,
        &settings(),
        DateTime::<Utc>::from_timestamp_millis(BASE).expect("ack time"),
    );
    assert!(acknowledged.events.is_empty());
    let stored: Value = serde_json::from_slice(&fs::read(&path).expect("read transition state"))
        .expect("parse transition state");
    let parsed = parse_transition_document(&stored).expect("validate transition state");
    let entered = parsed
        .events
        .iter()
        .find(|event| event.kind == PersistedTransitionKind::ThresholdEnter)
        .expect("persisted crossing");
    assert_eq!(
        entered.acknowledged_at.as_deref(),
        Some(timestamp(5).as_str())
    );
    assert!(reopened.load_view(&crossed, &settings()).events.is_empty());
}

#[test]
fn malformed_state_recovers_to_a_finite_baseline() {
    let directory = TestDirectory::new();
    let path = directory.path().join("transitions-v1.json");
    fs::write(&path, b"{truncated").expect("write malformed state");
    let view = TransitionStore::new(&path).evaluate(&history(vec![snapshot(0, 40.0)]), &settings());
    assert_eq!(view.availability, TransitionAvailability::Recovered);
    assert!(view.events.is_empty());
    let saved: Value = serde_json::from_slice(&fs::read(&path).expect("read recovered state"))
        .expect("parse recovered JSON");
    assert_eq!(
        saved["events"][0]["kind"],
        Value::String("threshold_baseline".to_owned())
    );
}

#[test]
fn retention_is_bounded_by_count_and_age() {
    let mut current = TransitionDocument::default();
    for minute in 0..(TRANSITION_MAX_EVENTS as i64 + 40) {
        current = append_transition_events(&current, vec![event(minute)]).document;
    }
    assert_eq!(current.events.len(), TRANSITION_MAX_EVENTS);
    assert_eq!(
        current.events.last().expect("newest event").occurred_at,
        event(TRANSITION_MAX_EVENTS as i64 + 39).occurred_at
    );

    let outside_window = TRANSITION_MAX_AGE_MILLIS / 60_000 + 1;
    let aged = append_transition_events(&document(vec![event(0)]), vec![event(outside_window)]);
    assert_eq!(aged.document.events, [event(outside_window)]);
}

#[test]
fn future_schema_is_preserved_across_load_evaluate_acknowledge_and_clear() {
    let directory = TestDirectory::new();
    let path = directory.path().join("transitions-v1.json");
    let future = format!(
        "{{\"schemaVersion\":{},\"future\":\"keep\"}}\n",
        TRANSITION_SCHEMA_VERSION + 1
    );
    fs::write(&path, &future).expect("write future state");
    let store = TransitionStore::new(&path);
    let samples = history(vec![snapshot(0, 40.0), snapshot(5, 20.0)]);
    assert_eq!(
        store.load_view(&samples, &settings()).availability,
        TransitionAvailability::Incompatible
    );
    assert_eq!(
        store.evaluate(&samples, &settings()).availability,
        TransitionAvailability::Incompatible
    );
    assert_eq!(
        store
            .acknowledge(
                &samples,
                &settings(),
                DateTime::<Utc>::from_timestamp_millis(BASE).expect("ack time")
            )
            .availability,
        TransitionAvailability::Incompatible
    );
    assert_eq!(
        store.clear().availability,
        TransitionAvailability::Incompatible
    );
    assert_eq!(
        fs::read_to_string(&path).expect("read future state"),
        future
    );
}

#[cfg(unix)]
#[test]
fn symlink_directory_and_read_only_targets_fail_closed_without_losing_bytes() {
    let directory = TestDirectory::new();
    let destination = directory.path().join("destination.json");
    let link = directory.path().join("link.json");
    let original = serde_json::to_vec(&document(vec![event(0)])).expect("serialize state");
    fs::write(&destination, &original).expect("write destination");
    symlink(&destination, &link).expect("create symlink");
    let linked = TransitionStore::new(&link);
    assert_eq!(
        linked
            .evaluate(
                &history(vec![snapshot(0, 40.0), snapshot(5, 20.0)]),
                &settings()
            )
            .availability,
        TransitionAvailability::Unavailable
    );
    assert_eq!(
        linked.clear().availability,
        TransitionAvailability::Unavailable
    );
    assert_eq!(fs::read(&destination).expect("read destination"), original);

    let read_only = directory.path().join("readonly.json");
    fs::write(&read_only, &original).expect("write read-only state");
    fs::set_permissions(&read_only, fs::Permissions::from_mode(0o400))
        .expect("make state read-only");
    let result = TransitionStore::new(&read_only).evaluate(
        &history(vec![snapshot(0, 40.0), snapshot(5, 20.0)]),
        &settings(),
    );
    assert_eq!(result.availability, TransitionAvailability::Unavailable);
    assert_eq!(
        fs::read(&read_only).expect("read preserved state"),
        original
    );
    fs::set_permissions(&read_only, fs::Permissions::from_mode(0o600)).expect("restore state mode");

    let target_directory = directory.path().join("target-directory");
    fs::create_dir(&target_directory).expect("create target directory");
    assert_eq!(
        TransitionStore::new(&target_directory).clear().availability,
        TransitionAvailability::Unavailable
    );
}

#[test]
fn clear_removes_only_transition_history_and_leaves_neighboring_state() {
    let directory = TestDirectory::new();
    let state = directory.path().join("herdr-quota");
    fs::create_dir(&state).expect("create state directory");
    let transitions = state.join("transitions-v1.json");
    let quota_history = state.join("history-v1.json");
    write_transition_document_atomic(&transitions, &document(vec![event(0)]))
        .expect("write transitions");
    fs::write(&quota_history, b"quota-history-stays\n").expect("write quota history");

    let cleared = TransitionStore::new(&transitions).clear();
    assert_eq!(cleared.availability, TransitionAvailability::FirstRun);
    assert!(!transitions.exists());
    assert_eq!(
        fs::read(&quota_history).expect("read quota history"),
        b"quota-history-stays\n"
    );
}
