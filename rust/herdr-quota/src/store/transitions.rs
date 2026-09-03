//! Bounded transition reduction, review, acknowledgement, and persistence.
//!
//! This module consumes only already-sanitized history facts. It does not
//! collect quota data and deliberately has no notification or OS integration.

use std::cmp::max;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, SecondsFormat, Utc};
use serde::Serialize;
use serde::ser::{SerializeStruct, Serializer};
use serde_json::{Map, Value};
use thiserror::Error;

use crate::domain::provider::MarketedProvider;

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};

/// The current transition document schema.
pub const TRANSITION_SCHEMA_VERSION: u64 = 2;
/// The maximum number of persisted transition records.
pub const TRANSITION_MAX_EVENTS: usize = 256;
/// The maximum age of persisted records, measured from the newest record.
pub const TRANSITION_MAX_AGE_MILLIS: i64 = 30 * 24 * 60 * 60 * 1_000;
/// A rollback beyond this tolerance begins a new transition segment.
pub const TRANSITION_CLOCK_SKEW_MILLIS: i64 = 5 * 60 * 1_000;

static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Remaining-capacity policy used by transition evaluation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemainingThreshold {
    Off,
    Percent25,
    Percent10,
    Percent5,
}

impl RemainingThreshold {
    fn percent(self) -> Option<f64> {
        match self {
            Self::Off => None,
            Self::Percent25 => Some(25.0),
            Self::Percent10 => Some(10.0),
            Self::Percent5 => Some(5.0),
        }
    }
}

impl Serialize for RemainingThreshold {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Off => serializer.serialize_str("off"),
            Self::Percent25 => serializer.serialize_u8(25),
            Self::Percent10 => serializer.serialize_u8(10),
            Self::Percent5 => serializer.serialize_u8(5),
        }
    }
}

/// The finite settings projection that can affect local transition state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransitionSettings {
    pub hidden_providers: Vec<MarketedProvider>,
    pub remaining_threshold: RemainingThreshold,
    pub forecast_before_reset: bool,
}

impl Default for TransitionSettings {
    fn default() -> Self {
        Self {
            hidden_providers: Vec::new(),
            remaining_threshold: RemainingThreshold::Off,
            forecast_before_reset: false,
        }
    }
}

/// The policy persisted with an event for channel-specific deduplication.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransitionPolicy {
    pub remaining_threshold: RemainingThreshold,
    pub forecast_before_reset: bool,
}

/// Trust state of a provider in sanitized history.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HistoryDataHealth {
    Current,
    Stale,
    Unavailable,
    Error,
    Unknown,
}

/// Projection state retained by sanitized quota history.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HistoryRunwayState {
    ExhaustedNow,
    ProjectedExhaustion,
    ThroughReset,
    Unknown,
}

/// Confidence retained by sanitized quota history.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HistoryProjectionConfidence {
    Early,
    Established,
}

/// A bounded runway fact used by forecast transitions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HistoryRunway {
    pub state: HistoryRunwayState,
    pub confidence: Option<HistoryProjectionConfidence>,
}

/// One sanitized quota fact used by transition evaluation.
#[derive(Clone, Debug, PartialEq)]
pub struct HistoryFact {
    pub scope: String,
    pub limit: Option<String>,
    pub remaining: f64,
    pub reset_at: Option<String>,
    pub runway: Option<HistoryRunway>,
}

/// One provider's facts at a history sample.
#[derive(Clone, Debug, PartialEq)]
pub struct HistoryProviderSnapshot {
    pub provider: MarketedProvider,
    pub data_health: HistoryDataHealth,
    pub auth_eligible: bool,
    pub facts: Vec<HistoryFact>,
}

/// One timestamped sanitized history sample.
#[derive(Clone, Debug, PartialEq)]
pub struct HistorySnapshot {
    pub captured_at: String,
    pub providers: Vec<HistoryProviderSnapshot>,
}

/// The retained history that transition reduction may inspect.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TransitionHistory {
    pub snapshots: Vec<HistorySnapshot>,
}

/// A persisted baseline or actionable transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PersistedTransitionKind {
    ThresholdBaseline,
    ForecastBaseline,
    ThresholdEnter,
    ThresholdRecovery,
    ForecastEnter,
    ForecastRecovery,
}

impl PersistedTransitionKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::ThresholdBaseline => "threshold_baseline",
            Self::ForecastBaseline => "forecast_baseline",
            Self::ThresholdEnter => "threshold_enter",
            Self::ThresholdRecovery => "threshold_recovery",
            Self::ForecastEnter => "forecast_enter",
            Self::ForecastRecovery => "forecast_recovery",
        }
    }

    fn channel(self) -> TransitionChannel {
        match self {
            Self::ThresholdBaseline | Self::ThresholdEnter | Self::ThresholdRecovery => {
                TransitionChannel::Threshold
            }
            Self::ForecastBaseline | Self::ForecastEnter | Self::ForecastRecovery => {
                TransitionChannel::Forecast
            }
        }
    }

    fn is_baseline(self) -> bool {
        matches!(self, Self::ThresholdBaseline | Self::ForecastBaseline)
    }

    fn is_actionable(self) -> bool {
        !self.is_baseline()
    }
}

impl Serialize for PersistedTransitionKind {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

/// An actionable transition kind shown in review.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransitionKind {
    ThresholdEnter,
    ThresholdRecovery,
    ForecastEnter,
    ForecastRecovery,
}

impl TryFrom<PersistedTransitionKind> for TransitionKind {
    type Error = ();

