import type {
  EffectiveAvailability,
  ProviderQuota,
  QuotaWindow,
} from "./types.js";

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

/** At most three characters, for the aligned tier reset column. */
export function compactCountdown(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds <= 0) return "now";
  if (seconds < 60) return "<1m";
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 48) return `${hours}h`;
  return `${Math.floor(hours / 24)}d`;
}

export function ageText(iso?: string, now = new Date()): string {
  if (!iso) return "age unknown";
  const seconds = Math.max(0, (now.getTime() - Date.parse(iso)) / 1000);
  if (!Number.isFinite(seconds)) return "age unknown";
  return seconds < 10 ? "just now" : `${formatDuration(seconds)} ago`;
}

const MARKETED_NAMES: Record<string, string> = {
  claude: "Claude",
  codex: "OpenAI Codex",
  cursor: "Cursor",
  kimi: "Kimi",
};

export function displayName(provider: ProviderQuota): string {
  return (
    MARKETED_NAMES[provider.provider.toLowerCase()] ??
    provider.label ??
    provider.provider
  );
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

/**
 * Effective aggregate availability. It stays available for emphasis and
 * ordering decisions, but the sidebar never substitutes it for tier rows.
 */
export function effectivePercent(provider: ProviderQuota): number | undefined {
  const effective = primaryEffective(provider)?.effectivePercentRemaining;
  if (effective !== undefined) return effective;
  return limitingWindow(provider)?.percentRemaining;
}
