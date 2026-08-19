export type PixelLogo = readonly string[];

const LOGOS: Record<string, PixelLogo> = {
  claude: ["# #", "###", "# #"],
  codex: ["###", "#  ", "###"],
  cursor: ["#  ", "## ", "###"],
  kimi: [" ##", "#  ", " ##"],
};

const FALLBACK: PixelLogo = [" # ", "# #", " # "];

export function providerLogo(provider: string): PixelLogo {
  return LOGOS[provider.toLowerCase()] ?? FALLBACK;
}

export function isFallbackLogo(provider: string): boolean {
  return !(provider.toLowerCase() in LOGOS);
}
