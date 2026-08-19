export type PixelLogo = readonly [string, string, string, string, string];

const LOGOS: Record<string, PixelLogo> = {
  // Claude's radial spark, OpenAI's woven rosette, Cursor's pointer, and
  // Kimi's crescent are deliberately ASCII-only so their silhouettes survive
  // monochrome terminals and fonts without block or private-use glyphs.
  claude: ["#  #  #", " # # # ", "#######", " # # # ", "#  #  #"],
  codex: ["  ###  ", " ## ## ", "## # ##", " ## ## ", "  ###  "],
  cursor: ["#      ", "##     ", "# #    ", "#  #   ", "#####  "],
  kimi: ["  ###  ", " ##    ", "##     ", " ##    ", "  ###  "],
};

const FALLBACK: PixelLogo = [
  "   #   ",
  "  # #  ",
  " #   # ",
  "  # #  ",
  "   #   ",
];

export function providerLogo(provider: string): PixelLogo {
  return LOGOS[provider.toLowerCase()] ?? FALLBACK;
}

export function isFallbackLogo(provider: string): boolean {
  return !(provider.toLowerCase() in LOGOS);
}
