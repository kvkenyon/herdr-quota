import type { DashboardSettings, SettingsAvailability } from "./settings.js";

/**
 * The complete product-owned provider catalog. Keep this finite: quota-axi
 * can report other providers, but this sidebar does not query, render, or
 * persist them.
 */
export const MARKETED_PROVIDERS = [
  {
    id: "claude",
    label: "Claude",
    recoveryInstruction: "claude, then /login",
  },
  { id: "codex", label: "OpenAI Codex", recoveryInstruction: "codex login" },
  { id: "cursor", label: "Cursor", recoveryInstruction: "cursor-agent login" },
  { id: "kimi", label: "Kimi", recoveryInstruction: "kimi login" },
] as const;

export type SupportedProvider = (typeof MARKETED_PROVIDERS)[number]["id"];
export type MarketedProviderLabel =
  (typeof MARKETED_PROVIDERS)[number]["label"];

export function marketedProvider(
  id: string,
): (typeof MARKETED_PROVIDERS)[number] | undefined {
  return MARKETED_PROVIDERS.find((provider) => provider.id === id);
}

export function marketedProviderLabel(
  label: string,
): (typeof MARKETED_PROVIDERS)[number] | undefined {
  return MARKETED_PROVIDERS.find((provider) => provider.label === label);
}

export type PaceStatus = "ahead" | "on_pace" | "behind" | "mixed" | "unknown";
export type RunwayStatus =
  "exhausted_now" | "projected_exhaustion" | "through_reset" | "unknown";

export interface QuotaWindow {
  id: string;
  label: string;
  kind: string;
  percentUsed?: number;
  percentRemaining?: number;
  startsAt?: string;
  resetsAt?: string;
  resetText?: string;
  windowSeconds?: number;
  spentUsd?: number;
  limitUsd?: number;
  pace?: {
    status: PaceStatus;
    reservePercentPoints?: number;
    burnMultiple?: number;
    projectedExhaustedAt?: string;
    projectionConfidence?: "early" | "established";
  };
}

export interface EffectiveAvailability {
  scope: string;
  status: "known" | "unknown";
  effectivePercentRemaining?: number;
  boundedBy: string[];
  limitingWindowIds?: string[];
  pace?: {
    status: PaceStatus;
    worstReservePercentPoints?: number;
    worstReserveWindowId?: string;
    unknownWindowIds?: string[];
  };
  runway?: {
    status: RunwayStatus;
    usableRunwaySeconds?: number;
    projectedExhaustedAt?: string;
    limitingWindowId?: string;
    projectionConfidence?: "early" | "established";
    unmeasurableWindowIds?: string[];
  };
}

export interface ProviderQuota {
  provider: string;
  label?: string;
  source?: string;
  plan?: string;
  windows: QuotaWindow[];
  effective: EffectiveAvailability[];
  semanticsStatus?: "known" | "partial" | "unknown";
  credits?: {
    remaining?: number;
    unlimited?: boolean;
    unit?: "usd" | "credits";
  };
  state: {
    status: string;
    stale: boolean;
    refreshedAt?: string;
    authStatus?: string;
    reason?: string;
    remedyCommand?: string;
    errorCode?: string;
  };
}

export interface QuotaReport {
  generatedAt: string;
  schemaVersion: 5;
  providers: ProviderQuota[];
  adaptationWarnings: string[];
}

export type CollectorFailureKind =
  "timeout" | "missing_executable" | "incompatible_output" | "network_process";

export interface CollectorFailure {
  kind: CollectorFailureKind;
  retryAt?: Date;
}

export type HistoryAvailability =
  | "ready"
  | "first_run"
  | "recovered"
  | "incompatible"
  | "unavailable"
  | "clock_skew"
  | "no_usable_data";

export type HistoryEvidenceKind =
  | "reset"
  | "remaining_drop"
  | "remaining_gain"
  | "pace_worse"
  | "pace_better"
  | "projection_earlier"
  | "projection_later";

export interface HistoryEvidence {
  kind: HistoryEvidenceKind;
  provider: string;
  scope: string;
  limit?: string;
  amount?: number;
}

export interface HistoryView {
  availability: HistoryAvailability;
  evidence?: HistoryEvidence;
}

export type TransitionAvailability =
  | "ready"
  | "first_run"
  | "recovered"
  | "incompatible"
  | "unavailable"
  | "clock_skew";

export type TransitionKind =
  | "threshold_enter"
  | "threshold_recovery"
  | "forecast_enter"
  | "forecast_recovery";

export interface TransitionDisplayEvent {
  kind: TransitionKind;
  provider: string;
  scope: string;
  limit?: string;
  threshold: "off" | 25 | 10 | 5;
  occurredAt: string;
  remaining?: number;
}

export interface TransitionView {
  availability: TransitionAvailability;
  events: TransitionDisplayEvent[];
}

export type PreferenceFocus =
  | SupportedProvider
  | "meter"
  | "threshold"
  | "forecast"
  | "save"
  | "cancel"
  | "reset"
  | "clear_transitions";

export interface PreferencesState {
  draft: DashboardSettings;
  focus: PreferenceFocus;
  confirmReset: boolean;
  confirmTransitionClear: boolean;
  saving: boolean;
  notice?:
    "save_failed" | "transition_clear_failed" | "transition_history_cleared";
}

export interface DashboardState {
  report?: QuotaReport;
  history?: HistoryView;
  loading: boolean;
  failure?: CollectorFailure;
  lastAttemptAt?: Date;
  scroll: number;
  settings?: DashboardSettings;
  settingsAvailability?: SettingsAvailability;
  preferences?: PreferencesState;
  transitions?: TransitionView;
  transitionReview?: boolean;
}
