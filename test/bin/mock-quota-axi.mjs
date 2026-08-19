#!/usr/bin/env node
import { readFile } from "node:fs/promises";

if (process.argv.slice(2).join(" ") !== "--json --full") process.exit(9);
process.stdout.write(
  await readFile(new URL("../fixtures/complete.json", import.meta.url), "utf8"),
);
