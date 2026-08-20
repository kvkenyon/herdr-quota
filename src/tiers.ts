import type { ProviderQuota, QuotaWindow } from "./types.js";
import { friendlyProviderError } from "./sanitize.js";

/**
 * The marketed provider set. Anything else quota-axi can report (Grok,
 * GitHub Copilot, future providers) is neither queried nor rendered.
 */
export const ALLOWED_PROVIDERS = ["claude", "codex", "cursor", "kimi"] as const;

export function isAllowedProvider(provider: string): boolean {
  return (ALLOWED_PROVIDERS as readonly string[]).includes(
    provider.toLowerCase(),
  );
}

export type TierConclusion =
  | { kind: "on_pace" }
  | { kind: "ahead"; projectedExhaustedAt?: string }
  | { kind: "spend"; spentUsd: number; limitUsd?: number }
  | { kind: "not_reported" }
  | { kind: "unknown" };

export interface TierRow {
  id: string;
  label: string;
  compactLabel: string;
  percentRemaining?: number;
  resetsAt?: string;
  conclusion: TierConclusion;
  limiting: boolean;
}

export type ProviderPresentation =
  | { kind: "tiers"; rows: TierRow[] }
  | { kind: "recovery"; instruction: string }
  | { kind: "message"; message: string };

export interface ProviderAnnotation {
  text: string;
  tone: "bad" | "warn" | "muted";
}

const RECOVERY_INSTRUCTIONS: Record<string, string> = {
  claude: "claude, then /login",
  codex: "codex login",
  cursor: "cursor-agent login",
  kimi: "kimi login",
};

interface TierLabel {
  label: string;
  compact: string;
}

function titleCaseSlug(slug: string): string {
  return slug
    .replaceAll(/[-_]+/g, " ")
    .replaceAll(/\b\w/g, (letter) => letter.toUpperCase());
}

function passthrough(window: QuotaWindow): TierLabel {
  return { label: window.label, compact: window.label };
}

function claudeTierLabel(window: QuotaWindow): TierLabel {
  if (window.id === "five_hour")
    return { label: "Session", compact: "Session" };
  if (window.id === "seven_day") return { label: "Week", compact: "Week" };
  if (window.id === "seven_day_opus")
    return { label: "Opus week", compact: "Opus" };
  if (window.id === "extra_usage")
    return { label: "Extra usage", compact: "Extra" };
  const model = /^model:(.+)$/.exec(window.id)?.[1];
  if (model) {
    const name = titleCaseSlug(model);
    return { label: `${name} week`, compact: name };
  }
  return passthrough(window);
}

function codexModelName(window: QuotaWindow): string {
  const base = window.label.replace(
    /\s+(?:sessions?|weekly|week|5h|5 hours?|7d|7 days?)$/i,
    "",
  );
  const stripped = base.replace(/^GPT-[\d.]+-Codex-?/i, "");
  return stripped || base;
}

function codexTierLabel(window: QuotaWindow): TierLabel {
  if (window.id === "five_hour")
    return { label: "Session", compact: "Session" };
  if (window.id === "weekly") return { label: "Week", compact: "Week" };
  if (window.id.startsWith("code_review_five_hour"))
    return { label: "Review 5h", compact: "Review 5h" };
  if (window.id.startsWith("code_review_weekly"))
    return { label: "Review week", compact: "Review wk" };
  const model = /^model:.+:(5h|7d)(?:_\d+)?$/.exec(window.id);
  if (model) {
    const name = codexModelName(window);
    return model[1] === "7d"
      ? { label: `${name} week`, compact: name }
      : { label: `${name} 5h`, compact: `${name} 5h` };
  }
  return passthrough(window);
}

function cursorTierLabel(window: QuotaWindow): TierLabel {
  if (window.id === "included_usage")
    return { label: "Included", compact: "Included" };
  if (window.id === "auto_usage") return { label: "Auto", compact: "Auto" };
  if (window.id === "api_usage")
    return { label: "3rd-party models", compact: "3rd-party" };
  if (window.id === "spend_limit")
    return { label: "Spend limit", compact: "Spend" };
  return passthrough(window);
}

function kimiTierLabel(window: QuotaWindow): TierLabel {
  if (window.id === "five_hour")
    return { label: "Session", compact: "Session" };
  if (window.id === "weekly") return { label: "Week", compact: "Week" };
  return passthrough(window);
}

