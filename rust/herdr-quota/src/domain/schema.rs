//! Adapt quota-axi schema-v5 data to safe product data.

use serde::Serialize;
use serde_json::{Map, Value};
use thiserror::Error;

use super::provider::{
    CreditUnit, Credits, EffectiveAvailability, EffectivePace, EffectiveStatus, MarketedProvider,
    PaceStatus, ProjectionConfidence, ProviderQuota, ProviderState, ProviderStatus, QuotaWindow,
    Runway, RunwayStatus, SemanticsStatus, WindowPace,
};
use crate::sanitize::{sanitize_display_text, sanitize_process_error};

/// The supported quota-axi schema version.
pub const SCHEMA_VERSION: u8 = 5;

/// A safe quota report.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaReport {
    pub generated_at: String,
    pub schema_version: u8,
    pub providers: Vec<ProviderQuota>,
    pub adaptation_warnings: Vec<String>,
}

/// An error at the top-level schema boundary.
#[derive(Debug, Error)]
pub enum SchemaError {
    #[error("The quota-axi output is not valid JSON.")]
    InvalidJson(#[source] serde_json::Error),
    #[error("The quota-axi JSON schema is not supported. Expected version 5.")]
    UnsupportedSchema,
    #[error("The quota-axi schema-v5 response is not valid.")]
    InvalidResponse,
}

fn object(value: &Value) -> Option<&Map<String, Value>> {
    value.as_object()
}

fn text(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .and_then(sanitize_display_text)
}

fn number(value: Option<&Value>, minimum: Option<f64>, maximum: Option<f64>) -> Option<f64> {
    let value = value.and_then(Value::as_f64)?;
    if minimum.is_some_and(|minimum| value < minimum)
        || maximum.is_some_and(|maximum| value > maximum)
    {
        None
    } else {
        Some(value)
    }
}

fn valid_iso_timestamp(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() < 20 || !bytes.is_ascii() {
        return false;
    }
    for index in [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18] {
        if !bytes[index].is_ascii_digit() {
            return false;
        }
    }
    if bytes[4] != b'-'
        || bytes[7] != b'-'
        || !matches!(bytes[10], b'T' | b't')
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return false;
    }
    let pair = |index: usize| (bytes[index] - b'0') * 10 + bytes[index + 1] - b'0';
    let year = u32::from(bytes[0] - b'0') * 1000
        + u32::from(bytes[1] - b'0') * 100
        + u32::from(bytes[2] - b'0') * 10
        + u32::from(bytes[3] - b'0');
    let month = pair(5);
    let day = pair(8);
    let leap_year =
        year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let days_in_month = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap_year => 29,
        2 => 28,
        _ => return false,
    };
    if day == 0 || day > days_in_month || pair(11) > 23 || pair(14) > 59 || pair(17) > 59 {
        return false;
    }

    let suffix = &value[19..];
    if suffix == "Z" || suffix == "z" {
        return true;
    }
    if !matches!(bytes[19], b'.' | b'+' | b'-') {
        return false;
    }
    let suffix = suffix.strip_prefix('.').unwrap_or(suffix);
    let zone_index = suffix.find(['Z', 'z', '+', '-']);
    let Some(zone_index) = zone_index else {
        return false;
    };
    let (fraction, zone) = suffix.split_at(zone_index);
    if value.as_bytes()[19] == b'.'
        && (fraction.is_empty() || !fraction.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return false;
    }
    if zone == "Z" || zone == "z" {
        return true;
    }
    zone.len() == 6
        && matches!(zone.as_bytes()[0], b'+' | b'-')
        && zone.as_bytes()[1..3].iter().all(u8::is_ascii_digit)
        && zone.as_bytes()[3] == b':'
        && zone.as_bytes()[4..6].iter().all(u8::is_ascii_digit)
        && zone.as_bytes()[1..3]
            .iter()
            .fold(0, |value, byte| value * 10 + byte - b'0')
            <= 23
        && zone.as_bytes()[4..6]
            .iter()
            .fold(0, |value, byte| value * 10 + byte - b'0')
            <= 59
}

fn iso(value: Option<&Value>) -> Option<String> {
    text(value).filter(|value| valid_iso_timestamp(value))
}

