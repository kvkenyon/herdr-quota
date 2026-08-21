import type { ChildProcess } from "node:child_process";
import { collectQuota } from "./collect.js";
import { safeCollectorFailure } from "./failure.js";
import { LocalHistory, type HistoryDocument } from "./history.js";
import { TerminalInputParser, type DashboardAction } from "./keys.js";
import {
  applyPreferenceAction,
  openPreferences,
  type PreferenceAction,
} from "./preferences.js";
import { RefreshScheduler } from "./refresh.js";
import {
  applyDashboardScroll,
  clampDashboardScroll,
  renderDashboard,
} from "./render.js";
import {
  cloneSettings,
  defaultSettings,
  SettingsStore,
  type DashboardSettings,
  type SettingsLoadResult,
  type SupportedProvider,
} from "./settings.js";
import { releaseSidebarStateSync } from "./sidebar-state.js";
import { LocalTransitions, transitionPolicyEnabled } from "./transitions.js";
import type {
  DashboardState,
  HistoryView,
  PreferencesState,
  QuotaReport,
  TransitionView,
} from "./types.js";

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

export interface DashboardSettingsRepository {
  load(): Promise<SettingsLoadResult>;
  save(settings: DashboardSettings): Promise<void>;
}

export interface DashboardHistoryRepository {
  record(report: QuotaReport): Promise<HistoryView>;
  retained?(): Promise<HistoryDocument | undefined>;
  current?(): HistoryDocument | undefined;
}

export interface DashboardTransitionRepository {
  loadView(
    history: HistoryDocument,
    settings: DashboardSettings,
  ): Promise<TransitionView>;
  evaluate(
    history: HistoryDocument,
    settings: DashboardSettings,
  ): Promise<TransitionView>;
  baseline(
    history: HistoryDocument,
    settings: DashboardSettings,
    now?: Date,
    channels?: readonly ("threshold" | "forecast")[],
    providers?: readonly SupportedProvider[],
  ): Promise<TransitionView>;
  acknowledge(
    history: HistoryDocument,
    settings: DashboardSettings,
    now?: Date,
  ): Promise<TransitionView>;
  clear(): Promise<TransitionView>;
}

export interface DashboardAppOptions {
  collect?: () => Promise<QuotaReport>;
  history?: DashboardHistoryRepository;
  settings?: DashboardSettingsRepository;
  transitions?: DashboardTransitionRepository;
}

export class DashboardApp {
  private state: DashboardState = {
    loading: true,
    scroll: 0,
    settings: defaultSettings(),
    settingsAvailability: "first_run",
  };
  private readonly children = new ChildProcessTracker();
  private readonly inputParser = new TerminalInputParser();
  private inputTimer?: NodeJS.Timeout;
  private closed = false;
  private ageTimer?: NodeJS.Timeout;
  private readonly history: DashboardHistoryRepository;
  private readonly settings: DashboardSettingsRepository;
  private readonly transitions: DashboardTransitionRepository;
  private readonly refreshScheduler: RefreshScheduler<QuotaReport>;
  private transitionQueue: Promise<void> = Promise.resolve();

