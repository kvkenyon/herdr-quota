import assert from "node:assert/strict";
import test from "node:test";
import { HISTORY_SCHEMA_VERSION } from "../dist/history.js";
import { defaultSettings } from "../dist/settings.js";
import {
  TRANSITION_SCHEMA_VERSION,
  appendTransitionEvents,
  baselineTransitions,
  evaluateTransitions,
  parseTransitionDocument,
  transitionView,
} from "../dist/transitions.js";

const START = Date.parse("2026-08-20T12:00:00.000Z");
const RESET = "2026-08-27T12:00:00.000Z";

function settings(overrides = {}) {
  return {
    ...defaultSettings(),
    remainingThreshold: 25,
    ...overrides,
  };
}

function emptyTransitions() {
  return { schemaVersion: TRANSITION_SCHEMA_VERSION, events: [] };
}

function point(
  minute,
  remaining,
  {
    resetAt = RESET,
    runway = "through_reset",
    confidence,
    health = "current",
    authEligible = true,
    present = true,
  } = {},
) {
  const facts =
    present && health === "current" && authEligible
      ? [
          {
            scope: "All models",
            limit: "Week",
            remaining,
            resetAt,
            runway: {
              state: runway,
              ...(confidence ? { confidence } : {}),
            },
          },
        ]
      : [];
  return {
    capturedAt: new Date(START + minute * 60_000).toISOString(),
    providers: present
      ? [
          {
            provider: "OpenAI Codex",
            dataHealth: health,
            authEligible,
            facts,
          },
        ]
      : [],
  };
}

function history(...snapshots) {
  return { schemaVersion: HISTORY_SCHEMA_VERSION, snapshots };
}

function kinds(result) {
  return result.generated
    .map((event) => event.kind)
    .filter((kind) => !kind.endsWith("baseline"));
}

test("threshold state covers first sample, crossing, dedupe, recovery, and recross", () => {
  let document = emptyTransitions();
  let snapshots = [point(0, 40)];
  let update = evaluateTransitions(document, history(...snapshots), settings());
  assert.deepEqual(kinds(update), []);
  assert.deepEqual(
    update.generated.map((event) => event.kind),
    ["threshold_baseline"],
  );
  document = update.document;

  snapshots.push(point(5, 23));
  update = evaluateTransitions(document, history(...snapshots), settings());
  assert.deepEqual(kinds(update), ["threshold_enter"]);
  document = update.document;

  update = evaluateTransitions(document, history(...snapshots), settings());
  assert.deepEqual(kinds(update), []);

  snapshots.push(point(10, 31));
  update = evaluateTransitions(document, history(...snapshots), settings());
  assert.deepEqual(kinds(update), ["threshold_recovery"]);
  document = update.document;

  snapshots.push(point(15, 20));
  update = evaluateTransitions(document, history(...snapshots), settings());
  assert.deepEqual(kinds(update), ["threshold_enter"]);
});

test("reset cycles baseline independently and never fabricate replenishment recovery", () => {
  const configured = settings();
  let document = evaluateTransitions(
    emptyTransitions(),
    history(point(0, 40)),
    configured,
  ).document;
  let snapshots = [point(0, 40), point(5, 20)];
  let update = evaluateTransitions(document, history(...snapshots), configured);
  assert.deepEqual(kinds(update), ["threshold_enter"]);
  document = update.document;

  const nextReset = "2026-09-03T12:00:00.000Z";
  snapshots.push(point(10, 100, { resetAt: nextReset }));
  update = evaluateTransitions(document, history(...snapshots), configured);
  assert.deepEqual(kinds(update), []);
  assert.equal(update.generated.at(-1)?.kind, "threshold_baseline");
  document = update.document;

  snapshots.push(point(15, 20, { resetAt: nextReset }));
  update = evaluateTransitions(document, history(...snapshots), configured);
  assert.deepEqual(kinds(update), ["threshold_enter"]);
});

test("sub-minute reset timestamp jitter stays in one authoritative cycle", () => {
  const firstReset = "2026-08-27T12:00:00.145Z";
  const jitteredReset = "2026-08-27T12:00:00.147Z";
  let document = evaluateTransitions(
    emptyTransitions(),
    history(point(0, 40, { resetAt: firstReset })),
    settings(),
  ).document;
  const crossed = evaluateTransitions(
    document,
    history(
      point(0, 40, { resetAt: firstReset }),
      point(5, 20, { resetAt: jitteredReset }),
    ),
    settings(),
  );
  assert.deepEqual(kinds(crossed), ["threshold_enter"]);
  assert.equal(
    crossed.document.events.at(-1).cycle,
    "2026-08-27T12:00:00.000Z",
  );
});

