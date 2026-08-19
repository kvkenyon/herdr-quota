import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import {
  compactCountdown,
  displayName,
  effectivePercent,
  formatDuration,
  formatPercent,
  limitingWindow,
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

test("the effective aggregate stays available for emphasis", async () => {
  const codex = (await providers("complete")).find(
    (item) => item.provider === "codex",
  );
  assert.equal(effectivePercent(codex), 79);
  assert.equal(limitingWindow(codex).id, "weekly");
});

test("marketed display names take precedence over wire labels", async () => {
  const codex = (await providers("complete")).find(
    (item) => item.provider === "codex",
  );
  assert.equal(codex.label, "Codex");
  assert.equal(displayName(codex), "OpenAI Codex");
});

test("formats bounded durations", () => {
  assert.equal(formatDuration(-3), "0s");
  assert.equal(formatDuration(90), "1m");
  assert.equal(formatDuration(90061), "1d 1h");
});

test("compact countdowns fit the three-character reset column", () => {
  assert.equal(compactCountdown(-5), "now");
  assert.equal(compactCountdown(30), "<1m");
  assert.equal(compactCountdown(45 * 60), "45m");
  assert.equal(compactCountdown(33 * 3600), "33h");
  assert.equal(compactCountdown(47 * 3600 + 1800), "47h");
  assert.equal(compactCountdown(22 * 86400), "22d");
});
