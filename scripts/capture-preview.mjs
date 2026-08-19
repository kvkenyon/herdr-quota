import { readFile, writeFile, mkdir } from "node:fs/promises";
import { adaptQuotaResponse } from "../dist/schema.js";
import { renderPlain } from "../dist/render.js";

const fixture = JSON.parse(
  await readFile(
    new URL("../test/fixtures/complete.json", import.meta.url),
    "utf8",
  ),
);
const text = renderPlain(
  { report: adaptQuotaResponse(fixture), loading: false, scroll: 0 },
  { width: 36, height: 23, now: new Date("2026-08-18T18:00:00.000Z") },
);
const lines = text.split("\n");
const escape = (value) =>
  value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
const lineHeight = 18;
const padding = 24;
const svgWidth = 340;
const svg = `<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" width="${svgWidth}" height="${padding * 2 + lines.length * lineHeight}" viewBox="0 0 ${svgWidth} ${padding * 2 + lines.length * lineHeight}">
  <rect width="${svgWidth}" height="100%" rx="14" fill="#0d1117"/>
  <circle cx="24" cy="18" r="5" fill="#ff5f56"/><circle cx="42" cy="18" r="5" fill="#ffbd2e"/><circle cx="60" cy="18" r="5" fill="#27c93f"/>
  <g fill="#e6edf3" font-family="ui-monospace, SFMono-Regular, Menlo, Consolas, monospace" font-size="13">
${lines.map((line, index) => `    <text x="24" y="${padding + 18 + index * lineHeight}" xml:space="preserve">${escape(line)}</text>`).join("\n")}
  </g>
</svg>
`;
await mkdir(new URL("../docs", import.meta.url), { recursive: true });
await writeFile(
  new URL("../docs/dashboard-preview.svg", import.meta.url),
  svg,
  "utf8",
);
