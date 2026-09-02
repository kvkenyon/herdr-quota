import { randomUUID } from "node:crypto";
import { constants } from "node:fs";
import { chmod, lstat, mkdir, open, rename, unlink } from "node:fs/promises";
import { homedir } from "node:os";
import { dirname, join } from "node:path";
import { MARKETED_PROVIDERS, type SupportedProvider } from "./types.js";

export const SETTINGS_SCHEMA_VERSION = 3;
export const SUPPORTED_PROVIDERS: readonly SupportedProvider[] =
  MARKETED_PROVIDERS.map((provider) => provider.id);

export type { SupportedProvider } from "./types.js";
export type MeterMode = "remaining" | "used";
export type RemainingThreshold = "off" | 25 | 10 | 5;

export interface DashboardSettings {
  schemaVersion: 3;
  providerOrder: SupportedProvider[];
  hiddenProviders: SupportedProvider[];
  meterMode: MeterMode;
  remainingThreshold: RemainingThreshold;
  forecastBeforeReset: boolean;
}

export type SettingsAvailability =
  "ready" | "first_run" | "recovered" | "incompatible" | "unavailable";

export interface SettingsLoadResult {
  settings: DashboardSettings;
  availability: SettingsAvailability;
}

interface FileStat {
  isFile(): boolean;
  isSymbolicLink(): boolean;
  mode: number;
  uid: number;
}

export interface SettingsFileHandle {
  stat(): Promise<{ isFile(): boolean }>;
  readFile(options: { encoding: "utf8" }): Promise<string>;
  writeFile(value: string, options: { encoding: "utf8" }): Promise<unknown>;
  sync(): Promise<unknown>;
  close(): Promise<unknown>;
}

export interface SettingsFileOperations {
  lstat(path: string): Promise<FileStat>;
  mkdir(
    path: string,
    options: { recursive: true; mode: number },
  ): Promise<unknown>;
  open(path: string, flags: number, mode?: number): Promise<SettingsFileHandle>;
  rename(from: string, to: string): Promise<unknown>;
  unlink(path: string): Promise<unknown>;
  chmod(path: string, mode: number): Promise<unknown>;
}

const FILE_OPERATIONS: SettingsFileOperations = {
  lstat: (path) => lstat(path),
  mkdir: (path, options) => mkdir(path, options),
  open: (path, flags, mode) => open(path, flags, mode),
  rename: (from, to) => rename(from, to),
  unlink: (path) => unlink(path),
  chmod: (path, mode) => chmod(path, mode),
};

class SettingsDocumentError extends Error {
  constructor(readonly kind: "corrupt" | "incompatible" | "unsafe") {
    super(`settings_${kind}`);
  }
}

function errorCode(error: unknown): string | undefined {
  return (error as NodeJS.ErrnoException | undefined)?.code;
}

