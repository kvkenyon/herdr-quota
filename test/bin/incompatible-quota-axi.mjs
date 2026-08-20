#!/usr/bin/env node
process.stdout.write(
  JSON.stringify({
    schemaVersion: 6,
    generatedAt: new Date().toISOString(),
    providers: [],
    raw: "Bearer secret.token.value /home/alice/.codex/auth.json",
  }),
);
