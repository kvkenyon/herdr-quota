import { execFile } from "node:child_process";
import { randomUUID } from "node:crypto";
import {
  mkdir,
  readFile,
  rename,
  rmdir,
  unlink,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { basename, dirname, join } from "node:path";
import { promisify } from "node:util";
import { pathToFileURL } from "node:url";
import { sanitizeProcessError } from "./sanitize.js";
import {
  planRebuild,
  resizeForTarget,
  type PaneRect,
  type RebuildPlan,
} from "./sidebar-layout.js";

const execFileAsync = promisify(execFile);

export interface SidebarContext {
  workspace: string;
  tab: string;
  focusedPane: string;
  stateFile: string;
}

export interface SidebarState {
  phase: "evacuating" | "open";
  token: string;
  workspace: string;
  tab: string;
  originalFocus: string;
  plan: RebuildPlan;
  parked: string[];
  parkingPlaceholder?: string;
  sidebarPane?: string;
}

export interface SidebarStore {
  load(): Promise<SidebarState | undefined>;
  save(state: SidebarState): Promise<void>;
  remove(): Promise<void>;
}

export interface HerdrApi {
  livePanes(): Promise<Set<string>>;
  layout(pane: string): Promise<{ areaWidth: number; rects: PaneRect[] }>;
  createParkingTab(workspace: string): Promise<{
    tab: string;
    placeholder: string;
  }>;
  movePane(
    pane: string,
    tab: string,
    direction: "right" | "down",
    target?: string,
    ratio?: number,
  ): Promise<void>;
  openSidebar(
    target: string,
    stateFile: string,
    token: string,
  ): Promise<string>;
  closePluginPane(pane: string): Promise<void>;
  closePane(pane: string): Promise<void>;
  resizePane(
    pane: string,
    direction: "left" | "right",
    amount: number,
  ): Promise<void>;
}

function stringAt(value: unknown, path: string[]): string {
  let current: unknown = value;
  for (const key of path) {
    if (!current || typeof current !== "object")
      throw new Error(`Herdr response is missing ${path.join(".")}`);
    current = (current as Record<string, unknown>)[key];
  }
  if (typeof current !== "string")
    throw new Error(`Herdr response is missing ${path.join(".")}`);
  return current;
}

function numberAt(value: unknown, path: string[]): number {
  let current: unknown = value;
  for (const key of path) {
    if (!current || typeof current !== "object")
      throw new Error(`Herdr response is missing ${path.join(".")}`);
    current = (current as Record<string, unknown>)[key];
  }
  if (typeof current !== "number")
    throw new Error(`Herdr response is missing ${path.join(".")}`);
  return current;
}

export class FileSidebarStore implements SidebarStore {
  constructor(readonly path: string) {}

  async load(): Promise<SidebarState | undefined> {
    try {
      return JSON.parse(await readFile(this.path, "utf8")) as SidebarState;
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === "ENOENT") return undefined;
      throw error;
    }
  }

  async save(state: SidebarState): Promise<void> {
    await mkdir(dirname(this.path), { recursive: true, mode: 0o700 });
    const temporary = `${this.path}.${process.pid}.${state.token}.tmp`;
    await writeFile(temporary, `${JSON.stringify(state)}\n`, { mode: 0o600 });
    await rename(temporary, this.path);
  }

  async remove(): Promise<void> {
    try {
      await unlink(this.path);
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
    }
    const sessionDirectory = dirname(this.path);
    await rmdir(sessionDirectory).catch(() => undefined);
    await rmdir(dirname(sessionDirectory)).catch(() => undefined);
  }
}

export class CliHerdrApi implements HerdrApi {
  constructor(private readonly bin = process.env.HERDR_BIN_PATH || "herdr") {}

  private async run(args: string[]): Promise<unknown> {
    const { stdout } = await execFileAsync(this.bin, args, {
      timeout: 20_000,
      maxBuffer: 2 * 1024 * 1024,
      env: process.env,
    });
    return JSON.parse(stdout) as unknown;
  }

  async livePanes(): Promise<Set<string>> {
    const value = await this.run(["pane", "list"]);
    const panes = (value as { result?: { panes?: unknown[] } }).result?.panes;
    if (!Array.isArray(panes)) throw new Error("Herdr pane list is malformed");
    return new Set(panes.map((pane) => stringAt(pane, ["pane_id"])));
  }

