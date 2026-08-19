import type {
  EffectiveAvailability,
  ProviderQuota,
  QuotaWindow,
} from "./types.js";

export function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

export function formatPercent(value?: number): string {
  return value === undefined ? "--" : `${Math.round(value)}%`;
}

export function formatDuration(seconds: number): string {
  const safe = Math.max(0, Math.round(seconds));
  if (safe < 60) return `${safe}s`;
  const minutes = Math.floor(safe / 60);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ${minutes % 60}m`;
  const days = Math.floor(hours / 24);
  return `${days}d ${hours % 24}h`;
}

export function relativeTime(iso?: string, now = new Date()): string {
  if (!iso) return "reset unknown";
  const millis = Date.parse(iso) - now.getTime();
  if (!Number.isFinite(millis)) return "reset unknown";
  if (millis <= 0) return `reset ${formatDuration(-millis / 1000)} ago`;
  return `resets in ${formatDuration(millis / 1000)}`;
}

export function ageText(iso?: string, now = new Date()): string {
  if (!iso) return "age unknown";
  const seconds = Math.max(0, (now.getTime() - Date.parse(iso)) / 1000);
  if (!Number.isFinite(seconds)) return "age unknown";
  return seconds < 10 ? "just now" : `${formatDuration(seconds)} ago`;
}

export function displayName(provider: ProviderQuota): string {
  const known: Record<string, string> = {
    claude: "Claude",
    codex: "OpenAI Codex",
    cursor: "Cursor",
    kimi: "Kimi",
  };
  return (
    provider.label ??
    known[provider.provider.toLowerCase()] ??
    titleCase(provider.provider)
  );
}

function titleCase(value: string): string {
  return value
    .replaceAll(/[-_]+/g, " ")
    .replaceAll(/\b\w/g, (letter) => letter.toUpperCase());
}

export function primaryEffective(
  provider: ProviderQuota,
): EffectiveAvailability | undefined {
  return (
    provider.effective.find(
      (item) => item.scope === "all_models" || item.scope === "all_products",
    ) ??
    provider.effective.find((item) => item.status === "known") ??
    provider.effective[0]
  );
}

export function limitingWindow(
  provider: ProviderQuota,
): QuotaWindow | undefined {
  const effective = primaryEffective(provider);
  const id =
    effective?.runway?.limitingWindowId ?? effective?.limitingWindowIds?.[0];
  if (id) return provider.windows.find((window) => window.id === id);
  return provider.windows
    .filter((window) => window.percentRemaining !== undefined)
    .toSorted(
      (a, b) => (a.percentRemaining ?? 101) - (b.percentRemaining ?? 101),
    )[0];
}

export function effectivePercent(provider: ProviderQuota): number | undefined {
  const effective = primaryEffective(provider)?.effectivePercentRemaining;
  if (effective !== undefined) return effective;
  return limitingWindow(provider)?.percentRemaining;
}

export function paceSummary(provider: ProviderQuota): string {
  const effective = primaryEffective(provider);
  const runway = effective?.runway;
  if (runway?.status === "exhausted_now") return "exhausted now";
  if (runway?.status === "projected_exhaustion") {
    return runway.usableRunwaySeconds === undefined
      ? "may run out before reset"
      : `may run out in ${formatDuration(runway.usableRunwaySeconds)}`;
  }
  if (runway?.status === "through_reset") return "pace lasts to reset";
  const pace = effective?.pace?.status;
  if (pace === "ahead" || pace === "mixed") return "pace at risk";
  if (pace === "on_pace" || pace === "behind") return "pace lasts to reset";
  return "pace unknown";
}

export function health(provider: ProviderQuota): {
  label: string;
  tone: "good" | "warn" | "bad" | "muted";
} {
  if (
    provider.state.status === "auth_required" ||
    provider.state.authStatus === "unusable"
  ) {
    return { label: "AUTH REQUIRED", tone: "bad" };
  }
  if (provider.state.stale || provider.state.status === "stale") {
    return { label: "STALE", tone: "warn" };
  }
  if (provider.state.status === "rate_limited")
    return { label: "RATE LIMITED", tone: "warn" };
  if (provider.state.status === "unavailable")
    return { label: "UNAVAILABLE", tone: "muted" };
  if (provider.state.status === "error") return { label: "ERROR", tone: "bad" };
  const hasReading =
    effectivePercent(provider) !== undefined ||
    provider.windows.some(
      (window) =>
        window.percentRemaining !== undefined ||
        window.spentUsd !== undefined ||
        window.limitUsd !== undefined,
    ) ||
    provider.credits?.remaining !== undefined ||
    provider.credits?.unlimited === true;
  if (!hasReading) return { label: "UNAVAILABLE", tone: "muted" };
  return { label: "HEALTHY", tone: "good" };
}

export function formatCredits(provider: ProviderQuota): string | undefined {
  const credits = provider.credits;
  if (credits?.unlimited) return "credits unlimited";
  if (credits?.remaining === undefined) return undefined;
  return credits.unit === "usd"
    ? `$${credits.remaining.toFixed(2)} credits`
    : `${credits.remaining.toLocaleString("en-US")} credits`;
}

export function spendText(window: QuotaWindow): string | undefined {
  if (window.spentUsd === undefined && window.limitUsd === undefined)
    return undefined;
  const spent =
    window.spentUsd === undefined ? "--" : `$${window.spentUsd.toFixed(2)}`;
  const limit =
    window.limitUsd === undefined ? "--" : `$${window.limitUsd.toFixed(2)}`;
  return `${spent} / ${limit}`;
}