fn strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(|value| text(Some(value)))
                .collect()
        })
        .unwrap_or_default()
}

fn masked_email(value: &str) -> Option<String> {
    let (local, domain) = value.rsplit_once('@')?;
    if local.is_empty() || domain.is_empty() || domain.contains('@') {
        return None;
    }
    let first = local.chars().next()?;
    Some(format!("{first}•••@{domain}"))
}

/// Keep only quota-axi's documented display-only account fields. In
/// particular, accountId and every unknown account field are discarded.
fn account_label(value: Option<&Value>) -> Option<String> {
    let account = value.and_then(object)?;
    text(account.get("email"))
        .and_then(|email| masked_email(&email))
        .or_else(|| {
            text(account.get("organization"))
                .filter(|organization| sanitize_process_error(organization) == *organization)
        })
}

fn pace_status(value: Option<&str>) -> Option<PaceStatus> {
    match value {
        Some("ahead") => Some(PaceStatus::Ahead),
        Some("on_pace") => Some(PaceStatus::OnPace),
        Some("behind") => Some(PaceStatus::Behind),
        Some("mixed") => Some(PaceStatus::Mixed),
        Some("unknown") => Some(PaceStatus::Unknown),
        _ => None,
    }
}

fn runway_status(value: Option<&str>) -> Option<RunwayStatus> {
    match value {
        Some("exhausted_now") => Some(RunwayStatus::ExhaustedNow),
        Some("projected_exhaustion") => Some(RunwayStatus::ProjectedExhaustion),
        Some("through_reset") => Some(RunwayStatus::ThroughReset),
        Some("unknown") => Some(RunwayStatus::Unknown),
        _ => None,
    }
}

fn projection_confidence(value: Option<&Value>) -> Option<ProjectionConfidence> {
    match value.and_then(Value::as_str) {
        Some("early") => Some(ProjectionConfidence::Early),
        Some("established") => Some(ProjectionConfidence::Established),
        _ => None,
    }
}

fn adapt_window(value: &Value) -> Option<QuotaWindow> {
    let raw = object(value)?;
    let id = text(raw.get("id"))?;
    let pace = raw
        .get("pace")
        .and_then(object)
        .and_then(|pace| {
            pace_status(text(pace.get("status")).as_deref()).map(|status| (pace, status))
        })
        .map(|(pace, status)| WindowPace {
            status,
            reserve_percent_points: number(pace.get("reservePercentPoints"), None, None),
            burn_multiple: number(pace.get("burnMultiple"), Some(0.0), None),
            projected_exhausted_at: iso(pace.get("projectedExhaustedAt")),
            projection_confidence: projection_confidence(pace.get("projectionConfidence")),
        });

    Some(QuotaWindow {
        label: text(raw.get("label")).unwrap_or_else(|| id.replace('_', " ")),
        id,
        kind: text(raw.get("kind")).unwrap_or_else(|| "unknown".to_owned()),
        percent_used: number(raw.get("percentUsed"), Some(0.0), Some(100.0)),
        percent_remaining: number(raw.get("percentRemaining"), Some(0.0), Some(100.0)),
        starts_at: iso(raw.get("startsAt")),
        resets_at: iso(raw.get("resetsAt")),
        reset_text: text(raw.get("resetText")),
        window_seconds: number(raw.get("windowSeconds"), Some(1.0), None),
        spent_usd: number(raw.get("spentUsd"), Some(0.0), None),
        limit_usd: number(raw.get("limitUsd"), Some(0.0), None),
        pace,
    })
}

