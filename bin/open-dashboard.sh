#!/usr/bin/env bash
set -euo pipefail

HERDR_BIN="${HERDR_BIN_PATH:-herdr}"
exec "$HERDR_BIN" plugin pane open \
  --plugin herdr-quota \
  --entrypoint dashboard \
  --placement overlay \
  --focus
