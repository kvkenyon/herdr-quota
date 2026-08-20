import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import {
  HISTORY_EQUIVALENT_INTERVAL_MS,
  HISTORY_MAX_AGE_MS,
  HISTORY_MAX_SNAPSHOTS,
  HISTORY_SCHEMA_VERSION,
  LocalHistory,
  historyView,
  normalizeHistorySnapshot,
  parseHistoryDocument,
  updateHistoryDocument,
  writeHistoryDocumentAtomic,
} from "../dist/history.js";
import { adaptQuotaResponse } from "../dist/schema.js";

const timelineFixture = JSON.parse(
  await readFile(
    new URL("fixtures/history-timelines.json", import.meta.url),
    "utf8",
  ),
);
const completeFixture = JSON.parse(
  await readFile(new URL("fixtures/complete.json", import.meta.url), "utf8"),
);

function emptyDocument() {
  return { schemaVersion: HISTORY_SCHEMA_VERSION, snapshots: [] };
}

function providerFact(point) {
  if (point.present === false) return [];
  if (point.authEligible === false) {
    return [
      {
        provider: timelineFixture.provider,
        dataHealth: "unavailable",
        authEligible: false,
        facts: [],
      },
    ];
  }
  return [
    {
      provider: timelineFixture.provider,
      dataHealth: "current",
      authEligible: true,
      facts: [
        {
          scope: timelineFixture.scope,
          limit: timelineFixture.limit,
          remaining: point.remaining,
          resetAt: point.resetAt ?? timelineFixture.resetAt,
          pace: {
            state: point.pace,
            ...(point.reserve === undefined ? {} : { reserve: point.reserve }),
          },
          runway: {
            state: point.runway,
            ...(point.projection
              ? {
                  projectedAt: point.projection,
                  confidence: "established",
                }
              : {}),
          },
        },
      ],
    },
  ];
}

function timeline(name) {
  return {
    schemaVersion: HISTORY_SCHEMA_VERSION,
    snapshots: timelineFixture.timelines[name].map((point) => ({
      capturedAt: point.at,
      providers: providerFact(point),
    })),
  };
}

function snapshot(at, remaining = 50) {
  return {
    capturedAt: new Date(at).toISOString(),
    providers: [
      {
        provider: "Claude",
        dataHealth: "current",
        authEligible: true,
        facts: [
          {
            scope: "All models",
            limit: "Week",
            remaining,
            resetAt: "2026-09-01T00:00:00.000Z",
            pace: { state: "on_pace", reserve: 4 },
            runway: { state: "through_reset" },
          },
        ],
      },
    ],
  };
}

function report() {
  return adaptQuotaResponse(JSON.parse(JSON.stringify(completeFixture)));
}

test("normalizes only the finite privacy allow-list", () => {
  const value = report();
  const claude = value.providers[0];
  claude.label = "alice@example.com";
  claude.source = "/Users/alice/.config/secret-token";
  claude.plan = "Bearer top-secret-value";
  claude.state.reason = "sk-private-token-value";
  claude.state.remedyCommand = "read /home/alice/auth.json";
  claude.windows.find((window) => window.id === "model:fable").label =
    "alice@example.com";
  const codex = value.providers.find(
    (provider) => provider.provider === "codex",
  );
  codex.windows.find((window) => window.id.startsWith("model:")).label =
    "Bearer alice@example.com /Users/alice/auth.json";

  const normalized = normalizeHistorySnapshot(
    value,
    new Date("2026-08-20T12:00:00.000Z"),
  );
  assert.ok(normalized);
  const persisted = JSON.stringify(normalized);
  assert.doesNotMatch(
    persisted,
    /alice|example|Users|home|secret|token|Bearer|auth\.json|remedy|source|plan/i,
  );
  assert.deepEqual(Object.keys(normalized).toSorted(), [
    "capturedAt",
    "providers",
  ]);
  for (const provider of normalized.providers) {
    assert.deepEqual(Object.keys(provider).toSorted(), [
      "authEligible",
      "dataHealth",
      "facts",
      "provider",
    ]);
  }
});

test("stale, auth, error, and unknown-only reports cannot create snapshots", () => {
  for (const state of ["stale", "auth_required", "error", "unavailable"]) {
    const value = report();
    for (const provider of value.providers) {
      provider.state.status = state;
      provider.state.stale = state === "stale";
    }
    assert.equal(normalizeHistorySnapshot(value), undefined, state);
  }

  const unknown = report();
  for (const provider of unknown.providers) {
    provider.state.status = "fresh";
    provider.state.stale = false;
    provider.semanticsStatus = "unknown";
  }
  assert.equal(normalizeHistorySnapshot(unknown), undefined);
});

