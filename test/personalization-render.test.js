import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { historyView, HISTORY_SCHEMA_VERSION } from "../dist/history.js";
import { openPreferences, preferenceFocusOrder } from "../dist/preferences.js";
import { meterPercent, renderDashboard, renderPlain } from "../dist/render.js";
import { adaptQuotaResponse } from "../dist/schema.js";
import { defaultSettings } from "../dist/settings.js";

const NOW = new Date("2026-08-18T18:00:00.000Z");
const rawFixture = JSON.parse(
  await readFile(new URL("fixtures/complete.json", import.meta.url), "utf8"),
);

function report() {
  return adaptQuotaResponse(JSON.parse(JSON.stringify(rawFixture)));
}

function render(settings = defaultSettings(), options = {}) {
  return renderPlain(
    { report: report(), settings, loading: false, scroll: 0 },
    { width: 36, height: 23, now: NOW, ...options },
  );
}

test("explicit defaults preserve v0.2.0 provider, attention, and remaining semantics", () => {
  const implicit = renderPlain(
    { report: report(), loading: false, scroll: 0 },
    { width: 36, height: 23, now: NOW },
  );
  const explicit = render(defaultSettings());
  assert.equal(explicit, implicit);
  assert.match(explicit, /^! Claude Fable · out 13h/m);
  assert.ok(
    ["Claude", "OpenAI Codex", "Cursor", "Kimi"].every(
      (provider, index, providers) =>
        index === 0 ||
        explicit.indexOf(providers[index - 1]) < explicit.indexOf(provider),
    ),
  );
  assert.match(explicit, /Session {5}███▉ {2}98%/);
  assert.doesNotMatch(explicit.split("\n")[0], /used/i);
});

test("every provider can be hidden individually with an explicit hidden count", () => {
  const labels = new Map([
    ["claude", /^Claude\s*$/m],
    ["codex", /^OpenAI Codex\s*$/m],
    ["cursor", /^Cursor\s*$/m],
    ["kimi", /^Kimi\s*$/m],
    ["grok", /^Grok\s*$/m],
    ["copilot", /^GitHub Copilot\s*$/m],
  ]);
  for (const [provider, pattern] of labels) {
    const output = render({
      ...defaultSettings(),
      hiddenProviders: [provider],
    });
    assert.doesNotMatch(output, pattern, provider);
    assert.match(output, /^1 hidden provider · p Preferences/m, provider);
  }
});

test("all hidden is an honest p-key recovery state at every product size", () => {
  const settings = {
    ...defaultSettings(),
    hiddenProviders: ["claude", "codex", "cursor", "kimi", "grok", "copilot"],
  };
  for (const width of [20, 24, 36]) {
    for (const height of [6, 8, 12, 23]) {
      const output = render(settings, { width, height });
      assert.match(output, /No providers shown/);
      assert.match(output, /Press p for (?:Preferences|prefs)/);
      assert.match(output, /6 hidden/);
      assert.doesNotMatch(output, /All known|Limits on pace|%|█/);
      assert.equal(output.split("\n").length, height);
    }
  }
});

test("provider sections follow deterministic user order without changing attention", () => {
  const settings = {
    ...defaultSettings(),
    providerOrder: ["cursor", "kimi", "claude", "codex"],
  };
  const output = render(settings);
  assert.match(output, /^! Claude Fable · out 13h/m);
  const order = ["Cursor", "Kimi", "Claude", "OpenAI Codex"].map((label) =>
    output.indexOf(`\n${label}`),
  );
  assert.ok(order.every((position) => position >= 0));
  assert.deepEqual(
    order,
    [...order].sort((left, right) => left - right),
  );

  const hiddenLimiting = render({
    ...settings,
    hiddenProviders: ["claude"],
  });
  assert.match(hiddenLimiting, /^! Cursor 3rd-party · out 9d/m);
  assert.doesNotMatch(hiddenLimiting, /^Claude\s*$/m);
});