test("forecast enters and exits only on established authoritative runway", () => {
  const configured = settings({
    remainingThreshold: "off",
    forecastBeforeReset: true,
  });
  let snapshots = [point(0, 60)];
  let update = evaluateTransitions(
    emptyTransitions(),
    history(...snapshots),
    configured,
  );
  assert.deepEqual(
    update.generated.map((event) => event.kind),
    ["forecast_baseline"],
  );
  let document = update.document;

  snapshots.push(
    point(5, 55, {
      runway: "projected_exhaustion",
      confidence: "early",
    }),
  );
  update = evaluateTransitions(document, history(...snapshots), configured);
  assert.deepEqual(kinds(update), []);

  snapshots.push(
    point(10, 50, {
      runway: "projected_exhaustion",
      confidence: "established",
    }),
  );
  update = evaluateTransitions(document, history(...snapshots), configured);
  assert.deepEqual(kinds(update), ["forecast_enter"]);
  document = update.document;

  update = evaluateTransitions(document, history(...snapshots), configured);
  assert.deepEqual(kinds(update), []);

  snapshots.push(point(15, 48));
  update = evaluateTransitions(document, history(...snapshots), configured);
  assert.deepEqual(kinds(update), ["forecast_recovery"]);
});

test("exhausted-now is forecast risk but unknown runway is never a transition", () => {
  const configured = settings({
    remainingThreshold: "off",
    forecastBeforeReset: true,
  });
  let document = evaluateTransitions(
    emptyTransitions(),
    history(point(0, 60)),
    configured,
  ).document;
  let update = evaluateTransitions(
    document,
    history(point(0, 60), point(5, 20, { runway: "unknown" })),
    configured,
  );
  assert.deepEqual(kinds(update), []);
  update = evaluateTransitions(
    document,
    history(
      point(0, 60),
      point(5, 20, { runway: "unknown" }),
      point(10, 0, { runway: "exhausted_now" }),
    ),
    configured,
  );
  assert.deepEqual(kinds(update), ["forecast_enter"]);
});

test("policy and visibility changes establish baselines without synthetic events", () => {
  const original = settings({ remainingThreshold: 25 });
  const samples = history(point(0, 30), point(5, 8));
  let document = evaluateTransitions(
    emptyTransitions(),
    history(point(0, 30)),
    original,
  ).document;

  const tightened = settings({ remainingThreshold: 10 });
  let update = baselineTransitions(
    document,
    samples,
    tightened,
    new Date(START + 6 * 60_000),
    ["threshold"],
  );
  document = update.document;
  update = evaluateTransitions(document, samples, tightened);
  assert.deepEqual(kinds(update), []);

  const hidden = settings({ hiddenProviders: ["codex"] });
  update = evaluateTransitions(document, samples, hidden);
  assert.deepEqual(update.generated, []);
  assert.deepEqual(transitionView(document, samples, hidden).events, []);

  const shown = settings({
    providerOrder: ["kimi", "codex", "cursor", "claude"],
  });
  update = baselineTransitions(
    document,
    samples,
    shown,
    new Date(START + 7 * 60_000),
  );
  assert.deepEqual(kinds(update), []);
  assert.ok(update.generated.every((event) => event.kind.endsWith("baseline")));
});

test("baselines retain the current sample across wall-clock skew", () => {
  const thresholdSettings = settings();
  const first = history(point(0, 40));
  let document = baselineTransitions(
    emptyTransitions(),
    first,
    thresholdSettings,
    new Date(START + 60 * 60_000),
    ["threshold"],
  ).document;
  assert.equal(
    document.events.at(-1).occurredAt,
    first.snapshots[0].capturedAt,
  );
  let update = evaluateTransitions(
    document,
    history(point(0, 40), point(5, 20)),
    thresholdSettings,
  );
  assert.deepEqual(kinds(update), ["threshold_enter"]);

  const forecastSettings = settings({
    remainingThreshold: "off",
    forecastBeforeReset: true,
  });
  document = baselineTransitions(
    emptyTransitions(),
    first,
    forecastSettings,
    new Date(START - 60 * 60_000),
    ["forecast"],
  ).document;
  update = evaluateTransitions(
    document,
    history(
      point(0, 40),
      point(5, 35, {
        runway: "projected_exhaustion",
        confidence: "established",
      }),
    ),
    forecastSettings,
  );
  assert.deepEqual(kinds(update), ["forecast_enter"]);
});

test("visibility baselines target only changed providers", () => {
  const configured = settings();
  const codex = point(0, 20).providers[0];
  const claude = {
    ...structuredClone(codex),
    provider: "Claude",
  };
  const samples = history({
    capturedAt: new Date(START).toISOString(),
    providers: [codex, claude],
  });
  const existing = evaluateTransitions(
    emptyTransitions(),
    samples,
    configured,
  ).document;
  const update = baselineTransitions(
    existing,
    samples,
    settings({ hiddenProviders: ["claude"] }),
    new Date(START + 10 * 60_000),
    ["threshold", "forecast"],
    ["claude"],
  );
  assert.ok(update.generated.length > 0);
  assert.ok(update.generated.every((event) => event.provider === "Claude"));
  assert.equal(
    update.document.events.filter((event) => event.provider === "OpenAI Codex")
      .length,
    existing.events.filter((event) => event.provider === "OpenAI Codex").length,
  );
});

