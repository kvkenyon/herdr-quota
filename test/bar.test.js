import assert from "node:assert/strict";
import test from "node:test";
import { remainingBar } from "../dist/bar.js";

test("a solid bar means exactly full and an empty track exactly empty", () => {
  assert.equal(remainingBar(100, 4), "████");
  assert.equal(remainingBar(0, 4), "────");
  // A hair under full still shows a notch, a hair over empty still shows a
  // sliver, so neither extreme can be claimed by rounding.
  assert.equal(remainingBar(99.9, 4), "███▉");
  assert.equal(remainingBar(0.1, 4), "▏───");
});

test("partial cells separate percentages a whole cell would collapse", () => {
  assert.notEqual(remainingBar(69, 4), remainingBar(74, 4));
  assert.equal(remainingBar(69, 4), "██▊─");
  assert.equal(remainingBar(74, 4), "██▉─");
  assert.equal(remainingBar(50, 4), "██──");
  assert.equal(remainingBar(25, 4), "█───");
});

test("an unknown reading draws nothing and cannot look exhausted", () => {
  assert.equal(remainingBar(undefined, 4), "    ");
  assert.equal(remainingBar(Number.NaN, 4), "    ");
  assert.notEqual(remainingBar(undefined, 4), remainingBar(0, 4));
});

test("bars keep their exact width at every size and value", () => {
  for (const width of [1, 2, 4, 6, 10]) {
    for (let percent = -20; percent <= 120; percent += 0.5) {
      const bar = remainingBar(percent, width);
      assert.equal(bar.length, width, `${percent}% at ${width}`);
      assert.match(bar, /^[█▏▎▍▌▋▊▉─]+$/);
    }
  }
  assert.equal(remainingBar(50, 0), "");
  assert.equal(remainingBar(50, -1), "");
});

test("a wider bar never shows less than a narrower one", () => {
  const filled = (bar) => [...bar].filter((cell) => cell !== "─").length;
  for (let percent = 1; percent <= 100; percent += 1) {
    const narrow = filled(remainingBar(percent, 4)) / 4;
    const wide = filled(remainingBar(percent, 10)) / 10;
    assert.ok(
      Math.abs(narrow - wide) <= 0.25,
      `${percent}%: ${narrow} vs ${wide}`,
    );
  }
});
