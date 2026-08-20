import type { ChildProcess } from "node:child_process";
import { collectQuota } from "./collect.js";
import { TerminalInputParser } from "./keys.js";
import { RefreshScheduler } from "./refresh.js";
import { renderDashboard } from "./render.js";
import { sanitizeProcessError } from "./sanitize.js";
import { releaseSidebarStateSync } from "./sidebar-state.js";
import type { DashboardState, QuotaReport } from "./types.js";

const ALT_ENTER = "\x1b[?1049h\x1b[?25l";
const ALT_EXIT = "\x1b[?25h\x1b[?1049l\x1b[0m";

export class ChildProcessTracker {
  private readonly active = new Set<ChildProcess>();

  update(child: ChildProcess, active: boolean) {
    if (active) this.active.add(child);
    else this.active.delete(child);
  }

  terminateAll() {
    for (const child of this.active) {
      if (!child.killed) child.kill("SIGTERM");
      setTimeout(() => {
        if (child.exitCode === null && child.signalCode === null)
          child.kill("SIGKILL");
      }, 250).unref();
    }
  }
}

export class DashboardApp {
  private state: DashboardState = { loading: true, scroll: 0 };
  private readonly children = new ChildProcessTracker();
  private readonly inputParser = new TerminalInputParser();
  private inputTimer?: NodeJS.Timeout;
  private closed = false;
  private ageTimer?: NodeJS.Timeout;
  private readonly refreshScheduler: RefreshScheduler<QuotaReport>;

  constructor() {
    this.refreshScheduler = new RefreshScheduler({
      collect: () =>
        collectQuota({
          onChild: (child, active) => {
            this.children.update(child, active);
          },
        }),
      onStart: () => {
        this.state.loading = true;
        this.state.error = undefined;
        this.state.lastAttemptAt = new Date();
        this.render();
      },
      onSuccess: (report) => {
        this.state.report = report;
        this.state.error = undefined;
        this.state.scroll = 0;
      },
      onFailure: (error) => {
        this.state.error = sanitizeProcessError(error);
      },
      onSettled: () => {
        this.state.loading = false;
        this.render();
      },
      cancelActive: () => this.terminateChildren(),
    });
  }

  async run(): Promise<void> {
    if (!process.stdin.isTTY || !process.stdout.isTTY) {
      throw new Error("AI Quota needs an interactive terminal");
    }
    process.stdout.write(ALT_ENTER);
    process.stdin.setRawMode(true);
    process.stdin.resume();
    process.stdin.on("data", this.onInput);
    process.stdout.on("resize", this.render);
    process.once("SIGTERM", this.close);
    process.once("SIGINT", this.close);
    this.ageTimer = setInterval(this.render, 30_000);
    this.render();
    await this.refreshScheduler.start();
  }

  private readonly render = () => {
    if (this.closed) return;
    const screen = renderDashboard(this.state, {
      width: process.stdout.columns || 80,
      height: process.stdout.rows || 24,
    });
    process.stdout.write(`\x1b[H${screen}\x1b[J`);
  };

  private readonly onInput = (input: Buffer) => {
    if (this.inputTimer) clearTimeout(this.inputTimer);
    this.handleActions(this.inputParser.push(input));
    this.inputTimer = setTimeout(() => {
      this.inputTimer = undefined;
      this.handleActions(this.inputParser.flush());
    }, 30);
  };

  private handleActions(actions: ReturnType<TerminalInputParser["push"]>) {
    for (const action of actions) {
      if (action === "quit") {
        this.close();
        return;
      } else if (action === "refresh") void this.refreshScheduler.manual();
    }
  }

  private terminateChildren() {
    this.children.terminateAll();
  }

  readonly close = () => {
    if (this.closed) return;
    this.closed = true;
    this.refreshScheduler.close();
    if (this.ageTimer) clearInterval(this.ageTimer);
    if (this.inputTimer) clearTimeout(this.inputTimer);
    process.stdin.off("data", this.onInput);
    process.stdout.off("resize", this.render);
    if (process.stdin.isTTY) {
      process.stdin.setRawMode(false);
      process.stdin.pause();
    }
    releaseSidebarStateSync(
      process.env.HERDR_QUOTA_STATE_FILE,
      process.env.HERDR_QUOTA_STATE_TOKEN,
    );
    process.stdout.write(ALT_EXIT);
    process.exitCode = 0;
  };
}