  async layout(
    pane: string,
  ): Promise<{ areaWidth: number; rects: PaneRect[] }> {
    const value = await this.run(["pane", "layout", "--pane", pane]);
    const layout = (value as { result?: { layout?: unknown } }).result?.layout;
    if (!layout || typeof layout !== "object")
      throw new Error("Herdr pane layout is malformed");
    const panes = (layout as { panes?: unknown[] }).panes;
    if (!Array.isArray(panes))
      throw new Error("Herdr pane layout is malformed");
    const originX = numberAt(layout, ["area", "x"]);
    const originY = numberAt(layout, ["area", "y"]);
    return {
      areaWidth: numberAt(layout, ["area", "width"]),
      rects: panes.map((item) => ({
        paneId: stringAt(item, ["pane_id"]),
        x: numberAt(item, ["rect", "x"]) - originX,
        y: numberAt(item, ["rect", "y"]) - originY,
        width: numberAt(item, ["rect", "width"]),
        height: numberAt(item, ["rect", "height"]),
      })),
    };
  }

  async createParkingTab(
    workspace: string,
  ): Promise<{ tab: string; placeholder: string }> {
    const value = await this.run([
      "tab",
      "create",
      "--workspace",
      workspace,
      "--no-focus",
    ]);
    return {
      tab: stringAt(value, ["result", "tab", "tab_id"]),
      placeholder: stringAt(value, ["result", "root_pane", "pane_id"]),
    };
  }

  async movePane(
    pane: string,
    tab: string,
    direction: "right" | "down",
    target?: string,
    ratio?: number,
  ): Promise<void> {
    const args = ["pane", "move", pane, "--tab", tab, "--split", direction];
    if (target) args.push("--target-pane", target);
    if (ratio !== undefined) args.push("--ratio", String(ratio));
    args.push("--no-focus");
    await this.run(args);
  }

  async openSidebar(
    target: string,
    stateFile: string,
    token: string,
  ): Promise<string> {
    const value = await this.run([
      "plugin",
      "pane",
      "open",
      "--plugin",
      "herdr-quota",
      "--entrypoint",
      "dashboard",
      "--placement",
      "split",
      "--target-pane",
      target,
      "--direction",
      "right",
      "--env",
      `HERDR_QUOTA_STATE_FILE=${stateFile}`,
      "--env",
      `HERDR_QUOTA_STATE_TOKEN=${token}`,
      "--focus",
    ]);
    return stringAt(value, ["result", "plugin_pane", "pane", "pane_id"]);
  }

  async closePluginPane(pane: string): Promise<void> {
    await this.run(["plugin", "pane", "close", pane]);
  }

  async closePane(pane: string): Promise<void> {
    await this.run(["pane", "close", pane]);
  }

  async resizePane(
    pane: string,
    direction: "left" | "right",
    amount: number,
  ): Promise<void> {
    await this.run([
      "pane",
      "resize",
      "--pane",
      pane,
      "--direction",
      direction,
      "--amount",
      String(amount),
    ]);
  }
}

async function restoreEvacuation(
  api: HerdrApi,
  store: SidebarStore,
  state: SidebarState,
): Promise<void> {
  let live = await api.livePanes();
  if (state.sidebarPane && live.has(state.sidebarPane)) {
    await api.closePluginPane(state.sidebarPane);
    live = await api.livePanes();
  }

  for (const step of state.plan.steps) {
    if (!state.parked.includes(step.pane)) continue;
    if (!live.has(step.pane)) {
      state.parked = state.parked.filter((pane) => pane !== step.pane);
      await store.save(state);
      continue;
    }
    if (!live.has(step.target))
      throw new Error(`cannot restore pane beside missing ${step.target}`);
    await api.movePane(
      step.pane,
      state.tab,
      step.direction,
      step.target,
      step.ratio,
    );
    state.parked = state.parked.filter((pane) => pane !== step.pane);
    await store.save(state);
    live.add(step.pane);
  }

  if (state.parkingPlaceholder && live.has(state.parkingPlaceholder))
    await api.closePane(state.parkingPlaceholder);
  await store.remove();
}

