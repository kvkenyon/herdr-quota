import { pad, stripAnsi, truncate, visibleLength } from "./ansi.js";
import {
  ageText,
  displayName,
  effectivePercent,
  formatCredits,
  formatPercent,
  health,
  limitingWindow,
  paceSummary,
  primaryEffective,
  relativeTime,
  spendText,
} from "./format.js";
import { providerLogo } from "./logos.js";
import { friendlyProviderError } from "./sanitize.js";
import type { DashboardState, ProviderQuota, QuotaWindow } from "./types.js";

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

function bar(percent: number | undefined, width: number): string {
  if (percent === undefined) return `[${"?".padEnd(width, "-")}]`;
  const filled = Math.round(
    (Math.max(0, Math.min(100, percent)) / 100) * width,
  );
  return `[${"#".repeat(filled)}${"-".repeat(width - filled)}]`;
}

function cardHeader(
  provider: ProviderQuota,
  width: number,
  color: boolean,
  details: readonly string[],
): string[] {
  const logo = providerLogo(provider.provider);
  const logoWidth = Math.max(...logo.map(visibleLength));
  const detailWidth = Math.max(0, width - logoWidth - 2);
  const tone = providerColor(provider.provider);
  const state = health(provider);
  const stateColor =
    state.tone === "good"
      ? "green"
      : state.tone === "warn"
        ? "yellow"
        : state.tone === "bad"
          ? "red"
          : "dim";
  const name = displayName(provider);
  const metadata =
    [provider.plan, provider.source].filter(Boolean).join(" | ") ||
    "local provider";
  const badge = colorize(`[${state.label}]`, stateColor, color);
  const firstDetail =
    pad(
      colorize(
        truncate(name, detailWidth - visibleLength(badge) - 1),
        "bold",
        color,
      ),
      Math.max(0, detailWidth - visibleLength(badge) - 1),
    ) +
    " " +
    badge;
  const content = [firstDetail, metadata, ...details];
  return logo.map(
    (row, index) =>
      `${colorize(row, tone, color)}  ${truncate(content[index] ?? "", detailWidth)}`,
  );
}

function projectionSummary(
  provider: ProviderQuota,
  width: number,
  now: Date,
): string {
  const runway = primaryEffective(provider)?.runway;
  if (runway?.projectedExhaustedAt) {
    const exhaustion = relativeTime(runway.projectedExhaustedAt, now).replace(
      /^resets in /,
      "",
    );
    const confidence = runway.projectionConfidence;
    const full = `exhausts in ${exhaustion}${confidence ? ` | confidence ${confidence}` : ""}`;
    if (full.length <= width) return full;
    const abbreviatedConfidence =
      confidence === "established" ? "est." : confidence;
    return `projection ${exhaustion}${abbreviatedConfidence ? ` | conf ${abbreviatedConfidence}` : ""}`;
  }
  return runway?.projectionConfidence
    ? `projection confidence ${runway.projectionConfidence}`
    : "";
}

function windowLine(window: QuotaWindow, width: number, now: Date): string {
  const name = truncate(
    window.label,
    Math.max(8, Math.min(19, Math.floor(width * 0.3))),
  );
  const percentage = formatPercent(window.percentRemaining);
  const reset = relativeTime(window.resetsAt, now);
  const barWidth = Math.max(
    4,
    Math.min(12, width - name.length - percentage.length - reset.length - 8),
  );
  return truncate(
    `  ${name.padEnd(Math.min(19, Math.floor(width * 0.3)))} ${bar(window.percentRemaining, barWidth)} ${percentage.padStart(4)} | ${reset}`,
    width,
  );
}

