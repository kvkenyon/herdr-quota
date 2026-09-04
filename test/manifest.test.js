import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { parse } from "smol-toml";

const manifestText = await readFile(
  new URL("../herdr-plugin.toml", import.meta.url),
  "utf8",
);
const manifest = parse(manifestText);
const readme = await readFile(new URL("../README.md", import.meta.url), "utf8");
const packageJson = JSON.parse(
  await readFile(new URL("../package.json", import.meta.url), "utf8"),
);

test("manifest exposes a split-pane action without promising an unsupported binding", () => {
  assert.match(manifestText, /^id = "herdr-quota"$/m);
  assert.match(manifestText, /^min_herdr_version = "0\.7\.3"$/m);
  assert.match(manifestText, /^platforms = \["macos", "linux"\]$/m);
  assert.match(manifestText, /\[\[panes\]\][\s\S]*?placement = "split"/);
  assert.match(manifestText, /\[\[actions\]\][\s\S]*?id = "open-dashboard"/);
  assert.match(manifestText, /command = \["node", "dist\/sidebar\.js"\]/);
  assert.doesNotMatch(manifestText, /\[\[keys\.command\]\]/);
  assert.doesNotMatch(manifestText, /\[\[(events|startup)\]\]/);
});

test("package and manifest advertise the same v0.4.0 implementation", () => {
  assert.equal(packageJson.version, "0.4.0");
  assert.equal(manifest.version, packageJson.version);
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
