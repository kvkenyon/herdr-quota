import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import {
  effectivePercent,
  formatDuration,
  formatPercent,
  limitingWindow,
  paceSummary,
} from "../dist/format.js";
import { adaptQuotaResponse } from "../dist/schema.js";

async function providers(name) {
  const raw = JSON.parse(
    await readFile(new URL(`fixtures/${name}.json`, import.meta.url), "utf8"),
  );
  return adaptQuotaResponse(raw).providers;
}

test("unknown percentages never become misleading zeroes", async () => {
  const [, cursor] = await providers("stale-unknown");
  assert.equal(formatPercent(effectivePercent(cursor)), "--");
});

test("uses quota-axi effective headroom and limiting window", async () => {
  const [, codex] = await providers("complete");
  assert.equal(effectivePercent(codex), 42);
  assert.equal(limitingWindow(codex).id, "five_hour");
  assert.match(paceSummary(codex), /may run out in 1h 12m/);
});

test("formats bounded durations", () => {
  assert.equal(formatDuration(-3), "0s");
  assert.equal(formatDuration(90), "1m");
  assert.equal(formatDuration(90061), "1d 1h");
});

test("missing runway evidence never becomes a synthetic zero", () => {
  const provider = {
    provider: "future",
    windows: [],
    effective: [
      {
        scope: "all_models",
        status: "known",
        boundedBy: [],
        runway: { status: "projected_exhaustion" },
      },
    ],
    state: { status: "fresh", stale: false },
  };
  assert.equal(paceSummary(provider), "may run out before reset");
});
