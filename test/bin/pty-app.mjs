import { appendFile, readFile } from "node:fs/promises";
import { DashboardApp } from "../../dist/app.js";
import { LocalHistory } from "../../dist/history.js";
import { adaptQuotaResponse } from "../../dist/schema.js";
import { SettingsStore } from "../../dist/settings.js";
import { LocalTransitions } from "../../dist/transitions.js";

const fixture = adaptQuotaResponse(
  JSON.parse(
    await readFile(
      new URL("../fixtures/complete.json", import.meta.url),
      "utf8",
    ),
  ),
);
const countPath = process.env.TEST_COLLECT_COUNT_PATH;
const settingsPath = process.env.TEST_SETTINGS_PATH;
const historyPath = process.env.TEST_HISTORY_PATH;
const transitionPath = process.env.TEST_TRANSITION_PATH;
if (!countPath || !settingsPath || !historyPath || !transitionPath)
  throw new Error("PTY test paths are required");

const app = new DashboardApp({
  async collect() {
    await appendFile(countPath, "collect\n");
    return structuredClone(fixture);
  },
  history: new LocalHistory(historyPath),
  settings: new SettingsStore(settingsPath),
  transitions: new LocalTransitions(transitionPath),
});

try {
  await app.run();
} catch (error) {
  app.close();
  process.stderr.write(
    `${error instanceof Error ? error.message : "PTY app failed"}\n`,
  );
  process.exitCode = 1;
}
