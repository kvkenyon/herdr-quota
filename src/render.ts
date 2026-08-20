import { pad, stripAnsi, truncate, visibleLength } from "./ansi.js";
import { selectAttention, type Attention } from "./attention.js";
import { BAR_TRACK, remainingBar } from "./bar.js";
import {
  ageText,
  compactCountdown,
  displayName,
  formatPercent,
} from "./format.js";
import {
  presentProvider,
  providerAnnotation,
  type ProviderPresentation,
  type TierConclusion,
  type TierRow,
} from "./tiers.js";
import type {
  CollectorFailureKind,
  DashboardState,
  HistoryEvidence,
  ProviderQuota,
} from "./types.js";

const RESET = "\x1b[0m";
const COLORS = {
  cyan: "\x1b[38;5;81m",
  yellow: "\x1b[38;5;220m",
  red: "\x1b[38;5;203m",
  white: "\x1b[38;5;255m",
  dim: "\x1b[38;5;244m",
  // A healthy gauge is a large solid shape, so it is drawn quieter than the
  // text at the same level of risk and leaves the colour to the tiers that
  // need attention.
  steel: "\x1b[38;5;251m",
  track: "\x1b[38;5;238m",
  bold: "\x1b[1m",
};
type Tone = keyof typeof COLORS;

export interface RenderOptions {
  width: number;
  height: number;
  color?: boolean;
  now?: Date;
}

interface Layout {
  width: number;
  color: boolean;
  now: Date;
}

const INDENT = 1;
const PERCENT_WIDTH = 4;
const RESET_WIDTH = 3;
const SEPARATOR = " · ";
const MIN_PACE_WIDTH = 7;
const MIN_BAR_WIDTH = 4;
const MAX_BAR_WIDTH = 10;
// One label column for the whole sidebar, so every percentage and gauge lines
// up from the first tier to the last.
const PREFERRED_LABEL_WIDTH = 11;
// indent + label gap + percent column + reset gap + reset column
const ROW_OVERHEAD = INDENT + 1 + PERCENT_WIDTH + 1 + RESET_WIDTH;

function colorize(value: string, tone: Tone, enabled: boolean): string {
  return enabled ? `${COLORS[tone]}${value}${RESET}` : value;
}

function annotationTone(tone: "bad" | "warn" | "muted"): Tone {
  return tone === "bad" ? "red" : tone === "warn" ? "yellow" : "dim";
}

function percentTone(percent?: number): Tone {
  if (percent === undefined) return "dim";
  if (percent <= 10) return "red";
  if (percent <= 25) return "yellow";
  return "white";
}

function conclusionCandidates(
  conclusion: TierConclusion,
  now: Date,
): { texts: string[]; alert: boolean } {
  switch (conclusion.kind) {
    case "on_pace":
      return { texts: ["on pace"], alert: false };
    case "ahead": {
      if (conclusion.projectedExhaustedAt) {
        const runway =
          (Date.parse(conclusion.projectedExhaustedAt) - now.getTime()) / 1000;
        if (Number.isFinite(runway) && runway > 0) {
          const countdown = compactCountdown(runway);
          return {
            texts: [`out in ${countdown}`, `out ${countdown}`, "ahead"],
            alert: true,
          };
        }
      }
      return { texts: ["ahead"], alert: true };
    }
    case "spend": {
      const spent = `$${Math.round(conclusion.spentUsd)}`;
      if (conclusion.limitUsd === undefined) {
        return { texts: [`${spent} spent`, "--"], alert: false };
      }
      const limit = `$${Math.round(conclusion.limitUsd)}`;
      return {
        texts: [`${spent} of ${limit}`, `${spent}/${limit}`, "--"],
        alert: false,
      };
    }
    case "not_reported":
      return { texts: ["not reported", "--"], alert: false };
    case "unknown":
      return { texts: ["--"], alert: false };
  }
}

function fittingText(candidates: string[], width: number): string {
  return candidates.find((text) => text.length <= width) ?? "";
}

function unique(values: string[]): string[] {
  return [...new Set(values)];
}

