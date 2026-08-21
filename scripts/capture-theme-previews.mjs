import { mkdir, readFile, writeFile } from "node:fs/promises";
import { adaptQuotaResponse } from "../dist/schema.js";
import { renderDashboard } from "../dist/render.js";

const fixture = adaptQuotaResponse(
  JSON.parse(
    await readFile(
      new URL("../test/fixtures/complete.json", import.meta.url),
      "utf8",
    ),
  ),
);

const themes = [
  {
    id: "dark",
    name: "Herdr · Rosé Pine",
    background: "#191724",
    foreground: "#e0def4",
    border: "#6e6a86",
    red: "#eb6f92",
  },
  {
    id: "light",
    name: "Herdr · Rosé Pine Dawn",
    background: "#faf4ed",
    foreground: "#575279",
    border: "#9893a5",
    red: "#b4637a",
  },
];
const widths = [20, 24, 36];
const heights = [8, 23];
const cellWidth = 7.8;
const lineHeight = 16;
const panelPadding = 12;
const gap = 18;
const top = 52;
const ANSI_PATTERN = new RegExp(`${String.fromCharCode(27)}\\[([0-9;]*)m`, "g");

function escape(value) {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

function ansiSpans(line, theme) {
  const chunks = [];
  let offset = 0;
  let bold = false;
  let red = false;
  for (const match of line.matchAll(ANSI_PATTERN)) {
    const text = line.slice(offset, match.index);
    if (text) chunks.push({ text, bold, red });
    for (const code of (match[1] || "0").split(";").map(Number)) {
      if (code === 0) {
        bold = false;
        red = false;
      } else if (code === 1) bold = true;
      else if (code === 22) bold = false;
      else if (code === 31) red = true;
      else if (code === 39) red = false;
    }
    offset = (match.index ?? 0) + match[0].length;
  }
  const tail = line.slice(offset);
  if (tail) chunks.push({ text: tail, bold, red });
  return chunks
    .map(
      (chunk) =>
        `<tspan fill="${chunk.red ? theme.red : theme.foreground}" font-weight="${chunk.bold ? 700 : 400}">${escape(chunk.text)}</tspan>`,
    )
    .join("");
}

function panel(theme, width, height, x, y) {
  const text = renderDashboard(
    {
      report: fixture,
      history: {
        availability: "ready",
        evidence: {
          kind: "pace_worse",
          provider: "Claude",
          scope: "Fable",
          limit: "Fable",
        },
      },
      loading: false,
      scroll: 0,
    },
    {
      width,
      height,
      now: new Date("2026-08-18T18:00:00.000Z"),
      color: true,
    },
  );
  const panelWidth = panelPadding * 2 + width * cellWidth;
  const panelHeight = panelPadding * 2 + height * lineHeight;
  const lines = text.split("\n");
  return `<g transform="translate(${x} ${y})">
    <text x="0" y="-8" fill="${theme.foreground}" font-family="ui-monospace, SFMono-Regular, Menlo, Consolas, monospace" font-size="12" font-weight="700">${width} × ${height}</text>
    <rect width="${panelWidth}" height="${panelHeight}" rx="10" fill="${theme.background}" stroke="${theme.border}"/>
${lines
  .map(
    (line, index) =>
      `    <text x="${panelPadding}" y="${panelPadding + 12 + index * lineHeight}" xml:space="preserve" font-family="ui-monospace, SFMono-Regular, Menlo, Consolas, monospace" font-size="13">${ansiSpans(line, theme)}</text>`,
  )
  .join("\n")}
  </g>`;
}

await mkdir(new URL("../docs", import.meta.url), { recursive: true });
for (const theme of themes) {
  const columnWidths = widths.map(
    (width) => panelPadding * 2 + width * cellWidth,
  );
  const canvasWidth =
    gap * 4 + columnWidths.reduce((total, width) => total + width, 0);
  const rowHeights = heights.map(
    (height) => panelPadding * 2 + height * lineHeight,
  );
  const canvasHeight = top + gap * 3 + rowHeights[0] + rowHeights[1];
  const panels = [];
  let y = top;
  for (let row = 0; row < heights.length; row += 1) {
    let x = gap;
    for (let column = 0; column < widths.length; column += 1) {
      panels.push(panel(theme, widths[column], heights[row], x, y));
      x += columnWidths[column] + gap;
    }
    y += rowHeights[row] + gap * 2;
  }
  const svg = `<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" width="${canvasWidth}" height="${canvasHeight}" viewBox="0 0 ${canvasWidth} ${canvasHeight}">
  <rect width="100%" height="100%" fill="${theme.background}"/>
  <text x="${gap}" y="30" fill="${theme.foreground}" font-family="-apple-system, BlinkMacSystemFont, Segoe UI, sans-serif" font-size="18" font-weight="700">${theme.name} · exact PTY sizes</text>
  ${panels.join("\n  ")}
</svg>
`;
  await writeFile(
    new URL(`../docs/theme-preview-${theme.id}.svg`, import.meta.url),
    svg,
    "utf8",
  );
}
