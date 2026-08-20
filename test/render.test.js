import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { stripAnsi } from "../dist/ansi.js";
import { renderDashboard, renderPlain } from "../dist/render.js";
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
  assert.match(output, /^r refresh · q\/esc close$/m);
  assert.doesNotMatch(output, /more rows/);
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

test("grok, copilot, and unknown providers are never rendered", async () => {
  const output = render(await report("excluded-providers"));
  assert.match(output, /Claude/);
  assert.match(output, /Kimi/);
  assert.doesNotMatch(output, /Grok|Copilot|Future/i);
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

test("short panes cut whole rows and say how much is hidden", async () => {
  const output = render(await report("complete"), { height: 12 });
  assert.equal(output.split("\n").length, 12);
  assert.match(output, /^! Claude Fable · out 13h/m);
  assert.match(output, /^\+8 more rows/m);
  assert.doesNotMatch(output, /^\s*$/m);
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
    "r refresh · q/esc close",
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
    "+8 more rows",
    "r refresh · q close",
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

test("color emphasizes risk while text keeps the meaning", async () => {
  const output = renderDashboard(
    { report: await report("complete"), loading: false, scroll: 0 },
    { width: 36, height: 23, now: NOW, color: true },
  );
  assert.ok(output.includes("\x1b[38;5;203m  9%\x1b[0m"));
  assert.ok(output.includes("\x1b[38;5;203mout 13h\x1b[0m"));
  assert.ok(output.includes("\x1b[38;5;220mout 9d\x1b[0m"));
  assert.ok(output.includes("\x1b[1mClaude\x1b[0m"));
  // An at-risk gauge takes the tone of its own percentage, a healthy one is
  // drawn quieter than that percentage, and the track recedes behind both.
  assert.ok(output.includes("\x1b[38;5;203m▎\x1b[0m\x1b[38;5;238m───\x1b[0m"));
  assert.ok(output.includes("\x1b[38;5;251m██▎\x1b[0m\x1b[38;5;238m─\x1b[0m"));
  for (const glyph of "█▏▎▍▌▋▊▉")
    assert.ok(!output.includes(`\x1b[38;5;255m${glyph}`), glyph);
  assert.match(stripAnsi(output), /Fable week {2}▎─── {3}9% 33h · out 13h/);
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
