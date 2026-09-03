//! Closed quota-history facts and conservative change evidence.

use std::sync::LazyLock;
use std::time::SystemTime;

use regex::Regex;
use serde::Serialize;
use time::format_description::well_known::Rfc3339;
use time::macros::format_description;
use time::{OffsetDateTime, PrimitiveDateTime, UtcOffset};

use super::provider::{
    EffectiveAvailability, EffectiveStatus, MarketedProvider, PaceStatus, ProjectionConfidence,
    ProviderQuota, ProviderStatus, RunwayStatus, SemanticsStatus,
};
use super::schema::QuotaReport;

/// The persisted quota-history schema version.
pub const HISTORY_SCHEMA_VERSION: u8 = 2;
/// The largest number of facts retained for one provider in one sample.
pub const HISTORY_MAX_FACTS_PER_PROVIDER: usize = 8;

const MEANINGFUL_REMAINING_CHANGE: f64 = 10.0;
const MEANINGFUL_RESERVE_CHANGE: f64 = 10.0;
const MEANINGFUL_PROJECTION_CHANGE_MILLIS: i128 = 2 * 60 * 60 * 1_000;
const HISTORY_TIMESTAMP_FORMAT: &[time::format_description::FormatItem<'static>] =
    format_description!("[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z");

static MODEL_ID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^model:([A-Za-z0-9][A-Za-z0-9_-]{0,39})(?::(5h|7d)(?:_\d+)?)?$")
        .expect("the model history pattern is static")
});
static NUMBERED_LIMIT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^limit:(\d{1,3})$").expect("the limit history pattern is static")
});

/// A provider identity permitted in quota history.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HistoryProviderName(MarketedProvider);

impl HistoryProviderName {
    /// Build the history identity for a marketed provider.
    pub const fn new(provider: MarketedProvider) -> Self {
        Self(provider)
    }

    /// Resolve an exact persisted product label.
    pub fn from_label(label: &str) -> Option<Self> {
        MarketedProvider::ALL
            .into_iter()
            .find(|provider| provider.label() == label)
            .map(Self)
    }

    /// Return the provider catalog entry.
    pub const fn marketed(self) -> MarketedProvider {
        self.0
    }

    /// Return the persisted product label.
    pub const fn label(self) -> &'static str {
        self.0.label()
    }

    pub(crate) fn order(self) -> usize {
        MarketedProvider::ALL
            .iter()
            .position(|provider| *provider == self.0)
            .expect("a history provider always belongs to the catalog")
    }
}

impl Serialize for HistoryProviderName {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.label())
    }
}

/// Finite health state retained to prevent evidence across gaps.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryDataHealth {
    Current,
    Stale,
    Unavailable,
    Error,
    Unknown,
}

/// An authoritative pace state suitable for history.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryPaceState {
    Ahead,
    OnPace,
    Behind,
    Mixed,
}

/// Bounded pace evidence for a quota fact.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HistoryPaceFact {
    pub state: HistoryPaceState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reserve: Option<f64>,
}

/// An authoritative runway state suitable for history.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryRunwayState {
    ExhaustedNow,
    ProjectedExhaustion,
    ThroughReset,
}

/// Bounded runway evidence for a quota fact.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryRunwayFact {
    pub state: HistoryRunwayState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projected_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<ProjectionConfidence>,
}

/// One sanitized effective-quota fact.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryFact {
    pub scope: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<String>,
    pub remaining: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pace: Option<HistoryPaceFact>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runway: Option<HistoryRunwayFact>,
}

/// One marketed provider at a collection instant.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryProviderSnapshot {
    pub provider: HistoryProviderName,
    pub data_health: HistoryDataHealth,
    pub auth_eligible: bool,
    pub facts: Vec<HistoryFact>,
}

/// A bounded, sanitized collection sample.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistorySnapshot {
    pub captured_at: String,
    pub providers: Vec<HistoryProviderSnapshot>,
}

/// The complete local quota-history document.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryDocument {
    pub schema_version: u8,
    pub snapshots: Vec<HistorySnapshot>,
}

impl Default for HistoryDocument {
    fn default() -> Self {
        Self {
            schema_version: HISTORY_SCHEMA_VERSION,
            snapshots: Vec::new(),
        }
    }
}

/// Finite history status for the dashboard.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HistoryAvailability {
    Ready,
    FirstRun,
    Recovered,
    Incompatible,
    Unavailable,
    ClockSkew,
    NoUsableData,
}

/// A conservative material change kind.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HistoryEvidenceKind {
    Reset,
    RemainingDrop,
    RemainingGain,
    PaceWorse,
    PaceBetter,
    ProjectionEarlier,
    ProjectionLater,
}

/// The single highest-priority material change for the dashboard.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryEvidence {
    pub kind: HistoryEvidenceKind,
    pub provider: HistoryProviderName,
    pub scope: String,
    pub limit: Option<String>,
    /// Rounded percentage points or projection seconds, depending on `kind`.
    pub amount: Option<i64>,
}

