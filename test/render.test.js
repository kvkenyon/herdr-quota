import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { stripAnsi } from "../dist/ansi.js";
import { isFallbackLogo, providerLogo } from "../dist/logos.js";
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

test("renders the complete wide scan path and all provider labels", async () => {
  const output = renderPlain(
    { report: await report("complete"), loading: false, scroll: 0 },
    { width: 112, height: 40, now: NOW },
  );
  for (const label of ["Claude", "OpenAI Codex", "Cursor", "Kimi"])
    assert.match(output, new RegExp(label));
  assert.match(output, /64% remaining/);
  assert.match(output, /limited by week/);
  assert.match(output, /pace lasts to reset/);
  assert.match(output, /may run out in 1h 12m/);
  assert.match(output, /exhausts in 1h 12m \| confidence established/);
  const projectionLine = output
    .split("\n")
    .find((line) => line.includes("exhausts in 1h 12m"));
  assert.ok(projectionLine);
  assert.doesNotMatch(projectionLine, />/);
  assert.match(output, /bounds: five_hour \+ seven_day/);
});

test("narrow output stays inside its terminal width", async () => {
  const width = 42;
  const output = renderDashboard(
    { report: await report("narrow"), loading: false, scroll: 0 },
    { width, height: 18, now: NOW, color: true },
  );
  for (const line of output.split("\n"))
    assert.ok(
      stripAnsi(line).length <= width,
      `${stripAnsi(line).length}: ${stripAnsi(line)}`,
    );
  assert.match(stripAnsi(output), /Kimi/);
});

test("scrolling never overwrites narrow provider status badges", async () => {
  const output = renderPlain(
    { report: await report("partial-failure"), loading: false, scroll: 0 },
    { width: 54, height: 18, now: NOW },
  );
  assert.match(output, /\[HEALTHY\]/);
  assert.doesNotMatch(output, /\[HEALTH>/);
});

test("partial failures remain independent and sanitized", async () => {
  const output = renderPlain(
    { report: await report("partial-failure"), loading: false, scroll: 0 },
    { width: 72, height: 40, now: NOW },
  );
  assert.match(output, /Claude/);
  assert.match(output, /AUTH REQUIRED/);
  assert.match(output, /UNAVAILABLE/);
  assert.doesNotMatch(output, /credentials_missing|network_unavailable/);
});

test("stale and unknown data do not render zero percent", async () => {
  const output = renderPlain(
    { report: await report("stale-unknown"), loading: false, scroll: 0 },
    { width: 72, height: 32, now: NOW },
  );
  assert.match(output, /STALE/);
  assert.match(output, /UNAVAILABLE/);
  assert.match(output, /-- remaining/);
  assert.doesNotMatch(output, /0% remaining/);
});

test("future providers use a tasteful generic cell logo", async () => {
  assert.equal(isFallbackLogo("future-lab"), true);
  assert.deepEqual(providerLogo("future-lab"), [
    "   #   ",
    "  # #  ",
    " #   # ",
    "  # #  ",
    "   #   ",
  ]);
  const output = renderPlain(
    { report: await report("extra-provider"), loading: false, scroll: 0 },
    { width: 64, height: 22, now: NOW },
  );
  assert.match(output, /Future Lab/);
  assert.ok(output.includes(" #   # "));
});

test("known providers have distinct recognizable monochrome silhouettes", () => {
  assert.deepEqual(providerLogo("claude"), [
    "#  #  #",
    " # # # ",
    "#######",
    " # # # ",
    "#  #  #",
  ]);
  assert.deepEqual(providerLogo("codex"), [
    "  ###  ",
    " ## ## ",
    "## # ##",
    " ## ## ",
    "  ###  ",
  ]);
  assert.deepEqual(providerLogo("cursor"), [
    "#      ",
    "##     ",
    "# #    ",
    "#  #   ",
    "#####  ",
  ]);
  assert.deepEqual(providerLogo("kimi"), [
    "  ###  ",
    " ##    ",
    "##     ",
    " ##    ",
    "  ###  ",
  ]);
});

test("live rendering color-enhances marks while keeping text labels", async () => {
  const output = renderDashboard(
    { report: await report("complete"), loading: false, scroll: 0 },
    { width: 112, height: 40, now: NOW, color: true },
  );
  assert.ok(output.includes("\x1b[38;5;215m#  #  #\x1b[0m"));
  assert.ok(output.includes("\x1b[38;5;81m  ###  \x1b[0m"));
  assert.match(stripAnsi(output), /Claude/);
  assert.match(stripAnsi(output), /OpenAI Codex/);
});