fn adapt_effective(value: &Value) -> Option<EffectiveAvailability> {
    let raw = object(value)?;
    let scope = text(raw.get("scope"))?;
    let pace = raw
        .get("pace")
        .and_then(object)
        .and_then(|pace| {
            pace_status(text(pace.get("status")).as_deref()).map(|status| (pace, status))
        })
        .map(|(pace, status)| EffectivePace {
            status,
            worst_reserve_percent_points: number(pace.get("worstReservePercentPoints"), None, None),
            worst_reserve_window_id: text(pace.get("worstReserveWindowId")),
            unknown_window_ids: strings(pace.get("unknownWindowIds")),
        });
    let runway = raw
        .get("runway")
        .and_then(object)
        .and_then(|runway| {
            runway_status(text(runway.get("status")).as_deref()).map(|status| (runway, status))
        })
        .map(|(runway, status)| Runway {
            status,
            usable_runway_seconds: number(runway.get("usableRunwaySeconds"), Some(0.0), None),
            projected_exhausted_at: iso(runway.get("projectedExhaustedAt")),
            limiting_window_id: text(runway.get("limitingWindowId")),
            projection_confidence: projection_confidence(runway.get("projectionConfidence")),
            unmeasurable_window_ids: strings(runway.get("unmeasurableWindowIds")),
        });

    Some(EffectiveAvailability {
        scope,
        status: if raw.get("status").and_then(Value::as_str) == Some("known") {
            EffectiveStatus::Known
        } else {
            EffectiveStatus::Unknown
        },
        effective_percent_remaining: number(
            raw.get("effectivePercentRemaining"),
            Some(0.0),
            Some(100.0),
        ),
        bounded_by: strings(raw.get("boundedBy")),
        limiting_window_ids: strings(raw.get("limitingWindowIds")),
        pace,
        runway,
    })
}

fn invalid_provider(label: String) -> ProviderQuota {
    ProviderQuota {
        provider: label.clone(),
        account_label: None,
        account_reported: false,
        label: Some(label),
        source: None,
        plan: None,
        windows: Vec::new(),
        effective: Vec::new(),
        semantics_status: None,
        credits: None,
        state: ProviderState {
            status: ProviderStatus::Error,
            stale: false,
            refreshed_at: None,
            auth_status: None,
            reason: None,
            remedy_command: None,
            error_code: Some("schema_invalid".to_owned()),
        },
    }
}

fn provider_status(value: Option<&str>) -> ProviderStatus {
    match value {
        Some("fresh") => ProviderStatus::Fresh,
        Some("stale") => ProviderStatus::Stale,
        Some("unavailable") => ProviderStatus::Unavailable,
        Some("auth_required") => ProviderStatus::AuthRequired,
        Some("rate_limited") => ProviderStatus::RateLimited,
        Some("error") => ProviderStatus::Error,
        _ => ProviderStatus::Error,
    }
}

fn semantics_status(value: Option<&Value>) -> Option<SemanticsStatus> {
    match value.and_then(Value::as_str) {
        Some("known") => Some(SemanticsStatus::Known),
        Some("partial") => Some(SemanticsStatus::Partial),
        Some("unknown") => Some(SemanticsStatus::Unknown),
        _ => None,
    }
}

fn credit_unit(value: Option<&Value>) -> Option<CreditUnit> {
    match value.and_then(Value::as_str) {
        Some("usd") => Some(CreditUnit::Usd),
        Some("credits") => Some(CreditUnit::Credits),
        _ => None,
    }
}

fn adapt_provider(value: &Value, index: usize, warnings: &mut Vec<String>) -> ProviderQuota {
    let Some(raw) = object(value) else {
        warnings.push(format!("provider {} did not match schema v5", index + 1));
        return invalid_provider(format!("provider-{}", index + 1));
    };
    let Some(provider) = text(raw.get("provider")) else {
        warnings.push(format!("provider {} did not match schema v5", index + 1));
        return invalid_provider(format!("provider-{}", index + 1));
    };
    let (Some(state), Some(window_values)) = (
        raw.get("state").and_then(object),
        raw.get("windows").and_then(Value::as_array),
    ) else {
        warnings.push(format!("{provider} did not match schema v5"));
        return invalid_provider(provider);
    };

    let windows = window_values
        .iter()
        .filter_map(adapt_window)
        .collect::<Vec<_>>();
    if windows.len() != window_values.len() {
        warnings.push(format!("{provider} omitted malformed windows"));
    }
    let semantics = raw.get("quotaSemantics").and_then(object);
    let effective = semantics
        .and_then(|value| value.get("effectiveAvailability"))
        .and_then(Value::as_array)
        .map(|values| values.iter().filter_map(adapt_effective).collect())
        .unwrap_or_default();
    let status = provider_status(text(state.get("status")).as_deref());
    let credits = raw.get("credits").and_then(object).map(|credits| Credits {
        remaining: number(credits.get("remaining"), Some(0.0), None),
        unlimited: credits.get("unlimited").and_then(Value::as_bool),
        unit: credit_unit(credits.get("unit")),
    });

    ProviderQuota {
        provider,
        account_label: account_label(raw.get("account")),
        account_reported: raw.get("account").and_then(object).is_some(),
        label: text(raw.get("label")),
        source: text(raw.get("source")),
        plan: text(raw.get("plan")),
        windows,
        effective,
        semantics_status: semantics.and_then(|value| semantics_status(value.get("status"))),
        credits,
        state: ProviderState {
            status,
            stale: state.get("stale").and_then(Value::as_bool) == Some(true)
                || status == ProviderStatus::Stale,
            refreshed_at: iso(state.get("refreshedAt")),
            auth_status: text(state.get("authStatus")),
            reason: text(state.get("reason")),
            remedy_command: text(state.get("remedyCommand")),
            error_code: text(state.get("error")),
        },
    }
}

