import {
  cloneSettings,
  defaultSettings,
  type DashboardSettings,
  type SupportedProvider,
} from "./settings.js";
import type { PreferenceFocus, PreferencesState } from "./types.js";

export type PreferenceAction =
  | "focus_down"
  | "focus_up"
  | "page_down"
  | "page_up"
  | "toggle"
  | "move_up"
  | "move_down"
  | "previous"
  | "next"
  | "activate"
  | "save"
  | "cancel"
  | "reset"
  | "confirm"
  | "decline";

export type PreferenceCommand = "none" | "save" | "cancel";

export interface PreferenceUpdate {
  state: PreferencesState;
  command: PreferenceCommand;
}

export function preferenceFocusOrder(
  settings: DashboardSettings,
): PreferenceFocus[] {
  return [...settings.providerOrder, "meter", "save", "cancel", "reset"];
}

export function openPreferences(settings: DashboardSettings): PreferencesState {
  return {
    draft: cloneSettings(settings),
    focus: settings.providerOrder[0] ?? "meter",
    confirmReset: false,
    saving: false,
  };
}

export function settingsEqual(
  left: DashboardSettings,
  right: DashboardSettings,
): boolean {
  return (
    left.meterMode === right.meterMode &&
    left.providerOrder.join("\0") === right.providerOrder.join("\0") &&
    left.hiddenProviders.join("\0") === right.hiddenProviders.join("\0")
  );
}

function isProviderFocus(value: PreferenceFocus): value is SupportedProvider {
  return (
    value === "claude" ||
    value === "codex" ||
    value === "cursor" ||
    value === "kimi"
  );
}

function moveFocus(state: PreferencesState, amount: number): PreferencesState {
  const order = preferenceFocusOrder(state.draft);
  const current = Math.max(0, order.indexOf(state.focus));
  const next = Math.max(0, Math.min(order.length - 1, current + amount));
  return { ...state, focus: order[next] ?? state.focus, notice: undefined };
}

function toggleProvider(
  settings: DashboardSettings,
  provider: SupportedProvider,
): DashboardSettings {
  const hidden = settings.hiddenProviders.includes(provider)
    ? settings.hiddenProviders.filter((item) => item !== provider)
    : [...settings.hiddenProviders, provider];
  return { ...settings, hiddenProviders: hidden };
}

function toggleFocused(state: PreferencesState): PreferencesState {
  if (isProviderFocus(state.focus)) {
    return {
      ...state,
      draft: toggleProvider(state.draft, state.focus),
      notice: undefined,
    };
  }
  if (state.focus === "meter") {
    return {
      ...state,
      draft: {
        ...state.draft,
        meterMode: state.draft.meterMode === "remaining" ? "used" : "remaining",
      },
      notice: undefined,
    };
  }
  return state;
}

function moveVisibleProvider(
  state: PreferencesState,
  direction: -1 | 1,
): PreferencesState {
  if (
    !isProviderFocus(state.focus) ||
    state.draft.hiddenProviders.includes(state.focus)
  )
    return state;

  const visible = state.draft.providerOrder.filter(
    (provider) => !state.draft.hiddenProviders.includes(provider),
  );
  const visibleIndex = visible.indexOf(state.focus);
  const neighbor = visible[visibleIndex + direction];
  if (!neighbor) return state;

  const order = [...state.draft.providerOrder];
  const currentIndex = order.indexOf(state.focus);
  const neighborIndex = order.indexOf(neighbor);
  order[currentIndex] = neighbor;
  order[neighborIndex] = state.focus;
  return {
    ...state,
    draft: { ...state.draft, providerOrder: order },
    notice: undefined,
  };
}

function resetDraft(state: PreferencesState): PreferencesState {
  return {
    ...state,
    draft: defaultSettings(),
    focus: "reset",
    confirmReset: false,
    notice: undefined,
  };
}

export function applyPreferenceAction(
  current: PreferencesState,
  action: PreferenceAction,
  pageSize = 4,
): PreferenceUpdate {
  if (current.saving) return { state: current, command: "none" };

  if (current.confirmReset) {
    if (action === "confirm")
      return { state: resetDraft(current), command: "none" };
    if (action === "decline" || action === "cancel") {
      return {
        state: { ...current, confirmReset: false },
        command: "none",
      };
    }
    return { state: current, command: "none" };
  }

  switch (action) {
    case "focus_down":
      return { state: moveFocus(current, 1), command: "none" };
    case "focus_up":
      return { state: moveFocus(current, -1), command: "none" };
    case "page_down":
      return {
        state: moveFocus(current, Math.max(1, pageSize)),
        command: "none",
      };
    case "page_up":
      return {
        state: moveFocus(current, -Math.max(1, pageSize)),
        command: "none",
      };
    case "toggle":
    case "previous":
    case "next":
      return { state: toggleFocused(current), command: "none" };
    case "move_up":
      return { state: moveVisibleProvider(current, -1), command: "none" };
    case "move_down":
      return { state: moveVisibleProvider(current, 1), command: "none" };
    case "activate":
      if (current.focus === "save") return { state: current, command: "save" };
      if (current.focus === "cancel")
        return { state: current, command: "cancel" };
      if (current.focus === "reset") {
        return {
          state: { ...current, confirmReset: true },
          command: "none",
        };
      }
      return { state: toggleFocused(current), command: "none" };
    case "save":
      return { state: current, command: "save" };
    case "cancel":
      return { state: current, command: "cancel" };
    case "reset":
      return {
        state: { ...current, focus: "reset", confirmReset: true },
        command: "none",
      };
    case "confirm":
    case "decline":
      return { state: current, command: "none" };
  }
}
