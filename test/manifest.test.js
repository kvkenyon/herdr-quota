import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const manifest = await readFile(
  new URL("../herdr-plugin.toml", import.meta.url),
  "utf8",
);
const readme = await readFile(new URL("../README.md", import.meta.url), "utf8");
const packageJson = JSON.parse(
  await readFile(new URL("../package.json", import.meta.url), "utf8"),
);

test("manifest exposes a split-pane action without promising an unsupported binding", () => {
  assert.match(manifest, /^id = "herdr-quota"$/m);
  assert.match(manifest, /^min_herdr_version = "0\.7\.3"$/m);
  assert.match(manifest, /^platforms = \["macos", "linux"\]$/m);
  assert.match(manifest, /\[\[panes\]\][\s\S]*?placement = "split"/);
  assert.match(manifest, /\[\[actions\]\][\s\S]*?id = "open-dashboard"/);
  assert.match(manifest, /command = \["node", "dist\/sidebar\.js"\]/);
  assert.doesNotMatch(manifest, /\[\[keys\.command\]\]/);
  assert.doesNotMatch(manifest, /\[\[(events|startup)\]\]/);
});

test("package and manifest advertise the same v0.4.0 implementation", () => {
  assert.equal(packageJson.version, "0.4.0");
  assert.match(manifest, /^version = "0\.4\.0"$/m);
  assert.match(readme, /five minutes after each completed attempt/);
  assert.match(readme, /10, 20, then at most 30 minutes/);
});

test("first-use docs provide the supported binding and reload path", () => {
  assert.match(readme, /\[\[keys\.command\]\][\s\S]*?key = "prefix\+u"/);
  assert.match(readme, /command = "herdr-quota\.open-dashboard"/);
  assert.match(readme, /herdr server reload-config/);
  assert.match(
    readme,
    /herdr plugin action invoke herdr-quota\.open-dashboard/,
  );
  assert.match(readme, /plugin manifests cannot install them/);
});

test("user docs name Preferences, exact navigation, and safe failure states", () => {
  assert.match(readme, /j\/k · PgUp\/PgDn · p prefs · r · q\/esc/);
  assert.match(readme, /settings\.json/);
  assert.match(readme, /remaining.*used/s);
  assert.match(readme, /0o600|0600/);
  assert.match(readme, /Rows 5–8 of 16/);
  for (const label of [
    "Quota check timed out",
    "quota-axi missing",
    "Incompatible output",
    "Network/process failed",
  ])
    assert.ok(readme.includes(label), label);
  assert.match(readme, /512 snapshots or 30 days/);
  assert.match(readme, /15 minutes/);
  assert.match(readme, /history-v1\.json/);
  assert.match(readme, /transitions-v1\.json/);
  assert.match(readme, /25%.*10%.*5%/s);
  assert.match(readme, /forecast before reset/i);
  assert.match(readme, /a alert/);
});
