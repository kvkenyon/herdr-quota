import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { adaptQuotaResponse } from "../dist/schema.js";
import {
  ALLOWED_PROVIDERS,
  isAllowedProvider,
  presentProvider,
  providerAnnotation,
  providerTiers,
} from "../dist/tiers.js";
import { MARKETED_PROVIDERS } from "../dist/types.js";

async function providers(name) {
  const raw = JSON.parse(
    await readFile(new URL(`fixtures/${name}.json`, import.meta.url), "utf8"),
  );
  return adaptQuotaResponse(raw).providers;
}

function byId(list, provider) {
  return list.find((item) => item.provider === provider);
}

test("only the four marketed providers are allowed", () => {
  assert.deepEqual(
    MARKETED_PROVIDERS.map((provider) => provider.id),
    ["claude", "codex", "cursor", "kimi"],
  );
  assert.deepEqual(
    [...ALLOWED_PROVIDERS],
    ["claude", "codex", "cursor", "kimi"],
  );
  assert.equal(isAllowedProvider("Claude"), true);
  assert.equal(isAllowedProvider("grok"), false);
  assert.equal(isAllowedProvider("copilot"), false);
});

test("claude tiers get concise human labels in provider order", async () => {
  const claude = byId(await providers("multiple-windows"), "claude");
  assert.deepEqual(
    providerTiers(claude).map((row) => row.label),
    ["Session", "Week", "Opus week", "Fable week", "Extra usage"],
  );
});

test("claude extra usage keeps spend evidence and unknown pace", async () => {
  const claude = byId(await providers("complete"), "claude");
  const rows = providerTiers(claude);
  const extra = rows.find((row) => row.id === "extra_usage");
  assert.deepEqual(extra.conclusion, {
    kind: "spend",
    spentUsd: 207.08,
    limitUsd: 500,
  });
  const fable = rows.find((row) => row.id === "model:fable");
  assert.equal(fable.conclusion.kind, "ahead");
  assert.equal(
    fable.conclusion.projectedExhaustedAt,
    "2026-08-19T07:21:00.000Z",
  );
});

test("codex gets an explicit code-review row when none is reported", async () => {
  const codex = byId(await providers("complete"), "codex");
  const rows = providerTiers(codex);
  assert.deepEqual(
    rows.map((row) => row.label),
    ["Week", "Spark week", "Code review"],
  );
  assert.equal(rows.at(-1).conclusion.kind, "not_reported");
  assert.equal(rows.at(-1).percentRemaining, undefined);
});

test("codex renders real code-review windows instead of the placeholder", async () => {
  const codex = byId(await providers("code-review"), "codex");
  const labels = providerTiers(codex).map((row) => row.label);
  assert.deepEqual(labels, [
    "Session",
    "Week",
    "Spark week",
    "Review 5h",
    "Review week",
  ]);
  assert.ok(!labels.includes("Code review"));
});

test("codex model session and week labels compact semantically", async () => {
  const codex = byId(await providers("codex-model-windows"), "codex");
  const rows = providerTiers(codex);
  assert.deepEqual(
    rows.slice(0, 4).map((row) => [row.label, row.compactLabel]),
    [
      ["Session", "Session"],
      ["Week", "Week"],
      ["Spark 5h", "Spark 5h"],
      ["Spark week", "Spark"],
    ],
  );
  assert.ok(rows.every((row) => !row.label.includes("session 5h")));
});

test("cursor separates included, auto, and 3rd-party model buckets", async () => {
  const cursor = byId(await providers("complete"), "cursor");
  const rows = providerTiers(cursor);
  assert.deepEqual(
    rows.map((row) => row.label),
    ["Included", "Auto", "3rd-party models"],
  );
  assert.ok(!rows.some((row) => row.label.includes("API")));
  const thirdParty = rows.at(-1);
  assert.equal(thirdParty.percentRemaining, 49);
  assert.equal(thirdParty.conclusion.kind, "ahead");
  assert.equal(thirdParty.limiting, true);
});

test("kimi keeps self-described additional limits in provider order", async () => {
  const kimi = byId(await providers("multiple-windows"), "kimi");
  assert.deepEqual(
    providerTiers(kimi).map((row) => row.label),
    ["Week", "Session", "daily boost"],
  );
});

test("the aggregate limiting tier is flagged without hiding the others", async () => {
  const claude = byId(await providers("complete"), "claude");
  const rows = providerTiers(claude);
  assert.deepEqual(
    rows.filter((row) => row.limiting).map((row) => row.id),
    ["seven_day"],
  );
  assert.equal(rows.length, 4);
});

test("signed-out providers replace tiers with one recovery instruction", async () => {
  const mixed = await providers("mixed-auth");
  assert.deepEqual(presentProvider(byId(mixed, "claude")), {
    kind: "recovery",
    instruction: "claude, then /login",
  });
  assert.deepEqual(presentProvider(byId(mixed, "kimi")), {
    kind: "recovery",
    instruction: "kimi login",
  });
  assert.equal(presentProvider(byId(mixed, "codex")).kind, "tiers");
  assert.equal(presentProvider(byId(mixed, "cursor")).kind, "tiers");
});

function bareProvider(provider, state) {
  return { provider, windows: [], effective: [], state };
}

test("every provider has its own sign-in remedy", () => {
  const auth = { status: "auth_required", stale: false };
  assert.equal(
    presentProvider(bareProvider("codex", auth)).instruction,
    "codex login",
  );
  assert.equal(
    presentProvider(bareProvider("cursor", auth)).instruction,
    "cursor-agent login",
  );
});

test("refreshable expiry also asks for the owning CLI login", () => {
  const provider = bareProvider("cursor", {
    status: "unavailable",
    stale: false,
    authStatus: "expired_refreshable",
  });
  assert.equal(presentProvider(provider).kind, "recovery");
  assert.equal(providerAnnotation(provider).text, "signed out");
});

test("keychain approval is asked for, not conflated with sign-out", () => {
  const provider = bareProvider("claude", {
    status: "unavailable",
    stale: false,
    authStatus: "unusable",
    reason: "keychain_access_required",
  });
  assert.deepEqual(presentProvider(provider), {
    kind: "message",
    message: "Keychain approval required",
  });
  assert.equal(providerAnnotation(provider), undefined);
});

test("an unreadable provider is distinct from zero quota", async () => {
  const kimi = byId(await providers("partial-failure"), "kimi");
  assert.deepEqual(presentProvider(kimi), {
    kind: "message",
    message: "Network unavailable",
  });
  assert.equal(providerAnnotation(kimi).text, "no reading");
});
