import assert from "node:assert/strict";
import test from "node:test";
import {
  actionForInput,
  actionsForInput,
  TerminalInputParser,
} from "../dist/keys.js";
import { ChildProcessTracker } from "../dist/app.js";

test("q and Escape close, r refreshes, and secondary keys are inert", () => {
  assert.equal(actionForInput("q"), "quit");
  assert.equal(actionForInput("\x1b"), "quit");
  assert.equal(actionForInput("r"), "refresh");
  assert.equal(actionForInput("j"), "none");
  assert.equal(actionForInput("\x1b[A"), "none");
  assert.equal(actionForInput("x"), "none");
});

test("fragmented escape sequences are never mistaken for bare Escape", () => {
  const parser = new TerminalInputParser();
  assert.deepEqual(parser.push("\x1b"), []);
  assert.deepEqual(parser.push("["), []);
  assert.deepEqual(parser.push("A"), []);
  assert.deepEqual(parser.push("\x1b"), []);
  assert.deepEqual(parser.flush(), ["quit"]);
});

test("completed refreshes cannot release another refresh child", () => {
  const tracker = new ChildProcessTracker();
  const first = fakeChild();
  const second = fakeChild();
  tracker.update(first, true);
  tracker.update(second, true);
  tracker.update(first, false);
  tracker.terminateAll();
  assert.deepEqual(first.signals, []);
  assert.deepEqual(second.signals, ["SIGTERM"]);
});

function fakeChild() {
  return {
    killed: false,
    exitCode: 0,
    signalCode: null,
    signals: [],
    kill(signal) {
      this.killed = true;
      this.signals.push(signal);
      return true;
    },
  };
}

test("coalesced input preserves repeated essential actions", () => {
  assert.deepEqual(actionsForInput("rrq"), ["refresh", "refresh", "quit"]);
  assert.deepEqual(actionsForInput("jk\x1b[B\x1b[A"), ["none", "none"]);
});
