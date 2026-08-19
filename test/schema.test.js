import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { adaptQuotaResponse } from "../dist/schema.js";

async function fixture(name) {
  return JSON.parse(
    await readFile(new URL(`fixtures/${name}.json`, import.meta.url), "utf8"),
  );
}

test("adapts the complete schema-v5 fixture", async () => {
  const report = adaptQuotaResponse(await fixture("complete"));
  assert.equal(report.schemaVersion, 5);
  assert.deepEqual(
    report.providers.map((provider) => provider.provider),
    ["claude", "codex", "cursor", "kimi"],
  );
  assert.equal(report.providers[0].effective[0].effectivePercentRemaining, 64);
  assert.equal(report.adaptationWarnings.length, 0);
});

test("preserves a future provider id", async () => {
  const report = adaptQuotaResponse(await fixture("extra-provider"));
  assert.equal(report.providers[0].provider, "future-lab");
  assert.equal(report.providers[0].effective[0].effectivePercentRemaining, 73);
});

test("isolates malformed providers instead of dropping healthy siblings", async () => {
  const raw = await fixture("partial-failure");
  raw.providers[1] = { provider: "cursor", windows: "not-an-array", state: {} };
  const report = adaptQuotaResponse(raw);
  assert.equal(report.providers[0].state.status, "fresh");
  assert.equal(report.providers[1].state.status, "error");
  assert.equal(report.providers[2].provider, "kimi");
  assert.match(report.adaptationWarnings[0], /cursor/);
});

test("rejects an unversioned or changed top-level contract", () => {
  assert.throws(
    () => adaptQuotaResponse({ schemaVersion: 6, providers: [] }),
    /expected version 5/,
  );
  assert.throws(
    () => adaptQuotaResponse({ schemaVersion: 5, providers: [] }),
    /Invalid/,
  );
});

test("strips terminal control sequences from normalized display fields", async () => {
  const raw = await fixture("extra-provider");
  raw.providers[0].label = "Future\x1b[2J Lab\n";
  raw.providers[0].plan = "Research\rPlan";
  const report = adaptQuotaResponse(raw);
  assert.equal(report.providers[0].label, "Future Lab");
  assert.equal(report.providers[0].plan, "ResearchPlan");
});
