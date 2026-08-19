#!/usr/bin/env node
import { DashboardApp } from "./app.js";
import { sanitizeProcessError } from "./sanitize.js";

const app = new DashboardApp();
try {
  await app.run();
} catch (error) {
  app.close();
  process.stderr.write(`${sanitizeProcessError(error)}\n`);
  process.exitCode = 1;
}
