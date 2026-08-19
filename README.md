# AI Quota for Herdr

[![CI](https://github.com/kvkenyon/herdr-quota/actions/workflows/ci.yml/badge.svg)](https://github.com/kvkenyon/herdr-quota/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

A focused, session-modal dashboard for AI subscription quota. Press `prefix+u` from any Herdr view to inspect Claude, OpenAI Codex, Cursor, Kimi, and any future provider exposed by [`quota-axi`](https://github.com/kunchenguid/quota-axi), without changing the current tab or split layout.

![AI Quota dashboard showing provider headroom, resets, pace, and bounding windows](docs/dashboard-preview.svg)

The primary line on every card answers how much remains, whether the current pace lasts, which window limits the account, and when it resets. Text labels accompany every color and ASCII-cell mark. Unknown readings display `--`, never a misleading zero.

## Install

Requirements:

- Herdr 0.7.3 or newer on macOS or Linux
- Node.js 22.19 or newer and npm
- At least one supported provider's official app or CLI signed in locally

```bash
herdr plugin install kvkenyon/herdr-quota
```

Herdr runs `npm ci` and `npm run build` from the pinned lockfile. This installs `quota-axi` inside the plugin and compiles the small TypeScript TUI; it does not require a global `quota-axi` installation.

To update, run the same install command again. Herdr replaces the managed checkout only after its build succeeds:

```bash
herdr plugin install kvkenyon/herdr-quota
```

To remove it:

```bash
herdr plugin uninstall herdr-quota
```

The first public release can also be installed directly with the repository command above. After the reviewed release is tagged for marketplace discovery, search the Herdr marketplace for **AI Quota** or the `quota` category; marketplace discovery does not change the install source.

## Use

| Control           | Action                                        |
| ----------------- | --------------------------------------------- |
| `prefix+u`        | Open the dashboard as a focused overlay       |
| `r`               | Refresh now, keeping the last reading visible |
| `j` / `k`, arrows | Scroll in a small popup                       |
| `q` / Escape      | Close cleanly and restore the previous view   |

You can also invoke the action explicitly:

```bash
herdr plugin action invoke herdr-quota.open-dashboard
```

The overlay adapts to narrow and wide terminals. Wide views use two aligned columns; narrow views use one full-width card. A bounded refresh runs on open and when `r` is pressed. The header reports refresh activity and reading age without blanking existing data.

## What it shows

For every provider, the dashboard preserves all useful normalized evidence when available:

- effective remaining headroom and the limiting window;
- reset countdown, pace through reset, projected exhaustion/runway, and confidence;
- every bounding session, weekly, monthly, model, or credit window;
- plan, data source, credits, and spend limit;
- explicit healthy, stale, authentication-required, rate-limited, unavailable, and error states.

Provider failures are isolated. For example, an expired Cursor login does not hide fresh Claude and Codex readings. Future provider IDs automatically get a generic ASCII-cell mark and their text label.

## Data and privacy

This plugin delegates provider access to the plugin-local `quota-axi@~0.1.29` executable and consumes its public JSON schema version 5. It does not duplicate provider authentication, inspect browser databases itself, or implement vendor refresh-token flows.

- Credentials remain in stores owned by official provider tools.
- Provider network requests are made by `quota-axi` directly to first-party endpoints.
- The JSON response lives only in dashboard memory; it is not written to disk.
- Raw authenticated responses, account identifiers, and credentials are never logged.
- Refreshes have a 12-second process deadline and 2 MiB output limit.
- Child-process and provider errors are reduced to bounded, sanitized messages.

See [Data sources and maintenance](docs/data-sources.md) for the inspected source selection, endpoints, local artifacts, and known uncertainties.

### macOS Keychain approval

Claude Code and the Cursor CLI may keep their live access token in the login Keychain. `quota-axi` deliberately avoids prompting during ordinary dashboard refreshes. If a card asks for Keychain approval, run this once in a terminal and choose **Always Allow**:

```bash
npx --yes quota-axi@0.1.29 --allow-keychain-prompt --provider claude,cursor
```

The approval and non-secret access marker are managed by `quota-axi`; this plugin never receives or stores the credential.

## Troubleshooting

**A provider says authentication required.** Open that provider's official CLI and sign in again. For Claude or Cursor on macOS, see the Keychain step above.

**A reading is stale.** The provider was unreachable but `quota-axi` had a last-known safe snapshot. The card keeps its capture age visible. Press `r` when connectivity returns.

**A provider is absent.** `quota-axi` may not have found an installed credential surface for it. Check its credential report with `npx --yes quota-axi@0.1.29 auth`; this prints source status, not secret values.

**Refresh times out.** The popup stops `quota-axi` after 12 seconds so an unavailable vendor cannot strand the pane. Other providers remain visible when `quota-axi` returns their independent failure records before that boundary.

**The plugin does not build.** Confirm `node --version` is at least 22.19 and `npm --version` works, then rerun the install command. Herdr retains the previous working install when a managed update build fails.

## Develop

```bash
npm ci
npm run check
npm run preview
```

`npm run check` runs formatting, lint, strict type checking, all sanitized-fixture tests, and regenerates the deterministic README preview. Tests cover complete and partial data, stale/unknown readings, multiple windows, narrow rendering, future providers, schema adaptation, timeout behavior, controls, and error sanitization.

The plugin has one runtime entrypoint, `dist/index.js`, produced from `src/index.ts`. Provider contract changes should start in `src/schema.ts` and [docs/data-sources.md](docs/data-sources.md), never by copying vendor auth code into this repository.

## License and references

MIT licensed. See [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) for the MIT-licensed Herdr plugins reviewed during design. No source code was copied from those projects.
