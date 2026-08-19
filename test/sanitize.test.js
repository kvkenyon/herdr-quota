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
