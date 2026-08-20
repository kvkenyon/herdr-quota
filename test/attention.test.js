import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { selectAttention } from "../dist/attention.js";
import { adaptQuotaResponse } from "../dist/schema.js";

async function report(name) {
  return adaptQuotaResponse(
    JSON.parse(
      await readFile(new URL(`fixtures/${name}.json`, import.meta.url), "utf8"),
    ),
  );
}

function provider(reportValue, id) {
  return reportValue.providers.find((item) => item.provider === id);
}

function makeHealthy(reportValue) {
  for (const item of reportValue.providers) {
    item.state.status = "fresh";
    item.state.stale = false;
    for (const effective of item.effective) {
      effective.status = "known";
      effective.effectivePercentRemaining = Math.max(
        50,
        effective.effectivePercentRemaining ?? 50,
      );
      effective.pace = { status: "on_pace" };
      effective.runway = { status: "through_reset" };
    }
  }
}

test("the earliest established projected constraint wins across providers", async () => {
  const attention = selectAttention(await report("complete"));
  assert.deepEqual(attention, {
    kind: "constraint",
    severity: "critical",
    provider: "Claude",
    tier: "Fable week",
    compactTier: "Fable",
    constraint: "projected",
    percentRemaining: 9,
    projectedExhaustedAt: "2026-08-19T07:21:00.000Z",
    projectionConfidence: "established",
    resetsAt: "2026-08-20T03:00:00.000Z",
  });
});

test("exhausted and low constraints retain provider-owned tier labels", async () => {
  const exhausted = await report("complete");
  makeHealthy(exhausted);
  const fable = provider(exhausted, "claude").effective[1];
  fable.effectivePercentRemaining = 0;
  fable.limitingWindowIds = ["model:fable"];
  fable.runway = {
    status: "exhausted_now",
    limitingWindowId: "model:fable",
    projectionConfidence: "established",
  };
  assert.equal(selectAttention(exhausted).constraint, "exhausted");
  assert.equal(selectAttention(exhausted).compactTier, "Fable");

  const low = await report("complete");
  makeHealthy(low);
  const codex = provider(low, "codex").effective[0];
  codex.effectivePercentRemaining = 10;
  codex.limitingWindowIds = ["weekly"];
  assert.deepEqual(selectAttention(low), {
    kind: "constraint",
    severity: "critical",
    provider: "Codex",
    tier: "Week",
    compactTier: "Week",
    constraint: "low",
    percentRemaining: 10,
    resetsAt: "2026-08-19T17:35:00.000Z",
  });
});

test("a weekly exhaustion beats a reassuring full session", async () => {
  const value = await report("complete");
  makeHealthy(value);
  const claude = provider(value, "claude");
  claude.windows.find((window) => window.id === "five_hour").percentRemaining =
    100;
  claude.windows.find((window) => window.id === "seven_day").percentRemaining =
    0;
  claude.effective[0].effectivePercentRemaining = 0;
  claude.effective[0].limitingWindowIds = ["seven_day"];
  claude.effective[0].runway = {
    status: "exhausted_now",
    limitingWindowId: "seven_day",
    projectionConfidence: "established",
  };

  const attention = selectAttention(value);
  assert.equal(attention.constraint, "exhausted");
  assert.equal(attention.tier, "Week");
  assert.notEqual(attention.tier, "Session");
});

test("when multiple limits are spent, the longer block wins", async () => {
  const value = await report("complete");
  makeHealthy(value);
  const claude = provider(value, "claude").effective[0];
  claude.effectivePercentRemaining = 0;
  claude.limitingWindowIds = ["seven_day"];
  claude.runway = {
    status: "exhausted_now",
    limitingWindowId: "seven_day",
  };
  const codex = provider(value, "codex").effective[0];
  codex.effectivePercentRemaining = 0;
  codex.limitingWindowIds = ["weekly"];
  codex.runway = { status: "exhausted_now", limitingWindowId: "weekly" };

  const attention = selectAttention(value);
  assert.equal(attention.provider, "Claude");
  assert.equal(attention.tier, "Week");
});

test("unknown limiting ids never escape into attention text", async () => {
  const value = await report("complete");
  makeHealthy(value);
  const cursor = provider(value, "cursor").effective[0];
  cursor.effectivePercentRemaining = 5;
  cursor.limitingWindowIds = ["internal:future-window"];

  const attention = selectAttention(value);
  assert.equal(attention.provider, "Cursor");
  assert.equal(attention.tier, undefined);
  assert.equal(attention.compactTier, undefined);
  assert.doesNotMatch(JSON.stringify(attention), /future-window/);
});

test("healthy, partial, stale, and auth states remain honest", async () => {
  const healthy = await report("complete");
  makeHealthy(healthy);
  assert.deepEqual(selectAttention(healthy), { kind: "healthy", tracked: 4 });

  assert.deepEqual(selectAttention(await report("partial-failure")), {
    kind: "data_health",
    reason: "unreadable",
    unreadable: 2,
    tracked: 1,
  });
  assert.deepEqual(selectAttention(await report("stale-unknown")), {
    kind: "data_health",
    reason: "unreadable",
    unreadable: 2,
    tracked: 0,
  });

  // A current known constraint remains more useful than an auth summary; the
  // signed-out providers still keep their honest detail rows.
  const mixed = selectAttention(await report("mixed-auth"));
  assert.equal(mixed.kind, "constraint");
  assert.equal(mixed.provider, "Cursor");
});

test("a stale dangerous prior reading cannot produce a current forecast", async () => {
  const value = await report("complete");
  makeHealthy(value);
  const claude = provider(value, "claude");
  claude.state.status = "stale";
  claude.state.stale = true;
  claude.effective[1].effectivePercentRemaining = 1;
  claude.effective[1].runway = {
    status: "projected_exhaustion",
    projectedExhaustedAt: "2026-08-18T19:00:00.000Z",
    limitingWindowId: "model:fable",
    projectionConfidence: "established",
  };

  assert.deepEqual(selectAttention(value), {
    kind: "data_health",
    reason: "unreadable",
    unreadable: 1,
    tracked: 3,
  });
});
