import { execFile } from "node:child_process";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

const run = promisify(execFile);
const root = fileURLToPath(new URL("../", import.meta.url));

await run(
  "cargo",
  [
    "run",
    "--quiet",
    "--manifest-path",
    "rust/herdr-quota/Cargo.toml",
    "--",
    "preview",
    "--fixture",
    "test/fixtures/launch.json",
    "--width",
    "36",
    "--height",
    "12",
    "--svg",
    "docs/dashboard-preview.svg",
  ],
  { cwd: root },
);
