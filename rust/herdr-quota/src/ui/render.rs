//! A bounded Ratatui dashboard renderer.
//!
//! The renderer accepts only adapted schema-v5 data. It deliberately produces
//! whole semantic rows before placing them in a fixed-height frame, so scrolling
//! cannot split a provider heading from the row model or leak collector text.

use std::collections::BTreeSet;
use std::io::{self, Stdout};

use chrono::{Datelike, Timelike};
use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    widgets::Widget,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::domain::{
    provider::{
        EffectiveAvailability, EffectiveStatus, MarketedProvider, PaceStatus, ProjectionConfidence,
        ProviderQuota, ProviderStatus, RunwayStatus, SemanticsStatus,
    },
    schema::QuotaReport,
    tiers::TierConclusion,
};
use crate::store::settings::{DashboardSettings, SettingsStore, StartupView};
use crate::ui::{
    bar::MeterMode,
    model::{ProviderDetail, ProviderVisibility, ProviderVisibilityMap, dashboard_model},
};

/// The finite dashboard surface currently shown in the terminal.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DashboardView {
    #[default]
    Overview,
    Details,
    Preferences,
    TransitionReview,
}

/// Local rendering inputs; collection and quota semantics do not depend on them.
#[derive(Clone, Debug, Default)]
pub struct DashboardConfig {
    /// IDs explicitly hidden by the user. They never inflate availability summary text.
    pub user_hidden: BTreeSet<String>,
    /// The first semantic row visible in the scrollable region.
    pub scroll: usize,
    /// Whether optional Ratatui colour may reinforce the textual markers.
    pub color: bool,
    /// The current finite dashboard surface.
    pub view: DashboardView,
    /// The stable cursor into the visible provider roster.
    pub selected_provider: usize,
    /// The editable startup preference shown by Preferences.
    pub startup_view: StartupView,
    saved_startup_view: StartupView,
    return_view: DashboardView,
    save_failed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SemanticRow {
    text: String,
    style: RowStyle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowStyle {
    Normal,
    Heading,
    Warning,
    Critical,
}

impl RowStyle {
    fn ratatui(self, color: bool) -> Style {
        if !color {
            return Style::default();
        }
        match self {
            Self::Normal => Style::default(),
            Self::Heading => Style::default().add_modifier(Modifier::BOLD),
            Self::Warning => Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(Color::Yellow),
            Self::Critical => Style::default().add_modifier(Modifier::BOLD).fg(Color::Red),
        }
    }
}

fn provider_id(provider: &ProviderQuota) -> Option<MarketedProvider> {
    MarketedProvider::from_id(&provider.provider.to_ascii_lowercase())
}

fn has_current_quota(provider: &ProviderQuota) -> bool {
    provider.state.status == ProviderStatus::Fresh
        && !provider.state.stale
        && !matches!(
            provider.state.auth_status.as_deref(),
            Some("unusable" | "expired_refreshable")
        )
}

fn has_trustworthy_quota(provider: &ProviderQuota) -> bool {
    has_current_quota(provider)
        && provider.semantics_status == Some(SemanticsStatus::Known)
        && provider.effective.iter().any(decision_grade)
}

fn decision_grade(effective: &EffectiveAvailability) -> bool {
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

fn limiting_window_id(effective: &EffectiveAvailability) -> Option<&str> {
    effective
        .runway
        .as_ref()
        .and_then(|runway| runway.limiting_window_id.as_deref())
        .or_else(|| {
            effective
                .pace
                .as_ref()
                .and_then(|pace| pace.worst_reserve_window_id.as_deref())
        })
        .or_else(|| effective.limiting_window_ids.first().map(String::as_str))
}

fn timestamp(value: Option<&str>) -> i64 {
    value
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.timestamp_millis())
        .unwrap_or(i64::MAX)
}

fn percent(value: Option<f64>) -> String {
    value
        .map(|value| format!("{:>3}%", value.round() as i64))
        .unwrap_or_else(|| " --".into())
}

fn gauge(value: Option<f64>, cells: usize) -> String {
    let Some(value) = value else {
        return "".to_owned();
    };
    let fill = ((value.clamp(0.0, 100.0) / 100.0) * cells as f64).round() as usize;
    format!(
        "{}{}",
        "#".repeat(fill),
        "-".repeat(cells.saturating_sub(fill))
    )
}

fn hidden_unavailable_count(report: &QuotaReport, config: &DashboardConfig) -> usize {
    let report_ids: BTreeSet<_> = report
        .providers
        .iter()
        .filter_map(provider_id)
        .map(MarketedProvider::id)
        .collect();
    MarketedProvider::ALL
        .iter()
        .filter(|provider| {
            !config.user_hidden.contains(provider.id()) && !report_ids.contains(provider.id())
        })
        .count()
}

fn visible_model(
    report: &QuotaReport,
    config: &DashboardConfig,
) -> crate::ui::model::DashboardModel {
    let visibility = config
        .user_hidden
        .iter()
        .filter_map(|id| MarketedProvider::from_id(id))
        .map(|provider| (provider, ProviderVisibility::UserDisabled))
        .collect::<ProviderVisibilityMap>();
    dashboard_model(report, MeterMode::Remaining, &visibility)
}

fn compact_provider_name(provider: MarketedProvider) -> &'static str {
    match provider {
        MarketedProvider::Codex => "Codex",
        MarketedProvider::Copilot => "Copilot",
        other => other.label(),
    }
}

fn effective_risk(effective: &EffectiveAvailability) -> u8 {
    match effective.runway.as_ref().map(|runway| runway.status) {
        Some(RunwayStatus::ExhaustedNow) => 0,
        Some(RunwayStatus::ProjectedExhaustion)
            if effective
                .runway
                .as_ref()
                .and_then(|runway| runway.projection_confidence)
                == Some(ProjectionConfidence::Established) =>
        {
            1
        }
        _ if effective
            .pace
            .as_ref()
            .is_some_and(|pace| matches!(pace.status, PaceStatus::Ahead | PaceStatus::Mixed)) =>
        {
            2
        }
        _ => 3,
    }
}

fn limiting_effective(provider: &ProviderQuota) -> Option<&EffectiveAvailability> {
    provider
        .effective
        .iter()
        .enumerate()
        .filter(|(_, effective)| decision_grade(effective))
        .min_by(|(left_order, left), (right_order, right)| {
            effective_risk(left)
                .cmp(&effective_risk(right))
                .then_with(|| {
                    timestamp(
                        left.runway
                            .as_ref()
                            .and_then(|runway| runway.projected_exhausted_at.as_deref()),
                    )
                    .cmp(&timestamp(
                        right
                            .runway
                            .as_ref()
                            .and_then(|runway| runway.projected_exhausted_at.as_deref()),
                    ))
                })
                .then_with(|| {
                    left.effective_percent_remaining
                        .unwrap_or(101.0)
                        .total_cmp(&right.effective_percent_remaining.unwrap_or(101.0))
                })
                .then_with(|| left_order.cmp(right_order))
        })
        .map(|(_, effective)| effective)
}

fn reset_date(value: Option<&str>, compact: bool) -> Option<String> {
    let date = chrono::DateTime::parse_from_rfc3339(value?).ok()?;
    Some(if compact {
        format!("{}/{}", date.month(), date.day())
    } else {
        format!("{:02}/{:02}", date.month(), date.day())
    })
}

fn moment(value: Option<&str>, compact: bool) -> Option<String> {
    let date = chrono::DateTime::parse_from_rfc3339(value?).ok()?;
    Some(if compact {
        format!(
            "{}/{} {:02}:{:02}",
            date.month(),
            date.day(),
            date.hour(),
            date.minute()
        )
    } else {
        format!(
            "{:02}/{:02} {:02}:{:02}",
            date.month(),
            date.day(),
            date.hour(),
            date.minute()
        )
    })
}

fn fitting(candidates: impl IntoIterator<Item = String>, width: usize) -> String {
    candidates
        .into_iter()
        .find(|candidate| UnicodeWidthStr::width(candidate.as_str()) <= width)
        .unwrap_or_default()
}

fn whole_token_prefix(value: &str, width: usize) -> String {
    let mut result = String::new();
    for token in value.split_whitespace() {
        let candidate = if result.is_empty() {
            token.to_owned()
        } else {
            format!("{result} {token}")
        };
        if UnicodeWidthStr::width(candidate.as_str()) > width {
            break;
        }
        result = candidate;
    }
    result
}

fn frame_text(value: &str, width: usize) -> String {
    if UnicodeWidthStr::width(value) <= width {
        value.to_owned()
    } else {
        whole_token_prefix(value, width)
    }
}

fn hidden_sibling_unsafe(
    section: &crate::ui::model::ProviderSection,
    provider: &ProviderQuota,
) -> bool {
    provider.semantics_status == Some(SemanticsStatus::Partial)
        || provider
            .effective
            .iter()
            .any(|effective| effective.status != EffectiveStatus::Known)
        || matches!(&section.detail, ProviderDetail::Tiers(tiers) if tiers
            .iter()
            .any(|tier| matches!(tier.conclusion, TierConclusion::NotReported)))
}

fn has_decision_safe_quota(
    section: &crate::ui::model::ProviderSection,
    provider: &ProviderQuota,
) -> bool {
    has_trustworthy_quota(provider) && !hidden_sibling_unsafe(section, provider)
}

fn overview_row(
    section: &crate::ui::model::ProviderSection,
    provider: &ProviderQuota,
    selected: bool,
    width: usize,
) -> SemanticRow {
    let current = has_current_quota(provider);
    let annotation = section.annotation.as_ref().map(|value| value.text);
    let tiers = match &section.detail {
        ProviderDetail::Tiers(tiers) => Some(tiers.as_slice()),
        ProviderDetail::Recovery { .. } | ProviderDetail::Message { .. } => None,
    };
    let has_hidden_unknown = hidden_sibling_unsafe(section, provider);

    let (marker, state, tier, reset, displayed, style) = if !current || tiers.is_none() {
        (
            '?',
            annotation.unwrap_or("non-current").to_owned(),
            None,
            None,
            None,
            RowStyle::Warning,
        )
    } else if has_hidden_unknown {
        (
            '?',
            "? partial".to_owned(),
            None,
            None,
            None,
            RowStyle::Warning,
        )
    } else if provider.semantics_status != Some(SemanticsStatus::Known) {
        (
            '?',
            "? unknown".to_owned(),
            None,
            None,
            None,
            RowStyle::Warning,
        )
    } else if let Some(effective) = limiting_effective(provider) {
        let limiting_id = limiting_window_id(effective);
        let tier = tiers
            .and_then(|tiers| limiting_id.and_then(|id| tiers.iter().find(|tier| tier.id == id)));
        let remaining = effective.effective_percent_remaining;
        let critical =
            effective_risk(effective) <= 1 || remaining.is_some_and(|value| value <= 10.0);
        let warning = effective_risk(effective) == 2;
        (
            if critical {
                '!'
            } else if warning {
                '?'
            } else {
                '='
            },
            percent(remaining).trim().to_owned(),
            tier.filter(|tier| {
                !(section.provider == MarketedProvider::Cursor && tier.id == "api_usage")
            })
            .map(|tier| tier.compact_label.as_str()),
            reset_date(tier.and_then(|tier| tier.resets_at.as_deref()), width <= 23),
            remaining,
            if critical {
                RowStyle::Critical
            } else if warning {
                RowStyle::Warning
            } else {
                RowStyle::Normal
            },
        )
    } else {
        (
            '?',
            "? unknown".to_owned(),
            None,
            None,
            None,
            RowStyle::Warning,
        )
    };

    let cursor = if selected { '>' } else { ' ' };
    let name = compact_provider_name(section.provider);
    let prefix = format!("{cursor}{marker}{name}");
    let tier = tier.map(str::to_owned);
    let mut candidates = Vec::new();
    if width >= 24 {
        if let Some(displayed) = displayed {
            candidates.push(format!(
                "{prefix} [{}] {state}{}{}",
                gauge(Some(displayed), 6),
                tier.as_deref()
                    .map(|value| format!(" {value}"))
                    .unwrap_or_default(),
                reset
                    .as_deref()
                    .map(|value| format!(" {value}"))
                    .unwrap_or_default(),
            ));
        }
    }
    let text_priority = if width <= 23 {
        [
            (tier.as_deref(), reset.as_deref()),
            (None, reset.as_deref()),
            (tier.as_deref(), None),
            (None, None),
        ]
    } else {
        [
            (tier.as_deref(), reset.as_deref()),
            (tier.as_deref(), None),
            (None, reset.as_deref()),
            (None, None),
        ]
    };
    for (shown_tier, shown_reset) in text_priority {
        candidates.push(format!(
            "{prefix} {state}{}{}",
            shown_tier
                .map(|value| format!(" {value}"))
                .unwrap_or_default(),
            shown_reset
                .map(|value| format!(" {value}"))
                .unwrap_or_default(),
        ));
    }
    let text = candidates
        .into_iter()
        .find(|candidate| UnicodeWidthStr::width(candidate.as_str()) <= width)
        .unwrap_or_else(|| {
            if matches!(state.as_str(), "? partial" | "? unknown") {
                let narrow_name = if section.provider == MarketedProvider::Copilot {
                    "GitHub"
                } else {
                    name
                };
                return truncate(&format!("{cursor}{narrow_name}{state}"), width);
            }
            let compact_state = match state.as_str() {
                "signed out" => "out",
                "rate limited" => "rate",
                "unavailable" => "down",
                "non-current" => "old",
                "no reading" | "consumer quota unavailable" => "none",
                other => other,
            };
            fitting(
                [
                    format!("{prefix} {compact_state}"),
                    format!("{prefix}{compact_state}"),
                    prefix,
                ],
                width,
            )
        });
    SemanticRow { text, style }
}

fn overview_rows(report: &QuotaReport, config: &DashboardConfig, width: u16) -> Vec<SemanticRow> {
    let model = visible_model(report, config);
    let mut rows = Vec::new();
    let selected = config
        .selected_provider
        .min(model.providers.len().saturating_sub(1));
    for (index, section) in model.providers.iter().enumerate() {
        let Some(provider) = report
            .providers
            .iter()
            .find(|provider| provider_id(provider) == Some(section.provider))
        else {
            continue;
        };
        rows.push(overview_row(
            section,
            provider,
            index == selected,
            width as usize,
        ));
    }
    let hidden_unavailable = hidden_unavailable_count(report, config);
    if hidden_unavailable > 0 {
        let noun = if hidden_unavailable == 1 {
            "provider"
        } else {
            "providers"
        };
        rows.push(SemanticRow {
            text: fitting(
                [
                    format!("? {hidden_unavailable} unavailable {noun} hidden"),
                    format!("? {hidden_unavailable} unavailable hidden"),
                    format!("? {hidden_unavailable} unavailable"),
                ],
                width as usize,
            ),
            style: RowStyle::Warning,
        });
    }
    rows.extend(overview_evidence_rows(report, config, width));
    rows
}

fn overview_evidence_rows(
    report: &QuotaReport,
    config: &DashboardConfig,
    width: u16,
) -> Vec<SemanticRow> {
    let model = visible_model(report, config);
    let selected = model
        .providers
        .get(
            config
                .selected_provider
                .min(model.providers.len().saturating_sub(1)),
        )
        .map(|section| section.provider);
    let mut rows = Vec::new();
    for section in &model.providers {
        let Some(provider) = report
            .providers
            .iter()
            .find(|provider| provider_id(provider) == Some(section.provider))
        else {
            continue;
        };
        if !has_decision_safe_quota(section, provider) {
            continue;
        }
        let Some(effective) = limiting_effective(provider) else {
            continue;
        };
        let name = compact_provider_name(section.provider);
        let compact = width <= 23;
        let limiting = limiting_window_id(effective)
            .and_then(|id| provider.windows.iter().find(|window| window.id == id));
        if let Some(reset) = moment(
            limiting.and_then(|window| window.resets_at.as_deref()),
            compact,
        ) {
            rows.push(SemanticRow {
                text: fitting(
                    [
                        format!("  {name} reset {reset}"),
                        format!("  reset {reset} · {name}"),
                        format!(
                            "  {name} reset {}",
                            reset
                                .split_once(' ')
                                .map_or(reset.as_str(), |(date, _)| date)
                        ),
                    ],
                    width as usize,
                ),
                style: RowStyle::Normal,
            });
        }
        let runway = effective.runway.as_ref();
        if runway.is_some_and(|runway| {
            runway.status == RunwayStatus::ProjectedExhaustion
                && runway.projection_confidence == Some(ProjectionConfidence::Established)
        }) && selected != Some(section.provider)
        {
            let when = moment(
                runway.and_then(|runway| runway.projected_exhausted_at.as_deref()),
                compact,
            )
            .unwrap_or_else(|| "before reset".into());
            let date = when.split_once(' ').map_or(when.as_str(), |(date, _)| date);
            rows.push(SemanticRow {
                text: fitting(
                    [
                        format!("! {name} out {when}"),
                        format!("!{name} out {date}"),
                        format!("! {name} out"),
                    ],
                    width as usize,
                ),
                style: RowStyle::Critical,
            });
        } else if runway.is_some_and(|runway| runway.status == RunwayStatus::ThroughReset) {
            rows.push(SemanticRow {
                text: fitting(
                    [
                        format!("= {name} through reset"),
                        format!("= {name} on pace"),
                    ],
                    width as usize,
                ),
                style: RowStyle::Normal,
            });
        }
    }
    rows.retain(|row| !row.text.is_empty());
    rows
}

fn detail_rows(report: &QuotaReport, config: &DashboardConfig, width: u16) -> Vec<SemanticRow> {
    let model = visible_model(report, config);
    let Some(section) = model.providers.get(
        config
            .selected_provider
            .min(model.providers.len().saturating_sub(1)),
    ) else {
        return Vec::new();
    };
    let mut rows = Vec::new();
    {
        let current = report
            .providers
            .iter()
            .find(|provider| provider_id(provider) == Some(section.provider))
            .is_some_and(has_current_quota);
        let annotation = section.annotation.as_ref().map(|value| value.text);
        rows.push(SemanticRow {
            text: annotation
                .map(|value| {
                    fitting(
                        [
                            format!("> {} · {value}", section.label),
                            format!("> {}", section.label),
                        ],
                        width as usize,
                    )
                })
                .unwrap_or_else(|| format!("> {}", section.label)),
            style: if annotation.is_some() {
                RowStyle::Warning
            } else {
                RowStyle::Heading
            },
        });
        match &section.detail {
            ProviderDetail::Recovery { instruction } => rows.push(SemanticRow {
                text: fitting(
                    [format!("  ? {instruction}"), "  ? sign in".into()],
                    width as usize,
                ),
                style: RowStyle::Warning,
            }),
            ProviderDetail::Message { message } => {
                let compact = whole_token_prefix(message, width.saturating_sub(4) as usize);
                let compact = if compact.is_empty() {
                    "  ? unavailable".into()
                } else {
                    format!("  ? {compact}")
                };
                rows.push(SemanticRow {
                    text: fitting(
                        [
                            format!("  ? {message}"),
                            compact,
                            "  ? unavailable".into(),
                        ],
                        width as usize,
                    ),
                    style: RowStyle::Warning,
                })
            }
            ProviderDetail::Tiers(tiers) => {
                for tier in tiers {
                    let percent = percent(tier.percent_remaining);
                    let label_source = if width >= 30 {
                        &tier.label
                    } else {
                        &tier.compact_label
                    };
                    let label_limit = if width >= 30 { 15 } else { 9 };
                    let label = whole_token_prefix(label_source, label_limit);
                    if !current {
                        rows.push(SemanticRow {
                            text: fitting(
                                [
                                    format!(" ~ last known {label} {percent}"),
                                    format!(" ~ {label} {percent}"),
                                    format!(" ~ {percent}"),
                                ],
                                width as usize,
                            ),
                            style: RowStyle::Warning,
                        });
                        continue;
                    }
                    let candidate_conclusion = match tier.conclusion {
                        TierConclusion::NotReported => " · not reported",
                        TierConclusion::OnPace => " · on pace",
                        TierConclusion::Ahead { .. } => " · ahead",
                        TierConclusion::Spend { .. } => " · spend",
                        TierConclusion::Unknown => "",
                    };
                    let critical = tier.percent_remaining.is_some_and(|value| value <= 10.0);
                    let warning = matches!(tier.conclusion, TierConclusion::NotReported);
                    let marker = if critical {
                        "!"
                    } else if warning {
                        "?"
                    } else {
                        " "
                    };
                    let label_budget = if width >= 30 { 15 } else { 9 };
                    let aligned = format!(" {marker}{label:<label_budget$} {percent}");
                    let compact = if label.is_empty() {
                        format!(" {marker}{percent}")
                    } else {
                        format!(" {marker}{label} {percent}")
                    };
                    let meter = (width >= 30 && tier.displayed_percent.is_some())
                        .then(|| format!(" [{}]", gauge(tier.displayed_percent, 6)))
                        .unwrap_or_default();
                    rows.push(SemanticRow {
                        text: fitting(
                            [
                                format!("{aligned}{candidate_conclusion}"),
                                format!("{aligned}{meter}"),
                                aligned,
                                format!("{compact}{candidate_conclusion}"),
                                compact,
                                format!(" {marker}{percent}"),
                            ],
                            width as usize,
                        ),
                        style: if critical {
                            RowStyle::Critical
                        } else if warning {
                            RowStyle::Warning
                        } else {
                            RowStyle::Normal
                        },
                    });
                }
            }
        }
    }
    rows
}

fn semantic_rows(report: &QuotaReport, config: &DashboardConfig, width: u16) -> Vec<SemanticRow> {
    match config.view {
        DashboardView::Overview => overview_rows(report, config, width),
        DashboardView::Details => detail_rows(report, config, width),
        DashboardView::Preferences | DashboardView::TransitionReview => Vec::new(),
    }
}

fn attention(report: &QuotaReport, config: &DashboardConfig, width: usize) -> (String, RowStyle) {
    let model = visible_model(report, config);
    let visible: Vec<_> = model
        .providers
        .iter()
        .filter_map(|section| {
            report
                .providers
                .iter()
                .find(|provider| provider_id(provider) == Some(section.provider))
                .map(|provider| (section, provider))
        })
        .collect();
    if visible.is_empty() {
        return ("? No providers shown".into(), RowStyle::Warning);
    }
    let trustworthy: Vec<_> = visible
        .iter()
        .filter_map(|(section, provider)| {
            has_decision_safe_quota(section, provider).then_some(*provider)
        })
        .collect();
    let constraints = trustworthy
        .iter()
        .enumerate()
        .flat_map(|(provider_order, provider)| {
            provider
                .effective
                .iter()
                .enumerate()
                .filter_map(move |(effective_order, effective)| {
                    if !decision_grade(effective) {
                        return None;
                    }
                    let runway = effective.runway.as_ref()?;
                    let rank = match runway.status {
                        RunwayStatus::ExhaustedNow => 0,
                        RunwayStatus::ProjectedExhaustion
                            if runway.projection_confidence
                                == Some(ProjectionConfidence::Established) =>
                        {
                            1
                        }
                        _ => return None,
                    };
                    let time = if rank == 1 {
                        timestamp(runway.projected_exhausted_at.as_deref())
                    } else {
                        limiting_window_id(effective)
                            .and_then(|id| provider.windows.iter().find(|window| window.id == id))
                            .map(|window| timestamp(window.resets_at.as_deref()))
                            .unwrap_or(i64::MAX)
                    };
                    Some((
                        rank,
                        time,
                        effective.effective_percent_remaining.unwrap_or(101.0),
                        provider_order,
                        effective_order,
                        provider,
                        effective,
                    ))
                })
        })
        .min_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| {
                    if left.0 == 1 {
                        left.1
                            .cmp(&right.1)
                            .then_with(|| left.2.total_cmp(&right.2))
                    } else {
                        left.2
                            .total_cmp(&right.2)
                            .then_with(|| right.1.cmp(&left.1))
                    }
                })
                .then_with(|| left.3.cmp(&right.3))
                .then_with(|| left.4.cmp(&right.4))
        });
    if let Some((_, _, _, _, _, provider, effective)) = constraints {
        let limiting_id = limiting_window_id(effective);
        let runway = effective
            .runway
            .as_ref()
            .expect("constraint requires runway");
        let consequences = match runway.status {
            RunwayStatus::ProjectedExhaustion => {
                moment(runway.projected_exhausted_at.as_deref(), width <= 23)
                    .map(|when| {
                        vec![
                            format!("out {when}"),
                            format!(
                                "out {}",
                                when.split_once(' ').map_or(when.as_str(), |(date, _)| date)
                            ),
                            "out before reset".into(),
                        ]
                    })
                    .unwrap_or_else(|| vec!["out before reset".into()])
            }
            RunwayStatus::ExhaustedNow => {
                let reset = limiting_id
                    .and_then(|id| provider.windows.iter().find(|window| window.id == id))
                    .and_then(|window| moment(window.resets_at.as_deref(), width <= 23));
                reset
                    .map(|when| {
                        vec![
                            format!("out now · reset {when}"),
                            format!(
                                "out now · reset {}",
                                when.split_once(' ').map_or(when.as_str(), |(date, _)| date)
                            ),
                            "out now".into(),
                        ]
                    })
                    .unwrap_or_else(|| vec!["out now".into()])
            }
            RunwayStatus::ThroughReset | RunwayStatus::Unknown => vec!["needs review".into()],
        };
        return (
            fitting(
                consequences
                    .into_iter()
                    .map(|consequence| format!("! {consequence}")),
                width,
            ),
            RowStyle::Critical,
        );
    }
    let current_effective: Vec<_> = trustworthy
        .iter()
        .flat_map(|provider| {
            provider
                .effective
                .iter()
                .filter(|item| decision_grade(item))
        })
        .collect();
    if current_effective.iter().any(|effective| {
        effective
            .pace
            .as_ref()
            .is_some_and(|pace| matches!(pace.status, PaceStatus::Ahead | PaceStatus::Mixed))
    }) {
        return (
            fitting(
                ["? Pace needs review".into(), "? Pace review".into()],
                width,
            ),
            RowStyle::Warning,
        );
    }
    if visible
        .iter()
        .any(|(_, provider)| !has_current_quota(provider))
    {
        return (
            fitting(
                ["? Limits non-current".into(), "? Non-current".into()],
                width,
            ),
            RowStyle::Warning,
        );
    }
    if visible
        .iter()
        .any(|(section, provider)| !has_decision_safe_quota(section, provider))
    {
        return (
            fitting(
                ["? Quota data partial".into(), "? Data partial".into()],
                width,
            ),
            RowStyle::Warning,
        );
    }
    if !current_effective.is_empty()
        && current_effective.iter().all(|effective| {
            effective
                .runway
                .as_ref()
                .is_some_and(|runway| runway.status == RunwayStatus::ThroughReset)
                || effective.pace.as_ref().is_some_and(|pace| {
                    matches!(pace.status, PaceStatus::Behind | PaceStatus::OnPace)
                })
        })
    {
        return ("= Limits on pace".into(), RowStyle::Normal);
    }
    (
        fitting(
            ["? Quota data partial".into(), "? Data partial".into()],
            width,
        ),
        RowStyle::Warning,
    )
}