/// Dashboard-facing history state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryView {
    pub availability: HistoryAvailability,
    pub evidence: Option<HistoryEvidence>,
}

/// A compact selected-provider trace derived only from retained safe facts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryTrend {
    /// Six to eight one-cell samples, oldest to newest. `·` is an unsafe gap.
    pub cells: String,
    /// The material consequence selected by the existing evidence ranking.
    pub evidence: HistoryEvidence,
    /// Time between the two samples that produced `evidence`.
    pub elapsed_seconds: i64,
}

pub(crate) fn timestamp_millis(value: &str) -> Option<i128> {
    PrimitiveDateTime::parse(value, HISTORY_TIMESTAMP_FORMAT)
        .ok()
        .map(|timestamp| timestamp.assume_utc().unix_timestamp_nanos() / 1_000_000)
}

pub(crate) fn canonical_history_timestamp(now: SystemTime) -> Option<String> {
    OffsetDateTime::from(now)
        .to_offset(UtcOffset::UTC)
        .format(HISTORY_TIMESTAMP_FORMAT)
        .ok()
}

fn canonical_upstream_timestamp(value: &str) -> Option<String> {
    OffsetDateTime::parse(value, &Rfc3339)
        .ok()?
        .to_offset(UtcOffset::UTC)
        .format(HISTORY_TIMESTAMP_FORMAT)
        .ok()
}

pub(crate) fn safe_identity(value: &str) -> bool {
    if value.is_empty() || value.len() > 32 || !value.is_ascii() {
        return false;
    }
    let mut bytes = value.bytes();
    if !bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphanumeric())
        || !bytes.all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b' ' | b'.' | b'_' | b'+' | b'-')
        })
    {
        return false;
    }
    let lower = value.to_ascii_lowercase();
    ![
        "bearer",
        "secret",
        "token",
        "credential",
        "account",
        "auth",
        "apikey",
        "api-key",
        "api_key",
    ]
    .iter()
    .any(|word| lower.contains(word))
}

fn provider_health(provider: &ProviderQuota) -> (HistoryDataHealth, bool) {
    let auth_eligible = provider.state.status != ProviderStatus::AuthRequired
        && !matches!(
            provider.state.auth_status.as_deref(),
            Some("unusable" | "expired_refreshable")
        );
    if !auth_eligible {
        return (HistoryDataHealth::Unavailable, false);
    }
    if provider.state.stale || provider.state.status == ProviderStatus::Stale {
        return (HistoryDataHealth::Stale, true);
    }
    let health = match provider.state.status {
        ProviderStatus::Fresh => HistoryDataHealth::Current,
        ProviderStatus::Error => HistoryDataHealth::Error,
        ProviderStatus::Unavailable | ProviderStatus::RateLimited => HistoryDataHealth::Unavailable,
        ProviderStatus::AuthRequired => HistoryDataHealth::Unknown,
        ProviderStatus::Stale => HistoryDataHealth::Stale,
    };
    (health, true)
}

fn model_identity(id: &str) -> Option<(String, Option<&str>)> {
    let captures = MODEL_ID.captures(id)?;
    let slug = captures
        .get(1)?
        .as_str()
        .strip_prefix("codex_")
        .unwrap_or(captures.get(1)?.as_str());
    let mut at_word_boundary = true;
    let model = slug
        .chars()
        .map(|character| {
            let character = if character == '_' { ' ' } else { character };
            let output = if at_word_boundary {
                character.to_ascii_uppercase()
            } else {
                character
            };
            at_word_boundary = !character.is_ascii_alphanumeric();
            output
        })
        .collect::<String>();
    safe_identity(&model).then(|| (model, captures.get(2).map(|value| value.as_str())))
}

fn safe_scope_label(effective: &EffectiveAvailability) -> Option<String> {
    match effective.scope.as_str() {
        "all_models" => Some("All models".to_owned()),
        "all_products" => Some("All products".to_owned()),
        scope => model_identity(scope).map(|(model, _)| model),
    }
}