    fn try_from(value: PersistedTransitionKind) -> Result<Self, Self::Error> {
        match value {
            PersistedTransitionKind::ThresholdEnter => Ok(Self::ThresholdEnter),
            PersistedTransitionKind::ThresholdRecovery => Ok(Self::ThresholdRecovery),
            PersistedTransitionKind::ForecastEnter => Ok(Self::ForecastEnter),
            PersistedTransitionKind::ForecastRecovery => Ok(Self::ForecastRecovery),
            PersistedTransitionKind::ThresholdBaseline
            | PersistedTransitionKind::ForecastBaseline => Err(()),
        }
    }
}

/// One finite transition audit record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransitionEvent {
    pub provider: MarketedProvider,
    pub scope: String,
    pub limit: Option<String>,
    pub cycle: String,
    pub policy: TransitionPolicy,
    pub kind: PersistedTransitionKind,
    pub occurred_at: String,
    pub acknowledged_at: Option<String>,
}

impl Serialize for TransitionEvent {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let field_count =
            6 + usize::from(self.limit.is_some()) + usize::from(self.acknowledged_at.is_some());
        let mut state = serializer.serialize_struct("TransitionEvent", field_count)?;
        state.serialize_field("provider", self.provider.label())?;
        state.serialize_field("scope", &self.scope)?;
        if let Some(limit) = &self.limit {
            state.serialize_field("limit", limit)?;
        }
        state.serialize_field("cycle", &self.cycle)?;
        state.serialize_field("policy", &self.policy)?;
        state.serialize_field("kind", &self.kind)?;
        state.serialize_field("occurredAt", &self.occurred_at)?;
        if let Some(acknowledged_at) = &self.acknowledged_at {
            state.serialize_field("acknowledgedAt", acknowledged_at)?;
        }
        state.end()
    }
}

/// The finite transition document persisted on disk.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransitionDocument {
    pub schema_version: u64,
    pub events: Vec<TransitionEvent>,
}

impl Default for TransitionDocument {
    fn default() -> Self {
        Self {
            schema_version: TRANSITION_SCHEMA_VERSION,
            events: Vec::new(),
        }
    }
}

/// The outcome of one pure transition reduction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransitionEvaluation {
    pub document: TransitionDocument,
    pub generated: Vec<TransitionEvent>,
    pub clock_skew: bool,
}

/// Storage state attached to a transition review.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransitionAvailability {
    Ready,
    FirstRun,
    Recovered,
    Incompatible,
    Unavailable,
    ClockSkew,
}

/// A finite actionable event suitable for rendering in the TUI.
#[derive(Clone, Debug, PartialEq)]
pub struct TransitionDisplayEvent {
    pub kind: TransitionKind,
    pub provider: MarketedProvider,
    pub scope: String,
    pub limit: Option<String>,
    pub threshold: RemainingThreshold,
    pub occurred_at: String,
    pub remaining: Option<f64>,
}

/// The current review surface. Events are newest first.
#[derive(Clone, Debug, PartialEq)]
pub struct TransitionView {
    pub availability: TransitionAvailability,
    pub events: Vec<TransitionDisplayEvent>,
}

/// Select transition channels when establishing a baseline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransitionChannel {
    Threshold,
    Forecast,
}

