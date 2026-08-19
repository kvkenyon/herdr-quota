export type DashboardAction =
  "quit" | "refresh" | "scroll-up" | "scroll-down" | "none";

export function actionsForInput(input: Buffer | string): DashboardAction[] {
  const value = Buffer.isBuffer(input) ? input.toString("utf8") : input;
  const actions: DashboardAction[] = [];
  for (let index = 0; index < value.length; index += 1) {
    const key = value[index];
    if (
      key === "\x1b" &&
      value[index + 1] === "[" &&
      (value[index + 2] === "A" || value[index + 2] === "B")
    ) {
      actions.push(value[index + 2] === "A" ? "scroll-up" : "scroll-down");
      index += 2;
    } else if (key === "q" || key === "Q" || key === "\x1b" || key === "\x03") {
      actions.push("quit");
    } else if (key === "r" || key === "R") {
      actions.push("refresh");
    } else if (key === "k") {
      actions.push("scroll-up");
    } else if (key === "j") {
      actions.push("scroll-down");
    }
  }
  return actions.length ? actions : ["none"];
}

export function actionForInput(input: Buffer | string): DashboardAction {
  return actionsForInput(input)[0] ?? "none";
}