test("pane reopen dedupes and retained-history catch-up emits once", () => {
  const configured = settings();
  const first = history(point(0, 40));
  const baseline = evaluateTransitions(
    emptyTransitions(),
    first,
    configured,
  ).document;
  const reopenedDocument = parseTransitionDocument(
    JSON.parse(JSON.stringify(baseline)),
  );
  const retained = history(point(0, 40), point(30, 20));
  const catchUp = evaluateTransitions(reopenedDocument, retained, configured);
  assert.deepEqual(kinds(catchUp), ["threshold_enter"]);
  const secondOpen = evaluateTransitions(
    parseTransitionDocument(JSON.parse(JSON.stringify(catchUp.document))),
    retained,
    configured,
  );
  assert.deepEqual(kinds(secondOpen), []);
  assert.equal(
    transitionView(secondOpen.document, retained, configured).events.length,
    1,
  );
});

test("a new baseline archives older unacknowledged cues instead of resurrecting them", () => {
  const configured = settings();
  const samples = history(point(0, 40), point(5, 20));
  let document = evaluateTransitions(
    emptyTransitions(),
    history(point(0, 40)),
    configured,
  ).document;
  document = evaluateTransitions(document, samples, configured).document;
  assert.equal(transitionView(document, samples, configured).events.length, 1);
  document = baselineTransitions(
    document,
    samples,
    configured,
    new Date(START + 6 * 60_000),
    ["threshold"],
  ).document;
  assert.deepEqual(transitionView(document, samples, configured).events, []);
});

test("stale, auth, unavailable, error, unknown, and missing gaps never fabricate state", () => {
  for (const gap of [
    { health: "stale" },
    { authEligible: false },
    { health: "unavailable" },
    { health: "error" },
    { health: "unknown" },
    { present: false },
  ]) {
    const configured = settings();
    let document = evaluateTransitions(
      emptyTransitions(),
      history(point(0, 40)),
      configured,
    ).document;
    const before = document.events.length;
    let update = evaluateTransitions(
      document,
      history(point(0, 40), point(5, 0, gap)),
      configured,
    );
    assert.deepEqual(kinds(update), [], JSON.stringify(gap));
    assert.equal(update.document.events.length, before, JSON.stringify(gap));

    update = evaluateTransitions(
      document,
      history(point(0, 40), point(5, 0, gap), point(10, 20)),
      configured,
    );
    assert.deepEqual(kinds(update), ["threshold_enter"], JSON.stringify(gap));
    document = update.document;

    update = evaluateTransitions(
      document,
      history(point(0, 40), point(5, 0, gap), point(10, 20), point(15, 0, gap)),
      configured,
    );
    assert.deepEqual(kinds(update), [], JSON.stringify(gap));

    update = evaluateTransitions(
      document,
      history(
        point(0, 40),
        point(5, 0, gap),
        point(10, 20),
        point(15, 0, gap),
        point(20, 35),
      ),
      configured,
    );
    assert.deepEqual(
      kinds(update),
      ["threshold_recovery"],
      JSON.stringify(gap),
    );
  }
});

test("a reset reached through a gap cannot recover the prior cycle", () => {
  const configured = settings();
  let document = evaluateTransitions(
    emptyTransitions(),
    history(point(0, 40)),
    configured,
  ).document;
  let update = evaluateTransitions(
    document,
    history(point(0, 40), point(5, 20)),
    configured,
  );
  document = update.document;
  update = evaluateTransitions(
    document,
    history(
      point(0, 40),
      point(5, 20),
      point(10, 0, { health: "error" }),
      point(15, 100, { resetAt: "2026-09-03T12:00:00.000Z" }),
    ),
    configured,
  );
  assert.deepEqual(kinds(update), []);
  assert.equal(update.generated.at(-1)?.kind, "threshold_baseline");
});

test("whole-check failure is a no-call gap and cannot mutate transition state", () => {
  const configured = settings();
  const document = evaluateTransitions(
    emptyTransitions(),
    history(point(0, 40)),
    configured,
  ).document;
  const persisted = JSON.stringify(document);
  // Collector failure never calls evaluateTransitions; the bounded state is
  // therefore exactly unchanged before, during, and after the failed attempt.
  assert.equal(JSON.stringify(document), persisted);
});

test("clock rollback starts a new bounded transition segment", () => {
  const configured = settings();
  const future = evaluateTransitions(
    emptyTransitions(),
    history(point(60, 40)),
    configured,
  ).document;
  const earlier = {
    ...future.events[0],
    occurredAt: new Date(START).toISOString(),
    cycle: "unbounded",
  };
  const update = appendTransitionEvents(future, [earlier]);
  assert.deepEqual(update.document.events, [earlier]);
});