fn position(scroll: usize, total: usize, viewport: usize) -> String {
    if total == 0 {
        return "Rows 0–0 of 0".into();
    }
    let start = scroll.min(total.saturating_sub(1)) + 1;
    let end = (start - 1 + viewport).min(total);
    format!("Rows {start}–{end} of {total}")
}

/// Return exactly `height` plain, cell-width-bounded lines for a report.
pub fn render_lines(
    report: &QuotaReport,
    width: u16,
    height: u16,
    config: &DashboardConfig,
) -> Vec<String> {
    render_frame(report, width, height, config)
        .into_iter()
        .map(|row| row.text)
        .collect()
}

fn render_frame(
    report: &QuotaReport,
    width: u16,
    height: u16,
    config: &DashboardConfig,
) -> Vec<SemanticRow> {
    let width = width.max(1) as usize;
    let height = height.max(1) as usize;
    if config.view == DashboardView::Preferences {
        return render_preferences(width, height, config);
    }
    let rows = match config.view {
        DashboardView::Overview | DashboardView::Details => {
            semantic_rows(report, config, width as u16)
        }
        DashboardView::TransitionReview => vec![SemanticRow {
            text: "No new transition events".into(),
            style: RowStyle::Normal,
        }],
        DashboardView::Preferences => unreachable!("handled above"),
    };
    let title = if config.view == DashboardView::Details {
        let model = visible_model(report, config);
        model
            .providers
            .get(
                config
                    .selected_provider
                    .min(model.providers.len().saturating_sub(1)),
            )
            .map(|section| {
                fitting(
                    [
                        format!("Herdr Quota · {}", compact_provider_name(section.provider)),
                        "Herdr Quota".into(),
                    ],
                    width,
                )
            })
            .unwrap_or_else(|| "Herdr Quota".into())
    } else if config.view == DashboardView::TransitionReview {
        "Transition review".into()
    } else {
        "Herdr Quota".into()
    };
    let (attention, attention_style) = attention(report, config, width);
    let controls = match (config.view, width >= 30) {
        (DashboardView::Overview, true) => "j/k · enter details · p · q",
        (DashboardView::Overview, false) => "j/k enter p q",
        (DashboardView::Details, true) => "j/k · esc overview · q",
        (DashboardView::Details, false) => "j/k esc q",
        (DashboardView::TransitionReview, true) => "a/enter acknowledge · esc",
        (DashboardView::TransitionReview, false) => "a/enter ack · esc",
        (DashboardView::Preferences, _) => unreachable!("handled above"),
    };
    let body_start = if config.view == DashboardView::Details && height >= 5 {
        3
    } else {
        2.min(height)
    };
    let footer = height.saturating_sub(1);
    let body_end = footer.max(body_start);
    let viewport = body_end.saturating_sub(body_start);
    let scroll = if config.view == DashboardView::Overview {
        config
            .selected_provider
            .saturating_add(1)
            .saturating_sub(viewport)
            .min(rows.len().saturating_sub(viewport))
    } else {
        config.scroll.min(rows.len().saturating_sub(viewport))
    };
    let mut output = vec![
        SemanticRow {
            text: String::new(),
            style: RowStyle::Normal
        };
        height
    ];
    output[0] = SemanticRow {
        text: frame_text(&title, width),
        style: RowStyle::Heading,
    };
    if height > 1 {
        output[1] = SemanticRow {
            text: frame_text(&attention, width),
            style: attention_style,
        };
    }
    if height > 2 && config.view == DashboardView::Details {
        output[2].text = frame_text(&position(scroll, rows.len(), viewport), width);
    }
    if rows.is_empty() && viewport > 0 {
        output[body_start] = SemanticRow {
            text: frame_text("? No providers shown", width),
            style: RowStyle::Warning,
        };
    }
    for (index, row) in rows.iter().skip(scroll).take(viewport).enumerate() {
        output[body_start + index] = SemanticRow {
            text: frame_text(&row.text, width),
            style: row.style,
        };
    }
    if height > 1 {
        output[footer].text = frame_text(controls, width);
    }
    output
        .into_iter()
        .map(|row| SemanticRow {
            text: pad_cells(&row.text, width),
            ..row
        })
        .collect()
}