fn safe_limit_identity(provider: MarketedProvider, id: &str) -> Option<String> {
    let fixed = match (provider, id) {
        (MarketedProvider::Claude, "five_hour") => Some("Session"),
        (MarketedProvider::Claude, "seven_day") => Some("Week"),
        (MarketedProvider::Claude, "seven_day_opus") => Some("Opus"),
        (MarketedProvider::Claude, "extra_usage") => Some("Extra"),
        (MarketedProvider::Codex, "five_hour") => Some("Session"),
        (MarketedProvider::Codex, "weekly") => Some("Week"),
        (MarketedProvider::Cursor, "included_usage") => Some("Included"),
        (MarketedProvider::Cursor, "auto_usage") => Some("Auto"),
        (MarketedProvider::Cursor, "api_usage") => Some("3rd-party"),
        (MarketedProvider::Cursor, "spend_limit") => Some("Spend"),
        (MarketedProvider::Kimi, "five_hour") => Some("Session"),
        (MarketedProvider::Kimi, "weekly") => Some("Week"),
        (MarketedProvider::Grok, "credits") => Some("Consumer quota"),
        (MarketedProvider::Copilot, "chat") => Some("Chat"),
        (MarketedProvider::Copilot, "completions") => Some("Completions"),
        (MarketedProvider::Copilot, "premium_interactions") => Some("Premium"),
        _ => None,
    };
    if let Some(label) = fixed {
        return Some(label.to_owned());
    }
    if id.starts_with("code_review_five_hour") {
        return Some("Review 5h".to_owned());
    }
    if id.starts_with("code_review_weekly") {
        return Some("Review wk".to_owned());
    }
    if let Some((model, period)) = model_identity(id) {
        return Some(match period {
            Some("5h") => format!("{model} 5h"),
            Some("7d") => format!("{model} week"),
            _ => model,
        });
    }
    NUMBERED_LIMIT
        .captures(id)
        .and_then(|captures| captures.get(1))
        .map(|number| format!("Limit {}", number.as_str()))
}

fn safe_limit(
    provider: &ProviderQuota,
    marketed: MarketedProvider,
    effective: &EffectiveAvailability,
) -> (Option<String>, Option<String>) {
    let id = effective
        .runway
        .as_ref()
        .and_then(|runway| runway.limiting_window_id.as_deref())
        .or_else(|| {
            effective
                .pace
                .as_ref()
                .and_then(|pace| pace.worst_reserve_window_id.as_deref())
        })
        .or_else(|| effective.limiting_window_ids.first().map(String::as_str));
    let Some(id) = id else {
        return (None, None);
    };
    let reset_at = provider
        .windows
        .iter()
        .find(|window| window.id == id)
        .and_then(|window| window.resets_at.as_deref())
        .and_then(canonical_upstream_timestamp);
    (safe_limit_identity(marketed, id), reset_at)
}

fn normalize_pace(effective: &EffectiveAvailability) -> Option<HistoryPaceFact> {
    let pace = effective.pace.as_ref()?;
    let state = match pace.status {
        PaceStatus::Ahead => HistoryPaceState::Ahead,
        PaceStatus::OnPace => HistoryPaceState::OnPace,
        PaceStatus::Behind => HistoryPaceState::Behind,
        PaceStatus::Mixed => HistoryPaceState::Mixed,
        PaceStatus::Unknown => return None,
    };
    let reserve = pace
        .worst_reserve_percent_points
        .filter(|value| value.is_finite() && (-10_000.0..=10_000.0).contains(value));
    Some(HistoryPaceFact { state, reserve })
}

fn normalize_runway(effective: &EffectiveAvailability) -> Option<HistoryRunwayFact> {
    let runway = effective.runway.as_ref()?;
    let state = match runway.status {
        RunwayStatus::ExhaustedNow => HistoryRunwayState::ExhaustedNow,
        RunwayStatus::ProjectedExhaustion => HistoryRunwayState::ProjectedExhaustion,
        RunwayStatus::ThroughReset => HistoryRunwayState::ThroughReset,
        RunwayStatus::Unknown => return None,
    };
    Some(HistoryRunwayFact {
        state,
        projected_at: runway
            .projected_exhausted_at
            .as_deref()
            .and_then(canonical_upstream_timestamp),
        confidence: runway.projection_confidence,
    })
}

fn normalize_provider(provider: &ProviderQuota) -> Option<HistoryProviderSnapshot> {
    let marketed = MarketedProvider::from_id(&provider.provider.to_ascii_lowercase())?;
    let (data_health, auth_eligible) = provider_health(provider);
    let mut facts = Vec::new();
    if data_health == HistoryDataHealth::Current
        && auth_eligible
        && provider.semantics_status != Some(SemanticsStatus::Unknown)
    {
        for effective in &provider.effective {
            if facts.len() >= HISTORY_MAX_FACTS_PER_PROVIDER
                || effective.status != EffectiveStatus::Known
            {
                continue;
            }
            let Some(remaining) = effective
                .effective_percent_remaining
                .filter(|value| value.is_finite() && (0.0..=100.0).contains(value))
            else {
                continue;
            };
            let Some(scope) = safe_scope_label(effective) else {
                continue;
            };
            let (limit, reset_at) = safe_limit(provider, marketed, effective);
            facts.push(HistoryFact {
                scope,
                limit,
                remaining,
                reset_at,
                pace: normalize_pace(effective),
                runway: normalize_runway(effective),
            });
        }
    }
    facts.sort_by(|left, right| left.scope.cmp(&right.scope));
    Some(HistoryProviderSnapshot {
        provider: HistoryProviderName::new(marketed),
        data_health,
        auth_eligible,
        facts,
    })
}

