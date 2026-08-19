import assert from "node:assert/strict";
import test from "node:test";
import {
  friendlyProviderError,
  sanitizeProcessError,
} from "../dist/sanitize.js";

test("sanitizes tokens, accounts, paths, and multiline process errors", () => {
  const raw =
    "Bearer secret.token.value\n/home/alice/.codex/auth.json alice@example.com api-key-abcdefghijk";
  const clean = sanitizeProcessError(raw);
  assert.doesNotMatch(clean, /secret|alice|example|abcdefghijk|\n/);
  assert.match(clean, /redacted/);
});

test("strips terminal control sequences while preserving readable text", () => {
  const raw =
    "before\u001b[2J\u001b[31mred\u001b[0m\u001b]0;secret title\u0007after\u0000\u009b31m";
  const clean = sanitizeProcessError(raw);

  assert.equal(clean, "beforeredafter31m");
  assert.doesNotMatch(clean, /[\u0000-\u0008\u000b-\u001f\u007f-\u009f]/);
});

test("maps known provider codes and suppresses arbitrary raw messages", () => {
  assert.equal(
    friendlyProviderError("request_timeout"),
    "Provider request timed out",
  );
  assert.equal(
    friendlyProviderError("token=secret-account-detail"),
    "Quota unavailable",
  );
});
