import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import test from "node:test";
import { promisify } from "node:util";
import { defaultSettings } from "../dist/settings.js";

const execFileAsync = promisify(execFile);
const ROOT = resolve(new URL("..", import.meta.url).pathname);

async function drive(width, height, steps, setup) {
  const directory = await mkdtemp(join(tmpdir(), "herdr-quota-pty-"));
  const settingsPath = join(directory, "config", "settings.json");
  const countPath = join(directory, "collect-count.txt");
  await writeFile(countPath, "");
  if (setup?.settings) {
    await mkdir(join(directory, "config"), { recursive: true });
    await writeFile(settingsPath, `${JSON.stringify(setup.settings)}\n`, {
      mode: 0o600,
    });
  } else if (setup?.unsafeTarget) {
    await mkdir(settingsPath, { recursive: true });
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
    ],
    { cwd: ROOT, timeout: 15_000, maxBuffer: 2 * 1024 * 1024 },
  );
  const result = JSON.parse(stdout);
  result.settingsPath = settingsPath;
  result.collectCount = (await readFile(countPath, "utf8"))
    .split("\n")
    .filter(Boolean).length;
  result.cleanup = () => rm(directory, { recursive: true, force: true });
  return result;
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
    { text: "jjjj" },
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
