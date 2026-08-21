import { randomUUID } from "node:crypto";
import { constants } from "node:fs";
import { lstat, mkdir, open, rename, unlink } from "node:fs/promises";
import { homedir } from "node:os";
import { dirname, join } from "node:path";
import type {
  HistoryDocument,
  HistoryFact,
  HistoryProviderSnapshot,
  HistorySnapshot,
} from "./history.js";
import type {
  DashboardSettings,
  RemainingThreshold,
  SupportedProvider,
} from "./settings.js";
import type {
  TransitionAvailability,
  TransitionDisplayEvent,
  TransitionKind,
  TransitionView,
} from "./types.js";

export const TRANSITION_SCHEMA_VERSION = 1;
export const TRANSITION_MAX_EVENTS = 256;
export const TRANSITION_MAX_AGE_MS = 30 * 24 * 60 * 60_000;
export const TRANSITION_CLOCK_SKEW_MS = 5 * 60_000;

const PROVIDERS = ["Claude", "OpenAI Codex", "Cursor", "Kimi"] as const;
type TransitionProvider = (typeof PROVIDERS)[number];
type PersistedTransitionKind =
  "threshold_baseline" | "forecast_baseline" | TransitionKind;

export interface TransitionPolicy {
  remainingThreshold: RemainingThreshold;
  forecastBeforeReset: boolean;
}

export interface TransitionEvent {
  provider: TransitionProvider;
  scope: string;
  limit?: string;
  cycle: string;
  policy: TransitionPolicy;
  kind: PersistedTransitionKind;
  occurredAt: string;
  acknowledgedAt?: string;
}

export interface TransitionDocument {
  schemaVersion: 1;
  events: TransitionEvent[];
}

interface FileStat {
  isFile(): boolean;
  isSymbolicLink(): boolean;
  mode: number;
  uid: number;
}

export interface TransitionFileHandle {
  stat(): Promise<{ isFile(): boolean }>;
  readFile(options: { encoding: "utf8" }): Promise<string>;
  writeFile(value: string, options: { encoding: "utf8" }): Promise<unknown>;
  sync(): Promise<unknown>;
  close(): Promise<unknown>;
}

export interface TransitionFileOperations {
  lstat(path: string): Promise<FileStat>;
  mkdir(
    path: string,
    options: { recursive: true; mode: number },
  ): Promise<unknown>;
  open(
    path: string,
    flags: number,
    mode?: number,
  ): Promise<TransitionFileHandle>;
  rename(from: string, to: string): Promise<unknown>;
  unlink(path: string): Promise<unknown>;
}

const FILE_OPERATIONS: TransitionFileOperations = {
  lstat: (path) => lstat(path),
  mkdir: (path, options) => mkdir(path, options),
  open: (path, flags, mode) => open(path, flags, mode),
  rename: (from, to) => rename(from, to),
  unlink: (path) => unlink(path),
};

class TransitionDocumentError extends Error {
  constructor(readonly kind: "corrupt" | "incompatible" | "unsafe") {
    super(`transitions_${kind}`);
  }
}

function errorCode(error: unknown): string | undefined {
  return (error as NodeJS.ErrnoException | undefined)?.code;
}