fn render_preferences(width: usize, height: usize, config: &DashboardConfig) -> Vec<SemanticRow> {
    let mut output = vec![
        SemanticRow {
            text: String::new(),
            style: RowStyle::Normal,
        };
        height
    ];
    output[0] = SemanticRow {
        text: truncate("Preferences", width),
        style: RowStyle::Heading,
    };
    if height > 1 {
        output[1] = SemanticRow {
            text: truncate("Startup view", width),
            style: RowStyle::Normal,
        };
    }
    if height > 2 {
        let value = match config.startup_view {
            StartupView::Overview => "> overview",
            StartupView::Details => "> details",
        };
        output[2] = SemanticRow {
            text: truncate(value, width),
            style: RowStyle::Heading,
        };
    }
    if height > 3 && config.save_failed {
        output[3] = SemanticRow {
            text: truncate("? Save failed", width),
            style: RowStyle::Warning,
        };
    }
    if height > 1 {
        let footer = height - 1;
        output[footer].text = truncate(
            if width >= 24 {
                "←/→ · enter save · esc"
            } else {
                "←/→ enter esc"
            },
            width,
        );
    }
    output
        .into_iter()
        .map(|row| SemanticRow {
            text: pad_cells(&row.text, width),
            ..row
        })
        .collect()
}

