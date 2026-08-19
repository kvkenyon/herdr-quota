import { pad, stripAnsi, truncate, visibleLength } from "./ansi.js";
import {
  ageText,
  compactCountdown,
  displayName,
  formatPercent,
} from "./format.js";
import {
  presentProvider,
  providerAnnotation,
  type TierConclusion,
  type TierRow,
} from "./tiers.js";
import type { DashboardState, ProviderQuota } from "./types.js";

const RESET = "\x1b[0m";
const COLORS = {
  cyan: "\x1b[38;5;81m",
  yellow: "\x1b[38;5;220m",
  red: "\x1b[38;5;203m",
  white: "\x1b[38;5;255m",
  dim: "\x1b[38;5;244m",
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
// Shared label column across ordinary sections, so Claude, Codex, and Kimi
// tiers align vertically; only wider labels (Cursor) widen their own section.
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

function resetCell(row: TierRow, now: Date): string {
  if (!row.resetsAt) return "--";
  const seconds = (Date.parse(row.resetsAt) - now.getTime()) / 1000;
  return Number.isFinite(seconds) ? compactCountdown(seconds) : "--";
}

interface SectionColumns {
  labelWidth: number;
  compact: boolean;
  paceWidth: number;
}

function sectionColumns(rows: TierRow[], width: number): SectionColumns {
  const fullWidest = Math.max(...rows.map((row) => row.label.length));
  const compactWidest = Math.max(...rows.map((row) => row.compactLabel.length));
  const withPace =
    width >= ROW_OVERHEAD + SEPARATOR.length + MIN_PACE_WIDTH + 9;
  const budget = withPace
    ? width - ROW_OVERHEAD - SEPARATOR.length - MIN_PACE_WIDTH
    : width - ROW_OVERHEAD;
  const compact = fullWidest > budget;
  const natural = compact ? compactWidest : fullWidest;
  const labelWidth = Math.min(Math.max(natural, PREFERRED_LABEL_WIDTH), budget);
  const paceWidth = withPace
    ? width - ROW_OVERHEAD - SEPARATOR.length - labelWidth
    : 0;
  return { labelWidth, compact, paceWidth };
}

function tierLine(
  row: TierRow,
  columns: SectionColumns,
  layout: Layout,
): string {
  const label = truncate(
    columns.compact ? row.compactLabel : row.label,
    columns.labelWidth,
  );
  const paddedLabel = label.padEnd(columns.labelWidth);
  const styledLabel =
    layout.color && row.limiting
      ? colorize(paddedLabel, "bold", true)
      : paddedLabel;

  const percent = formatPercent(row.percentRemaining).padStart(PERCENT_WIDTH);
  const styledPercent = colorize(
    percent,
    percentTone(row.percentRemaining),
    layout.color,
  );

  if (row.conclusion.kind === "not_reported" && columns.paceWidth > 0) {
    const note = fittingText(
      ["not reported", "--"],
      layout.width - INDENT - columns.labelWidth - 1 - PERCENT_WIDTH - 2,
    );
    return `${" ".repeat(INDENT)}${styledLabel} ${styledPercent}  ${colorize(note, "dim", layout.color)}`;
  }

  const reset = resetCell(row, layout.now).padStart(RESET_WIDTH);
  const styledReset = colorize(reset, "dim", layout.color);
  const base = `${" ".repeat(INDENT)}${styledLabel} ${styledPercent} ${styledReset}`;
  if (columns.paceWidth <= 0) return base;

  const { texts, alert } = conclusionCandidates(row.conclusion, layout.now);
  const pace = fittingText(texts, columns.paceWidth);
  if (!pace) return base;
  const paceTone: Tone = alert
    ? row.percentRemaining !== undefined && row.percentRemaining <= 10
      ? "red"
      : "yellow"
    : "dim";
  return `${base}${colorize(SEPARATOR, "dim", layout.color)}${colorize(pace, paceTone, layout.color)}`;
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

function sectionLines(provider: ProviderQuota, layout: Layout): string[] {
  const lines = [headerLine(provider, layout)];
  const presentation = presentProvider(provider);
  if (presentation.kind === "recovery") {
    lines.push(
      `${" ".repeat(INDENT)}${truncate(presentation.instruction, layout.width - INDENT)}`,
    );
  } else if (presentation.kind === "message") {
    lines.push(
      `${" ".repeat(INDENT)}${colorize(truncate(presentation.message, layout.width - INDENT), "dim", layout.color)}`,
    );
  } else {
    const columns = sectionColumns(presentation.rows, layout.width);
    for (const row of presentation.rows)
      lines.push(tierLine(row, columns, layout));
  }
  return lines;
}

function contentLines(
  state: DashboardState,
  layout: Layout,
  height: number,
): string[] {
  const providers = state.report?.providers ?? [];
  if (!providers.length) {
    const message = state.loading ? "Refreshing quota…" : "No quota readings";
    const hint = state.loading
      ? "This may take a few seconds"
      : "Sign in, then press r";
    return ["", colorize(message, "cyan", layout.color), hint];
  }

  const lines = providers.flatMap((provider, index) => [
    ...(index > 0 ? [""] : []),
    ...sectionLines(provider, layout),
  ]);
  if (lines.length < height) lines.unshift("");
  if (lines.length <= height) return lines;
  const shown = lines.slice(0, height - 1);
  while (shown.length && stripAnsi(shown.at(-1) ?? "").trim() === "")
    shown.pop();
  const hidden = lines.length - shown.length;
  shown.push(colorize(`+${hidden} more rows`, "dim", layout.color));
  return shown;
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

function footerLine(state: DashboardState, layout: Layout): string {
  const { width, color } = layout;
  const text = state.error
    ? width >= 34
      ? `${colorize("refresh failed", "red", color)} · r retry · q close`
      : width >= 20
        ? `${colorize("failed", "red", color)} · r retry · q`
        : `${colorize("failed", "red", color)} · r · q`
    : width >= 23
      ? "r refresh · q/esc close"
      : width >= 19
        ? "r refresh · q close"
        : "r · q/esc close";
  return truncate(text, width);
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
