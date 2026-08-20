import assert from "node:assert/strict";
import test from "node:test";
import { setImmediate as waitForImmediate } from "node:timers/promises";
import {
  FAILURE_BACKOFF_MS,
  NORMAL_REFRESH_MS,
  RefreshScheduler,
} from "../dist/refresh.js";

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

function fakeTimers() {
  let current;
  const cleared = [];
  return {
    setTimer(callback, delayMs) {
      const timer = {
        callback() {
          if (current === timer) current = undefined;
          callback();
        },
        delayMs,
      };
      current = timer;
      return current;
    },
    clearTimer(timer) {
      cleared.push(timer);
      if (current === timer) current = undefined;
    },
    current: () => current,
    cleared,
  };
}

async function flush() {
  await waitForImmediate();
}

test("refresh is completion-relative and never overlaps automatically", async () => {
  const timers = fakeTimers();
  const attempts = [];
  const scheduler = new RefreshScheduler({
    collect() {
      const attempt = deferred();
      attempts.push(attempt);
      return attempt.promise;
    },
    onStart() {},
    onSuccess() {},
    onFailure() {},
    onSettled() {},
    cancelActive() {},
    setTimer: timers.setTimer,
    clearTimer: timers.clearTimer,
  });

  const initial = scheduler.start();
  assert.equal(attempts.length, 1);
  assert.equal(timers.current(), undefined);
  attempts[0].resolve("first");
  await initial;
  assert.equal(timers.current().delayMs, NORMAL_REFRESH_MS);

  timers.current().callback();
  assert.equal(attempts.length, 2);
  assert.equal(timers.current(), undefined);
  await flush();
  assert.equal(attempts.length, 2);
  attempts[1].resolve("second");
  await flush();
  assert.equal(timers.current().delayMs, NORMAL_REFRESH_MS);
  scheduler.close();
});

test("manual refresh preempts an attempt and resets normal scheduling", async () => {
  const timers = fakeTimers();
  const attempts = [];
  let cancellations = 0;
  const successes = [];
  const scheduler = new RefreshScheduler({
    collect() {
      const attempt = deferred();
      attempts.push(attempt);
      return attempt.promise;
    },
    onStart() {},
    onSuccess(value) {
      successes.push(value);
    },
    onFailure() {},
    onSettled() {},
    cancelActive() {
      cancellations++;
    },
    setTimer: timers.setTimer,
    clearTimer: timers.clearTimer,
  });

  const first = scheduler.start();
  const manual = scheduler.manual();
  assert.equal(cancellations, 1);
  assert.equal(attempts.length, 2);
  attempts[0].resolve("superseded");
  attempts[1].resolve("manual");
  await Promise.all([first, manual]);
  assert.deepEqual(successes, ["manual"]);
  assert.equal(timers.current().delayMs, NORMAL_REFRESH_MS);
  scheduler.close();
});

test("whole-collector failures back off 10, 20, then 30 minutes", async () => {
  const timers = fakeTimers();
  let attempt = 0;
  let lastGood = "previous";
  let failures = 0;
  let cancellations = 0;
  const scheduled = [];
  const scheduler = new RefreshScheduler({
    collect() {
      attempt++;
      return attempt <= 4
        ? Promise.reject(new Error(`failure ${attempt}`))
        : Promise.resolve("fresh");
    },
    onStart() {},
    onSuccess(value) {
      lastGood = value;
    },
    onFailure() {
      failures++;
    },
    onScheduled(delayMs, afterFailure) {
      scheduled.push([delayMs, afterFailure]);
    },
    onSettled() {},
    cancelActive() {
      cancellations++;
    },
    setTimer: timers.setTimer,
    clearTimer: timers.clearTimer,
  });

  await scheduler.start();
  assert.equal(lastGood, "previous");
  assert.equal(timers.current().delayMs, FAILURE_BACKOFF_MS[0]);
  for (const expected of [
    FAILURE_BACKOFF_MS[1],
    FAILURE_BACKOFF_MS[2],
    FAILURE_BACKOFF_MS[2],
  ]) {
    timers.current().callback();
    await flush();
    assert.equal(timers.current().delayMs, expected);
    assert.equal(lastGood, "previous");
  }
  assert.equal(failures, 4);
  assert.deepEqual(scheduled.slice(0, 4), [
    [FAILURE_BACKOFF_MS[0], true],
    [FAILURE_BACKOFF_MS[1], true],
    [FAILURE_BACKOFF_MS[2], true],
    [FAILURE_BACKOFF_MS[2], true],
  ]);

  await scheduler.manual();
  assert.equal(lastGood, "fresh");
  assert.equal(timers.current().delayMs, NORMAL_REFRESH_MS);
  assert.deepEqual(scheduled.at(-1), [NORMAL_REFRESH_MS, false]);
  assert.equal(cancellations, 1);
  scheduler.close();
});

test("manual retry resets failure backoff before scheduling its result", async () => {
  const timers = fakeTimers();
  const scheduled = [];
  const scheduler = new RefreshScheduler({
    collect: () => Promise.reject(new Error("failed")),
    onStart() {},
    onSuccess() {},
    onFailure() {},
    onScheduled(delayMs, afterFailure) {
      scheduled.push([delayMs, afterFailure]);
    },
    onSettled() {},
    cancelActive() {},
    setTimer: timers.setTimer,
    clearTimer: timers.clearTimer,
  });

  await scheduler.start();
  timers.current().callback();
  await flush();
  assert.deepEqual(scheduled.slice(-1)[0], [FAILURE_BACKOFF_MS[1], true]);

  await scheduler.manual();
  assert.deepEqual(scheduled.at(-1), [FAILURE_BACKOFF_MS[0], true]);
  scheduler.close();
});

test("close clears the timer, cancels children, and ignores late results", async () => {
  const timers = fakeTimers();
  const attempt = deferred();
  const events = [];
  let cancellations = 0;
  const scheduler = new RefreshScheduler({
    collect: () => attempt.promise,
    onStart: () => events.push("start"),
    onSuccess: () => events.push("success"),
    onFailure: () => events.push("failure"),
    onSettled: () => events.push("settled"),
    cancelActive: () => cancellations++,
    setTimer: timers.setTimer,
    clearTimer: timers.clearTimer,
  });

  const running = scheduler.start();
  scheduler.close();
  attempt.resolve("late");
  await running;
  assert.deepEqual(events, ["start"]);
  assert.equal(timers.current(), undefined);
  assert.equal(cancellations, 1);

  // Closing again is idempotent and cannot kill another process.
  scheduler.close();
  assert.equal(cancellations, 1);
});

test("successful async post-collection work completes before scheduling", async () => {
  const timers = fakeTimers();
  const persisted = deferred();
  const events = [];
  const scheduler = new RefreshScheduler({
    collect: async () => "fresh",
    async onSuccess() {
      events.push("history-start");
      await persisted.promise;
      events.push("history-done");
    },
    onStart() {},
    onFailure() {},
    onSettled() {
      events.push("settled");
    },
    cancelActive() {},
    setTimer: timers.setTimer,
    clearTimer: timers.clearTimer,
  });

  const running = scheduler.start();
  await flush();
  assert.deepEqual(events, ["history-start"]);
  assert.equal(timers.current(), undefined);
  persisted.resolve();
  await running;
  assert.deepEqual(events, ["history-start", "history-done", "settled"]);
  assert.equal(timers.current().delayMs, NORMAL_REFRESH_MS);
  scheduler.close();
});
