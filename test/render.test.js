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
  for (const label of ["Claude", "OpenAI Codex", "Cursor", "Kimi"])
    assert.match(output, new RegExp(label));
  assert.match(output, /Session {5,}98% {1,2}3h · on pace/);
  assert.match(output, /Week {8,}53% 33h · on pace/);
  assert.match(output, /Fable week {3,}9% 33h · out in 13h/);
  assert.match(output, /Extra usage {2}59% {2}-- · \$207 of \$500/);
  assert.match(output, /Week {8,}79% 23h · on pace/);
  assert.match(output, /Spark week {2}100% {2}7d · on pace/);
  assert.match(output, /Code review {3}-- {2}not reported/);
  assert.match(output, /Included {10}69% 22d · on pace/);
  assert.match(output, /Auto {14}74% 22d · on pace/);
  assert.match(output, /3rd-party models {2}49% 22d · out 9d/);
  assert.match(output, /Week {8,}100% {2}2d · on pace/);
  assert.match(output, /Session {5}100% 57m · on pace/);
  assert.match(output, /^r refresh · q\/esc close$/m);
  assert.doesNotMatch(output, /more rows/);
  for (const line of output.split("\n")) assert.ok(line.length <= 36, line);
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
  assert.match(output, /Spark week {2}100%/);
  assert.match(output, /3rd-party models {2}49%/);
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
  assert.match(output, /Week {8,}24% 42h · --/);
  assert.match(output, /Included {4,}-- {2}-- · --/);
  assert.doesNotMatch(output, /0%/);
});

test("narrow widths degrade honestly without wrapping or clipping", async () => {
  for (const width of [38, 32, 29, 24, 20]) {
    for (const fixture of ["complete", "mixed-auth"]) {
      const output = renderDashboard(
        { report: await report(fixture), loading: false, scroll: 0 },
        { width, height: 30, now: NOW, color: true },
      );
      const plain = stripAnsi(output);
      for (const line of plain.split("\n")) {
        assert.ok(line.length <= width, `${fixture}@${width}: ${line}`);
        assert.equal(line.trimEnd().endsWith(">"), false, line);
      }
      assert.match(plain, /Claude/);
      assert.match(plain, /Kimi/);
    }
  }
  const narrow = renderPlain(
    { report: await report("complete"), loading: false, scroll: 0 },
    { width: 24, height: 30, now: NOW },
  );
  assert.match(narrow, /3rd-party {2,}49% 22d/);
});

test("short panes cut whole rows and say how much is hidden", async () => {
  const output = render(await report("complete"), { height: 12 });
  assert.equal(output.split("\n").length, 12);
  assert.match(output, /\+\d+ more rows/);
});

test("color emphasizes risk while text keeps the meaning", async () => {
  const output = renderDashboard(
    { report: await report("complete"), loading: false, scroll: 0 },
    { width: 36, height: 23, now: NOW, color: true },
  );
  assert.ok(output.includes("\x1b[38;5;203m  9%\x1b[0m"));
  assert.ok(output.includes("\x1b[38;5;203mout in 13h\x1b[0m"));
  assert.ok(output.includes("\x1b[38;5;220mout 9d\x1b[0m"));
  assert.ok(output.includes("\x1b[1mClaude\x1b[0m"));
  assert.match(stripAnsi(output), /Fable week {3,}9% 33h · out in 13h/);
});
