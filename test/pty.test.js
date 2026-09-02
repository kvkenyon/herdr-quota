import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import {
  chmod,
  mkdir,
  mkdtemp,
  readFile,
  rm,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import test from "node:test";
import { promisify } from "node:util";
import { defaultSettings } from "../dist/settings.js";
import { adaptQuotaResponse } from "../dist/schema.js";
import {
  HISTORY_SCHEMA_VERSION,
  normalizeHistorySnapshot,
} from "../dist/history.js";
import {
  TRANSITION_SCHEMA_VERSION,
  evaluateTransitions,
} from "../dist/transitions.js";

const execFileAsync = promisify(execFile);
const ROOT = resolve(new URL("..", import.meta.url).pathname);

async function drive(width, height, steps, setup) {
  const directory = await mkdtemp(join(tmpdir(), "herdr-quota-pty-"));
  const settingsPath = join(directory, "config", "settings.json");
  const countPath = join(directory, "collect-count.txt");
  const historyPath = join(directory, "state", "history-v1.json");
  const transitionPath = join(directory, "state", "transitions-v1.json");
  await writeFile(countPath, "");
  if (setup?.settings) {
    await mkdir(join(directory, "config"), { recursive: true });
    await writeFile(settingsPath, `${JSON.stringify(setup.settings)}\n`, {
      mode: 0o600,
    });
  } else if (setup?.settingsText) {
    await mkdir(join(directory, "config"), { recursive: true });
    await writeFile(settingsPath, setup.settingsText, { mode: 0o400 });
  } else if (setup?.unsafeTarget) {
    await mkdir(settingsPath, { recursive: true });
  }
  if (setup?.history) {
    await mkdir(join(directory, "state"), { recursive: true });
    await writeFile(historyPath, `${JSON.stringify(setup.history)}\n`, {
      mode: 0o600,
    });
  }
  if (setup?.transitions) {
    await mkdir(join(directory, "state"), { recursive: true });
    await writeFile(transitionPath, `${JSON.stringify(setup.transitions)}\n`, {
      mode: 0o600,
    });
  }
  const { stdout } = await execFileAsync(
    "python3",
    [
      "test/bin/pty-driver.py",
      String(width),
      String(height),
      settingsPath,
      countPath,
      JSON.stringify(steps),
      ROOT,
      historyPath,
      transitionPath,
    ],
    { cwd: ROOT, timeout: 15_000, maxBuffer: 2 * 1024 * 1024 },
  );
  const result = JSON.parse(stdout);
  result.settingsPath = settingsPath;
  result.historyPath = historyPath;
  result.transitionPath = transitionPath;
  result.collectCount = (await readFile(countPath, "utf8"))
    .split("\n")
    .filter(Boolean).length;
  result.cleanup = () => rm(directory, { recursive: true, force: true });
  return result;
}

async function transitionJourneySetup() {
  const report = adaptQuotaResponse(
    JSON.parse(
      await readFile(
        new URL("fixtures/complete.json", import.meta.url),
        "utf8",
      ),
    ),
  );
  const previous = normalizeHistorySnapshot(
    report,
    new Date(Date.now() - 60_000),
  );
  assert.ok(previous);
  const risky = previous.providers
    .flatMap((provider) => provider.facts)
    .find((fact) => fact.remaining <= 25);
  assert.ok(risky);
  risky.remaining = 40;
  risky.runway = { state: "through_reset" };
  const history = {
    schemaVersion: HISTORY_SCHEMA_VERSION,
    snapshots: [previous],
  };
  const settings = { ...defaultSettings(), remainingThreshold: 25 };
  const transitions = evaluateTransitions(
    { schemaVersion: TRANSITION_SCHEMA_VERSION, events: [] },
    history,
    settings,
  ).document;
  return { settings, history, transitions };
}

test("real PTY rendering is exact and bounded at every required size", async (context) => {
  if (process.platform === "win32") {
    context.skip("POSIX PTYs are required");
    return;
  }
  for (const width of [20, 24, 36]) {
    for (const height of [6, 8, 12, 23]) {
      const result = await drive(width, height, [{ text: "q" }]);
      try {
        const screen = result.screens[0];
        const lines = screen.split("\n");
        assert.equal(lines.length, height, `${width}x${height}`);
        assert.match(lines[0], /Quota/);
        assert.match(lines.at(-1), /p prefs/);
        assert.doesNotMatch(screen, /History starts|→/);
        for (const line of lines)
          assert.ok(line.length <= width, `${width}x${height}: ${line}`);
        assert.equal(result.collectCount, 1);
        assert.equal(result.exitCode, 0);
      } finally {
        await result.cleanup();
      }
    }
  }
});

test("Preferences saves immediately, persists across reopen, and never overlaps collectors", async () => {
  const result = await drive(20, 6, [
    { text: "p" },
    { text: "jjjjj" },
    { text: " " },
    { text: "s" },
    { text: "q" },
  ]);
  try {
    assert.match(result.screens[1], /^Prefs/m);
    assert.match(result.screens[2], /> Meter: remaining/);
    assert.match(result.screens[3], /> Meter: used/);
    assert.match(result.screens[4].split("\n")[0], /Quota · used/);
    assert.match(result.screens[4], /Claude/);
    assert.equal(result.collectCount, 1);
    const saved = JSON.parse(await readFile(result.settingsPath, "utf8"));
    assert.equal(saved.meterMode, "used");

    const reopened = await drive(20, 6, [{ text: "q" }], {
      settings: saved,
    });
    try {
      assert.match(reopened.screens[0].split("\n")[0], /Quota · used/);
      assert.equal(reopened.collectCount, 1);
    } finally {
      await reopened.cleanup();
    }
  } finally {
    await result.cleanup();
  }
});

test("cancel discards the draft and fragmented arrows keep Preferences open", async () => {
  const result = await drive(24, 8, [
    { text: "p" },
    { text: " " },
    { text: "c" },
    { text: "p" },
    { hex: "1b", settle: false, delay: 0.005 },
    { text: "[", settle: false, delay: 0.005 },
    { text: "B" },
    { text: "q" },
  ]);
  try {
    assert.match(result.screens[2], /\[ \] Claude/);
    assert.match(result.screens[3], /Claude/);
    assert.doesNotMatch(result.screens[3], /hidden/);
    assert.match(result.screens[4], /^Preferences/m);
    assert.match(result.screens[5], /> 2 \[x\] OpenAI Codex/);
    await assert.rejects(() => readFile(result.settingsPath, "utf8"), {
      code: "ENOENT",
    });
    assert.equal(result.collectCount, 1);
  } finally {
    await result.cleanup();
  }
});

test("reset confirmation changes only the draft until an explicit save", async () => {
  const custom = {
    ...defaultSettings(),
    hiddenProviders: ["claude", "kimi"],
    meterMode: "used",
  };
  const result = await drive(
    20,
    6,
    [
      { text: "p" },
      { text: "x" },
      { text: "n" },
      { text: "x" },
      { text: "y" },
      { text: "s" },
      { text: "q" },
    ],
    { settings: custom },
  );
  try {
    assert.match(result.screens[2], /Reset draft\?/);
    assert.match(result.screens[2], /Save still required/);
    assert.match(result.screens[4], /Reset draft\?/);
    assert.match(result.screens[5], /Reset defaults/);
    assert.deepEqual(
      JSON.parse(await readFile(result.settingsPath, "utf8")),
      defaultSettings(),
    );
    assert.equal(result.collectCount, 1);
  } finally {
    await result.cleanup();
  }
});

test("settings I/O failure stays finite and leaves the live collector/report intact", async () => {
  const result = await drive(
    20,
    8,
    [{ text: "p" }, { text: "s" }, { hex: "1b" }, { text: "q" }],
    { unsafeTarget: true },
  );
  try {
    assert.match(result.screens[2], /Save failed/);
    assert.match(result.screens[3], /Claude/);
    assert.equal(result.collectCount, 1);
  } finally {
    await result.cleanup();
  }
});

test("real PTY reviews and acknowledges a persisted transition without adding a line", async () => {
  const setup = await transitionJourneySetup();
  const result = await drive(
    24,
    8,
    [
      { text: "", delay: 0.3 },
      { text: "a" },
      { text: "a", delay: 0.3 },
      { text: "q" },
    ],
    setup,
  );
  try {
    assert.match(result.screens[1].split("\n")[0], /Quota.*!/);
    assert.match(result.screens[1], /a alert/);
    assert.match(result.screens[2], /crossed 25%/);
    assert.match(result.screens[2], /% left/);
    assert.match(result.screens[2].split("\n").at(-1), /a\/enter ack/);
    assert.doesNotMatch(result.screens[3].split("\n")[0], /!/);
    assert.doesNotMatch(result.screens[3], /a alert/);
    const stored = JSON.parse(await readFile(result.transitionPath, "utf8"));
    assert.ok(
      stored.events.find((event) => event.kind === "threshold_enter")
        ?.acknowledgedAt,
    );
    assert.equal(result.collectCount, 1);
  } finally {
    await result.cleanup();
  }
});

test("Preferences clear confirmation preserves quota history and read-only future settings", async () => {
  const setup = await transitionJourneySetup();
  const futureSettings = '{"schemaVersion":99,"future":"keep"}\n';
  const result = await drive(
    24,
    8,
    [
      { text: "", delay: 0.3 },
      { text: "p" },
      { text: "jjjjjjjjjjj" },
      { text: "\r" },
      { text: "y", delay: 0.3 },
      { text: "q" },
    ],
    { ...setup, settings: undefined, settingsText: futureSettings },
  );
  try {
    assert.match(result.screens[3], /Clear transition hist/);
    assert.match(result.screens[4], /Clear transition/);
    assert.match(result.screens[4], /Quota history stays/);
    assert.match(result.screens[4].split("\n").at(-1), /y clear/);
    assert.match(result.screens[5], /Transition history/);
    assert.equal(await readFile(result.settingsPath, "utf8"), futureSettings);
    const quotaHistory = JSON.parse(await readFile(result.historyPath, "utf8"));
    assert.ok(quotaHistory.snapshots.length >= 1);
    await assert.rejects(() => readFile(result.transitionPath, "utf8"), {
      code: "ENOENT",
    });
    assert.equal(result.collectCount, 1);
  } finally {
    await chmod(result.settingsPath, 0o600).catch(() => undefined);
    await result.cleanup();
  }
});