function cardLines(
  provider: ProviderQuota,
  width: number,
  color: boolean,
  now: Date,
): string[] {
  const remaining = effectivePercent(provider);
  const limited = limitingWindow(provider);
  const effective = primaryEffective(provider);
  const remainingText =
    remaining === undefined
      ? "-- remaining"
      : `${formatPercent(remaining)} remaining`;
  const pace = paceSummary(provider);
  const limitText = limited
    ? `limited by ${limited.label}`
    : "limiting window unknown";
  const reset = relativeTime(limited?.resetsAt, now);
  const primaryTone =
    remaining === undefined
      ? "dim"
      : remaining <= 10
        ? "red"
        : remaining <= 30
          ? "yellow"
          : "green";
  const logoWidth = Math.max(
    ...providerLogo(provider.provider).map(visibleLength),
  );
  const detailWidth = Math.max(0, width - logoWidth - 2);
  const lines = cardHeader(provider, width, color, [
    `${colorize(remainingText, primaryTone, color)} | ${pace}`,
    `${limitText} | ${reset}`,
    projectionSummary(provider, detailWidth, now),
  ]);

  const credits = formatCredits(provider);
  if (credits) lines.push(truncate(`  ${credits}`, width));

  if (provider.windows.length) {
    lines.push(colorize("  WINDOWS", "dim", color));
    for (const window of provider.windows) {
      lines.push(windowLine(window, width, now));
      const spend = spendText(window);
      if (spend) lines.push(truncate(`    spend ${spend}`, width));
    }
  } else {
    const auth =
      provider.state.status === "auth_required" ||
      provider.state.authStatus === "unusable";
    const message = auth
      ? provider.state.reason === "keychain_access_required"
        ? "Keychain approval needed | run quota-axi --allow-keychain-prompt once"
        : "Sign in with the provider's official CLI"
      : friendlyProviderError(provider.state.errorCode);
    lines.push(colorize(`  ${message}`, auth ? "yellow" : "red", color));
  }

  if (provider.effective.length > 1) {
    const secondary = provider.effective
      .slice(1)
      .map(
        (item) =>
          `${item.scope} ${formatPercent(item.effectivePercentRemaining)}`,
      )
      .join(" | ");
    lines.push(
      colorize(truncate(`  scopes: ${secondary}`, width), "dim", color),
    );
  }
  const bounding = effective?.boundedBy ?? [];
  if (bounding.length)
    lines.push(
      colorize(
        truncate(`  bounds: ${bounding.join(" + ")}`, width),
        "dim",
        color,
      ),
    );
  return lines.map((line) => pad(line, width));
}

function joinColumns(left: string[], right: string[], width: number): string[] {
  const height = Math.max(left.length, right.length);
  return Array.from(
    { length: height },
    (_, index) =>
      `${pad(left[index] ?? "", width)} | ${pad(right[index] ?? "", width)}`,
  );
}

function contentLines(
  state: DashboardState,
  width: number,
  color: boolean,
  now: Date,
): string[] {
  const providers = state.report?.providers ?? [];
  if (!providers.length) {
    if (state.loading)
      return [
        "",
        colorize("  Loading provider quota...", "cyan", color),
        "",
        "  This first refresh can take a few seconds.",
      ];
    return [
      "",
      colorize("  No provider readings available", "yellow", color),
      "",
      "  Sign in with a supported provider CLI, then press r.",
    ];
  }
  if (width >= 92) {
    const gap = 3;
    const columnWidth = Math.floor((width - gap) / 2);
    const rows: string[] = [];
    for (let index = 0; index < providers.length; index += 2) {
      const leftProvider = providers[index];
      if (!leftProvider) continue;
      const left = cardLines(leftProvider, columnWidth, color, now);
      const rightProvider = providers[index + 1];
      const right = rightProvider
        ? cardLines(rightProvider, columnWidth, color, now)
        : [];
      rows.push(...joinColumns(left, right, columnWidth), "");
    }
    return rows;
  }
  const cardWidth = Math.max(24, width);
  return providers.flatMap((provider) => [
    ...cardLines(provider, cardWidth, color, now),
    "",
  ]);
}

function titleLine(
  state: DashboardState,
  width: number,
  color: boolean,
  now: Date,
): string {
  const title = colorize("AI SUBSCRIPTION QUOTA", "bold", color);
  const age = state.report
    ? `updated ${ageText(state.report.generatedAt, now)}`
    : "not updated";
  const activity = state.loading
    ? colorize("refreshing...", "cyan", color)
    : age;
  return (
    pad(title, Math.max(0, width - visibleLength(activity) - 2)) +
    "  " +
    activity
  );
}

export function renderDashboard(
  state: DashboardState,
  options: RenderOptions,
): string {
  const width = Math.max(24, Math.floor(options.width));
  const height = Math.max(8, Math.floor(options.height));
  const color = options.color ?? !process.env.NO_COLOR;
  const now = options.now ?? new Date();
  const header = [
    titleLine(state, width, color, now),
    colorize("-".repeat(width), "dim", color),
  ];
  const footerText = state.error
    ? `${colorize("Refresh failed:", "red", color)} ${truncate(state.error, width - 27)}  |  r retry  q close`
    : "r refresh  |  j/k scroll  |  q/esc close";
  const footer =
    colorize("-".repeat(width), "dim", color) +
    "\n" +
    truncate(footerText, width);
  const available = Math.max(1, height - header.length - 2);
  const content = contentLines(state, width, color, now);
  const maxScroll = Math.max(0, content.length - available);
  const scroll = Math.min(maxScroll, Math.max(0, state.scroll));
  const visible = content.slice(scroll, scroll + available);
  while (visible.length < available) visible.push("");
  return [
    ...header,
    ...visible.map((line) => truncate(line, width)),
    footer,
  ].join("\n");
}

export function renderPlain(
  state: DashboardState,
  options: Omit<RenderOptions, "color">,
): string {
  return stripAnsi(renderDashboard(state, { ...options, color: false }));
}
