import { pad, stripAnsi, truncate, visibleLength } from "./ansi.js";
import {
  ageText,
  displayName,
  effectivePercent,
  formatPercent,
  health,
  limitingWindow,
  paceSummary,
  relativeTime,
} from "./format.js";
import { providerLogo } from "./logos.js";
import type { DashboardState, ProviderQuota } from "./types.js";

const RESET = "\x1b[0m";
const COLORS = {
  cyan: "\x1b[38;5;81m",
  green: "\x1b[38;5;78m",
  yellow: "\x1b[38;5;220m",
  red: "\x1b[38;5;203m",
  orange: "\x1b[38;5;215m",
  purple: "\x1b[38;5;141m",
  blue: "\x1b[38;5;75m",
  white: "\x1b[38;5;255m",
  dim: "\x1b[38;5;244m",
  bold: "\x1b[1m",
};

export interface RenderOptions {
  width: number;
  height: number;
  color?: boolean;
  now?: Date;
}

function colorize(
  value: string,
  color: keyof typeof COLORS,
  enabled: boolean,
): string {
  return enabled ? `${COLORS[color]}${value}${RESET}` : value;
}

function providerColor(id: string): keyof typeof COLORS {
  const colors: Record<string, keyof typeof COLORS> = {
    claude: "orange",
    codex: "cyan",
    cursor: "blue",
    kimi: "purple",
  };
  return colors[id.toLowerCase()] ?? "white";
}

function healthStyle(provider: ProviderQuota): {
  label: string;
  tone: keyof typeof COLORS;
} {
  const state = health(provider);
  const labels: Record<string, string> = {
    HEALTHY: "OK",
    "AUTH REQUIRED": "AUTH",
    "RATE LIMITED": "LIMITED",
  };
  return {
    label: labels[state.label] ?? state.label,
    tone:
      state.tone === "good"
        ? "green"
        : state.tone === "warn"
          ? "yellow"
          : state.tone === "bad"
            ? "red"
            : "dim",
  };
}

function conclusion(provider: ProviderQuota, width: number): string {
  const state = health(provider).label;
  if (state === "AUTH REQUIRED") return "sign-in required";
  if (state === "RATE LIMITED") return "rate limited";
  if (state === "UNAVAILABLE") return "quota unknown";
  if (state === "ERROR") return "provider error";
  const pace = paceSummary(provider);
  if (pace.length <= width) return pace;
  const runway = pace.match(/^may run out in (\S+)/)?.[1];
  if (runway) return `${width >= 17 ? "may " : ""}run out in ${runway}`;
  return pace;
}

function resetSummary(provider: ProviderQuota, now: Date): string {
  return relativeTime(limitingWindow(provider)?.resetsAt, now).replace(
    /^resets? /,
    "",
  );
}

function providerLines(
  provider: ProviderQuota,
  width: number,
  color: boolean,
  now: Date,
): string[] {
  const mark = providerLogo(provider.provider);
  const markTone = providerColor(provider.provider);
  const state = healthStyle(provider);
  const name = displayName(provider);
  const contentWidth = Math.max(1, width - 4);
  const status = colorize(state.label, state.tone, color);
  const nameWidth = Math.max(1, contentWidth - visibleLength(status) - 1);
  const first = `${colorize(mark[0], markTone, color)} ${truncate(name, nameWidth)} ${status}`;

  const remaining = effectivePercent(provider);
  const remainingText = `${formatPercent(remaining)} left`;
  const remainingTone =
    remaining === undefined
      ? "dim"
      : remaining <= 10
        ? "red"
        : remaining <= 30
          ? "yellow"
          : "green";
  const reset = resetSummary(provider, now);
  const fullDetail = `${remainingText} · ${reset}`;
  const compactDetail = `${formatPercent(remaining)} · ${reset.replace(/^in /, "")}`;
  const detailText =
    fullDetail.length <= contentWidth ? fullDetail : compactDetail;
  const detail = colorize(detailText, remainingTone, color);
  const second = `${colorize(mark[1], markTone, color)} ${truncate(detail, contentWidth)}`;
  const third = `    ${colorize(truncate(conclusion(provider, contentWidth), contentWidth), state.tone, color)}`;
  return [first, second, third].map((line) => pad(line, width));
}

function contentLines(
  state: DashboardState,
  width: number,
  height: number,
  color: boolean,
  now: Date,
): string[] {
  const providers = state.report?.providers ?? [];
  if (!providers.length) {
    const message = state.loading ? "Refreshing quota…" : "No quota readings";
    const hint = state.loading
      ? "This may take a few seconds"
      : "Sign in, then press r";
    return ["", colorize(message, "cyan", color), hint];
  }

  const capacity = Math.max(1, Math.floor((height + 1) / 4));
  const visibleProviders = providers.slice(0, capacity);
  const lines = visibleProviders.flatMap((provider, index) => [
    ...providerLines(provider, width, color, now),
    ...(index < visibleProviders.length - 1 ? [""] : []),
  ]);
  const hidden = providers.length - visibleProviders.length;
  if (hidden > 0 && lines.length < height)
    lines.push(colorize(`+${hidden} more providers`, "dim", color));
  return lines;
}

function titleLine(
  state: DashboardState,
  width: number,
  color: boolean,
  now: Date,
): string {
  const title = colorize(width >= 28 ? "AI quota" : "Quota", "bold", color);
  const activity = state.loading
    ? colorize("refreshing", "cyan", color)
    : state.report
      ? ageText(state.report.generatedAt, now)
      : "not updated";
  return truncate(
    `${title}${" ".repeat(Math.max(1, width - visibleLength(title) - visibleLength(activity)))}${activity}`,
    width,
  );
}

function footerLine(
  state: DashboardState,
  width: number,
  color: boolean,
): string {
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
  const width = Math.max(16, Math.floor(options.width));
  const height = Math.max(6, Math.floor(options.height));
  const color = options.color ?? !process.env.NO_COLOR;
  const now = options.now ?? new Date();
  const available = Math.max(1, height - 2);
  const content = contentLines(state, width, available, color, now).slice(
    0,
    available,
  );
  while (content.length < available) content.push("");
  return [
    titleLine(state, width, color, now),
    ...content.map((line) => truncate(line, width)),
    footerLine(state, width, color),
  ].join("\n");
}

export function renderPlain(
  state: DashboardState,
  options: Omit<RenderOptions, "color">,
): string {
  return stripAnsi(renderDashboard(state, { ...options, color: false }));
}