test("a persisted second usable run produces established change evidence", async () => {
  const operations = memoryOperations(undefined);
  const history = new LocalHistory("/state/history.json", operations);
  const first = await history.record(
    report(),
    new Date("2026-08-20T12:00:00.000Z"),
  );
  assert.equal(first.availability, "first_run");
  assert.equal(first.evidence, undefined);

  const changed = report();
  const claude = changed.providers.find(
    (provider) => provider.provider === "claude",
  );
  claude.effective[0].effectivePercentRemaining -= 17;
  const second = await history.record(
    changed,
    new Date("2026-08-20T12:05:00.000Z"),
  );
  assert.equal(second.evidence?.kind, "remaining_drop");
  assert.equal(second.evidence?.amount, 17);
  assert.equal(
    parseHistoryDocument(JSON.parse(operations.target())).snapshots.length,
    2,
  );
});

test("deduplicates equivalent samples by cadence but records real changes", () => {
  const start = Date.parse("2026-08-20T12:00:00.000Z");
  const first = updateHistoryDocument(emptyDocument(), snapshot(start));
  assert.equal(first.wrote, true);

  const duplicate = updateHistoryDocument(
    first.document,
    snapshot(start + HISTORY_EQUIVALENT_INTERVAL_MS - 1),
  );
  assert.equal(duplicate.wrote, false);
  assert.equal(duplicate.document.snapshots.length, 1);

  const cadence = updateHistoryDocument(
    duplicate.document,
    snapshot(start + HISTORY_EQUIVALENT_INTERVAL_MS),
  );
  assert.equal(cadence.wrote, true);
  assert.equal(cadence.document.snapshots.length, 2);

  const changed = updateHistoryDocument(
    cadence.document,
    snapshot(start + HISTORY_EQUIVALENT_INTERVAL_MS + 1, 49),
  );
  assert.equal(changed.wrote, true);
  assert.equal(changed.document.snapshots.length, 3);
});

test("bounds retention independently by count and age", () => {
  const start = Date.parse("2026-08-01T00:00:00.000Z");
  let document = emptyDocument();
  for (let index = 0; index < HISTORY_MAX_SNAPSHOTS + 20; index++) {
    document = updateHistoryDocument(
      document,
      snapshot(start + index * 60_000, index % 2 ? 49 : 50),
    ).document;
  }
  assert.equal(document.snapshots.length, HISTORY_MAX_SNAPSHOTS);

  const now = Date.parse("2026-10-01T00:00:00.000Z");
  const aged = {
    schemaVersion: HISTORY_SCHEMA_VERSION,
    snapshots: [
      snapshot(now - HISTORY_MAX_AGE_MS - 1),
      snapshot(now - HISTORY_MAX_AGE_MS + 1, 49),
    ],
  };
  const retained = updateHistoryDocument(aged, snapshot(now, 48));
  assert.deepEqual(
    retained.document.snapshots.map((item) => item.capturedAt),
    [aged.snapshots[1].capturedAt, new Date(now).toISOString()],
  );
});

test("timeline fixtures produce only established conservative signals", () => {
  const expected = new Map([
    ["paceWorsening", ["pace_worse", undefined]],
    ["paceImproving", ["pace_better", undefined]],
    ["ordinary", ["series", undefined]],
    ["meaningfulDrop", ["remaining_drop", 17]],
    ["reset", ["series", undefined]],
    ["projectionEarlier", ["projection_earlier", 18_000]],
    ["projectionLater", ["projection_later", 18_000]],
  ]);
  for (const [name, [kind, amount]] of expected) {
    const view = historyView(timeline(name));
    assert.equal(view.evidence?.kind, kind, name);
    assert.equal(view.evidence?.amount, amount, name);
  }

  const resetAtBoundary = {
    ...timeline("reset"),
    snapshots: timeline("reset").snapshots.slice(0, 2),
  };
  assert.equal(historyView(resetAtBoundary).evidence?.kind, "reset");
  assert.deepEqual(
    historyView(timeline("ordinary")).evidence?.remainingSeries,
    [62, 60, 58],
  );
});

test("provider disappearance, auth gaps, and first run never signal", () => {
  for (const name of ["providerGap", "authGap", "firstRun"]) {
    const view = historyView(timeline(name));
    assert.equal(view.evidence, undefined, name);
  }
  assert.equal(historyView(timeline("firstRun")).availability, "first_run");
});

test("reset changes segment the sparkline even without a replenishment signal", () => {
  const value = timeline("reset");
  const view = historyView(value);
  assert.equal(view.evidence?.kind, "series");
  assert.deepEqual(view.evidence.remainingSeries, [100, 98]);
});