/// A transition document validation or storage failure.
#[derive(Debug, Error)]
pub enum TransitionError {
    #[error("transitions_corrupt")]
    Corrupt,
    #[error("transitions_incompatible")]
    Incompatible,
    #[error("transitions_unsafe")]
    Unsafe,
    #[error("transitions_unavailable: {0}")]
    Unavailable(#[source] io::Error),
}

impl From<io::Error> for TransitionError {
    fn from(error: io::Error) -> Self {
        Self::Unavailable(error)
    }
}

/// Parse only schema v1 or v2 and return the in-memory v2 document.
pub fn parse_transition_document(value: &Value) -> Result<TransitionDocument, TransitionError> {
    let object = value.as_object().ok_or(TransitionError::Corrupt)?;
    let schema_value = object
        .get("schemaVersion")
        .ok_or(TransitionError::Corrupt)?;
    if !schema_value.is_number() {
        return Err(TransitionError::Corrupt);
    }
    let schema_version = schema_value.as_u64();
    if schema_version != Some(1) && schema_version != Some(TRANSITION_SCHEMA_VERSION) {
        return Err(TransitionError::Incompatible);
    }
    if !exact_keys(object, &["schemaVersion", "events"]) {
        return Err(TransitionError::Corrupt);
    }
    let values = object
        .get("events")
        .and_then(Value::as_array)
        .ok_or(TransitionError::Corrupt)?;
    if values.len() > TRANSITION_MAX_EVENTS {
        return Err(TransitionError::Corrupt);
    }
    let events = values
        .iter()
        .map(parse_event)
        .collect::<Result<Vec<_>, _>>()?;
    if events.windows(2).any(|pair| {
        timestamp_millis(&pair[0].occurred_at).expect("validated timestamp")
            > timestamp_millis(&pair[1].occurred_at).expect("validated timestamp")
    }) {
        return Err(TransitionError::Corrupt);
    }
    Ok(TransitionDocument {
        schema_version: TRANSITION_SCHEMA_VERSION,
        events,
    })
}

fn parse_event(value: &Value) -> Result<TransitionEvent, TransitionError> {
    let object = value.as_object().ok_or(TransitionError::Corrupt)?;
    let mut keys = vec!["provider", "scope", "cycle", "policy", "kind", "occurredAt"];
    if object.contains_key("limit") {
        keys.push("limit");
    }
    if object.contains_key("acknowledgedAt") {
        keys.push("acknowledgedAt");
    }
    if !exact_keys(object, &keys) {
        return Err(TransitionError::Corrupt);
    }

    let provider =
        provider_from_label(text(object, "provider")?).ok_or(TransitionError::Corrupt)?;
    let scope = safe_identity(text(object, "scope")?).ok_or(TransitionError::Corrupt)?;
    let limit = object
        .get("limit")
        .map(|_| safe_identity(text(object, "limit")?).ok_or(TransitionError::Corrupt))
        .transpose()?;
    let cycle = text(object, "cycle")?.to_owned();
    if cycle != "unbounded" && canonical_timestamp(&cycle).is_none() {
        return Err(TransitionError::Corrupt);
    }
    let policy = parse_policy(object.get("policy").ok_or(TransitionError::Corrupt)?)?;
    let kind = parse_kind(text(object, "kind")?).ok_or(TransitionError::Corrupt)?;
    let occurred_at =
        canonical_timestamp(text(object, "occurredAt")?).ok_or(TransitionError::Corrupt)?;
    let acknowledged_at = object
        .get("acknowledgedAt")
        .map(|_| {
            canonical_timestamp(text(object, "acknowledgedAt")?).ok_or(TransitionError::Corrupt)
        })
        .transpose()?;
    if acknowledged_at.as_ref().is_some_and(|acknowledged| {
        timestamp_millis(acknowledged).expect("validated timestamp")
            < timestamp_millis(&occurred_at).expect("validated timestamp")
    }) {
        return Err(TransitionError::Corrupt);
    }

    Ok(TransitionEvent {
        provider,
        scope,
        limit,
        cycle,
        policy,
        kind,
        occurred_at,
        acknowledged_at,
    })
}

fn parse_policy(value: &Value) -> Result<TransitionPolicy, TransitionError> {
    let object = value.as_object().ok_or(TransitionError::Corrupt)?;
    if !exact_keys(object, &["remainingThreshold", "forecastBeforeReset"]) {
        return Err(TransitionError::Corrupt);
    }
    let remaining_threshold = match object.get("remainingThreshold") {
        Some(Value::String(value)) if value == "off" => RemainingThreshold::Off,
        Some(Value::Number(value)) if value.as_u64() == Some(25) => RemainingThreshold::Percent25,
        Some(Value::Number(value)) if value.as_u64() == Some(10) => RemainingThreshold::Percent10,
        Some(Value::Number(value)) if value.as_u64() == Some(5) => RemainingThreshold::Percent5,
        _ => return Err(TransitionError::Corrupt),
    };
    let forecast_before_reset = object
        .get("forecastBeforeReset")
        .and_then(Value::as_bool)
        .ok_or(TransitionError::Corrupt)?;
    Ok(TransitionPolicy {
        remaining_threshold,
        forecast_before_reset,
    })
}

fn parse_kind(value: &str) -> Option<PersistedTransitionKind> {
    match value {
        "threshold_baseline" => Some(PersistedTransitionKind::ThresholdBaseline),
        "forecast_baseline" => Some(PersistedTransitionKind::ForecastBaseline),
        "threshold_enter" => Some(PersistedTransitionKind::ThresholdEnter),
        "threshold_recovery" => Some(PersistedTransitionKind::ThresholdRecovery),
        "forecast_enter" => Some(PersistedTransitionKind::ForecastEnter),
        "forecast_recovery" => Some(PersistedTransitionKind::ForecastRecovery),
        _ => None,
    }
}

fn provider_from_label(label: &str) -> Option<MarketedProvider> {
    MarketedProvider::ALL
        .into_iter()
        .find(|provider| provider.label() == label)
}

fn text<'a>(object: &'a Map<String, Value>, key: &str) -> Result<&'a str, TransitionError> {
    object
        .get(key)
        .and_then(Value::as_str)
        .ok_or(TransitionError::Corrupt)
}

fn exact_keys(object: &Map<String, Value>, expected: &[&str]) -> bool {
    object.len() == expected.len() && expected.iter().all(|key| object.contains_key(*key))
}

fn safe_identity(value: &str) -> Option<String> {
    if value.is_empty() || value.len() > 32 {
        return None;
    }
    let mut characters = value.chars();
    let first = characters.next()?;
    if !first.is_ascii_alphanumeric()
        || !characters.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, ' ' | '.' | '_' | '+' | '-')
        })
    {
        return None;
    }
    let lower = value.to_ascii_lowercase();
    for sensitive in [
        "bearer",
        "secret",
        "token",
        "credential",
        "account",
        "auth",
        "apikey",
        "api_key",
        "api-key",
        "api key",
    ] {
        if lower.contains(sensitive) {
            return None;
        }
    }
    Some(value.to_owned())
}

fn canonical_timestamp(value: &str) -> Option<String> {
    if value.len() != 24 || !value.ends_with('Z') {
        return None;
    }
    let parsed = DateTime::parse_from_rfc3339(value).ok()?;
    let canonical = parsed
        .with_timezone(&Utc)
        .to_rfc3339_opts(SecondsFormat::Millis, true);
    (canonical == value).then_some(canonical)
}

fn timestamp_millis(value: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.timestamp_millis())
}

/// Return whether at least one transition channel is enabled.
pub fn transition_policy_enabled(settings: &TransitionSettings) -> bool {
    settings.remaining_threshold != RemainingThreshold::Off || settings.forecast_before_reset
}

