//! A bounded Ratatui dashboard renderer.
//!
//! The renderer accepts only adapted schema-v5 data. It deliberately produces
//! whole semantic rows before placing them in a fixed-height frame, so scrolling
//! cannot split a provider heading from the row model or leak collector text.

use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{Datelike, Timelike};
use crossterm::event::KeyCode;
use ratatui::{
    Frame,
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    widgets::Widget,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::domain::{
    history_evidence::{HistoryDocument, HistoryEvidenceKind, HistoryTrend, history_trend},
    provider::{
        EffectiveAvailability, EffectiveStatus, MarketedProvider, PaceStatus, ProjectionConfidence,
        ProviderQuota, ProviderStatus, RunwayStatus, SemanticsStatus,
    },
    schema::QuotaReport,
    tiers::TierConclusion,
};
use crate::store::settings::{ProviderIdentityMode, StartupView};
use crate::ui::{
    bar::{MeterMode, remaining_bar},
    model::{
        ProviderDetail, ProviderVisibility, ProviderVisibilityMap, dashboard_model,
        provider_section,
    },
    readiness::{
        ProviderReadiness, decision_grade, provider_readiness, quota_readiness, readiness_line,
    },
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
#[derive(Clone, Debug)]
pub struct DashboardConfig {
    /// IDs explicitly hidden by the user. They never inflate availability summary text.
    pub user_hidden: BTreeSet<String>,
    /// The first semantic row visible in the scrollable region.
    pub scroll: usize,
    /// Whether optional Ratatui colour may reinforce the textual markers.
    pub color: bool,
    /// Saved provider order. An empty value preserves report order for previews.
    pub provider_order: Vec<String>,
    /// The locally selected meter interpretation.
    pub meter_mode: MeterMode,
    /// The finite provider logo/name presentation.
    pub provider_identity: ProviderIdentityMode,
    /// Whether the terminal can render the stable Unicode logo-derived marks.
    pub logo_glyphs: bool,
    /// Whether controls that require the live runtime may be advertised.
    pub interactive: bool,
    /// Whether the compact provider-readiness summary belongs on this launch frame.
    pub first_run: bool,
    /// Number of in-pane transition cues available for review.
    pub transition_count: usize,
    /// Current bounded collector state, when visible.
    pub status: Option<DashboardStatus>,
    /// Rounded-up minutes until the scheduled retry for a collector failure.
    pub retry_minutes: Option<u64>,
    /// Retained schema-v2 safe history used only for selected-provider detail.
    pub history: Option<HistoryDocument>,
    /// The current finite dashboard surface.
    pub view: DashboardView,
    /// The stable cursor into the visible provider/account roster.
    pub selected_provider: usize,
    /// The editable startup preference shown by Preferences.
    pub startup_view: StartupView,
    pub(super) saved_startup_view: StartupView,
    pub(super) return_view: DashboardView,
    pub(super) save_failed: bool,
}

impl Default for DashboardConfig {
    fn default() -> Self {
        Self {
            user_hidden: BTreeSet::new(),
            scroll: 0,
            color: false,
            provider_order: Vec::new(),
            meter_mode: MeterMode::Remaining,
            provider_identity: ProviderIdentityMode::LogoAndName,
            logo_glyphs: true,
            interactive: false,
            first_run: false,
            transition_count: 0,
            status: None,
            retry_minutes: None,
            history: None,
            view: DashboardView::Overview,
            selected_provider: 0,
            startup_view: StartupView::Overview,
            saved_startup_view: StartupView::Overview,
            return_view: DashboardView::Overview,
            save_failed: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DashboardStatus {
    Refreshing,
    Timeout,
    MissingExecutable,
    IncompatibleOutput,
    NetworkProcess,
}

impl DashboardStatus {
    fn is_failure(self) -> bool {
        self != Self::Refreshing
    }
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
    let Some(value) = value.filter(|value| value.is_finite()) else {
        return "unavailable".into();
    };
    let value = value.clamp(0.0, 100.0);
    if value.fract() == 0.0 {
        format!("{value:.0}%")
    } else {
        let bounded = (value * 100.0).floor() / 100.0;
        let mut value = format!("{bounded:.2}");
        while value.ends_with('0') {
            value.pop();
        }
        format!("{value}%")
    }
}

fn visible_marketed(config: &DashboardConfig) -> Vec<MarketedProvider> {
    let mut providers = MarketedProvider::ALL
        .into_iter()
        .filter(|provider| !config.user_hidden.contains(provider.id()))
        .collect::<Vec<_>>();
    if !config.provider_order.is_empty() {
        providers.sort_by_key(|provider| {
            config
                .provider_order
                .iter()
                .position(|id| id == provider.id())
                .unwrap_or(usize::MAX)
        });
    }
    providers
}

#[derive(Clone, Copy)]
struct VisibleTarget {
    marketed: MarketedProvider,
    report_index: Option<usize>,
    account_number: Option<usize>,
}

fn visible_targets(report: &QuotaReport, config: &DashboardConfig) -> Vec<VisibleTarget> {
    let mut targets = Vec::new();
    for marketed in visible_marketed(config) {
        let matching = report
            .providers
            .iter()
            .enumerate()
            .filter(|(_, provider)| provider_id(provider) == Some(marketed))
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        if matching.is_empty() {
            targets.push(VisibleTarget {
                marketed,
                report_index: None,
                account_number: None,
            });
        } else {
            let multiple = matching.len() > 1;
            targets.extend(matching.into_iter().enumerate().map(
                |(account_index, report_index)| VisibleTarget {
                    marketed,
                    report_index: Some(report_index),
                    account_number: multiple.then_some(account_index + 1),
                },
            ));
        }
    }
    targets
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
    let mut model = dashboard_model(report, config.meter_mode, &visibility);
    if !config.provider_order.is_empty() {
        model.providers.sort_by_key(|section| {
            config
                .provider_order
                .iter()
                .position(|id| id == section.provider.id())
                .unwrap_or(usize::MAX)
        });
    }
    model
}

pub(super) fn visible_provider_count(report: &QuotaReport, config: &DashboardConfig) -> usize {
    visible_targets(report, config).len()
}

fn compact_provider_name(provider: MarketedProvider) -> &'static str {
    match provider {
        MarketedProvider::Codex => "Codex",
        MarketedProvider::Copilot => "Copilot",
        other => other.label(),
    }
}

fn narrow_provider_name(provider: MarketedProvider) -> &'static str {
    if provider == MarketedProvider::Copilot {
        "GitHub"
    } else {
        compact_provider_name(provider)
    }
}

fn boundary_provider_name(provider: MarketedProvider) -> &'static str {
    match provider {
        MarketedProvider::Claude => "C",
        MarketedProvider::Codex => "O",
        MarketedProvider::Cursor => "U",
        MarketedProvider::Kimi => "K",
        MarketedProvider::Grok => "G",
        MarketedProvider::Copilot => "H",
    }
}

fn provider_logo_glyph(provider: MarketedProvider) -> &'static str {
    match provider {
        MarketedProvider::Claude => "✳ ",
        MarketedProvider::Codex => "⌬ ",
        MarketedProvider::Cursor => "◩ ",
        MarketedProvider::Kimi => "◐ ",
        MarketedProvider::Grok => "╳ ",
        MarketedProvider::Copilot => "◉◉",
    }
}

fn provider_logo_fallback(provider: MarketedProvider) -> &'static str {
    match provider {
        MarketedProvider::Claude => "CL",
        MarketedProvider::Codex => "OA",
        MarketedProvider::Cursor => "CU",
        MarketedProvider::Kimi => "KM",
        MarketedProvider::Grok => "xA",
        MarketedProvider::Copilot => "GH",
    }
}

fn provider_identity(
    provider: MarketedProvider,
    mode: ProviderIdentityMode,
    logo_glyphs: bool,
) -> String {
    let mark = if logo_glyphs {
        provider_logo_glyph(provider)
    } else {
        provider_logo_fallback(provider)
    };
    match mode {
        ProviderIdentityMode::LogoOnly => mark.into(),
        ProviderIdentityMode::LogoAndName => {
            format!("{mark} {}", compact_provider_name(provider))
        }
        ProviderIdentityMode::NameOnly => compact_provider_name(provider).into(),
    }
}

