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

test("36-cell sidebar presents the complete two-second scan without scrolling", async () => {
  const output = renderPlain(
    { report: await report("complete"), loading: false, scroll: 0 },
    { width: 36, height: 23, now: NOW },
  );
  for (const label of ["Claude", "OpenAI Codex", "Cursor", "Kimi"])
    assert.match(output, new RegExp(label));
  assert.match(output, /64% left/);
  assert.match(output, /in 4d 0h/);
  assert.match(output, /on pace/);
  assert.match(output, /may run out in 1h 12m/);
  assert.match(output, /^r refresh · q\/esc close$/m);
  assert.doesNotMatch(
    output,
    /plan|source|credits|confidence|WINDOWS|bounds:/i,
  );
  for (const line of output.split("\n")) assert.ok(line.length <= 36, line);
});

test("representative slim and narrow widths never wrap or clip health text", async () => {
  for (const width of [38, 32, 24, 20]) {
    const output = renderDashboard(
      { report: await report("partial-failure"), loading: false, scroll: 0 },
      { width, height: 23, now: NOW, color: true },
    );
    const plain = stripAnsi(output);
    for (const line of plain.split("\n"))
      assert.ok(line.length <= width, `${width}: ${line}`);
    assert.match(plain, /AUTH/);
    assert.match(plain, /UNAVAILABLE/);
    for (const line of plain.split("\n"))
      assert.equal(line.trimEnd().endsWith(">"), false, line);
  }
});

test("provider failures remain independent and unknown stays unknown", async () => {
  const partial = renderPlain(
    { report: await report("partial-failure"), loading: false, scroll: 0 },
    { width: 36, height: 23, now: NOW },
  );
  assert.match(partial, /Claude OK/);
  assert.match(partial, /Cursor AUTH/);
  assert.match(partial, /sign-in required/);
  assert.match(partial, /Kimi UNAVAILABLE/);
  assert.doesNotMatch(partial, /credentials_missing|network_unavailable/);

  const unknown = renderPlain(
    { report: await report("stale-unknown"), loading: false, scroll: 0 },
    { width: 36, height: 23, now: NOW },
  );
  assert.match(unknown, /STALE/);
  assert.match(unknown, /-- left/);
  assert.doesNotMatch(unknown, /0% left/);
});

test("provider marks are distinct ASCII accents no larger than 3x2", () => {
  const marks = ["claude", "codex", "cursor", "kimi"].map(providerLogo);
  assert.equal(new Set(marks.map((mark) => mark.join("\n"))).size, 4);
  for (const mark of marks) {
    assert.equal(mark.length, 2);
    assert.ok(mark.every((line) => line.length <= 3));
  }
  assert.equal(isFallbackLogo("future-lab"), true);
  assert.deepEqual(providerLogo("future-lab"), [" . ", "/ \\"]);
});

test("live rendering adds color while labels retain meaning", async () => {
  const output = renderDashboard(
    { report: await report("complete"), loading: false, scroll: 0 },
    { width: 36, height: 23, now: NOW, color: true },
  );
  assert.ok(output.includes("\x1b[38;5;215m\\|/\x1b[0m"));
  assert.ok(output.includes("\x1b[38;5;81mo-o\x1b[0m"));
  assert.match(stripAnsi(output), /Claude OK/);
  assert.match(stripAnsi(output), /OpenAI Codex OK/);
});
