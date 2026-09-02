import type {
  EffectiveAvailability,
  PaceStatus,
  ProviderQuota,
  QuotaReport,
  QuotaWindow,
} from "./types.js";
import { marketedProvider } from "./types.js";
import { stripAnsi } from "./ansi.js";

const STATUSES = new Set([
  "fresh",
  "stale",
  "unavailable",
  "auth_required",
  "rate_limited",
  "error",
]);
const PACE = new Set(["ahead", "on_pace", "behind", "mixed", "unknown"]);
const RUNWAY = new Set([
  "exhausted_now",
  "projected_exhaustion",
  "through_reset",
  "unknown",
]);
function object(value: unknown): Record<string, unknown> | undefined {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : undefined;
}

function text(value: unknown): string | undefined {
  if (typeof value !== "string") return undefined;
  const cleaned = stripAnsi(value)
    .split("")
    .filter((character) => {
      const code = character.charCodeAt(0);
      return code > 31 && (code < 127 || code > 159);
    })
    .join("")
    .trim()
    .slice(0, 256);
  return cleaned || undefined;
}

function number(
  value: unknown,
  min?: number,
  max?: number,
): number | undefined {
  if (typeof value !== "number" || !Number.isFinite(value)) return undefined;
  if (min !== undefined && value < min) return undefined;
  if (max !== undefined && value > max) return undefined;
  return value;
}

function iso(value: unknown): string | undefined {
  const candidate = text(value);
  return candidate && !Number.isNaN(Date.parse(candidate))
    ? candidate
    : undefined;
}

function strings(value: unknown): string[] {
  return Array.isArray(value)
    ? value.map(text).filter((item): item is string => !!item)
    : [];
}

function adaptWindow(value: unknown): QuotaWindow | undefined {
  const raw = object(value);
  const id = text(raw?.id);
  if (!raw || !id) return undefined;
  const paceRaw = object(raw.pace);
  const paceStatus = text(paceRaw?.status);
  const projectionConfidence: "early" | "established" | undefined =
    paceRaw?.projectionConfidence === "early" ||
    paceRaw?.projectionConfidence === "established"
      ? paceRaw.projectionConfidence
      : undefined;
  const pace =
    paceStatus && PACE.has(paceStatus)
      ? {
          status: paceStatus as PaceStatus,
          ...(number(paceRaw?.reservePercentPoints) === undefined
            ? {}
            : { reservePercentPoints: number(paceRaw?.reservePercentPoints) }),
          ...(number(paceRaw?.burnMultiple, 0) === undefined
            ? {}
            : { burnMultiple: number(paceRaw?.burnMultiple, 0) }),
          ...(iso(paceRaw?.projectedExhaustedAt)
            ? { projectedExhaustedAt: iso(paceRaw?.projectedExhaustedAt) }
            : {}),
          ...(projectionConfidence ? { projectionConfidence } : {}),
        }
      : undefined;
  return {
    id,
    label: text(raw.label) ?? id.replaceAll("_", " "),
    kind: text(raw.kind) ?? "unknown",
    ...(number(raw.percentUsed, 0, 100) === undefined
      ? {}
      : { percentUsed: number(raw.percentUsed, 0, 100) }),
    ...(number(raw.percentRemaining, 0, 100) === undefined
      ? {}
      : { percentRemaining: number(raw.percentRemaining, 0, 100) }),
    ...(iso(raw.startsAt) ? { startsAt: iso(raw.startsAt) } : {}),
    ...(iso(raw.resetsAt) ? { resetsAt: iso(raw.resetsAt) } : {}),
    ...(text(raw.resetText) ? { resetText: text(raw.resetText) } : {}),
    ...(number(raw.windowSeconds, 1) === undefined
      ? {}
      : { windowSeconds: number(raw.windowSeconds, 1) }),
    ...(number(raw.spentUsd, 0) === undefined
      ? {}
      : { spentUsd: number(raw.spentUsd, 0) }),
    ...(number(raw.limitUsd, 0) === undefined
      ? {}
      : { limitUsd: number(raw.limitUsd, 0) }),
    ...(pace ? { pace } : {}),
  };
}

