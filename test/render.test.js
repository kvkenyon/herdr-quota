import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { stripAnsi } from "../dist/ansi.js";
import {
  applyDashboardScroll,
  clampDashboardScroll,
  dashboardScrollMetrics,
  renderDashboard,
  renderPlain,
} from "../dist/render.js";
import { adaptQuotaResponse } from "../dist/schema.js";

const NOW = new Date("2026-08-18T18:00:00.000Z");

async function report(name) {
  return adaptQuotaResponse(
    JSON.parse(
      await readFile(new URL(`fixtures/${name}.json`, import.meta.url), "utf8"),
    ),
  );
}

function render(reportValue, options = {}) {
  return renderPlain(
    { report: reportValue, loading: false, scroll: 0 },
    { width: 36, height: 23, now: NOW, ...options },
  );
}

test("36-cell sidebar shows every provider tier without scrolling", async () => {
  const output = render(await report("complete"));
  const lines = output.split("\n").map((line) => line.trimEnd());
  assert.deepEqual(lines.slice(0, 21), [
    "AI Quota                      1m ago",
    "! Claude Fable · out 13h",
    "Claude",
    " Session     ███▉  98%  3h · on pace",
    " Week        ██──  53% 33h · on pace",
    " Fable week  ▎───   9% 33h · out 13h",
    " Extra usage ██▎─  59% $207 of $500",
    "",
    "OpenAI Codex",
    " Week        ███▏  79% 23h · on pace",
    " Spark week  ████ 100%  7d · on pace",
    " Code review        -- not reported",
    "",
    "Cursor",
    " Included    ██▊─  69% 22d · on pace",
    " Auto        ██▉─  74% 22d · on pace",
    " 3rd-party   █▉──  49% 22d · out 9d",
    "",
    "Kimi",
    " Week        ████ 100%  2d · on pace",
    " Session     ████ 100% 57m · on pace",
  ]);
  assert.match(output, /^j\/k Pg · p prefs · r · q\/esc$/m);
  assert.doesNotMatch(output, /more rows/);
  assert.doesNotMatch(output, /Rows \d/);
  for (const line of output.split("\n")) assert.ok(line.length <= 36, line);
});

test("gauges, percentages, and countdowns share one column across providers", async () => {
  const output = render(await report("complete"));
  const tiers = output
    .split("\n")
    .filter((line) => line.startsWith(" ") && /\d%|-- /.test(line));
  assert.equal(tiers.length, 12);
  for (const line of tiers) {
    // Gauge, then the value right-aligned in the same four cells, for every
    // tier of every provider.
    assert.match(line.slice(13, 18), /^[█▏▎▍▌▋▊▉─ ]{4} $/, line);
    assert.match(line.slice(18, 22), /^(?: *\d+%| +--)$/, line);
  }
});

test("a gauge only ever restates the percentage beside it", async () => {
  const output = render(await report("complete"));
  for (const line of output.split("\n")) {
    const gauge = /^ .{11} ([█▏▎▍▌▋▊▉─]{4}) +(\d+)%/.exec(line);
    if (!gauge) continue;
    const filled = [...gauge[1]].filter((cell) => cell !== "─").length;
    const expected = (Number(gauge[2]) / 100) * 4;
    assert.ok(
      Math.abs(filled - expected) <= 1,
      `${line}: ${filled} cells for ${gauge[2]}%`,
    );
  }
});

test("full, exhausted, and unknown tiers are distinct at a glance", async () => {
  const complete = await report("complete");
  const claude = complete.providers.find(
    (provider) => provider.provider === "claude",
  );
  claude.windows.find((window) => window.id === "five_hour").percentRemaining =
    0;
  claude.windows.find((window) => window.id === "seven_day").percentRemaining =
    99;

  const output = render(complete);
  // Exactly empty, exactly full, a notch short of full, and no reading.
  assert.match(output, /^ Session {5}──── {3}0%/m);
  assert.match(output, /^ Spark week {2}████ 100%/m);
  assert.match(output, /^ Week {8}███▉ {2}99%/m);
  assert.match(output, /^ Code review {8}--/m);
});

