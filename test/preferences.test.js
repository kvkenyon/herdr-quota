import assert from "node:assert/strict";
import test from "node:test";
import {
  applyPreferenceAction,
  openPreferences,
  preferenceFocusOrder,
  settingsEqual,
} from "../dist/preferences.js";
import { defaultSettings } from "../dist/settings.js";

function atFocus(preferences, focus) {
  return { ...preferences, focus };
}

test("defaults open as an isolated draft in the supported provider order", () => {
  const settings = defaultSettings();
  const preferences = openPreferences(settings);
  assert.deepEqual(preferenceFocusOrder(preferences.draft), [
    "claude",
    "codex",
    "cursor",
    "kimi",
    "meter",
    "save",
    "cancel",
    "reset",
  ]);
  assert.notEqual(preferences.draft, settings);
  preferences.draft.hiddenProviders.push("claude");
  assert.deepEqual(settings.hiddenProviders, []);
});

test("every supported provider can be hidden and shown independently", () => {
  for (const provider of ["claude", "codex", "cursor", "kimi"]) {
    let preferences = atFocus(openPreferences(defaultSettings()), provider);
    preferences = applyPreferenceAction(preferences, "toggle").state;
    assert.deepEqual(preferences.draft.hiddenProviders, [provider]);
    preferences = applyPreferenceAction(preferences, "toggle").state;
    assert.deepEqual(preferences.draft.hiddenProviders, []);
  }
});

test("u/d reorder visible providers deterministically without moving hidden rows", () => {
  let preferences = openPreferences({
    ...defaultSettings(),
    hiddenProviders: ["codex"],
  });
  preferences = atFocus(preferences, "cursor");
  preferences = applyPreferenceAction(preferences, "move_up").state;
  assert.deepEqual(preferences.draft.providerOrder, [
    "cursor",
    "codex",
    "claude",
    "kimi",
  ]);
  assert.equal(preferences.focus, "cursor");

  preferences = atFocus(preferences, "codex");
  const unchanged = applyPreferenceAction(preferences, "move_down").state;
  assert.deepEqual(
    unchanged.draft.providerOrder,
    preferences.draft.providerOrder,
  );
});

test("meter, save, and cancel controls do not mutate active settings", () => {
  const active = defaultSettings();
  let preferences = atFocus(openPreferences(active), "meter");
  preferences = applyPreferenceAction(preferences, "next").state;
  assert.equal(preferences.draft.meterMode, "used");
  assert.equal(active.meterMode, "remaining");
  assert.equal(settingsEqual(active, preferences.draft), false);
  assert.equal(applyPreferenceAction(preferences, "save").command, "save");
  assert.equal(applyPreferenceAction(preferences, "cancel").command, "cancel");
  assert.deepEqual(active, defaultSettings());
});

test("reset is confirmed, resets only the draft, and still requires save", () => {
  const active = {
    ...defaultSettings(),
    providerOrder: ["kimi", "cursor", "codex", "claude"],
    hiddenProviders: ["claude", "codex"],
    meterMode: "used",
  };
  let preferences = openPreferences(active);
  preferences = applyPreferenceAction(preferences, "reset").state;
  assert.equal(preferences.confirmReset, true);

  preferences = applyPreferenceAction(preferences, "decline").state;
  assert.deepEqual(preferences.draft, active);
  preferences = applyPreferenceAction(preferences, "reset").state;
  preferences = applyPreferenceAction(preferences, "confirm").state;
  assert.deepEqual(preferences.draft, defaultSettings());
  assert.equal(preferences.confirmReset, false);
  assert.deepEqual(active.hiddenProviders, ["claude", "codex"]);
});

test("j/k, arrows, and page movement clamp focus at short-pane boundaries", () => {
  let preferences = openPreferences(defaultSettings());
  preferences = applyPreferenceAction(preferences, "focus_up").state;
  assert.equal(preferences.focus, "claude");
  preferences = applyPreferenceAction(preferences, "page_down", 4).state;
  assert.equal(preferences.focus, "meter");
  preferences = applyPreferenceAction(preferences, "page_down", 4).state;
  assert.equal(preferences.focus, "reset");
  preferences = applyPreferenceAction(preferences, "focus_down").state;
  assert.equal(preferences.focus, "reset");
  preferences = applyPreferenceAction(preferences, "page_up", 4).state;
  assert.equal(preferences.focus, "kimi");
});