fn policy_for(settings: &TransitionSettings) -> TransitionPolicy {
    TransitionPolicy {
        remaining_threshold: settings.remaining_threshold,
        forecast_before_reset: settings.forecast_before_reset,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FactIdentity {
    provider: MarketedProvider,
    scope: String,
    limit: Option<String>,
    cycle: String,
}

struct CurrentFact<'a> {
    identity: FactIdentity,
    fact: &'a HistoryFact,
    captured_at: &'a str,
}

fn cycle_identity(reset_at: Option<&str>) -> String {
    let Some(reset_at) = reset_at else {
        return "unbounded".to_owned();
    };
    let milliseconds = timestamp_millis(reset_at).expect("sanitized history reset timestamp");
    let rounded =
        milliseconds.div_euclid(60_000) + i64::from(milliseconds.rem_euclid(60_000) >= 30_000);
    DateTime::<Utc>::from_timestamp_millis(rounded * 60_000)
        .expect("valid rounded timestamp")
        .to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn identity_for(provider: &HistoryProviderSnapshot, fact: &HistoryFact) -> FactIdentity {
    FactIdentity {
        provider: provider.provider,
        scope: fact.scope.clone(),
        limit: fact.limit.clone(),
        cycle: cycle_identity(fact.reset_at.as_deref()),
    }
}

fn same_identity(event: &TransitionEvent, identity: &FactIdentity) -> bool {
    event.provider == identity.provider
        && event.scope == identity.scope
        && event.limit == identity.limit
        && event.cycle == identity.cycle
}

fn channel_policy_matches(
    event: &TransitionEvent,
    settings: &TransitionSettings,
    channel: TransitionChannel,
) -> bool {
    match channel {
        TransitionChannel::Threshold => {
            event.policy.remaining_threshold == settings.remaining_threshold
        }
        TransitionChannel::Forecast => {
            event.policy.forecast_before_reset == settings.forecast_before_reset
        }
    }
}

fn visible(provider: MarketedProvider, settings: &TransitionSettings) -> bool {
    !settings.hidden_providers.contains(&provider)
}

fn current_facts<'a>(
    history: &'a TransitionHistory,
    settings: &TransitionSettings,
    providers: Option<&[MarketedProvider]>,
) -> Vec<CurrentFact<'a>> {
    let Some(current) = history.snapshots.last() else {
        return Vec::new();
    };
    current
        .providers
        .iter()
        .filter(|provider| {
            provider.data_health == HistoryDataHealth::Current
                && provider.auth_eligible
                && visible(provider.provider, settings)
                && providers.is_none_or(|allowed| allowed.contains(&provider.provider))
        })
        .flat_map(|provider| {
            provider.facts.iter().map(|fact| CurrentFact {
                identity: identity_for(provider, fact),
                fact,
                captured_at: &current.captured_at,
            })
        })
        .collect()
}

fn event_for(
    current: &CurrentFact<'_>,
    policy: TransitionPolicy,
    kind: PersistedTransitionKind,
) -> TransitionEvent {
    TransitionEvent {
        provider: current.identity.provider,
        scope: current.identity.scope.clone(),
        limit: current.identity.limit.clone(),
        cycle: current.identity.cycle.clone(),
        policy,
        kind,
        occurred_at: current.captured_at.to_owned(),
        acknowledged_at: None,
    }
}

fn latest_channel_event<'a>(
    document: &'a TransitionDocument,
    identity: &FactIdentity,
    settings: &TransitionSettings,
    channel: TransitionChannel,
) -> Option<&'a TransitionEvent> {
    document.events.iter().rev().find(|event| {
        event.kind.channel() == channel
            && same_identity(event, identity)
            && channel_policy_matches(event, settings, channel)
    })
}

fn latest_baseline_at(
    document: &TransitionDocument,
    identity: &FactIdentity,
    settings: &TransitionSettings,
    channel: TransitionChannel,
) -> Option<i64> {
    document
        .events
        .iter()
        .rev()
        .find(|event| {
            event.kind
                == match channel {
                    TransitionChannel::Threshold => PersistedTransitionKind::ThresholdBaseline,
                    TransitionChannel::Forecast => PersistedTransitionKind::ForecastBaseline,
                }
                && same_identity(event, identity)
                && channel_policy_matches(event, settings, channel)
        })
        .and_then(|event| timestamp_millis(&event.occurred_at))
}

fn fact_in<'a>(snapshot: &'a HistorySnapshot, identity: &FactIdentity) -> Option<&'a HistoryFact> {
    let provider = snapshot.providers.iter().find(|provider| {
        provider.provider == identity.provider
            && provider.data_health == HistoryDataHealth::Current
            && provider.auth_eligible
    })?;
    provider.facts.iter().find(|fact| {
        fact.scope == identity.scope
            && fact.limit == identity.limit
            && cycle_identity(fact.reset_at.as_deref()) == identity.cycle
    })
}