function attentionDetail(
  attention: Extract<Attention, { kind: "constraint" }>,
  now: Date,
): string[] {
  if (attention.constraint === "exhausted") {
    if (attention.resetsAt) {
      const seconds = (Date.parse(attention.resetsAt) - now.getTime()) / 1000;
      if (Number.isFinite(seconds) && seconds > 0) {
        const reset = compactCountdown(seconds);
        return [`spent · resets ${reset}`, `reset ${reset}`, "spent"];
      }
    }
    return ["spent"];
  }
  if (attention.constraint === "projected") {
    if (
      attention.projectionConfidence === "established" &&
      attention.projectedExhaustedAt
    ) {
      const seconds =
        (Date.parse(attention.projectedExhaustedAt) - now.getTime()) / 1000;
      if (Number.isFinite(seconds) && seconds > 0)
        return [`out ${compactCountdown(seconds)}`];
    }
    return ["ahead"];
  }
  return attention.percentRemaining === undefined
    ? ["low"]
    : [
        `${Math.round(attention.percentRemaining)}% left`,
        `${Math.round(attention.percentRemaining)}%`,
      ];
}

function attentionText(attention: Attention, width: number, now: Date): string {
  if (attention.kind === "healthy") {
    return fittingText(
      [
        "= All known limits on pace",
        "= Known limits on pace",
        "= Limits on pace",
      ],
      width,
    );
  }
  if (attention.kind === "data_health") {
    if (attention.reason === "pace_unknown") {
      return fittingText(
        [
          `? Pace unavailable · ${attention.tracked} tracked`,
          "? Pace unavailable",
          "? Pace unknown",
        ],
        width,
      );
    }
    const noun = attention.unreadable === 1 ? "provider" : "providers";
    return fittingText(
      [
        `? ${attention.unreadable} ${noun} unreadable · ${attention.tracked} tracked`,
        `? ${attention.unreadable} unreadable · ${attention.tracked} ok`,
        `? ${attention.unreadable} unreadable`,
      ],
      width,
    );
  }

  const labels = unique([
    ...(attention.compactTier
      ? [`${attention.provider} ${attention.compactTier}`]
      : []),
    ...(attention.tier ? [`${attention.provider} ${attention.tier}`] : []),
    attention.provider,
  ]);
  const details = attentionDetail(attention, now);
  const candidates = labels.flatMap((label) =>
    details.flatMap((detail) => [
      `! ${label} · ${detail}`,
      `! ${label} ${detail}`,
    ]),
  );
  return (
    fittingText(candidates, width) || truncate(`! ${attention.provider}`, width)
  );
}

function attentionTone(attention: Attention): Tone {
  if (attention.kind === "healthy") return "cyan";
  if (attention.kind === "data_health") return "yellow";
  return attention.severity === "critical" ? "red" : "yellow";
}

function attentionLine(
  state: DashboardState,
  layout: Layout,
): string | undefined {
  if (!state.report?.providers.length) return undefined;
  const attention = selectAttention(state.report);
  return colorize(
    attentionText(attention, layout.width, layout.now),
    attentionTone(attention),
    layout.color,
  );
}

function historySubject(evidence: HistoryEvidence): string {
  const detail =
    evidence.limit ??
    (evidence.scope === "All models" || evidence.scope === "All products"
      ? undefined
      : evidence.scope);
  return detail ? `${evidence.provider} ${detail}` : evidence.provider;
}

function historySparkline(values: number[]): string {
  const cells = "▁▂▃▄▅▆▇█";
  const minimum = Math.min(...values);
  const maximum = Math.max(...values);
  if (minimum === maximum) return values.map(() => "▄").join("");
  return values
    .map(
      (value) =>
        cells[
          Math.min(
            7,
            Math.max(
              0,
              Math.round(((value - minimum) / (maximum - minimum)) * 7),
            ),
          )
        ],
    )
    .join("");
}

