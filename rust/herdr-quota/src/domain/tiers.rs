//! Provider-owned tier labels and safe detail presentations.

use crate::domain::provider::{
    EffectiveStatus, MarketedProvider, PaceStatus, ProviderQuota, ProviderStatus, QuotaWindow,
    SemanticsStatus,
};
use crate::sanitize::friendly_provider_error;

/// The conclusion supported by one individual quota window.
#[derive(Clone, Debug, PartialEq)]
pub enum TierConclusion {
    OnPace,
    Ahead {
        projected_exhausted_at: Option<String>,
    },
    Spend {
        spent_usd: f64,
        limit_usd: Option<f64>,
    },
    NotReported,
    Unknown,
}

/// One provider-owned quota tier. Aggregate effective availability never
/// replaces these rows: it only identifies the limiting row.
#[derive(Clone, Debug, PartialEq)]
pub struct TierRow {
    pub id: String,
    pub label: String,
    pub compact_label: String,
    pub percent_remaining: Option<f64>,
    pub resets_at: Option<String>,
    pub conclusion: TierConclusion,
    pub limiting: bool,
}

/// The safe content of one provider section.
#[derive(Clone, Debug, PartialEq)]
pub enum ProviderPresentation {
    Tiers(Vec<TierRow>),
    Recovery { instruction: &'static str },
    Message { message: &'static str },
}

/// A compact provider-status annotation for a future renderer.
#[derive(Clone, Debug, PartialEq)]
pub struct ProviderAnnotation {
    pub text: &'static str,
    pub tone: AnnotationTone,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AnnotationTone {
    Bad,
    Warn,
    Muted,
}

fn provider_kind(provider: &ProviderQuota) -> Option<MarketedProvider> {
    MarketedProvider::from_id(&provider.provider.to_ascii_lowercase())
}

fn title_case_slug(slug: &str) -> String {
    slug.split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut characters = part.chars();
            match characters.next() {
                Some(first) => format!("{}{}", first.to_uppercase(), characters.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn passthrough(window: &QuotaWindow) -> (String, String) {
    (window.label.clone(), window.label.clone())
}

fn claude_label(window: &QuotaWindow) -> (String, String) {
    match window.id.as_str() {
        "five_hour" => ("Session".into(), "Session".into()),
        "seven_day" => ("Week".into(), "Week".into()),
        "seven_day_opus" => ("Opus week".into(), "Opus".into()),
        "extra_usage" => ("Extra usage".into(), "Extra".into()),
        _ => match window.id.strip_prefix("model:") {
            Some(model) => {
                let name = title_case_slug(model);
                (format!("{name} week"), name)
            }
            None => passthrough(window),
        },
    }
}

fn codex_model_name(window: &QuotaWindow) -> String {
    let suffixes = [
        " sessions",
        " session",
        " weekly",
        " week",
        " 5h",
        " 5 hours",
        " 7d",
        " 7 days",
    ];
    let mut base = window.label.clone();
    let lower = base.to_ascii_lowercase();
    if let Some(suffix) = suffixes.iter().find(|suffix| lower.ends_with(**suffix)) {
        base.truncate(base.len() - suffix.len());
    }
    let lower = base.to_ascii_lowercase();
    if lower.starts_with("gpt-") {
        if let Some(index) = lower.find("-codex-") {
            base = base[index + "-codex-".len()..].to_owned();
        } else if lower.ends_with("-codex") {
            base.truncate(base.len() - "-codex".len());
        }
    }
    if base.is_empty() {
        window.label.clone()
    } else {
        base
    }
}

fn codex_label(window: &QuotaWindow) -> (String, String) {
    match window.id.as_str() {
        "five_hour" => ("Session".into(), "Session".into()),
        "weekly" => ("Week".into(), "Week".into()),
        _ if window.id.starts_with("code_review_five_hour") => {
            ("Review 5h".into(), "Review 5h".into())
        }
        _ if window.id.starts_with("code_review_weekly") => {
            ("Review week".into(), "Review wk".into())
        }
        _ => {
            let model = window.id.strip_prefix("model:").and_then(|id| {
                let (_, duration) = id.rsplit_once(':')?;
                ["5h", "7d"].into_iter().find(|candidate| {
                    duration == *candidate
                        || duration
                            .strip_prefix(&format!("{candidate}_"))
                            .is_some_and(|suffix| {
                                !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
                            })
                })
            });
            match model {
                Some("7d") => {
                    let name = codex_model_name(window);
                    (format!("{name} week"), name)
                }
                Some("5h") => {
                    let name = codex_model_name(window);
                    (format!("{name} 5h"), format!("{name} 5h"))
                }
                _ => passthrough(window),
            }
        }
    }
}

fn cursor_label(window: &QuotaWindow) -> (String, String) {
    match window.id.as_str() {
        "included_usage" => ("Included".into(), "Included".into()),
        "auto_usage" => ("Auto".into(), "Auto".into()),
        "api_usage" => ("3rd-party models".into(), "3rd-party".into()),
        "spend_limit" => ("Spend limit".into(), "Spend".into()),
        _ => passthrough(window),
    }
}

fn kimi_label(window: &QuotaWindow) -> (String, String) {
    match window.id.as_str() {
        "five_hour" => ("Session".into(), "Session".into()),
        "weekly" => ("Week".into(), "Week".into()),
        _ => passthrough(window),
    }
}

fn grok_label(window: &QuotaWindow) -> (String, String) {
    if window.id == "credits" {
        ("Consumer quota".into(), "Consumer".into())
    } else {
        passthrough(window)
    }
}

fn copilot_label(window: &QuotaWindow) -> (String, String) {
    match window.id.as_str() {
        "chat" => ("Chat".into(), "Chat".into()),
        "completions" => ("Completions".into(), "Complete".into()),
        "premium_interactions" => ("Premium".into(), "Premium".into()),
        _ => passthrough(window),
    }
}

fn tier_label(provider: Option<MarketedProvider>, window: &QuotaWindow) -> (String, String) {
    match provider {
        Some(MarketedProvider::Claude) => claude_label(window),
        Some(MarketedProvider::Codex) => codex_label(window),
        Some(MarketedProvider::Cursor) => cursor_label(window),
        Some(MarketedProvider::Kimi) => kimi_label(window),
        Some(MarketedProvider::Grok) => grok_label(window),
        Some(MarketedProvider::Copilot) => copilot_label(window),
        None => passthrough(window),
    }
}

fn tier_conclusion(window: &QuotaWindow) -> TierConclusion {
    match window.pace.as_ref().map(|pace| pace.status) {
        Some(PaceStatus::Ahead) => TierConclusion::Ahead {
            projected_exhausted_at: window
                .pace
                .as_ref()
                .and_then(|pace| pace.projected_exhausted_at.clone()),
        },
        Some(PaceStatus::OnPace | PaceStatus::Behind) => TierConclusion::OnPace,
        _ => match window.spent_usd {
            Some(spent_usd) => TierConclusion::Spend {
                spent_usd,
                limit_usd: window.limit_usd,
            },
            None => TierConclusion::Unknown,
        },
    }
}

fn limiting_window_ids(provider: &ProviderQuota) -> Vec<&str> {
    let primary = provider
        .effective
        .iter()
        .find(|item| matches!(item.scope.as_str(), "all_models" | "all_products"))
        .or_else(|| {
            provider
                .effective
                .iter()
                .find(|item| item.status == EffectiveStatus::Known)
        });
    primary
        .map(|effective| {
            effective
                .limiting_window_ids
                .iter()
                .map(String::as_str)
                .collect()
        })
        .unwrap_or_default()
}

/// Maps every reported window to an honest provider-owned tier, in source
/// order. Only the six marketed providers receive product labels.
pub fn provider_tiers(provider: &ProviderQuota) -> Vec<TierRow> {
    let kind = provider_kind(provider);
    let limiting = limiting_window_ids(provider);
    let mut rows = provider
        .windows
        .iter()
        .map(|window| {
            let (label, compact_label) = tier_label(kind, window);
            TierRow {
                id: window.id.clone(),
                label,
                compact_label,
                percent_remaining: window.percent_remaining,
                resets_at: window.resets_at.clone(),
                conclusion: tier_conclusion(window),
                limiting: limiting.contains(&window.id.as_str()),
            }
        })
        .collect::<Vec<_>>();

    if kind == Some(MarketedProvider::Codex)
        && !provider.windows.is_empty()
        && !provider
            .windows
            .iter()
            .any(|window| window.id.starts_with("code_review_"))
    {
        rows.push(TierRow {
            id: "code_review".into(),
            label: "Code review".into(),
            compact_label: "Review".into(),
            percent_remaining: None,
            resets_at: None,
            conclusion: TierConclusion::NotReported,
            limiting: false,
        });
    }
    rows
}

/// True only when the provider cannot safely use its existing sign-in.
pub fn provider_needs_sign_in(provider: &ProviderQuota) -> bool {
    provider.state.status == ProviderStatus::AuthRequired
        || matches!(
            provider.state.auth_status.as_deref(),
            Some("unusable" | "expired_refreshable")
        )
}

fn grok_consumer_quota_unavailable(provider: &ProviderQuota) -> bool {
    provider_kind(provider) == Some(MarketedProvider::Grok)
        && provider.state.status == ProviderStatus::Fresh
        && !provider.state.stale
        && provider.state.auth_status.as_deref() == Some("usable")
        && provider.semantics_status != Some(SemanticsStatus::Partial)
        && provider.windows.is_empty()
}

/// Chooses provider detail without ever presenting unreadable data as zero.
pub fn present_provider(provider: &ProviderQuota) -> ProviderPresentation {
    if provider.state.reason.as_deref() == Some("keychain_access_required") {
        return ProviderPresentation::Message {
            message: "Keychain approval required",
        };
    }
    if provider_needs_sign_in(provider) {
        return ProviderPresentation::Recovery {
            instruction: provider_kind(provider)
                .map(MarketedProvider::recovery_instruction)
                .unwrap_or("sign in with the provider CLI"),
        };
    }
    if grok_consumer_quota_unavailable(provider) {
        return ProviderPresentation::Message {
            message: "Consumer quota unavailable",
        };
    }
    if provider.windows.is_empty() {
        return ProviderPresentation::Message {
            message: friendly_provider_error(
                provider
                    .state
                    .error_code
                    .as_deref()
                    .or(provider.state.reason.as_deref()),
            ),
        };
    }
    ProviderPresentation::Tiers(provider_tiers(provider))
}

/// Returns a status annotation while preserving the separate recovery detail.
pub fn provider_annotation(provider: &ProviderQuota) -> Option<ProviderAnnotation> {
    if provider.state.reason.as_deref() == Some("keychain_access_required") {
        return None;
    }
    if provider_needs_sign_in(provider) {
        return Some(ProviderAnnotation {
            text: "signed out",
            tone: AnnotationTone::Bad,
        });
    }
    if provider.state.stale || provider.state.status == ProviderStatus::Stale {
        return Some(ProviderAnnotation {
            text: "stale",
            tone: AnnotationTone::Warn,
        });
    }
    if provider.state.status == ProviderStatus::RateLimited {
        return Some(ProviderAnnotation {
            text: "rate limited",
            tone: AnnotationTone::Warn,
        });
    }
    if provider.state.status == ProviderStatus::Error {
        return Some(ProviderAnnotation {
            text: "error",
            tone: AnnotationTone::Bad,
        });
    }
    if provider.semantics_status == Some(SemanticsStatus::Partial) {
        return Some(ProviderAnnotation {
            text: "partial data",
            tone: AnnotationTone::Warn,
        });
    }
    if grok_consumer_quota_unavailable(provider) {
        return Some(ProviderAnnotation {
            text: "consumer quota unavailable",
            tone: AnnotationTone::Muted,
        });
    }
    if provider.state.status == ProviderStatus::Unavailable || provider.windows.is_empty() {
        return Some(ProviderAnnotation {
            text: "no reading",
            tone: AnnotationTone::Muted,
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::provider::{EffectiveAvailability, ProviderState};

    fn provider(id: &str, windows: Vec<QuotaWindow>) -> ProviderQuota {
        ProviderQuota {
            provider: id.into(),
            label: None,
            source: None,
            plan: None,
            windows,
            effective: vec![],
            semantics_status: Some(SemanticsStatus::Known),
            credits: None,
            state: ProviderState {
                status: ProviderStatus::Fresh,
                stale: false,
                refreshed_at: None,
                auth_status: Some("usable".into()),
                reason: None,
                remedy_command: None,
                error_code: None,
            },
        }
    }

    fn window(id: &str) -> QuotaWindow {
        QuotaWindow {
            id: id.into(),
            label: id.into(),
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
        }
    }

    #[test]
    fn maps_the_six_marketed_provider_labels() {
        let cases = [
            ("claude", "five_hour", "Session"),
            ("codex", "weekly", "Week"),
            ("cursor", "api_usage", "3rd-party models"),
            ("kimi", "five_hour", "Session"),
            ("grok", "credits", "Consumer quota"),
            ("copilot", "premium_interactions", "Premium"),
        ];
        for (id, window_id, expected) in cases {
            assert_eq!(
                provider_tiers(&provider(id, vec![window(window_id)]))[0].label,
                expected
            );
        }
    }

    #[test]
    fn codex_keeps_the_review_workload_explicit() {
        let rows = provider_tiers(&provider("codex", vec![window("weekly")]));
        assert!(matches!(
            rows.last().unwrap().conclusion,
            TierConclusion::NotReported
        ));
        assert_eq!(rows.last().unwrap().label, "Code review");
    }

    #[test]
    fn codex_model_duration_requires_an_exact_supported_suffix() {
        let mut valid = window("model:gpt-5.1-codex-mini:7d_2");
        valid.label = "gpt-5.1-codex-mini weekly".into();
        let mut malformed = window("model:future:7d_beta");
        malformed.label = "Future provider label".into();

        let rows = provider_tiers(&provider("codex", vec![valid, malformed]));

        assert_eq!(rows[0].label, "mini week");
        assert_eq!(rows[1].label, "Future provider label");
        assert_eq!(rows[1].compact_label, "Future provider label");
    }

    #[test]
    fn limiting_rows_come_from_effective_availability_without_hiding_other_rows() {
        let mut quota = provider("claude", vec![window("five_hour"), window("seven_day")]);
        quota.effective.push(EffectiveAvailability {
            scope: "all_models".into(),
            status: EffectiveStatus::Known,
            effective_percent_remaining: Some(0.0),
            bounded_by: vec![],
            limiting_window_ids: vec!["seven_day".into()],
            pace: None,
            runway: None,
        });
        let rows = provider_tiers(&quota);
        assert_eq!(rows.iter().filter(|row| row.limiting).count(), 1);
        assert!(rows.iter().any(|row| row.id == "five_hour"));
    }

    #[test]
    fn recovery_uses_each_owners_safe_command() {
        for kind in MarketedProvider::ALL {
            let mut quota = provider(kind.id(), vec![]);
            quota.state.status = ProviderStatus::AuthRequired;
            assert_eq!(
                present_provider(&quota),
                ProviderPresentation::Recovery {
                    instruction: kind.recovery_instruction()
                }
            );
        }
    }

    #[test]
    fn grok_cli_only_access_is_not_a_zero_quota_reading() {
        let quota = provider("grok", vec![]);
        assert_eq!(
            present_provider(&quota),
            ProviderPresentation::Message {
                message: "Consumer quota unavailable"
            }
        );
        assert_eq!(
            provider_annotation(&quota).unwrap().text,
            "consumer quota unavailable"
        );
    }
}
