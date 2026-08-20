const ESCAPE = new RegExp(String.raw`\x1B\[[0-?]*[ -/]*[@-~]`, "g");

export function stripAnsi(value: string): string {
  return value.replaceAll(ESCAPE, "");
}

export function visibleLength(value: string): number {
  return stripAnsi(value).length;
}

export function truncate(value: string, width: number): string {
  if (width <= 0) return "";
  const plain = stripAnsi(value);
  if (plain.length <= width) return value;
  return width === 1 ? "…" : `${plain.slice(0, width - 1)}…`;
}

export function pad(value: string, width: number): string {
  const clean = truncate(value, width);
  return clean + " ".repeat(Math.max(0, width - visibleLength(clean)));
}
