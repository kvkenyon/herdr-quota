#!/usr/bin/env node
process.stderr.write(
  "\u001b[2JBearer secret.token.value from /home/alice/.codex/auth.json " +
    "alice@example.com api-key-abcdefghijk\u001b]0;private\u0007\u0000\n",
);
process.exit(2);
