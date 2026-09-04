//! Closed, display-safe readiness and provenance copy for marketed providers.

use chrono::DateTime;

use crate::domain::{
    provider::{
        EffectiveAvailability, EffectiveStatus, MarketedProvider, PaceStatus, ProviderQuota,
        ProviderStatus, RunwayStatus, SemanticsStatus,
    },
    schema::QuotaReport,
    tiers::provider_needs_sign_in,
};

/// The only provider readiness/provenance states the UI may expose.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProviderReadiness {
    Live,
    Stale(Option<String>),
    Auth,
    Partial,
    QuotaUnavailable,
    Unsupported,
}

impl ProviderReadiness {
    pub fn text(&self) -> String {
        match self {
            Self::Live => "live".into(),
            Self::Stale(Some(age)) => format!("stale {age}"),
            Self::Stale(None) => "stale".into(),
            Self::Auth => "auth".into(),
            Self::Partial => "partial".into(),
            Self::QuotaUnavailable => "quota unavailable".into(),
            Self::Unsupported => "unsupported".into(),
        }
    }

    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Live)
    }

    pub fn needs_sign_in(&self) -> bool {
        matches!(self, Self::Auth)
    }
}

fn provider_id(provider: &ProviderQuota) -> Option<MarketedProvider> {
    MarketedProvider::from_id(&provider.provider.to_ascii_lowercase())
}

fn stale_age(report: &QuotaReport, provider: &ProviderQuota) -> Option<String> {
    let generated = DateTime::parse_from_rfc3339(&report.generated_at).ok()?;
    let refreshed = DateTime::parse_from_rfc3339(provider.state.refreshed_at.as_deref()?).ok()?;
    let seconds = generated
        .signed_duration_since(refreshed)
        .num_seconds()
        .max(0) as u64;
    Some(match seconds {
        0..=59 => "0m".into(),
        60..=3_599 => format!("{}m", seconds / 60),
        3_600..=86_399 => format!("{}h", seconds / 3_600),
        _ => format!("{}d", seconds / 86_400),
    })
}

fn evidence_is_partial(provider: &ProviderQuota, marketed: MarketedProvider) -> bool {
    provider.semantics_status != Some(SemanticsStatus::Known)
        || provider
            .effective
            .iter()
            .any(|effective| effective.status != EffectiveStatus::Known)
        || (marketed == MarketedProvider::Codex
            && !provider.windows.is_empty()
            && !provider
                .windows
                .iter()
                .any(|window| window.id.starts_with("code_review_")))
}

pub(crate) fn decision_grade(effective: &EffectiveAvailability) -> bool {
    effective.status == EffectiveStatus::Known
        && (effective.effective_percent_remaining.is_some()
            || effective
                .runway
                .as_ref()
                .is_some_and(|runway| runway.status != RunwayStatus::Unknown)
            || effective
                .pace
                .as_ref()
                .is_some_and(|pace| pace.status != PaceStatus::Unknown))
}

/// Classify one provider/account record without forwarding source, account,
/// or error text.
pub(crate) fn quota_readiness(
    report: &QuotaReport,
    provider: &ProviderQuota,
    marketed: MarketedProvider,
) -> ProviderReadiness {
    if provider.state.reason.as_deref() == Some("keychain_access_required") {
        return ProviderReadiness::QuotaUnavailable;
    }
    if provider_needs_sign_in(provider) {
        return ProviderReadiness::Auth;
    }
    if provider.state.stale || provider.state.status == ProviderStatus::Stale {
        return ProviderReadiness::Stale(stale_age(report, provider));
    }
    if !matches!(provider.state.status, ProviderStatus::Fresh) {
        return ProviderReadiness::QuotaUnavailable;
    }
    if provider.semantics_status == Some(SemanticsStatus::Partial) {
        return ProviderReadiness::Partial;
    }
    if provider.windows.is_empty() {
        return ProviderReadiness::QuotaUnavailable;
    }
    if evidence_is_partial(provider, marketed) {
        return ProviderReadiness::Partial;
    }
    if !provider.effective.iter().any(decision_grade) {
        return ProviderReadiness::QuotaUnavailable;
    }
    ProviderReadiness::Live
}

