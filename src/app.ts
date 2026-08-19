import type { ChildProcess } from "node:child_process";
import { collectQuota } from "./collect.js";
import { actionsForInput } from "./keys.js";
import { renderDashboard } from "./render.js";
import { sanitizeProcessError } from "./sanitize.js";
import type { DashboardState } from "./types.js";

const ALT_ENTER = "\x1b[?1049h\x1b[?25l";
const ALT_EXIT = "\x1b[?25h\x1b[?1049l\x1b[0m";

export class DashboardApp {
  private state: DashboardState = { loading: true, scroll: 0 };
  private child?: ChildProcess;
  private closed = false;
  private refreshSequence = 0;
  private ageTimer?: NodeJS.Timeout;

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
    await this.refresh();
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
    for (const action of actionsForInput(input)) {
      if (action === "quit") {
        this.close();
        return;
      } else if (action === "refresh") void this.refresh();
      else if (action === "scroll-up") {
        this.state.scroll = Math.max(0, this.state.scroll - 2);
        this.render();
      } else if (action === "scroll-down") {
        this.state.scroll += 2;
        this.render();
      }
    }
  };

  private async refresh(): Promise<void> {
    const sequence = ++this.refreshSequence;
    this.state.loading = true;
    this.state.error = undefined;
    this.state.lastAttemptAt = new Date();
    this.render();
    try {
      const report = await collectQuota({
        onChild: (child) => (this.child = child),
      });
      if (this.closed || sequence !== this.refreshSequence) return;
      this.state.report = report;
      this.state.error = undefined;
      this.state.scroll = 0;
    } catch (error) {
      if (this.closed || sequence !== this.refreshSequence) return;
      this.state.error = sanitizeProcessError(error);
    } finally {
      if (!this.closed && sequence === this.refreshSequence) {
        this.state.loading = false;
        this.render();
      }
    }
  }

  readonly close = () => {
    if (this.closed) return;
    this.closed = true;
    this.refreshSequence++;
    if (this.ageTimer) clearInterval(this.ageTimer);
    if (this.child && !this.child.killed) {
      const child = this.child;
      child.kill("SIGTERM");
      setTimeout(() => {
        if (child.exitCode === null && child.signalCode === null)
          child.kill("SIGKILL");
      }, 250).unref();
    }
    process.stdin.off("data", this.onInput);
    process.stdout.off("resize", this.render);
    if (process.stdin.isTTY) {
      process.stdin.setRawMode(false);
      process.stdin.pause();
    }
    process.stdout.write(ALT_EXIT);
    process.exitCode = 0;
  };
}
