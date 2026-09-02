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
import { preferenceFocusOrder, settingsEqual } from "./preferences.js";
import {
  defaultSettings,
  isSupportedProvider,
  type DashboardSettings,
  type MeterMode,
  type SupportedProvider,
} from "./settings.js";
import type {
  CollectorFailureKind,
  DashboardState,
  HistoryEvidence,
  PreferencesState,
  ProviderQuota,
  TransitionDisplayEvent,
} from "./types.js";

const RESET = "\x1b[0m";
const COLORS = {
  // Essential evidence inherits the terminal foreground so it remains
  // readable in both Herdr themes. Warnings use weight; critical evidence may
  // add the terminal's semantic red, but its words and markers keep meaning.
  cyan: "",
  yellow: "\x1b[1m",
  red: "\x1b[1;31m",
  white: "",
  dim: "",
  steel: "",
  track: "",
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
  settings: DashboardSettings;
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
  const style = COLORS[tone];
  return enabled && style ? `${style}${value}${RESET}` : value;
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

export function meterPercent(
  percentRemaining: number | undefined,
  mode: MeterMode,
): number | undefined {
  if (percentRemaining === undefined || !Number.isFinite(percentRemaining))
    return undefined;
  const remaining = Math.max(0, Math.min(100, percentRemaining));
  return mode === "used" ? 100 - remaining : remaining;
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
    if (attention.reason === "partial") {
      const noun = attention.partial === 1 ? "provider" : "providers";
      const verb = attention.partial === 1 ? "has" : "have";
      return fittingText(
        [
          `? ${attention.partial} ${noun} ${verb} partial data · ${attention.tracked} tracked`,
          `? ${attention.partial} partial · ${attention.tracked} tracked`,
          `? ${attention.partial} partial`,
        ],
        width,
      );
    }
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

function providerId(provider: ProviderQuota): SupportedProvider | undefined {
  const id = provider.provider.toLowerCase();
  return isSupportedProvider(id) ? id : undefined;
}

function visibleProvidersInSourceOrder(
  state: DashboardState,
  settings: DashboardSettings,
): ProviderQuota[] {
  return (state.report?.providers ?? []).filter((provider) => {
    const id = providerId(provider);
    return id !== undefined && !settings.hiddenProviders.includes(id);
  });
}

function orderedVisibleProviders(
  state: DashboardState,
  settings: DashboardSettings,
): ProviderQuota[] {
  const rank = new Map(
    settings.providerOrder.map((provider, index) => [provider, index]),
  );
  return visibleProvidersInSourceOrder(state, settings).toSorted(
    (left, right) =>
      (rank.get(providerId(left)!) ?? Number.MAX_SAFE_INTEGER) -
      (rank.get(providerId(right)!) ?? Number.MAX_SAFE_INTEGER),
  );
}

function historyProviderId(value: string): SupportedProvider | undefined {
  const normalized = value.toLowerCase();
  if (normalized === "openai codex") return "codex";
  return isSupportedProvider(normalized) ? normalized : undefined;
}

function historyVisible(
  state: DashboardState,
  settings: DashboardSettings,
): boolean {
  const evidence = state.history?.evidence;
  if (!evidence) return true;
  const id = historyProviderId(evidence.provider);
  return id !== undefined && !settings.hiddenProviders.includes(id);
}

function attentionLine(
  state: DashboardState,
  layout: Layout,
): string | undefined {
  const providers = visibleProvidersInSourceOrder(state, layout.settings);
  if (!state.report || !providers.length) return undefined;
  const attention = selectAttention({ ...state.report, providers });
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
    case "remaining_gain": {
      const amount = `${Math.round(evidence.amount ?? 0)}pp`;
      return [
        `↑ ${subject} · ${amount} gain`,
        `↑ ${subject} ${amount}`,
        `↑ ${provider} ${amount}`,
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
  }
}

function historyTone(evidence: HistoryEvidence): Tone {
  return evidence.kind === "pace_better" ||
    evidence.kind === "remaining_gain" ||
    evidence.kind === "projection_later" ||
    evidence.kind === "reset"
    ? "cyan"
    : "yellow";
}

function historyLine(
  state: DashboardState,
  layout: Layout,
): string | undefined {
  const history = state.history;
  if (
    !history ||
    history.availability === "no_usable_data" ||
    !historyVisible(state, layout.settings)
  )
    return undefined;
  if (history.evidence) {
    const text =
      fittingText(historyEvidenceCandidates(history.evidence), layout.width) ||
      truncate(`~ ${history.evidence.provider} history`, layout.width);
    return colorize(text, historyTone(history.evidence), layout.color);
  }
  if (history.availability === "first_run" || history.availability === "ready")
    return undefined;
  const candidates =
    history.availability === "recovered"
      ? ["~ History restarted safely", "~ History restarted"]
      : history.availability === "clock_skew"
        ? ["~ History restarted after clock change", "~ History restarted"]
        : history.availability === "incompatible" ||
            history.availability === "unavailable"
          ? ["~ Local history unavailable", "~ History unavailable"]
          : ["~ History unavailable"];
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

function showsHistory(
  state: DashboardState,
  available: number,
  settings: DashboardSettings,
): boolean {
  return (
    available >= 8 &&
    !!state.history &&
    !state.failure &&
    historyVisible(state, settings) &&
    state.history.availability !== "no_usable_data" &&
    (state.history.evidence !== undefined ||
      (state.history.availability !== "ready" &&
        state.history.availability !== "first_run"))
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
  const displayed = meterPercent(
    row.percentRemaining,
    layout.settings.meterMode,
  );
  const bar = remainingBar(displayed, columns.barWidth);
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

  const percent = formatPercent(
    meterPercent(row.percentRemaining, layout.settings.meterMode),
  ).padStart(PERCENT_WIDTH);
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

function detailRowCount(
  state: DashboardState,
  settings: DashboardSettings,
): number {
  return orderedVisibleProviders(state, settings).reduce((count, provider) => {
    const presentation = presentProvider(provider);
    return (
      count + 1 + (presentation.kind === "tiers" ? presentation.rows.length : 1)
    );
  }, 0);
}

function scrollMetricsForAvailable(
  state: DashboardState,
  available: number,
  settings: DashboardSettings,
): DashboardScrollMetrics {
  const rowCount = detailRowCount(state, settings);
  const hidden = settings.hiddenProviders.length;
  const fixedRows =
    (rowCount > 0 ? 1 : 0) +
    (rowCount > 0 && hidden > 0 ? 1 : 0) +
    (state.failure ? 1 : 0) +
    (showsHistory(state, available, settings) ? 1 : 0);
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
  return scrollMetricsForAvailable(
    state,
    Math.max(1, safeHeight - 2),
    state.settings ?? defaultSettings(),
  );
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

function hiddenLine(settings: DashboardSettings, layout: Layout): string {
  const count = settings.hiddenProviders.length;
  const noun = count === 1 ? "provider" : "providers";
  const text = fittingText(
    [
      `${count} hidden ${noun} · p Preferences`,
      `${count} hidden · p prefs`,
      `${count} hidden · p`,
    ],
    layout.width,
  );
  return colorize(text || `${count} hidden`, "yellow", layout.color);
}

function contentLines(
  state: DashboardState,
  layout: Layout,
  height: number,
): string[] {
  const hidden = layout.settings.hiddenProviders.length;
  if (hidden === layout.settings.providerOrder.length) {
    const failure = failureLine(state, layout);
    return [
      ...(failure ? [failure] : []),
      colorize("No providers shown", "yellow", layout.color),
      fittingText(
        ["Press p for Preferences", "Press p for prefs"],
        layout.width,
      ),
      hiddenLine(layout.settings, layout),
    ].slice(0, height);
  }

  const providers = orderedVisibleProviders(state, layout.settings);
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
      ...(hidden > 0 ? [hiddenLine(layout.settings, layout)] : []),
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
  const history = showsHistory(state, height, layout.settings)
    ? historyLine(state, layout)
    : undefined;
  const hiddenDisclosure =
    hidden > 0 ? hiddenLine(layout.settings, layout) : undefined;
  const fixed = [attention, hiddenDisclosure, failure, history].filter(
    (line): line is string => line !== undefined,
  );
  const metrics = scrollMetricsForAvailable(state, height, layout.settings);

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

const PREFERENCE_PROVIDER_NAMES: Record<SupportedProvider, string> = {
  claude: "Claude",
  codex: "OpenAI Codex",
  cursor: "Cursor",
  kimi: "Kimi",
  grok: "Grok",
  copilot: "GitHub Copilot",
};

function preferenceTitle(
  state: DashboardState,
  preferences: PreferencesState,
  layout: Layout,
): string {
  const focusOrder = preferenceFocusOrder(preferences.draft);
  const focus = Math.max(0, focusOrder.indexOf(preferences.focus)) + 1;
  const dirty = settingsEqual(
    state.settings ?? defaultSettings(),
    preferences.draft,
  )
    ? ""
    : "*";
  const title = layout.width >= 24 ? `Preferences${dirty}` : `Prefs${dirty}`;
  const activity = preferences.saving
    ? "saving"
    : `${focus}/${focusOrder.length}`;
  return truncate(
    `${colorize(title, "bold", layout.color)}${" ".repeat(
      Math.max(1, layout.width - title.length - activity.length),
    )}${activity}`,
    layout.width,
  );
}

function preferenceRows(
  preferences: PreferencesState,
  layout: Layout,
): { focus: PreferencesState["focus"]; line: string }[] {
  const visible = preferences.draft.providerOrder.filter(
    (provider) => !preferences.draft.hiddenProviders.includes(provider),
  );
  const providers = preferences.draft.providerOrder.map((provider) => {
    const selected = preferences.focus === provider;
    const shown = !preferences.draft.hiddenProviders.includes(provider);
    const rank = shown ? String(visible.indexOf(provider) + 1) : "-";
    const text = `${selected ? ">" : " "} ${rank} [${shown ? "x" : " "}] ${PREFERENCE_PROVIDER_NAMES[provider]}`;
    return {
      focus: provider,
      line: colorize(
        truncate(text, layout.width),
        selected ? "bold" : "white",
        layout.color,
      ),
    };
  });
  const action = (
    focus:
      | "meter"
      | "threshold"
      | "forecast"
      | "save"
      | "cancel"
      | "reset"
      | "clear_transitions",
    label: string,
  ) => {
    const selected = preferences.focus === focus;
    const text = `${selected ? ">" : " "} ${label}`;
    return {
      focus,
      line: colorize(
        truncate(text, layout.width),
        selected ? "bold" : "white",
        layout.color,
      ),
    };
  };
  return [
    ...providers,
    action("meter", `Meter: ${preferences.draft.meterMode}`),
    action(
      "threshold",
      `Capacity cue: ${
        preferences.draft.remainingThreshold === "off"
          ? "off"
          : `${preferences.draft.remainingThreshold}%`
      }`,
    ),
    action(
      "forecast",
      `Forecast cue: ${preferences.draft.forecastBeforeReset ? "on" : "off"}`,
    ),
    action("save", "Save"),
    action("cancel", "Cancel"),
    action("reset", "Reset defaults"),
    action("clear_transitions", "Clear transition history"),
  ];
}

function preferenceContentLines(
  preferences: PreferencesState,
  layout: Layout,
  available: number,
): string[] {
  if (preferences.confirmTransitionClear) {
    return [
      colorize("Clear transition history?", "yellow", layout.color),
      "Quota history stays",
      "Provider settings stay",
    ].slice(0, available);
  }
  if (preferences.confirmReset) {
    return [
      colorize("Reset draft?", "yellow", layout.color),
      "Show all providers",
      "Remaining meter · cues off",
      "Save still required",
    ].slice(0, available);
  }

  const rows = preferenceRows(preferences, layout);
  const focus = Math.max(
    0,
    rows.findIndex((row) => row.focus === preferences.focus),
  );
  const start = Math.min(
    Math.max(0, focus - available + 1),
    Math.max(0, rows.length - available),
  );
  return rows.slice(start, start + available).map((row) => row.line);
}

function preferenceFooter(
  preferences: PreferencesState,
  layout: Layout,
): string {
  if (preferences.notice === "save_failed")
    return truncate("Save failed · s retry · esc", layout.width);
  if (preferences.notice === "transition_clear_failed")
    return truncate("Clear failed · esc", layout.width);
  if (preferences.notice === "transition_history_cleared")
    return truncate("Transition history cleared", layout.width);
  if (preferences.saving) return "Saving…";
  if (preferences.confirmTransitionClear)
    return truncate("y clear · n/esc back", layout.width);
  if (preferences.confirmReset)
    return truncate("y reset · n/esc back", layout.width);
  if (isSupportedProvider(preferences.focus)) {
    const hidden = preferences.draft.hiddenProviders.includes(
      preferences.focus,
    );
    return truncate(
      fittingText(
        hidden
          ? ["j/k select · space show", "j/k · space show"]
          : ["j/k · space toggle · u/d reorder", "space · u/d reorder"],
        layout.width,
      ),
      layout.width,
    );
  }
  if (preferences.focus === "meter")
    return truncate("j/k · ←/→ or space", layout.width);
  if (preferences.focus === "threshold" || preferences.focus === "forecast")
    return truncate("j/k · ←/→ or space", layout.width);
  return truncate("j/k · enter · esc cancel", layout.width);
}

function transitionSubject(event: TransitionDisplayEvent): string {
  const provider = event.provider === "OpenAI Codex" ? "Codex" : event.provider;
  const detail =
    event.limit ??
    (event.scope === "All models" || event.scope === "All products"
      ? undefined
      : event.scope);
  return detail ? `${provider} ${detail}` : provider;
}

function transitionText(event: TransitionDisplayEvent): string {
  const subject = transitionSubject(event);
  switch (event.kind) {
    case "threshold_enter":
      return `${subject} crossed ${event.threshold}%${
        event.remaining === undefined
          ? ""
          : ` · ${Math.round(event.remaining)}% left`
      }`;
    case "threshold_recovery":
      return `${subject} recovered above ${event.threshold}%`;
    case "forecast_enter":
      return `${subject} now exhausts before reset`;
    case "forecast_recovery":
      return `${subject} now lasts through reset`;
  }
}

function wrapText(value: string, width: number): string[] {
  const lines: string[] = [];
  let current = "";
  for (const word of value.split(" ")) {
    if (!current) {
      current = word;
    } else if (current.length + word.length + 1 <= width) {
      current += ` ${word}`;
    } else {
      lines.push(truncate(current, width));
      current = word;
    }
  }
  if (current) lines.push(truncate(current, width));
  return lines;
}

function transitionReviewLines(
  state: DashboardState,
  layout: Layout,
  available: number,
): string[] {
  const events = state.transitions?.events ?? [];
  if (!events.length) return ["No new transition events"];
  const lines: string[] = [];
  for (const event of events) {
    if (lines.length && lines.length < available) lines.push("");
    lines.push(...wrapText(transitionText(event), layout.width));
    if (lines.length >= available) break;
  }
  if (lines.length > available && available > 0)
    lines[available - 1] = truncate(
      `+${Math.max(1, events.length - 1)} more`,
      layout.width,
    );
  return lines.slice(0, available);
}

function titleLine(state: DashboardState, layout: Layout): string {
  if (state.preferences)
    return preferenceTitle(state, state.preferences, layout);
  if (state.transitionReview) {
    const count = state.transitions?.events.length ?? 0;
    const title = layout.width >= 24 ? "Quota transition" : "Transition";
    return truncate(
      `${colorize(title, "bold", layout.color)}${" ".repeat(
        Math.max(1, layout.width - title.length - String(count).length),
      )}${count}`,
      layout.width,
    );
  }
  const base = layout.width >= 28 ? "AI Quota" : "Quota";
  const meter = layout.settings.meterMode === "used" ? " · used" : "";
  const marker = (state.transitions?.events.length ?? 0) > 0 ? " !" : "";
  const title = colorize(`${base}${meter}${marker}`, "bold", layout.color);
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

function footerLine(state: DashboardState, layout: Layout): string {
  if (state.preferences) return preferenceFooter(state.preferences, layout);
  if (state.transitionReview) {
    if (state.transitions?.availability === "unavailable")
      return truncate("Ack failed · esc", layout.width);
    return truncate(
      fittingText(
        ["a/enter acknowledge · esc", "a/enter ack · esc", "a ack · esc"],
        layout.width,
      ),
      layout.width,
    );
  }
  if ((state.transitions?.events.length ?? 0) > 0) {
    return truncate(
      fittingText(
        [
          "j/k · Pg · a alert · p prefs · r/q",
          "j/k · a alert · p/r/q",
          "j/k · a alert · r/q",
        ],
        layout.width,
      ),
      layout.width,
    );
  }
  return truncate(
    fittingText(
      [
        "j/k · PgUp/PgDn · p prefs · r · q/esc",
        "j/k Pg · p prefs · r · q/esc",
        "j/k · p prefs · r/q",
      ],
      layout.width,
    ),
    layout.width,
  );
}

export function renderDashboard(
  state: DashboardState,
  options: RenderOptions,
): string {
  const layout: Layout = {
    width: Math.max(16, Math.floor(options.width)),
    color: options.color ?? !process.env.NO_COLOR,
    now: options.now ?? new Date(),
    settings: state.settings ?? defaultSettings(),
  };
  const height = Math.max(6, Math.floor(options.height));
  const available = Math.max(1, height - 2);
  const content = (
    state.transitionReview
      ? transitionReviewLines(state, layout, available)
      : state.preferences
        ? preferenceContentLines(state.preferences, layout, available)
        : contentLines(state, layout, available)
  ).slice(0, available);
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