test("no provider art, aggregates-only output, or implementation jargon", async () => {
  const output = render(await report("complete"));
  assert.doesNotMatch(output, /[\\*]|o-o|\|_>|\(_/);
  assert.doesNotMatch(output, /left\b|WINDOWS|bounds:|scope|semantics/i);
  assert.doesNotMatch(output, /plan|source|credits|confidence/i);
  assert.doesNotMatch(output, /all_models|seven_day|five_hour|api_usage/);
});

test("grok and unknown providers are never rendered", async () => {
  const output = render(await report("excluded-providers"));
  assert.match(output, /Claude/);
  assert.match(output, /Kimi/);
  assert.match(output, /GitHub Copilot/);
  assert.match(output, /github-copilot-cli auth login/);
  assert.doesNotMatch(output, /Grok|Future/i);
});

test("Copilot renders reported windows, resets, partial state, and sign-in honestly", async () => {
  const quota = render(await report("copilot"));
  assert.match(quota, /GitHub Copilot/);
  assert.match(quota, /Chat.*60%.*13d.*--/);
  assert.match(quota, /Completions.*40%.*13d.*--/);
  assert.match(quota, /Premium.*20%.*13d.*--/);
  assert.doesNotMatch(quota, /on pace|out \d/);

  const signedOut = render(await report("excluded-providers"));
  assert.match(signedOut, /GitHub Copilot.*signed out/);
  assert.match(signedOut, /github-copilot-cli auth login/);
  assert.doesNotMatch(
    signedOut.split("GitHub Copilot")[1].split("Kimi")[0],
    /%/,
  );
});

test("mixed authentication shows per-provider recovery without numbers", async () => {
  const output = render(await report("mixed-auth"));
  assert.match(output, /Claude {2,}signed out/);
  assert.match(output, /^ claude, then \/login/m);
  assert.match(output, /Kimi {2,}signed out/);
  assert.match(output, /^ kimi login/m);
  assert.match(output, /Spark week {2}████ 100%/);
  assert.match(output, /3rd-party {3}█▉── {2}49%/);
  const claudeSection = output.split("OpenAI Codex")[0];
  assert.doesNotMatch(claudeSection.split("Claude")[1] ?? "", /%/);
  assert.doesNotMatch(output, /credentials|expired|keychain/i);
});

test("an unavailable reading is distinct from zero quota", async () => {
  const output = render(await report("partial-failure"));
  assert.match(output, /Cursor {2,}signed out/);
  assert.match(output, /^ cursor-agent login/m);
  assert.match(output, /Kimi {2,}no reading/);
  assert.match(output, /Network unavailable/);
  assert.doesNotMatch(output, /0%/);
  assert.doesNotMatch(output, /retry/);
});

test("stale readings stay visible without pretending pace is known", async () => {
  const output = render(await report("stale-unknown"));
  assert.match(output, /OpenAI Codex {2,}stale/);
  assert.match(output, /Week {8}▉─── {2}24% 42h · --/);
  // No reading at all: a blank gauge and one dash, never a filled bar.
  assert.match(output, /^ Included {11}--\s*$/m);
  assert.doesNotMatch(output, /0%/);
});

test("narrow widths degrade honestly without wrapping or clipping", async () => {
  for (const width of [38, 36, 32, 29, 24, 20]) {
    for (const height of [12, 23, 30]) {
      for (const fixture of ["complete", "mixed-auth", "codex-model-windows"]) {
        const output = renderDashboard(
          { report: await report(fixture), loading: false, scroll: 0 },
          { width, height, now: NOW, color: true },
        );
        const plain = stripAnsi(output);
        assert.match(plain.split("\n")[1], /^[!?=] /);
        for (const line of plain.split("\n")) {
          assert.ok(
            line.length <= width,
            `${fixture}@${width}x${height}: ${line}`,
          );
          assert.equal(line.trimEnd().endsWith(">"), false, line);
        }
      }
    }
  }
  const narrow = renderPlain(
    { report: await report("complete"), loading: false, scroll: 0 },
    { width: 24, height: 30, now: NOW },
  );
  assert.match(narrow, /3rd-party {2,}49% 22d/);
});

test("a pane too narrow for both keeps the pace and drops the gauge", async () => {
  const complete = await report("complete");
  const at = (width) =>
    renderPlain(
      { report: complete, loading: false, scroll: 0 },
      { width, height: 30, now: NOW },
    );

  // 36 is the design target: every column present.
  assert.match(at(36), /^ Session {5}███▉ {2}98% {2}3h · on pace\s*$/m);
  // Too tight for both, and the conclusion is the column nothing replaces.
  assert.match(at(34), /^ Session {6}98% {2}3h · on pace\s*$/m);
  assert.doesNotMatch(at(34), /█/);
  // Below a pace conclusion the freed cells go back to the gauge.
  assert.match(at(26), /^ Session {5}███▉ {2}98% {2}3h\s*$/m);
  // Nothing left to spend: numbers only.
  assert.doesNotMatch(at(22), /█/);
});

test("short panes cut whole rows and report the visible position", async () => {
  const output = render(await report("complete"), { height: 12 });
  assert.equal(output.split("\n").length, 12);
  assert.match(output, /^! Claude Fable · out 13h/m);
  assert.match(output, /^Rows 1–8 of 16/m);
  assert.match(output, /^j\/k Pg · p prefs · r · q\/esc$/m);
  assert.doesNotMatch(output, /more rows/);
  assert.doesNotMatch(output, /^\s*$/m);
});

test("every provider/detail row is reachable with pinned context at exact pane sizes", async () => {
  const value = await report("complete");
  for (const width of [20, 24, 36]) {
    const reference = render(value, { width, height: 40 })
      .split("\n")
      .slice(2, -1)
      .map((line) => line.trimEnd())
      .filter(Boolean);
    assert.equal(reference.length, 16, `${width}-column detail count`);

    for (const height of [6, 8, 12, 23]) {
      const state = { report: value, loading: false, scroll: 0 };
      const reached = Array(reference.length).fill(false);
      let pinned;

      while (true) {
        const metrics = dashboardScrollMetrics(state, height);
        const lines = renderPlain(state, { width, height, now: NOW })
          .split("\n")
          .map((line) => line.trimEnd());
        assert.equal(lines.length, height);
        for (const line of lines)
          assert.ok(line.length <= width, `${width}x${height}: ${line}`);

        const currentPinned = [lines[0], lines[1], lines.at(-1)];
        pinned ??= currentPinned;
        assert.deepEqual(currentPinned, pinned, `${width}x${height} pinned`);

        const detailEnd = metrics.overflowing ? -2 : -1;
        const visible = lines.slice(2, detailEnd).filter(Boolean);
        assert.deepEqual(
          visible,
          reference.slice(
            metrics.scroll,
            metrics.scroll + metrics.viewportRows,
          ),
          `${width}x${height} at ${metrics.scroll}`,
        );
        for (
          let index = metrics.scroll;
          index < metrics.scroll + metrics.viewportRows;
          index++
        )
          reached[index] = true;

        if (metrics.overflowing) {
          assert.equal(
            lines.at(-2),
            `Rows ${metrics.scroll + 1}–${metrics.scroll + metrics.viewportRows} of ${metrics.rowCount}`,
          );
        }
        if (metrics.scroll === metrics.maxScroll) break;
        applyDashboardScroll(state, "scroll_down", height);
      }
      assert.ok(reached.every(Boolean), `${width}x${height} reachable`);
    }
  }
});

test("line and page navigation clamp after data, auth, and height changes", async () => {
  const value = await report("complete");
  const state = { report: value, loading: false, scroll: 0 };

  assert.equal(applyDashboardScroll(state, "page_down", 8), 4);
  assert.equal(applyDashboardScroll(state, "scroll_down", 8), 5);
  assert.equal(applyDashboardScroll(state, "page_up", 8), 1);
  assert.equal(applyDashboardScroll(state, "scroll_up", 8), 0);
  assert.equal(applyDashboardScroll(state, "scroll_up", 8), 0);

  state.scroll = 999;
  assert.equal(clampDashboardScroll(state, 8), 12);
  state.scroll = clampDashboardScroll(state, 8);
  state.report.providers = state.report.providers.slice(0, 2);
  assert.equal(clampDashboardScroll(state, 8), 5);

  state.scroll = clampDashboardScroll(state, 8);
  state.report.providers[0].state.status = "auth_required";
  assert.equal(clampDashboardScroll(state, 8), 2);

  state.scroll = clampDashboardScroll(state, 8);
  assert.equal(clampDashboardScroll(state, 23), 0);
});

test("safe top-level failures retain last-good detail and show automatic retry timing", async () => {
  const value = await report("complete");
  const expectedAt20 = new Map([
    ["timeout", "Timeout · retry 10m"],
    ["missing_executable", "Missing · retry 10m"],
    ["incompatible_output", "Schema · retry 10m"],
    ["network_process", "Failed · retry 10m"],
  ]);

  for (const [kind, expected] of expectedAt20) {
    const state = {
      report: value,
      loading: false,
      scroll: 999,
      failure: {
        kind,
        retryAt: new Date(NOW.getTime() + 10 * 60_000),
        raw: "\u001b[2JBearer secret /home/alice/.codex/auth.json",
      },
    };
    const output = renderPlain(state, { width: 20, height: 8, now: NOW });
    const lines = output.split("\n").map((line) => line.trimEnd());
    assert.ok(lines.includes(expected));
    assert.ok(lines.includes("! Claude · out 13h"));
    assert.ok(lines.includes("Rows 14–16 of 16"));
    assert.ok(lines.includes("Kimi"));
    assert.ok(lines.includes(" Session    100% 57m"));
    assert.doesNotMatch(output, /secret|alice|auth\.json|Bearer/i);
    assert.equal(output.includes("\u001b"), false);
    for (const line of output.split("\n")) assert.ok(line.length <= 20, line);
  }
});

test("failure copy expands safely and first-load failures remain actionable", () => {
  const cases = new Map([
    ["timeout", "Quota check timed out · retry 10m"],
    ["missing_executable", "quota-axi missing · retry 10m"],
    ["incompatible_output", "Incompatible output · retry 10m"],
    ["network_process", "Network/process failed · retry 10m"],
  ]);
  for (const [kind, expected] of cases) {
    const output = renderPlain(
      {
        loading: false,
        scroll: 0,
        failure: {
          kind,
          retryAt: new Date(NOW.getTime() + 10 * 60_000),
        },
      },
      { width: 36, height: 6, now: NOW },
    );
    const lines = output.split("\n").map((line) => line.trimEnd());
    assert.ok(lines.includes(expected));
    assert.ok(lines.includes("No quota readings"));
    assert.ok(lines.includes("Press r to retry now"));
  }
});

test("24x30 and 20x12 no-color views are stable snapshots", async () => {
  const value = await report("complete");
  const at24 = render(value, { width: 24, height: 30 })
    .split("\n")
    .map((line) => line.trimEnd());
  assert.deepEqual(at24, [
    "Quota             1m ago",
    "! Claude Fable · out 13h",
    "Claude",
    " Session      98%  3h",
    " Week         53% 33h",
    " Fable week    9% 33h",
    " Extra usage  59%  --",
    "",
    "OpenAI Codex",
    " Week         79% 23h",
    " Spark week  100%  7d",
    " Code review   --  --",
    "",
    "Cursor",
    " Included     69% 22d",
    " Auto         74% 22d",
    " 3rd-party    49% 22d",
    "",
    "Kimi",
    " Week        100%  2d",
    " Session     100% 57m",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "",
    "j/k · p prefs · r/q",
  ]);

  const at20 = render(value, { width: 20, height: 12 })
    .split("\n")
    .map((line) => line.trimEnd());
  assert.deepEqual(at20, [
    "Quota         1m ago",
    "! Claude · out 13h",
    "Claude",
    " Session     98%  3h",
    " Week        53% 33h",
    " Fable week   9% 33h",
    " Extra       59%  --",
    "OpenAI Codex",
    " Week        79% 23h",
    " Spark week 100%  7d",
    "Rows 1–8 of 16",
    "j/k · p prefs · r/q",
  ]);
});

test("attention text is truthful for health, partial data, and early projections", async () => {
  const healthy = await report("complete");
  for (const provider of healthy.providers) {
    provider.state.status = "fresh";
    provider.state.stale = false;
    for (const effective of provider.effective) {
      effective.effectivePercentRemaining = 60;
      effective.pace = { status: "on_pace" };
      effective.runway = { status: "through_reset" };
    }
  }
  assert.match(render(healthy), /^= All known limits on pace/m);
  assert.match(
    render(await report("partial-failure")),
    /^\? 2 providers unreadable · 1 tracked/m,
  );
  assert.match(
    render(await report("stale-unknown")),
    /^\? 2 providers unreadable · 0 tracked/m,
  );

  const early = await report("complete");
  early.providers[0].effective[1].runway.projectionConfidence = "early";
  early.providers.find(
    (provider) => provider.provider === "cursor",
  ).effective[0].runway = {
    status: "through_reset",
    projectionConfidence: "established",
  };
  const output = render(early);
  assert.match(output, /^\? Pace unavailable · 4 tracked/m);
});

test("unknown limiting ids render provider-only attention", async () => {
  const value = await report("complete");
  for (const provider of value.providers) {
    for (const effective of provider.effective) {
      effective.effectivePercentRemaining = 60;
      effective.pace = { status: "on_pace" };
      effective.runway = { status: "through_reset" };
    }
  }
  const cursor = value.providers.find(
    (provider) => provider.provider === "cursor",
  );
  cursor.effective[0].effectivePercentRemaining = 0;
  cursor.effective[0].limitingWindowIds = ["internal:future-window"];
  cursor.effective[0].runway = {
    status: "exhausted_now",
    limitingWindowId: "internal:future-window",
  };
  const output = render(value);
  assert.match(output, /^! Cursor · spent/m);
  assert.doesNotMatch(output, /internal|future-window/);
});

test("exhausted attention explains the constraint without color", async () => {
  const exhausted = await report("complete");
  for (const provider of exhausted.providers) {
    for (const effective of provider.effective) {
      effective.effectivePercentRemaining = 60;
      effective.pace = { status: "on_pace" };
      effective.runway = { status: "through_reset" };
    }
  }
  const fable = exhausted.providers[0].effective[1];
  fable.effectivePercentRemaining = 0;
  fable.limitingWindowIds = ["model:fable"];
  fable.runway = {
    status: "exhausted_now",
    limitingWindowId: "model:fable",
    projectionConfidence: "established",
  };
  assert.match(render(exhausted), /^! Claude Fable · spent · resets 33h/m);
});

test("ANSI stays legible in light and dark themes without carrying meaning", async () => {
  const output = renderDashboard(
    { report: await report("complete"), loading: false, scroll: 0 },
    { width: 36, height: 23, now: NOW, color: true },
  );
  assert.ok(output.includes("\x1b[1;31m  9%\x1b[0m"));
  assert.ok(output.includes("\x1b[1;31mout 13h\x1b[0m"));
  assert.ok(output.includes("\x1b[1mout 9d\x1b[0m"));
  assert.ok(output.includes("\x1b[1mClaude\x1b[0m"));
  assert.ok(output.includes("\x1b[1;31m▎\x1b[0m───"));
  const ansiCodes = output
    .split("\x1b[")
    .slice(1)
    .map((sequence) => sequence.split("m")[0]);
  assert.ok(ansiCodes.every((code) => !code.split(";").includes("2")));
  assert.ok(
    ansiCodes.every(
      (code) =>
        !["38;5;220", "38;5;238", "38;5;244", "38;5;251", "38;5;255"].includes(
          code,
        ),
    ),
  );
  assert.match(output, /100% {2}7d · on pace/);
  assert.match(output, /59% \$207 of \$500/);
  assert.match(stripAnsi(output), /Fable week {2}▎─── {3}9% 33h · out 13h/);
  assert.equal(
    stripAnsi(output),
    renderPlain(
      { report: await report("complete"), loading: false, scroll: 0 },
      { width: 36, height: 23, now: NOW },
    ),
  );
});

test("expired exhaustion projections fall back to ahead", async () => {
  const complete = await report("complete");
  const fable = complete.providers
    .find((provider) => provider.provider === "claude")
    .windows.find((window) => window.id === "model:fable");
  fable.pace.projectedExhaustedAt = "2026-08-18T17:59:59.000Z";

  const output = render(complete);
  assert.match(output, /Fable week {2}▎─── {3}9% 33h · ahead/);
  assert.doesNotMatch(output, /out(?: in)? now/);
});

test("history evidence has exact responsive hierarchy at product pane sizes", async () => {
  const value = await report("complete");
  const history = {
    availability: "ready",
    evidence: {
      kind: "pace_worse",
      provider: "Claude",
      scope: "Fable",
      limit: "Fable",
    },
  };
  const at = (width, height) =>
    renderPlain(
      { report: value, history, loading: false, scroll: 0 },
      { width, height, now: NOW },
    )
      .split("\n")
      .map((line) => line.trimEnd());

  assert.deepEqual(at(36, 23), [
    "AI Quota                      1m ago",
    "! Claude Fable · out 13h",
    "↓ Claude Fable · pace worse",
    "Claude",
    " Session     ███▉  98%  3h · on pace",
    " Week        ██──  53% 33h · on pace",
    " Fable week  ▎───   9% 33h · out 13h",
    " Extra usage ██▎─  59% $207 of $500",
    "",
    "OpenAI Codex",
    " Week        ███▏  79% 23h · on pace",
    " Spark week  ████ 100%  7d · on pace",
    " Code review        -- not reported",
    "",
    "Cursor",
    " Included    ██▊─  69% 22d · on pace",
    " Auto        ██▉─  74% 22d · on pace",
    " 3rd-party   █▉──  49% 22d · out 9d",
    "",
    "Kimi",
    " Week        ████ 100%  2d · on pace",
    " Session     ████ 100% 57m · on pace",
    "j/k Pg · p prefs · r · q/esc",
  ]);
  assert.deepEqual(at(24, 12), [
    "Quota             1m ago",
    "! Claude Fable · out 13h",
    "↓ Claude Fable pace",
    "Claude",
    " Session      98%  3h",
    " Week         53% 33h",
    " Fable week    9% 33h",
    " Extra usage  59%  --",
    "OpenAI Codex",
    " Week         79% 23h",
    "Rows 1–7 of 16",
    "j/k · p prefs · r/q",
  ]);
  assert.deepEqual(at(20, 8), [
    "Quota         1m ago",
    "! Claude · out 13h",
    "Claude",
    " Session     98%  3h",
    " Week        53% 33h",
    " Fable week   9% 33h",
    "Rows 1–4 of 16",
    "j/k · p prefs · r/q",
  ]);

  const gain = renderPlain(
    {
      report: value,
      history: {
        availability: "ready",
        evidence: {
          kind: "remaining_gain",
          provider: "Claude",
          scope: "Fable",
          limit: "Fable",
          amount: 17,
        },
      },
      loading: false,
      scroll: 0,
    },
    { width: 36, height: 23, now: NOW },
  );
  assert.match(gain, /^↑ Claude Fable · 17pp gain\s*$/m);
});

test("history keeps every live row reachable at all required widths and heights", async () => {
  const value = await report("complete");
  const history = {
    availability: "ready",
    evidence: {
      kind: "remaining_drop",
      provider: "Claude",
      scope: "Fable",
      limit: "Fable",
      amount: 17,
    },
  };
  for (const width of [20, 24, 36]) {
    const reference = render(value, { width, height: 40 })
      .split("\n")
      .slice(2, -1)
      .map((line) => line.trimEnd())
      .filter(Boolean);

    for (const height of [6, 8, 12, 23]) {
      const state = { report: value, history, loading: false, scroll: 0 };
      const reached = Array(reference.length).fill(false);
      while (true) {
        const metrics = dashboardScrollMetrics(state, height);
        const lines = renderPlain(state, { width, height, now: NOW })
          .split("\n")
          .map((line) => line.trimEnd());
        assert.equal(lines.length, height, `${width}x${height}`);
        assert.match(lines[1], /^! /);
        assert.match(lines.at(-1), /^j\/k/);
        const historyRows = height >= 10 ? 1 : 0;
        if (historyRows) assert.match(lines[2], /^↓ /);
        else assert.doesNotMatch(lines.join("\n"), /17pp/);

        const detailStart = 2 + historyRows;
        const detailEnd = metrics.overflowing ? -2 : -1;
        const visible = lines.slice(detailStart, detailEnd).filter(Boolean);
        assert.deepEqual(
          visible,
          reference.slice(
            metrics.scroll,
            metrics.scroll + metrics.viewportRows,
          ),
          `${width}x${height} at ${metrics.scroll}`,
        );
        for (
          let index = metrics.scroll;
          index < metrics.scroll + metrics.viewportRows;
          index++
        )
          reached[index] = true;
        for (const line of lines)
          assert.ok(line.length <= width, `${width}x${height}: ${line}`);

        if (metrics.scroll === metrics.maxScroll) break;
        applyDashboardScroll(state, "scroll_down", height);
      }
      assert.ok(reached.every(Boolean), `${width}x${height} reachable`);
    }
  }
});

test("history availability notes are finite and collector failures suppress old signals", async () => {
  const value = await report("complete");
  for (const availability of [
    "recovered",
    "clock_skew",
    "incompatible",
    "unavailable",
  ]) {
    const output = renderPlain(
      {
        report: value,
        history: {
          availability,
          raw: "Bearer secret /Users/alice/auth.json",
        },
        loading: false,
        scroll: 0,
      },
      { width: 24, height: 12, now: NOW },
    );
    assert.match(output, /^~ History/m);
    assert.doesNotMatch(output, /Bearer|secret|alice|auth\.json/i);
  }

  const firstRun = renderPlain(
    {
      report: value,
      history: { availability: "first_run" },
      loading: false,
      scroll: 0,
    },
    { width: 24, height: 12, now: NOW },
  );
  assert.doesNotMatch(firstRun, /^~ History/m);

  const failed = renderPlain(
    {
      report: value,
      history: {
        availability: "ready",
        evidence: {
          kind: "pace_worse",
          provider: "Claude",
          scope: "Fable",
          limit: "Fable",
        },
      },
      failure: {
        kind: "network_process",
        retryAt: new Date(NOW.getTime() + 10 * 60_000),
      },
      loading: false,
      scroll: 0,
    },
    { width: 36, height: 12, now: NOW },
  );
  assert.match(failed, /^Network\/process failed/m);
  assert.doesNotMatch(failed, /pace worse|^↓ /m);
});
