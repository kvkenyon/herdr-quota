export type ProviderMark = readonly [string, string];

const MARKS: Record<string, ProviderMark> = {
  claude: ["\\|/", "-*-"],
  codex: ["o-o", "-o-"],
  cursor: ["|\\ ", "|_>"],
  kimi: ["(_ ", " (_"],
};

const FALLBACK: ProviderMark = [" . ", "/ \\"];

export function providerLogo(provider: string): ProviderMark {
  return MARKS[provider.toLowerCase()] ?? FALLBACK;
}

export function isFallbackLogo(provider: string): boolean {
  return !(provider.toLowerCase() in MARKS);
}