function isObject(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function exactKeys(value: Record<string, unknown>, keys: string[]): boolean {
  const expected = keys.toSorted();
  const actual = Object.keys(value).toSorted();
  return (
    actual.length === expected.length &&
    actual.every((key, index) => key === expected[index])
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

function threshold(value: unknown): value is RemainingThreshold {
  return value === "off" || value === 25 || value === 10 || value === 5;
}

function parsePolicy(value: unknown): TransitionPolicy | undefined {
  if (
    !isObject(value) ||
    !exactKeys(value, ["remainingThreshold", "forecastBeforeReset"]) ||
    !threshold(value.remainingThreshold) ||
    typeof value.forecastBeforeReset !== "boolean"
  )
    return undefined;
  return {
    remainingThreshold: value.remainingThreshold,
    forecastBeforeReset: value.forecastBeforeReset,
  };
}

const KINDS = new Set<PersistedTransitionKind>([
  "threshold_baseline",
  "forecast_baseline",
  "threshold_enter",
  "threshold_recovery",
  "forecast_enter",
  "forecast_recovery",
]);

function parseEvent(value: unknown): TransitionEvent | undefined {
  if (!isObject(value)) return undefined;
  const keys = [
    "provider",
    "scope",
    ...(value.limit === undefined ? [] : ["limit"]),
    "cycle",
    "policy",
    "kind",
    "occurredAt",
    ...(value.acknowledgedAt === undefined ? [] : ["acknowledgedAt"]),
  ];
  const policy = parsePolicy(value.policy);
  if (
    !exactKeys(value, keys) ||
    !PROVIDERS.includes(value.provider as TransitionProvider) ||
    !safeIdentity(value.scope) ||
    (value.limit !== undefined && !safeIdentity(value.limit)) ||
    (value.cycle !== "unbounded" && !iso(value.cycle)) ||
    !policy ||
    !KINDS.has(value.kind as PersistedTransitionKind) ||
    !iso(value.occurredAt) ||
    (value.acknowledgedAt !== undefined && !iso(value.acknowledgedAt)) ||
    (typeof value.acknowledgedAt === "string" &&
      Date.parse(value.acknowledgedAt) < Date.parse(value.occurredAt))
  )
    return undefined;
  return {
    provider: value.provider as TransitionProvider,
    scope: value.scope,
    ...(value.limit === undefined ? {} : { limit: value.limit }),
    cycle: value.cycle,
    policy,
    kind: value.kind as PersistedTransitionKind,
    occurredAt: value.occurredAt,
    ...(value.acknowledgedAt === undefined
      ? {}
      : { acknowledgedAt: value.acknowledgedAt }),
  };
}

export function parseTransitionDocument(value: unknown): TransitionDocument {
  if (!isObject(value)) throw new TransitionDocumentError("corrupt");
  if (value.schemaVersion !== TRANSITION_SCHEMA_VERSION) {
    if (typeof value.schemaVersion === "number")
      throw new TransitionDocumentError("incompatible");
    throw new TransitionDocumentError("corrupt");
  }
  if (
    !exactKeys(value, ["schemaVersion", "events"]) ||
    !Array.isArray(value.events) ||
    value.events.length > TRANSITION_MAX_EVENTS
  )
    throw new TransitionDocumentError("corrupt");
  const events = value.events.map(parseEvent);
  if (events.some((event) => event === undefined))
    throw new TransitionDocumentError("corrupt");
  for (let index = 1; index < events.length; index++) {
    if (
      Date.parse(events[index - 1]!.occurredAt) >
      Date.parse(events[index]!.occurredAt)
    )
      throw new TransitionDocumentError("corrupt");
  }
  return {
    schemaVersion: TRANSITION_SCHEMA_VERSION,
    events: events as TransitionEvent[],
  };
}

export function transitionPolicyEnabled(settings: DashboardSettings): boolean {
  return settings.remainingThreshold !== "off" || settings.forecastBeforeReset;
}

function policyFor(settings: DashboardSettings): TransitionPolicy {
  return {
    remainingThreshold: settings.remainingThreshold,
    forecastBeforeReset: settings.forecastBeforeReset,
  };
}

function providerId(provider: TransitionProvider): SupportedProvider {
  if (provider === "OpenAI Codex") return "codex";
  return provider.toLowerCase() as SupportedProvider;
}

function visible(provider: TransitionProvider, settings: DashboardSettings) {
  return !settings.hiddenProviders.includes(providerId(provider));
}

interface FactIdentity {
  provider: TransitionProvider;
  scope: string;
  limit?: string;
  cycle: string;
}

interface CurrentFact {
  identity: FactIdentity;
  fact: HistoryFact;
  capturedAt: string;
}

function cycleIdentity(resetAt: string | undefined): string {
  if (!resetAt) return "unbounded";
  const parsed = Date.parse(resetAt);
  return new Date(Math.round(parsed / 60_000) * 60_000).toISOString();
}

function identityFor(
  provider: HistoryProviderSnapshot,
  fact: HistoryFact,
): FactIdentity {
  return {
    provider: provider.provider,
    scope: fact.scope,
    ...(fact.limit ? { limit: fact.limit } : {}),
    cycle: cycleIdentity(fact.resetAt),
  };
}

function sameIdentity(event: TransitionEvent, identity: FactIdentity): boolean {
  return (
    event.provider === identity.provider &&
    event.scope === identity.scope &&
    event.limit === identity.limit &&
    event.cycle === identity.cycle
  );
}

function channelPolicyMatches(
  event: TransitionEvent,
  settings: DashboardSettings,
  channel: "threshold" | "forecast",
): boolean {
  return channel === "threshold"
    ? event.policy.remainingThreshold === settings.remainingThreshold
    : event.policy.forecastBeforeReset === settings.forecastBeforeReset;
}

function currentFacts(
  history: HistoryDocument,
  settings: DashboardSettings,
  providers?: readonly SupportedProvider[],
): CurrentFact[] {
  const current = history.snapshots.at(-1);
  if (!current) return [];
  const facts: CurrentFact[] = [];
  for (const provider of current.providers) {
    if (
      provider.dataHealth !== "current" ||
      !provider.authEligible ||
      (!visible(provider.provider, settings) &&
        !providers?.includes(providerId(provider.provider))) ||
      (providers && !providers.includes(providerId(provider.provider)))
    )
      continue;
    for (const fact of provider.facts) {
      facts.push({
        identity: identityFor(provider, fact),
        fact,
        capturedAt: current.capturedAt,
      });
    }
  }
  return facts;
}

function eventFor(
  current: CurrentFact,
  policy: TransitionPolicy,
  kind: PersistedTransitionKind,
  occurredAt = current.capturedAt,
): TransitionEvent {
  return {
    ...current.identity,
    policy,
    kind,
    occurredAt,
  };
}

function latestChannelEvent(
  document: TransitionDocument,
  identity: FactIdentity,
  settings: DashboardSettings,
  channel: "threshold" | "forecast",
): TransitionEvent | undefined {
  const kinds =
    channel === "threshold"
      ? new Set<PersistedTransitionKind>([
          "threshold_baseline",
          "threshold_enter",
          "threshold_recovery",
        ])
      : new Set<PersistedTransitionKind>([
          "forecast_baseline",
          "forecast_enter",
          "forecast_recovery",
        ]);
  return document.events
    .filter(
      (event) =>
        sameIdentity(event, identity) &&
        channelPolicyMatches(event, settings, channel) &&
        kinds.has(event.kind),
    )
    .at(-1);
}

function latestBaselineAt(
  document: TransitionDocument,
  identity: FactIdentity,
  settings: DashboardSettings,
  channel: "threshold" | "forecast",
): number | undefined {
  const kind =
    channel === "threshold" ? "threshold_baseline" : "forecast_baseline";
  const baseline = document.events
    .filter(
      (event) =>
        event.kind === kind &&
        sameIdentity(event, identity) &&
        channelPolicyMatches(event, settings, channel),
    )
    .at(-1);
  return baseline ? Date.parse(baseline.occurredAt) : undefined;
}

function factIn(
  snapshot: HistorySnapshot,
  identity: FactIdentity,
): HistoryFact | undefined {
  const provider = snapshot.providers.find(
    (item) => item.provider === identity.provider,
  );
  if (!provider || provider.dataHealth !== "current" || !provider.authEligible)
    return undefined;
  return provider.facts.find(
    (fact) =>
      fact.scope === identity.scope &&
      fact.limit === identity.limit &&
      cycleIdentity(fact.resetAt) === identity.cycle,
  );
}

function previousFact(
  history: HistoryDocument,
  identity: FactIdentity,
  baselineAt: number | undefined,
  usable: (fact: HistoryFact) => boolean,
): HistoryFact | undefined {
  for (let index = history.snapshots.length - 2; index >= 0; index--) {
    const snapshot = history.snapshots[index]!;
    if (
      baselineAt !== undefined &&
      Date.parse(snapshot.capturedAt) < baselineAt
    )
      return undefined;
    const fact = factIn(snapshot, identity);
    if (fact && usable(fact)) return fact;
  }
  return undefined;
}

type ForecastClass = "safe" | "risky";

function forecastClass(fact: HistoryFact): ForecastClass | undefined {
  if (fact.runway?.state === "through_reset") return "safe";
  if (fact.runway?.state === "exhausted_now") return "risky";
  if (
    fact.runway?.state === "projected_exhaustion" &&
    fact.runway.confidence === "established"
  )
    return "risky";
  return undefined;
}

export interface TransitionEvaluation {
  document: TransitionDocument;
  generated: TransitionEvent[];
  clockSkew: boolean;
}

export function baselineTransitions(
  document: TransitionDocument,
  history: HistoryDocument,
  settings: DashboardSettings,
  now = new Date(),
  channels: readonly ("threshold" | "forecast")[] = ["threshold", "forecast"],
  providers?: readonly SupportedProvider[],
): TransitionEvaluation {
  void now;
  if (!transitionPolicyEnabled(settings))
    return { document, generated: [], clockSkew: false };
  const policy = policyFor(settings);
  const generated: TransitionEvent[] = [];
  for (const current of currentFacts(history, settings, providers)) {
    if (channels.includes("threshold") && settings.remainingThreshold !== "off")
      generated.push(eventFor(current, policy, "threshold_baseline"));
    if (channels.includes("forecast") && settings.forecastBeforeReset)
      generated.push(eventFor(current, policy, "forecast_baseline"));
  }
  return appendTransitionEvents(document, generated);
}

export function evaluateTransitions(
  document: TransitionDocument,
  history: HistoryDocument,
  settings: DashboardSettings,
): TransitionEvaluation {
  if (!transitionPolicyEnabled(settings))
    return { document, generated: [], clockSkew: false };
  const policy = policyFor(settings);
  const generated: TransitionEvent[] = [];
  for (const current of currentFacts(history, settings)) {
    if (settings.remainingThreshold !== "off") {
      const latest = latestChannelEvent(
        document,
        current.identity,
        settings,
        "threshold",
      );
      if (!latest) {
        generated.push(eventFor(current, policy, "threshold_baseline"));
      } else {
        const active = latest.kind === "threshold_enter";
        const previous = previousFact(
          history,
          current.identity,
          latestBaselineAt(document, current.identity, settings, "threshold"),
          () => true,
        );
        if (
          !active &&
          previous &&
          previous.remaining > settings.remainingThreshold &&
          current.fact.remaining <= settings.remainingThreshold
        ) {
          generated.push(eventFor(current, policy, "threshold_enter"));
        } else if (
          active &&
          current.fact.remaining > settings.remainingThreshold
        ) {
          generated.push(eventFor(current, policy, "threshold_recovery"));
        }
      }
    }

    if (settings.forecastBeforeReset) {
      const latest = latestChannelEvent(
        document,
        current.identity,
        settings,
        "forecast",
      );
      if (!latest) {
        generated.push(eventFor(current, policy, "forecast_baseline"));
      } else {
        const active = latest.kind === "forecast_enter";
        const currentForecast = forecastClass(current.fact);
        const previous = previousFact(
          history,
          current.identity,
          latestBaselineAt(document, current.identity, settings, "forecast"),
          (fact) => forecastClass(fact) !== undefined,
        );
        const previousForecast = previous ? forecastClass(previous) : undefined;
        if (
          !active &&
          previousForecast === "safe" &&
          currentForecast === "risky"
        ) {
          generated.push(eventFor(current, policy, "forecast_enter"));
        } else if (active && currentForecast === "safe") {
          generated.push(eventFor(current, policy, "forecast_recovery"));
        }
      }
    }
  }
  return appendTransitionEvents(document, generated);
}

export function appendTransitionEvents(
  document: TransitionDocument,
  generated: TransitionEvent[],
): TransitionEvaluation {
  if (!generated.length) return { document, generated: [], clockSkew: false };
  const previousAt = document.events.at(-1)?.occurredAt;
  const firstAt = Date.parse(generated[0]!.occurredAt);
  let events = document.events;
  const clockSkew =
    previousAt && firstAt < Date.parse(previousAt) - TRANSITION_CLOCK_SKEW_MS;
  if (clockSkew) {
    events = [];
  }
  events = [...events, ...generated].toSorted(
    (left, right) => Date.parse(left.occurredAt) - Date.parse(right.occurredAt),
  );
  const newest = Math.max(
    ...events.map((event) => Date.parse(event.occurredAt)),
  );
  const cutoff = newest - TRANSITION_MAX_AGE_MS;
  events = events
    .filter((event) => Date.parse(event.occurredAt) >= cutoff)
    .slice(-TRANSITION_MAX_EVENTS);
  return {
    document: { schemaVersion: TRANSITION_SCHEMA_VERSION, events },
    generated,
    clockSkew: !!clockSkew,
  };
}

function displayFact(
  history: HistoryDocument,
  event: TransitionEvent,
): HistoryFact | undefined {
  const snapshot = history.snapshots.find(
    (item) => item.capturedAt === event.occurredAt,
  );
  return snapshot ? factIn(snapshot, event) : undefined;
}

function eventChannel(kind: PersistedTransitionKind): "threshold" | "forecast" {
  return kind.startsWith("threshold") ? "threshold" : "forecast";
}

function afterLatestBaseline(
  document: TransitionDocument,
  event: TransitionEvent,
  settings: DashboardSettings,
): boolean {
  const baselineAt = latestBaselineAt(
    document,
    event,
    settings,
    eventChannel(event.kind),
  );
  return baselineAt === undefined || Date.parse(event.occurredAt) > baselineAt;
}

function displayEvents(
  document: TransitionDocument,
  history: HistoryDocument,
  settings: DashboardSettings,
): TransitionDisplayEvent[] {
  return document.events
    .filter(
      (event) =>
        event.kind !== "threshold_baseline" &&
        event.kind !== "forecast_baseline" &&
        event.acknowledgedAt === undefined &&
        visible(event.provider, settings) &&
        channelPolicyMatches(event, settings, eventChannel(event.kind)) &&
        afterLatestBaseline(document, event, settings),
    )
    .map((event) => {
      const fact = displayFact(history, event);
      return {
        kind: event.kind as TransitionKind,
        provider: event.provider,
        scope: event.scope,
        ...(event.limit ? { limit: event.limit } : {}),
        threshold: event.policy.remainingThreshold,
        occurredAt: event.occurredAt,
        ...(fact ? { remaining: fact.remaining } : {}),
      };
    })
    .toReversed();
}

export function transitionView(
  document: TransitionDocument,
  history: HistoryDocument,
  settings: DashboardSettings,
  availability: TransitionAvailability = "ready",
): TransitionView {
  return {
    availability,
    events: transitionPolicyEnabled(settings)
      ? displayEvents(document, history, settings)
      : [],
  };
}

export function transitionPathFromEnvironment(
  environment: NodeJS.ProcessEnv = process.env,
  home = homedir(),
): string {
  if (environment.HERDR_QUOTA_TRANSITION_FILE)
    return environment.HERDR_QUOTA_TRANSITION_FILE;
  const stateRoot = environment.XDG_STATE_HOME || join(home, ".local", "state");
  return join(stateRoot, "herdr-quota", "transitions-v1.json");
}

async function regularTarget(
  path: string,
  operations: TransitionFileOperations,
): Promise<boolean> {
  try {
    const stat = await operations.lstat(path);
    if (stat.isSymbolicLink() || !stat.isFile())
      throw new TransitionDocumentError("unsafe");
    return true;
  } catch (error) {
    if (errorCode(error) === "ENOENT") return false;
    throw error;
  }
}

async function replaceableTarget(
  path: string,
  operations: TransitionFileOperations,
): Promise<boolean> {
  let stat: FileStat;
  try {
    stat = await operations.lstat(path);
  } catch (error) {
    if (errorCode(error) === "ENOENT") return false;
    throw error;
  }
  if (stat.isSymbolicLink() || !stat.isFile())
    throw new TransitionDocumentError("unsafe");
  const currentUid = process.getuid?.();
  if (
    (currentUid !== undefined && stat.uid !== currentUid) ||
    (stat.mode & 0o200) === 0
  )
    throw new TransitionDocumentError("unsafe");
  const text = await readRegularFile(path, operations);
  try {
    parseTransitionDocument(JSON.parse(text as string) as unknown);
  } catch (error) {
    if (
      error instanceof TransitionDocumentError &&
      error.kind === "incompatible"
    )
      throw error;
  }
  return true;
}

async function readRegularFile(
  path: string,
  operations: TransitionFileOperations,
): Promise<string | undefined> {
  if (!(await regularTarget(path, operations))) return undefined;
  const noFollow = constants.O_NOFOLLOW ?? 0;
  const handle = await operations.open(path, constants.O_RDONLY | noFollow);
  try {
    if (!(await handle.stat()).isFile())
      throw new TransitionDocumentError("unsafe");
    return await handle.readFile({ encoding: "utf8" });
  } finally {
    await handle.close();
  }
}

type LoadResult =
  | {
      kind: "ready" | "missing" | "corrupt";
      document: TransitionDocument;
    }
  | { kind: "incompatible" }
  | { kind: "unavailable" };

async function loadDocument(
  path: string,
  operations: TransitionFileOperations,
): Promise<LoadResult> {
  let text: string | undefined;
  try {
    text = await readRegularFile(path, operations);
    if (text === undefined)
      return {
        kind: "missing",
        document: { schemaVersion: TRANSITION_SCHEMA_VERSION, events: [] },
      };
    return {
      kind: "ready",
      document: parseTransitionDocument(JSON.parse(text) as unknown),
    };
  } catch (error) {
    if (
      error instanceof TransitionDocumentError &&
      error.kind === "incompatible"
    )
      return { kind: "incompatible" };
    if (
      error instanceof SyntaxError ||
      (error instanceof TransitionDocumentError && error.kind === "corrupt")
    )
      return {
        kind: "corrupt",
        document: { schemaVersion: TRANSITION_SCHEMA_VERSION, events: [] },
      };
    return { kind: "unavailable" };
  }
}

function serializedDocument(document: TransitionDocument): string {
  return `${JSON.stringify(parseTransitionDocument(document))}\n`;
}

export async function writeTransitionDocumentAtomic(
  path: string,
  document: TransitionDocument,
  operations: TransitionFileOperations = FILE_OPERATIONS,
): Promise<void> {
  await operations.mkdir(dirname(path), { recursive: true, mode: 0o700 });
  await replaceableTarget(path, operations);
  const temporary = `${path}.${process.pid}.${randomUUID()}.tmp`;
  const noFollow = constants.O_NOFOLLOW ?? 0;
  let handle: TransitionFileHandle | undefined;
  let renamed = false;
  try {
    handle = await operations.open(
      temporary,
      constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL | noFollow,
      0o600,
    );
    await handle.writeFile(serializedDocument(document), { encoding: "utf8" });
    await handle.sync();
    await handle.close();
    handle = undefined;
    await replaceableTarget(path, operations);
    await operations.rename(temporary, path);
    renamed = true;
  } finally {
    await handle?.close().catch(() => undefined);
    if (!renamed) await operations.unlink(temporary).catch(() => undefined);
  }
}

function availabilityFor(load: LoadResult): TransitionAvailability {
  if (load.kind === "missing") return "first_run";
  if (load.kind === "corrupt") return "recovered";
  if (load.kind === "incompatible") return "incompatible";
  if (load.kind === "unavailable") return "unavailable";
  return "ready";
}

export class LocalTransitions {
  constructor(
    readonly path = transitionPathFromEnvironment(),
    private readonly operations: TransitionFileOperations = FILE_OPERATIONS,
  ) {}

  async loadView(
    history: HistoryDocument,
    settings: DashboardSettings,
  ): Promise<TransitionView> {
    const loaded = await loadDocument(this.path, this.operations);
    if (loaded.kind === "incompatible" || loaded.kind === "unavailable")
      return { availability: availabilityFor(loaded), events: [] };
    return transitionView(
      loaded.document,
      history,
      settings,
      availabilityFor(loaded),
    );
  }

  async evaluate(
    history: HistoryDocument,
    settings: DashboardSettings,
  ): Promise<TransitionView> {
    const loaded = await loadDocument(this.path, this.operations);
    if (loaded.kind === "incompatible" || loaded.kind === "unavailable")
      return { availability: availabilityFor(loaded), events: [] };
    const update = evaluateTransitions(loaded.document, history, settings);
    if (update.generated.length) {
      try {
        await writeTransitionDocumentAtomic(
          this.path,
          update.document,
          this.operations,
        );
      } catch {
        return transitionView(
          loaded.document,
          history,
          settings,
          "unavailable",
        );
      }
    }
    return transitionView(
      update.document,
      history,
      settings,
      update.clockSkew ? "clock_skew" : availabilityFor(loaded),
    );
  }

  async baseline(
    history: HistoryDocument,
    settings: DashboardSettings,
    now = new Date(),
    channels?: readonly ("threshold" | "forecast")[],
    providers?: readonly SupportedProvider[],
  ): Promise<TransitionView> {
    const loaded = await loadDocument(this.path, this.operations);
    if (loaded.kind === "incompatible" || loaded.kind === "unavailable")
      return { availability: availabilityFor(loaded), events: [] };
    const update = baselineTransitions(
      loaded.document,
      history,
      settings,
      now,
      channels,
      providers,
    );
    if (update.generated.length) {
      try {
        await writeTransitionDocumentAtomic(
          this.path,
          update.document,
          this.operations,
        );
      } catch {
        return transitionView(
          loaded.document,
          history,
          settings,
          "unavailable",
        );
      }
    }
    return transitionView(
      update.document,
      history,
      settings,
      update.clockSkew ? "clock_skew" : "ready",
    );
  }

  async acknowledge(
    history: HistoryDocument,
    settings: DashboardSettings,
    now = new Date(),
  ): Promise<TransitionView> {
    const loaded = await loadDocument(this.path, this.operations);
    if (loaded.kind === "incompatible" || loaded.kind === "unavailable")
      return { availability: availabilityFor(loaded), events: [] };
    const visibleEvents = new Set(
      loaded.document.events.filter(
        (event) =>
          event.kind !== "threshold_baseline" &&
          event.kind !== "forecast_baseline" &&
          event.acknowledgedAt === undefined &&
          visible(event.provider, settings) &&
          channelPolicyMatches(event, settings, eventChannel(event.kind)) &&
          afterLatestBaseline(loaded.document, event, settings),
      ),
    );
    if (!visibleEvents.size)
      return transitionView(loaded.document, history, settings);
    const acknowledgedAt = new Date(
      Math.max(
        now.getTime(),
        ...[...visibleEvents].map((event) => Date.parse(event.occurredAt)),
      ),
    ).toISOString();
    const document: TransitionDocument = {
      schemaVersion: TRANSITION_SCHEMA_VERSION,
      events: loaded.document.events.map((event) =>
        visibleEvents.has(event) ? { ...event, acknowledgedAt } : event,
      ),
    };
    try {
      await writeTransitionDocumentAtomic(this.path, document, this.operations);
    } catch {
      return transitionView(loaded.document, history, settings, "unavailable");
    }
    return transitionView(document, history, settings);
  }

  async clear(): Promise<TransitionView> {
    const loaded = await loadDocument(this.path, this.operations);
    if (loaded.kind === "incompatible" || loaded.kind === "unavailable")
      return { availability: availabilityFor(loaded), events: [] };
    if (loaded.kind === "missing")
      return { availability: "first_run", events: [] };
    try {
      await replaceableTarget(this.path, this.operations);
      await this.operations.unlink(this.path);
      return { availability: "first_run", events: [] };
    } catch {
      return {
        availability: "unavailable",
        events: [],
      };
    }
  }
}
