import type { CollectorFailure, CollectorFailureKind } from "./types.js";

const SAFE_MESSAGES: Record<CollectorFailureKind, string> = {
  timeout: "Quota check timed out",
  missing_executable: "quota-axi executable is missing",
  incompatible_output: "quota-axi output is incompatible",
  network_process: "Quota network/process check failed",
};

/** A collector error whose public fields contain only allow-listed values. */
export class CollectorFailureError extends Error {
  readonly kind: CollectorFailureKind;

  constructor(kind: CollectorFailureKind) {
    super(SAFE_MESSAGES[kind]);
    this.name = "CollectorFailureError";
    this.kind = kind;
  }
}

export function missingExecutable(error: unknown): boolean {
  return (
    !!error &&
    typeof error === "object" &&
    "code" in error &&
    (error as { code?: unknown }).code === "ENOENT"
  );
}

/** Converts even arbitrary child/process errors into a finite display-safe state. */
export function safeCollectorFailure(error: unknown): CollectorFailure {
  if (error instanceof CollectorFailureError) return { kind: error.kind };
  return {
    kind: missingExecutable(error) ? "missing_executable" : "network_process",
  };
}
