//! A bounded Ratatui dashboard renderer.
//!
//! The renderer accepts only adapted schema-v5 data. It deliberately produces
//! whole semantic rows before placing them in a fixed-height frame, so scrolling
//! cannot split a provider heading from the row model or leak collector text.

use std::collections::BTreeSet;
use std::io::{self, Stdout};

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
    provider::{MarketedProvider, ProviderQuota, ProviderStatus},
    schema::QuotaReport,
};

/// Local rendering inputs; collection and quota semantics do not depend on them.
#[derive(Clone, Debug, Default)]
pub struct DashboardConfig {
    /// IDs explicitly hidden by the user. They never inflate availability summary text.
    pub user_hidden: BTreeSet<String>,
    /// The first semantic row visible in the scrollable region.
    pub scroll: usize,
    /// Whether optional Ratatui colour may reinforce the textual markers.
    pub color: bool,
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

fn display_name(provider: &ProviderQuota) -> &str {
    provider
        .label
        .as_deref()
        .filter(|label| !label.is_empty())
        .unwrap_or_else(|| {
            provider_id(provider)
                .map(MarketedProvider::label)
                .unwrap_or("Provider")
        })
}

fn provider_state(provider: &ProviderQuota) -> Option<&'static str> {
    match provider.state.status {
        ProviderStatus::Fresh if !provider.state.stale => None,
        ProviderStatus::Fresh | ProviderStatus::Stale => Some("stale"),
        ProviderStatus::Unavailable => Some("unavailable"),
        ProviderStatus::AuthRequired => Some("sign-in required"),
        ProviderStatus::RateLimited => Some("rate limited"),
        ProviderStatus::Error => Some("quota unavailable"),
    }
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

fn semantic_rows(report: &QuotaReport, config: &DashboardConfig, width: u16) -> Vec<SemanticRow> {
    let report_ids: BTreeSet<_> = report
        .providers
        .iter()
        .filter_map(provider_id)
        .map(MarketedProvider::id)
        .collect();
    let hidden_unavailable = MarketedProvider::ALL
        .iter()
        .filter(|provider| {
            !config.user_hidden.contains(provider.id()) && !report_ids.contains(provider.id())
        })
        .count()
        + report
            .providers
            .iter()
            .filter(|provider| {
                provider_id(provider).is_some_and(|id| !config.user_hidden.contains(id.id()))
                    && provider.state.status == ProviderStatus::Unavailable
            })
            .count();

    let mut rows = Vec::new();
    for provider in report.providers.iter().filter(|provider| {
        provider_id(provider).is_some_and(|id| !config.user_hidden.contains(id.id()))
    }) {
        let state = provider_state(provider);
        let heading = if let Some(state) = state {
            format!("> {} · {}", display_name(provider), state)
        } else {
            format!("> {}", display_name(provider))
        };
        rows.push(SemanticRow {
            text: heading,
            style: if provider.state.status == ProviderStatus::Unavailable {
                RowStyle::Warning
            } else {
                RowStyle::Heading
            },
        });
        if state.is_some() && provider.windows.is_empty() {
            rows.push(SemanticRow {
                text: "  ? Quota unavailable".into(),
                style: RowStyle::Warning,
            });
            continue;
        }
        if provider.windows.is_empty() {
            rows.push(SemanticRow {
                text: "  ? No quota reported".into(),
                style: RowStyle::Warning,
            });
            continue;
        }
        for window in &provider.windows {
            let label_budget = if width >= 30 { 15 } else { 9 };
            let label = truncate(&window.label, label_budget);
            let meter = if width >= 30 {
                format!(" [{}]", gauge(window.percent_remaining, 6))
            } else {
                String::new()
            };
            let reset = window
                .reset_text
                .as_deref()
                .map(|value| format!(" · {value}"))
                .unwrap_or_default();
            let text = format!(
                "  {label:<label_budget$} {}{meter}{reset}",
                percent(window.percent_remaining)
            );
            let critical = window.percent_remaining.is_some_and(|value| value <= 10.0);
            rows.push(SemanticRow {
                text,
                style: if critical {
                    RowStyle::Critical
                } else {
                    RowStyle::Normal
                },
            });
        }
    }
    if hidden_unavailable > 0 {
        let noun = if hidden_unavailable == 1 {
            "provider"
        } else {
            "providers"
        };
        rows.push(SemanticRow {
            text: format!("? {hidden_unavailable} unavailable {noun} hidden"),
            style: RowStyle::Warning,
        });
    }
    rows
}

fn attention(report: &QuotaReport, config: &DashboardConfig) -> (String, RowStyle) {
    let visible: Vec<_> = report
        .providers
        .iter()
        .filter(|provider| {
            provider_id(provider).is_some_and(|id| !config.user_hidden.contains(id.id()))
        })
        .collect();
    if visible.is_empty() {
        return (
            "? No providers shown · Press p for prefs".into(),
            RowStyle::Warning,
        );
    }
    if let Some((provider, remaining)) = visible
        .iter()
        .flat_map(|provider| {
            provider.windows.iter().filter_map(move |window| {
                window
                    .percent_remaining
                    .map(|remaining| (provider, remaining))
            })
        })
        .min_by(|(_, left), (_, right)| left.total_cmp(right))
    {
        if remaining <= 25.0 {
            return (
                format!("! {} · {}% left", display_name(provider), remaining.round()),
                if remaining <= 10.0 {
                    RowStyle::Critical
                } else {
                    RowStyle::Warning
                },
            );
        }
    }
    if visible.iter().any(|provider| {
        provider
            .windows
            .iter()
            .any(|window| window.percent_remaining.is_none())
    }) {
        return ("? Some limits unknown".into(), RowStyle::Warning);
    }
    ("= All known limits on pace".into(), RowStyle::Normal)
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
    let width = width.max(1) as usize;
    let height = height.max(1) as usize;
    let rows = semantic_rows(report, config, width as u16);
    let title = "Herdr Quota";
    let (attention, _) = attention(report, config);
    let controls = if width >= 30 {
        "j/k · PgUp/PgDn · r · q"
    } else {
        "j/k · r · q"
    };
    let body_start = if height >= 5 { 3 } else { 2.min(height) };
    let footer = height.saturating_sub(1);
    let body_end = footer.max(body_start);
    let viewport = body_end.saturating_sub(body_start);
    let scroll = config.scroll.min(rows.len().saturating_sub(viewport));
    let mut output = vec![String::new(); height];
    output[0] = truncate(title, width);
    if height > 1 {
        output[1] = truncate(&attention, width);
    }
    if height > 2 {
        output[2] = truncate(&position(scroll, rows.len(), viewport), width);
    }
    if rows.is_empty() && viewport > 0 {
        output[body_start] = truncate("? No providers shown · Press p for prefs", width);
    }
    for (index, row) in rows.iter().skip(scroll).take(viewport).enumerate() {
        output[body_start + index] = truncate(&row.text, width);
    }
    if height > 1 {
        output[footer] = truncate(controls, width);
    }
    output
        .into_iter()
        .map(|line| pad_cells(&line, width))
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
    lines: &'a [String],
    config: &'a DashboardConfig,
}

impl Widget for Dashboard<'_> {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        for (y, line) in self.lines.iter().take(area.height as usize).enumerate() {
            let style = if y == 0 {
                RowStyle::Heading
            } else if line.starts_with('!') {
                RowStyle::Critical
            } else if line.starts_with('?') {
                RowStyle::Warning
            } else {
                RowStyle::Normal
            };
            buffer.set_stringn(
                area.x,
                area.y + y as u16,
                line,
                area.width as usize,
                style.ratatui(self.config.color),
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
    let lines = render_lines(report, width, height, config);
    let area = Rect::new(0, 0, width.max(1), height.max(1));
    let mut buffer = Buffer::empty(area);
    Dashboard {
        lines: &lines,
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
                r#"<text x="8" y="{}">{}</text>"#,
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

/// Drive the interactive Crossterm dashboard. `j`/`k`, Page keys, `r`, `q`, and Escape are local.
pub fn dashboard(report: &QuotaReport) -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let result = dashboard_loop(&mut stdout, report);
    disable_raw_mode()?;
    execute!(stdout, LeaveAlternateScreen)?;
    result
}

fn dashboard_loop(stdout: &mut Stdout, report: &QuotaReport) -> io::Result<()> {
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let mut config = DashboardConfig {
        color: std::env::var_os("NO_COLOR").is_none(),
        ..DashboardConfig::default()
    };
    loop {
        terminal.draw(|frame| {
            let area = frame.area();
            let lines = render_lines(report, area.width, area.height, &config);
            frame.render_widget(
                Dashboard {
                    lines: &lines,
                    config: &config,
                },
                area,
            );
        })?;
        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                KeyCode::Char('j') | KeyCode::Down => {
                    config.scroll = config.scroll.saturating_add(1)
                }
                KeyCode::Char('k') | KeyCode::Up => config.scroll = config.scroll.saturating_sub(1),
                KeyCode::PageDown => config.scroll = config.scroll.saturating_add(8),
                KeyCode::PageUp => config.scroll = config.scroll.saturating_sub(8),
                KeyCode::Char('r') => {}
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::provider::{ProviderState, QuotaWindow};

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
            effective: vec![],
            semantics_status: None,
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
                scroll: 2,
                ..DashboardConfig::default()
            },
        );
        assert_eq!(first[0], later[0]);
        assert_eq!(first[1], later[1]);
        assert_eq!(first[11], later[11]);
        assert!(first[2].starts_with("Rows "));
        assert!(later[2].starts_with("Rows "));
    }

