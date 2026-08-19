import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import {
  planRebuild,
  resizeForTarget,
  targetSidebarWidth,
} from "../dist/sidebar-layout.js";
import { releaseSidebarStateSync } from "../dist/sidebar-state.js";
import { toggleSidebar } from "../dist/sidebar.js";

test("layout plan preserves an asymmetric split tree under one anchor", () => {
  const plan = planRebuild([
    { paneId: "p1", x: 0, y: 0, width: 40, height: 100 },
    { paneId: "p2", x: 40, y: 0, width: 60, height: 50 },
    { paneId: "p3", x: 40, y: 50, width: 60, height: 50 },
  ]);
  assert.equal(plan.anchor, "p1");
  assert.deepEqual(
    plan.steps.map(({ pane, direction, target }) => ({
      pane,
      direction,
      target,
    })),
    [
      { pane: "p2", direction: "right", target: "p1" },
      { pane: "p3", direction: "down", target: "p2" },
    ],
  );
  assert.ok(Math.abs(plan.steps[0].ratio - 0.4) < 0.001);
  assert.ok(Math.abs(plan.steps[1].ratio - 0.5) < 0.001);
});

test("sidebar width targets 36 cells and degrades without starving the tab", () => {
  assert.equal(targetSidebarWidth(160), 36);
  assert.equal(targetSidebarWidth(80), 36);
  assert.equal(targetSidebarWidth(54), 30);
  assert.equal(targetSidebarWidth(48), 24);
  assert.equal(targetSidebarWidth(36), 18);
  assert.deepEqual(resizeForTarget(120, 60), {
    direction: "right",
    amount: 0.2,
  });
  assert.deepEqual(resizeForTarget(54, 27), {
    direction: "left",
    amount: 3 / 54,
  });
});

test("toggle evacuates, rebuilds, opens full-height right, then closes cleanly", async () => {
  const api = new MockHerdr();
  const store = new MemoryStore();
  const context = {
    workspace: "w1",
    tab: "t1",
    focusedPane: "p2",
    stateFile: "/tmp/quota-t1.json",
  };

  assert.equal(await toggleSidebar(api, store, context, "token-1"), "opened");
  assert.deepEqual(api.operations, [
    "layout p2",
    "create-tab w1",
    "move p2 -> parking right",
    "move p3 -> parking right",
    "open p1 right token-1",
    "move p2 -> t1 right p1 0.4",
    "move p3 -> t1 down p2 0.5",
    "close-pane placeholder",
    "layout sidebar",
    "resize sidebar right 0.2",
  ]);
  assert.equal(store.state.phase, "open");
  assert.deepEqual(store.state.parked, []);

  api.operations.length = 0;
  assert.equal(await toggleSidebar(api, store, context, "token-2"), "closed");
  assert.deepEqual(api.operations, ["live", "close-plugin sidebar"]);
  assert.equal(store.state, undefined);
});

test("dashboard cleanup only removes its own fully-open state", async () => {
  const directory = await mkdtemp(join(tmpdir(), "herdr-quota-test-"));
  const path = join(directory, "state.json");
  try {
    await writeFile(path, JSON.stringify({ phase: "evacuating", token: "a" }));
    assert.equal(releaseSidebarStateSync(path, "a"), false);
    assert.equal(existsSync(path), true);

    await writeFile(path, JSON.stringify({ phase: "open", token: "new" }));
    assert.equal(releaseSidebarStateSync(path, "old"), false);
    assert.equal(existsSync(path), true);
    assert.equal(releaseSidebarStateSync(path, "new"), true);
    assert.equal(existsSync(path), false);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

class MemoryStore {
  state;

  async load() {
    return this.state ? JSON.parse(JSON.stringify(this.state)) : undefined;
  }

  async save(state) {
    this.state = JSON.parse(JSON.stringify(state));
  }

  async remove() {
    this.state = undefined;
  }
}

class MockHerdr {
  operations = [];
  live = new Set(["p1", "p2", "p3", "sidebar"]);

  async livePanes() {
    this.operations.push("live");
    return new Set(this.live);
  }

  async layout(pane) {
    this.operations.push(`layout ${pane}`);
    if (pane === "p2") {
      return {
        areaWidth: 120,
        rects: [
          { paneId: "p1", x: 0, y: 0, width: 48, height: 100 },
          { paneId: "p2", x: 48, y: 0, width: 72, height: 50 },
          { paneId: "p3", x: 48, y: 50, width: 72, height: 50 },
        ],
      };
    }
    return {
      areaWidth: 120,
      rects: [
        { paneId: "p1", x: 0, y: 0, width: 24, height: 100 },
        { paneId: "p2", x: 24, y: 0, width: 36, height: 50 },
        { paneId: "p3", x: 24, y: 50, width: 36, height: 50 },
        { paneId: "sidebar", x: 60, y: 0, width: 60, height: 100 },
      ],
    };
  }

  async createParkingTab(workspace) {
    this.operations.push(`create-tab ${workspace}`);
    return { tab: "parking", placeholder: "placeholder" };
  }

  async movePane(pane, tab, direction, target, ratio) {
    this.operations.push(
      `move ${pane} -> ${tab} ${direction}${target ? ` ${target} ${ratio}` : ""}`,
    );
  }

  async openSidebar(target, _stateFile, token) {
    this.operations.push(`open ${target} right ${token}`);
    return "sidebar";
  }

  async closePluginPane(pane) {
    this.operations.push(`close-plugin ${pane}`);
    this.live.delete(pane);
  }

  async closePane(pane) {
    this.operations.push(`close-pane ${pane}`);
  }

  async resizePane(pane, direction, amount) {
    this.operations.push(`resize ${pane} ${direction} ${amount}`);
  }
}