/// Classify one marketed provider. Every reported account must be live before
/// the provider is summarized as live; otherwise the first non-live account
/// keeps its explicit state.
pub fn provider_readiness(report: &QuotaReport, marketed: MarketedProvider) -> ProviderReadiness {
    let states = report
        .providers
        .iter()
        .filter(|provider| provider_id(provider) == Some(marketed))
        .map(|provider| quota_readiness(report, provider, marketed))
        .collect::<Vec<_>>();
    if states.is_empty() {
        return ProviderReadiness::Unsupported;
    }
    states
        .iter()
        .find(|state| !state.is_ready())
        .cloned()
        .unwrap_or(ProviderReadiness::Live)
}

/// First-run readiness copy counts only trustworthy live evidence and explicit auth needs.
pub fn readiness_line(report: &QuotaReport) -> String {
    let readiness = MarketedProvider::ALL.map(|provider| provider_readiness(report, provider));
    let ready = readiness.iter().filter(|state| state.is_ready()).count();
    let sign_in = readiness
        .iter()
        .filter(|state| state.needs_sign_in())
        .count();
    format!("{ready} ready · {sign_in} sign-in")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::provider::{EffectiveAvailability, ProviderState, QuotaWindow};

    fn provider(id: &str, status: ProviderStatus) -> ProviderQuota {
        ProviderQuota {
            provider: id.into(),
            account_label: None,
            account_reported: false,
            label: Some("private@example.com".into()),
            source: Some("/Users/private/auth.json".into()),
            plan: Some("account-123".into()),
            windows: vec![QuotaWindow {
                id: if id == "codex" {
                    "code_review_weekly".into()
                } else {
                    "weekly".into()
                },
                label: "Week".into(),
                kind: "limit".into(),
                percent_used: None,
                percent_remaining: Some(50.0),
                starts_at: None,
                resets_at: None,
                reset_text: None,
                window_seconds: None,
                spent_usd: None,
                limit_usd: None,
                pace: None,
            }],
            effective: vec![EffectiveAvailability {
                scope: "all_models".into(),
                status: EffectiveStatus::Known,
                effective_percent_remaining: Some(50.0),
                bounded_by: vec![],
                limiting_window_ids: vec![],
                pace: None,
                runway: None,
            }],
            semantics_status: Some(SemanticsStatus::Known),
            credits: None,
            state: ProviderState {
                status,
                stale: false,
                refreshed_at: Some("2026-09-03T10:00:00Z".into()),
                auth_status: Some("usable".into()),
                reason: Some("Bearer raw-secret".into()),
                remedy_command: Some("open /Users/private/auth.json".into()),
                error_code: Some("account-123".into()),
            },
        }
    }

    fn report(providers: Vec<ProviderQuota>) -> QuotaReport {
        QuotaReport {
            generated_at: "2026-09-03T12:00:00Z".into(),
            schema_version: 5,
            providers,
            adaptation_warnings: vec!["private@example.com".into()],
        }
    }

    #[test]
    fn finite_states_distinguish_live_auth_stale_partial_unavailable_and_unsupported() {
        let mut auth = provider("codex", ProviderStatus::AuthRequired);
        auth.state.auth_status = Some("unusable".into());
        let mut stale = provider("cursor", ProviderStatus::Stale);
        stale.state.stale = true;
        let mut partial = provider("kimi", ProviderStatus::Fresh);
        partial.semantics_status = Some(SemanticsStatus::Partial);
        let mut no_quota = provider("grok", ProviderStatus::Fresh);
        no_quota.windows.clear();
        let report = report(vec![
            provider("claude", ProviderStatus::Fresh),
            auth,
            stale,
            partial,
            no_quota,
        ]);

        assert_eq!(
            provider_readiness(&report, MarketedProvider::Claude).text(),
            "live"
        );
        assert_eq!(
            provider_readiness(&report, MarketedProvider::Codex).text(),
            "auth"
        );
        assert_eq!(
            provider_readiness(&report, MarketedProvider::Cursor).text(),
            "stale 2h"
        );
        assert_eq!(
            provider_readiness(&report, MarketedProvider::Kimi).text(),
            "partial"
        );
        assert_eq!(
            provider_readiness(&report, MarketedProvider::Grok).text(),
            "quota unavailable"
        );
        assert_eq!(
            provider_readiness(&report, MarketedProvider::Copilot).text(),
            "unsupported"
        );
    }

    #[test]
    fn readiness_copy_does_not_forward_private_or_raw_fields() {
        let report = report(vec![provider("claude", ProviderStatus::Fresh)]);
        let text = MarketedProvider::ALL
            .map(|provider| provider_readiness(&report, provider).text())
            .join("\n");
        assert!(!text.contains("private"));
        assert!(!text.contains("Users"));
        assert!(!text.contains("account"));
        assert!(!text.contains("Bearer"));
        assert!(!text.contains("source"));
        assert!(!text.contains("local estimate"));
    }

    #[test]
    fn first_run_count_uses_all_marketed_provider_states() {
        let report = report(vec![
            provider("claude", ProviderStatus::Fresh),
            provider("codex", ProviderStatus::Fresh),
            provider("cursor", ProviderStatus::Fresh),
            provider("kimi", ProviderStatus::Fresh),
            provider("grok", ProviderStatus::AuthRequired),
            provider("copilot", ProviderStatus::AuthRequired),
        ]);
        assert_eq!(readiness_line(&report), "4 ready · 2 sign-in");
    }

    #[test]
    fn fresh_known_provider_requires_decision_grade_effective_quota() {
        let mut empty = provider("claude", ProviderStatus::Fresh);
        empty.effective.clear();
        let mut valueless = provider("cursor", ProviderStatus::Fresh);
        valueless.effective[0].effective_percent_remaining = None;

        let report = report(vec![empty, valueless]);

        assert_eq!(
            provider_readiness(&report, MarketedProvider::Claude),
            ProviderReadiness::QuotaUnavailable
        );
        assert_eq!(
            provider_readiness(&report, MarketedProvider::Cursor),
            ProviderReadiness::QuotaUnavailable
        );
        assert_eq!(readiness_line(&report), "0 ready · 0 sign-in");
    }

    #[test]
    fn partial_and_keychain_states_precede_quota_shape() {
        let mut partial = provider("claude", ProviderStatus::Fresh);
        partial.semantics_status = Some(SemanticsStatus::Partial);
        partial.windows.clear();
        partial.effective.clear();
        let mut keychain = provider("cursor", ProviderStatus::Fresh);
        keychain.state.reason = Some("keychain_access_required".into());

        let report = report(vec![partial, keychain]);

        assert_eq!(
            provider_readiness(&report, MarketedProvider::Claude),
            ProviderReadiness::Partial
        );
        assert_eq!(
            provider_readiness(&report, MarketedProvider::Cursor),
            ProviderReadiness::QuotaUnavailable
        );
        assert_eq!(readiness_line(&report), "0 ready · 0 sign-in");
        let copy = provider_readiness(&report, MarketedProvider::Cursor).text();
        assert!(!copy.contains("keychain"));
        assert!(!copy.contains("store"));
    }

    #[test]
    fn unknown_semantics_without_windows_is_quota_unavailable() {
        let mut unknown = provider("grok", ProviderStatus::Fresh);
        unknown.semantics_status = Some(SemanticsStatus::Unknown);
        unknown.windows.clear();
        unknown.effective.clear();

        assert_eq!(
            provider_readiness(&report(vec![unknown]), MarketedProvider::Grok),
            ProviderReadiness::QuotaUnavailable
        );
    }
}