test("schema mismatch is preserved and corrupt history recovers safely", async () => {
  const incompatibleText = '{"schemaVersion":99,"snapshots":[]}\n';
  const incompatible = memoryOperations(incompatibleText);
  const incompatibleHistory = new LocalHistory(
    "/state/history.json",
    incompatible,
  );
  assert.deepEqual(
    await incompatibleHistory.record(
      report(),
      new Date("2026-08-20T12:00:00.000Z"),
    ),
    { availability: "incompatible" },
  );
  assert.equal(incompatible.target(), incompatibleText);
  assert.deepEqual(incompatible.events, ["read"]);

  const corrupt = memoryOperations("{truncated");
  const recovered = await new LocalHistory(
    "/state/history.json",
    corrupt,
  ).record(report(), new Date("2026-08-20T12:00:00.000Z"));
  assert.equal(recovered.availability, "recovered");
  assert.equal(
    parseHistoryDocument(JSON.parse(corrupt.target())).snapshots.length,
    1,
  );
});

test("clock rollback restarts a segment instead of connecting future data", async () => {
  const future = {
    schemaVersion: HISTORY_SCHEMA_VERSION,
    snapshots: [snapshot("2026-08-20T13:00:00.000Z")],
  };
  const operations = memoryOperations(`${JSON.stringify(future)}\n`);
  const view = await new LocalHistory("/state/history.json", operations).record(
    report(),
    new Date("2026-08-20T12:00:00.000Z"),
  );
  assert.equal(view.availability, "clock_skew");
  const stored = parseHistoryDocument(JSON.parse(operations.target()));
  assert.equal(stored.snapshots.length, 1);
  assert.equal(stored.snapshots[0].capturedAt, "2026-08-20T12:00:00.000Z");
});

test("atomic writes use a private sibling then rename", async () => {
  const operations = memoryOperations(undefined);
  await writeHistoryDocumentAtomic(
    "/state/history.json",
    { schemaVersion: HISTORY_SCHEMA_VERSION, snapshots: [snapshot(0)] },
    operations,
  );
  assert.deepEqual(operations.events.slice(0, 3), [
    "mkdir 448",
    "write 384 wx",
    "rename",
  ]);
  assert.equal(operations.lastTemporary.endsWith(".tmp"), true);
  assert.equal(
    parseHistoryDocument(JSON.parse(operations.target())).snapshots.length,
    1,
  );
});

test("write interruption keeps the prior document and isolates the live view", async () => {
  const original = `${JSON.stringify({
    schemaVersion: HISTORY_SCHEMA_VERSION,
    snapshots: [snapshot("2026-08-20T11:00:00.000Z")],
  })}\n`;
  const operations = memoryOperations(original, { failRename: true });
  const view = await new LocalHistory("/state/history.json", operations).record(
    report(),
    new Date("2026-08-20T12:00:00.000Z"),
  );
  assert.deepEqual(view, { availability: "unavailable" });
  assert.equal(operations.target(), original);
  assert.equal(operations.events.at(-1), "unlink");
});

test("permission failure and unusable reports perform no history write", async () => {
  const denied = memoryOperations(undefined, { failRead: "EACCES" });
  assert.deepEqual(
    await new LocalHistory("/state/history.json", denied).record(report()),
    { availability: "unavailable" },
  );
  assert.deepEqual(denied.events, ["read"]);

  const authOnly = report();
  for (const provider of authOnly.providers)
    provider.state.status = "auth_required";
  const untouched = memoryOperations(undefined);
  assert.deepEqual(
    await new LocalHistory("/state/history.json", untouched).record(authOnly),
    { availability: "no_usable_data" },
  );
  assert.deepEqual(untouched.events, []);
});

test("history parser rejects extra private fields", () => {
  const privateDocument = {
    schemaVersion: HISTORY_SCHEMA_VERSION,
    snapshots: [
      {
        ...snapshot(0),
        token: "sk-private-token-value",
      },
    ],
  };
  assert.throws(() => parseHistoryDocument(privateDocument), /history_corrupt/);
});

function memoryOperations(initial, options = {}) {
  let target = initial;
  const temporary = new Map();
  const events = [];
  let lastTemporary = "";
  return {
    events,
    get lastTemporary() {
      return lastTemporary;
    },
    target: () => target,
    async readFile() {
      events.push("read");
      if (options.failRead) {
        const error = new Error("denied secret path");
        error.code = options.failRead;
        throw error;
      }
      if (target === undefined) {
        const error = new Error("missing");
        error.code = "ENOENT";
        throw error;
      }
      return target;
    },
    async mkdir(_path, settings) {
      events.push(`mkdir ${settings.mode}`);
    },
    async writeFile(path, value, settings) {
      events.push(`write ${settings.mode} ${settings.flag}`);
      lastTemporary = path;
      temporary.set(path, value);
    },
    async rename(from, to) {
      events.push("rename");
      assert.equal(to, "/state/history.json");
      if (options.failRename) throw new Error("interrupted private path");
      target = temporary.get(from);
      temporary.delete(from);
    },
    async unlink(path) {
      events.push("unlink");
      temporary.delete(path);
    },
  };
}
