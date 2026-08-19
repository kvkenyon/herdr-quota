import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const manifest = await readFile(
  new URL("../herdr-plugin.toml", import.meta.url),
  "utf8",
);
const readme = await readFile(new URL("../README.md", import.meta.url), "utf8");

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