function historyEvidenceCandidates(evidence: HistoryEvidence): string[] {
  const subject = historySubject(evidence);
  const provider = evidence.provider;
  switch (evidence.kind) {
    case "reset":
      return [
        `↻ ${subject} · reset`,
        `↻ ${subject} reset`,
        `↻ ${provider} reset`,
      ];
    case "remaining_drop": {
      const amount = `${Math.round(evidence.amount ?? 0)}pp`;
      return [
        `↓ ${subject} · ${amount} drop`,
        `↓ ${subject} ${amount}`,
        `↓ ${provider} ${amount}`,
      ];
    }
    case "pace_worse":
      return [
        `↓ ${subject} · pace worse`,
        `↓ ${subject} pace`,
        `↓ ${provider} pace`,
      ];
    case "pace_better":
      return [
        `↑ ${subject} · pace better`,
        `↑ ${subject} pace`,
        `↑ ${provider} pace`,
      ];
    case "projection_earlier": {
      const movement = evidence.amount
        ? `${compactCountdown(evidence.amount)} sooner`
        : "runway worse";
      return [
        `↘ ${subject} · out ${movement}`,
        `↘ ${subject} ${movement}`,
        `↘ ${provider} ${movement}`,
      ];
    }
    case "projection_later": {
      const movement = evidence.amount
        ? `${compactCountdown(evidence.amount)} later`
        : "runway better";
      return [
        `↗ ${subject} · out ${movement}`,
        `↗ ${subject} ${movement}`,
        `↗ ${provider} ${movement}`,
      ];
    }
    case "series": {
      const values = evidence.remainingSeries ?? [];
      const spark = historySparkline(values);
      const first = values[0];
      const current = values.at(-1);
      const movement = `${first ?? "--"}→${current ?? "--"}%`;
      return [
        `~ ${subject} ${spark} · ${movement}`,
        `~ ${subject} ${movement}`,
        `~ ${provider} ${movement}`,
      ];
    }
  }
}

function historyTone(evidence: HistoryEvidence): Tone {
  return evidence.kind === "pace_better" ||
    evidence.kind === "projection_later" ||
    evidence.kind === "reset"
    ? "cyan"
    : evidence.kind === "series"
      ? "dim"
      : "yellow";
}

function historyLine(
  state: DashboardState,
  layout: Layout,
): string | undefined {
  const history = state.history;
  if (!history || history.availability === "no_usable_data") return undefined;
  if (history.evidence) {
    const text =
      fittingText(historyEvidenceCandidates(history.evidence), layout.width) ||
      truncate(`~ ${history.evidence.provider} history`, layout.width);
    return colorize(text, historyTone(history.evidence), layout.color);
  }
  const candidates =
    history.availability === "first_run"
      ? ["~ History starts with this sample", "~ History starts now"]
      : history.availability === "recovered"
        ? ["~ History restarted safely", "~ History restarted"]
        : history.availability === "clock_skew"
          ? ["~ History restarted after clock change", "~ History restarted"]
          : history.availability === "incompatible" ||
              history.availability === "unavailable"
            ? ["~ Local history unavailable", "~ History unavailable"]
            : ["~ History needs another sample", "~ History needs sample"];
  const text =
    fittingText(candidates, layout.width) ||
    truncate("~ History unavailable", layout.width);
  const tone =
    history.availability === "incompatible" ||
    history.availability === "unavailable"
      ? "yellow"
      : "dim";
  return colorize(text, tone, layout.color);
}

function showsHistory(state: DashboardState, available: number): boolean {
  return (
    available >= 8 &&
    !!state.history &&
    !state.failure &&
    state.history.availability !== "no_usable_data"
  );
}

function resetCell(row: TierRow, now: Date): string | undefined {
  if (!row.resetsAt) return undefined;
  const seconds = (Date.parse(row.resetsAt) - now.getTime()) / 1000;
  return Number.isFinite(seconds) ? compactCountdown(seconds) : undefined;
}

interface TierColumns {
  labelWidth: number;
  barWidth: number;
  paceWidth: number;
}

/** The full label when it fits, otherwise the provider's shorter form. */
function labelFor(row: TierRow, labelWidth: number): string {
  return row.label.length <= labelWidth ? row.label : row.compactLabel;
}

/**
 * Solves one set of columns for every tier in the sidebar.
 *
 * Columns are claimed in the order they earn their space: the label, then the
 * pace conclusion that no other column can replace, then the gauge, which
 * restates a percentage that is already on the row. The gauge is reserved
 * before the label is sized, though, because one long label would otherwise
 * eat the few cells the gauges need and drop them from every row.
 */
function tierColumns(rows: TierRow[], width: number): TierColumns {
  const withPace =
    width >= ROW_OVERHEAD + SEPARATOR.length + MIN_PACE_WIDTH + 9;
  const paceReserve = withPace ? SEPARATOR.length + MIN_PACE_WIDTH : 0;
  const barReserve =
    width >=
    ROW_OVERHEAD + PREFERRED_LABEL_WIDTH + MIN_BAR_WIDTH + 1 + paceReserve
      ? MIN_BAR_WIDTH + 1
      : 0;

  const budget = width - ROW_OVERHEAD - paceReserve - barReserve;
  const widest = Math.max(
    ...rows.map((row) => labelFor(row, budget).length),
    PREFERRED_LABEL_WIDTH,
  );
  // Past the preferred column, a long label only earns its width on a pane
  // wide enough to carry a pace conclusion. Narrower panes keep their rows
  // tight instead of stretching a mostly empty column.
  const labelWidth = Math.min(
    withPace ? widest : PREFERRED_LABEL_WIDTH,
    budget,
  );

  const slack = width - ROW_OVERHEAD - labelWidth - paceReserve;
  const barWidth = barReserve ? Math.min(MAX_BAR_WIDTH, slack - 1) : 0;
  const paceWidth = withPace
    ? width -
      ROW_OVERHEAD -
      labelWidth -
      (barWidth ? barWidth + 1 : 0) -
      SEPARATOR.length
    : 0;
  return { labelWidth, barWidth, paceWidth };
}

