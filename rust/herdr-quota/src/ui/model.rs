//! Renderer-independent dashboard data assembled from safe domain values.

use std::collections::BTreeMap;

use crate::domain::attention::{Attention, select_attention_from};
use crate::domain::provider::MarketedProvider;
use crate::domain::schema::QuotaReport;
use crate::domain::tiers::{
    ProviderAnnotation, ProviderPresentation, present_provider, provider_annotation,
};
use crate::ui::bar::{MeterMode, displayed_percent};

/// Why a provider is absent from the visible dashboard. `UserDisabled` is an
/// explicit choice and must never be rolled into a hidden-provider disclosure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderVisibility {
    Visible,
    UserDisabled,
    HiddenElsewhere,
}

pub type ProviderVisibilityMap = BTreeMap<MarketedProvider, ProviderVisibility>;

/// A renderer-neutral tier with the percentage already selected for its meter.
#[derive(Clone, Debug, PartialEq)]
pub struct TierView {
    pub id: String,
    pub label: String,
    pub compact_label: String,
    pub percent_remaining: Option<f64>,
    pub displayed_percent: Option<f64>,
    pub resets_at: Option<String>,
    pub conclusion: crate::domain::tiers::TierConclusion,
    pub limiting: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ProviderDetail {
    Tiers(Vec<TierView>),
    Recovery { instruction: &'static str },
    Message { message: &'static str },
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProviderSection {
    pub provider: MarketedProvider,
    pub label: &'static str,
    pub annotation: Option<ProviderAnnotation>,
    pub detail: ProviderDetail,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HiddenProviderSummary {
    pub count: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DashboardModel {
    pub attention: Attention,
    pub providers: Vec<ProviderSection>,
    /// `None` means there is no non-user-disabled omission to disclose.
    pub hidden_summary: Option<HiddenProviderSummary>,
}

fn visibility_for(
    visibility: &ProviderVisibilityMap,
    provider: MarketedProvider,
) -> ProviderVisibility {
    visibility
        .get(&provider)
        .copied()
        .unwrap_or(ProviderVisibility::Visible)
}

fn tier_detail(presentation: ProviderPresentation, meter_mode: MeterMode) -> ProviderDetail {
    match presentation {
        ProviderPresentation::Tiers(rows) => ProviderDetail::Tiers(
            rows.into_iter()
                .map(|row| TierView {
                    displayed_percent: displayed_percent(row.percent_remaining, meter_mode),
                    id: row.id,
                    label: row.label,
                    compact_label: row.compact_label,
                    percent_remaining: row.percent_remaining,
                    resets_at: row.resets_at,
                    conclusion: row.conclusion,
                    limiting: row.limiting,
                })
                .collect(),
        ),
        ProviderPresentation::Recovery { instruction } => ProviderDetail::Recovery { instruction },
        ProviderPresentation::Message { message } => ProviderDetail::Message { message },
    }
}

/// Creates the complete pure model. It intentionally owns no settings or
/// persistence: callers provide the finite visibility decision for each
/// marketed provider.
pub fn dashboard_model(
    report: &QuotaReport,
    meter_mode: MeterMode,
    visibility: &ProviderVisibilityMap,
) -> DashboardModel {
    let providers = report
        .providers
        .iter()
        .filter_map(|quota| {
            let provider = MarketedProvider::from_id(&quota.provider.to_ascii_lowercase())?;
            (visibility_for(visibility, provider) == ProviderVisibility::Visible)
                .then_some((provider, quota))
        })
        .collect::<Vec<_>>();
    let hidden_count = report
        .providers
        .iter()
        .filter(|quota| {
            MarketedProvider::from_id(&quota.provider.to_ascii_lowercase()).is_none_or(|provider| {
                visibility_for(visibility, provider) == ProviderVisibility::HiddenElsewhere
            })
        })
        .count();
    let sections = providers
        .iter()
        .map(|(provider, quota)| ProviderSection {
            provider: *provider,
            label: provider.label(),
            annotation: provider_annotation(quota),
            detail: tier_detail(present_provider(quota), meter_mode),
        })
        .collect();
    DashboardModel {
        attention: select_attention_from(providers.iter().map(|(_, quota)| *quota)),
        providers: sections,
        hidden_summary: (hidden_count > 0).then_some(HiddenProviderSummary {
            count: hidden_count,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::provider::{
        ProviderQuota, ProviderState, ProviderStatus, QuotaWindow, SemanticsStatus,
    };

    fn quota(provider: &str, remaining: Option<f64>) -> ProviderQuota {
        ProviderQuota {
            provider: provider.into(),
            label: Some("untrusted label".into()),
            source: None,
            plan: None,
            windows: vec![QuotaWindow {
                id: "weekly".into(),
                label: "weekly".into(),
                kind: "limit".into(),
                percent_used: None,
                percent_remaining: remaining,
                starts_at: None,
                resets_at: None,
                reset_text: None,
                window_seconds: None,
                spent_usd: None,
                limit_usd: None,
                pace: None,
            }],
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

    fn report(providers: Vec<ProviderQuota>) -> QuotaReport {
        QuotaReport {
            generated_at: "2026-09-02T00:00:00.000Z".into(),
            schema_version: 5,
            providers,
            adaptation_warnings: vec![],
        }
    }

    #[test]
    fn user_disabled_providers_are_never_summarized() {
        let mut visibility = ProviderVisibilityMap::new();
        visibility.insert(MarketedProvider::Claude, ProviderVisibility::UserDisabled);
        let model = dashboard_model(
            &report(vec![quota("claude", Some(40.0))]),
            MeterMode::Remaining,
            &visibility,
        );
        assert!(model.providers.is_empty());
        assert_eq!(model.hidden_summary, None);
    }

    #[test]
    fn only_other_hidden_reasons_are_disclosed_and_zero_has_no_line() {
        let report = report(vec![
            quota("claude", Some(40.0)),
            quota("cursor", Some(80.0)),
        ]);
        let mut visibility = ProviderVisibilityMap::new();
        visibility.insert(MarketedProvider::Claude, ProviderVisibility::UserDisabled);
        visibility.insert(
            MarketedProvider::Cursor,
            ProviderVisibility::HiddenElsewhere,
        );
        let model = dashboard_model(&report, MeterMode::Remaining, &visibility);
        assert_eq!(
            model.hidden_summary,
            Some(HiddenProviderSummary { count: 1 })
        );

        visibility.insert(MarketedProvider::Cursor, ProviderVisibility::Visible);
        assert_eq!(
            dashboard_model(&report, MeterMode::Remaining, &visibility).hidden_summary,
            None
        );
    }

    #[test]
    fn unrecognized_provider_records_are_disclosed_as_hidden_elsewhere() {
        let model = dashboard_model(
            &report(vec![quota("provider-1", None)]),
            MeterMode::Remaining,
            &ProviderVisibilityMap::new(),
        );
        assert!(model.providers.is_empty());
        assert_eq!(
            model.hidden_summary,
            Some(HiddenProviderSummary { count: 1 })
        );
    }

    #[test]
    fn meter_mode_uses_the_complement_without_turning_unknown_into_zero() {
        let report = report(vec![quota("claude", Some(31.0)), quota("cursor", None)]);
        let model = dashboard_model(&report, MeterMode::Used, &ProviderVisibilityMap::new());
        let ProviderDetail::Tiers(claude) = &model.providers[0].detail else {
            panic!("tiers expected")
        };
        let ProviderDetail::Tiers(cursor) = &model.providers[1].detail else {
            panic!("tiers expected")
        };
        assert_eq!(claude[0].displayed_percent, Some(69.0));
        assert_eq!(cursor[0].displayed_percent, None);
    }

    #[test]
    fn sections_always_use_explicit_marketed_labels() {
        let model = dashboard_model(
            &report(vec![quota("copilot", Some(50.0))]),
            MeterMode::Remaining,
            &ProviderVisibilityMap::new(),
        );
        assert_eq!(model.providers[0].label, "GitHub Copilot");
    }
}
