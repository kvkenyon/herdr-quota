import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const manifest = await readFile(
  new URL("../herdr-plugin.toml", import.meta.url),
  "utf8",
);
const opener = await readFile(
  new URL("../bin/open-dashboard.sh", import.meta.url),
  "utf8",
);

test("manifest declares the overlay pane, action, platforms, and prefix+u binding", () => {
  assert.match(manifest, /^id = "herdr-quota"$/m);
  assert.match(manifest, /^min_herdr_version = "0\.7\.3"$/m);
  assert.match(manifest, /^platforms = \["macos", "linux"\]$/m);
  assert.match(manifest, /\[\[panes\]\][\s\S]*?placement = "overlay"/);
  assert.match(manifest, /\[\[actions\]\][\s\S]*?id = "open-dashboard"/);
  assert.match(
    manifest,
    /\[\[keys\.command\]\][\s\S]*?key = "prefix\+u"[\s\S]*?command = "herdr-quota\.open-dashboard"/,
  );
  assert.doesNotMatch(manifest, /\[\[(events|startup)\]\]/);
  assert.match(opener, /--placement overlay/);
});
