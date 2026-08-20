import { spawn, type ChildProcess } from "node:child_process";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { CollectorFailureError, missingExecutable } from "./failure.js";
import { adaptQuotaResponse } from "./schema.js";
import { ALLOWED_PROVIDERS } from "./tiers.js";
import type { QuotaReport } from "./types.js";

const MAX_STDOUT = 2 * 1024 * 1024;
export const DEFAULT_TIMEOUT_MS = 12_000;

export interface CollectorOptions {
  executable?: string;
  timeoutMs?: number;
  spawnProcess?: typeof spawn;
  onChild?: (child: ChildProcess, active: boolean) => void;
}

export function localQuotaAxiExecutable(): string {
  const here = dirname(fileURLToPath(import.meta.url));
  return resolve(
    here,
    "..",
    "node_modules",
    ".bin",
    process.platform === "win32" ? "quota-axi.cmd" : "quota-axi",
  );
}

export async function collectQuota(
  options: CollectorOptions = {},
): Promise<QuotaReport> {
  const executable = options.executable ?? localQuotaAxiExecutable();
  const timeoutMs = options.timeoutMs ?? DEFAULT_TIMEOUT_MS;
  const spawnProcess = options.spawnProcess ?? spawn;

  return await new Promise<QuotaReport>((resolvePromise, reject) => {
    let stdout = Buffer.alloc(0);
    let settled = false;
    const args = [
      "--json",
      "--full",
      "--provider",
      ALLOWED_PROVIDERS.join(","),
    ];
    let child: ChildProcess;
    try {
      child = spawnProcess(executable, args, {
        cwd: resolve(dirname(fileURLToPath(import.meta.url)), ".."),
        env: { ...process.env, NO_COLOR: "1", TERM: "dumb" },
        stdio: ["ignore", "pipe", "pipe"],
        shell: false,
      });
    } catch (error) {
      reject(
        new CollectorFailureError(
          missingExecutable(error) ? "missing_executable" : "network_process",
        ),
      );
      return;
    }
    options.onChild?.(child, true);

    const finish = (error?: CollectorFailureError, report?: QuotaReport) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      options.onChild?.(child, false);
      if (error) reject(error);
      else if (report) resolvePromise(report);
      else reject(new CollectorFailureError("network_process"));
    };

    const timer = setTimeout(() => {
      child.kill("SIGTERM");
      setTimeout(() => child.kill("SIGKILL"), 500).unref();
      finish(new CollectorFailureError("timeout"));
    }, timeoutMs);

    child.on("error", (error) =>
      finish(
        new CollectorFailureError(
          missingExecutable(error) ? "missing_executable" : "network_process",
        ),
      ),
    );
    child.stdout?.on("data", (chunk: Buffer) => {
      stdout = Buffer.concat([stdout, chunk]);
      if (stdout.byteLength > MAX_STDOUT) {
        child.kill("SIGTERM");
        finish(new CollectorFailureError("incompatible_output"));
      }
    });
    // Drain but never retain arbitrary child output. The UI consumes only the
    // allow-listed failure kind selected here.
    child.stderr?.resume();
    child.on("close", (code) => {
      if (settled) return;
      if (code !== 0) {
        finish(new CollectorFailureError("network_process"));
        return;
      }
      try {
        finish(
          undefined,
          adaptQuotaResponse(JSON.parse(stdout.toString("utf8"))),
        );
      } catch {
        finish(new CollectorFailureError("incompatible_output"));
      }
    });
  });
}
