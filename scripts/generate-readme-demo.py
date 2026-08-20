#!/usr/bin/env python3
"""Generate the README hero animation from the deterministic dashboard preview."""

from __future__ import annotations

import math
import re
import shutil
import subprocess
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PREVIEW = ROOT / "docs" / "dashboard-preview.svg"
OUTPUT = ROOT / "docs" / "readme-demo.gif"
WIDTH = 960
HEIGHT = 540
FPS = 12
FRAMES = 72
PANEL_X = 556
PANEL_Y = 39


def require(command: str) -> None:
    if shutil.which(command) is None:
        raise SystemExit(f"{command} is required to generate {OUTPUT.relative_to(ROOT)}")


def panel_x(frame: int) -> float:
    # The real product is visible in frame zero for README/social previews.
    # Motion belongs to the attention glow, not a delayed product reveal.
    return PANEL_X


def glow_opacity(frame: int) -> float:
    if frame < 24:
        return 0.0
    phase = min((frame - 24) / 18, 1.0)
    fade = min((FRAMES - frame - 1) / 12, 1.0)
    return max(0.0, 0.16 + 0.20 * math.sin(phase * math.pi) ** 2) * max(0.0, fade)


def dashboard_inner() -> str:
    source = PREVIEW.read_text()
    match = re.search(r"<svg[^>]*>(.*)</svg>\s*$", source, re.DOTALL)
    if not match:
        raise SystemExit(f"cannot parse {PREVIEW}")
    return match.group(1)


def frame_svg(frame: int, dashboard: str) -> str:
    x = panel_x(frame)
    glow = glow_opacity(frame)
    return f'''<svg xmlns="http://www.w3.org/2000/svg" width="{WIDTH}" height="{HEIGHT}" viewBox="0 0 {WIDTH} {HEIGHT}">
  <defs>
    <radialGradient id="wash" cx="72%" cy="44%" r="72%">
      <stop offset="0" stop-color="#18344a" stop-opacity="0.48"/>
      <stop offset="0.54" stop-color="#0b1823" stop-opacity="0.20"/>
      <stop offset="1" stop-color="#070b10" stop-opacity="0"/>
    </radialGradient>
    <linearGradient id="accent" x1="0" y1="0" x2="1" y2="0">
      <stop offset="0" stop-color="#58a6ff"/>
      <stop offset="1" stop-color="#3fb950"/>
    </linearGradient>
    <filter id="shadow" x="-40%" y="-30%" width="180%" height="170%">
      <feDropShadow dx="0" dy="18" stdDeviation="22" flood-color="#000000" flood-opacity="0.55"/>
    </filter>
    <filter id="glow" x="-50%" y="-50%" width="200%" height="200%">
      <feGaussianBlur stdDeviation="16"/>
    </filter>
    <pattern id="grid" width="32" height="32" patternUnits="userSpaceOnUse">
      <path d="M 32 0 L 0 0 0 32" fill="none" stroke="#8b949e" stroke-opacity="0.045"/>
    </pattern>
  </defs>
  <rect width="100%" height="100%" rx="24" fill="#070b10"/>
  <rect width="100%" height="100%" rx="24" fill="url(#grid)"/>
  <rect width="100%" height="100%" rx="24" fill="url(#wash)"/>
  <rect x="60" y="58" width="80" height="3" rx="1.5" fill="url(#accent)"/>
  <text x="60" y="100" fill="#8b949e" font-family="-apple-system, BlinkMacSystemFont, Segoe UI, sans-serif" font-size="13" font-weight="700" letter-spacing="2.2">HERDR PLUGIN</text>
  <text x="60" y="167" fill="#f0f6fc" font-family="-apple-system, BlinkMacSystemFont, Segoe UI, sans-serif" font-size="46" font-weight="700" letter-spacing="-1.2">See the next limit</text>
  <text x="60" y="218" fill="#f0f6fc" font-family="-apple-system, BlinkMacSystemFont, Segoe UI, sans-serif" font-size="46" font-weight="700" letter-spacing="-1.2">before it blocks work.</text>
  <text x="60" y="266" fill="#b1bac4" font-family="-apple-system, BlinkMacSystemFont, Segoe UI, sans-serif" font-size="18">Live limit. Local change. One slim pane.</text>
  <text x="60" y="308" fill="#8b949e" font-family="-apple-system, BlinkMacSystemFont, Segoe UI, sans-serif" font-size="15">Claude  ·  Codex  ·  Cursor  ·  Kimi</text>
  <rect x="60" y="346" width="152" height="42" rx="10" fill="#161b22" stroke="#30363d"/>
  <text x="82" y="372" fill="#e6edf3" font-family="ui-monospace, SFMono-Regular, Menlo, monospace" font-size="14">prefix + u</text>
  <text x="60" y="440" fill="#8b949e" font-family="-apple-system, BlinkMacSystemFont, Segoe UI, sans-serif" font-size="13">Limiting capacity · resets · pace trend</text>
  <text x="60" y="469" fill="#8b949e" font-family="-apple-system, BlinkMacSystemFont, Segoe UI, sans-serif" font-size="13">Sanitized local history · no telemetry</text>
  <rect x="{x - 8:.2f}" y="{PANEL_Y - 8}" width="356" height="478" rx="22" fill="#58a6ff" opacity="{glow:.3f}" filter="url(#glow)"/>
  <g transform="translate({x:.2f} {PANEL_Y})" filter="url(#shadow)">
    <svg width="340" height="462" viewBox="0 0 340 462">{dashboard}</svg>
  </g>
  <rect x="0.75" y="0.75" width="958.5" height="538.5" rx="23.25" fill="none" stroke="#30363d" stroke-width="1.5"/>
</svg>'''


def main() -> None:
    require("magick")
    require("ffmpeg")
    dashboard = dashboard_inner()
    with tempfile.TemporaryDirectory(prefix="herdr-quota-demo-") as directory:
        frames = Path(directory)
        for index in range(FRAMES):
            svg = frames / f"frame-{index:03d}.svg"
            png = frames / f"frame-{index:03d}.png"
            svg.write_text(frame_svg(index, dashboard))
            subprocess.run(
                ["magick", str(svg), "-strip", str(png)],
                check=True,
                cwd=ROOT,
                stdout=subprocess.DEVNULL,
            )
        subprocess.run(
            [
                "ffmpeg",
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-framerate",
                str(FPS),
                "-i",
                str(frames / "frame-%03d.png"),
                "-vf",
                "split[s0][s1];[s0]palettegen=max_colors=128:stats_mode=diff[p];[s1][p]paletteuse=dither=bayer:bayer_scale=3:diff_mode=rectangle",
                "-loop",
                "0",
                str(OUTPUT),
            ],
            check=True,
            cwd=ROOT,
        )
    print(f"generated {OUTPUT.relative_to(ROOT)} ({OUTPUT.stat().st_size / 1024:.0f} KiB)")


if __name__ == "__main__":
    main()