fn previous_fact<'a, F>(
    history: &'a TransitionHistory,
    identity: &FactIdentity,
    baseline_at: Option<i64>,
    usable: F,
) -> Option<&'a HistoryFact>
where
    F: Fn(&HistoryFact) -> bool,
{
    for snapshot in history.snapshots.iter().rev().skip(1) {
        if baseline_at.is_some_and(|baseline| {
            timestamp_millis(&snapshot.captured_at).expect("sanitized history timestamp") < baseline
        }) {
            return None;
        }
        if let Some(fact) = fact_in(snapshot, identity).filter(|fact| usable(fact)) {
            return Some(fact);
        }
    }
    None
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ForecastClass {
    Safe,
    Risky,
}

fn forecast_class(fact: &HistoryFact) -> Option<ForecastClass> {
    match fact.runway {
        Some(HistoryRunway {
            state: HistoryRunwayState::ThroughReset,
            ..
        }) => Some(ForecastClass::Safe),
        Some(HistoryRunway {
            state: HistoryRunwayState::ExhaustedNow,
            ..
        }) => Some(ForecastClass::Risky),
        Some(HistoryRunway {
            state: HistoryRunwayState::ProjectedExhaustion,
            confidence: Some(HistoryProjectionConfidence::Established),
        }) => Some(ForecastClass::Risky),
        _ => None,
    }
}

/// Establish selected channel baselines from the current trustworthy sample.
pub fn baseline_transitions(
    document: &TransitionDocument,
    history: &TransitionHistory,
    settings: &TransitionSettings,
    channels: &[TransitionChannel],
    providers: Option<&[MarketedProvider]>,
) -> TransitionEvaluation {
    if !transition_policy_enabled(settings) {
        return unchanged(document);
    }
    let policy = policy_for(settings);
    let mut generated = Vec::new();
    for current in current_facts(history, settings, providers) {
        if channels.contains(&TransitionChannel::Threshold)
            && settings.remaining_threshold != RemainingThreshold::Off
        {
            generated.push(event_for(
                &current,
                policy,
                PersistedTransitionKind::ThresholdBaseline,
            ));
        }
        if channels.contains(&TransitionChannel::Forecast) && settings.forecast_before_reset {
            generated.push(event_for(
                &current,
                policy,
                PersistedTransitionKind::ForecastBaseline,
            ));
        }
    }
    append_transition_events(document, generated)
}

/// Reduce the latest trustworthy history sample into deduplicated transitions.
pub fn evaluate_transitions(
    document: &TransitionDocument,
    history: &TransitionHistory,
    settings: &TransitionSettings,
) -> TransitionEvaluation {
    if !transition_policy_enabled(settings) {
        return unchanged(document);
    }
    let policy = policy_for(settings);
    let mut generated = Vec::new();
    for current in current_facts(history, settings, None) {
        if let Some(threshold) = settings.remaining_threshold.percent() {
            match latest_channel_event(
                document,
                &current.identity,
                settings,
                TransitionChannel::Threshold,
            ) {
                None => generated.push(event_for(
                    &current,
                    policy,
                    PersistedTransitionKind::ThresholdBaseline,
                )),
                Some(latest) => {
                    let active = latest.kind == PersistedTransitionKind::ThresholdEnter;
                    let previous = previous_fact(
                        history,
                        &current.identity,
                        latest_baseline_at(
                            document,
                            &current.identity,
                            settings,
                            TransitionChannel::Threshold,
                        ),
                        |_| true,
                    );
                    if !active
                        && previous.is_some_and(|fact| fact.remaining > threshold)
                        && current.fact.remaining <= threshold
                    {
                        generated.push(event_for(
                            &current,
                            policy,
                            PersistedTransitionKind::ThresholdEnter,
                        ));
                    } else if active && current.fact.remaining > threshold {
                        generated.push(event_for(
                            &current,
                            policy,
                            PersistedTransitionKind::ThresholdRecovery,
                        ));
                    }
                }
            }
        }

        if settings.forecast_before_reset {
            match latest_channel_event(
                document,
                &current.identity,
                settings,
                TransitionChannel::Forecast,
            ) {
                None => generated.push(event_for(
                    &current,
                    policy,
                    PersistedTransitionKind::ForecastBaseline,
                )),
                Some(latest) => {
                    let active = latest.kind == PersistedTransitionKind::ForecastEnter;
                    let current_forecast = forecast_class(current.fact);
                    let previous_forecast = previous_fact(
                        history,
                        &current.identity,
                        latest_baseline_at(
                            document,
                            &current.identity,
                            settings,
                            TransitionChannel::Forecast,
                        ),
                        |fact| forecast_class(fact).is_some(),
                    )
                    .and_then(forecast_class);
                    if !active
                        && previous_forecast == Some(ForecastClass::Safe)
                        && current_forecast == Some(ForecastClass::Risky)
                    {
                        generated.push(event_for(
                            &current,
                            policy,
                            PersistedTransitionKind::ForecastEnter,
                        ));
                    } else if active && current_forecast == Some(ForecastClass::Safe) {
                        generated.push(event_for(
                            &current,
                            policy,
                            PersistedTransitionKind::ForecastRecovery,
                        ));
                    }
                }
            }
        }
    }
    append_transition_events(document, generated)
}

fn unchanged(document: &TransitionDocument) -> TransitionEvaluation {
    TransitionEvaluation {
        document: document.clone(),
        generated: Vec::new(),
        clock_skew: false,
    }
}

/// Append, sort, age-limit, count-limit, and rollback-segment new events.
pub fn append_transition_events(
    document: &TransitionDocument,
    generated: Vec<TransitionEvent>,
) -> TransitionEvaluation {
    if generated.is_empty() {
        return unchanged(document);
    }
    let clock_skew = document.events.last().is_some_and(|previous| {
        timestamp_millis(&generated[0].occurred_at).expect("generated timestamp")
            < timestamp_millis(&previous.occurred_at).expect("persisted timestamp")
                - TRANSITION_CLOCK_SKEW_MILLIS
    });
    let mut events = if clock_skew {
        Vec::new()
    } else {
        document.events.clone()
    };
    events.extend(generated.iter().cloned());
    events.sort_by_key(|event| timestamp_millis(&event.occurred_at).expect("validated timestamp"));
    let newest = events
        .iter()
        .filter_map(|event| timestamp_millis(&event.occurred_at))
        .max()
        .expect("generated events are non-empty");
    let cutoff = newest - TRANSITION_MAX_AGE_MILLIS;
    events.retain(|event| {
        timestamp_millis(&event.occurred_at).expect("validated timestamp") >= cutoff
    });
    if events.len() > TRANSITION_MAX_EVENTS {
        events.drain(..events.len() - TRANSITION_MAX_EVENTS);
    }
    TransitionEvaluation {
        document: TransitionDocument {
            schema_version: TRANSITION_SCHEMA_VERSION,
            events,
        },
        generated,
        clock_skew,
    }
}

fn latest_baseline_for_event(
    document: &TransitionDocument,
    event: &TransitionEvent,
    settings: &TransitionSettings,
) -> Option<i64> {
    let identity = FactIdentity {
        provider: event.provider,
        scope: event.scope.clone(),
        limit: event.limit.clone(),
        cycle: event.cycle.clone(),
    };
    latest_baseline_at(document, &identity, settings, event.kind.channel())
}

fn reviewable(
    document: &TransitionDocument,
    event: &TransitionEvent,
    settings: &TransitionSettings,
) -> bool {
    event.kind.is_actionable()
        && event.acknowledged_at.is_none()
        && visible(event.provider, settings)
        && channel_policy_matches(event, settings, event.kind.channel())
        && latest_baseline_for_event(document, event, settings).is_none_or(|baseline| {
            timestamp_millis(&event.occurred_at).expect("validated timestamp") > baseline
        })
}

fn display_fact<'a>(
    history: &'a TransitionHistory,
    event: &TransitionEvent,
) -> Option<&'a HistoryFact> {
    let snapshot = history
        .snapshots
        .iter()
        .find(|snapshot| snapshot.captured_at == event.occurred_at)?;
    fact_in(
        snapshot,
        &FactIdentity {
            provider: event.provider,
            scope: event.scope.clone(),
            limit: event.limit.clone(),
            cycle: event.cycle.clone(),
        },
    )
}