/**
 * The gauge. Its fill answers to the same risk level as the percentage beside
 * it, so a row never signals two different levels, but a healthy fill is
 * muted: it is a far larger shape than the text and would otherwise shout
 * over the tiers that actually need attention.
 */
function barCell(row: TierRow, columns: TierColumns, layout: Layout): string {
  if (columns.barWidth <= 0) return "";
  const bar = remainingBar(row.percentRemaining, columns.barWidth);
  if (!layout.color || row.percentRemaining === undefined) return `${bar} `;

  const risk = percentTone(row.percentRemaining);
  const tone: Tone = risk === "white" ? "steel" : risk;
  const trackAt = bar.indexOf(BAR_TRACK);
  // An exhausted tier is all track, and that bare track is the loudest thing
  // on the row, so it takes the alert tone instead of receding into grey.
  if (trackAt === 0) return `${colorize(bar, tone, true)} `;
  const fill = trackAt === -1 ? bar : bar.slice(0, trackAt);
  const track = trackAt === -1 ? "" : bar.slice(trackAt);
  return `${colorize(fill, tone, true)}${track ? colorize(track, "track", true) : ""} `;
}

function conclusionTone(row: TierRow, alert: boolean): Tone {
  if (!alert) return "dim";
  return row.percentRemaining !== undefined && row.percentRemaining <= 10
    ? "red"
    : "yellow";
}

function tierLine(row: TierRow, columns: TierColumns, layout: Layout): string {
  const label = truncate(
    labelFor(row, columns.labelWidth),
    columns.labelWidth,
  ).padEnd(columns.labelWidth);
  const styledLabel =
    layout.color && row.limiting ? colorize(label, "bold", true) : label;

  const percent = formatPercent(row.percentRemaining).padStart(PERCENT_WIDTH);
  const styledPercent = colorize(
    percent,
    percentTone(row.percentRemaining),
    layout.color,
  );
  const head = `${" ".repeat(INDENT)}${styledLabel} ${barCell(row, columns, layout)}${styledPercent}`;

  const { texts, alert } = conclusionCandidates(row.conclusion, layout.now);
  const reset = resetCell(row, layout.now);

  // A tier with no reset countdown gives that column back to its conclusion,
  // so a real figure is shown where a placeholder dash would otherwise sit.
  if (reset === undefined) {
    // Nothing is known about this tier beyond the dash already in the
    // percentage, so the row ends rather than repeating it.
    if (row.conclusion.kind === "unknown" && columns.paceWidth > 0) return head;
    const zone =
      RESET_WIDTH +
      (columns.paceWidth > 0 ? SEPARATOR.length + columns.paceWidth : 0);
    const text = fittingText([...texts, "--"], zone);
    if (!text) return head;
    // With no pace column the zone is just the reset column, so what lands
    // there stays right-aligned under the other countdowns.
    const cell = columns.paceWidth > 0 ? text : text.padStart(RESET_WIDTH);
    return `${head} ${colorize(cell, conclusionTone(row, alert), layout.color)}`;
  }

  const base = `${head} ${colorize(reset.padStart(RESET_WIDTH), "dim", layout.color)}`;
  if (columns.paceWidth <= 0) return base;
  const pace = fittingText(texts, columns.paceWidth);
  if (!pace) return base;
  return `${base}${colorize(SEPARATOR, "dim", layout.color)}${colorize(pace, conclusionTone(row, alert), layout.color)}`;
}

