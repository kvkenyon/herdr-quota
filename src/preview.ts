#!/usr/bin/env node
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { adaptQuotaResponse } from "./schema.js";
import { renderPlain } from "./render.js";

const fixture = process.argv[2] ?? "test/fixtures/complete.json";
const width = Number(process.env.COLUMNS ?? 112);
const height = Number(process.env.LINES ?? 40);
const report = adaptQuotaResponse(
  JSON.parse(await readFile(resolve(fixture), "utf8")),
);
process.stdout.write(
  `${renderPlain(
    {
      report,
      history: {
        availability: "ready",
        evidence: {
          kind: "pace_worse",
          provider: "Claude",
          scope: "Fable",
          limit: "Fable",
        },
      },
      loading: false,
      scroll: 0,
    },
    { width, height, now: new Date("2026-08-18T18:00:00.000Z") },
  )}\n`,
);
