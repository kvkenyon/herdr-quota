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
  | "pace_worse"
  | "pace_better"
  | "projection_earlier"
  | "projection_later"
  | "series";

export interface HistoryEvidence {
  kind: HistoryEvidenceKind;
  provider: string;
  scope: string;
  limit?: string;
  amount?: number;
  remainingSeries?: number[];
}

export interface HistoryView {
  availability: HistoryAvailability;
  evidence?: HistoryEvidence;
}

export interface DashboardState {
  report?: QuotaReport;
  history?: HistoryView;
  loading: boolean;
  failure?: CollectorFailure;
  lastAttemptAt?: Date;
  scroll: number;
}