export async function toggleSidebar(
  api: HerdrApi,
  store: SidebarStore,
  context: SidebarContext,
  token = randomUUID(),
): Promise<"opened" | "closed"> {
  const existing = await store.load();
  if (existing?.tab === context.tab && existing.phase === "open") {
    const live = await api.livePanes();
    if (existing.sidebarPane && live.has(existing.sidebarPane)) {
      await api.closePluginPane(existing.sidebarPane);
      await store.remove();
      return "closed";
    }
    await store.remove();
  } else if (existing?.tab === context.tab) {
    await restoreEvacuation(api, store, existing);
  } else if (existing) {
    await store.remove();
  }

  const original = await api.layout(context.focusedPane);
  const plan = planRebuild(original.rects);
  const state: SidebarState = {
    phase: "evacuating",
    token,
    workspace: context.workspace,
    tab: context.tab,
    originalFocus: context.focusedPane,
    plan,
    parked: [],
  };
  await store.save(state);

  try {
    if (original.rects.length > 1) {
      const parking = await api.createParkingTab(context.workspace);
      state.parkingPlaceholder = parking.placeholder;
      await store.save(state);
      for (const rect of original.rects) {
        if (rect.paneId === plan.anchor) continue;
        await api.movePane(rect.paneId, parking.tab, "right");
        state.parked.push(rect.paneId);
        await store.save(state);
      }
    }

    state.sidebarPane = await api.openSidebar(
      plan.anchor,
      context.stateFile,
      token,
    );
    await store.save(state);

    for (const step of plan.steps) {
      await api.movePane(
        step.pane,
        context.tab,
        step.direction,
        step.target,
        step.ratio,
      );
      state.parked = state.parked.filter((pane) => pane !== step.pane);
      await store.save(state);
    }

    if (state.parkingPlaceholder) {
      await api.closePane(state.parkingPlaceholder);
      state.parkingPlaceholder = undefined;
      await store.save(state);
    }

    const current = await api.layout(state.sidebarPane);
    const sidebar = current.rects.find(
      (rect) => rect.paneId === state.sidebarPane,
    );
    if (!sidebar) throw new Error("opened sidebar is missing from its layout");
    const resize = resizeForTarget(current.areaWidth, sidebar.width);
    if (resize)
      await api.resizePane(state.sidebarPane, resize.direction, resize.amount);

    state.phase = "open";
    await store.save(state);
    return "opened";
  } catch (error) {
    await restoreEvacuation(api, store, state).catch(() => undefined);
    throw error;
  }
}

function safePart(value: string): string {
  return value.replaceAll(/[^A-Za-z0-9_-]/g, "_").slice(0, 80);
}

export function contextFromEnvironment(
  env: NodeJS.ProcessEnv = process.env,
): SidebarContext {
  let context: Record<string, unknown> = {};
  try {
    context = JSON.parse(env.HERDR_PLUGIN_CONTEXT_JSON ?? "{}") as Record<
      string,
      unknown
    >;
  } catch {
    context = {};
  }
  const workspace = env.HERDR_WORKSPACE_ID ?? context.workspace_id;
  const tab = env.HERDR_TAB_ID ?? context.tab_id;
  const focusedPane = env.HERDR_PANE_ID ?? context.focused_pane_id;
  if (
    typeof workspace !== "string" ||
    typeof tab !== "string" ||
    typeof focusedPane !== "string"
  )
    throw new Error("AI Quota requires an active Herdr pane");

  const socket = env.HERDR_SOCKET_PATH;
  const session = socket
    ? basename(dirname(socket))
    : env.HERDR_SESSION || "local";
  const stateFile = join(
    tmpdir(),
    "herdr-quota",
    safePart(session),
    `${safePart(tab)}.json`,
  );
  return { workspace, tab, focusedPane, stateFile };
}

async function main() {
  const context = contextFromEnvironment();
  const store = new FileSidebarStore(context.stateFile);
  await toggleSidebar(new CliHerdrApi(), store, context);
}

const invokedPath = process.argv[1]
  ? pathToFileURL(process.argv[1]).href
  : undefined;
if (invokedPath === import.meta.url) {
  main().catch((error) => {
    process.stderr.write(`${sanitizeProcessError(error)}\n`);
    process.exitCode = 1;
  });
}
