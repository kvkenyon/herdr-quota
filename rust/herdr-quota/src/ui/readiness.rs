//! Closed, display-safe readiness and provenance copy for marketed providers.

use chrono::DateTime;

use crate::domain::{
    provider::{EffectiveStatus, MarketedProvider, ProviderQuota, ProviderStatus, SemanticsStatus},
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

/// Classify one marketed provider without forwarding source, account, or error text.
pub fn provider_readiness(report: &QuotaReport, marketed: MarketedProvider) -> ProviderReadiness {
    let Some(provider) = report
        .providers
        .iter()
        .find(|provider| provider_id(provider) == Some(marketed))
    else {
        return ProviderReadiness::Unsupported;
    };
    if provider_needs_sign_in(provider) {
        return ProviderReadiness::Auth;
    }
    if provider.state.stale || provider.state.status == ProviderStatus::Stale {
        return ProviderReadiness::Stale(stale_age(report, provider));
    }
    if !matches!(provider.state.status, ProviderStatus::Fresh) {
        return ProviderReadiness::QuotaUnavailable;
    }
    if provider.windows.is_empty() {
        return ProviderReadiness::QuotaUnavailable;
    }
    if evidence_is_partial(provider, marketed) {
        return ProviderReadiness::Partial;
    }
    ProviderReadiness::Live
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
}
