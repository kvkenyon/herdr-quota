const FULL = "█";
/** Left-aligned partial cells, so a bar grows smoothly instead of in jumps. */
const EIGHTHS = ["", "▏", "▎", "▍", "▌", "▋", "▊", "▉"];

/**
 * The unfilled part of the gauge, drawn as a rule rather than a shaded block.
 * It keeps a spent tier legible as a bare line instead of a slab of texture.
 */
export const BAR_TRACK = "─";

/**
 * A gauge of the allowance still left in a tier, drawn at one eighth of a cell
 * of precision so even a four-cell bar separates 74% from 69%.
 *
 * The filled part always means remaining, never spent, so the bar and the
 * percentage beside it never disagree. Two clamps keep the extremes truthful:
 * a solid bar means exactly full and an empty track means exactly empty, so
 * 99% keeps a visible notch and 1% keeps a visible sliver. An unknown reading
 * draws nothing, which is why it can never be misread as exhausted.
 */
export function remainingBar(
  percentRemaining: number | undefined,
  width: number,
): string {
  if (width <= 0) return "";
  if (percentRemaining === undefined || !Number.isFinite(percentRemaining))
    return " ".repeat(width);

  const capacity = width * 8;
  const percent = Math.min(100, Math.max(0, percentRemaining));
  let eighths = Math.floor((percent / 100) * capacity);
  if (percent >= 100) eighths = capacity;
  else if (percent <= 0) eighths = 0;
  else eighths = Math.min(Math.max(eighths, 1), capacity - 1);

  const solid = Math.floor(eighths / 8);
  const partial = EIGHTHS[eighths % 8] ?? "";
  return (
    FULL.repeat(solid) +
    partial +
    BAR_TRACK.repeat(width - solid - (partial ? 1 : 0))
  );
}
