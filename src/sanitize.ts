const SAFE_CODES: Record<string, string> = {
  credentials_missing: "Sign-in not found",
  credentials_invalid: "Sign-in could not be read",
  credentials_expired: "Sign-in expired",
  keychain_prompt_required: "Keychain approval required",
  provider_auth_rejected: "Provider rejected sign-in",
  provider_rate_limited: "Provider temporarily rate limited",
  request_timeout: "Provider request timed out",
  network_unavailable: "Network unavailable",
  schema_invalid: "Provider response changed",
  quota_unavailable: "Quota unavailable",
};

const TERMINAL_STRING = new RegExp(
  "\\u001b(?:\\][^\\u0007\\u001b]*(?:\\u0007|\\u001b\\\\)|[PX^_][\\s\\S]*?\\u001b\\\\)",
  "g",
);
const TERMINAL_SEQUENCE = new RegExp(
  "(?:\\u001b\\[[0-?]*[ -/]*[@-~]|\\u001b[ -/]*[@-~])",
  "g",
);
const TERMINAL_CONTROL = new RegExp(
  "[\\u0000-\\u0008\\u000b\\u000c\\u000e-\\u001f\\u007f-\\u009f]",
  "g",
);

export function friendlyProviderError(code?: string): string {
  if (!code) return "Quota unavailable";
  const normalized = code.toLowerCase().replaceAll(/[^a-z0-9]+/g, "_");
  return SAFE_CODES[normalized] ?? "Quota unavailable";
}

export function sanitizeProcessError(value: unknown): string {
  let text: string;
  if (value instanceof Error) text = value.message;
  else if (typeof value === "string") text = value;
  else if (typeof value === "number" || typeof value === "boolean")
    text = String(value);
  else text = "Unknown error";
  text = text
    .replaceAll(TERMINAL_STRING, "")
    .replaceAll(TERMINAL_SEQUENCE, "")
    .replaceAll(TERMINAL_CONTROL, "")
    .replaceAll(/Bearer\s+[A-Za-z0-9._~+/-]+/gi, "Bearer [redacted]")
    .replaceAll(/(?:sk|api|key|token)[-_][A-Za-z0-9._~+/-]{8,}/gi, "[redacted]")
    .replaceAll(/\b[A-Za-z0-9._~+/-]{24,}\b/g, "[redacted]")
    .replaceAll(/(?:\/Users\/|\/home\/)[^\s:'"]+/g, "[local path]")
    .replaceAll(/[A-Z]:\\[^\s:'"]+/gi, "[local path]")
    .replaceAll(/[\w.+-]+@[\w.-]+\.[A-Za-z]{2,}/g, "[account]")
    .replaceAll(/[\r\n\t]+/g, " ")
    .trim();
  return text.slice(0, 180) || "Quota refresh failed";
}