function headerLine(provider: ProviderQuota, layout: Layout): string {
  const name = displayName(provider);
  const annotation = providerAnnotation(provider);
  if (!annotation) {
    return colorize(truncate(name, layout.width), "bold", layout.color);
  }
  const status = annotation.text;
  const nameWidth = Math.max(1, layout.width - status.length - 1);
  const shownName = truncate(name, nameWidth);
  const gap = Math.max(1, layout.width - shownName.length - status.length);
  return `${colorize(shownName, "bold", layout.color)}${" ".repeat(gap)}${colorize(status, annotationTone(annotation.tone), layout.color)}`;
}

function sectionLines(
  provider: ProviderQuota,
  presentation: ProviderPresentation,
  columns: TierColumns,
  layout: Layout,
): string[] {
  const lines = [headerLine(provider, layout)];
  if (presentation.kind === "recovery") {
    lines.push(
      `${" ".repeat(INDENT)}${truncate(presentation.instruction, layout.width - INDENT)}`,
    );
  } else if (presentation.kind === "message") {
    lines.push(
      `${" ".repeat(INDENT)}${colorize(truncate(presentation.message, layout.width - INDENT), "dim", layout.color)}`,
    );
  } else {
    for (const row of presentation.rows)
      lines.push(tierLine(row, columns, layout));
  }
  return lines;
}

export interface DashboardScrollMetrics {
  rowCount: number;
  viewportRows: number;
  scroll: number;
  maxScroll: number;
  overflowing: boolean;
}

function detailRowCount(state: DashboardState): number {
  return (state.report?.providers ?? []).reduce((count, provider) => {
    const presentation = presentProvider(provider);
    return (
      count + 1 + (presentation.kind === "tiers" ? presentation.rows.length : 1)
    );
  }, 0);
}

function scrollMetricsForAvailable(
  state: DashboardState,
  available: number,
): DashboardScrollMetrics {
  const rowCount = detailRowCount(state);
  const fixedRows =
    (rowCount > 0 ? 1 : 0) +
    (state.failure ? 1 : 0) +
    (showsHistory(state, available) ? 1 : 0);
  const overflowing = fixedRows + rowCount > available;
  const viewportRows = overflowing
    ? Math.max(0, available - fixedRows - 1)
    : rowCount;
  const maxScroll = overflowing ? Math.max(0, rowCount - viewportRows) : 0;
  const requested = Number.isFinite(state.scroll)
    ? Math.max(0, Math.floor(state.scroll))
    : 0;
  return {
    rowCount,
    viewportRows,
    scroll: Math.min(requested, maxScroll),
    maxScroll,
    overflowing,
  };
}

export function dashboardScrollMetrics(
  state: DashboardState,
  height: number,
): DashboardScrollMetrics {
  const safeHeight = Math.max(6, Math.floor(height));
  return scrollMetricsForAvailable(state, Math.max(1, safeHeight - 2));
}

export function clampDashboardScroll(
  state: DashboardState,
  height: number,
): number {
  return dashboardScrollMetrics(state, height).scroll;
}

type ScrollAction = "scroll_down" | "scroll_up" | "page_down" | "page_up";

export function applyDashboardScroll(
  state: DashboardState,
  action: ScrollAction,
  height: number,
): number {
  const metrics = dashboardScrollMetrics(state, height);
  const page = Math.max(1, metrics.viewportRows);
  const delta =
    action === "scroll_down"
      ? 1
      : action === "scroll_up"
        ? -1
        : action === "page_down"
          ? page
          : -page;
  state.scroll = Math.max(
    0,
    Math.min(metrics.maxScroll, metrics.scroll + delta),
  );
  return state.scroll;
}

function failureLabels(kind: CollectorFailureKind, retry: string): string[] {
  switch (kind) {
    case "timeout":
      return [
        `Quota check timed out · retry ${retry}`,
        `Timeout · retry ${retry}`,
      ];
    case "missing_executable":
      return [`quota-axi missing · retry ${retry}`, `Missing · retry ${retry}`];
    case "incompatible_output":
      return [
        `Incompatible output · retry ${retry}`,
        `Schema · retry ${retry}`,
      ];
    case "network_process":
      return [
        `Network/process failed · retry ${retry}`,
        `Failed · retry ${retry}`,
      ];
  }
}

function failureLine(
  state: DashboardState,
  layout: Layout,
): string | undefined {
  if (!state.failure) return undefined;
  const seconds = state.failure.retryAt
    ? (state.failure.retryAt.getTime() - layout.now.getTime()) / 1000
    : 0;
  const retry = state.failure.retryAt ? compactCountdown(seconds) : "soon";
  const text =
    fittingText(failureLabels(state.failure.kind, retry), layout.width) ||
    truncate("Refresh failed", layout.width);
  return colorize(text, "red", layout.color);
}

