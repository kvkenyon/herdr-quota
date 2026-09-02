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
  assert.equal(report.providers[0].effective[0].effectivePercentRemaining, 53);
  assert.equal(report.providers[0].windows.length, 4);
  assert.equal(report.adaptationWarnings.length, 0);
});

test("drops grok and unknown providers even when returned", async () => {
  const report = adaptQuotaResponse(await fixture("excluded-providers"));
  assert.deepEqual(
    report.providers.map((provider) => provider.provider),
    ["claude", "copilot", "kimi"],
  );
  assert.equal(report.adaptationWarnings.length, 0);
});

test("preserves unknown window and tier ids inside allowed providers", async () => {
  const report = adaptQuotaResponse(await fixture("multiple-windows"));
  const kimi = report.providers.find((item) => item.provider === "kimi");
  assert.deepEqual(
    kimi.windows.map((window) => window.id),
    ["weekly", "five_hour", "limit:2"],
  );
  assert.equal(kimi.windows[2].label, "daily boost");
});

test("isolates malformed providers instead of dropping healthy siblings", async () => {
  const raw = await fixture("partial-failure");
  raw.providers[1] = { provider: "cursor", windows: "not-an-array", state: {} };
  const report = adaptQuotaResponse(raw);
  assert.equal(report.providers[0].state.status, "fresh");
  assert.equal(report.providers[1].state.status, "error");
  assert.equal(report.providers[1].state.errorCode, "schema_invalid");
  assert.equal(report.providers[2].provider, "kimi");
  assert.match(report.adaptationWarnings[0], /cursor/);
});

test("an entry with an unreadable provider id stays visible as an error", async () => {
  const raw = await fixture("partial-failure");
  raw.providers[0] = { windows: [] };
  const report = adaptQuotaResponse(raw);
  assert.equal(report.providers[0].provider, "provider-1");
  assert.equal(report.providers[0].state.errorCode, "schema_invalid");
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
  const raw = await fixture("complete");
  raw.providers[0].label = "Cla\x1b[2Jude\n";
  raw.providers[0].windows[0].label = "ses\rsion";
  const report = adaptQuotaResponse(raw);
  assert.equal(report.providers[0].label, "Claude");
  assert.equal(report.providers[0].windows[0].label, "session");
});
