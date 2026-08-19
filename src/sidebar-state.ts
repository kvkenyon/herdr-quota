import { readFileSync, rmdirSync, unlinkSync } from "node:fs";
import { dirname } from "node:path";

interface SidebarIdentity {
  phase?: string;
  token?: string;
}

export function releaseSidebarStateSync(
  path: string | undefined,
  token: string | undefined,
): boolean {
  if (!path || !token) return false;
  try {
    const state = JSON.parse(readFileSync(path, "utf8")) as SidebarIdentity;
    if (state.phase !== "open" || state.token !== token) return false;
    unlinkSync(path);
    removeEmptyStateDirectories(path);
    return true;
  } catch {
    return false;
  }
}

function removeEmptyStateDirectories(path: string) {
  const sessionDirectory = dirname(path);
  try {
    rmdirSync(sessionDirectory);
    rmdirSync(dirname(sessionDirectory));
  } catch {
    // Another tab or session still owns state in the shared runtime tree.
  }
}
