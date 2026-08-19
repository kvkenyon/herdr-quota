import assert from "node:assert/strict";
import test from "node:test";
import { collectQuota } from "../dist/collect.js";

const executable = (name) => new URL(`bin/${name}`, import.meta.url).pathname;

test("invokes the selected plugin-local compatible executable with JSON full flags", async () => {
  const report = await collectQuota({
    executable: executable("mock-quota-axi.mjs"),
    timeoutMs: 1000,
  });
  assert.equal(report.providers.length, 4);
});

test("bounds a stalled quota-axi process", async () => {
  const started = Date.now();
  await assert.rejects(
    collectQuota({
      executable: executable("slow-quota-axi.mjs"),
      timeoutMs: 50,
    }),
    /timed out after 1s/,
  );
  assert.ok(Date.now() - started < 1000);
});

test("sanitizes child-process failures", async () => {
  await assert.rejects(
    collectQuota({
      executable: executable("failing-quota-axi.mjs"),
      timeoutMs: 1000,
    }),
    (error) => {
      assert.doesNotMatch(error.message, /secret|alice|example/);
      return true;
    },
  );
});
