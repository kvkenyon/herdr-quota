import { randomUUID } from "node:crypto";
import { mkdir, readFile, rename, unlink, writeFile } from "node:fs/promises";
import { homedir } from "node:os";
import { dirname, join } from "node:path";
import { displayName } from "./format.js";
import { providerNeedsSignIn } from "./tiers.js";
import type {
  EffectiveAvailability,
  HistoryAvailability,
  HistoryEvidence,
  HistoryEvidenceKind,
  HistoryView,
  PaceStatus,
  ProviderQuota,
  QuotaReport,
  RunwayStatus,
} from "./types.js";

export const HISTORY_SCHEMA_VERSION = 1;
export const HISTORY_MAX_SNAPSHOTS = 512;
export const HISTORY_MAX_AGE_MS = 30 * 24 * 60 * 60_000;
export const HISTORY_EQUIVALENT_INTERVAL_MS = 15 * 60_000;
export const HISTORY_CLOCK_SKEW_MS = 5 * 60_000;
const HISTORY_MAX_FACTS_PER_PROVIDER = 8;
const MEANINGFUL_REMAINING_DROP = 10;
const MEANINGFUL_RESERVE_CHANGE = 10;
const MEANINGFUL_PROJECTION_CHANGE_MS = 2 * 60 * 60_000;

const PROVIDERS = ["Claude", "OpenAI Codex", "Cursor", "Kimi"] as const;
type HistoryProviderName = (typeof PROVIDERS)[number];
type HistoryDataHealth =
  "current" | "stale" | "unavailable" | "error" | "unknown";
type HistoryPaceStatus = Exclude<PaceStatus, "unknown">;
type HistoryRunwayStatus = Exclude<RunwayStatus, "unknown">;

export interface HistoryPaceFact {
  state: HistoryPaceStatus;
  reserve?: number;
}

export interface HistoryRunwayFact {
  state: HistoryRunwayStatus;
  projectedAt?: string;
  confidence?: "early" | "established";
}

export interface HistoryFact {
  scope: string;
  limit?: string;
  remaining: number;
  resetAt?: string;
  pace?: HistoryPaceFact;
  runway?: HistoryRunwayFact;
}

export interface HistoryProviderSnapshot {
  provider: HistoryProviderName;
  dataHealth: HistoryDataHealth;
  authEligible: boolean;
  facts: HistoryFact[];
}

export interface HistorySnapshot {
  capturedAt: string;
  providers: HistoryProviderSnapshot[];
}

export interface HistoryDocument {
  schemaVersion: 1;
  snapshots: HistorySnapshot[];
}

export interface HistoryFileOperations {
  readFile(path: string, encoding: "utf8"): Promise<string>;
  mkdir(
    path: string,
    options: { recursive: true; mode: number },
  ): Promise<unknown>;
  writeFile(
    path: string,
    value: string,
    options: { encoding: "utf8"; mode: number; flag: "wx" },
  ): Promise<unknown>;
  rename(from: string, to: string): Promise<unknown>;
  unlink(path: string): Promise<unknown>;
}

const FILE_OPERATIONS: HistoryFileOperations = {
  readFile: (path, encoding) => readFile(path, encoding),
  mkdir: (path, options) => mkdir(path, options),
  writeFile: (path, value, options) => writeFile(path, value, options),
  rename: (from, to) => rename(from, to),
  unlink: (path) => unlink(path),
};

