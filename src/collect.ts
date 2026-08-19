import { spawn, type ChildProcess } from "node:child_process";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { adaptQuotaResponse } from "./schema.js";
import { sanitizeProcessError } from "./sanitize.js";
import type { QuotaReport } from "./types.js";

const MAX_STDOUT = 2 * 1024 * 1024;
const MAX_STDERR = 32 * 1024;
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
    let stderr = Buffer.alloc(0);
    let settled = false;
    const child = spawnProcess(executable, ["--json", "--full"], {
      cwd: resolve(dirname(fileURLToPath(import.meta.url)), ".."),
      env: { ...process.env, NO_COLOR: "1", TERM: "dumb" },
      stdio: ["ignore", "pipe", "pipe"],
      shell: false,
    });
    options.onChild?.(child, true);

    const finish = (error?: unknown, report?: QuotaReport) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      options.onChild?.(child, false);
      if (error) reject(new Error(sanitizeProcessError(error)));
      else if (report) resolvePromise(report);
      else reject(new Error("Quota refresh failed"));
    };

    const timer = setTimeout(() => {
      child.kill("SIGTERM");
      setTimeout(() => child.kill("SIGKILL"), 500).unref();
      finish(
        new Error(`quota-axi timed out after ${Math.ceil(timeoutMs / 1000)}s`),
      );
    }, timeoutMs);

    child.on("error", (error) => finish(error));
    child.stdout?.on("data", (chunk: Buffer) => {
      stdout = Buffer.concat([stdout, chunk]);
      if (stdout.byteLength > MAX_STDOUT) {
        child.kill("SIGTERM");
        finish(new Error("quota-axi output exceeded the 2 MiB safety limit"));
      }
    });
    child.stderr?.on("data", (chunk: Buffer) => {
      if (stderr.byteLength < MAX_STDERR)
        stderr = Buffer.concat([stderr, chunk]).subarray(0, MAX_STDERR);
    });
    child.on("close", (code) => {
      if (settled) return;
      if (code !== 0) {
        finish(
          new Error(
            stderr.toString("utf8") ||
              `quota-axi exited with status ${code ?? "unknown"}`,
          ),
        );
        return;
      }
      try {
        finish(
          undefined,
          adaptQuotaResponse(JSON.parse(stdout.toString("utf8"))),
        );
      } catch (error) {
        finish(error);
      }
    });
  });
}
