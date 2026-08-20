import assert from "node:assert/strict";
import test from "node:test";
import { collectQuota } from "../dist/collect.js";
import { CollectorFailureError } from "../dist/failure.js";

const executable = (name) => new URL(`bin/${name}`, import.meta.url).pathname;

test("invokes the selected plugin-local compatible executable with JSON full flags", async () => {
  const report = await collectQuota({
    executable: executable("mock-quota-axi.mjs"),
    timeoutMs: 1000,
  });
  assert.equal(report.providers.length, 4);
});

async function rejectsAs(kind, options) {
  await assert.rejects(collectQuota(options), (error) => {
    assert.ok(error instanceof CollectorFailureError);
    assert.equal(error.kind, kind);
    assert.doesNotMatch(
      error.message,
      /secret|alice|example|auth\.json|api-key/i,
    );
    assert.equal(error.message.includes("\u001b"), false);
    return true;
  });
}

test("bounds a stalled quota-axi process with an allow-listed timeout", async () => {
  const started = Date.now();
  await rejectsAs("timeout", {
    executable: executable("slow-quota-axi.mjs"),
    timeoutMs: 50,
  });
  assert.ok(Date.now() - started < 1000);
});

test("distinguishes a missing plugin-local executable without its path", async () => {
  await rejectsAs("missing_executable", {
    executable: executable("does-not-exist-quota-axi"),
    timeoutMs: 1000,
  });
});

test("classifies changed/schema output without exposing the payload", async () => {
  await rejectsAs("incompatible_output", {
    executable: executable("incompatible-quota-axi.mjs"),
    timeoutMs: 1000,
  });
});

test("suppresses arbitrary ANSI, control, path, credential, and process output", async () => {
  await rejectsAs("network_process", {
    executable: executable("failing-quota-axi.mjs"),
    timeoutMs: 1000,
  });
});
