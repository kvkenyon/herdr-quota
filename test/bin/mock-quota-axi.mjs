#!/usr/bin/env node
import { readFile } from "node:fs/promises";

const expected = "--json --full --provider claude,codex,cursor,kimi";
if (process.argv.slice(2).join(" ") !== expected) process.exit(9);
process.stdout.write(
  await readFile(new URL("../fixtures/complete.json", import.meta.url), "utf8"),
);