fn compact_annotation(value: &str) -> &str {
    match value {
        "signed out" => "out",
        "rate limited" => "rate",
        "unavailable" => "down",
        "non-current" => "old",
        "partial data" => "partial",
        "no reading" | "consumer quota unavailable" => "none",
        other => other,
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

fn countdown(reset: &str, generated_at: &str) -> Option<String> {
    let reset = chrono::DateTime::parse_from_rfc3339(reset).ok()?;
    let generated = chrono::DateTime::parse_from_rfc3339(generated_at).ok()?;
    let seconds = (reset.timestamp() - generated.timestamp()).max(0) as u64;
    if seconds == 0 {
        return Some("now".into());
    }
    if seconds < 60 {
        return Some("<1m".into());
    }
    let minutes = seconds / 60;
    let days = minutes / (24 * 60);
    let hours = (minutes / 60) % 24;
    let minutes = minutes % 60;
    Some(if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    })
}

fn reset_context(
    resets_at: Option<&str>,
    reset_text: Option<&str>,
    generated_at: &str,
    width: usize,
) -> String {
    let Some(resets_at) = resets_at else {
        return reset_text
            .map(|value| fitting([format!("reset {value}"), value.to_owned()], width))
            .unwrap_or_else(|| "reset unavailable".into());
    };
    let timestamp = moment(Some(resets_at), width <= 23).unwrap_or_else(|| "unavailable".into());
    let remaining = countdown(resets_at, generated_at);
    fitting(
        [
            remaining
                .as_ref()
                .map(|remaining| format!("reset {timestamp} · in {remaining}"))
                .unwrap_or_default(),
            remaining
                .as_ref()
                .map(|remaining| format!("in {remaining}"))
                .unwrap_or_default(),
            format!("reset {timestamp}"),
            "reset unavailable".into(),
        ],
        width,
    )
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
    report: &QuotaReport,
    section: &crate::ui::model::ProviderSection,
    provider: &ProviderQuota,
) -> bool {
    quota_readiness(report, provider, section.provider) == ProviderReadiness::Live
        && has_trustworthy_quota(provider)
        && !hidden_sibling_unsafe(section, provider)
}

fn readiness_row(
    provider: MarketedProvider,
    readiness: &ProviderReadiness,
    selected: bool,
    width: usize,
    account_number: Option<usize>,
) -> SemanticRow {
    let state = readiness.text();
    let cursor = if selected { '>' } else { ' ' };
    let compact = account_number.map_or_else(
        || compact_provider_name(provider).to_owned(),
        |number| format!("{} A{number}", compact_provider_name(provider)),
    );
    let narrow = account_number.map_or_else(
        || narrow_provider_name(provider).to_owned(),
        |number| format!("{} A{number}", narrow_provider_name(provider)),
    );
    let marker = if selected { '>' } else { '?' };
    let text = fitting(
        [
            format!("{cursor}?{compact} · {state}"),
            format!("{cursor}?{compact} {state}"),
            format!("{cursor}?{narrow} {state}"),
            format!(
                "{marker}{}{} {state}",
                boundary_provider_name(provider),
                account_number.map_or_else(String::new, |number| number.to_string())
            ),
        ],
        width,
    );
    SemanticRow {
        text,
        style: RowStyle::Warning,
    }
}

fn provider_heading(
    provider: MarketedProvider,
    identity_mode: ProviderIdentityMode,
    logo_glyphs: bool,
    selected: bool,
    width: usize,
) -> SemanticRow {
    let cursor = if selected { '>' } else { ' ' };
    SemanticRow {
        text: truncate(
            &format!(
                "{cursor}{}",
                provider_identity(provider, identity_mode, logo_glyphs)
            ),
            width,
        ),
        style: RowStyle::Heading,
    }
}

fn overview_account_heading(
    provider: &ProviderQuota,
    account_number: Option<usize>,
    selected: bool,
    width: usize,
) -> SemanticRow {
    let cursor = if selected { '>' } else { ' ' };
    let number = account_number.unwrap_or(1);
    let base = format!("{cursor} Account {number}");
    let text = provider
        .account_label
        .as_deref()
        .map_or(base.clone(), |label| {
            let prefix = format!("{base} · ");
            let budget = width.saturating_sub(UnicodeWidthStr::width(prefix.as_str()));
            format!("{prefix}{}", elide_middle(label, budget))
        });
    SemanticRow {
        text: truncate(&text, width),
        style: RowStyle::Heading,
    }
}

fn overview_window_rows(
    section: &crate::ui::model::ProviderSection,
    provider: &ProviderQuota,
    report: &QuotaReport,
    width: usize,
) -> Vec<SemanticRow> {
    let annotation = section.annotation.as_ref().map(|value| value.text);
    let mut rows = Vec::new();
    if let Some(annotation) = annotation {
        rows.push(SemanticRow {
            text: fitting(
                [
                    format!("  ? {annotation}"),
                    format!("  ? {}", compact_annotation(annotation)),
                ],
                width,
            ),
            style: RowStyle::Warning,
        });
    }
    let ProviderDetail::Tiers(tiers) = &section.detail else {
        let message = match &section.detail {
            ProviderDetail::Recovery { instruction } => format!("  ? {instruction}"),
            ProviderDetail::Message { message } => format!("  ? {message}"),
            ProviderDetail::Tiers(_) => unreachable!(),
        };
        rows.push(SemanticRow {
            text: fitting([message, "  ? unavailable".into()], width),
            style: RowStyle::Warning,
        });
        return rows;
    };

    for tier in tiers {
        let displayed = tier.displayed_percent;
        let numeric = percent(displayed);
        let critical = tier.percent_remaining.is_some_and(|value| value <= 10.0);
        let missing = displayed.is_none();
        let style = if critical {
            RowStyle::Critical
        } else if missing {
            RowStyle::Warning
        } else {
            RowStyle::Normal
        };
        let label = if width >= 30 {
            elide_middle(&tier.label, 12)
        } else {
            elide_middle(&tier.compact_label, width.saturating_sub(numeric.len() + 4))
        };
        let marker = if critical {
            '!'
        } else if missing {
            '?'
        } else {
            ' '
        };
        if width >= 30 {
            let bar = remaining_bar(displayed, 8);
            rows.push(SemanticRow {
                text: fitting(
                    [
                        format!(" {marker}{label:<12} {bar} {numeric}"),
                        format!(" {marker}{label} {numeric}"),
                    ],
                    width,
                ),
                style,
            });
        } else {
            rows.push(SemanticRow {
                text: fitting(
                    [
                        format!(" {marker}{label} {numeric}"),
                        format!(" {marker}{numeric}"),
                    ],
                    width,
                ),
                style,
            });
            if displayed.is_some() {
                rows.push(SemanticRow {
                    text: format!("   {}", remaining_bar(displayed, 8)),
                    style,
                });
            }
        }
        let reset_text = provider
            .windows
            .iter()
            .find(|window| window.id == tier.id)
            .and_then(|window| window.reset_text.as_deref());
        let reset = reset_context(
            tier.resets_at.as_deref(),
            reset_text,
            &report.generated_at,
            width.saturating_sub(3),
        );
        rows.push(SemanticRow {
            text: format!("   {reset}"),
            style: if tier.resets_at.is_none() && reset_text.is_none() {
                RowStyle::Warning
            } else {
                RowStyle::Normal
            },
        });
    }
    rows
}

fn overview_rows(report: &QuotaReport, config: &DashboardConfig, width: u16) -> Vec<SemanticRow> {
    let width = usize::from(width);
    let targets = visible_targets(report, config);
    let selected = config
        .selected_provider
        .min(targets.len().saturating_sub(1));
    let mut rows = Vec::new();
    let mut last_provider = None;
    for (index, target) in targets.into_iter().enumerate() {
        let first_account = last_provider != Some(target.marketed);
        if first_account {
            rows.push(provider_heading(
                target.marketed,
                config.provider_identity,
                config.logo_glyphs,
                index == selected,
                width,
            ));
            last_provider = Some(target.marketed);
        }
        let Some(report_index) = target.report_index else {
            rows.push(SemanticRow {
                text: fitting(["  ? unsupported".into(), "? unsupported".into()], width),
                style: RowStyle::Warning,
            });
            continue;
        };
        let provider = &report.providers[report_index];
        rows.push(overview_account_heading(
            provider,
            target.account_number,
            index == selected,
            width,
        ));
        let section = provider_section(target.marketed, provider, config.meter_mode);
        rows.extend(overview_window_rows(&section, provider, report, width));
    }
    rows
}

fn overview_selected_anchor(rows: &[SemanticRow], viewport: usize) -> usize {
    let selected = rows
        .iter()
        .position(|row| row.text.starts_with("> Account"))
        .or_else(|| rows.iter().position(|row| row.text.starts_with('>')))
        .unwrap_or(0);
    let end = rows
        .iter()
        .enumerate()
        .skip(selected + 1)
        .find(|(_, row)| row.style == RowStyle::Heading)
        .map_or(rows.len(), |(index, _)| index)
        .saturating_sub(1);
    end.min(selected.saturating_add(viewport.saturating_sub(1)))
}

fn overview_evidence_rows(
    report: &QuotaReport,
    config: &DashboardConfig,
    width: u16,
) -> Vec<SemanticRow> {
    let model = visible_model(report, config);
    let targets = visible_targets(report, config);
    let selected = targets
        .get(
            config
                .selected_provider
                .min(targets.len().saturating_sub(1)),
        )
        .map(|target| target.marketed);
    let trustworthy: Vec<_> = model
        .providers
        .iter()
        .filter_map(|section| {
            report
                .providers
                .iter()
                .find(|provider| provider_id(provider) == Some(section.provider))
                .filter(|provider| has_decision_safe_quota(report, section, provider))
        })
        .collect();
    let decision_provider =
        decision_constraint(&trustworthy).and_then(|(provider, _)| provider_id(provider));
    let mut rows = Vec::new();
    for section in &model.providers {
        if selected == Some(section.provider) || decision_provider == Some(section.provider) {
            continue;
        }
        if report
            .providers
            .iter()
            .filter(|provider| provider_id(provider) == Some(section.provider))
            .count()
            > 1
        {
            continue;
        }
        let Some(provider) = report
            .providers
            .iter()
            .find(|provider| provider_id(provider) == Some(section.provider))
        else {
            continue;
        };
        if !has_decision_safe_quota(report, section, provider) {
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
        }) {
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

fn compact_duration(seconds: i64) -> String {
    let seconds = seconds.max(0);
    if seconds >= 60 * 60 {
        let rounded = (seconds as f64 / (60 * 60) as f64).round() as i64;
        if seconds % (60 * 60) == 0 {
            format!("{rounded}h")
        } else {
            format!("~{rounded}h")
        }
    } else if seconds >= 60 {
        let rounded = (seconds as f64 / 60.0).round() as i64;
        if seconds % 60 == 0 {
            format!("{rounded}m")
        } else {
            format!("~{rounded}m")
        }
    } else {
        format!("{seconds}s")
    }
}

fn trend_consequence(trend: &HistoryTrend) -> String {
    let elapsed = compact_duration(trend.elapsed_seconds);
    let amount = trend.evidence.amount.unwrap_or_default().abs();
    match trend.evidence.kind {
        HistoryEvidenceKind::Reset => "↻ reset".into(),
        HistoryEvidenceKind::RemainingDrop => format!("↓ {amount}pp/{elapsed}"),
        HistoryEvidenceKind::RemainingGain => format!("↑ {amount}pp/{elapsed}"),
        HistoryEvidenceKind::PaceWorse => format!("↓ pace/{elapsed}"),
        HistoryEvidenceKind::PaceBetter => format!("↑ pace/{elapsed}"),
        HistoryEvidenceKind::ProjectionEarlier => trend
            .evidence
            .amount
            .filter(|value| *value != 0)
            .map_or_else(
                || "↘ out sooner".into(),
                |value| format!("↘ out {} sooner", compact_duration(value.abs())),
            ),
        HistoryEvidenceKind::ProjectionLater => trend
            .evidence
            .amount
            .filter(|value| *value != 0)
            .map_or_else(
                || "↗ out later".into(),
                |value| format!("↗ out {} later", compact_duration(value.abs())),
            ),
    }
}

fn trend_row(
    history: Option<&HistoryDocument>,
    provider: MarketedProvider,
    width: usize,
) -> Option<SemanticRow> {
    if width < 30 {
        return None;
    }
    let trend = history_trend(history?, provider)?;
    let consequence = trend_consequence(&trend);
    let subject = trend.evidence.limit.as_deref().or_else(|| {
        (!matches!(trend.evidence.scope.as_str(), "All models" | "All products"))
            .then_some(trend.evidence.scope.as_str())
    });
    let mut candidates = Vec::new();
    if let Some(subject) = subject {
        candidates.push(format!("  {}  {subject} {consequence}", trend.cells));
    }
    candidates.push(format!("  {}  {consequence}", trend.cells));
    candidates.push(format!("  {} {consequence}", trend.cells));
    let text = fitting(candidates, width);
    (!text.is_empty()).then_some(SemanticRow {
        text,
        style: RowStyle::Normal,
    })
}

fn take_cells(value: &str, width: usize) -> String {
    let mut shown = String::new();
    let mut cells = 0;
    for character in value.chars() {
        let next = UnicodeWidthChar::width(character).unwrap_or(0);
        if cells + next > width {
            break;
        }
        shown.push(character);
        cells += next;
    }
    shown
}

fn take_cells_from_end(value: &str, width: usize) -> String {
    let mut shown = Vec::new();
    let mut cells = 0;
    for character in value.chars().rev() {
        let next = UnicodeWidthChar::width(character).unwrap_or(0);
        if cells + next > width {
            break;
        }
        shown.push(character);
        cells += next;
    }
    shown.into_iter().rev().collect()
}

fn elide_middle(value: &str, width: usize) -> String {
    if UnicodeWidthStr::width(value) <= width {
        return value.to_owned();
    }
    if width <= 1 {
        return "…".chars().take(width).collect();
    }
    let left = (width - 1) / 2;
    let right = width - 1 - left;
    format!(
        "{}…{}",
        take_cells(value, left),
        take_cells_from_end(value, right)
    )
}

fn account_heading(provider: &ProviderQuota, index: usize, count: usize, width: usize) -> String {
    let number = index + 1;
    let fallback = format!("Account {number}");
    let Some(label) = provider.account_label.as_deref() else {
        return format!("  {fallback}");
    };
    let prefix = if count > 1 {
        format!("  {fallback} · ")
    } else {
        format!(
            "> {} · ",
            compact_provider_name(provider_id(provider).unwrap())
        )
    };
    let budget = width.saturating_sub(UnicodeWidthStr::width(prefix.as_str()));
    format!("{prefix}{}", elide_middle(label, budget))
}

fn account_tier_rows(
    section: &crate::ui::model::ProviderSection,
    current: bool,
    width: usize,
    nested: bool,
) -> Vec<SemanticRow> {
    let indent = if nested { "   " } else { " " };
    match &section.detail {
        ProviderDetail::Recovery { instruction } => vec![SemanticRow {
            text: fitting(
                [
                    format!("{indent}? {instruction}"),
                    format!("{indent}? sign in"),
                ],
                width,
            ),
            style: RowStyle::Warning,
        }],
        ProviderDetail::Message { message } => vec![SemanticRow {
            text: fitting(
                [
                    format!("{indent}? {message}"),
                    format!("{indent}? unavailable"),
                ],
                width,
            ),
            style: RowStyle::Warning,
        }],
        ProviderDetail::Tiers(tiers) => {
            let mut rows = Vec::new();
            for tier in tiers {
                let critical = tier.percent_remaining.is_some_and(|value| value <= 10.0);
                let missing = tier.displayed_percent.is_none();
                let marker = if critical {
                    '!'
                } else if missing {
                    '?'
                } else {
                    ' '
                };
                let label_source = if width >= 30 {
                    &tier.label
                } else {
                    &tier.compact_label
                };
                let label_budget = if width >= 30 { 13 } else { 9 };
                let label = elide_middle(label_source, label_budget);
                let remaining = percent(tier.displayed_percent);
                let status = if current { "" } else { " last" };
                let bar = tier
                    .displayed_percent
                    .map(|value| remaining_bar(Some(value), 8));
                let usage = if width >= 30 {
                    bar.as_ref().map_or_else(
                        || format!("{indent}{marker}{label} {remaining}{status}"),
                        |bar| format!("{indent}{marker}{label:<13} {bar} {remaining}{status}"),
                    )
                } else {
                    format!("{indent}{marker}{label} {remaining}{status}")
                };
                let reset = tier
                    .resets_at
                    .as_deref()
                    .and_then(|value| moment(Some(value), width <= 23))
                    .unwrap_or_else(|| "unavailable".into());
                let style = if critical {
                    RowStyle::Critical
                } else if missing {
                    RowStyle::Warning
                } else {
                    RowStyle::Normal
                };
                rows.push(SemanticRow {
                    text: fitting(
                        [usage, format!("{indent}{marker}{remaining}{status}")],
                        width,
                    ),
                    style,
                });
                if width < 30
                    && let Some(bar) = bar
                {
                    rows.push(SemanticRow {
                        text: format!("{indent}  {bar}"),
                        style,
                    });
                }
                rows.push(SemanticRow {
                    text: fitting(
                        [
                            format!("{indent}reset {reset}"),
                            format!("{indent}reset unavailable"),
                        ],
                        width,
                    ),
                    style: if tier.resets_at.is_none() {
                        RowStyle::Warning
                    } else {
                        RowStyle::Normal
                    },
                });
            }
            rows
        }
    }
}

fn account_detail_rows(
    report: &QuotaReport,
    history: Option<&HistoryDocument>,
    config: &DashboardConfig,
    target: VisibleTarget,
    provider: &ProviderQuota,
    account_count: usize,
    width: usize,
) -> Vec<SemanticRow> {
    let selected = target.marketed;
    let account_index = target.account_number.unwrap_or(1) - 1;
    let multiple = account_count > 1;
    let mut rows = Vec::new();
    if multiple {
        rows.push(SemanticRow {
            text: format!("> {}", selected.label()),
            style: RowStyle::Heading,
        });
    }
    let section = provider_section(selected, provider, config.meter_mode);
    if !multiple && has_decision_safe_quota(report, &section, provider) {
        rows.extend(trend_row(history, selected, width));
    }
    let annotation = section.annotation.as_ref().map(|value| value.text);
    let mut heading = account_heading(provider, account_index, account_count, width);
    if let Some(annotation) = annotation {
        heading = fitting(
            [
                format!("{heading} · {annotation}"),
                heading,
                format!(
                    "  Account {} · {}",
                    account_index + 1,
                    compact_annotation(annotation)
                ),
            ],
            width,
        );
    }
    rows.push(SemanticRow {
        text: heading,
        style: if annotation.is_some() {
            RowStyle::Warning
        } else {
            RowStyle::Heading
        },
    });
    rows.extend(account_tier_rows(
        &section,
        has_current_quota(provider),
        width,
        multiple,
    ));
    rows
}

fn detail_rows_with_history(
    report: &QuotaReport,
    history: Option<&HistoryDocument>,
    config: &DashboardConfig,
    width: u16,
) -> Vec<SemanticRow> {
    let targets = visible_targets(report, config);
    let Some(target) = targets
        .get(
            config
                .selected_provider
                .min(targets.len().saturating_sub(1)),
        )
        .copied()
    else {
        return Vec::new();
    };
    let selected = target.marketed;
    let Some(report_index) = target.report_index else {
        return vec![readiness_row(
            selected,
            &ProviderReadiness::Unsupported,
            true,
            width as usize,
            None,
        )];
    };
    let provider = &report.providers[report_index];
    let account_count = report
        .providers
        .iter()
        .filter(|provider| provider_id(provider) == Some(selected))
        .count();
    if account_count > 1 || provider.account_reported {
        return account_detail_rows(
            report,
            history,
            config,
            target,
            provider,
            account_count,
            width as usize,
        );
    }
    let section = provider_section(selected, provider, config.meter_mode);
    let mut rows = Vec::new();
    {
        let current = has_current_quota(provider);
        let annotation = section.annotation.as_ref().map(|value| value.text);
        rows.push(SemanticRow {
            text: annotation
                .map(|value| {
                    fitting(
                        [
                            format!("> {} · {value}", section.label),
                            format!(
                                ">?{} {}",
                                narrow_provider_name(section.provider),
                                compact_annotation(value)
                            ),
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
        if has_decision_safe_quota(report, &section, provider) {
            rows.extend(trend_row(history, section.provider, width as usize));
        }
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
                        [format!("  ? {message}"), compact, "  ? unavailable".into()],
                        width as usize,
                    ),
                    style: RowStyle::Warning,
                })
            }
            ProviderDetail::Tiers(tiers) => {
                for tier in tiers {
                    let percent = percent(tier.displayed_percent);
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
                    let meter = if width >= 30 && tier.displayed_percent.is_some() {
                        format!(" {}", remaining_bar(tier.displayed_percent, 6))
                    } else {
                        String::new()
                    };
                    rows.push(SemanticRow {
                        text: fitting(
                            [
                                format!("{aligned}{meter}"),
                                format!("{aligned}{candidate_conclusion}"),
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
                    if width < 30
                        && let Some(displayed) = tier.displayed_percent
                    {
                        rows.push(SemanticRow {
                            text: format!("   {}", remaining_bar(Some(displayed), 8)),
                            style: if critical {
                                RowStyle::Critical
                            } else {
                                RowStyle::Normal
                            },
                        });
                    }
                    if !candidate_conclusion.is_empty() {
                        rows.push(SemanticRow {
                            text: fitting(
                                [
                                    format!("   {}", candidate_conclusion.trim()),
                                    candidate_conclusion.trim().into(),
                                ],
                                width as usize,
                            ),
                            style: if warning {
                                RowStyle::Warning
                            } else {
                                RowStyle::Normal
                            },
                        });
                    }
                }
            }
        }
    }
    rows
}

fn semantic_rows_with_history(
    report: &QuotaReport,
    history: Option<&HistoryDocument>,
    config: &DashboardConfig,
    width: u16,
) -> Vec<SemanticRow> {
    match config.view {
        DashboardView::Overview => overview_rows(report, config, width),
        DashboardView::Details => detail_rows_with_history(report, history, config, width),
        DashboardView::Preferences | DashboardView::TransitionReview => Vec::new(),
    }
}

#[cfg(test)]
fn semantic_rows(report: &QuotaReport, config: &DashboardConfig, width: u16) -> Vec<SemanticRow> {
    semantic_rows_with_history(report, None, config, width)
}

fn decision_constraint<'a>(
    trustworthy: &[&'a ProviderQuota],
) -> Option<(&'a ProviderQuota, &'a EffectiveAvailability)> {
    trustworthy
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
                        *provider,
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
        })
        .map(|(_, _, _, _, _, provider, effective)| (provider, effective))
}

fn attention(report: &QuotaReport, config: &DashboardConfig, width: usize) -> (String, RowStyle) {
    let marketed = visible_marketed(config);
    let visible = report
        .providers
        .iter()
        .filter_map(|provider| {
            let provider_kind = provider_id(provider)?;
            marketed.contains(&provider_kind).then(|| {
                (
                    provider_section(provider_kind, provider, config.meter_mode),
                    provider,
                )
            })
        })
        .collect::<Vec<_>>();
    if marketed.is_empty() {
        return ("? No providers shown".into(), RowStyle::Warning);
    }
    let trustworthy: Vec<_> = visible
        .iter()
        .filter_map(|(section, provider)| {
            has_decision_safe_quota(report, section, provider).then_some(*provider)
        })
        .collect();
    if let Some((provider, effective)) = decision_constraint(&trustworthy) {
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
    let readiness = marketed
        .iter()
        .map(|provider| provider_readiness(report, *provider))
        .filter(|state| !matches!(state, ProviderReadiness::Unsupported))
        .collect::<Vec<_>>();
    if readiness.iter().any(|state| {
        matches!(
            state,
            ProviderReadiness::Auth
                | ProviderReadiness::Stale(_)
                | ProviderReadiness::QuotaUnavailable
                | ProviderReadiness::Unsupported
        )
    }) {
        return (
            fitting(
                ["? Limits non-current".into(), "? Non-current".into()],
                width,
            ),
            RowStyle::Warning,
        );
    }
    if readiness
        .iter()
        .any(|state| matches!(state, ProviderReadiness::Partial))
        || visible
            .iter()
            .any(|(section, provider)| !has_decision_safe_quota(report, section, provider))
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
    render_lines_with_history(report, None, width, height, config)
}

/// Render with an optional retained safe-history document.
pub fn render_lines_with_history(
    report: &QuotaReport,
    history: Option<&HistoryDocument>,
    width: u16,
    height: u16,
    config: &DashboardConfig,
) -> Vec<String> {
    render_frame(Some(report), history, width, height, config)
        .into_iter()
        .map(|row| row.text)
        .collect()
}

/// Return one live dashboard frame, including bounded first-load state.
pub fn render_dashboard_lines(
    report: Option<&QuotaReport>,
    width: u16,
    height: u16,
    config: &DashboardConfig,
) -> Vec<String> {
    render_frame(report, None, width, height, config)
        .into_iter()
        .map(|row| row.text)
        .collect()
}

fn empty_report() -> QuotaReport {
    QuotaReport {
        generated_at: String::new(),
        schema_version: 5,
        providers: Vec::new(),
        adaptation_warnings: Vec::new(),
    }
}

fn age_text(generated_at: &str) -> (String, String) {
    let generated = chrono::DateTime::parse_from_rfc3339(generated_at)
        .ok()
        .map(|value| value.timestamp())
        .unwrap_or_default();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let seconds = now.saturating_sub(generated).max(0) as u64;
    match seconds {
        0..=59 => ("just now".into(), "now".into()),
        60..=3_599 => {
            let minutes = seconds / 60;
            (format!("{minutes}m ago"), format!("{minutes}m"))
        }
        3_600..=86_399 => {
            let hours = seconds / 3_600;
            (format!("{hours}h ago"), format!("{hours}h"))
        }
        _ => {
            let days = seconds / 86_400;
            (format!("{days}d ago"), format!("{days}d"))
        }
    }
}

fn title_with_activity(
    title: &str,
    report: Option<&QuotaReport>,
    config: &DashboardConfig,
    width: usize,
) -> String {
    if !config.interactive {
        return frame_text(title, width);
    }
    let (long, compact) = if config.status == Some(DashboardStatus::Refreshing) {
        ("refreshing".into(), "↻".into())
    } else if let Some(report) = report {
        let (long_age, compact_age) = age_text(&report.generated_at);
        if config.status.is_some_and(DashboardStatus::is_failure) {
            (format!("last {long_age}"), format!("last {compact_age}"))
        } else {
            (long_age, compact_age)
        }
    } else {
        ("not updated".into(), "never".into())
    };
    for shown_title in [title, "Herdr Quota"] {
        for activity in [&long, &compact] {
            let title_width = UnicodeWidthStr::width(shown_title);
            let activity_width = UnicodeWidthStr::width(activity.as_str());
            if title_width + 1 + activity_width <= width {
                let gap = width - title_width - activity_width;
                return format!("{shown_title}{}{activity}", " ".repeat(gap));
            }
        }
    }
    frame_text(title, width)
}

fn retry_text(config: &DashboardConfig) -> String {
    config
        .retry_minutes
        .filter(|minutes| *minutes > 0)
        .map(|minutes| format!("{minutes}m"))
        .unwrap_or_else(|| "soon".into())
}

fn failure_line(config: &DashboardConfig, width: usize) -> Option<String> {
    let status = config.status.filter(|status| status.is_failure())?;
    let retry = retry_text(config);
    let labels = match status {
        DashboardStatus::Timeout => vec![
            format!("? Quota check timed out · retry {retry}"),
            format!("Timeout · retry {retry}"),
            format!("Timeout · {retry}"),
        ],
        DashboardStatus::MissingExecutable => vec![
            format!("? quota-axi missing · retry {retry}"),
            format!("Missing · retry {retry}"),
            format!("Missing · {retry}"),
        ],
        DashboardStatus::IncompatibleOutput => vec![
            format!("? Incompatible output · retry {retry}"),
            format!("Schema · retry {retry}"),
            format!("Schema · {retry}"),
        ],
        DashboardStatus::NetworkProcess => vec![
            format!("? Network/process failed · retry {retry}"),
            format!("Failed · retry {retry}"),
            format!("Failed · {retry}"),
        ],
        DashboardStatus::Refreshing => return None,
    };
    Some(fitting(labels, width))
}

fn render_frame(
    report: Option<&QuotaReport>,
    history: Option<&HistoryDocument>,
    width: u16,
    height: u16,
    config: &DashboardConfig,
) -> Vec<SemanticRow> {
    let width = width.max(1) as usize;
    let height = height.max(1) as usize;
    if config.view == DashboardView::Preferences {
        return render_preferences(width, height, config);
    }
    let no_report = report.is_none();
    let empty = empty_report();
    let report_value = report.unwrap_or(&empty);
    let mut rows = if no_report {
        vec![
            SemanticRow {
                text: "No quota readings".into(),
                style: RowStyle::Warning,
            },
            SemanticRow {
                text: if config.status == Some(DashboardStatus::Refreshing) {
                    fitting(
                        ["Collection in progress".into(), "Collecting quota".into()],
                        width,
                    )
                } else {
                    fitting(
                        ["Press r to retry now".into(), "Press r to retry".into()],
                        width,
                    )
                },
                style: RowStyle::Normal,
            },
        ]
    } else {
        match config.view {
            DashboardView::Overview | DashboardView::Details => {
                semantic_rows_with_history(report_value, history, config, width as u16)
            }
            DashboardView::TransitionReview => vec![SemanticRow {
                text: "No new transition events".into(),
                style: RowStyle::Normal,
            }],
            DashboardView::Preferences => unreachable!("handled above"),
        }
    };
    let title = if matches!(
        config.view,
        DashboardView::Overview | DashboardView::Details
    ) {
        let targets = visible_targets(report_value, config);
        targets
            .get(
                config
                    .selected_provider
                    .min(targets.len().saturating_sub(1)),
            )
            .map(|target| {
                fitting(
                    [
                        format!("Herdr Quota · {}", compact_provider_name(target.marketed)),
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
    let title = if matches!(
        config.view,
        DashboardView::Overview | DashboardView::Details
    ) && config.meter_mode == MeterMode::Used
    {
        fitting([format!("{title} · used"), "Quota · used".into()], width)
    } else {
        title
    };
    let (attention, attention_style) = if let Some(failure) = failure_line(config, width) {
        (failure, RowStyle::Warning)
    } else if no_report {
        ("? Waiting for quota".into(), RowStyle::Warning)
    } else {
        attention(report_value, config, width)
    };
    let controls = match (config.view, width >= 30, config.interactive) {
        (DashboardView::Overview, true, true) => "j/k · enter details · p · r · q",
        (DashboardView::Overview, false, true) => "j/k enter p r q",
        (DashboardView::Details, true, true) => "j/k · esc overview · r · q",
        (DashboardView::Details, false, true) => "j/k esc r q",
        (DashboardView::Overview, true, false) => "j/k · enter details · p · q",
        (DashboardView::Overview, false, false) => "j/k enter p q",
        (DashboardView::Details, true, false) => "j/k · esc overview · q",
        (DashboardView::Details, false, false) => "j/k esc q",
        (DashboardView::TransitionReview, true, _) => "a/enter acknowledge · esc",
        (DashboardView::TransitionReview, false, _) => "a/enter ack · esc",
        (DashboardView::Preferences, _, _) => unreachable!("handled above"),
    };
    let show_readiness = config.first_run && !no_report && config.view == DashboardView::Overview;
    let body_start = if show_readiness {
        3.min(height)
    } else if config.view == DashboardView::Details && height >= 5 {
        3
    } else {
        2.min(height)
    };
    let footer = height.saturating_sub(1);
    let body_end = footer.max(body_start);
    let viewport = body_end.saturating_sub(body_start);
    if config.view == DashboardView::Overview
        && let Some(report) = report
    {
        let spare = viewport.saturating_sub(rows.len());
        rows.extend(
            overview_evidence_rows(report, config, width as u16)
                .into_iter()
                .take(spare),
        );
    }
    let scroll = if config.view == DashboardView::Overview {
        overview_selected_anchor(&rows, viewport)
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
        text: title_with_activity(&title, report, config, width),
        style: RowStyle::Heading,
    };
    if height > 1 {
        output[1] = SemanticRow {
            text: frame_text(&attention, width),
            style: attention_style,
        };
    }
    if show_readiness && height > 2 {
        output[2] = SemanticRow {
            text: frame_text(&readiness_line(report_value), width),
            style: RowStyle::Heading,
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

pub(super) fn clamp_scroll(
    report: &QuotaReport,
    width: u16,
    height: u16,
    config: &DashboardConfig,
) -> usize {
    let rows =
        semantic_rows_with_history(report, config.history.as_ref(), config, width.max(1)).len();
    let height = usize::from(height.max(1));
    let body_start = if config.view == DashboardView::Details && height >= 5 {
        3
    } else {
        2.min(height)
    };
    let viewport = height.saturating_sub(1).saturating_sub(body_start);
    config.scroll.min(rows.saturating_sub(viewport))
}

pub(super) fn draw_dashboard(
    frame: &mut Frame<'_>,
    report: Option<&QuotaReport>,
    config: &DashboardConfig,
) {
    let area = frame.area();
    let rows = render_frame(
        report,
        config.history.as_ref(),
        area.width,
        area.height,
        config,
    );
    frame.render_widget(
        Dashboard {
            rows: &rows,
            config,
        },
        area,
    );
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
    let rows = render_frame(Some(report), config.history.as_ref(), width, height, config);
    let area = Rect::new(0, 0, width.max(1), height.max(1));
    let mut buffer = Buffer::empty(area);
    Dashboard {
        rows: &rows,
        config,
    }
    .render(area, &mut buffer);
    buffer
}

/// Render one live dashboard state through Ratatui to a fixed buffer.
pub fn render_dashboard_buffer(
    report: Option<&QuotaReport>,
    width: u16,
    height: u16,
    config: &DashboardConfig,
) -> Buffer {
    let rows = render_frame(report, config.history.as_ref(), width, height, config);
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
        r##"<svg xmlns="http://www.w3.org/2000/svg" role="img" aria-labelledby="title description" width="{}" height="{}" viewBox="0 0 {} {}"><title id="title">Herdr Quota dashboard</title><desc id="description">Provider marks: starburst Claude; hexagonal knot OpenAI Codex; faceted square Cursor; half moon Kimi; diagonal cross Grok by xAI; paired goggles GitHub Copilot. Every mark has a readable provider-name mode and an ASCII fallback.</desc><rect width="100%" height="100%" fill="#191724"/><g fill="#e0def4" font-family="monospace" font-size="14">{rows}</g></svg>"##,
        u32::from(width) * 9 + 16,
        u32::from(height) * 18 + 8,
        u32::from(width) * 9 + 16,
        u32::from(height) * 18 + 8
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum InputAction {
    None,
    Quit,
    SavePreferences,
    AcknowledgeTransition,
}

pub(super) fn handle_key(
    config: &mut DashboardConfig,
    key: KeyCode,
    provider_count: usize,
) -> InputAction {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::history_evidence::{
        HISTORY_SCHEMA_VERSION, HistoryDataHealth, HistoryEvidence, HistoryFact,
        HistoryProviderName, HistoryProviderSnapshot, HistorySnapshot,
    };
    use crate::domain::provider::{
        EffectiveAvailability, EffectivePace, ProviderState, QuotaWindow, Runway, WindowPace,
    };
    use crate::store::settings::DashboardSettings;

    fn trend_history() -> HistoryDocument {
        let provider = HistoryProviderName::new(MarketedProvider::Claude);
        let snapshots = [80.0, 76.0, 72.0, 68.0, 64.0, 46.0]
            .into_iter()
            .enumerate()
            .map(|(index, remaining)| HistorySnapshot {
                captured_at: format!("2026-09-02T12:{:02}:00.000Z", index * 5),
                providers: vec![HistoryProviderSnapshot {
                    provider,
                    data_health: HistoryDataHealth::Current,
                    auth_eligible: true,
                    facts: vec![HistoryFact {
                        scope: "All models".into(),
                        limit: Some("Week".into()),
                        remaining,
                        reset_at: Some("2026-09-08T12:00:00.000Z".into()),
                        pace: None,
                        runway: None,
                    }],
                }],
            })
            .collect();
        HistoryDocument {
            schema_version: HISTORY_SCHEMA_VERSION,
            snapshots,
        }
    }

    fn provider(id: &str, remaining: Option<f64>, status: ProviderStatus) -> ProviderQuota {
        ProviderQuota {
            provider: id.into(),
            account_label: None,
            account_reported: false,
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
        if config.view == DashboardView::Overview {
            return (0..visible_provider_count(report, config).max(1))
                .flat_map(|selected_provider| {
                    render_lines(
                        report,
                        columns,
                        rows,
                        &DashboardConfig {
                            selected_provider,
                            ..config.clone()
                        },
                    )
                })
                .collect();
        }
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
        assert!(first[0].contains("Claude"));
        assert!(later[0].contains("Codex"));
        assert_eq!(first[1], later[1]);
        assert_eq!(first[11], later[11]);
        assert!(first.iter().any(|line| line.starts_with(">✳")));
        assert!(later.iter().any(|line| line.starts_with(">⌬")));
        assert!(later.iter().any(|line| line.starts_with("> Account")));
    }

    #[test]
    fn every_provider_summary_is_reachable() {
        let report = report(
            MarketedProvider::ALL
                .iter()
                .map(|market| provider(market.id(), Some(50.0), ProviderStatus::Fresh))
                .collect(),
        );
        let seen = reachable_lines(&report, 20, 12, &DashboardConfig::default());
        for provider in MarketedProvider::ALL {
            assert!(seen.iter().any(|line| {
                line.contains(compact_provider_name(provider))
                    || (provider == MarketedProvider::Copilot && line.contains("GitHub"))
            }));
        }
    }

    #[test]
    fn includes_required_text_markers_and_valueless_quota_is_unavailable() {
        let report = report(vec![provider("claude", None, ProviderStatus::Fresh)]);
        let output = render_lines(&report, 36, 12, &DashboardConfig::default()).join("\n");
        assert!(output.contains("Herdr Quota"));
        assert!(output.contains("? Limits non-current"));
        assert!(output.contains("Claude"));
        assert!(output.contains("unavailable"));
        assert!(output.contains("j/k · enter details · p · q"));
    }

    #[test]
    fn empty_state_only_advertises_available_controls() {
        let report = report(vec![]);
        let lines = semantic_rows(&report, &DashboardConfig::default(), 36);
        let output = lines
            .iter()
            .map(|row| row.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(output.contains("Claude"));
        assert!(output.contains("Copilot"));
        assert_eq!(output.matches("unsupported").count(), 6);
        assert!(!output.contains("providers hidden"));
        assert!(!output.contains("prefs"));
        assert!(!output.contains("Press p"));
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

    fn live_config(status: Option<DashboardStatus>, view: DashboardView) -> DashboardConfig {
        DashboardConfig {
            interactive: true,
            status,
            retry_minutes: status
                .is_some_and(DashboardStatus::is_failure)
                .then_some(10),
            view,
            ..DashboardConfig::default()
        }
    }

    #[test]
    fn refresh_changes_only_the_existing_activity_slot_at_every_supported_size() {
        let report = report(vec![
            provider("claude", Some(50.0), ProviderStatus::Fresh),
            provider("codex", Some(60.0), ProviderStatus::Fresh),
            provider("cursor", Some(70.0), ProviderStatus::Fresh),
            provider("kimi", Some(80.0), ProviderStatus::Fresh),
        ]);
        for (columns, rows) in [(36, 23), (36, 12), (24, 8), (20, 12), (16, 12)] {
            for view in [DashboardView::Overview, DashboardView::Details] {
                let idle = live_config(None, view);
                let refreshing = live_config(Some(DashboardStatus::Refreshing), view);
                let before = render_dashboard_lines(Some(&report), columns, rows, &idle);
                let during = render_dashboard_lines(Some(&report), columns, rows, &refreshing);

                assert_eq!(before.len(), rows as usize);
                assert!(before.iter().all(|line| width(line) == columns as usize));
                assert_ne!(before[0], during[0]);
                assert_eq!(before[1..], during[1..], "{columns}x{rows}");
                assert!(before[0].starts_with("Herdr Quota"));
                assert!(during[0].starts_with("Herdr Quota"));
                assert!(
                    during[0].contains("refreshing") || during[0].contains('↻'),
                    "{columns}x{rows}"
                );
                assert!(during.last().expect("footer").contains('r'));
            }
        }
    }

    #[test]
    fn refreshed_values_keep_the_fixed_decision_provider_position_and_footer_slots() {
        let first = report(vec![provider("claude", Some(50.0), ProviderStatus::Fresh)]);
        let second = report(vec![provider("claude", Some(40.0), ProviderStatus::Fresh)]);

        for columns in [36, 20] {
            let overview = live_config(None, DashboardView::Overview);
            let first_overview = render_dashboard_lines(Some(&first), columns, 12, &overview);
            let second_overview = render_dashboard_lines(Some(&second), columns, 12, &overview);
            assert!(first_overview[1].starts_with("= Limits on pace"));
            assert!(first_overview.iter().any(|line| line.starts_with(">✳")));
            assert!(second_overview.iter().any(|line| line.starts_with(">✳")));
            assert!(first_overview.iter().any(|line| line.contains("50%")));
            assert!(second_overview.iter().any(|line| line.contains("40%")));

            let details = render_dashboard_lines(
                Some(&second),
                columns,
                12,
                &live_config(None, DashboardView::Details),
            );
            assert!(details[2].starts_with("Rows "));
            assert!(details[3].starts_with("> Claude"));
            assert!(details.last().expect("footer").contains('r'));
        }
    }

    #[test]
    fn first_load_failures_are_actionable_and_last_good_failures_stay_identifiably_old() {
        let cases = [
            (DashboardStatus::Timeout, "Quota check timed out"),
            (DashboardStatus::MissingExecutable, "quota-axi missing"),
            (DashboardStatus::IncompatibleOutput, "Incompatible output"),
            (DashboardStatus::NetworkProcess, "Network/process failed"),
        ];

        for (status, expected) in cases {
            let failed = live_config(Some(status), DashboardView::Overview);
            let first = render_dashboard_lines(None, 36, 12, &failed);
            assert!(first[0].contains("not updated"));
            assert!(first[1].contains(expected));
            assert!(first[1].contains("retry 10m"));
            assert_eq!(first[2].trim_end(), "No quota readings");
            assert_eq!(first[3].trim_end(), "Press r to retry now");
            assert!(first.last().expect("footer").contains('r'));

            let last_report = report(vec![provider("claude", Some(50.0), ProviderStatus::Fresh)]);
            let last = render_dashboard_lines(Some(&last_report), 36, 12, &failed);
            let idle = render_dashboard_lines(
                Some(&last_report),
                36,
                12,
                &live_config(None, DashboardView::Overview),
            );
            assert!(last[0].contains("last "));
            assert!(last[1].contains(expected));
            assert!(last.iter().any(|line| line.starts_with(">✳")));
            assert!(last.iter().any(|line| line.contains("50%")));
            assert_eq!(last[2..], idle[2..]);
            assert!(last.iter().all(|line| !line.contains("No quota readings")));
        }

        let narrow = render_dashboard_lines(
            None,
            20,
            12,
            &live_config(Some(DashboardStatus::Timeout), DashboardView::Overview),
        );
        assert_eq!(narrow[1].trim_end(), "Timeout · retry 10m");
        assert_eq!(narrow[2].trim_end(), "No quota readings");
        assert_eq!(narrow[3].trim_end(), "Press r to retry now");
    }

    #[test]
    fn dynamic_no_color_keeps_every_cell_on_the_terminal_default_palette() {
        let report = report(vec![provider("claude", Some(9.0), ProviderStatus::Fresh)]);
        let config = live_config(Some(DashboardStatus::Refreshing), DashboardView::Overview);
        for (columns, rows) in [(36, 12), (20, 12)] {
            let buffer = render_dashboard_buffer(Some(&report), columns, rows, &config);
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
            assert!(
                lines
                    .iter()
                    .any(|line| line.starts_with(" !") && line.contains("9%"))
            );
            assert!(lines.iter().any(|line| line.contains("unavailable")));
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
                assert!(
                    lines.iter().any(|line| {
                        line.contains("signed out")
                            || line.contains("stale")
                            || line.contains("rate limited")
                            || line.contains("partial data")
                            || line.contains("partial")
                            || line.contains("error")
                            || line.contains(" down")
                            || line.contains(" rate")
                            || line.contains(" out")
                            || line.contains(" old")
                            || line.contains("last")
                            || line.contains("unavailable")
                    }),
                    "{status:?} at {columns}: {lines:?}"
                );
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
                assert!(lines.iter().any(|line| line.contains("signed out")));
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
    fn keychain_unavailable_provider_cannot_drive_decisions_or_history() {
        let mut keychain = provider("claude", Some(40.0), ProviderStatus::Fresh);
        keychain.state.reason = Some("keychain_access_required".into());
        keychain.windows[0].resets_at = Some("2026-09-05T12:00:00Z".into());
        let runway = keychain.effective[0].runway.as_mut().unwrap();
        runway.status = RunwayStatus::ProjectedExhaustion;
        runway.projected_exhausted_at = Some("2026-09-03T12:00:00Z".into());
        runway.projection_confidence = Some(ProjectionConfidence::Established);
        let available = provider("kimi", Some(80.0), ProviderStatus::Fresh);
        let report = report(vec![keychain, available]);

        let overview = DashboardConfig {
            selected_provider: 3,
            ..DashboardConfig::default()
        };
        let lines = render_lines(&report, 36, 12, &overview);
        assert_eq!(lines[1].trim_end(), "? Limits non-current");
        assert!(overview_evidence_rows(&report, &overview, 36).is_empty());

        let details =
            render_lines_with_history(&report, Some(&trend_history()), 36, 12, &detail_config());
        assert!(
            details
                .iter()
                .any(|line| line.contains("Keychain approval"))
        );
        assert!(details.iter().all(|line| !line.contains("18pp")));
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
        assert!(lines[1].starts_with("! out 09/02 17:00"));

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
        assert!(lines[1].starts_with("! out now · reset 09/02 13:00"));

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
        assert!(
            lines
                .iter()
                .any(|line| line.contains("pace tier") && line.contains("80%"))
        );
    }

    #[test]
    fn narrow_titles_and_exhaustion_consequences_elide_whole_tokens() {
        let mut exhausted = provider("claude", Some(0.0), ProviderStatus::Fresh);
        exhausted.windows[0].resets_at = Some("2026-09-02T13:00:00Z".into());
        let exhausted = report(vec![exhausted]);

        for columns in 16..=23 {
            let lines = render_lines(&exhausted, columns, 12, &DashboardConfig::default());
            assert!(
                lines[1].starts_with("! out now"),
                "{columns}: {:?}",
                lines[1]
            );
        }

        let details = DashboardConfig {
            view: DashboardView::Details,
            ..DashboardConfig::default()
        };
        for provider_id in ["claude", "copilot"] {
            let lines = render_lines(
                &report(vec![provider(
                    provider_id,
                    Some(50.0),
                    ProviderStatus::Fresh,
                )]),
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
        assert!(lines.iter().any(|line| line.contains("unavailable")));

        let signed_out = provider("copilot", Some(50.0), ProviderStatus::AuthRequired);
        let lines = render_lines(
            &report(vec![signed_out]),
            16,
            12,
            &DashboardConfig {
                selected_provider: 5,
                ..details.clone()
            },
        );
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
        assert_eq!(lines[3].trim_end(), ">?Claude partial");

        for (mut provider, expected) in [
            (
                provider("copilot", Some(50.0), ProviderStatus::AuthRequired),
                ">?GitHub out",
            ),
            (
                provider("claude", Some(50.0), ProviderStatus::Unavailable),
                ">?Claude down",
            ),
        ] {
            provider.state.auth_status = Some("usable".into());
            let lines = render_lines(
                &report(vec![provider]),
                16,
                12,
                &DashboardConfig {
                    view: DashboardView::Details,
                    selected_provider: if expected.contains("GitHub") { 5 } else { 0 },
                    ..DashboardConfig::default()
                },
            );
            assert_eq!(lines[3].trim_end(), expected);
        }
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
            (no_windows, "? Limits non-current"),
            (unknown_semantics, "? Quota data partial"),
            (partial_semantics, "? Quota data partial"),
            (incomplete_siblings, "? Limits non-current"),
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
    fn marketed_roster_omits_non_marketed_records_without_a_hidden_badge() {
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
        assert!(output.contains("Claude"));
        assert!(output.contains("Codex"));
        assert!(output.contains("unavailable"));
        assert!(output.contains("Grok"));
        assert!(output.contains("unsupported"));
        assert!(!output.contains("unavailable providers hidden"));
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
        let output = semantic_rows(&complete_with_unavailable, &DashboardConfig::default(), 36)
            .into_iter()
            .map(|row| row.text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(output.contains("Claude"));
        assert!(output.contains("unavailable"));
        assert!(!output.contains("unavailable provider"));

        let mut future = provider("future-lab", Some(50.0), ProviderStatus::Fresh);
        future.provider = "future-lab".into();
        let output =
            render_lines(&report(vec![future]), 36, 23, &DashboardConfig::default()).join("\n");
        assert!(!output.contains("future-lab"));
        assert!(!output.contains("provider hidden"));
    }

    #[test]
    fn renderer_uses_semantic_provider_labels_and_synthetic_rows() {
        let mut codex = provider("codex", Some(50.0), ProviderStatus::Fresh);
        codex.label = Some("collector label".into());
        codex.windows[0].id = "weekly".into();
        codex.windows[0].label = "collector weekly label".into();
        let lines = render_lines(
            &report(vec![codex]),
            36,
            23,
            &DashboardConfig {
                selected_provider: 1,
                ..detail_config()
            },
        );
        assert!(lines.iter().any(|line| line.starts_with("> OpenAI Codex")));
        assert!(lines.iter().any(|line| line.starts_with("  Week")));
        assert!(lines.iter().any(|line| line.contains("Code review")));
        assert!(lines.iter().any(|line| line.contains("not reported")));

        let unavailable = report(vec![provider(
            "codex",
            Some(50.0),
            ProviderStatus::Unavailable,
        )]);
        let lines = render_lines(
            &unavailable,
            36,
            23,
            &DashboardConfig {
                selected_provider: 1,
                ..detail_config()
            },
        );
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
        assert!(compact.iter().any(|line| line.contains("· ahead")));
    }

    #[test]
    fn overview_fits_four_provider_summaries_with_fixed_decision_and_footer_at_36x12() {
        let report = report(
            ["claude", "codex", "cursor", "kimi"]
                .into_iter()
                .map(|id| provider(id, Some(50.0), ProviderStatus::Fresh))
                .collect(),
        );
        let config = DashboardConfig::default();
        let lines = render_lines(&report, 36, 12, &config);
        let semantic = semantic_rows(&report, &config, 36)
            .into_iter()
            .map(|row| row.text)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(lines[0].starts_with("Herdr Quota · Claude"));
        assert!(lines[1].starts_with("? Quota data partial"));
        for provider in ["Claude", "Codex", "Cursor", "Kimi", "Grok", "Copilot"] {
            assert!(
                semantic.contains(provider),
                "missing {provider}: {semantic}"
            );
        }
        assert_eq!(semantic.matches("primary tier").count(), 4);
        assert_eq!(semantic.matches("unsupported").count(), 2);
        assert_eq!(lines[11].trim_end(), "j/k · enter details · p · q");
    }

    #[test]
    fn overview_evidence_uses_only_spare_rows_and_excludes_primary_providers() {
        let mut claude = provider("claude", Some(40.0), ProviderStatus::Fresh);
        claude.windows[0].resets_at = Some("2026-09-05T12:00:00Z".into());
        claude.effective[0].runway = Some(Runway {
            status: RunwayStatus::ProjectedExhaustion,
            usable_runway_seconds: Some(86_400.0),
            projected_exhausted_at: Some("2026-09-03T12:00:00Z".into()),
            limiting_window_id: Some("main".into()),
            projection_confidence: Some(ProjectionConfidence::Established),
            unmeasurable_window_ids: vec![],
        });
        let mut kimi = provider("kimi", Some(80.0), ProviderStatus::Fresh);
        kimi.windows[0].resets_at = Some("2026-09-06T12:00:00Z".into());
        let report = report(vec![claude, kimi]);
        let config = DashboardConfig {
            selected_provider: 3,
            ..DashboardConfig::default()
        };

        let lines = render_lines(&report, 36, 12, &config);

        assert!(lines[1].contains("out 09/03 12:00"), "{lines:?}");
        assert!(lines.iter().any(|line| line.contains("80%")), "{lines:?}");
        assert!(lines.iter().any(|line| line.starts_with(">◐")), "{lines:?}");
        assert!(
            lines.iter().any(|line| line.starts_with("> Account")),
            "{lines:?}"
        );
        assert!(
            lines.iter().any(|line| line.contains("reset 09/06")),
            "{lines:?}"
        );
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
    fn first_run_launch_frame_keeps_decision_readiness_roster_and_controls_at_36_and_20() {
        let report = crate::domain::schema::parse_quota_response(include_str!(
            "../../../../test/fixtures/launch.json"
        ))
        .expect("sanitized launch fixture");
        let config = DashboardConfig {
            first_run: true,
            ..DashboardConfig::default()
        };

        for width in [36, 20] {
            let lines = render_lines(&report, width, 12, &config);
            assert!(lines[0].starts_with("Herdr Quota"));
            assert!(lines[1].starts_with("! out "));
            assert!(!lines[1].contains("Claude"));
            assert_eq!(lines[2].trim_end(), "4 ready · 2 sign-in");
            let roster = semantic_rows(&report, &config, width)
                .into_iter()
                .map(|row| row.text)
                .collect::<Vec<_>>()
                .join("\n");
            for provider in ["Claude", "Codex", "Cursor", "Kimi", "Grok", "Copilot"] {
                assert!(
                    roster.contains(provider),
                    "{width}: missing {provider} in {roster:?}"
                );
            }
            assert_eq!(roster.matches("Account 1").count(), 6);
            assert!(lines[11].contains("enter"));
            assert!(lines[11].contains('p'));
            assert!(
                lines
                    .iter()
                    .all(|line| UnicodeWidthStr::width(line.as_str()) == usize::from(width))
            );
            for clipped in ["unavail", "unsupport", "parti", "stal"] {
                assert!(lines.iter().all(|line| !line.trim_end().ends_with(clipped)));
            }
        }
    }

    #[test]
    fn narrow_readiness_rows_keep_whole_finite_state_tokens() {
        let mut report = crate::domain::schema::parse_quota_response(include_str!(
            "../../../../test/fixtures/launch.json"
        ))
        .expect("sanitized launch fixture");
        let grok = report
            .providers
            .iter_mut()
            .find(|provider| provider.provider == "grok")
            .expect("Grok fixture");
        grok.state.status = ProviderStatus::Fresh;
        grok.state.auth_status = Some("usable".into());
        grok.semantics_status = Some(SemanticsStatus::Known);
        let lines = semantic_rows(&report, &DashboardConfig::default(), 20)
            .into_iter()
            .map(|row| row.text)
            .collect::<Vec<_>>();
        assert!(lines.iter().any(|line| line.contains("Grok")));
        assert!(lines.iter().any(|line| line.contains("unavailable")));
        assert!(lines.iter().any(|line| line.contains("Copilot")));
        assert!(lines.iter().any(|line| line.contains("signed out")));
        assert!(lines.iter().all(|line| !line.ends_with("unavailab")));
    }

    #[test]
    fn overview_evidence_excludes_selected_and_decision_providers() {
        let mut claude = provider("claude", Some(40.0), ProviderStatus::Fresh);
        claude.windows[0].resets_at = Some("2026-09-05T12:00:00Z".into());
        claude.effective[0].runway = Some(Runway {
            status: RunwayStatus::ProjectedExhaustion,
            usable_runway_seconds: Some(86_400.0),
            projected_exhausted_at: Some("2026-09-03T12:00:00Z".into()),
            limiting_window_id: Some("main".into()),
            projection_confidence: Some(ProjectionConfidence::Established),
            unmeasurable_window_ids: vec![],
        });
        let mut kimi = provider("kimi", Some(80.0), ProviderStatus::Fresh);
        kimi.windows[0].resets_at = Some("2026-09-06T12:00:00Z".into());
        let report = report(vec![claude, kimi]);
        let config = DashboardConfig {
            selected_provider: 3,
            ..DashboardConfig::default()
        };

        let lines = render_lines(&report, 36, 12, &config);

        assert!(lines[1].contains("out 09/03 12:00"), "{lines:?}");
        assert!(lines.iter().any(|line| line.contains("80%")), "{lines:?}");
        assert!(lines.iter().any(|line| line.starts_with(">◐")), "{lines:?}");
        assert!(
            lines.iter().any(|line| line.starts_with("> Account")),
            "{lines:?}"
        );
        assert!(
            lines.iter().any(|line| line.contains("reset 09/06")),
            "{lines:?}"
        );
    }

    #[test]
    fn overview_uses_exact_partial_and_unknown_states_when_siblings_are_unsafe() {
        let codex = provider("codex", Some(79.0), ProviderStatus::Fresh);
        let mut cursor = provider("cursor", None, ProviderStatus::Fresh);
        cursor.semantics_status = Some(SemanticsStatus::Unknown);
        cursor.effective.clear();
        let report = report(vec![codex, cursor]);
        let lines = semantic_rows(&report, &DashboardConfig::default(), 36)
            .into_iter()
            .map(|row| row.text)
            .collect::<Vec<_>>();

        assert!(
            lines
                .iter()
                .any(|line| line.contains("Code review") && line.contains("unavailable"))
        );
        assert!(lines.iter().any(|line| line.contains("Cursor")));
        assert!(lines.iter().any(|line| line.contains("unavailable")));
        assert!(lines.iter().all(|line| !line.contains("Cursor 0%")));
    }

    #[test]
    fn narrow_overview_keeps_separate_proportional_bars_percentages_and_reset_context() {
        let mut kimi = provider("kimi", Some(50.0), ProviderStatus::Fresh);
        kimi.windows[0].resets_at = Some("2026-09-02T17:00:00Z".into());
        let kimi_report = report(vec![kimi]);

        for columns in 16..=23 {
            let lines = render_lines(
                &kimi_report,
                columns,
                12,
                &DashboardConfig {
                    selected_provider: 3,
                    ..DashboardConfig::default()
                },
            );
            assert!(
                lines.iter().any(|line| line.starts_with(">◐")),
                "{columns}: {lines:?}"
            );
            assert!(
                lines.iter().any(|line| line.contains("50%")),
                "{columns}: {lines:?}"
            );
            assert!(
                lines.iter().any(|line| line.contains("████")),
                "{columns}: {lines:?}"
            );
            assert!(
                lines.iter().any(|line| line.contains("in 17h 0m")),
                "{columns}: {lines:?}"
            );
        }
        let rows = render_lines(
            &kimi_report,
            23,
            12,
            &DashboardConfig {
                selected_provider: 3,
                ..DashboardConfig::default()
            },
        );
        assert!(rows.iter().any(|line| line.contains("in 17h 0m")));

        let partial_report = report(vec![provider("codex", Some(50.0), ProviderStatus::Fresh)]);
        let partial = semantic_rows(&partial_report, &DashboardConfig::default(), 16);
        assert!(partial.iter().any(|line| line.text.contains("Code")));
        assert!(partial.iter().any(|line| line.text.contains("unavailable")));

        let mut unknown = provider("cursor", None, ProviderStatus::Fresh);
        unknown.semantics_status = Some(SemanticsStatus::Unknown);
        unknown.effective.clear();
        let unknown_report = report(vec![unknown]);
        let unknown = semantic_rows(&unknown_report, &DashboardConfig::default(), 16);
        assert!(unknown.iter().any(|line| line.text.contains("Cursor")));
        assert!(unknown.iter().any(|line| line.text.contains("unavailable")));
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

        assert_eq!(
            handle_key(&mut config, KeyCode::Enter, MarketedProvider::ALL.len()),
            InputAction::None
        );
        assert_eq!(config.view, DashboardView::Details);
        let details = render_lines(&report, 20, 12, &config).join("\n");
        for label in ["primary", "tier 2", "tier 3", "tier 4"] {
            assert!(details.contains(label), "missing {label:?} in {details:?}");
        }
        assert_eq!(
            handle_key(&mut config, KeyCode::Esc, MarketedProvider::ALL.len()),
            InputAction::None
        );
        assert_eq!(config.view, DashboardView::Overview);
        assert_eq!(config.selected_provider, 0);
        let overview = render_lines(&report, 20, 12, &config);
        assert!(overview[0].contains("Claude"));
        assert!(overview.iter().any(|line| line.starts_with("> Account")));
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
        let mut config = DashboardConfig {
            view: DashboardView::Details,
            startup_view: settings.startup_view,
            saved_startup_view: settings.startup_view,
            ..DashboardConfig::default()
        };
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
    fn detail_trend_is_plain_bounded_and_elides_before_decision_copy() {
        let report = report(vec![provider("claude", Some(46.0), ProviderStatus::Fresh)]);
        let history = trend_history();
        let mut plain = detail_config();
        plain.color = false;

        let wide = render_lines_with_history(&report, Some(&history), 36, 12, &plain);
        let trend = wide
            .iter()
            .find(|line| line.contains("18pp/5m"))
            .expect("roomy selected-provider detail shows the trend");
        let cells = trend
            .split_whitespace()
            .next()
            .expect("trace cells precede consequence");
        assert!((6..=10).contains(&cells.chars().count()));
        assert!(trend.contains("↓ 18pp/5m"));
        assert!(!trend.contains("\u{1b}["));

        let without_history = render_lines(&report, 36, 12, &plain);
        assert_eq!(wide[1], without_history[1], "decision copy is unchanged");
        let narrow = render_lines_with_history(&report, Some(&history), 20, 12, &plain);
        assert_eq!(narrow, render_lines(&report, 20, 12, &plain));
    }

    #[test]
    fn trend_consequence_keeps_projection_precision_coarse() {
        let trend = HistoryTrend {
            cells: "██████".into(),
            evidence: HistoryEvidence {
                kind: HistoryEvidenceKind::ProjectionEarlier,
                provider: HistoryProviderName::new(MarketedProvider::Claude),
                scope: "All models".into(),
                limit: Some("Week".into()),
                amount: Some(3 * 60 * 60),
            },
            elapsed_seconds: 5 * 60,
        };
        assert_eq!(trend_consequence(&trend), "↘ out 3h sooner");
    }

    #[test]
    fn trend_consequence_treats_projection_transition_sentinels_as_unmeasured() {
        let consequence = |kind| {
            trend_consequence(&HistoryTrend {
                cells: "██████".into(),
                evidence: HistoryEvidence {
                    kind,
                    provider: HistoryProviderName::new(MarketedProvider::Claude),
                    scope: "All models".into(),
                    limit: Some("Week".into()),
                    amount: Some(0),
                },
                elapsed_seconds: 5 * 60,
            })
        };

        assert_eq!(
            consequence(HistoryEvidenceKind::ProjectionEarlier),
            "↘ out sooner"
        );
        assert_eq!(
            consequence(HistoryEvidenceKind::ProjectionLater),
            "↗ out later"
        );
    }

    #[test]
    fn detail_trend_marks_history_gaps_and_suppresses_unsafe_live_state() {
        let report = report(vec![provider("claude", Some(46.0), ProviderStatus::Fresh)]);
        let mut history = trend_history();
        history.snapshots[2].providers[0].data_health = HistoryDataHealth::Unavailable;
        history.snapshots[2].providers[0].auth_eligible = false;
        history.snapshots[2].providers[0].facts.clear();
        history.snapshots[3].providers[0].facts[0].reset_at =
            Some("2026-09-09T12:00:00.000Z".into());
        let lines = render_lines_with_history(&report, Some(&history), 36, 12, &detail_config());
        let trend = lines
            .iter()
            .find(|line| line.contains("18pp/5m"))
            .expect("material evidence remains current across older gaps");
        assert!(trend.contains("··"));

        let mut partial = report.clone();
        partial.providers[0].semantics_status = Some(SemanticsStatus::Partial);
        assert!(
            !render_lines_with_history(&partial, Some(&history), 36, 12, &detail_config())
                .iter()
                .any(|line| line.contains("18pp"))
        );

        let mut stale = report;
        stale.providers[0].state.status = ProviderStatus::Stale;
        stale.providers[0].state.stale = true;
        assert!(
            !render_lines_with_history(&stale, Some(&history), 36, 12, &detail_config())
                .iter()
                .any(|line| line.contains("18pp"))
        );
    }

    #[test]
    fn multi_account_overview_rows_are_individually_navigable() {
        let report = crate::domain::schema::parse_quota_response(include_str!(
            "../../../../test/fixtures/multi-account.json"
        ))
        .expect("multi-account fixture must adapt");
        let mut config = DashboardConfig::default();
        let count = visible_provider_count(&report, &config);

        assert_eq!(count, 8, "three Claude accounts plus five provider rows");
        for width in [20, 24, 36] {
            for (selected_provider, account) in
                [(0, "Account 1"), (1, "Account 2"), (2, "Account 3")]
            {
                let overview = render_lines(
                    &report,
                    width,
                    12,
                    &DashboardConfig {
                        selected_provider,
                        ..config.clone()
                    },
                )
                .join("\n");
                assert!(overview.contains(account), "{width}: {overview}");
                assert!(overview.contains("> Account"), "{width}: {overview}");
            }
        }

        assert_eq!(
            handle_key(&mut config, KeyCode::Down, count),
            InputAction::None
        );
        assert_eq!(config.selected_provider, 1);
        assert_eq!(
            handle_key(&mut config, KeyCode::Enter, count),
            InputAction::None
        );
        assert_eq!(config.view, DashboardView::Details);
        let detail = render_lines(&report, 36, 12, &config).join("\n");
        assert!(detail.contains("Account 2 · Research Team"), "{detail}");
        assert!(detail.contains("Week") && detail.contains("8%"), "{detail}");
        assert!(
            detail.contains("Opus week") && detail.contains("55%"),
            "{detail}"
        );
        assert!(detail.contains("reset 09/07 09:15"), "{detail}");
        assert!(!detail.contains("Account 1"));
        assert!(!detail.contains("Account 3"));
    }

    #[test]
    fn account_details_keep_each_reset_explicit_and_private_at_narrow_sizes() {
        let report = crate::domain::schema::parse_quota_response(include_str!(
            "../../../../test/fixtures/multi-account.json"
        ))
        .expect("multi-account fixture must adapt");
        for width in [20, 24, 36] {
            let first = DashboardConfig {
                view: DashboardView::Details,
                selected_provider: 0,
                ..DashboardConfig::default()
            };
            let first = render_lines(&report, width, 12, &first).join("\n");
            assert!(first.contains("Account 1"), "{width}: {first}");
            if width == 36 {
                assert!(first.contains("p•••@example.com"), "{width}: {first}");
            } else {
                assert!(first.contains('…'), "{width}: {first}");
            }
            assert!(first.contains("reset unavailable"), "{width}: {first}");

            let third = DashboardConfig {
                view: DashboardView::Details,
                selected_provider: 2,
                ..DashboardConfig::default()
            };
            let third = render_lines(&report, width, 12, &third).join("\n");
            assert!(third.contains("Account 3"), "{width}: {third}");
            assert!(third.contains("reset unavailable"), "{width}: {third}");
            for secret in [
                "opaque-account-secret",
                "sk-secret",
                "/Users/private",
                "credential",
                "refreshToken",
                "token-secret",
                "primary.user@example.com",
            ] {
                assert!(!first.contains(secret), "{width}: leaked {secret}");
                assert!(!third.contains(secret), "{width}: leaked {secret}");
            }
            assert!(
                first
                    .lines()
                    .all(|line| UnicodeWidthStr::width(line) <= width as usize)
            );
            assert!(
                third
                    .lines()
                    .all(|line| UnicodeWidthStr::width(line) <= width as usize)
            );
        }

        let mut anonymous_single = report.clone();
        anonymous_single
            .providers
            .retain(|provider| provider.provider != "claude");
        anonymous_single
            .providers
            .insert(0, report.providers[2].clone());
        let detail = render_lines(
            &anonymous_single,
            24,
            12,
            &DashboardConfig {
                view: DashboardView::Details,
                ..DashboardConfig::default()
            },
        )
        .join("\n");
        assert!(detail.contains("Account 1"), "{detail}");
    }

    #[test]
    fn exhaustive_overview_preserves_every_safe_account_window_percent_and_reset() {
        let report = crate::domain::schema::parse_quota_response(include_str!(
            "../../../../test/fixtures/multi-account.json"
        ))
        .expect("multi-account fixture must adapt");

        for width in [20, 24, 36] {
            let rows = semantic_rows(&report, &DashboardConfig::default(), width);
            let text = rows
                .iter()
                .map(|row| row.text.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            for account in ["Account 1", "Account 2", "Account 3"] {
                assert!(text.contains(account), "{width}: {text}");
            }
            for (label, value) in [
                ("Session", "72%"),
                ("Week", "41%"),
                ("Week", "8%"),
                ("Opus", "55%"),
                ("Session", "64%"),
                ("Included", "67%"),
            ] {
                assert!(text.contains(label), "{width}: missing {label}");
                assert!(text.contains(value), "{width}: missing {value}");
            }
            assert_eq!(
                text.matches("reset unavailable").count(),
                2,
                "{width}: {text}"
            );
            assert!(text.contains("in 4h 30m"), "{width}: {text}");
            assert!(
                rows.iter()
                    .all(|row| UnicodeWidthStr::width(row.text.as_str()) <= width as usize)
            );
            for secret in [
                "opaque-account-secret",
                "sk-secret",
                "/Users/private",
                "credentialPath",
                "refreshToken",
                "primary.user@example.com",
            ] {
                assert!(!text.contains(secret), "{width}: leaked {secret}");
            }
        }
    }

    #[test]
    fn provider_identity_modes_are_exact_and_ascii_fallback_is_explicit() {
        let report = report(vec![provider("claude", Some(50.0), ProviderStatus::Fresh)]);
        for (mode, expected, absent) in [
            (ProviderIdentityMode::LogoOnly, ">✳", "Claude"),
            (ProviderIdentityMode::LogoAndName, ">✳  Claude", "never"),
            (ProviderIdentityMode::NameOnly, ">Claude", "✳"),
        ] {
            let rows = semantic_rows(
                &report,
                &DashboardConfig {
                    provider_identity: mode,
                    ..DashboardConfig::default()
                },
                36,
            );
            let heading = rows.first().expect("provider heading").text.as_str();
            assert!(heading.starts_with(expected), "{mode:?}: {heading:?}");
            assert!(!heading.contains(absent), "{mode:?}: {heading:?}");
        }

        let rows = semantic_rows(
            &report,
            &DashboardConfig {
                provider_identity: ProviderIdentityMode::LogoOnly,
                logo_glyphs: false,
                ..DashboardConfig::default()
            },
            36,
        );
        assert_eq!(rows.first().expect("fallback heading").text, ">CL");
    }

    #[test]
    fn numeric_percent_is_fractional_clamped_and_never_turns_unknown_into_zero() {
        assert_eq!(percent(Some(0.0)), "0%");
        assert_eq!(percent(Some(100.0)), "100%");
        assert_eq!(percent(Some(24.999)), "24.99%");
        assert_eq!(percent(Some(-1.0)), "0%");
        assert_eq!(percent(Some(101.0)), "100%");
        assert_eq!(percent(None), "unavailable");
        assert_eq!(percent(Some(f64::NAN)), "unavailable");
    }

    #[test]
    fn svg_escapes_terminal_text() {
        let svg = preview_svg(&["<safe & bounded>".into()], 20, 1);
        assert!(svg.contains("&lt;safe &amp; bounded&gt;"));
        assert!(svg.contains(r#"xml:space="preserve">&lt;safe &amp; bounded&gt;"#));
        assert!(svg.contains("aria-labelledby=\"title description\""));
        assert!(svg.contains("starburst Claude"));
    }
}
