import assert from "node:assert/strict";
import test from "node:test";
import { actionForInput, actionsForInput } from "../dist/keys.js";

test("q and Escape close, r refreshes, arrows and vim keys scroll", () => {
  assert.equal(actionForInput("q"), "quit");
  assert.equal(actionForInput("\x1b"), "quit");
  assert.equal(actionForInput("r"), "refresh");
  assert.equal(actionForInput("j"), "scroll-down");
  assert.equal(actionForInput("\x1b[A"), "scroll-up");
  assert.equal(actionForInput("x"), "none");
});

test("coalesced terminal input preserves repeated actions", () => {
  assert.deepEqual(actionsForInput("jjkr"), [
    "scroll-down",
    "scroll-down",
    "scroll-up",
    "refresh",
  ]);
  assert.deepEqual(actionsForInput("\x1b[B\x1b[A"), [
    "scroll-down",
    "scroll-up",
  ]);
});