/// Build the newest-first in-pane transition review.
pub fn transition_view(
    document: &TransitionDocument,
    history: &TransitionHistory,
    settings: &TransitionSettings,
    availability: TransitionAvailability,
) -> TransitionView {
    let events = if transition_policy_enabled(settings) {
        document
            .events
            .iter()
            .filter(|event| reviewable(document, event, settings))
            .rev()
            .filter_map(|event| {
                Some(TransitionDisplayEvent {
                    kind: TransitionKind::try_from(event.kind).ok()?,
                    provider: event.provider,
                    scope: event.scope.clone(),
                    limit: event.limit.clone(),
                    threshold: event.policy.remaining_threshold,
                    occurred_at: event.occurred_at.clone(),
                    remaining: display_fact(history, event).map(|fact| fact.remaining),
                })
            })
            .collect()
    } else {
        Vec::new()
    };
    TransitionView {
        availability,
        events,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LoadKind {
    Ready,
    Missing,
    Corrupt,
    Incompatible,
    Unavailable,
}

struct LoadResult {
    kind: LoadKind,
    document: Option<TransitionDocument>,
}

impl LoadResult {
    fn availability(&self) -> TransitionAvailability {
        match self.kind {
            LoadKind::Ready => TransitionAvailability::Ready,
            LoadKind::Missing => TransitionAvailability::FirstRun,
            LoadKind::Corrupt => TransitionAvailability::Recovered,
            LoadKind::Incompatible => TransitionAvailability::Incompatible,
            LoadKind::Unavailable => TransitionAvailability::Unavailable,
        }
    }
}

/// Persistent transition state for one dashboard installation.
#[derive(Clone, Debug)]
pub struct TransitionStore {
    path: PathBuf,
}

impl TransitionStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn from_environment() -> Result<Self, TransitionError> {
        Ok(Self::new(transition_path_from_environment()?))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load_view(
        &self,
        history: &TransitionHistory,
        settings: &TransitionSettings,
    ) -> TransitionView {
        let loaded = load_document(&self.path);
        let availability = loaded.availability();
        let Some(document) = loaded.document else {
            return empty_view(availability);
        };
        transition_view(&document, history, settings, availability)
    }

    pub fn evaluate(
        &self,
        history: &TransitionHistory,
        settings: &TransitionSettings,
    ) -> TransitionView {
        let loaded = load_document(&self.path);
        let availability = loaded.availability();
        let Some(document) = loaded.document else {
            return empty_view(availability);
        };
        let update = evaluate_transitions(&document, history, settings);
        if !update.generated.is_empty()
            && write_transition_document_atomic(&self.path, &update.document).is_err()
        {
            return transition_view(
                &document,
                history,
                settings,
                TransitionAvailability::Unavailable,
            );
        }
        transition_view(
            &update.document,
            history,
            settings,
            if update.clock_skew {
                TransitionAvailability::ClockSkew
            } else {
                availability
            },
        )
    }

    pub fn baseline(
        &self,
        history: &TransitionHistory,
        settings: &TransitionSettings,
        channels: &[TransitionChannel],
        providers: Option<&[MarketedProvider]>,
    ) -> TransitionView {
        let loaded = load_document(&self.path);
        let availability = loaded.availability();
        let Some(document) = loaded.document else {
            return empty_view(availability);
        };
        let update = baseline_transitions(&document, history, settings, channels, providers);
        if !update.generated.is_empty()
            && write_transition_document_atomic(&self.path, &update.document).is_err()
        {
            return transition_view(
                &document,
                history,
                settings,
                TransitionAvailability::Unavailable,
            );
        }
        transition_view(
            &update.document,
            history,
            settings,
            if update.clock_skew {
                TransitionAvailability::ClockSkew
            } else {
                TransitionAvailability::Ready
            },
        )
    }

    pub fn acknowledge(
        &self,
        history: &TransitionHistory,
        settings: &TransitionSettings,
        now: DateTime<Utc>,
    ) -> TransitionView {
        let loaded = load_document(&self.path);
        let availability = loaded.availability();
        let Some(mut document) = loaded.document else {
            return empty_view(availability);
        };
        let visible_indices = document
            .events
            .iter()
            .enumerate()
            .filter_map(|(index, event)| reviewable(&document, event, settings).then_some(index))
            .collect::<Vec<_>>();
        if visible_indices.is_empty() {
            return transition_view(&document, history, settings, TransitionAvailability::Ready);
        }
        let latest_event = visible_indices
            .iter()
            .filter_map(|index| timestamp_millis(&document.events[*index].occurred_at))
            .max()
            .expect("reviewable events have timestamps");
        let acknowledged_at =
            DateTime::<Utc>::from_timestamp_millis(max(now.timestamp_millis(), latest_event))
                .expect("valid acknowledgement timestamp")
                .to_rfc3339_opts(SecondsFormat::Millis, true);
        for index in visible_indices {
            document.events[index].acknowledged_at = Some(acknowledged_at.clone());
        }
        if write_transition_document_atomic(&self.path, &document).is_err() {
            return transition_view(
                &load_document(&self.path).document.unwrap_or_default(),
                history,
                settings,
                TransitionAvailability::Unavailable,
            );
        }
        transition_view(&document, history, settings, TransitionAvailability::Ready)
    }

    /// Delete only the transition document. Future and unsafe files survive.
    pub fn clear(&self) -> TransitionView {
        let loaded = load_document(&self.path);
        match loaded.kind {
            LoadKind::Incompatible | LoadKind::Unavailable => empty_view(loaded.availability()),
            LoadKind::Missing => empty_view(TransitionAvailability::FirstRun),
            LoadKind::Ready | LoadKind::Corrupt => match remove_replaceable(&self.path) {
                Ok(()) => empty_view(TransitionAvailability::FirstRun),
                Err(_) => empty_view(TransitionAvailability::Unavailable),
            },
        }
    }
}

fn empty_view(availability: TransitionAvailability) -> TransitionView {
    TransitionView {
        availability,
        events: Vec::new(),
    }
}

/// Return the transition path for a selected state home and user home.
pub fn transition_path(state_home: Option<&OsStr>, home: &Path) -> PathBuf {
    let root = state_home
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local").join("state"));
    root.join("herdr-quota").join("transitions-v1.json")
}

/// Return the transition path from the current process environment.
pub fn transition_path_from_environment() -> Result<PathBuf, TransitionError> {
    if let Some(path) =
        std::env::var_os("HERDR_QUOTA_TRANSITION_FILE").filter(|value| !value.is_empty())
    {
        return Ok(PathBuf::from(path));
    }
    let state_home = std::env::var_os("XDG_STATE_HOME");
    if let Some(state_home) = state_home.as_deref().filter(|value| !value.is_empty()) {
        return Ok(transition_path(Some(state_home), Path::new("")));
    }
    let home = home_directory().ok_or_else(|| {
        TransitionError::Unavailable(io::Error::new(
            io::ErrorKind::NotFound,
            "user home directory is not set",
        ))
    })?;
    Ok(transition_path(None, &home))
}

fn load_document(path: &Path) -> LoadResult {
    match read_regular(path) {
        Ok(None) => LoadResult {
            kind: LoadKind::Missing,
            document: Some(TransitionDocument::default()),
        },
        Ok(Some(bytes)) => match serde_json::from_slice::<Value>(&bytes) {
            Ok(value) => match parse_transition_document(&value) {
                Ok(document) => LoadResult {
                    kind: LoadKind::Ready,
                    document: Some(document),
                },
                Err(TransitionError::Incompatible) => LoadResult {
                    kind: LoadKind::Incompatible,
                    document: None,
                },
                Err(TransitionError::Corrupt) => LoadResult {
                    kind: LoadKind::Corrupt,
                    document: Some(TransitionDocument::default()),
                },
                Err(TransitionError::Unsafe | TransitionError::Unavailable(_)) => LoadResult {
                    kind: LoadKind::Unavailable,
                    document: None,
                },
            },
            Err(_) => LoadResult {
                kind: LoadKind::Corrupt,
                document: Some(TransitionDocument::default()),
            },
        },
        Err(_) => LoadResult {
            kind: LoadKind::Unavailable,
            document: None,
        },
    }
}

/// Persist a validated finite document through a private sibling replacement.
pub fn write_transition_document_atomic(
    path: &Path,
    document: &TransitionDocument,
) -> Result<(), TransitionError> {
    let value = serde_json::to_value(document).map_err(|error| {
        TransitionError::Unavailable(io::Error::new(io::ErrorKind::InvalidData, error))
    })?;
    let parsed = parse_transition_document(&value)?;
    let mut bytes = serde_json::to_vec(&parsed).map_err(|error| {
        TransitionError::Unavailable(io::Error::new(io::ErrorKind::InvalidData, error))
    })?;
    bytes.push(b'\n');

    create_private_parent(path)?;
    validate_replaceable(path)?;
    let (temporary_path, mut temporary) = create_private_sibling(path)?;
    let mut renamed = false;
    let result = (|| {
        temporary.write_all(&bytes)?;
        temporary.sync_all()?;
        drop(temporary);
        validate_replaceable(path)?;
        fs::rename(&temporary_path, path).map_err(TransitionError::Unavailable)?;
        renamed = true;
        Ok(())
    })();
    if !renamed {
        fs::remove_file(temporary_path).ok();
    }
    result
}

fn create_private_parent(path: &Path) -> Result<(), TransitionError> {
    let parent = parent_directory(path);
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(0o700);
    builder.create(parent).map_err(TransitionError::Unavailable)
}

fn read_regular(path: &Path) -> Result<Option<Vec<u8>>, TransitionError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(TransitionError::Unavailable(error)),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(TransitionError::Unsafe);
    }
    let mut file = open_for_read(path)?;
    if !file
        .metadata()
        .map_err(TransitionError::Unavailable)?
        .is_file()
    {
        return Err(TransitionError::Unsafe);
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(TransitionError::Unavailable)?;
    Ok(Some(bytes))
}

fn validate_replaceable(path: &Path) -> Result<(), TransitionError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(TransitionError::Unavailable(error)),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(TransitionError::Unsafe);
    }
    let mut file = open_for_read(path)?;
    let metadata = file.metadata().map_err(TransitionError::Unavailable)?;
    if !metadata.is_file() || !metadata_is_replaceable(&metadata) {
        return Err(TransitionError::Unsafe);
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(TransitionError::Unavailable)?;
    if let Ok(value) = serde_json::from_slice::<Value>(&bytes)
        && matches!(
            parse_transition_document(&value),
            Err(TransitionError::Incompatible)
        )
    {
        return Err(TransitionError::Incompatible);
    }
    Ok(())
}