function isObject(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function exactKeys(value: Record<string, unknown>, keys: string[]): boolean {
  const actual = Object.keys(value).toSorted();
  return (
    actual.length === keys.length &&
    actual.every((key, index) => key === keys.toSorted()[index])
  );
}

function finite(value: unknown, min: number, max: number): value is number {
  return (
    typeof value === "number" &&
    Number.isFinite(value) &&
    value >= min &&
    value <= max
  );
}

function iso(value: unknown): value is string {
  if (typeof value !== "string" || value.length !== 24) return false;
  const parsed = Date.parse(value);
  return Number.isFinite(parsed) && new Date(parsed).toISOString() === value;
}

function safeIdentity(value: unknown): value is string {
  return (
    typeof value === "string" &&
    value.length > 0 &&
    value.length <= 32 &&
    /^[A-Za-z0-9][A-Za-z0-9 ._+-]*$/.test(value) &&
    !/(?:bearer|secret|token|credential|account|auth|api.?key)/i.test(value)
  );
}

function parsePace(value: unknown): HistoryPaceFact | undefined {
  if (!isObject(value)) return undefined;
  const keys = ["state", ...(value.reserve === undefined ? [] : ["reserve"])];
  if (!exactKeys(value, keys)) return undefined;
  if (
    value.state !== "ahead" &&
    value.state !== "on_pace" &&
    value.state !== "behind" &&
    value.state !== "mixed"
  )
    return undefined;
  if (value.reserve !== undefined && !finite(value.reserve, -10_000, 10_000))
    return undefined;
  return {
    state: value.state,
    ...(value.reserve === undefined ? {} : { reserve: value.reserve }),
  };
}

function parseRunway(value: unknown): HistoryRunwayFact | undefined {
  if (!isObject(value)) return undefined;
  const keys = [
    "state",
    ...(value.projectedAt === undefined ? [] : ["projectedAt"]),
    ...(value.confidence === undefined ? [] : ["confidence"]),
  ];
  if (!exactKeys(value, keys)) return undefined;
  if (
    value.state !== "exhausted_now" &&
    value.state !== "projected_exhaustion" &&
    value.state !== "through_reset"
  )
    return undefined;
  if (value.projectedAt !== undefined && !iso(value.projectedAt))
    return undefined;
  if (
    value.confidence !== undefined &&
    value.confidence !== "early" &&
    value.confidence !== "established"
  )
    return undefined;
  return {
    state: value.state,
    ...(value.projectedAt === undefined
      ? {}
      : { projectedAt: value.projectedAt }),
    ...(value.confidence === undefined ? {} : { confidence: value.confidence }),
  };
}

function parseFact(value: unknown): HistoryFact | undefined {
  if (!isObject(value)) return undefined;
  const keys = [
    "scope",
    "remaining",
    ...(value.limit === undefined ? [] : ["limit"]),
    ...(value.resetAt === undefined ? [] : ["resetAt"]),
    ...(value.pace === undefined ? [] : ["pace"]),
    ...(value.runway === undefined ? [] : ["runway"]),
  ];
  if (
    !exactKeys(value, keys) ||
    !safeIdentity(value.scope) ||
    !finite(value.remaining, 0, 100) ||
    (value.limit !== undefined && !safeIdentity(value.limit)) ||
    (value.resetAt !== undefined && !iso(value.resetAt))
  )
    return undefined;
  const pace = value.pace === undefined ? undefined : parsePace(value.pace);
  const runway =
    value.runway === undefined ? undefined : parseRunway(value.runway);
  if (value.pace !== undefined && !pace) return undefined;
  if (value.runway !== undefined && !runway) return undefined;
  return {
    scope: value.scope,
    ...(value.limit === undefined ? {} : { limit: value.limit }),
    remaining: value.remaining,
    ...(value.resetAt === undefined ? {} : { resetAt: value.resetAt }),
    ...(pace ? { pace } : {}),
    ...(runway ? { runway } : {}),
  };
}

function parseProvider(value: unknown): HistoryProviderSnapshot | undefined {
  if (
    !isObject(value) ||
    !exactKeys(value, ["provider", "dataHealth", "authEligible", "facts"]) ||
    !PROVIDERS.includes(value.provider as HistoryProviderName) ||
    (value.dataHealth !== "current" &&
      value.dataHealth !== "stale" &&
      value.dataHealth !== "unavailable" &&
      value.dataHealth !== "error" &&
      value.dataHealth !== "unknown") ||
    typeof value.authEligible !== "boolean" ||
    !Array.isArray(value.facts) ||
    value.facts.length > HISTORY_MAX_FACTS_PER_PROVIDER
  )
    return undefined;
  const facts = value.facts.map(parseFact);
  if (facts.some((fact) => fact === undefined)) return undefined;
  if (
    (value.dataHealth !== "current" || !value.authEligible) &&
    facts.length > 0
  )
    return undefined;
  return {
    provider: value.provider as HistoryProviderName,
    dataHealth: value.dataHealth,
    authEligible: value.authEligible,
    facts: facts as HistoryFact[],
  };
}

function parseSnapshot(value: unknown): HistorySnapshot | undefined {
  if (
    !isObject(value) ||
    !exactKeys(value, ["capturedAt", "providers"]) ||
    !iso(value.capturedAt) ||
    !Array.isArray(value.providers) ||
    value.providers.length > PROVIDERS.length
  )
    return undefined;
  const providers = value.providers.map(parseProvider);
  if (providers.some((provider) => provider === undefined)) return undefined;
  if (
    new Set(providers.map((provider) => provider!.provider)).size !==
    providers.length
  )
    return undefined;
  return {
    capturedAt: value.capturedAt,
    providers: providers as HistoryProviderSnapshot[],
  };
}

export function parseHistoryDocument(value: unknown): HistoryDocument {
  if (!isObject(value)) throw new Error("history_corrupt");
  if (value.schemaVersion !== HISTORY_SCHEMA_VERSION)
    throw new Error("history_incompatible");
  if (
    !exactKeys(value, ["schemaVersion", "snapshots"]) ||
    !Array.isArray(value.snapshots) ||
    value.snapshots.length > HISTORY_MAX_SNAPSHOTS
  )
    throw new Error("history_corrupt");
  const snapshots = value.snapshots.map(parseSnapshot);
  if (snapshots.some((snapshot) => snapshot === undefined))
    throw new Error("history_corrupt");
  for (let index = 1; index < snapshots.length; index++) {
    if (
      Date.parse(snapshots[index - 1]!.capturedAt) >=
      Date.parse(snapshots[index]!.capturedAt)
    )
      throw new Error("history_corrupt");
  }
  return {
    schemaVersion: HISTORY_SCHEMA_VERSION,
    snapshots: snapshots as HistorySnapshot[],
  };
}

function providerHealth(provider: ProviderQuota): {
  dataHealth: HistoryDataHealth;
  authEligible: boolean;
} {
  const authEligible = !providerNeedsSignIn(provider);
  if (!authEligible) return { dataHealth: "unavailable", authEligible };
  if (provider.state.stale || provider.state.status === "stale")
    return { dataHealth: "stale", authEligible };
  if (provider.state.status === "fresh")
    return { dataHealth: "current", authEligible };
  if (provider.state.status === "error")
    return { dataHealth: "error", authEligible };
  if (
    provider.state.status === "unavailable" ||
    provider.state.status === "rate_limited"
  )
    return { dataHealth: "unavailable", authEligible };
  return { dataHealth: "unknown", authEligible };
}

function modelIdentity(
  id: string,
): { model: string; period?: "5h" | "7d" } | undefined {
  const match =
    /^model:([A-Za-z0-9][A-Za-z0-9_-]{0,39})(?::(5h|7d)(?:_\d+)?)?$/.exec(id);
  if (!match) return undefined;
  const slug = match[1]!.replace(/^codex_/, "").replaceAll("_", " ");
  const model = slug.replaceAll(/\b\w/g, (letter) => letter.toUpperCase());
  if (!safeIdentity(model)) return undefined;
  return {
    model,
    ...(match[2] === "5h" || match[2] === "7d" ? { period: match[2] } : {}),
  };
}

function safeScopeLabel(effective: EffectiveAvailability): string | undefined {
  if (effective.scope === "all_models") return "All models";
  if (effective.scope === "all_products") return "All products";
  return modelIdentity(effective.scope)?.model;
}

function safeLimitIdentity(
  provider: ProviderQuota,
  id: string,
): string | undefined {
  const fixed: Record<string, string> = {
    "claude:five_hour": "Session",
    "claude:seven_day": "Week",
    "claude:seven_day_opus": "Opus",
    "claude:extra_usage": "Extra",
    "codex:five_hour": "Session",
    "codex:weekly": "Week",
    "cursor:included_usage": "Included",
    "cursor:auto_usage": "Auto",
    "cursor:api_usage": "3rd-party",
    "cursor:spend_limit": "Spend",
    "kimi:five_hour": "Session",
    "kimi:weekly": "Week",
  };
  const known = fixed[`${provider.provider.toLowerCase()}:${id}`];
  if (known) return known;
  if (/^code_review_five_hour/.test(id)) return "Review 5h";
  if (/^code_review_weekly/.test(id)) return "Review wk";
  const model = modelIdentity(id);
  if (model)
    return model.period === "5h"
      ? `${model.model} 5h`
      : model.period === "7d"
        ? `${model.model} week`
        : model.model;
  const numbered = /^limit:(\d{1,3})$/.exec(id)?.[1];
  return numbered ? `Limit ${numbered}` : undefined;
}

function safeLimitLabel(
  provider: ProviderQuota,
  effective: EffectiveAvailability,
): { label?: string; resetAt?: string } {
  const limitId =
    effective.runway?.limitingWindowId ??
    effective.pace?.worstReserveWindowId ??
    effective.limitingWindowIds?.[0];
  if (!limitId) return {};
  const window = provider.windows.find((item) => item.id === limitId);
  const label = safeLimitIdentity(provider, limitId);
  return {
    ...(label ? { label } : {}),
    ...(window?.resetsAt
      ? { resetAt: new Date(window.resetsAt).toISOString() }
      : {}),
  };
}

function normalizePace(
  effective: EffectiveAvailability,
): HistoryPaceFact | undefined {
  const pace = effective.pace;
  if (!pace || pace.status === "unknown") return undefined;
  const reserve = pace.worstReservePercentPoints;
  return {
    state: pace.status,
    ...(reserve === undefined || !finite(reserve, -10_000, 10_000)
      ? {}
      : { reserve }),
  };
}

function normalizeRunway(
  effective: EffectiveAvailability,
): HistoryRunwayFact | undefined {
  const runway = effective.runway;
  if (!runway || runway.status === "unknown") return undefined;
  return {
    state: runway.status,
    ...(runway.projectedExhaustedAt
      ? { projectedAt: new Date(runway.projectedExhaustedAt).toISOString() }
      : {}),
    ...(runway.projectionConfidence
      ? { confidence: runway.projectionConfidence }
      : {}),
  };
}

function normalizeProvider(
  provider: ProviderQuota,
): HistoryProviderSnapshot | undefined {
  const marketed = displayName(provider);
  if (!PROVIDERS.includes(marketed as HistoryProviderName)) return undefined;
  const health = providerHealth(provider);
  let facts: HistoryFact[] = [];
  if (
    health.dataHealth === "current" &&
    health.authEligible &&
    provider.semanticsStatus !== "unknown"
  ) {
    for (const effective of provider.effective) {
      if (
        facts.length >= HISTORY_MAX_FACTS_PER_PROVIDER ||
        effective.status !== "known" ||
        effective.effectivePercentRemaining === undefined
      )
        continue;
      const scope = safeScopeLabel(effective);
      if (!scope) continue;
      const limit = safeLimitLabel(provider, effective);
      const pace = normalizePace(effective);
      const runway = normalizeRunway(effective);
      facts.push({
        scope,
        ...(limit.label ? { limit: limit.label } : {}),
        remaining: effective.effectivePercentRemaining,
        ...(limit.resetAt ? { resetAt: limit.resetAt } : {}),
        ...(pace ? { pace } : {}),
        ...(runway ? { runway } : {}),
      });
    }
  }
  facts = facts.toSorted((left, right) =>
    left.scope.localeCompare(right.scope),
  );
  return {
    provider: marketed as HistoryProviderName,
    ...health,
    facts,
  };
}

export function normalizeHistorySnapshot(
  report: QuotaReport,
  now = new Date(),
): HistorySnapshot | undefined {
  if (!Number.isFinite(now.getTime())) return undefined;
  const providers = report.providers
    .map(normalizeProvider)
    .filter((provider): provider is HistoryProviderSnapshot => !!provider)
    .toSorted(
      (left, right) =>
        PROVIDERS.indexOf(left.provider) - PROVIDERS.indexOf(right.provider),
    );
  if (!providers.some((provider) => provider.facts.length > 0))
    return undefined;
  return { capturedAt: now.toISOString(), providers };
}

function snapshotFingerprint(snapshot: HistorySnapshot): string {
  return JSON.stringify(snapshot.providers);
}

export interface HistoryUpdate {
  document: HistoryDocument;
  wrote: boolean;
  clockSkew: boolean;
}

export function updateHistoryDocument(
  document: HistoryDocument,
  snapshot: HistorySnapshot,
): HistoryUpdate {
  const currentAt = Date.parse(snapshot.capturedAt);
  const previous = document.snapshots.at(-1);
  const previousAt = previous ? Date.parse(previous.capturedAt) : undefined;
  if (
    previousAt !== undefined &&
    currentAt < previousAt - HISTORY_CLOCK_SKEW_MS
  ) {
    return {
      document: {
        schemaVersion: HISTORY_SCHEMA_VERSION,
        snapshots: [snapshot],
      },
      wrote: true,
      clockSkew: true,
    };
  }
  if (previousAt !== undefined && currentAt <= previousAt)
    return { document, wrote: false, clockSkew: false };

  const cutoff = currentAt - HISTORY_MAX_AGE_MS;
  let snapshots = document.snapshots.filter(
    (item) => Date.parse(item.capturedAt) >= cutoff,
  );
  const equivalent =
    previous && snapshotFingerprint(previous) === snapshotFingerprint(snapshot);
  const shouldAppend =
    !equivalent ||
    previousAt === undefined ||
    currentAt - previousAt >= HISTORY_EQUIVALENT_INTERVAL_MS;
  if (shouldAppend) snapshots.push(snapshot);
  snapshots = snapshots.slice(-HISTORY_MAX_SNAPSHOTS);
  const retainedChanged = snapshots.length !== document.snapshots.length;
  return {
    document: { schemaVersion: HISTORY_SCHEMA_VERSION, snapshots },
    wrote: shouldAppend || retainedChanged,
    clockSkew: false,
  };
}

function factIn(
  snapshot: HistorySnapshot,
  providerName: string,
  scope: string,
): HistoryFact | undefined {
  const provider = snapshot.providers.find(
    (item) => item.provider === providerName,
  );
  if (!provider || provider.dataHealth !== "current" || !provider.authEligible)
    return undefined;
  return provider.facts.find((fact) => fact.scope === scope);
}

function evidence(
  kind: HistoryEvidenceKind,
  provider: HistoryProviderSnapshot,
  fact: HistoryFact,
  amount?: number,
  remainingSeries?: number[],
): HistoryEvidence {
  return {
    kind,
    provider: provider.provider,
    scope: fact.scope,
    ...(fact.limit ? { limit: fact.limit } : {}),
    ...(amount === undefined ? {} : { amount: Math.round(amount) }),
    ...(remainingSeries ? { remainingSeries } : {}),
  };
}

function resetEvidence(previous: HistoryFact, current: HistoryFact): boolean {
  return (
    !!previous.resetAt &&
    !!current.resetAt &&
    previous.resetAt !== current.resetAt &&
    current.remaining - previous.remaining >= MEANINGFUL_REMAINING_DROP
  );
}

const PACE_RANK: Record<HistoryPaceStatus, number> = {
  behind: 0,
  on_pace: 1,
  mixed: 2,
  ahead: 3,
};

function paceChange(
  previous: HistoryPaceFact | undefined,
  current: HistoryPaceFact | undefined,
): "worse" | "better" | undefined {
  if (!previous || !current) return undefined;
  if (PACE_RANK[current.state] > PACE_RANK[previous.state]) return "worse";
  if (PACE_RANK[current.state] < PACE_RANK[previous.state]) return "better";
  if (previous.reserve === undefined || current.reserve === undefined)
    return undefined;
  const change = current.reserve - previous.reserve;
  if (change <= -MEANINGFUL_RESERVE_CHANGE) return "worse";
  if (change >= MEANINGFUL_RESERVE_CHANGE) return "better";
  return undefined;
}

function establishedProjection(runway?: HistoryRunwayFact): number | undefined {
  if (
    runway?.state !== "projected_exhaustion" ||
    runway.confidence !== "established" ||
    !runway.projectedAt
  )
    return undefined;
  const parsed = Date.parse(runway.projectedAt);
  return Number.isFinite(parsed) ? parsed : undefined;
}

function projectionChange(
  previous: HistoryRunwayFact | undefined,
  current: HistoryRunwayFact | undefined,
): { direction: "earlier" | "later"; amount: number } | undefined {
  if (!previous || !current) return undefined;
  if (
    previous.state === "through_reset" &&
    current.state === "projected_exhaustion" &&
    current.confidence === "established"
  )
    return { direction: "earlier", amount: 0 };
  if (
    previous.state === "projected_exhaustion" &&
    previous.confidence === "established" &&
    current.state === "through_reset"
  )
    return { direction: "later", amount: 0 };
  const before = establishedProjection(previous);
  const after = establishedProjection(current);
  if (before === undefined || after === undefined) return undefined;
  const difference = after - before;
  if (Math.abs(difference) < MEANINGFUL_PROJECTION_CHANGE_MS) return undefined;
  return {
    direction: difference < 0 ? "earlier" : "later",
    amount: Math.abs(difference) / 1000,
  };
}

interface RankedEvidence {
  rank: number;
  evidence: HistoryEvidence;
}

function evidenceForFact(
  snapshots: HistorySnapshot[],
  provider: HistoryProviderSnapshot,
  current: HistoryFact,
): RankedEvidence | undefined {
  const previousSnapshot = snapshots.at(-2);
  const previous = previousSnapshot
    ? factIn(previousSnapshot, provider.provider, current.scope)
    : undefined;
  if (!previous) return undefined;
  if (resetEvidence(previous, current))
    return { rank: 0, evidence: evidence("reset", provider, current) };
  if (previous.resetAt !== current.resetAt) return undefined;

  const pace = paceChange(previous.pace, current.pace);
  if (pace === "worse")
    return { rank: 1, evidence: evidence("pace_worse", provider, current) };

  const projection = projectionChange(previous.runway, current.runway);
  if (projection?.direction === "earlier")
    return {
      rank: 2,
      evidence: evidence(
        "projection_earlier",
        provider,
        current,
        projection.amount,
      ),
    };

  const drop = previous.remaining - current.remaining;
  if (drop >= MEANINGFUL_REMAINING_DROP)
    return {
      rank: 3,
      evidence: evidence("remaining_drop", provider, current, drop),
    };

  if (pace === "better")
    return { rank: 4, evidence: evidence("pace_better", provider, current) };
  if (projection?.direction === "later")
    return {
      rank: 5,
      evidence: evidence(
        "projection_later",
        provider,
        current,
        projection.amount,
      ),
    };

  const series: number[] = [];
  for (
    let index = snapshots.length - 1;
    index >= 0 && series.length < 6;
    index--
  ) {
    const point = factIn(snapshots[index]!, provider.provider, current.scope);
    if (!point || point.resetAt !== current.resetAt) break;
    series.unshift(Math.round(point.remaining));
  }
  if (series.length >= 2)
    return {
      rank: 6,
      evidence: evidence("series", provider, current, undefined, series),
    };
  return undefined;
}

export function historyView(
  document: HistoryDocument,
  availability: HistoryAvailability = "ready",
): HistoryView {
  const current = document.snapshots.at(-1);
  if (!current)
    return {
      availability: availability === "ready" ? "first_run" : availability,
    };
  const candidates: RankedEvidence[] = [];
  for (const provider of current.providers) {
    if (provider.dataHealth !== "current" || !provider.authEligible) continue;
    for (const fact of provider.facts) {
      const candidate = evidenceForFact(document.snapshots, provider, fact);
      if (candidate) candidates.push(candidate);
    }
  }
  const selected = candidates.toSorted(
    (left, right) => left.rank - right.rank,
  )[0];
  return {
    availability:
      availability === "ready" && document.snapshots.length < 2
        ? "first_run"
        : availability,
    ...(selected ? { evidence: selected.evidence } : {}),
  };
}

export function historyPathFromEnvironment(
  env: NodeJS.ProcessEnv = process.env,
  home = homedir(),
): string {
  if (env.HERDR_QUOTA_HISTORY_FILE) return env.HERDR_QUOTA_HISTORY_FILE;
  const stateRoot = env.XDG_STATE_HOME || join(home, ".local", "state");
  return join(stateRoot, "herdr-quota", "history-v1.json");
}

type LoadResult =
  | { kind: "ready" | "missing" | "corrupt"; document: HistoryDocument }
  | { kind: "incompatible" }
  | { kind: "unavailable" };

async function loadHistory(
  path: string,
  operations: HistoryFileOperations,
): Promise<LoadResult> {
  let raw: string;
  try {
    raw = await operations.readFile(path, "utf8");
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT")
      return {
        kind: "missing",
        document: { schemaVersion: HISTORY_SCHEMA_VERSION, snapshots: [] },
      };
    return { kind: "unavailable" };
  }
  try {
    return { kind: "ready", document: parseHistoryDocument(JSON.parse(raw)) };
  } catch (error) {
    if (error instanceof Error && error.message === "history_incompatible")
      return { kind: "incompatible" };
    return {
      kind: "corrupt",
      document: { schemaVersion: HISTORY_SCHEMA_VERSION, snapshots: [] },
    };
  }
}

export async function writeHistoryDocumentAtomic(
  path: string,
  document: HistoryDocument,
  operations: HistoryFileOperations = FILE_OPERATIONS,
): Promise<void> {
  await operations.mkdir(dirname(path), { recursive: true, mode: 0o700 });
  const temporary = `${path}.${process.pid}.${randomUUID()}.tmp`;
  try {
    await operations.writeFile(temporary, `${JSON.stringify(document)}\n`, {
      encoding: "utf8",
      mode: 0o600,
      flag: "wx",
    });
    await operations.rename(temporary, path);
  } catch (error) {
    await operations.unlink(temporary).catch(() => undefined);
    throw error;
  }
}

export class LocalHistory {
  constructor(
    readonly path = historyPathFromEnvironment(),
    private readonly operations: HistoryFileOperations = FILE_OPERATIONS,
  ) {}

  async record(report: QuotaReport, now = new Date()): Promise<HistoryView> {
    const snapshot = normalizeHistorySnapshot(report, now);
    if (!snapshot) return { availability: "no_usable_data" };
    const loaded = await loadHistory(this.path, this.operations);
    if (loaded.kind === "incompatible") return { availability: "incompatible" };
    if (loaded.kind === "unavailable") return { availability: "unavailable" };

    const update = updateHistoryDocument(loaded.document, snapshot);
    if (update.wrote) {
      try {
        await writeHistoryDocumentAtomic(
          this.path,
          update.document,
          this.operations,
        );
      } catch {
        return { availability: "unavailable" };
      }
    }
    const availability: HistoryAvailability = update.clockSkew
      ? "clock_skew"
      : loaded.kind === "corrupt"
        ? "recovered"
        : "ready";
    return historyView(update.document, availability);
  }
}