/// Normalize one report into the finite history allow-list.
pub fn normalize_history_snapshot(
    report: &QuotaReport,
    now: SystemTime,
) -> Option<HistorySnapshot> {
    let captured_at = canonical_history_timestamp(now)?;
    let mut providers = report
        .providers
        .iter()
        .filter_map(normalize_provider)
        .collect::<Vec<_>>();
    providers.sort_by_key(|provider| provider.provider.order());
    providers
        .iter()
        .any(|provider| !provider.facts.is_empty())
        .then_some(HistorySnapshot {
            captured_at,
            providers,
        })
}

fn fact_in<'a>(
    snapshot: &'a HistorySnapshot,
    provider: HistoryProviderName,
    scope: &str,
) -> Option<&'a HistoryFact> {
    let provider = snapshot
        .providers
        .iter()
        .find(|item| item.provider == provider)?;
    if provider.data_health != HistoryDataHealth::Current || !provider.auth_eligible {
        return None;
    }
    provider.facts.iter().find(|fact| fact.scope == scope)
}

fn evidence(
    kind: HistoryEvidenceKind,
    provider: &HistoryProviderSnapshot,
    fact: &HistoryFact,
    amount: Option<f64>,
) -> HistoryEvidence {
    HistoryEvidence {
        kind,
        provider: provider.provider,
        scope: fact.scope.clone(),
        limit: fact.limit.clone(),
        amount: amount.map(|value| value.round() as i64),
    }
}

fn reset_evidence(previous: &HistoryFact, current: &HistoryFact) -> bool {
    previous.reset_at.is_some()
        && current.reset_at.is_some()
        && previous.reset_at != current.reset_at
        && current.remaining - previous.remaining >= MEANINGFUL_REMAINING_CHANGE
}

fn pace_rank(state: HistoryPaceState) -> u8 {
    match state {
        HistoryPaceState::Behind => 0,
        HistoryPaceState::OnPace => 1,
        HistoryPaceState::Mixed => 2,
        HistoryPaceState::Ahead => 3,
    }
}

fn pace_change(
    previous: Option<&HistoryPaceFact>,
    current: Option<&HistoryPaceFact>,
) -> Option<bool> {
    let (previous, current) = (previous?, current?);
    if pace_rank(current.state) > pace_rank(previous.state) {
        return Some(true);
    }
    if pace_rank(current.state) < pace_rank(previous.state) {
        return Some(false);
    }
    let change = current.reserve? - previous.reserve?;
    if change <= -MEANINGFUL_RESERVE_CHANGE {
        Some(true)
    } else if change >= MEANINGFUL_RESERVE_CHANGE {
        Some(false)
    } else {
        None
    }
}

fn established_projection(runway: Option<&HistoryRunwayFact>) -> Option<i128> {
    let runway = runway?;
    (runway.state == HistoryRunwayState::ProjectedExhaustion
        && runway.confidence == Some(ProjectionConfidence::Established))
    .then_some(())?;
    timestamp_millis(runway.projected_at.as_deref()?)
}

fn projection_change(
    previous: Option<&HistoryRunwayFact>,
    current: Option<&HistoryRunwayFact>,
) -> Option<(bool, f64)> {
    let (previous, current) = (previous?, current?);
    if previous.state == HistoryRunwayState::ThroughReset
        && current.state == HistoryRunwayState::ProjectedExhaustion
        && current.confidence == Some(ProjectionConfidence::Established)
    {
        return Some((true, 0.0));
    }
    if previous.state == HistoryRunwayState::ProjectedExhaustion
        && previous.confidence == Some(ProjectionConfidence::Established)
        && current.state == HistoryRunwayState::ThroughReset
    {
        return Some((false, 0.0));
    }
    let difference =
        established_projection(Some(current))? - established_projection(Some(previous))?;
    if difference.abs() < MEANINGFUL_PROJECTION_CHANGE_MILLIS {
        return None;
    }
    Some((difference < 0, difference.abs() as f64 / 1_000.0))
}