fn remove_replaceable(path: &Path) -> Result<(), TransitionError> {
    validate_replaceable(path)?;
    fs::remove_file(path).map_err(TransitionError::Unavailable)
}

fn open_for_read(path: &Path) -> Result<File, TransitionError> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    options.open(path).map_err(|error| {
        if error.raw_os_error() == Some(libc_no_follow_error()) {
            TransitionError::Unsafe
        } else {
            TransitionError::Unavailable(error)
        }
    })
}

#[cfg(unix)]
fn libc_no_follow_error() -> i32 {
    libc::ELOOP
}

#[cfg(not(unix))]
fn libc_no_follow_error() -> i32 {
    i32::MIN
}

#[cfg(unix)]
fn metadata_is_replaceable(metadata: &fs::Metadata) -> bool {
    metadata_is_replaceable_by(metadata, unsafe { libc::geteuid() })
}

#[cfg(unix)]
fn metadata_is_replaceable_by(metadata: &fs::Metadata, effective_uid: u32) -> bool {
    metadata.uid() == effective_uid && metadata.mode() & 0o200 != 0
}

#[cfg(not(unix))]
fn metadata_is_replaceable(metadata: &fs::Metadata) -> bool {
    !metadata.permissions().readonly()
}

fn create_private_sibling(path: &Path) -> Result<(PathBuf, File), TransitionError> {
    for _ in 0..128 {
        let temporary_path = unique_sibling(path)?;
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        match options.open(&temporary_path) {
            Ok(file) => {
                #[cfg(unix)]
                if let Err(error) = file.set_permissions(fs::Permissions::from_mode(0o600)) {
                    drop(file);
                    fs::remove_file(&temporary_path).ok();
                    return Err(TransitionError::Unavailable(error));
                }
                return Ok((temporary_path, file));
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(TransitionError::Unavailable(error)),
        }
    }
    Err(TransitionError::Unavailable(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not create a unique transition temporary file",
    )))
}