/// Parse and adapt one quota-axi JSON response.
pub fn parse_quota_response(input: &str) -> Result<QuotaReport, SchemaError> {
    let value = serde_json::from_str(input).map_err(SchemaError::InvalidJson)?;
    adapt_quota_response(&value)
}

/// Adapt one quota-axi schema-v5 response.
pub fn adapt_quota_response(value: &Value) -> Result<QuotaReport, SchemaError> {
    let raw = object(value).ok_or(SchemaError::UnsupportedSchema)?;
    let version_is_five = raw
        .get("schemaVersion")
        .and_then(Value::as_f64)
        .is_some_and(|value| value == f64::from(SCHEMA_VERSION));
    if !version_is_five {
        return Err(SchemaError::UnsupportedSchema);
    }
    let generated_at = iso(raw.get("generatedAt")).ok_or(SchemaError::InvalidResponse)?;
    let providers = raw
        .get("providers")
        .and_then(Value::as_array)
        .ok_or(SchemaError::InvalidResponse)?;
    let accepted = providers.iter().filter(|value| {
        let provider = object(value).and_then(|raw| text(raw.get("provider")));
        provider.is_none_or(|provider| {
            MarketedProvider::from_id(&provider.to_ascii_lowercase()).is_some()
        })
    });
    let mut warnings = Vec::new();
    let providers = accepted
        .enumerate()
        .map(|(index, value)| adapt_provider(value, index, &mut warnings))
        .collect();

    Ok(QuotaReport {
        generated_at,
        schema_version: SCHEMA_VERSION,
        providers,
        adaptation_warnings: warnings,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{SchemaError, adapt_quota_response, parse_quota_response};
    use crate::domain::provider::{
        CreditUnit, EffectiveStatus, MarketedProvider, PaceStatus, ProjectionConfidence,
        ProviderStatus, RunwayStatus, SemanticsStatus,
    };

    fn provider(id: &str) -> Value {
        json!({
            "provider": id,
            "label": id,
            "windows": [],
            "quotaSemantics": {
                "status": "unknown",
                "effectiveAvailability": []
            },
            "state": { "status": "fresh", "stale": false }
        })
    }

    fn response(providers: Vec<Value>) -> Value {
        json!({
            "generatedAt": "2026-09-02T12:00:00.000Z",
            "schemaVersion": 5,
            "providers": providers
        })
    }

    #[test]
    fn rejects_a_top_level_schema_mismatch() {
        let error = adapt_quota_response(&json!({
            "generatedAt": "2026-09-02T12:00:00.000Z",
            "schemaVersion": 6,
            "providers": []
        }))
        .expect_err("schema 6 must fail");

        assert!(matches!(error, SchemaError::UnsupportedSchema));
        assert!(matches!(
            adapt_quota_response(&json!({ "schemaVersion": 5, "providers": [] })),
            Err(SchemaError::InvalidResponse)
        ));
    }

    #[test]
    fn isolates_a_malformed_provider_from_healthy_siblings() {
        let raw = response(vec![
            provider("claude"),
            json!({ "provider": "cursor", "windows": "bad", "state": {} }),
            provider("kimi"),
        ]);
        let report = adapt_quota_response(&raw).expect("the report must adapt");

        assert_eq!(report.providers[0].state.status, ProviderStatus::Fresh);
        assert_eq!(report.providers[1].state.status, ProviderStatus::Error);
        assert_eq!(
            report.providers[1].state.error_code.as_deref(),
            Some("schema_invalid")
        );
        assert_eq!(report.providers[2].provider, "kimi");
        assert!(report.adaptation_warnings[0].contains("cursor"));
    }

    #[test]
    fn keeps_unknown_tier_ids_for_an_allowed_provider() {
        let raw = response(vec![json!({
            "provider": "kimi",
            "windows": [{
                "id": "limit:2",
                "label": "daily boost",
                "kind": "unknown",
                "percentRemaining": 55
            }],
            "quotaSemantics": {
                "status": "partial",
                "effectiveAvailability": [{
                    "scope": "all_models",
                    "status": "known",
                    "boundedBy": ["limit:2"],
                    "limitingWindowIds": ["limit:2"]
                }]
            },
            "state": { "status": "fresh", "stale": false }
        })]);
        let report = adapt_quota_response(&raw).expect("the report must adapt");

        assert_eq!(report.providers[0].windows[0].id, "limit:2");
        assert_eq!(report.providers[0].windows[0].label, "daily boost");
        assert_eq!(
            report.providers[0].effective[0].limiting_window_ids,
            ["limit:2"]
        );
    }

    #[test]
    fn adapts_the_schema_v5_allow_list() {
        let raw = response(vec![json!({
            "provider": "claude",
            "label": "Claude",
            "source": "oauth",
            "plan": "max_20x",
            "windows": [{
                "id": "seven_day",
                "label": "week",
                "kind": "weekly",
                "percentUsed": 47,
                "percentRemaining": 53,
                "startsAt": "2026-08-27T12:00:00Z",
                "resetsAt": "2026-09-03T12:00:00Z",
                "resetText": "tomorrow",
                "windowSeconds": 604800,
                "spentUsd": 8.75,
                "limitUsd": 25,
                "pace": {
                    "status": "ahead",
                    "reservePercentPoints": -2.5,
                    "burnMultiple": 1.4,
                    "projectedExhaustedAt": "2026-09-03T10:00:00Z",
                    "projectionConfidence": "established"
                }
            }],
            "quotaSemantics": {
                "status": "known",
                "effectiveAvailability": [{
                    "scope": "all_models",
                    "status": "known",
                    "effectivePercentRemaining": 53,
                    "boundedBy": ["seven_day"],
                    "limitingWindowIds": ["seven_day"],
                    "pace": {
                        "status": "ahead",
                        "worstReservePercentPoints": -2.5,
                        "worstReserveWindowId": "seven_day",
                        "unknownWindowIds": ["future_window"]
                    },
                    "runway": {
                        "status": "projected_exhaustion",
                        "usableRunwaySeconds": 79200,
                        "projectedExhaustedAt": "2026-09-03T10:00:00Z",
                        "limitingWindowId": "seven_day",
                        "projectionConfidence": "early",
                        "unmeasurableWindowIds": ["future_window"]
                    }
                }]
            },
            "credits": { "remaining": 16.25, "unlimited": false, "unit": "usd" },
            "state": {
                "status": "fresh",
                "stale": false,
                "refreshedAt": "2026-09-02T12:00:00Z",
                "authStatus": "usable",
                "reason": "cached",
                "remedyCommand": "claude /login",
                "error": "quota_unavailable"
            }
        })]);
        let report = adapt_quota_response(&raw).expect("the report must adapt");
        let provider = &report.providers[0];
        let window = &provider.windows[0];
        let effective = &provider.effective[0];

        assert_eq!(provider.semantics_status, Some(SemanticsStatus::Known));
        assert_eq!(window.percent_remaining, Some(53.0));
        assert_eq!(
            window.pace.as_ref().map(|pace| pace.status),
            Some(PaceStatus::Ahead)
        );
        assert_eq!(
            window
                .pace
                .as_ref()
                .and_then(|pace| pace.projection_confidence),
            Some(ProjectionConfidence::Established)
        );
        assert_eq!(effective.status, EffectiveStatus::Known);
        assert_eq!(
            effective.runway.as_ref().map(|runway| runway.status),
            Some(RunwayStatus::ProjectedExhaustion)
        );
        assert_eq!(
            provider.credits.as_ref().and_then(|credits| credits.unit),
            Some(CreditUnit::Usd)
        );
        assert_eq!(provider.state.auth_status.as_deref(), Some("usable"));
        assert_eq!(
            provider.state.error_code.as_deref(),
            Some("quota_unavailable")
        );
    }

    #[test]
    fn adapts_all_six_marketed_providers_and_drops_other_ids() {
        let mut providers = MarketedProvider::ALL
            .iter()
            .map(|provider_id| provider(provider_id.id()))
            .collect::<Vec<_>>();
        providers.push(provider("future-lab"));
        let report = adapt_quota_response(&response(providers)).expect("the report must adapt");
        let ids = report
            .providers
            .iter()
            .map(|provider| provider.provider.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            ids,
            ["claude", "codex", "cursor", "kimi", "grok", "copilot"]
        );
    }

    #[test]
    fn bounds_and_cleans_schema_display_strings() {
        let long_label = format!("{}\u{001b}[2Jhidden\n", "x".repeat(300));
        let raw = response(vec![json!({
            "provider": "claude",
            "label": long_label,
            "windows": [{ "id": "five_hour", "label": "ses\rsion" }],
            "state": { "status": "fresh", "stale": false }
        })]);
        let report = adapt_quota_response(&raw).expect("the report must adapt");
        let label = report.providers[0]
            .label
            .as_deref()
            .expect("label must exist");

        assert_eq!(label.chars().count(), 256);
        assert!(label.chars().all(|character| character == 'x'));
        assert_eq!(report.providers[0].windows[0].label, "session");
    }

    #[test]
    fn arbitrary_input_fields_do_not_serialize() {
        let raw = json!({
            "generatedAt": "2026-09-02T12:00:00Z",
            "schemaVersion": 5,
            "secretTopLevel": "top-secret",
            "providers": [{
                "provider": "claude",
                "secretProviderField": "provider-secret",
                "windows": [{
                    "id": "five_hour",
                    "secretWindowField": "window-secret"
                }],
                "quotaSemantics": {
                    "secretSemanticsField": "semantics-secret",
                    "effectiveAvailability": []
                },
                "state": {
                    "status": "fresh",
                    "stale": false,
                    "secretStateField": "state-secret"
                }
            }]
        });
        let report = adapt_quota_response(&raw).expect("the report must adapt");
        let serialized = serde_json::to_value(report).expect("the report must serialize");
        let serialized = serialized.to_string();

        assert!(!serialized.contains("secret"));
        assert!(!serialized.contains("Secret"));
    }

    #[test]
    fn repeated_provider_accounts_keep_only_safe_presentation_identity() {
        let report =
            parse_quota_response(include_str!("../../../../test/fixtures/multi-account.json"))
                .expect("multi-account fixture must adapt");
        let claude = report
            .providers
            .iter()
            .filter(|provider| provider.provider == "claude")
            .collect::<Vec<_>>();

        assert_eq!(claude.len(), 3);
        assert_eq!(claude[0].account_label.as_deref(), Some("p•••@example.com"));
        assert_eq!(claude[1].account_label.as_deref(), Some("Research Team"));
        assert_eq!(claude[2].account_label, None);
        assert!(claude.iter().all(|provider| provider.account_reported));
        assert_eq!(claude[0].windows.len(), 2);
        assert_eq!(claude[1].windows.len(), 2);
        assert_eq!(claude[2].windows.len(), 1);

        let serialized = serde_json::to_string(&report).expect("safe report must serialize");
        for secret in [
            "opaque-account-secret",
            "sk-secret",
            "credentialPath",
            "/Users/private",
            "refreshToken",
            "token-secret",
            "primary.user@example.com",
        ] {
            assert!(!serialized.contains(secret), "leaked {secret}");
        }
    }

    #[test]
    fn parses_json_without_an_external_oracle() {
        let report = parse_quota_response(
            r#"{"generatedAt":"2026-09-02T12:00:00Z","schemaVersion":5,"providers":[]}"#,
        )
        .expect("the JSON must parse");

        assert_eq!(report.schema_version, 5);
        assert!(report.providers.is_empty());
    }
}