function isObject(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function isStringArray(value: unknown): value is string[] {
  return (
    Array.isArray(value) &&
    value.every((item: unknown): item is string => typeof item === "string")
  );
}

export function isSupportedProvider(value: string): value is SupportedProvider {
  return (SUPPORTED_PROVIDERS as readonly string[]).includes(value);
}

function providerList(
  value: unknown,
  appendMissing: boolean,
): SupportedProvider[] {
  if (!isStringArray(value)) throw new SettingsDocumentError("corrupt");

  const known = value.filter((item): item is SupportedProvider =>
    isSupportedProvider(item),
  );
  if (new Set(known).size !== known.length)
    throw new SettingsDocumentError("corrupt");

  return appendMissing
    ? [
        ...known,
        ...SUPPORTED_PROVIDERS.filter((provider) => !known.includes(provider)),
      ]
    : known;
}

export function defaultSettings(): DashboardSettings {
  return {
    schemaVersion: SETTINGS_SCHEMA_VERSION,
    providerOrder: [...SUPPORTED_PROVIDERS],
    hiddenProviders: [],
    meterMode: "remaining",
    remainingThreshold: "off",
    forecastBeforeReset: false,
  };
}

export function cloneSettings(settings: DashboardSettings): DashboardSettings {
  return {
    schemaVersion: SETTINGS_SCHEMA_VERSION,
    providerOrder: [...settings.providerOrder],
    hiddenProviders: [...settings.hiddenProviders],
    meterMode: settings.meterMode,
    remainingThreshold: settings.remainingThreshold,
    forecastBeforeReset: settings.forecastBeforeReset,
  };
}

export function normalizeSettings(value: DashboardSettings): DashboardSettings {
  return {
    schemaVersion: SETTINGS_SCHEMA_VERSION,
    providerOrder: providerList(value.providerOrder, true),
    hiddenProviders: providerList(value.hiddenProviders, false),
    meterMode: value.meterMode === "used" ? "used" : "remaining",
    remainingThreshold:
      value.remainingThreshold === 25 ||
      value.remainingThreshold === 10 ||
      value.remainingThreshold === 5
        ? value.remainingThreshold
        : "off",
    forecastBeforeReset: value.forecastBeforeReset === true,
  };
}

/**
 * Reads only the finite settings schemas. Extra object fields and future
 * provider ids are ignored. This prevents data from a newer writer from
 * entering the runtime or making a downgrade brittle.
 */
export function parseSettingsDocument(value: unknown): DashboardSettings {
  if (!isObject(value)) throw new SettingsDocumentError("corrupt");
  if (
    value.schemaVersion !== 1 &&
    value.schemaVersion !== 2 &&
    value.schemaVersion !== SETTINGS_SCHEMA_VERSION
  ) {
    if (typeof value.schemaVersion === "number")
      throw new SettingsDocumentError("incompatible");
    throw new SettingsDocumentError("corrupt");
  }
  if (value.meterMode !== "remaining" && value.meterMode !== "used")
    throw new SettingsDocumentError("corrupt");
  if (
    value.schemaVersion !== 1 &&
    value.remainingThreshold !== "off" &&
    value.remainingThreshold !== 25 &&
    value.remainingThreshold !== 10 &&
    value.remainingThreshold !== 5
  )
    throw new SettingsDocumentError("corrupt");
  if (
    value.schemaVersion !== 1 &&
    typeof value.forecastBeforeReset !== "boolean"
  )
    throw new SettingsDocumentError("corrupt");

  return {
    schemaVersion: SETTINGS_SCHEMA_VERSION,
    providerOrder: providerList(value.providerOrder, true),
    hiddenProviders: providerList(value.hiddenProviders, false),
    meterMode: value.meterMode,
    remainingThreshold:
      value.schemaVersion === 1
        ? "off"
        : (value.remainingThreshold as RemainingThreshold),
    forecastBeforeReset:
      value.schemaVersion !== 1 ? value.forecastBeforeReset === true : false,
  };
}

export function settingsPath(
  environment: NodeJS.ProcessEnv = process.env,
  home = homedir(),
): string {
  const configHome = environment.XDG_CONFIG_HOME || join(home, ".config");
  return join(configHome, "herdr-quota", "settings.json");
}

async function regularTarget(
  path: string,
  operations: SettingsFileOperations,
): Promise<boolean> {
  try {
    const stat = await operations.lstat(path);
    if (stat.isSymbolicLink() || !stat.isFile())
      throw new SettingsDocumentError("unsafe");
    return true;
  } catch (error) {
    if (errorCode(error) === "ENOENT") return false;
    throw error;
  }
}

async function replaceableTarget(
  path: string,
  operations: SettingsFileOperations,
): Promise<boolean> {
  let stat: FileStat;
  try {
    stat = await operations.lstat(path);
  } catch (error) {
    if (errorCode(error) === "ENOENT") return false;
    throw error;
  }
  if (stat.isSymbolicLink() || !stat.isFile())
    throw new SettingsDocumentError("unsafe");
  const currentUid = process.getuid?.();
  if (
    (currentUid !== undefined && stat.uid !== currentUid) ||
    (stat.mode & 0o200) === 0
  )
    throw new SettingsDocumentError("unsafe");

  const text = await readRegularFile(path, operations);
  try {
    parseSettingsDocument(JSON.parse(text as string) as unknown);
  } catch (error) {
    if (error instanceof SettingsDocumentError && error.kind === "incompatible")
      throw error;
  }
  return true;
}

async function readRegularFile(
  path: string,
  operations: SettingsFileOperations,
): Promise<string | undefined> {
  if (!(await regularTarget(path, operations))) return undefined;
  const noFollow = constants.O_NOFOLLOW ?? 0;
  const handle = await operations.open(path, constants.O_RDONLY | noFollow);
  try {
    if (!(await handle.stat()).isFile())
      throw new SettingsDocumentError("unsafe");
    return await handle.readFile({ encoding: "utf8" });
  } finally {
    await handle.close();
  }
}

function serializedSettings(settings: DashboardSettings): string {
  const normalized = normalizeSettings(settings);
  return `${JSON.stringify({
    schemaVersion: SETTINGS_SCHEMA_VERSION,
    providerOrder: normalized.providerOrder,
    hiddenProviders: normalized.hiddenProviders,
    meterMode: normalized.meterMode,
    remainingThreshold: normalized.remainingThreshold,
    forecastBeforeReset: normalized.forecastBeforeReset,
  })}\n`;
}

const LOCK_ATTEMPTS = 50;
const LOCK_DELAY_MS = 10;

async function withSettingsLock<T>(
  path: string,
  operations: SettingsFileOperations,
  action: () => Promise<T>,
): Promise<T> {
  const lockPath = `${path}.lock`;
  let handle: SettingsFileHandle | undefined;
  for (let attempt = 0; attempt < LOCK_ATTEMPTS; attempt += 1) {
    try {
      handle = await operations.open(
        lockPath,
        constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL,
        0o600,
      );
      break;
    } catch (error) {
      if (errorCode(error) !== "EEXIST" || attempt === LOCK_ATTEMPTS - 1)
        throw error;
      await new Promise((resolve) => setTimeout(resolve, LOCK_DELAY_MS));
    }
  }
  if (!handle) throw new Error("settings_lock_unavailable");
  try {
    return await action();
  } finally {
    await handle.close().catch(() => undefined);
    await operations.unlink(lockPath).catch(() => undefined);
  }
}

export class SettingsStore {
  constructor(
    readonly path = settingsPath(),
    private readonly operations: SettingsFileOperations = FILE_OPERATIONS,
  ) {}

  async load(): Promise<SettingsLoadResult> {
    let text: string | undefined;
    try {
      text = await readRegularFile(this.path, this.operations);
      if (text === undefined)
        return { settings: defaultSettings(), availability: "first_run" };
      return {
        settings: parseSettingsDocument(JSON.parse(text) as unknown),
        availability: "ready",
      };
    } catch (error) {
      if (
        error instanceof SettingsDocumentError &&
        error.kind === "incompatible"
      ) {
        return { settings: defaultSettings(), availability: "incompatible" };
      }
      if (
        error instanceof SyntaxError ||
        (error instanceof SettingsDocumentError && error.kind === "corrupt")
      ) {
        if (text !== undefined)
          await this.quarantine(text).catch(() => undefined);
        return { settings: defaultSettings(), availability: "recovered" };
      }
      return { settings: defaultSettings(), availability: "unavailable" };
    }
  }

  async save(settings: DashboardSettings): Promise<void> {
    await this.operations.mkdir(dirname(this.path), {
      recursive: true,
      mode: 0o700,
    });
    await withSettingsLock(this.path, this.operations, async () => {
      await replaceableTarget(this.path, this.operations);

      const temporary = `${this.path}.${process.pid}.${randomUUID()}.tmp`;
      const noFollow = constants.O_NOFOLLOW ?? 0;
      let handle: SettingsFileHandle | undefined;
      let renamed = false;
      try {
        handle = await this.operations.open(
          temporary,
          constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL | noFollow,
          0o600,
        );
        await handle.writeFile(serializedSettings(settings), {
          encoding: "utf8",
        });
        await handle.sync();
        await handle.close();
        handle = undefined;

        await replaceableTarget(this.path, this.operations);
        await this.operations.rename(temporary, this.path);
        renamed = true;
      } finally {
        await handle?.close().catch(() => undefined);
        if (!renamed)
          await this.operations.unlink(temporary).catch(() => undefined);
      }
    });
  }

  private async quarantine(expected: string): Promise<void> {
    await withSettingsLock(this.path, this.operations, async () => {
      const current = await readRegularFile(this.path, this.operations);
      if (current === undefined || current !== expected) return;
      const quarantine = `${this.path}.invalid-${Date.now()}-${randomUUID()}`;
      await this.operations.rename(this.path, quarantine);
      await this.operations.chmod(quarantine, 0o600).catch(() => undefined);
    });
  }
}
