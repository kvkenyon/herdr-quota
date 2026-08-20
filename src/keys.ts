export type DashboardAction =
  | "quit"
  | "refresh"
  | "scroll_down"
  | "scroll_up"
  | "page_down"
  | "page_up"
  | "none";

const CSI_SEQUENCE = new RegExp(String.raw`^\x1B\[[0-?]*[ -/]*[@-~]`);

function actionForSequence(sequence: string): DashboardAction {
  if (sequence === "\x1b[A") return "scroll_up";
  if (sequence === "\x1b[B") return "scroll_down";
  if (sequence === "\x1b[5~") return "page_up";
  if (sequence === "\x1b[6~") return "page_down";
  return "none";
}

function parseInput(value: string, holdIncomplete: boolean) {
  const actions: DashboardAction[] = [];
  let index = 0;
  while (index < value.length) {
    const key = value[index]!;
    if (key === "\x1b") {
      const rest = value.slice(index);
      if (rest.startsWith("\x1b[")) {
        const sequence = CSI_SEQUENCE.exec(rest)?.[0];
        if (sequence) {
          const action = actionForSequence(sequence);
          if (action !== "none") actions.push(action);
          index += sequence.length;
          continue;
        }
        if (holdIncomplete) return { actions, pending: rest };
      } else if (holdIncomplete && rest.length === 1) {
        return { actions, pending: rest };
      }
      actions.push("quit");
      index += 1;
      continue;
    }
    if (key === "q" || key === "Q" || key === "\x03") actions.push("quit");
    else if (key === "r" || key === "R") actions.push("refresh");
    else if (key === "j" || key === "J") actions.push("scroll_down");
    else if (key === "k" || key === "K") actions.push("scroll_up");
    index += 1;
  }
  return { actions, pending: "" };
}

export function actionsForInput(input: Buffer | string): DashboardAction[] {
  const value = Buffer.isBuffer(input) ? input.toString("utf8") : input;
  const { actions } = parseInput(value, false);
  return actions.length ? actions : ["none"];
}

export function actionForInput(input: Buffer | string): DashboardAction {
  return actionsForInput(input)[0] ?? "none";
}

export class TerminalInputParser {
  private pending = "";

  push(input: Buffer | string): DashboardAction[] {
    const chunk = Buffer.isBuffer(input) ? input.toString("utf8") : input;
    const value = this.pending + chunk;
    const parsed = parseInput(value, true);
    this.pending = parsed.pending;
    return parsed.actions;
  }

  flush(): DashboardAction[] {
    if (!this.pending) return [];
    const value = this.pending;
    this.pending = "";
    return parseInput(value, false).actions;
  }
}