function adaptEffective(value: unknown): EffectiveAvailability | undefined {
  const raw = object(value);
  const scope = text(raw?.scope);
  if (!raw || !scope) return undefined;
  const status = raw.status === "known" ? "known" : "unknown";
  const paceRaw = object(raw.pace);
  const paceStatus = text(paceRaw?.status);
  const runwayRaw = object(raw.runway);
  const runwayStatus = text(runwayRaw?.status);
  return {
    scope,
    status,
    ...(number(raw.effectivePercentRemaining, 0, 100) === undefined
      ? {}
      : {
          effectivePercentRemaining: number(
            raw.effectivePercentRemaining,
            0,
            100,
          ),
        }),
    boundedBy: strings(raw.boundedBy),
    ...(strings(raw.limitingWindowIds).length
      ? { limitingWindowIds: strings(raw.limitingWindowIds) }
      : {}),
    ...(paceStatus && PACE.has(paceStatus)
      ? {
          pace: {
            status: paceStatus as EffectiveAvailability["pace"] extends infer P
              ? P extends { status: infer S }
                ? S
                : never
              : never,
            ...(number(paceRaw?.worstReservePercentPoints) === undefined
              ? {}
              : {
                  worstReservePercentPoints: number(
                    paceRaw?.worstReservePercentPoints,
                  ),
                }),
            ...(text(paceRaw?.worstReserveWindowId)
              ? { worstReserveWindowId: text(paceRaw?.worstReserveWindowId) }
              : {}),
            ...(strings(paceRaw?.unknownWindowIds).length
              ? { unknownWindowIds: strings(paceRaw?.unknownWindowIds) }
              : {}),
          },
        }
      : {}),
    ...(runwayStatus && RUNWAY.has(runwayStatus)
      ? {
          runway: {
            status:
              runwayStatus as EffectiveAvailability["runway"] extends infer R
                ? R extends { status: infer S }
                  ? S
                  : never
                : never,
            ...(number(runwayRaw?.usableRunwaySeconds, 0) === undefined
              ? {}
              : {
                  usableRunwaySeconds: number(
                    runwayRaw?.usableRunwaySeconds,
                    0,
                  ),
                }),
            ...(iso(runwayRaw?.projectedExhaustedAt)
              ? { projectedExhaustedAt: iso(runwayRaw?.projectedExhaustedAt) }
              : {}),
            ...(text(runwayRaw?.limitingWindowId)
              ? { limitingWindowId: text(runwayRaw?.limitingWindowId) }
              : {}),
            ...(runwayRaw?.projectionConfidence === "early" ||
            runwayRaw?.projectionConfidence === "established"
              ? { projectionConfidence: runwayRaw.projectionConfidence }
              : {}),
            ...(strings(runwayRaw?.unmeasurableWindowIds).length
              ? {
                  unmeasurableWindowIds: strings(
                    runwayRaw?.unmeasurableWindowIds,
                  ),
                }
              : {}),
          },
        }
      : {}),
  };
}

function invalidProvider(label: string): ProviderQuota {
  return {
    provider: label,
    label,
    windows: [],
    effective: [],
    state: {
      status: "error",
      stale: false,
      errorCode: "schema_invalid",
    },
  };
}

function adaptProvider(
  value: unknown,
  index: number,
  warnings: string[],
): ProviderQuota {
  const raw = object(value);
  const provider = text(raw?.provider);
  if (!raw || !provider) {
    warnings.push(`provider ${index + 1} did not match schema v5`);
    return invalidProvider(`provider-${index + 1}`);
  }
  const stateRaw = object(raw.state);
  if (!stateRaw || !Array.isArray(raw.windows)) {
    warnings.push(`${provider} did not match schema v5`);
    return invalidProvider(provider);
  }
  const statusRaw = text(stateRaw.status);
  const status = statusRaw && STATUSES.has(statusRaw) ? statusRaw : "error";
  const windows = raw.windows
    .map(adaptWindow)
    .filter((item): item is QuotaWindow => !!item);
  if (windows.length !== raw.windows.length)
    warnings.push(`${provider} omitted malformed windows`);
  const semantics = object(raw.quotaSemantics);
  const effectiveRaw = semantics?.effectiveAvailability;
  const effective = Array.isArray(effectiveRaw)
    ? effectiveRaw
        .map(adaptEffective)
        .filter((item): item is EffectiveAvailability => !!item)
    : [];
  const creditsRaw = object(raw.credits);
  return {
    provider,
    ...(text(raw.label) ? { label: text(raw.label) } : {}),
    ...(text(raw.source) ? { source: text(raw.source) } : {}),
    ...(text(raw.plan) ? { plan: text(raw.plan) } : {}),
    windows,
    effective,
    ...(semantics?.status === "known" ||
    semantics?.status === "partial" ||
    semantics?.status === "unknown"
      ? { semanticsStatus: semantics.status }
      : {}),
    ...(creditsRaw
      ? {
          credits: {
            ...(number(creditsRaw.remaining, 0) === undefined
              ? {}
              : { remaining: number(creditsRaw.remaining, 0) }),
            ...(typeof creditsRaw.unlimited === "boolean"
              ? { unlimited: creditsRaw.unlimited }
              : {}),
            ...(creditsRaw.unit === "usd" || creditsRaw.unit === "credits"
              ? { unit: creditsRaw.unit }
              : {}),
          },
        }
      : {}),
    state: {
      status,
      stale: stateRaw.stale === true || status === "stale",
      ...(iso(stateRaw.refreshedAt)
        ? { refreshedAt: iso(stateRaw.refreshedAt) }
        : {}),
      ...(text(stateRaw.authStatus)
        ? { authStatus: text(stateRaw.authStatus) }
        : {}),
      ...(text(stateRaw.reason) ? { reason: text(stateRaw.reason) } : {}),
      ...(text(stateRaw.remedyCommand)
        ? { remedyCommand: text(stateRaw.remedyCommand) }
        : {}),
      ...(text(stateRaw.error) ? { errorCode: text(stateRaw.error) } : {}),
    },
  };
}

export function adaptQuotaResponse(value: unknown): QuotaReport {
  const raw = object(value);
  if (!raw || raw.schemaVersion !== 5) {
    throw new Error("Unsupported quota-axi JSON schema (expected version 5)");
  }
  const generatedAt = iso(raw.generatedAt);
  if (!generatedAt || !Array.isArray(raw.providers)) {
    throw new Error("Invalid quota-axi schema v5 response");
  }
  const warnings: string[] = [];
  // The product renders only the marketed providers. An entry whose provider
  // id cannot even be read still becomes a visible error card rather than
  // being silently discarded.
  const accepted = raw.providers.filter((item) => {
    const id = text(object(item)?.provider);
    return id === undefined || marketedProvider(id.toLowerCase()) !== undefined;
  });
  return {
    generatedAt,
    schemaVersion: 5,
    providers: accepted.map((item, index) =>
      adaptProvider(item, index, warnings),
    ),
    adaptationWarnings: warnings,
  };
}