fn truncate(value: &str, width: usize) -> String {
    if UnicodeWidthStr::width(value) <= width {
        return value.to_owned();
    }
    let mut output = String::new();
    let mut used = 0;
    for character in value.chars() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if used + character_width > width {
            break;
        }
        output.push(character);
        used += character_width;
    }
    output
}

fn pad_cells(value: &str, width: usize) -> String {
    format!(
        "{value}{}",
        " ".repeat(width.saturating_sub(UnicodeWidthStr::width(value)))
    )
}

struct Dashboard<'a> {
    rows: &'a [SemanticRow],
    config: &'a DashboardConfig,
}

impl Widget for Dashboard<'_> {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        for (y, row) in self.rows.iter().take(area.height as usize).enumerate() {
            buffer.set_stringn(
                area.x,
                area.y + y as u16,
                &row.text,
                area.width as usize,
                row.style.ratatui(self.config.color),
            );
        }
    }
}

/// Render once through Ratatui to a fixed buffer. Useful to callers that need a frame, not ANSI.
pub fn render_buffer(
    report: &QuotaReport,
    width: u16,
    height: u16,
    config: &DashboardConfig,
) -> Buffer {
    let rows = render_frame(report, width, height, config);
    let area = Rect::new(0, 0, width.max(1), height.max(1));
    let mut buffer = Buffer::empty(area);
    Dashboard {
        rows: &rows,
        config,
    }
    .render(area, &mut buffer);
    buffer
}

/// Serialize the same safe, plain preview lines as a standalone sanitized SVG.
pub fn preview_svg(lines: &[String], width: u16, height: u16) -> String {
    let escape = |value: &str| {
        value
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    };
    let rows = lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            format!(
                r#"<text x="8" y="{}" xml:space="preserve">{}</text>"#,
                18 + index * 18,
                escape(line)
            )
        })
        .collect::<String>();
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{}" height="{}" viewBox="0 0 {} {}"><rect width="100%" height="100%" fill="#191724"/><g fill="#e0def4" font-family="monospace" font-size="14">{rows}</g></svg>"##,
        u32::from(width) * 9 + 16,
        u32::from(height) * 18 + 8,
        u32::from(width) * 9 + 16,
        u32::from(height) * 18 + 8
    )
}

/// Drive the interactive Crossterm dashboard with finite local preferences.
pub fn dashboard(report: &QuotaReport) -> io::Result<()> {
    let settings_store = SettingsStore::from_environment().ok();
    let settings = settings_store
        .as_ref()
        .map(|store| store.load().settings)
        .unwrap_or_default();
    enable_raw_mode()?;
    let mut session = TerminalSession {
        raw: true,
        alternate: false,
    };
    let mut stdout = io::stdout();
    session.alternate = true;
    execute!(stdout, EnterAlternateScreen)?;
    let result = dashboard_loop(&mut stdout, report, settings_store.as_ref(), settings);
    let cleanup = session.restore(&mut stdout);
    result.and(cleanup)
}

struct TerminalSession {
    raw: bool,
    alternate: bool,
}

