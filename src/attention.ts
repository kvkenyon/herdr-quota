import { displayName } from "./format.js";
import {
  isAllowedProvider,
  providerNeedsSignIn,
  providerTiers,
} from "./tiers.js";
import type {
  EffectiveAvailability,
  ProviderQuota,
  QuotaReport,
} from "./types.js";

export type ConstraintKind = "exhausted" | "projected";

export type Attention =
  | {
      kind: "constraint";
      severity: "critical" | "warning";
      provider: string;
      tier?: string;
      compactTier?: string;
      constraint: ConstraintKind;
      percentRemaining?: number;
      projectedExhaustedAt?: string;
      projectionConfidence?: "early" | "established";
      resetsAt?: string;
    }
  | { kind: "healthy"; tracked: number }
  | {
      kind: "data_health";
      reason: "partial";
      partial: number;
      tracked: number;
    }
  | {
      kind: "data_health";
      reason: "unreadable" | "pace_unknown";
      unreadable: number;
      tracked: number;
    };

interface RankedConstraint {
  attention: Extract<Attention, { kind: "constraint" }>;
  rank: number;
  time: number;
  remaining: number;
  providerOrder: number;
  effectiveOrder: number;
}

function providerName(provider: ProviderQuota): string {
  const name = displayName(provider);
  return name === "OpenAI Codex" ? "Codex" : name;
}

function providerIsCurrent(provider: ProviderQuota): boolean {
  return (
    provider.state.status === "fresh" &&
    !provider.state.stale &&
    !providerNeedsSignIn(provider)
  );
}

function knownEffective(provider: ProviderQuota): EffectiveAvailability[] {
  return provider.effective.filter((item) => item.status === "known");
}

function decisionGrade(item: EffectiveAvailability): boolean {
  return (
    item.effectivePercentRemaining !== undefined ||
    (item.runway !== undefined && item.runway.status !== "unknown") ||
    (item.pace !== undefined && item.pace.status !== "unknown")
  );
}

function onPace(item: EffectiveAvailability): boolean {
  if (item.runway?.status === "through_reset") return true;
  return item.pace?.status === "on_pace" || item.pace?.status === "behind";
}

function numericTime(value?: string): number {
  if (!value) return Number.POSITIVE_INFINITY;
  const parsed = Date.parse(value);
  return Number.isFinite(parsed) ? parsed : Number.POSITIVE_INFINITY;
}

function constraintFor(
  provider: ProviderQuota,
  effective: EffectiveAvailability,
  providerOrder: number,
  effectiveOrder: number,
): RankedConstraint | undefined {
  const runway = effective.runway;
  const percent = effective.effectivePercentRemaining;
  let constraint: ConstraintKind;
  let rank: number;

  if (runway?.status === "exhausted_now") {
    constraint = "exhausted";
    rank = 0;
  } else if (
    runway?.status === "projected_exhaustion" &&
    runway.projectionConfidence === "established"
  ) {
    constraint = "projected";
    rank = 1;
  } else {
    return undefined;
  }

  const limitingId =
    runway?.limitingWindowId ??
    effective.pace?.worstReserveWindowId ??
    effective.limitingWindowIds?.[0];
  const row = limitingId
    ? providerTiers(provider).find((item) => item.id === limitingId)
    : undefined;
  const window = limitingId
    ? provider.windows.find((item) => item.id === limitingId)
    : undefined;

  return {
    attention: {
      kind: "constraint",
      severity: "critical",
      provider: providerName(provider),
      ...(row ? { tier: row.label, compactTier: row.compactLabel } : {}),
      constraint,
      ...(percent === undefined ? {} : { percentRemaining: percent }),
      ...(runway?.projectedExhaustedAt
        ? { projectedExhaustedAt: runway.projectedExhaustedAt }
        : {}),
      ...(runway?.projectionConfidence
        ? { projectionConfidence: runway.projectionConfidence }
        : {}),
      ...(window?.resetsAt ? { resetsAt: window.resetsAt } : {}),
    },
    rank,
    time:
      constraint === "projected"
        ? numericTime(runway?.projectedExhaustedAt)
        : numericTime(window?.resetsAt),
    remaining: percent ?? 101,
    providerOrder,
    effectiveOrder,
  };
}

function compareNumber(left: number, right: number): number {
  if (left === right) return 0;
  return left < right ? -1 : 1;
}

function compareConstraint(left: RankedConstraint, right: RankedConstraint) {
  const rank = left.rank - right.rank;
  if (rank) return rank;
  // Among forecasts, the first exhaustion wins. Among already exhausted
  // limits, the later/unknown recovery is the stronger block.
  const withinRank =
    left.rank === 1
      ? compareNumber(left.time, right.time) ||
        compareNumber(left.remaining, right.remaining)
      : compareNumber(left.remaining, right.remaining) ||
        compareNumber(right.time, left.time);
  return (
    withinRank ||
    left.providerOrder - right.providerOrder ||
    left.effectiveOrder - right.effectiveOrder
  );
}

/**
 * Selects the one decision-grade signal that belongs above provider detail.
 * It consumes only adapted schema-v5 state and never derives a quota cap.
 */
export function selectAttention(report: QuotaReport): Attention {
  const providers = report.providers.filter((provider) =>
    isAllowedProvider(provider.provider),
  );
  const constraints: RankedConstraint[] = [];
  const trackedEffective: EffectiveAvailability[] = [];
  let tracked = 0;
  let unreadable = 0;
  let partial = 0;

  providers.forEach((provider, providerOrder) => {
    if (provider.semanticsStatus === "partial") partial++;
    const known = knownEffective(provider).filter(decisionGrade);
    if (
      !providerIsCurrent(provider) ||
      (known.length === 0 && provider.semanticsStatus !== "partial")
    ) {
      unreadable++;
      return;
    }
    if (known.length === 0) return;
    tracked++;
    trackedEffective.push(...known);
    known.forEach((effective, effectiveOrder) => {
      const candidate = constraintFor(
        provider,
        effective,
        providerOrder,
        effectiveOrder,
      );
      if (candidate) constraints.push(candidate);
    });
  });

  const limiting = constraints.toSorted(compareConstraint)[0];
  if (limiting) return limiting.attention;
  if (unreadable > 0) {
    return { kind: "data_health", reason: "unreadable", unreadable, tracked };
  }
  if (partial > 0) {
    return { kind: "data_health", reason: "partial", partial, tracked };
  }
  if (tracked > 0 && trackedEffective.every(onPace)) {
    return { kind: "healthy", tracked };
  }
  return {
    kind: "data_health",
    reason: "pace_unknown",
    unreadable: 0,
    tracked,
  };
}