    #[test]
    fn every_semantic_row_is_reachable() {
        let report = report(
            MarketedProvider::ALL
                .iter()
                .map(|market| provider(market.id(), Some(50.0), ProviderStatus::Fresh))
                .collect(),
        );
        let total = semantic_rows(&report, &DashboardConfig::default(), 20).len();
        let seen: Vec<_> = (0..total)
            .flat_map(|scroll| {
                render_lines(
                    &report,
                    20,
                    12,
                    &DashboardConfig {
                        scroll,
                        ..DashboardConfig::default()
                    },
                )
                .into_iter()
                .skip(3)
                .take(8)
            })
            .collect();
        for row in semantic_rows(&report, &DashboardConfig::default(), 20) {
            assert!(
                seen.iter()
                    .any(|line| line.trim_end() == truncate(&row.text, 20))
            );
        }
    }

    #[test]
    fn includes_required_text_markers_and_unknown_is_not_zero() {
        let report = report(vec![provider("claude", None, ProviderStatus::Fresh)]);
        let output = render_lines(&report, 36, 12, &DashboardConfig::default()).join("\n");
        assert!(output.contains("Herdr Quota"));
        assert!(output.contains("? Some limits unknown"));
        assert!(output.contains(" --"));
        assert!(output.contains("Rows "));
        assert!(output.contains("j/k · PgUp/PgDn · r · q"));
    }