fn unique_sibling(path: &Path) -> Result<PathBuf, TransitionError> {
    let file_name = path.file_name().ok_or_else(|| {
        TransitionError::Unavailable(io::Error::new(
            io::ErrorKind::InvalidInput,
            "transition path has no file name",
        ))
    })?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let mut name = OsString::from(file_name);
    name.push(format!(
        ".{}-{timestamp}-{sequence}.tmp",
        std::process::id()
    ));
    Ok(parent_directory(path).join(name))
}

fn parent_directory(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

#[cfg(unix)]
fn home_directory() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

#[cfg(all(test, unix))]
mod tests {
    use super::metadata_is_replaceable_by;
    use std::fs;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    #[test]
    fn foreign_owned_and_read_only_metadata_is_not_replaceable() {
        let path = std::env::temp_dir().join(format!(
            "herdr-quota-transition-metadata-test-{}",
            std::process::id()
        ));
        fs::write(&path, b"state").expect("write metadata fixture");
        let metadata = fs::metadata(&path).expect("read metadata");
        assert!(metadata_is_replaceable_by(&metadata, metadata.uid()));
        assert!(!metadata_is_replaceable_by(
            &metadata,
            metadata.uid().wrapping_add(1)
        ));
        fs::set_permissions(&path, fs::Permissions::from_mode(0o400))
            .expect("make fixture read only");
        let metadata = fs::metadata(&path).expect("read read-only metadata");
        assert!(!metadata_is_replaceable_by(&metadata, metadata.uid()));
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .expect("restore fixture mode");
        fs::remove_file(path).expect("remove metadata fixture");
    }
}

#[cfg(windows)]
fn home_directory() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

#[cfg(not(any(unix, windows)))]
fn home_directory() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}
