import assert from "node:assert/strict";
import { constants } from "node:fs";
import {
  chmod,
  lstat,
  mkdir,
  mkdtemp,
  readFile,
  rm,
  symlink,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { HISTORY_SCHEMA_VERSION } from "../dist/history.js";
import { defaultSettings } from "../dist/settings.js";
import {
  LocalTransitions,
  TRANSITION_MAX_AGE_MS,
  TRANSITION_MAX_EVENTS,
  TRANSITION_SCHEMA_VERSION,
  appendTransitionEvents,
  parseTransitionDocument,
  transitionPathFromEnvironment,
  writeTransitionDocumentAtomic,
} from "../dist/transitions.js";

const BASE = Date.parse("2026-08-20T12:00:00.000Z");
const RESET = "2026-08-27T12:00:00.000Z";
const transitionsV1Fixture = await readFile(
  new URL("fixtures/transitions-v1.json", import.meta.url),
  "utf8",
);

function configured() {
  return { ...defaultSettings(), remainingThreshold: 25 };
}

function snapshot(minute, remaining) {
  return {
    capturedAt: new Date(BASE + minute * 60_000).toISOString(),
    providers: [
      {
        provider: "OpenAI Codex",
        dataHealth: "current",
        authEligible: true,
        facts: [
          {
            scope: "All models",
            limit: "Week",
            remaining,
            resetAt: RESET,
            runway: { state: "through_reset" },
          },
        ],
      },
    ],
  };
}

function history(...snapshots) {
  return { schemaVersion: HISTORY_SCHEMA_VERSION, snapshots };
}

function event(minute = 0, overrides = {}) {
  return {
    provider: "OpenAI Codex",
    scope: "All models",
    limit: "Week",
    cycle: RESET,
    policy: { remainingThreshold: 25, forecastBeforeReset: false },
    kind: "threshold_baseline",
    occurredAt: new Date(BASE + minute * 60_000).toISOString(),
    ...overrides,
  };
}

function document(events = []) {
  return { schemaVersion: TRANSITION_SCHEMA_VERSION, events };
}

async function temporaryDirectory() {
  return mkdtemp(join(tmpdir(), "herdr-quota-transitions-"));
}

test("transition path is a separate XDG state document", () => {
  assert.equal(
    transitionPathFromEnvironment({ XDG_STATE_HOME: "/state" }, "/unused"),
    "/state/herdr-quota/transitions-v1.json",
  );
  assert.equal(
    transitionPathFromEnvironment(
      { HERDR_QUOTA_TRANSITION_FILE: "/custom/events.json" },
      "/unused",
    ),
    "/custom/events.json",
  );
});

test("transition v1 migrates in memory and v2 is written on the next save", async () => {
  const directory = await temporaryDirectory();
  const path = join(directory, "transitions-v1.json");
  try {
    await writeFile(path, transitionsV1Fixture, { mode: 0o600 });
    const migrated = parseTransitionDocument(JSON.parse(transitionsV1Fixture));
    assert.equal(migrated.schemaVersion, TRANSITION_SCHEMA_VERSION);

    const store = new LocalTransitions(path);
    assert.equal(
      (await store.loadView(history(snapshot(0, 40)), configured()))
        .availability,
      "ready",
    );
    assert.equal(await readFile(path, "utf8"), transitionsV1Fixture);

    await writeTransitionDocumentAtomic(path, migrated);
    assert.equal(await readFile(path, "utf8"), `${JSON.stringify(migrated)}\n`);
    assert.equal(JSON.parse(await readFile(path, "utf8")).schemaVersion, 2);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("transition v2 keeps the four provider allow-list and is future to v0.3", () => {
  assert.equal(TRANSITION_SCHEMA_VERSION, 2);
  assert.ok(TRANSITION_SCHEMA_VERSION > 1);
  for (const provider of ["Claude", "OpenAI Codex", "Cursor", "Kimi"]) {
    assert.doesNotThrow(() =>
      parseTransitionDocument(document([event(0, { provider })])),
    );
  }
  assert.throws(
    () =>
      parseTransitionDocument(
        document([event(0, { provider: "GitHub Copilot" })]),
      ),
    /transitions_corrupt/,
  );
});

test("private atomic writes and acknowledgement survive pane reopen", async () => {
  const directory = await temporaryDirectory();
  const path = join(directory, "state", "herdr-quota", "transitions-v1.json");
  try {
    const store = new LocalTransitions(path);
    const first = history(snapshot(0, 40));
    assert.deepEqual((await store.evaluate(first, configured())).events, []);
    const crossed = history(snapshot(0, 40), snapshot(5, 20));
    const view = await store.evaluate(crossed, configured());
    assert.equal(view.events.length, 1);
    assert.equal(view.events[0].kind, "threshold_enter");
    assert.equal(view.events[0].remaining, 20);
    assert.equal((await lstat(path)).mode & 0o777, 0o600);

    const reopened = new LocalTransitions(path);
    assert.equal(
      (await reopened.loadView(crossed, configured())).events.length,
      1,
    );
    const acknowledged = await reopened.acknowledge(crossed, configured());
    assert.deepEqual(acknowledged.events, []);
    const stored = parseTransitionDocument(
      JSON.parse(await readFile(path, "utf8")),
    );
    assert.ok(
      stored.events.find((item) => item.kind === "threshold_enter")
        ?.acknowledgedAt,
    );
    assert.deepEqual(
      (await reopened.loadView(crossed, configured())).events,
      [],
    );
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("clear deletes only transition history and leaves neighboring quota history", async () => {
  const directory = await temporaryDirectory();
  const state = join(directory, "herdr-quota");
  const path = join(state, "transitions-v1.json");
  const quotaHistory = join(state, "history-v1.json");
  try {
    await mkdir(state, { recursive: true });
    await writeFile(path, `${JSON.stringify(document([event()]))}\n`, {
      mode: 0o600,
    });
    await writeFile(quotaHistory, "quota-history-stays\n", { mode: 0o600 });
    const cleared = await new LocalTransitions(path).clear();
    assert.equal(cleared.availability, "first_run");
    await assert.rejects(() => readFile(path, "utf8"), { code: "ENOENT" });
    assert.equal(await readFile(quotaHistory, "utf8"), "quota-history-stays\n");
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("unsupported future schema is preserved across load, evaluate, and clear", async () => {
  const directory = await temporaryDirectory();
  const path = join(directory, "transitions-v1.json");
  const future = `{"schemaVersion":${TRANSITION_SCHEMA_VERSION + 1},"future":"keep"}\n`;
  try {
    await writeFile(path, future, { mode: 0o600 });
    const store = new LocalTransitions(path);
    assert.equal(
      (await store.loadView(history(snapshot(0, 40)), configured()))
        .availability,
      "incompatible",
    );
    assert.equal(
      (await store.evaluate(history(snapshot(0, 40)), configured()))
        .availability,
      "incompatible",
    );
    assert.equal((await store.clear()).availability, "incompatible");
    assert.equal(await readFile(path, "utf8"), future);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("malformed and truncated data recovers to a finite baseline", async () => {
  const directory = await temporaryDirectory();
  const path = join(directory, "transitions-v1.json");
  try {
    await writeFile(path, "{truncated", { mode: 0o600 });
    const view = await new LocalTransitions(path).evaluate(
      history(snapshot(0, 40)),
      configured(),
    );
    assert.equal(view.availability, "recovered");
    assert.deepEqual(view.events, []);
    const stored = parseTransitionDocument(
      JSON.parse(await readFile(path, "utf8")),
    );
    assert.deepEqual(
      stored.events.map((item) => item.kind),
      ["threshold_baseline"],
    );
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("retention is bounded by count and age", () => {
  let current = document();
  for (let index = 0; index < TRANSITION_MAX_EVENTS + 40; index++) {
    current = appendTransitionEvents(current, [event(index)]).document;
  }
  assert.equal(current.events.length, TRANSITION_MAX_EVENTS);
  assert.equal(
    current.events.at(-1).occurredAt,
    event(TRANSITION_MAX_EVENTS + 39).occurredAt,
  );

  const outsideWindow = Math.floor(TRANSITION_MAX_AGE_MS / 60_000) + 1;
  const aged = appendTransitionEvents(document([event()]), [
    event(outsideWindow),
  ]).document;
  assert.deepEqual(aged.events, [event(outsideWindow)]);

  const tooMany = document(
    Array.from({ length: TRANSITION_MAX_EVENTS + 1 }, (_, index) =>
      event(index),
    ),
  );
  assert.throws(() => parseTransitionDocument(tooMany), /transitions_corrupt/);
});

test("mode-0400 and symlink targets fail closed without losing bytes", async () => {
  const directory = await temporaryDirectory();
  const readonlyPath = join(directory, "readonly.json");
  const destination = join(directory, "destination.json");
  const link = join(directory, "link.json");
  const original = `${JSON.stringify(document([event()]))}\n`;
  try {
    await writeFile(readonlyPath, original, { mode: 0o400 });
    const readonly = await new LocalTransitions(readonlyPath).evaluate(
      history(snapshot(0, 40), snapshot(5, 20)),
      configured(),
    );
    assert.equal(readonly.availability, "unavailable");
    assert.equal(await readFile(readonlyPath, "utf8"), original);

    await writeFile(destination, original, { mode: 0o600 });
    await symlink(destination, link);
    const linked = new LocalTransitions(link);
    assert.equal((await linked.clear()).availability, "unavailable");
    assert.equal(await readFile(destination, "utf8"), original);
  } finally {
    await chmod(readonlyPath, 0o600).catch(() => undefined);
    await rm(directory, { recursive: true, force: true });
  }
});

test("non-owned targets and interrupted replacement preserve the prior file", async () => {
  const original = `${JSON.stringify(document([event()]))}\n`;
  const nonOwned = memoryOperations(original, {
    uid: (process.getuid?.() ?? 0) + 1,
  });
  await assert.rejects(
    () =>
      writeTransitionDocumentAtomic(
        "/state/events.json",
        document([event(1)]),
        nonOwned,
      ),
    /transitions_unsafe/,
  );
  assert.equal(nonOwned.target(), original);
  assert.deepEqual(nonOwned.events, ["mkdir-448", "lstat"]);

  const interrupted = memoryOperations(original, { failRename: true });
  await assert.rejects(() =>
    writeTransitionDocumentAtomic(
      "/state/events.json",
      document([event(), event(1)]),
      interrupted,
    ),
  );
  assert.equal(interrupted.target(), original);
  assert.equal(interrupted.events.at(-1), "unlink-temp");
});

test("parser and writer reject secret or raw-payload fields", async () => {
  const unsafe = document([
    {
      ...event(),
      accountId: "account-secret",
      rawPayload: { token: "credential-secret" },
    },
  ]);
  assert.throws(() => parseTransitionDocument(unsafe), /transitions_corrupt/);
  assert.throws(
    () => parseTransitionDocument({ events: [] }),
    /transitions_corrupt/,
  );

  const directory = await temporaryDirectory();
  try {
    const path = join(directory, "events.json");
    await assert.rejects(() => writeTransitionDocumentAtomic(path, unsafe));
    await assert.rejects(() => readFile(path, "utf8"), { code: "ENOENT" });
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

function memoryOperations(initial, options = {}) {
  let target = initial;
  const temporary = new Map();
  const events = [];
  const targetPath = "/state/events.json";
  const stat = {
    isFile: () => true,
    isSymbolicLink: () => false,
    mode: options.mode ?? 0o100600,
    uid: options.uid ?? process.getuid?.() ?? 0,
  };
  return {
    events,
    target: () => target,
    async lstat(path) {
      events.push("lstat");
      if (path !== targetPath || target === undefined) {
        const error = new Error("missing");
        error.code = "ENOENT";
        throw error;
      }
      return stat;
    },
    async mkdir(_path, settings) {
      events.push(`mkdir-${settings.mode}`);
    },
    async open(path, flags, mode) {
      if (path === targetPath) {
        assert.ok(flags & (constants.O_RDONLY | constants.O_NOFOLLOW));
        events.push("open-target");
        return {
          async stat() {
            return { isFile: () => true };
          },
          async readFile() {
            return target;
          },
          async writeFile() {},
          async sync() {},
          async close() {},
        };
      }
      events.push(`open-temp-${mode}`);
      let value;
      temporary.set(path, undefined);
      return {
        async stat() {
          return { isFile: () => true };
        },
        async readFile() {
          return value;
        },
        async writeFile(text) {
          value = text;
          temporary.set(path, text);
        },
        async sync() {
          events.push("sync");
        },
        async close() {},
      };
    },
    async rename(from, to) {
      events.push("rename");
      assert.equal(to, targetPath);
      if (options.failRename) throw new Error("interrupted");
      target = temporary.get(from);
      temporary.delete(from);
    },
    async unlink(path) {
      if (path === targetPath) {
        events.push("unlink-target");
        target = undefined;
      } else {
        events.push("unlink-temp");
        temporary.delete(path);
      }
    },
  };
}