test("used meters complement only bounded known values and label the mode", async () => {
  assert.equal(meterPercent(0, "used"), 100);
  assert.equal(meterPercent(100, "used"), 0);
  assert.equal(meterPercent(-5, "used"), 100);
  assert.equal(meterPercent(105, "used"), 0);
  assert.equal(meterPercent(undefined, "used"), undefined);
  assert.equal(meterPercent(Number.NaN, "used"), undefined);

  const value = report();
  const claude = value.providers.find(
    (provider) => provider.provider === "claude",
  );
  claude.windows.find((window) => window.id === "five_hour").percentRemaining =
    0;
  claude.windows.find((window) => window.id === "seven_day").percentRemaining =
    100;
  const settings = { ...defaultSettings(), meterMode: "used" };
  const output = renderPlain(
    { report: value, settings, loading: false, scroll: 0 },
    { width: 36, height: 23, now: NOW },
  );
  assert.match(output.split("\n")[0], /AI Quota · used/);
  assert.match(output, /^ Session {5}████ 100%/m);
  assert.match(output, /^ Week {8}──── {3}0%/m);
  assert.match(output, /^ Code review {8}--/m);

  for (const fixture of [
    "partial-failure.json",
    "stale-unknown.json",
    "mixed-auth.json",
  ]) {
    const fixtureValue = adaptQuotaResponse(
      JSON.parse(
        await readFile(new URL(`fixtures/${fixture}`, import.meta.url), "utf8"),
      ),
    );
    const unavailable = renderPlain(
      { report: fixtureValue, settings, loading: false, scroll: 0 },
      { width: 36, height: 23, now: NOW },
    );
    assert.doesNotMatch(unavailable, /signed out.*(?:0|100)%/);
    assert.doesNotMatch(unavailable, /no reading.*(?:0|100)%/);
  }
});

test("screenshot-equivalent equal history never occupies the decision line", () => {
  const fact = (remaining, resetAt = "2026-08-27T03:39:43.000Z") => ({
    scope: "All models",
    limit: "Week",
    remaining,
    resetAt,
    pace: { state: "on_pace", reserve: -0.2 },
    runway: { state: "through_reset" },
  });
  const document = {
    schemaVersion: HISTORY_SCHEMA_VERSION,
    snapshots: [
      {
        capturedAt: "2026-08-20T21:43:19.590Z",
        providers: [
          {
            provider: "Claude",
            dataHealth: "current",
            authEligible: true,
            facts: [fact(100, "2026-08-21T02:40:00.017Z")],
          },
          {
            provider: "OpenAI Codex",
            dataHealth: "current",
            authEligible: true,
            facts: [fact(89.1)],
          },
          {
            provider: "Cursor",
            dataHealth: "current",
            authEligible: true,
            facts: [fact(3, "2026-09-09T19:34:39.000Z")],
          },
        ],
      },
      {
        capturedAt: "2026-08-20T21:49:48.154Z",
        providers: [
          {
            provider: "Claude",
            dataHealth: "current",
            authEligible: true,
            facts: [fact(100, "2026-08-21T02:39:59.782Z")],
          },
          {
            provider: "OpenAI Codex",
            dataHealth: "current",
            authEligible: true,
            facts: [fact(89.4)],
          },
          {
            provider: "Cursor",
            dataHealth: "current",
            authEligible: true,
            facts: [fact(3, "2026-09-09T19:34:39.000Z")],
          },
        ],
      },
    ],
  };
  const history = historyView(document);
  assert.equal(history.evidence, undefined);
  for (const width of [20, 24, 36]) {
    const output = renderPlain(
      { report: report(), history, loading: false, scroll: 0 },
      { width, height: 23, now: NOW },
    );
    const lines = output.split("\n").map((line) => line.trimEnd());
    assert.doesNotMatch(output, /~ |→|89→89|History starts|needs sample/);
    assert.match(lines[1], /^! /);
    assert.match(lines[2], /^Claude$/);
  }
});