fn evidence_for_fact(
    snapshots: &[HistorySnapshot],
    provider: &HistoryProviderSnapshot,
    current: &HistoryFact,
) -> Option<(u8, HistoryEvidence)> {
    let previous = fact_in(
        snapshots.get(snapshots.len().checked_sub(2)?)?,
        provider.provider,
        &current.scope,
    )?;
    if reset_evidence(previous, current) {
        return Some((
            0,
            evidence(HistoryEvidenceKind::Reset, provider, current, None),
        ));
    }
    if previous.reset_at != current.reset_at {
        return None;
    }
    let pace = pace_change(previous.pace.as_ref(), current.pace.as_ref());
    if pace == Some(true) {
        return Some((
            1,
            evidence(HistoryEvidenceKind::PaceWorse, provider, current, None),
        ));
    }
    let projection = projection_change(previous.runway.as_ref(), current.runway.as_ref());
    if let Some((true, amount)) = projection {
        return Some((
            2,
            evidence(
                HistoryEvidenceKind::ProjectionEarlier,
                provider,
                current,
                Some(amount),
            ),
        ));
    }
    let drop = previous.remaining - current.remaining;
    if drop >= MEANINGFUL_REMAINING_CHANGE {
        return Some((
            3,
            evidence(
                HistoryEvidenceKind::RemainingDrop,
                provider,
                current,
                Some(drop),
            ),
        ));
    }
    let gain = current.remaining - previous.remaining;
    if gain >= MEANINGFUL_REMAINING_CHANGE {
        return Some((
            4,
            evidence(
                HistoryEvidenceKind::RemainingGain,
                provider,
                current,
                Some(gain),
            ),
        ));
    }
    if pace == Some(false) {
        return Some((
            5,
            evidence(HistoryEvidenceKind::PaceBetter, provider, current, None),
        ));
    }
    if let Some((false, amount)) = projection {
        return Some((
            6,
            evidence(
                HistoryEvidenceKind::ProjectionLater,
                provider,
                current,
                Some(amount),
            ),
        ));
    }
    None
}

fn trend_cell(remaining: f64) -> char {
    const LEVELS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let index =
        ((remaining.clamp(0.0, 100.0) / 100.0) * (LEVELS.len() - 1) as f64).round() as usize;
    LEVELS[index]
}

/// Build one bounded material trend for a selected provider.
///
/// This does not create another evidence policy: it reuses the same immediate
/// comparison, materiality thresholds, and ranking as [`history_view`]. Older
/// samples only shape the trace, and unsafe or different-cycle samples become
/// visible gaps rather than being joined or estimated.
pub fn history_trend(
    document: &HistoryDocument,
    provider: MarketedProvider,
) -> Option<HistoryTrend> {
    const MIN_CELLS: usize = 6;
    const MAX_CELLS: usize = 8;

    if document.schema_version != HISTORY_SCHEMA_VERSION || document.snapshots.len() < MIN_CELLS {
        return None;
    }
    let current_snapshot = document.snapshots.last()?;
    let current_provider = current_snapshot.providers.iter().find(|item| {
        item.provider.marketed() == provider
            && item.data_health == HistoryDataHealth::Current
            && item.auth_eligible
    })?;
    let (_, evidence, current_fact) = current_provider
        .facts
        .iter()
        .filter_map(|fact| {
            evidence_for_fact(&document.snapshots, current_provider, fact)
                .map(|(rank, evidence)| (rank, evidence, fact))
        })
        .min_by_key(|(rank, _, _)| *rank)?;
    let current_reset = current_fact.reset_at.as_deref()?;
    let start = document.snapshots.len().saturating_sub(MAX_CELLS);
    let cells = document.snapshots[start..]
        .iter()
        .map(|snapshot| {
            fact_in(snapshot, current_provider.provider, &current_fact.scope)
                .filter(|fact| fact.reset_at.as_deref() == Some(current_reset))
                .map_or('·', |fact| trend_cell(fact.remaining))
        })
        .collect::<String>();
    let previous_at =
        timestamp_millis(&document.snapshots[document.snapshots.len() - 2].captured_at)?;
    let current_at = timestamp_millis(&current_snapshot.captured_at)?;
    let elapsed_seconds = i64::try_from((current_at - previous_at) / 1_000)
        .ok()
        .filter(|seconds| *seconds > 0)?;

    Some(HistoryTrend {
        cells,
        evidence,
        elapsed_seconds,
    })
}