impl TerminalSession {
    fn restore(&mut self, stdout: &mut Stdout) -> io::Result<()> {
        let leave = if self.alternate {
            self.alternate = false;
            execute!(stdout, LeaveAlternateScreen)
        } else {
            Ok(())
        };
        let raw = if self.raw {
            self.raw = false;
            disable_raw_mode()
        } else {
            Ok(())
        };
        leave.and(raw)
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = self.restore(&mut io::stdout());
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InputAction {
    None,
    Quit,
    SavePreferences,
    AcknowledgeTransition,
}

fn handle_key(config: &mut DashboardConfig, key: KeyCode, provider_count: usize) -> InputAction {
    if key == KeyCode::Char('q') {
        return InputAction::Quit;
    }
    match config.view {
        DashboardView::Overview => match key {
            KeyCode::Esc => InputAction::Quit,
            KeyCode::Char('j') | KeyCode::Down => {
                config.selected_provider = config
                    .selected_provider
                    .saturating_add(1)
                    .min(provider_count.saturating_sub(1));
                InputAction::None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                config.selected_provider = config.selected_provider.saturating_sub(1);
                InputAction::None
            }
            KeyCode::PageDown => {
                config.selected_provider = config
                    .selected_provider
                    .saturating_add(4)
                    .min(provider_count.saturating_sub(1));
                InputAction::None
            }
            KeyCode::PageUp => {
                config.selected_provider = config.selected_provider.saturating_sub(4);
                InputAction::None
            }
            KeyCode::Char(number @ '1'..='6') => {
                let index = number as usize - '1' as usize;
                if index < provider_count {
                    config.selected_provider = index;
                }
                InputAction::None
            }
            KeyCode::Enter if provider_count > 0 => {
                config.view = DashboardView::Details;
                config.scroll = 0;
                InputAction::None
            }
            KeyCode::Char('p') => {
                config.return_view = DashboardView::Overview;
                config.saved_startup_view = config.startup_view;
                config.save_failed = false;
                config.view = DashboardView::Preferences;
                InputAction::None
            }
            _ => InputAction::None,
        },
        DashboardView::Details => match key {
            KeyCode::Esc => {
                config.view = DashboardView::Overview;
                config.scroll = 0;
                InputAction::None
            }
            KeyCode::Char('j') | KeyCode::Down => {
                config.scroll = config.scroll.saturating_add(1);
                InputAction::None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                config.scroll = config.scroll.saturating_sub(1);
                InputAction::None
            }
            KeyCode::PageDown => {
                config.scroll = config.scroll.saturating_add(8);
                InputAction::None
            }
            KeyCode::PageUp => {
                config.scroll = config.scroll.saturating_sub(8);
                InputAction::None
            }
            KeyCode::Char('p') => {
                config.return_view = DashboardView::Details;
                config.saved_startup_view = config.startup_view;
                config.save_failed = false;
                config.view = DashboardView::Preferences;
                InputAction::None
            }
            _ => InputAction::None,
        },
        DashboardView::Preferences => match key {
            KeyCode::Esc => {
                config.startup_view = config.saved_startup_view;
                config.view = config.return_view;
                config.save_failed = false;
                InputAction::None
            }
            KeyCode::Left | KeyCode::Right | KeyCode::Char(' ') => {
                config.startup_view = match config.startup_view {
                    StartupView::Overview => StartupView::Details,
                    StartupView::Details => StartupView::Overview,
                };
                config.save_failed = false;
                InputAction::None
            }
            KeyCode::Enter => InputAction::SavePreferences,
            _ => InputAction::None,
        },
        DashboardView::TransitionReview => match key {
            KeyCode::Esc => {
                config.view = DashboardView::Overview;
                InputAction::None
            }
            KeyCode::Char('a') | KeyCode::Enter => InputAction::AcknowledgeTransition,
            KeyCode::Char('j') | KeyCode::Down => {
                config.scroll = config.scroll.saturating_add(1);
                InputAction::None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                config.scroll = config.scroll.saturating_sub(1);
                InputAction::None
            }
            _ => InputAction::None,
        },
    }
}

fn config_from_settings(settings: &DashboardSettings) -> DashboardConfig {
    DashboardConfig {
        user_hidden: settings
            .hidden_providers
            .iter()
            .map(|provider| provider.id().to_owned())
            .collect(),
        color: std::env::var_os("NO_COLOR").is_none(),
        view: match settings.startup_view {
            StartupView::Overview => DashboardView::Overview,
            StartupView::Details => DashboardView::Details,
        },
        startup_view: settings.startup_view,
        saved_startup_view: settings.startup_view,
        ..DashboardConfig::default()
    }
}

fn dashboard_loop(
    stdout: &mut Stdout,
    report: &QuotaReport,
    settings_store: Option<&SettingsStore>,
    mut settings: DashboardSettings,
) -> io::Result<()> {
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let mut config = config_from_settings(&settings);
    loop {
        terminal.draw(|frame| {
            let area = frame.area();
            let rows = render_frame(report, area.width, area.height, &config);
            frame.render_widget(
                Dashboard {
                    rows: &rows,
                    config: &config,
                },
                area,
            );
        })?;
        if let Event::Key(key) = event::read()? {
            let provider_count = visible_model(report, &config).providers.len();
            match handle_key(&mut config, key.code, provider_count) {
                InputAction::Quit => return Ok(()),
                InputAction::SavePreferences => {
                    settings.startup_view = config.startup_view;
                    if settings_store.is_some_and(|store| store.save(&settings).is_ok()) {
                        config.saved_startup_view = config.startup_view;
                        config.view = config.return_view;
                        config.save_failed = false;
                    } else {
                        config.save_failed = true;
                    }
                }
                InputAction::AcknowledgeTransition | InputAction::None => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::provider::{
        EffectiveAvailability, EffectivePace, ProviderState, QuotaWindow, Runway, WindowPace,
    };

    fn provider(id: &str, remaining: Option<f64>, status: ProviderStatus) -> ProviderQuota {
        ProviderQuota {
            provider: id.into(),
            label: Some(id.into()),
            source: None,
            plan: None,
            windows: vec![QuotaWindow {
                id: "main".into(),
                label: "primary tier".into(),
                kind: "session".into(),
                percent_used: None,
                percent_remaining: remaining,
                starts_at: None,
                resets_at: None,
                reset_text: Some("soon".into()),
                window_seconds: None,
                spent_usd: None,
                limit_usd: None,
                pace: None,
            }],
            effective: remaining
                .map(|remaining| EffectiveAvailability {
                    scope: "all".into(),
                    status: EffectiveStatus::Known,
                    effective_percent_remaining: Some(remaining),
                    bounded_by: vec![],
                    limiting_window_ids: vec!["main".into()],
                    pace: None,
                    runway: Some(Runway {
                        status: if remaining <= 10.0 {
                            RunwayStatus::ExhaustedNow
                        } else {
                            RunwayStatus::ThroughReset
                        },
                        usable_runway_seconds: None,
                        projected_exhausted_at: None,
                        limiting_window_id: Some("main".into()),
                        projection_confidence: None,
                        unmeasurable_window_ids: vec![],
                    }),
                })
                .into_iter()
                .collect(),
            semantics_status: Some(SemanticsStatus::Known),
            credits: None,
            state: ProviderState {
                status,
                stale: false,
                refreshed_at: None,
                auth_status: None,
                reason: None,
                remedy_command: None,
                error_code: None,
            },
        }
    }
    fn report(providers: Vec<ProviderQuota>) -> QuotaReport {
        QuotaReport {
            generated_at: "2026-09-02T00:00:00Z".into(),
            schema_version: 5,
            providers,
            adaptation_warnings: vec![],
        }
    }
    fn width(line: &str) -> usize {
        UnicodeWidthStr::width(line)
    }

    fn detail_config() -> DashboardConfig {
        DashboardConfig {
            view: DashboardView::Details,
            ..DashboardConfig::default()
        }
    }

    fn reachable_lines(
        report: &QuotaReport,
        columns: u16,
        rows: u16,
        config: &DashboardConfig,
    ) -> Vec<String> {
        let total = semantic_rows(report, config, columns).len();
        (0..total.max(1))
            .flat_map(|scroll| {
                render_lines(
                    report,
                    columns,
                    rows,
                    &DashboardConfig {
                        scroll,
                        ..config.clone()
                    },
                )
            })
            .collect()
    }

    fn reachable_style(
        report: &QuotaReport,
        columns: u16,
        rows: u16,
        config: &DashboardConfig,
        prefix: &str,
    ) -> Style {
        let total = semantic_rows(report, config, columns).len();
        for scroll in 0..total.max(1) {
            let scrolled = DashboardConfig {
                scroll,
                ..config.clone()
            };
            let lines = render_lines(report, columns, rows, &scrolled);
            if let Some(y) = lines.iter().position(|line| line.starts_with(prefix)) {
                return render_buffer(report, columns, rows, &scrolled).content
                    [y * columns as usize]
                    .style();
            }
        }
        panic!("rendered row starting with {prefix:?} was not reachable");
    }

    #[test]
    fn uses_exact_line_and_cell_dimensions_at_supported_sizes() {
        let report = report(vec![provider("claude", Some(9.0), ProviderStatus::Fresh)]);
        for (columns, rows) in [(36, 23), (20, 12), (24, 8)] {
            let lines = render_lines(&report, columns, rows, &DashboardConfig::default());
            assert_eq!(lines.len(), rows as usize);
            assert!(lines.iter().all(|line| width(line) == columns as usize));
        }
    }

    #[test]
    fn keeps_context_and_controls_pinned_while_semantic_rows_scroll() {
        let partial_report = report(vec![
            provider("claude", Some(9.0), ProviderStatus::Fresh),
            provider("codex", Some(50.0), ProviderStatus::Fresh),
        ]);
        let first = render_lines(&partial_report, 36, 12, &DashboardConfig::default());
        let later = render_lines(
            &partial_report,
            36,
            12,
            &DashboardConfig {
                selected_provider: 1,
                ..DashboardConfig::default()
            },
        );
        assert_eq!(first[0], later[0]);
        assert_eq!(first[1], later[1]);
        assert_eq!(first[11], later[11]);
        assert!(first[2].starts_with(">!Claude"));
        assert!(later[3].starts_with(">?Codex ? partial"));
    }

    #[test]
    fn every_provider_summary_is_reachable() {
        let report = report(
            MarketedProvider::ALL
                .iter()
                .map(|market| provider(market.id(), Some(50.0), ProviderStatus::Fresh))
                .collect(),
        );
        let seen = render_lines(&report, 20, 12, &DashboardConfig::default());
        for provider in MarketedProvider::ALL {
            assert!(seen.iter().any(|line| {
                line.contains(compact_provider_name(provider))
                    || (provider == MarketedProvider::Copilot && line.contains("GitHub"))
            }));
        }
    }

    #[test]
    fn includes_required_text_markers_and_unknown_is_not_zero() {
        let report = report(vec![provider("claude", None, ProviderStatus::Fresh)]);
        let output = render_lines(&report, 36, 12, &DashboardConfig::default()).join("\n");
        assert!(output.contains("Herdr Quota"));
        assert!(output.contains("? Quota data partial"));
        assert!(output.contains("? unknown"));
        assert!(output.contains("j/k · enter details · p · q"));
    }

    #[test]
    fn empty_state_only_advertises_available_controls() {
        let lines = render_lines(&report(vec![]), 36, 12, &DashboardConfig::default());
        assert!(
            lines
                .iter()
                .any(|line| line.trim_end() == "? No providers shown")
        );
        assert!(lines.iter().all(|line| !line.contains("prefs")));
        assert!(lines.iter().all(|line| !line.contains("Press p")));
    }

    #[test]
    fn static_dashboard_does_not_advertise_refresh() {
        let report = report(vec![provider("claude", Some(50.0), ProviderStatus::Fresh)]);
        for (columns, rows, controls) in [
            (36, 23, "j/k · enter details · p · q"),
            (20, 12, "j/k enter p q"),
        ] {
            let lines = render_lines(&report, columns, rows, &DashboardConfig::default());
            assert_eq!(lines.last().unwrap().trim_end(), controls);
            assert!(lines.iter().all(|line| !line.contains(" · r")));
        }
    }

    #[test]
    fn no_color_render_uses_default_foreground_styles() {
        let report = report(vec![
            provider("claude", Some(9.0), ProviderStatus::Fresh),
            provider("codex", Some(7.0), ProviderStatus::Fresh),
            provider("cursor", Some(50.0), ProviderStatus::Unavailable),
        ]);
        for (columns, rows) in [(20, 12), (36, 23)] {
            let plain = DashboardConfig::default();
            let colored = DashboardConfig {
                color: true,
                ..DashboardConfig::default()
            };
            assert_eq!(
                render_lines(&report, columns, rows, &plain),
                render_lines(&report, columns, rows, &colored)
            );
            let lines = reachable_lines(&report, columns, rows, &plain);
            assert!(lines.iter().any(|line| line.starts_with(">!Claude")));
            assert!(lines.iter().any(|line| line.starts_with(" ?Cursor")));
            let buffer = render_buffer(&report, columns, rows, &plain);
            assert!(buffer.content.iter().all(|cell| cell.fg == Color::Reset));
            assert!(buffer.content.iter().all(|cell| cell.bg == Color::Reset));
            assert!(
                buffer
                    .content
                    .iter()
                    .all(|cell| cell.modifier == Modifier::empty())
            );
        }
    }

    #[test]
    fn colored_buffer_preserves_semantic_row_styles() {
        let report = report(vec![
            provider("claude", Some(9.0), ProviderStatus::Fresh),
            provider("codex", Some(7.0), ProviderStatus::Fresh),
            provider("cursor", Some(50.0), ProviderStatus::Unavailable),
        ]);
        let config = DashboardConfig {
            color: true,
            ..detail_config()
        };
        let critical = reachable_style(&report, 36, 23, &config, " !primary tier");
        assert_eq!(critical.fg, Some(Color::Red));
        assert!(critical.add_modifier.contains(Modifier::BOLD));
        let warning = reachable_style(
            &report,
            36,
            23,
            &DashboardConfig {
                selected_provider: 2,
                ..config.clone()
            },
            "> Cursor",
        );
        assert_eq!(warning.fg, Some(Color::Yellow));
        assert!(warning.add_modifier.contains(Modifier::BOLD));
        let heading = reachable_style(&report, 36, 23, &config, "> Claude");
        assert_eq!(heading.fg, Some(Color::Reset));
        assert!(heading.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn noncurrent_windows_are_labeled_and_excluded_from_attention() {
        let states = [
            ProviderStatus::Unavailable,
            ProviderStatus::AuthRequired,
            ProviderStatus::RateLimited,
            ProviderStatus::Error,
            ProviderStatus::Stale,
        ];
        for (index, status) in states.into_iter().enumerate() {
            let provider_id = MarketedProvider::ALL[index].id();
            let report = report(vec![provider(provider_id, Some(7.0), status)]);
            for (columns, rows) in [(36, 23), (20, 12)] {
                let plain = DashboardConfig::default();
                let colored = DashboardConfig {
                    color: true,
                    ..DashboardConfig::default()
                };
                assert_eq!(
                    render_lines(&report, columns, rows, &plain),
                    render_lines(&report, columns, rows, &colored)
                );
                let lines = reachable_lines(&report, columns, rows, &plain);
                assert_eq!(lines[1].trim_end(), "? Limits non-current");
                assert!(lines.iter().any(|line| line.starts_with(">?")));
                assert!(lines.iter().all(|line| !line.starts_with(">!")));
                let buffer = render_buffer(&report, columns, rows, &plain);
                assert!(buffer.content.iter().all(|cell| cell.fg == Color::Reset));
                assert!(
                    buffer
                        .content
                        .iter()
                        .all(|cell| cell.modifier == Modifier::empty())
                );
            }
        }

        let mut stale_fresh = provider("claude", Some(7.0), ProviderStatus::Fresh);
        stale_fresh.state.stale = true;
        let stale_report = report(vec![stale_fresh]);
        let lines = reachable_lines(&stale_report, 36, 23, &DashboardConfig::default());
        assert_eq!(lines[1].trim_end(), "? Limits non-current");
        let lines = reachable_lines(&stale_report, 36, 23, &detail_config());
        assert!(lines.iter().any(|line| line.starts_with(" ~ last known")));
    }

    #[test]
    fn unusable_auth_is_noncurrent_and_actionable_siblings_still_rank() {
        for auth_status in ["unusable", "expired_refreshable"] {
            let mut auth_provider = provider("codex", Some(6.0), ProviderStatus::Fresh);
            auth_provider.state.auth_status = Some(auth_status.into());
            let auth_report = report(vec![auth_provider.clone()]);
            for (columns, rows) in [(36, 23), (20, 12)] {
                let plain = DashboardConfig::default();
                let colored = DashboardConfig {
                    color: true,
                    ..DashboardConfig::default()
                };
                let lines = reachable_lines(&auth_report, columns, rows, &plain);
                assert_eq!(lines[1].trim_end(), "? Limits non-current");
                assert!(
                    lines
                        .iter()
                        .any(|line| { line.starts_with(">?Codex signed out") })
                );
                assert!(lines.iter().all(|line| !line.starts_with(">!")));
                assert_eq!(
                    render_lines(&auth_report, columns, rows, &plain),
                    render_lines(&auth_report, columns, rows, &colored)
                );
                let buffer = render_buffer(&auth_report, columns, rows, &plain);
                assert!(buffer.content.iter().all(|cell| cell.fg == Color::Reset));
                assert!(
                    buffer
                        .content
                        .iter()
                        .all(|cell| cell.modifier == Modifier::empty())
                );
            }

            let mixed = report(vec![
                provider("claude", Some(7.0), ProviderStatus::Fresh),
                auth_provider,
            ]);
            for (columns, rows) in [(36, 23), (20, 12)] {
                let lines = render_lines(&mixed, columns, rows, &DashboardConfig::default());
                assert!(lines[1].starts_with("! out now"));
            }
        }
    }

    #[test]
    fn attention_uses_effective_availability_not_raw_window_percentages() {
        let mut healthy = provider("claude", Some(5.0), ProviderStatus::Fresh);
        healthy.effective[0].effective_percent_remaining = Some(80.0);
        healthy.effective[0].runway.as_mut().unwrap().status = RunwayStatus::ThroughReset;

        let mut projected = provider("claude", Some(80.0), ProviderStatus::Fresh);
        projected.effective[0].effective_percent_remaining = Some(80.0);
        let runway = projected.effective[0].runway.as_mut().unwrap();
        runway.status = RunwayStatus::ProjectedExhaustion;
        runway.projection_confidence = Some(ProjectionConfidence::Established);

        for (report, expected) in [
            (report(vec![healthy]), "= Limits on pace"),
            (report(vec![projected]), "! out before reset"),
        ] {
            for (columns, rows) in [(36, 23), (20, 12)] {
                let plain = DashboardConfig::default();
                let colored = DashboardConfig {
                    color: true,
                    ..DashboardConfig::default()
                };
                let lines = render_lines(&report, columns, rows, &plain);
                assert!(expected.starts_with(lines[1].trim_end()));
                assert_eq!(lines, render_lines(&report, columns, rows, &colored));
                let buffer = render_buffer(&report, columns, rows, &plain);
                assert!(buffer.content.iter().all(|cell| cell.fg == Color::Reset));
                assert!(
                    buffer
                        .content
                        .iter()
                        .all(|cell| cell.modifier == Modifier::empty())
                );
            }
        }
    }

    #[test]
    fn effective_constraints_rank_by_time_and_use_pace_limiting_tier() {
        let projected = |id: &str, remaining: f64, exhausted_at: &str| {
            let mut provider = provider(id, Some(remaining), ProviderStatus::Fresh);
            let runway = provider.effective[0].runway.as_mut().unwrap();
            runway.status = RunwayStatus::ProjectedExhaustion;
            runway.projected_exhausted_at = Some(exhausted_at.into());
            runway.projection_confidence = Some(ProjectionConfidence::Established);
            provider
        };
        let projections = report(vec![
            projected("claude", 10.0, "2026-09-02T17:00:00Z"),
            projected("codex", 80.0, "2026-09-02T13:00:00Z"),
        ]);
        let lines = render_lines(&projections, 36, 23, &DashboardConfig::default());
        assert!(lines[1].starts_with("! out 09/02 13:00"));

        let mut exhausted_soon = provider("claude", Some(0.0), ProviderStatus::Fresh);
        exhausted_soon.windows[0].resets_at = Some("2026-09-02T13:00:00Z".into());
        let mut exhausted_later = provider("codex", Some(0.0), ProviderStatus::Fresh);
        exhausted_later.windows[0].resets_at = Some("2026-09-02T17:00:00Z".into());
        let lines = render_lines(
            &report(vec![exhausted_soon, exhausted_later]),
            36,
            23,
            &DashboardConfig::default(),
        );
        assert!(lines[1].starts_with("! out now · reset 09/02 17:00"));

        let mut labeled = projected("claude", 80.0, "2026-09-02T13:00:00Z");
        let mut pace_window = labeled.windows[0].clone();
        pace_window.id = "pace".into();
        pace_window.label = "pace tier".into();
        labeled.windows.push(pace_window);
        labeled.effective[0]
            .runway
            .as_mut()
            .unwrap()
            .limiting_window_id = None;
        labeled.effective[0].limiting_window_ids = vec!["main".into()];
        labeled.effective[0].pace = Some(EffectivePace {
            status: PaceStatus::Ahead,
            worst_reserve_percent_points: None,
            worst_reserve_window_id: Some("pace".into()),
            unknown_window_ids: vec![],
        });
        let lines = render_lines(&report(vec![labeled]), 36, 23, &DashboardConfig::default());
        assert!(lines[1].starts_with("! out 09/02 13:00"));
        assert!(lines.iter().any(|line| line.contains("80% pace tier")));
    }

    #[test]
    fn narrow_titles_and_exhaustion_consequences_elide_whole_tokens() {
        let mut exhausted = provider("claude", Some(0.0), ProviderStatus::Fresh);
        exhausted.windows[0].resets_at = Some("2026-09-02T13:00:00Z".into());
        let exhausted = report(vec![exhausted]);

        for columns in 16..=23 {
            let lines = render_lines(&exhausted, columns, 12, &DashboardConfig::default());
            assert!(lines[1].starts_with("! out now"), "{columns}: {:?}", lines[1]);
        }

        let details = DashboardConfig {
            view: DashboardView::Details,
            ..DashboardConfig::default()
        };
        for provider_id in ["claude", "copilot"] {
            let lines = render_lines(
                &report(vec![provider(provider_id, Some(50.0), ProviderStatus::Fresh)]),
                16,
                12,
                &details,
            );
            assert_eq!(lines[0].trim_end(), "Herdr Quota");
        }
    }

    #[test]
    fn narrow_detail_rows_elide_only_whole_tokens() {
        let details = DashboardConfig {
            view: DashboardView::Details,
            ..DashboardConfig::default()
        };
        let mut long_tier = provider("claude", Some(50.0), ProviderStatus::Fresh);
        long_tier.windows[0].id = "extension-tier".into();
        long_tier.windows[0].label = "Long model".into();
        let lines = render_lines(&report(vec![long_tier]), 20, 12, &details);
        assert!(
            lines
                .iter()
                .any(|line| line.contains("Long") && line.contains("50%"))
        );
        assert!(lines.iter().all(|line| !line.contains("Long mode")));

        let mut unknown_tier = provider("claude", Some(50.0), ProviderStatus::Fresh);
        unknown_tier.windows[0].id = "extension-tier".into();
        unknown_tier.windows[0].label = "Long model".into();
        unknown_tier.windows[0].percent_remaining = None;
        let lines = render_lines(&report(vec![unknown_tier]), 16, 12, &details);
        assert!(lines.iter().any(|line| line.contains("--")));

        let signed_out = provider("copilot", Some(50.0), ProviderStatus::AuthRequired);
        let lines = render_lines(&report(vec![signed_out]), 16, 12, &details);
        assert!(lines.iter().any(|line| line.trim() == "? sign in"));

        let mut approval = provider("claude", Some(50.0), ProviderStatus::Fresh);
        approval.state.reason = Some("keychain_access_required".into());
        let lines = render_lines(&report(vec![approval]), 16, 12, &details);
        assert!(lines.iter().any(|line| line.trim() == "? Keychain"));
    }

    #[test]
    fn narrow_frame_preserves_complete_decision_and_heading_tokens() {
        let mut partial = provider("claude", Some(50.0), ProviderStatus::Fresh);
        partial.semantics_status = Some(SemanticsStatus::Partial);
        let lines = render_lines(
            &report(vec![partial]),
            16,
            12,
            &DashboardConfig {
                view: DashboardView::Details,
                ..DashboardConfig::default()
            },
        );

        assert_eq!(lines[1].trim_end(), "? Data partial");
        assert_eq!(lines[3].trim_end(), "> Claude");
    }

    #[test]
    fn attention_requires_explicit_current_pace_evidence() {
        let base = report(vec![provider("claude", Some(50.0), ProviderStatus::Fresh)]);
        let mut no_windows = base.clone();
        no_windows.providers[0].windows.clear();
        no_windows.providers[0].effective.clear();
        let mut unknown_semantics = base.clone();
        unknown_semantics.providers[0].semantics_status = Some(SemanticsStatus::Unknown);
        let mut partial_semantics = base.clone();
        partial_semantics.providers[0].semantics_status = Some(SemanticsStatus::Partial);
        let mut incomplete_siblings = base.clone();
        let mut sibling = provider("codex", Some(50.0), ProviderStatus::Fresh);
        sibling.windows.clear();
        sibling.effective.clear();
        incomplete_siblings.providers.push(sibling);
        let codex_with_unreported_sibling =
            report(vec![provider("codex", Some(50.0), ProviderStatus::Fresh)]);
        let mut codex_projected = provider("codex", Some(50.0), ProviderStatus::Fresh);
        let runway = codex_projected.effective[0].runway.as_mut().unwrap();
        runway.status = RunwayStatus::ProjectedExhaustion;
        runway.projected_exhausted_at = Some("2026-09-02T13:00:00Z".into());
        runway.projection_confidence = Some(ProjectionConfidence::Established);
        let codex_projected = report(vec![codex_projected]);

        let with_pace = |status| {
            let mut report = base.clone();
            report.providers[0].effective[0].runway = None;
            report.providers[0].effective[0].pace = Some(EffectivePace {
                status,
                worst_reserve_percent_points: None,
                worst_reserve_window_id: Some("main".into()),
                unknown_window_ids: vec![],
            });
            report
        };

        for (report, expected) in [
            (no_windows, "? Quota data partial"),
            (unknown_semantics, "? Quota data partial"),
            (partial_semantics, "? Quota data partial"),
            (incomplete_siblings, "? Quota data partial"),
            (codex_with_unreported_sibling, "? Quota data partial"),
            (codex_projected, "? Quota data partial"),
            (with_pace(PaceStatus::Ahead), "? Pace needs review"),
            (with_pace(PaceStatus::Mixed), "? Pace needs review"),
            (with_pace(PaceStatus::Behind), "= Limits on pace"),
            (with_pace(PaceStatus::OnPace), "= Limits on pace"),
        ] {
            for (columns, rows) in [(36, 23), (20, 12)] {
                let plain = DashboardConfig::default();
                let colored = DashboardConfig {
                    color: true,
                    ..DashboardConfig::default()
                };
                let lines = render_lines(&report, columns, rows, &plain);
                assert_eq!(lines[1].trim_end(), expected);
                assert_eq!(lines, render_lines(&report, columns, rows, &colored));
                let buffer = render_buffer(&report, columns, rows, &plain);
                assert!(buffer.content.iter().all(|cell| cell.fg == Color::Reset));
                assert!(
                    buffer
                        .content
                        .iter()
                        .all(|cell| cell.modifier == Modifier::empty())
                );
            }
        }
    }

    #[test]
    fn hidden_summary_only_counts_nonrendered_non_user_hidden_providers() {
        let partial_report = report(vec![
            provider("claude", Some(50.0), ProviderStatus::Fresh),
            provider("codex", Some(50.0), ProviderStatus::Unavailable),
        ]);
        let mut hidden = BTreeSet::new();
        hidden.insert("cursor".into());
        let output = reachable_lines(
            &partial_report,
            36,
            23,
            &DashboardConfig {
                user_hidden: hidden,
                ..DashboardConfig::default()
            },
        )
        .join("\n");
        assert!(output.contains(">=Claude") || output.contains(">?Claude"));
        assert!(output.contains(" ?Codex unavailable"));
        assert!(output.contains("3 unavailable providers hidden"));
        let complete = report(
            MarketedProvider::ALL
                .iter()
                .map(|market| provider(market.id(), Some(50.0), ProviderStatus::Fresh))
                .collect(),
        );
        assert!(
            !render_lines(&complete, 36, 23, &DashboardConfig::default())
                .join("\n")
                .contains("unavailable provider")
        );
        let mut complete_with_unavailable = complete;
        complete_with_unavailable.providers[0].state.status = ProviderStatus::Unavailable;
        let output = render_lines(
            &complete_with_unavailable,
            36,
            23,
            &DashboardConfig::default(),
        )
        .join("\n");
        assert!(output.contains(">?Claude unavailable"));
        assert!(!output.contains("unavailable provider"));
    }

    #[test]
    fn renderer_uses_semantic_provider_labels_and_synthetic_rows() {
        let mut codex = provider("codex", Some(50.0), ProviderStatus::Fresh);
        codex.label = Some("collector label".into());
        codex.windows[0].id = "weekly".into();
        codex.windows[0].label = "collector weekly label".into();
        let lines = render_lines(&report(vec![codex]), 36, 23, &detail_config());
        assert!(lines.iter().any(|line| line.starts_with("> OpenAI Codex")));
        assert!(lines.iter().any(|line| line.starts_with("  Week")));
        assert!(
            lines
                .iter()
                .any(|line| line.starts_with(" ?Code review") && line.contains("not reported"))
        );

        let unavailable = report(vec![provider(
            "codex",
            Some(50.0),
            ProviderStatus::Unavailable,
        )]);
        let lines = render_lines(&unavailable, 36, 23, &detail_config());
        assert!(
            lines
                .iter()
                .any(|line| line.starts_with("> OpenAI Codex · unavailable"))
        );
        assert!(lines.iter().any(|line| line.starts_with(" ~ last known")));
    }

    #[test]
    fn partial_annotation_keeps_current_semantic_tiers() {
        let mut claude = provider("claude", Some(50.0), ProviderStatus::Fresh);
        claude.semantics_status = Some(SemanticsStatus::Partial);
        claude.windows[0].pace = Some(WindowPace {
            status: PaceStatus::Ahead,
            reserve_percent_points: None,
            burn_multiple: None,
            projected_exhausted_at: None,
            projection_confidence: None,
        });

        let report = report(vec![claude]);
        let semantic = semantic_rows(&report, &detail_config(), 36);
        assert!(semantic.iter().any(|row| row.text.contains("· ahead")));

        let lines = render_lines(&report, 36, 23, &detail_config());
        assert!(
            lines
                .iter()
                .any(|line| line.starts_with("> Claude · partial data"))
        );
        assert!(lines.iter().any(|line| line.contains("· ahead")));
        assert!(!lines.iter().any(|line| line.starts_with(" ~ last known")));

        let compact = render_lines(&report, 20, 12, &detail_config());
        assert!(!compact.iter().any(|line| line.contains("· ah")));
    }

    #[test]
    fn overview_fits_four_provider_summaries_with_fixed_decision_and_footer_at_36x12() {
        let report = report(
            ["claude", "codex", "cursor", "kimi"]
                .into_iter()
                .map(|id| provider(id, Some(50.0), ProviderStatus::Fresh))
                .collect(),
        );
        let lines = render_lines(&report, 36, 12, &DashboardConfig::default());

        assert_eq!(lines[0].trim_end(), "Herdr Quota");
        assert!(lines[1].starts_with("= Limits on pace"));
        assert!(lines[2].starts_with(">=Claude"));
        assert!(lines[3].starts_with(" ?Codex ? partial"));
        assert!(lines[4].starts_with(" =Cursor"));
        assert!(lines[5].starts_with(" =Kimi"));
        assert_eq!(lines[11].trim_end(), "j/k · enter details · p · q");
    }

    #[test]
    fn complete_overview_uses_every_row_for_truthful_evidence_without_midword_clipping() {
        let report = crate::domain::schema::parse_quota_response(include_str!(
            "../../../../test/fixtures/complete.json"
        ))
        .expect("complete fixture");

        for columns in [36, 20] {
            let lines = render_lines(&report, columns, 12, &DashboardConfig::default());
            assert!(lines[2..11].iter().all(|line| !line.trim().is_empty()));
            assert!(lines.iter().all(|line| !line.trim_end().ends_with("Fabl")));
            assert!(lines.iter().all(|line| !line.trim_end().ends_with("prov")));
            assert!(lines.iter().all(|line| !line.contains("3rd-party")));
            assert!(lines[1].contains("out"));
            assert!(!lines[1].contains("Claude"));
            assert!(lines.iter().all(|line| !line.contains("Codex reset")));
            assert!(lines.iter().all(|line| !line.contains("Codex on pace")));
        }
    }

    #[test]
    fn overview_uses_exact_partial_and_unknown_states_when_siblings_are_unsafe() {
        let codex = provider("codex", Some(79.0), ProviderStatus::Fresh);
        let mut cursor = provider("cursor", None, ProviderStatus::Fresh);
        cursor.semantics_status = Some(SemanticsStatus::Unknown);
        cursor.effective.clear();
        let lines = render_lines(
            &report(vec![codex, cursor]),
            36,
            12,
            &DashboardConfig::default(),
        );

        assert!(lines.iter().any(|line| line.contains("?Codex ? partial")));
        assert!(lines.iter().any(|line| line.contains("?Cursor ? unknown")));
        assert!(lines.iter().all(|line| !line.contains("Cursor 0%")));
    }

    #[test]
    fn narrow_overview_drops_bars_and_preserves_marker_provider_value_and_compact_date() {
        let mut kimi = provider("kimi", Some(50.0), ProviderStatus::Fresh);
        kimi.windows[0].resets_at = Some("2026-09-02T17:00:00Z".into());
        let kimi_report = report(vec![kimi]);

        for columns in 16..=23 {
            let lines = render_lines(&kimi_report, columns, 12, &DashboardConfig::default());
            let row = lines[2].trim_end();
            assert!(row.starts_with(">=Kimi"), "{columns}: {row:?}");
            assert!(row.contains("50%"), "{columns}: {row:?}");
            assert!(!row.contains('['), "{columns}: {row:?}");
        }
        let row = render_lines(&kimi_report, 23, 12, &DashboardConfig::default())[2].clone();
        assert!(row.contains("9/2"));
        assert!(!row.contains("09/02"));

        let partial = render_lines(
            &report(vec![provider("codex", Some(50.0), ProviderStatus::Fresh)]),
            16,
            12,
            &DashboardConfig::default(),
        );
        assert_eq!(partial[2].trim_end(), ">Codex? partial");

        let mut unknown = provider("cursor", None, ProviderStatus::Fresh);
        unknown.semantics_status = Some(SemanticsStatus::Unknown);
        unknown.effective.clear();
        let unknown = render_lines(&report(vec![unknown]), 16, 12, &DashboardConfig::default());
        assert_eq!(unknown[2].trim_end(), ">Cursor? unknown");
    }

    #[test]
    fn enter_opens_selected_detail_escape_returns_and_all_tiers_are_immediately_reachable() {
        let mut claude = provider("claude", Some(50.0), ProviderStatus::Fresh);
        for index in 2..=4 {
            let mut window = claude.windows[0].clone();
            window.id = format!("tier_{index}");
            window.label = format!("tier {index}");
            claude.windows.push(window);
        }
        let report = report(vec![
            provider("cursor", Some(60.0), ProviderStatus::Fresh),
            claude,
        ]);
        let mut config = DashboardConfig::default();

        assert_eq!(handle_key(&mut config, KeyCode::Down, 2), InputAction::None);
        assert_eq!(config.selected_provider, 1);
        assert_eq!(
            handle_key(&mut config, KeyCode::Enter, 2),
            InputAction::None
        );
        assert_eq!(config.view, DashboardView::Details);
        let details = render_lines(&report, 20, 12, &config).join("\n");
        for label in ["primary", "tier 2", "tier 3", "tier 4"] {
            assert!(details.contains(label), "missing {label:?} in {details:?}");
        }
        assert_eq!(handle_key(&mut config, KeyCode::Esc, 2), InputAction::None);
        assert_eq!(config.view, DashboardView::Overview);
        assert_eq!(config.selected_provider, 1);
        assert!(render_lines(&report, 20, 12, &config)[3].starts_with(">=Claude"));
    }

    #[test]
    fn every_provider_detail_is_reachable_in_two_keys_from_overview() {
        for index in 0..MarketedProvider::ALL.len() {
            let mut config = DashboardConfig::default();
            let number = char::from_digit((index + 1) as u32, 10).expect("provider digit");
            assert_eq!(
                handle_key(
                    &mut config,
                    KeyCode::Char(number),
                    MarketedProvider::ALL.len()
                ),
                InputAction::None
            );
            assert_eq!(config.selected_provider, index);
            assert_eq!(
                handle_key(&mut config, KeyCode::Enter, MarketedProvider::ALL.len()),
                InputAction::None
            );
            assert_eq!(config.view, DashboardView::Details);
        }
    }

    #[test]
    fn enter_and_acknowledgement_are_modal_local() {
        let mut config = DashboardConfig::default();
        assert_eq!(
            handle_key(&mut config, KeyCode::Char('a'), 1),
            InputAction::None
        );
        assert_eq!(
            handle_key(&mut config, KeyCode::Enter, 1),
            InputAction::None
        );
        assert_eq!(config.view, DashboardView::Details);
        assert_eq!(
            handle_key(&mut config, KeyCode::Enter, 1),
            InputAction::None
        );

        config.view = DashboardView::Preferences;
        assert_eq!(
            handle_key(&mut config, KeyCode::Enter, 1),
            InputAction::SavePreferences
        );
        config.view = DashboardView::TransitionReview;
        assert_eq!(
            handle_key(&mut config, KeyCode::Enter, 1),
            InputAction::AcknowledgeTransition
        );
        assert_eq!(
            handle_key(&mut config, KeyCode::Char('a'), 1),
            InputAction::AcknowledgeTransition
        );
        assert_eq!(handle_key(&mut config, KeyCode::Esc, 1), InputAction::None);
        assert_eq!(config.view, DashboardView::Overview);
    }

    #[test]
    fn preferences_exposes_only_the_finite_startup_view_choice() {
        let settings = DashboardSettings {
            startup_view: StartupView::Details,
            ..DashboardSettings::default()
        };
        let mut config = config_from_settings(&settings);
        assert_eq!(config.view, DashboardView::Details);
        handle_key(&mut config, KeyCode::Char('p'), 1);
        assert_eq!(config.view, DashboardView::Preferences);
        let initial = render_lines(&report(vec![]), 20, 12, &config).join("\n");
        assert!(initial.contains("Startup view"));
        assert!(initial.contains("> details"));
        assert!(!initial.contains("theme"));
        assert!(!initial.contains("interval"));

        handle_key(&mut config, KeyCode::Right, 1);
        assert_eq!(config.startup_view, StartupView::Overview);
        handle_key(&mut config, KeyCode::Esc, 1);
        assert_eq!(config.startup_view, StartupView::Details);
        assert_eq!(config.view, DashboardView::Details);
    }

    #[test]
    fn svg_escapes_terminal_text() {
        let svg = preview_svg(&["<safe & bounded>".into()], 20, 1);
        assert!(svg.contains("&lt;safe &amp; bounded&gt;"));
        assert!(svg.contains(r#"xml:space="preserve">&lt;safe &amp; bounded&gt;"#));
    }
}