test("Preferences remains focused and fully reachable at all required sizes", () => {
  const settings = defaultSettings();
  for (const width of [20, 24, 36]) {
    for (const height of [6, 8, 12, 23]) {
      for (const focus of preferenceFocusOrder(settings)) {
        const preferences = { ...openPreferences(settings), focus };
        const output = renderPlain(
          {
            report: report(),
            settings,
            preferences,
            loading: false,
            scroll: 0,
          },
          { width, height, now: NOW },
        );
        assert.equal(output.split("\n").length, height);
        assert.match(output, /> /, `${width}x${height} ${focus}`);
        for (const line of output.split("\n"))
          assert.ok(line.length <= width, `${width}x${height}: ${line}`);
      }
    }
  }

  const confirmation = {
    ...openPreferences(settings),
    focus: "reset",
    confirmReset: true,
  };
  const output = renderPlain(
    {
      report: report(),
      settings,
      preferences: confirmation,
      loading: false,
      scroll: 0,
    },
    { width: 20, height: 6, now: NOW },
  );
  assert.match(output, /Reset draft\?/);
  assert.match(output, /Save still required/);
  assert.match(output, /y reset · n\/esc back/);
});

test("NO_COLOR retains every severity, state, and selection marker", () => {
  const settings = defaultSettings();
  const preferences = openPreferences(settings);
  const dashboard = renderPlain(
    { report: report(), settings, loading: false, scroll: 0 },
    { width: 36, height: 23, now: NOW },
  );
  assert.match(dashboard, /^! Claude Fable · out 13h/m);
  assert.match(dashboard, /Fable week.*9%.*out 13h/);
  assert.match(dashboard, /Code review.*-- not reported/);
  assert.match(dashboard, /p prefs/);

  const surface = renderDashboard(
    {
      report: report(),
      settings,
      preferences,
      loading: false,
      scroll: 0,
    },
    { width: 20, height: 6, now: NOW, color: false },
  );
  assert.ok(!surface.includes("\x1b["));
  assert.match(surface, /> 1 \[x\] Claude/);
});

test("new transition uses only a title marker and preserves the limiting answer", () => {
  const transitions = {
    availability: "ready",
    events: [
      {
        kind: "threshold_enter",
        provider: "OpenAI Codex",
        scope: "All models",
        limit: "Week",
        threshold: 25,
        occurredAt: "2026-08-18T17:59:00.000Z",
        remaining: 23,
      },
    ],
  };
  for (const width of [20, 24, 36]) {
    for (const height of [6, 8, 12, 23]) {
      const output = renderPlain(
        {
          report: report(),
          settings: { ...defaultSettings(), remainingThreshold: 25 },
          transitions,
          loading: false,
          scroll: 0,
        },
        { width, height, now: NOW },
      );
      const lines = output.split("\n");
      assert.match(lines[0], /Quota.*!/);
      assert.match(lines[1], /^! /);
      assert.match(lines.at(-1), /a alert/);
      assert.equal(lines.length, height);
      for (const line of lines) assert.ok(line.length <= width);
    }
  }
});

test("transition review is concise, no-color, and acknowledges from one key", () => {
  const state = {
    report: report(),
    settings: { ...defaultSettings(), remainingThreshold: 25 },
    transitions: {
      availability: "ready",
      events: [
        {
          kind: "threshold_enter",
          provider: "OpenAI Codex",
          scope: "All models",
          limit: "Week",
          threshold: 25,
          occurredAt: "2026-08-18T17:59:00.000Z",
          remaining: 23,
        },
      ],
    },
    transitionReview: true,
    loading: false,
    scroll: 0,
  };
  for (const width of [20, 24, 36]) {
    for (const height of [6, 8, 12, 23]) {
      const output = renderPlain(state, { width, height, now: NOW });
      assert.match(output, /Codex Week crossed/);
      assert.match(output, /25%/);
      assert.match(output, /23% left/);
      assert.match(output.split("\n").at(-1), /a(?:\/enter)? ack/);
      assert.equal(output.includes("\u001b["), false);
      assert.equal(output.split("\n").length, height);
    }
  }
});

test("clear-transition confirmation names preserved quota history and settings", () => {
  const preferences = {
    ...openPreferences(defaultSettings()),
    focus: "clear_transitions",
    confirmTransitionClear: true,
  };
  const output = renderPlain(
    {
      report: report(),
      settings: defaultSettings(),
      preferences,
      loading: false,
      scroll: 0,
    },
    { width: 24, height: 6, now: NOW },
  );
  assert.match(output, /Clear transition/);
  assert.match(output, /Quota history stays/);
  assert.match(output, /Provider settings stay/);
  assert.match(output, /y clear · n\/esc back/);
});