const TIER_LABELS: Record<string, (window: QuotaWindow) => TierLabel> = {
  claude: claudeTierLabel,
  codex: codexTierLabel,
  cursor: cursorTierLabel,
  kimi: kimiTierLabel,
};

function tierConclusion(window: QuotaWindow): TierConclusion {
  const pace = window.pace?.status;
  if (pace === "ahead") {
    return window.pace?.projectedExhaustedAt
      ? {
          kind: "ahead",
          projectedExhaustedAt: window.pace.projectedExhaustedAt,
        }
      : { kind: "ahead" };
  }
  if (pace === "on_pace" || pace === "behind") return { kind: "on_pace" };
  if (window.spentUsd !== undefined) {
    return window.limitUsd === undefined
      ? { kind: "spend", spentUsd: window.spentUsd }
      : { kind: "spend", spentUsd: window.spentUsd, limitUsd: window.limitUsd };
  }
  return { kind: "unknown" };
}

function limitingWindowIds(provider: ProviderQuota): Set<string> {
  const primary =
    provider.effective.find(
      (item) => item.scope === "all_models" || item.scope === "all_products",
    ) ?? provider.effective.find((item) => item.status === "known");
  return new Set(primary?.limitingWindowIds ?? []);
}

/**
 * Every trustworthy provider window becomes one tier row, in provider order.
 * Codex additionally gets an honest "Code review -- not reported" row when
 * neither code-review window is returned, because code review is a separate
 * workload that does not share the base quota.
 */
export function providerTiers(provider: ProviderQuota): TierRow[] {
  const toLabel = TIER_LABELS[provider.provider.toLowerCase()] ?? passthrough;
  const limiting = limitingWindowIds(provider);
  const rows: TierRow[] = provider.windows.map((window) => {
    const { label, compact } = toLabel(window);
    return {
      id: window.id,
      label,
      compactLabel: compact,
      ...(window.percentRemaining === undefined
        ? {}
        : { percentRemaining: window.percentRemaining }),
      ...(window.resetsAt ? { resetsAt: window.resetsAt } : {}),
      conclusion: tierConclusion(window),
      limiting: limiting.has(window.id),
    };
  });
  if (
    provider.provider.toLowerCase() === "codex" &&
    provider.windows.length > 0 &&
    !provider.windows.some((window) => window.id.startsWith("code_review_"))
  ) {
    rows.push({
      id: "code_review",
      label: "Code review",
      compactLabel: "Review",
      conclusion: { kind: "not_reported" },
      limiting: false,
    });
  }
  return rows;
}

export function providerNeedsSignIn(provider: ProviderQuota): boolean {
  return (
    provider.state.status === "auth_required" ||
    provider.state.authStatus === "unusable" ||
    provider.state.authStatus === "expired_refreshable"
  );
}

/**
 * Decides what fills a provider section. Signed-out and unreadable providers
 * never show numbers, so an unavailable reading is never mistaken for zero
 * quota.
 */
export function presentProvider(provider: ProviderQuota): ProviderPresentation {
  if (provider.state.reason === "keychain_access_required") {
    return { kind: "message", message: "Keychain approval required" };
  }
  if (providerNeedsSignIn(provider)) {
    return {
      kind: "recovery",
      instruction:
        RECOVERY_INSTRUCTIONS[provider.provider.toLowerCase()] ??
        "sign in with the provider CLI",
    };
  }
  if (!provider.windows.length) {
    return {
      kind: "message",
      message: friendlyProviderError(
        provider.state.errorCode ?? provider.state.reason,
      ),
    };
  }
  return { kind: "tiers", rows: providerTiers(provider) };
}

export function providerAnnotation(
  provider: ProviderQuota,
): ProviderAnnotation | undefined {
  if (provider.state.reason === "keychain_access_required") return undefined;
  if (providerNeedsSignIn(provider))
    return { text: "signed out", tone: "bad" };
  if (provider.state.stale || provider.state.status === "stale")
    return { text: "stale", tone: "warn" };
  if (provider.state.status === "rate_limited")
    return { text: "rate limited", tone: "warn" };
  if (provider.state.status === "error") return { text: "error", tone: "bad" };
  if (provider.state.status === "unavailable" || !provider.windows.length)
    return { text: "no reading", tone: "muted" };
  return undefined;
}