function positionLine(metrics: DashboardScrollMetrics, layout: Layout): string {
  const start = metrics.rowCount ? metrics.scroll + 1 : 0;
  const end = Math.min(metrics.rowCount, metrics.scroll + metrics.viewportRows);
  const text = fittingText(
    [
      `Rows ${start}–${end} of ${metrics.rowCount}`,
      `${start}–${end} / ${metrics.rowCount}`,
    ],
    layout.width,
  );
  return colorize(text, "dim", layout.color);
}

function contentLines(
  state: DashboardState,
  layout: Layout,
  height: number,
): string[] {
  const providers = state.report?.providers ?? [];
  if (!providers.length) {
    const failure = failureLine(state, layout);
    const message = state.loading ? "Refreshing quota…" : "No quota readings";
    const hint = state.loading
      ? "This may take a few seconds"
      : state.failure
        ? "Press r to retry now"
        : "Sign in, then press r";
    return [
      ...(failure ? [failure] : [""]),
      colorize(message, state.failure ? "red" : "cyan", layout.color),
      hint,
    ].slice(0, height);
  }

  const sections = providers.map((provider) => ({
    provider,
    presentation: presentProvider(provider),
  }));
  // One column solve for the whole sidebar keeps the gauges and percentages
  // on a single vertical line across every provider.
  const columns = tierColumns(
    sections.flatMap((section) =>
      section.presentation.kind === "tiers" ? section.presentation.rows : [],
    ),
    layout.width,
  );
  const detailSections = sections.map((section) =>
    sectionLines(section.provider, section.presentation, columns, layout),
  );
  const detail = detailSections.flat();
  const attention = attentionLine(state, layout);
  const failure = failureLine(state, layout);
  const history = showsHistory(state, height)
    ? historyLine(state, layout)
    : undefined;
  const fixed = [attention, failure, history].filter(
    (line): line is string => line !== undefined,
  );
  const metrics = scrollMetricsForAvailable(state, height);

  // Decorative provider gaps are admitted only after every real row fits.
  // When space is tight they disappear from the bottom upward and never
  // inflate the hidden-row count.
  if (fixed.length + detail.length <= height) {
    const separatorCount = Math.min(
      Math.max(0, detailSections.length - 1),
      height - fixed.length - detail.length,
    );
    const withSeparators = detailSections.flatMap((lines, index) => [
      ...(index > 0 && index <= separatorCount ? [""] : []),
      ...lines,
    ]);
    return [...fixed, ...withSeparators];
  }

  const shown = detail.slice(
    metrics.scroll,
    metrics.scroll + metrics.viewportRows,
  );
  return [
    ...fixed,
    ...shown,
    ...(metrics.overflowing ? [positionLine(metrics, layout)] : []),
  ].slice(0, height);
}

function titleLine(state: DashboardState, layout: Layout): string {
  const title = colorize(
    layout.width >= 28 ? "AI Quota" : "Quota",
    "bold",
    layout.color,
  );
  const activity = state.loading
    ? colorize("refreshing", "cyan", layout.color)
    : state.report
      ? ageText(state.report.generatedAt, layout.now)
      : "not updated";
  return truncate(
    `${title}${" ".repeat(Math.max(1, layout.width - visibleLength(title) - visibleLength(activity)))}${activity}`,
    layout.width,
  );
}

function footerLine(_state: DashboardState, layout: Layout): string {
  const text =
    layout.width >= 34
      ? "j/k scroll · PgUp/PgDn · r · q/esc"
      : "j/k PgUp/PgDn r/q";
  return truncate(text, layout.width);
}

export function renderDashboard(
  state: DashboardState,
  options: RenderOptions,
): string {
  const layout: Layout = {
    width: Math.max(16, Math.floor(options.width)),
    color: options.color ?? !process.env.NO_COLOR,
    now: options.now ?? new Date(),
  };
  const height = Math.max(6, Math.floor(options.height));
  const available = Math.max(1, height - 2);
  const content = contentLines(state, layout, available).slice(0, available);
  while (content.length < available) content.push("");
  return [
    titleLine(state, layout),
    ...content.map((line) => pad(line, layout.width)),
    footerLine(state, layout),
  ].join("\n");
}

export function renderPlain(
  state: DashboardState,
  options: Omit<RenderOptions, "color">,
): string {
  return stripAnsi(renderDashboard(state, { ...options, color: false }));
}