    #[test]
    fn no_color_render_uses_default_foreground_styles() {
        let report = report(vec![provider("claude", Some(9.0), ProviderStatus::Fresh)]);
        for (columns, rows) in [(20, 12), (36, 23)] {
            let buffer = render_buffer(&report, columns, rows, &DashboardConfig::default());
            assert!(buffer.content.iter().all(|cell| cell.fg == Color::Reset));
            assert!(buffer.content.iter().all(|cell| cell.bg == Color::Reset));
        }
    }

    #[test]
    fn hidden_summary_excludes_user_choice_but_counts_missing_and_unavailable() {
        let partial_report = report(vec![
            provider("claude", Some(50.0), ProviderStatus::Fresh),
            provider("codex", Some(50.0), ProviderStatus::Unavailable),
        ]);
        let mut hidden = BTreeSet::new();
        hidden.insert("cursor".into());
        let output = render_lines(
            &partial_report,
            36,
            23,
            &DashboardConfig {
                user_hidden: hidden,
                ..DashboardConfig::default()
            },
        )
        .join("\n");
        assert!(output.contains("4 unavailable providers hidden"));
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
    }

    #[test]
    fn svg_escapes_terminal_text() {
        let svg = preview_svg(&["<safe & bounded>".into()], 20, 1);
        assert!(svg.contains("&lt;safe &amp; bounded&gt;"));
    }
}
