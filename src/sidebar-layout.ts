export interface PaneRect {
  paneId: string;
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface MoveStep {
  pane: string;
  direction: "right" | "down";
  target: string;
  ratio: number;
}

export interface RebuildPlan {
  anchor: string;
  steps: MoveStep[];
}

interface Axis {
  direction: MoveStep["direction"];
  start: (rect: PaneRect) => number;
  end: (rect: PaneRect) => number;
}

const AXES: Axis[] = [
  {
    direction: "right",
    start: (rect) => rect.x,
    end: (rect) => rect.x + rect.width,
  },
  {
    direction: "down",
    start: (rect) => rect.y,
    end: (rect) => rect.y + rect.height,
  },
];

function regionBounds(rects: PaneRect[], axis: Axis): [number, number] {
  return [Math.min(...rects.map(axis.start)), Math.max(...rects.map(axis.end))];
}

function cleanCuts(rects: PaneRect[], axis: Axis): number[] {
  const [start, end] = regionBounds(rects, axis);
  return [...new Set(rects.map(axis.end))]
    .filter((cut) => cut > start && cut < end)
    .filter((cut) => {
      const before = rects.filter((rect) => axis.end(rect) <= cut);
      const after = rects.filter((rect) => axis.end(rect) > cut);
      if (!before.length || !after.length) return false;
      return (
        Math.max(...before.map(axis.end)) <= Math.min(...after.map(axis.start))
      );
    })
    .sort((left, right) => left - right);
}

function partition(rects: PaneRect[]): RebuildPlan {
  if (rects.length === 1) return { anchor: rects[0]!.paneId, steps: [] };

  for (const axis of AXES) {
    const cut = cleanCuts(rects, axis)[0];
    if (cut === undefined) continue;
    const [start, end] = regionBounds(rects, axis);
    const first = rects.filter((rect) => axis.end(rect) <= cut);
    const second = rects.filter((rect) => axis.end(rect) > cut);
    const left = partition(first);
    const right = partition(second);
    return {
      anchor: left.anchor,
      steps: [
        {
          pane: right.anchor,
          direction: axis.direction,
          target: left.anchor,
          ratio: (cut - start) / (end - start),
        },
        ...left.steps,
        ...right.steps,
      ],
    };
  }

  throw new Error("pane layout cannot be rebuilt safely");
}

export function planRebuild(rects: PaneRect[]): RebuildPlan {
  if (!rects.length) throw new Error("pane layout is empty");
  return partition(rects);
}

export function targetSidebarWidth(totalWidth: number): number {
  const total = Math.max(1, Math.floor(totalWidth));
  if (total >= 60) return 36;
  if (total >= 44) return total - 24;
  return Math.max(16, Math.floor(total / 2));
}

export function resizeForTarget(
  totalWidth: number,
  currentWidth: number,
  targetWidth = targetSidebarWidth(totalWidth),
): { direction: "left" | "right"; amount: number } | undefined {
  const difference = currentWidth - targetWidth;
  if (Math.abs(difference) < 1 || totalWidth <= 0) return undefined;
  return {
    direction: difference > 0 ? "right" : "left",
    amount: Math.abs(difference) / totalWidth,
  };
}
