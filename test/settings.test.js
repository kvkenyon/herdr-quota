import assert from "node:assert/strict";
import { constants } from "node:fs";
import { lstat } from "node:fs/promises";
import {
  chmod,
  mkdir,
  mkdtemp,
  readFile,
  readdir,
  rm,
  symlink,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import {
  defaultSettings,
  parseSettingsDocument,
  SettingsStore,
  settingsPath,
} from "../dist/settings.js";

async function temporaryDirectory() {
  return await mkdtemp(join(tmpdir(), "herdr-quota-settings-"));
}

test("first run uses v2 defaults at the XDG config path", async () => {
  const directory = await temporaryDirectory();
  try {
    const path = settingsPath({ XDG_CONFIG_HOME: directory }, "/unused");
    const loaded = await new SettingsStore(path).load();
    assert.deepEqual(loaded, {
      settings: defaultSettings(),
      availability: "first_run",
    });
    assert.equal(path, join(directory, "herdr-quota", "settings.json"));
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("atomic replacement writes only the finite schema with private permissions", async () => {
  const directory = await temporaryDirectory();
  const path = join(directory, "config", "herdr-quota", "settings.json");
  try {
    const store = new SettingsStore(path);
    await store.save({
      ...defaultSettings(),
      providerOrder: ["cursor", "claude", "kimi", "codex"],
      hiddenProviders: ["kimi"],
      meterMode: "used",
      accountId: "account-secret",
      rawPayload: { token: "credential-secret" },
      error: "/private/provider/path",
    });
    const text = await readFile(path, "utf8");
    assert.deepEqual(JSON.parse(text), {
      schemaVersion: 2,
      providerOrder: ["cursor", "claude", "kimi", "codex"],
      hiddenProviders: ["kimi"],
      meterMode: "used",
      remainingThreshold: "off",
      forecastBeforeReset: false,
    });
    assert.doesNotMatch(
      text,
      /account|secret|credential|payload|error|private|path/i,
    );
    assert.equal((await lstat(path)).mode & 0o777, 0o600);
    assert.deepEqual(
      (await readdir(join(directory, "config", "herdr-quota"))).filter((name) =>
        name.includes(".tmp"),
      ),
      [],
    );
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("v0.2.1 schema-v1 settings migrate in memory without rewriting", async () => {
  const directory = await temporaryDirectory();
  const path = join(directory, "herdr-quota", "settings.json");
  const legacy =
    '{"schemaVersion":1,"providerOrder":["cursor","claude","codex","kimi"],"hiddenProviders":["kimi"],"meterMode":"used"}\n';
  try {
    await mkdir(join(directory, "herdr-quota"), { recursive: true });
    await writeFile(path, legacy, { mode: 0o600 });
    const loaded = await new SettingsStore(path).load();
    assert.deepEqual(loaded, {
      availability: "ready",
      settings: {
        ...defaultSettings(),
        providerOrder: ["cursor", "claude", "codex", "kimi"],
        hiddenProviders: ["kimi"],
        meterMode: "used",
      },
    });
    assert.equal(await readFile(path, "utf8"), legacy);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("schema-v2 transition policy accepts only the finite product controls", () => {
  for (const remainingThreshold of ["off", 25, 10, 5]) {
    assert.equal(
      parseSettingsDocument({
        ...defaultSettings(),
        remainingThreshold,
      }).remainingThreshold,
      remainingThreshold,
    );
  }
  assert.throws(
    () =>
      parseSettingsDocument({
        ...defaultSettings(),
        remainingThreshold: 13,
      }),
    /settings_corrupt/,
  );
  assert.throws(
    () =>
      parseSettingsDocument({
        ...defaultSettings(),
        forecastBeforeReset: "yes",
      }),
    /settings_corrupt/,
  );
});

test("malformed JSON is quarantined and recovered without replacing its bytes", async () => {
  const directory = await temporaryDirectory();
  const settingsDirectory = join(directory, "herdr-quota");
  const path = join(settingsDirectory, "settings.json");
  try {
    await mkdir(settingsDirectory, { recursive: true });
    await writeFile(path, "{truncated", { mode: 0o600 });
    const loaded = await new SettingsStore(path).load();
    assert.equal(loaded.availability, "recovered");
    assert.deepEqual(loaded.settings, defaultSettings());
    const files = await readdir(settingsDirectory);
    const quarantine = files.find((name) =>
      name.startsWith("settings.json.invalid-"),
    );
    assert.ok(quarantine);
    assert.equal(
      await readFile(join(settingsDirectory, quarantine), "utf8"),
      "{truncated",
    );
    assert.equal(
      (await lstat(join(settingsDirectory, quarantine))).mode & 0o777,
      0o600,
    );
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("unsupported schemas remain intact and unknown fields are ignored", async () => {
  const directory = await temporaryDirectory();
  const path = join(directory, "herdr-quota", "settings.json");
  try {
    await mkdir(join(directory, "herdr-quota"), { recursive: true });
    const future = '{"schemaVersion":99,"future":"keep"}\n';
    await writeFile(path, future, { mode: 0o600 });
    const incompatible = await new SettingsStore(path).load();
    assert.equal(incompatible.availability, "incompatible");
    assert.deepEqual(incompatible.settings, defaultSettings());
    assert.equal(await readFile(path, "utf8"), future);
    await assert.rejects(
      () => new SettingsStore(path).save(defaultSettings()),
      /settings_incompatible/,
    );
    await assert.rejects(
      () =>
        new SettingsStore(path).save({
          ...defaultSettings(),
          meterMode: "used",
        }),
      /settings_incompatible/,
    );
    assert.equal(await readFile(path, "utf8"), future);
    assert.deepEqual(
      (await readdir(join(directory, "herdr-quota"))).filter((name) =>
        name.endsWith(".tmp"),
      ),
      [],
    );

    assert.deepEqual(
      parseSettingsDocument({
        ...defaultSettings(),
        providerOrder: ["future-provider", "cursor", "claude"],
        hiddenProviders: ["future-provider", "cursor"],
        futureField: { arbitrary: "ignored" },
      }),
      {
        ...defaultSettings(),
        providerOrder: ["cursor", "claude", "codex", "kimi"],
        hiddenProviders: ["cursor"],
      },
    );
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("symlink and non-regular settings targets are refused", async () => {
  const directory = await temporaryDirectory();
  const path = join(directory, "settings.json");
  const destination = join(directory, "elsewhere.json");
  try {
    await writeFile(destination, "do not touch", { mode: 0o600 });
    await symlink(destination, path);
    const store = new SettingsStore(path);
    assert.equal((await store.load()).availability, "unavailable");
    await assert.rejects(
      () => store.save(defaultSettings()),
      /settings_unsafe/,
    );
    assert.equal(await readFile(destination, "utf8"), "do not touch");

    await rm(path);
    await mkdir(path);
    assert.equal((await store.load()).availability, "unavailable");
    await assert.rejects(
      () => store.save(defaultSettings()),
      /settings_unsafe/,
    );
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("unwritable operations and interrupted replacement preserve the last valid file", async () => {
  const previous = `${JSON.stringify(defaultSettings())}\n`;
  for (const failAt of ["open", "write", "sync", "rename"]) {
    const operations = memoryOperations(previous, failAt);
    const store = new SettingsStore("/config/settings.json", operations);
    await assert.rejects(() =>
      store.save({ ...defaultSettings(), meterMode: "used" }),
    );
    assert.equal(operations.target(), previous, failAt);
    assert.equal(
      operations.files().some((name) => name.endsWith(".tmp")),
      false,
      failAt,
    );
  }
});

test("read-only and foreign-owned settings targets are refused before replacement", async () => {
  const directory = await temporaryDirectory();
  const path = join(directory, "settings.json");
  const previous = `${JSON.stringify(defaultSettings())}\n`;
  try {
    await writeFile(path, previous, { mode: 0o400 });
    await assert.rejects(
      () => new SettingsStore(path).save(defaultSettings()),
      /settings_unsafe/,
    );
    assert.equal(await readFile(path, "utf8"), previous);
    assert.deepEqual(
      (await readdir(directory)).filter((name) => name.endsWith(".tmp")),
      [],
    );

    const operations = memoryOperations(previous, undefined, {
      uid: (process.getuid?.() ?? 0) + 1,
    });
    await assert.rejects(
      () =>
        new SettingsStore("/config/settings.json", operations).save(
          defaultSettings(),
        ),
      /settings_unsafe/,
    );
    assert.equal(operations.target(), previous);
    assert.deepEqual(operations.files(), ["/config/settings.json"]);
  } finally {
    await chmod(path, 0o600).catch(() => undefined);
    await rm(directory, { recursive: true, force: true });
  }
});

test("a read-only directory failure is contained by the settings store", async (context) => {
  if (process.platform === "win32") {
    context.skip("POSIX permissions are required");
    return;
  }
  const directory = await temporaryDirectory();
  const path = join(directory, "locked", "settings.json");
  try {
    await mkdir(join(directory, "locked"), { mode: 0o500 });
    await chmod(join(directory, "locked"), 0o500);
    if (process.getuid?.() === 0) {
      context.skip("root bypasses directory write permissions");
      return;
    }
    await assert.rejects(() => new SettingsStore(path).save(defaultSettings()));
    assert.equal(
      (await new SettingsStore(path).load()).availability,
      "first_run",
    );
  } finally {
    await chmod(join(directory, "locked"), 0o700).catch(() => undefined);
    await rm(directory, { recursive: true, force: true });
  }
});

function memoryOperations(initial, failAt, statOverrides = {}) {
  const files = new Map([["/config/settings.json", initial]]);
  const regular = () => ({
    isFile: () => true,
    isSymbolicLink: () => false,
    mode: 0o100600,
    uid: process.getuid?.() ?? 0,
    ...statOverrides,
  });
  return {
    async lstat(path) {
      if (!files.has(path)) {
        const error = new Error("missing");
        error.code = "ENOENT";
        throw error;
      }
      return regular();
    },
    async mkdir() {},
    async open(path, flags) {
      if (failAt === "open" && flags & constants.O_CREAT)
        throw new Error("unwritable");
      if (flags & constants.O_CREAT) files.set(path, "");
      return {
        async stat() {
          return { isFile: () => true };
        },
        async readFile() {
          return files.get(path);
        },
        async writeFile(value) {
          if (failAt === "write") throw new Error("interrupted write");
          files.set(path, value);
        },
        async sync() {
          if (failAt === "sync") throw new Error("interrupted sync");
        },
        async close() {},
      };
    },
    async rename(from, to) {
      if (failAt === "rename") throw new Error("interrupted rename");
      files.set(to, files.get(from));
      files.delete(from);
    },
    async unlink(path) {
      files.delete(path);
    },
    async chmod() {},
    target: () => files.get("/config/settings.json"),
    files: () => [...files.keys()],
  };
}