  constructor(options: DashboardAppOptions = {}) {
    this.history = options.history ?? new LocalHistory();
    this.settings = options.settings ?? new SettingsStore();
    this.transitions = options.transitions ?? new LocalTransitions();
    this.refreshScheduler = new RefreshScheduler({
      collect:
        options.collect ??
        (() =>
          collectQuota({
            onChild: (child, active) => {
              this.children.update(child, active);
            },
          })),
      onStart: () => {
        this.state.loading = true;
        this.state.failure = undefined;
        this.state.lastAttemptAt = new Date();
        this.render();
      },
      onSuccess: async (report, isCurrent, serialize) => {
        this.state.report = report;
        this.state.failure = undefined;
        this.render();
        const result = await serialize(async () => {
          const history = await this.history.record(report);
          const document = this.history.current?.();
          const settings = this.state.settings ?? defaultSettings();
          const transitions =
            document &&
            transitionPolicyEnabled(settings) &&
            history.availability === "clock_skew"
              ? await this.runTransition(() =>
                  this.transitions.baseline(document, settings),
                )
              : document && transitionPolicyEnabled(settings)
                ? await this.runTransition(() =>
                    this.transitions.evaluate(document, settings),
                  )
                : { availability: "ready" as const, events: [] };
          return { history, transitions };
        });
        if (isCurrent()) {
          this.state.history = result.history;
          this.state.transitions = result.transitions;
        }
      },
      onFailure: (error) => {
        this.state.failure = safeCollectorFailure(error);
      },
      onScheduled: (delayMs, afterFailure) => {
        if (afterFailure && this.state.failure)
          this.state.failure.retryAt = new Date(Date.now() + delayMs);
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
    await this.loadSettings();
    await this.loadTransitions();
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
    this.state.scroll = clampDashboardScroll(
      this.state,
      process.stdout.rows || 24,
    );
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
    }, 100);
  };

  private handleActions(actions: ReturnType<TerminalInputParser["push"]>) {
    for (const action of actions) {
      if (action === "quit") {
        this.close();
        return;
      }
      if (this.state.preferences) {
        this.handlePreferenceAction(action);
        continue;
      }
      if (this.state.transitionReview) {
        if (action === "escape") {
          this.state.transitionReview = false;
          this.render();
        } else if (action === "acknowledge" || action === "activate") {
          void this.acknowledgeTransitions();
        }
        continue;
      }
      if (action === "escape") {
        this.close();
        return;
      } else if (action === "preferences") {
        this.state.preferences = openPreferences(
          this.state.settings ?? defaultSettings(),
        );
        this.render();
      } else if (
        action === "acknowledge" &&
        (this.state.transitions?.events.length ?? 0) > 0
      ) {
        this.state.transitionReview = true;
        this.render();
      } else if (action === "refresh") void this.refreshScheduler.manual();
      else if (this.isScrollAction(action)) {
        applyDashboardScroll(this.state, action, process.stdout.rows || 24);
        this.render();
      }
    }
  }

  private handlePreferenceAction(action: DashboardAction) {
    const preferences = this.state.preferences;
    if (!preferences) return;
    const mapped = this.preferenceAction(action, preferences);
    if (!mapped) return;
    const update = applyPreferenceAction(
      preferences,
      mapped,
      Math.max(1, (process.stdout.rows || 24) - 2),
    );
    this.state.preferences = update.state;
    if (update.command === "cancel") {
      this.state.preferences = undefined;
      this.render();
    } else if (update.command === "save") {
      void this.savePreferences(update.state);
    } else if (update.command === "clear_transitions") {
      void this.clearTransitionHistory(update.state);
    } else {
      this.render();
    }
  }

  private preferenceAction(
    action: DashboardAction,
    preferences: PreferencesState,
  ): PreferenceAction | undefined {
    if (action === "escape")
      return preferences.confirmReset || preferences.confirmTransitionClear
        ? "decline"
        : "cancel";
    if (action === "scroll_down") return "focus_down";
    if (action === "scroll_up") return "focus_up";
    if (action === "page_down") return "page_down";
    if (action === "page_up") return "page_up";
    if (action === "previous") return "previous";
    if (action === "next") return "next";
    if (action === "toggle") return "toggle";
    if (action === "activate") return "activate";
    if (action === "move_up") return "move_up";
    if (action === "move_down") return "move_down";
    if (action === "save") return "save";
    if (action === "cancel") return "cancel";
    if (action === "reset") return "reset";
    if (action === "confirm") return "confirm";
    if (action === "decline") return "decline";
    return undefined;
  }

  private async savePreferences(preferences: PreferencesState) {
    const draft = cloneSettings(preferences.draft);
    const previous = this.state.settings ?? defaultSettings();
    this.state.preferences = {
      ...preferences,
      saving: true,
      notice: undefined,
    };
    this.render();
    try {
      await this.settings.save(draft);
      if (this.closed) return;
      this.state.settings = draft;
      this.state.settingsAvailability = "ready";
      const document = this.history.current?.();
      const { channels, providers } = this.baselineScope(previous, draft);
      if (!transitionPolicyEnabled(draft)) {
        this.state.transitions = { availability: "ready", events: [] };
      } else if (document && channels.length) {
        this.state.transitions = await this.runTransition(() =>
          this.transitions.baseline(
            document,
            draft,
            new Date(),
            channels,
            providers,
          ),
        );
      } else if (document) {
        this.state.transitions = await this.runTransition(() =>
          this.transitions.loadView(document, draft),
        );
      }
      this.state.preferences = undefined;
      this.state.transitionReview = false;
      this.state.scroll = clampDashboardScroll(
        this.state,
        process.stdout.rows || 24,
      );
    } catch {
      if (this.closed) return;
      this.state.preferences = {
        ...preferences,
        saving: false,
        notice: "save_failed",
      };
    }
    this.render();
  }

  private async loadSettings() {
    try {
      const loaded = await this.settings.load();
      this.state.settings = loaded.settings;
      this.state.settingsAvailability = loaded.availability;
    } catch {
      this.state.settings = defaultSettings();
      this.state.settingsAvailability = "unavailable";
    }
  }

  private async loadTransitions() {
    const settings = this.state.settings ?? defaultSettings();
    if (!transitionPolicyEnabled(settings) || !this.history.retained) {
      this.state.transitions = { availability: "first_run", events: [] };
      return;
    }
    const document = await this.history.retained();
    if (!document) {
      this.state.transitions = { availability: "unavailable", events: [] };
      return;
    }
    this.state.transitions = await this.runTransition(() =>
      this.transitions.loadView(document, settings),
    );
  }

  private baselineScope(
    previous: DashboardSettings,
    next: DashboardSettings,
  ): {
    channels: ("threshold" | "forecast")[];
    providers?: SupportedProvider[];
  } {
    const visibilityChanged = previous.hiddenProviders.filter(
      (provider) => !next.hiddenProviders.includes(provider),
    );
    visibilityChanged.push(
      ...next.hiddenProviders.filter(
        (provider) => !previous.hiddenProviders.includes(provider),
      ),
    );
    const thresholdChanged =
      previous.remainingThreshold !== next.remainingThreshold;
    const forecastChanged =
      previous.forecastBeforeReset !== next.forecastBeforeReset;
    return {
      channels: [
        ...(visibilityChanged.length > 0 || thresholdChanged
          ? (["threshold"] as const)
          : []),
        ...(visibilityChanged.length > 0 || forecastChanged
          ? (["forecast"] as const)
          : []),
      ],
      ...(visibilityChanged.length > 0 && !thresholdChanged && !forecastChanged
        ? { providers: visibilityChanged }
        : {}),
    };
  }

  private async acknowledgeTransitions() {
    const document = this.history.current?.();
    const settings = this.state.settings ?? defaultSettings();
    if (!document) return;
    const view = await this.runTransition(() =>
      this.transitions.acknowledge(document, settings),
    );
    if (this.closed) return;
    this.state.transitions = view;
    if (view.events.length === 0) this.state.transitionReview = false;
    this.render();
  }

  private async clearTransitionHistory(preferences: PreferencesState) {
    this.state.preferences = {
      ...preferences,
      saving: true,
      notice: undefined,
    };
    this.render();
    let view = await this.runTransition(() => this.transitions.clear());
    const document = this.history.current?.();
    const settings = this.state.settings ?? defaultSettings();
    if (
      view.availability !== "unavailable" &&
      view.availability !== "incompatible" &&
      document &&
      transitionPolicyEnabled(settings)
    ) {
      view = await this.runTransition(() =>
        this.transitions.baseline(document, settings),
      );
    }
    if (this.closed) return;
    const failed =
      view.availability === "unavailable" ||
      view.availability === "incompatible";
    this.state.transitions = view;
    this.state.transitionReview = false;
    this.state.preferences = {
      ...preferences,
      confirmTransitionClear: false,
      saving: false,
      notice: failed ? "transition_clear_failed" : "transition_history_cleared",
    };
    this.render();
  }

  private runTransition<T>(operation: () => Promise<T>): Promise<T> {
    const result = this.transitionQueue.then(operation, operation);
    this.transitionQueue = result.then(
      () => undefined,
      () => undefined,
    );
    return result;
  }

  private isScrollAction(
    action: DashboardAction,
  ): action is "scroll_down" | "scroll_up" | "page_down" | "page_up" {
    return (
      action === "scroll_down" ||
      action === "scroll_up" ||
      action === "page_down" ||
      action === "page_up"
    );
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