/// Select the highest-priority material change from the latest two samples.
pub fn history_view(document: &HistoryDocument, availability: HistoryAvailability) -> HistoryView {
    let Some(current) = document.snapshots.last() else {
        return HistoryView {
            availability: if availability == HistoryAvailability::Ready {
                HistoryAvailability::FirstRun
            } else {
                availability
            },
            evidence: None,
        };
    };
    let mut selected: Option<(u8, HistoryEvidence)> = None;
    for provider in &current.providers {
        if provider.data_health != HistoryDataHealth::Current || !provider.auth_eligible {
            continue;
        }
        for fact in &provider.facts {
            let Some(candidate) = evidence_for_fact(&document.snapshots, provider, fact) else {
                continue;
            };
            if selected
                .as_ref()
                .is_none_or(|selected| candidate.0 < selected.0)
            {
                selected = Some(candidate);
            }
        }
    }
    HistoryView {
        availability: if availability == HistoryAvailability::Ready && document.snapshots.len() < 2
        {
            HistoryAvailability::FirstRun
        } else {
            availability
        },
        evidence: selected.map(|(_, evidence)| evidence),
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use super::*;
    use crate::domain::provider::{EffectivePace, ProviderState, QuotaWindow, Runway};

    fn history_time(value: &str) -> SystemTime {
        let millis = timestamp_millis(value).expect("test timestamp must parse");
        UNIX_EPOCH + Duration::from_millis(millis as u64)
    }

    fn provider(id: &str, status: ProviderStatus) -> ProviderQuota {
        ProviderQuota {
            provider: id.to_owned(),
            label: Some("private@example.com".to_owned()),
            source: Some("/private/token".to_owned()),
            plan: Some("Bearer secret".to_owned()),
            windows: vec![QuotaWindow {
                id: "weekly".to_owned(),
                label: "private@example.com".to_owned(),
                kind: "weekly".to_owned(),
                percent_used: None,
                percent_remaining: Some(60.0),
                starts_at: None,
                resets_at: Some("2026-09-08T12:00:00Z".to_owned()),
                reset_text: None,
                window_seconds: None,
                spent_usd: None,
                limit_usd: None,
                pace: None,
            }],
            effective: vec![EffectiveAvailability {
                scope: "all_models".to_owned(),
                status: EffectiveStatus::Known,
                effective_percent_remaining: Some(60.0),
                bounded_by: vec!["weekly".to_owned()],
                limiting_window_ids: vec!["weekly".to_owned()],
                pace: Some(EffectivePace {
                    status: PaceStatus::OnPace,
                    worst_reserve_percent_points: Some(5.0),
                    worst_reserve_window_id: Some("weekly".to_owned()),
                    unknown_window_ids: Vec::new(),
                }),
                runway: Some(Runway {
                    status: RunwayStatus::ThroughReset,
                    usable_runway_seconds: None,
                    projected_exhausted_at: None,
                    limiting_window_id: Some("weekly".to_owned()),
                    projection_confidence: None,
                    unmeasurable_window_ids: Vec::new(),
                }),
            }],
            semantics_status: Some(SemanticsStatus::Known),
            credits: None,
            state: ProviderState {
                status,
                stale: status == ProviderStatus::Stale,
                refreshed_at: None,
                auth_status: Some("usable".to_owned()),
                reason: Some("private reason".to_owned()),
                remedy_command: Some("read /private/auth".to_owned()),
                error_code: None,
            },
        }
    }

    fn report(providers: Vec<ProviderQuota>) -> QuotaReport {
        QuotaReport {
            generated_at: "2026-09-02T12:00:00Z".to_owned(),
            schema_version: 5,
            providers,
            adaptation_warnings: Vec::new(),
        }
    }

    fn fact(remaining: f64) -> HistoryFact {
        HistoryFact {
            scope: "All models".to_owned(),
            limit: Some("Week".to_owned()),
            remaining,
            reset_at: Some("2026-09-08T12:00:00.000Z".to_owned()),
            pace: Some(HistoryPaceFact {
                state: HistoryPaceState::OnPace,
                reserve: Some(5.0),
            }),
            runway: Some(HistoryRunwayFact {
                state: HistoryRunwayState::ThroughReset,
                projected_at: None,
                confidence: None,
            }),
        }
    }

    fn snapshot(at: &str, facts: Vec<HistoryFact>) -> HistorySnapshot {
        HistorySnapshot {
            captured_at: at.to_owned(),
            providers: vec![HistoryProviderSnapshot {
                provider: HistoryProviderName::new(MarketedProvider::Claude),
                data_health: HistoryDataHealth::Current,
                auth_eligible: true,
                facts,
            }],
        }
    }

    #[test]
    fn eligibility_keeps_only_finite_catalog_facts() {
        let mut providers = MarketedProvider::ALL
            .into_iter()
            .map(|provider_id| provider(provider_id.id(), ProviderStatus::Fresh))
            .collect::<Vec<_>>();
        providers.push(provider("future-provider", ProviderStatus::Fresh));
        providers[1].state.status = ProviderStatus::AuthRequired;
        providers[2].state.status = ProviderStatus::Stale;
        providers[2].state.stale = true;
        providers[3].semantics_status = Some(SemanticsStatus::Unknown);
        let snapshot = normalize_history_snapshot(
            &report(providers),
            history_time("2026-09-02T12:00:00.000Z"),
        )
        .expect("healthy catalog providers provide usable data");

        assert_eq!(snapshot.providers.len(), 6);
        assert!(snapshot.providers[0].facts.len() == 1);
        assert!(snapshot.providers[1].facts.is_empty());
        assert!(snapshot.providers[2].facts.is_empty());
        assert!(snapshot.providers[3].facts.is_empty());
        let persisted = serde_json::to_string(&snapshot).expect("history must serialize");
        assert!(!persisted.contains("private@example.com"));
        assert!(!persisted.contains("Bearer"));
        assert!(!persisted.contains("/private"));

        let unusable = report(
            MarketedProvider::ALL
                .into_iter()
                .map(|provider_id| provider(provider_id.id(), ProviderStatus::Unavailable))
                .collect(),
        );
        assert!(
            normalize_history_snapshot(&unusable, history_time("2026-09-02T12:00:00.000Z"))
                .is_none()
        );
    }

    #[test]
    fn normalization_caps_each_provider_at_eight_facts() {
        let mut claude = provider("claude", ProviderStatus::Fresh);
        claude.effective = (0..12)
            .map(|index| {
                let mut effective = claude.effective[0].clone();
                effective.scope = format!("model:m{index}");
                effective
            })
            .collect();
        let snapshot = normalize_history_snapshot(
            &report(vec![claude]),
            history_time("2026-09-02T12:00:00.000Z"),
        )
        .expect("the report has usable data");
        assert_eq!(snapshot.providers[0].facts.len(), 8);
    }

    #[test]
    fn materiality_ignores_noise_and_requires_established_projection() {
        let first = snapshot("2026-09-02T12:00:00.000Z", vec![fact(60.0)]);
        let mut noisy = fact(50.1);
        noisy.pace.as_mut().unwrap().reserve = Some(-4.9);
        noisy.runway = Some(HistoryRunwayFact {
            state: HistoryRunwayState::ProjectedExhaustion,
            projected_at: Some("2026-09-02T15:00:00.000Z".to_owned()),
            confidence: Some(ProjectionConfidence::Early),
        });
        let document = HistoryDocument {
            schema_version: HISTORY_SCHEMA_VERSION,
            snapshots: vec![first, snapshot("2026-09-02T12:05:00.000Z", vec![noisy])],
        };
        assert_eq!(
            history_view(&document, HistoryAvailability::Ready).evidence,
            None
        );

        let mut material = fact(50.0);
        material.pace.as_mut().unwrap().reserve = Some(-5.0);
        let document = HistoryDocument {
            schema_version: HISTORY_SCHEMA_VERSION,
            snapshots: vec![
                snapshot("2026-09-02T12:00:00.000Z", vec![fact(60.0)]),
                snapshot("2026-09-02T12:05:00.000Z", vec![material]),
            ],
        };
        assert_eq!(
            history_view(&document, HistoryAvailability::Ready)
                .evidence
                .map(|evidence| evidence.kind),
            Some(HistoryEvidenceKind::PaceWorse)
        );
    }

    #[test]
    fn ranking_prefers_reset_then_worse_pace_then_projection_then_capacity() {
        let mut previous_reset = fact(5.0);
        previous_reset.scope = "Reset scope".to_owned();
        let mut current_reset = fact(95.0);
        current_reset.scope = "Reset scope".to_owned();
        current_reset.reset_at = Some("2026-09-09T12:00:00.000Z".to_owned());

        let mut previous_pace = fact(60.0);
        previous_pace.scope = "Pace scope".to_owned();
        let mut current_pace = fact(59.0);
        current_pace.scope = "Pace scope".to_owned();
        current_pace.pace.as_mut().unwrap().state = HistoryPaceState::Ahead;

        let mut previous_drop = fact(70.0);
        previous_drop.scope = "Drop scope".to_owned();
        let mut current_drop = fact(40.0);
        current_drop.scope = "Drop scope".to_owned();

        let mut previous_projection = fact(60.0);
        previous_projection.scope = "Projection scope".to_owned();
        previous_projection.runway = Some(HistoryRunwayFact {
            state: HistoryRunwayState::ProjectedExhaustion,
            projected_at: Some("2026-09-03T02:00:00.000Z".to_owned()),
            confidence: Some(ProjectionConfidence::Established),
        });
        let mut current_projection = fact(59.0);
        current_projection.scope = "Projection scope".to_owned();
        current_projection.runway = Some(HistoryRunwayFact {
            state: HistoryRunwayState::ProjectedExhaustion,
            projected_at: Some("2026-09-02T21:00:00.000Z".to_owned()),
            confidence: Some(ProjectionConfidence::Established),
        });

        let ranked = |previous: Vec<HistoryFact>, current: Vec<HistoryFact>| HistoryDocument {
            schema_version: HISTORY_SCHEMA_VERSION,
            snapshots: vec![
                snapshot("2026-09-02T12:00:00.000Z", previous),
                snapshot("2026-09-02T12:05:00.000Z", current),
            ],
        };
        let select = |document: &HistoryDocument| {
            history_view(document, HistoryAvailability::Ready)
                .evidence
                .expect("a material change must be selected")
        };
        let document = ranked(
            vec![
                previous_drop.clone(),
                previous_projection.clone(),
                previous_pace.clone(),
                previous_reset,
            ],
            vec![
                current_drop.clone(),
                current_projection.clone(),
                current_pace.clone(),
                current_reset,
            ],
        );
        let selected = select(&document);
        assert_eq!(selected.kind, HistoryEvidenceKind::Reset);
        assert_eq!(selected.scope, "Reset scope");

        let document = ranked(
            vec![
                previous_drop.clone(),
                previous_projection.clone(),
                previous_pace,
            ],
            vec![
                current_drop.clone(),
                current_projection.clone(),
                current_pace,
            ],
        );
        assert_eq!(select(&document).kind, HistoryEvidenceKind::PaceWorse);

        let document = ranked(
            vec![previous_drop.clone(), previous_projection],
            vec![current_drop.clone(), current_projection],
        );
        assert_eq!(
            select(&document).kind,
            HistoryEvidenceKind::ProjectionEarlier
        );

        let document = ranked(vec![previous_drop], vec![current_drop]);
        assert_eq!(select(&document).kind, HistoryEvidenceKind::RemainingDrop);
    }

    #[test]
    fn immediate_gap_suppresses_old_evidence() {
        let mut gap = snapshot("2026-09-02T12:05:00.000Z", Vec::new());
        gap.providers[0].data_health = HistoryDataHealth::Unavailable;
        gap.providers[0].auth_eligible = false;
        let document = HistoryDocument {
            schema_version: HISTORY_SCHEMA_VERSION,
            snapshots: vec![
                snapshot("2026-09-02T12:00:00.000Z", vec![fact(70.0)]),
                gap,
                snapshot("2026-09-02T12:10:00.000Z", vec![fact(40.0)]),
            ],
        };
        assert_eq!(
            history_view(&document, HistoryAvailability::Ready).evidence,
            None
        );
    }

    #[test]
    fn selected_provider_trend_is_bounded_same_cycle_and_gap_preserving() {
        let reset = "2026-09-09T12:00:00.000Z";
        let mut snapshots = (0..8)
            .map(|index| {
                let mut item = fact(82.0 - index as f64 * 3.0);
                item.reset_at = Some(reset.to_owned());
                snapshot(
                    &format!("2026-09-02T12:{:02}:00.000Z", index * 5),
                    vec![item],
                )
            })
            .collect::<Vec<_>>();
        snapshots[2].providers[0].data_health = HistoryDataHealth::Unavailable;
        snapshots[2].providers[0].auth_eligible = false;
        snapshots[2].providers[0].facts.clear();
        snapshots[4].providers[0].facts[0].reset_at = Some("2026-09-08T12:00:00.000Z".to_owned());
        snapshots[6].providers[0].facts[0].remaining = 64.0;
        snapshots[7].providers[0].facts[0].remaining = 46.0;
        let document = HistoryDocument {
            schema_version: HISTORY_SCHEMA_VERSION,
            snapshots,
        };

        let trend = history_trend(&document, MarketedProvider::Claude)
            .expect("the immediate material drop is eligible");

        assert_eq!(trend.cells.chars().count(), 8);
        assert_eq!(trend.cells.chars().nth(2), Some('·'));
        assert_eq!(trend.cells.chars().nth(4), Some('·'));
        assert_eq!(trend.elapsed_seconds, 5 * 60);
        assert_eq!(trend.evidence.kind, HistoryEvidenceKind::RemainingDrop);
        assert_eq!(trend.evidence.amount, Some(18));
        assert_eq!(
            history_view(&document, HistoryAvailability::Ready).evidence,
            Some(trend.evidence)
        );
    }

    #[test]
    fn selected_provider_trend_requires_known_cycle_identity() {
        let mut snapshots = (0..6)
            .map(|index| {
                snapshot(
                    &format!("2026-09-02T12:{:02}:00.000Z", index * 5),
                    vec![fact(if index == 5 { 40.0 } else { 60.0 })],
                )
            })
            .collect::<Vec<_>>();
        snapshots[2].providers[0].facts[0].reset_at = None;
        let mut document = HistoryDocument {
            schema_version: HISTORY_SCHEMA_VERSION,
            snapshots,
        };

        let trend = history_trend(&document, MarketedProvider::Claude)
            .expect("a missing historical reset becomes a gap");
        assert_eq!(trend.cells.chars().nth(2), Some('·'));

        document.snapshots.last_mut().unwrap().providers[0].facts[0].reset_at = None;
        assert_eq!(history_trend(&document, MarketedProvider::Claude), None);
    }

    #[test]
    fn selected_provider_trend_suppresses_short_or_non_current_history() {
        let snapshots = (0..6)
            .map(|index| {
                snapshot(
                    &format!("2026-09-02T12:{:02}:00.000Z", index * 5),
                    vec![fact(if index == 5 { 40.0 } else { 60.0 })],
                )
            })
            .collect::<Vec<_>>();
        let mut non_current = HistoryDocument {
            schema_version: HISTORY_SCHEMA_VERSION,
            snapshots,
        };
        let mut incompatible = non_current.clone();
        incompatible.schema_version += 1;
        assert_eq!(history_trend(&incompatible, MarketedProvider::Claude), None);

        non_current.snapshots.last_mut().unwrap().providers[0].data_health =
            HistoryDataHealth::Stale;
        non_current.snapshots.last_mut().unwrap().providers[0]
            .facts
            .clear();
        assert_eq!(history_trend(&non_current, MarketedProvider::Claude), None);

        non_current.snapshots.pop();
        assert_eq!(history_trend(&non_current, MarketedProvider::Claude), None);
    }
}
