export const NORMAL_REFRESH_MS = 5 * 60_000;
export const FAILURE_BACKOFF_MS = [10 * 60_000, 20 * 60_000, 30 * 60_000];

type Timer = ReturnType<typeof setTimeout>;

export interface RefreshSchedulerOptions<T> {
  collect: () => Promise<T>;
  onStart: () => void;
  onSuccess: (
    value: T,
    isCurrent: () => boolean,
  ) => void | Promise<void>;
  onFailure: (error: unknown) => void;
  onScheduled?: (delayMs: number, afterFailure: boolean) => void;
  onSettled: () => void;
  cancelActive: () => void;
  setTimer?: (callback: () => void, delayMs: number) => Timer;
  clearTimer?: (timer: Timer) => void;
}

/** Owns one completion-relative refresh loop without owning display state. */
export class RefreshScheduler<T> {
  private readonly options: RefreshSchedulerOptions<T>;
  private readonly setTimer: NonNullable<
    RefreshSchedulerOptions<T>["setTimer"]
  >;
  private readonly clearTimer: NonNullable<
    RefreshSchedulerOptions<T>["clearTimer"]
  >;
  private timer?: Timer;
  private sequence = 0;
  private failures = 0;
  private closed = false;
  private successTail: Promise<void> = Promise.resolve();

  constructor(options: RefreshSchedulerOptions<T>) {
    this.options = options;
    this.setTimer = options.setTimer ?? setTimeout;
    this.clearTimer = options.clearTimer ?? clearTimeout;
  }

  start(): Promise<void> {
    return this.refresh(false);
  }

  manual(): Promise<void> {
    this.failures = 0;
    return this.refresh(true);
  }

  close() {
    if (this.closed) return;
    this.closed = true;
    this.sequence++;
    this.clearPending();
    this.options.cancelActive();
  }

  private clearPending() {
    if (this.timer === undefined) return;
    this.clearTimer(this.timer);
    this.timer = undefined;
  }

  private schedule(delayMs: number) {
    this.clearPending();
    this.timer = this.setTimer(() => {
      this.timer = undefined;
      void this.refresh(false);
    }, delayMs);
    (this.timer as Timer & { unref?: () => void }).unref?.();
  }

  private async refresh(preempt: boolean): Promise<void> {
    if (this.closed) return;
    this.clearPending();
    const sequence = ++this.sequence;
    if (preempt) this.options.cancelActive();
    this.options.onStart();
    let succeeded = false;
    try {
      const value = await this.options.collect();
      if (this.closed || sequence !== this.sequence) return;
      const previousSuccess = this.successTail;
      let releaseSuccess!: () => void;
      this.successTail = new Promise<void>((resolve) => {
        releaseSuccess = resolve;
      });
      await previousSuccess;
      try {
        if (this.closed || sequence !== this.sequence) return;
        await this.options.onSuccess(
          value,
          () => !this.closed && sequence === this.sequence,
        );
      } finally {
        releaseSuccess();
      }
      this.failures = 0;
      succeeded = true;
    } catch (error) {
      if (this.closed || sequence !== this.sequence) return;
      this.failures++;
      this.options.onFailure(error);
    } finally {
      if (!this.closed && sequence === this.sequence) {
        const delay = succeeded
          ? NORMAL_REFRESH_MS
          : (FAILURE_BACKOFF_MS[
              Math.min(this.failures - 1, FAILURE_BACKOFF_MS.length - 1)
            ] ?? FAILURE_BACKOFF_MS.at(-1)!);
        this.schedule(delay);
        this.options.onScheduled?.(delay, !succeeded);
        this.options.onSettled();
      }
    }
  }
}
