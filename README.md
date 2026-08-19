# AI Quota for Herdr

[![CI](https://github.com/kvkenyon/herdr-quota/actions/workflows/ci.yml/badge.svg)](https://github.com/kvkenyon/herdr-quota/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

A slim, full-height quota sidebar for Herdr. It keeps the current tab and split arrangement intact, adds a narrow column on the right, and restores the prior layout when closed.

![AI Quota sidebar showing per-provider quota tiers with remaining percentage, reset countdown, and pace](docs/dashboard-preview.svg)

The sidebar covers Claude, OpenAI Codex, Cursor, and Kimi. Each provider section lists its real quota tiers - for example Claude's session, week, per-model, and extra-usage windows, or Cursor's included, auto, and 3rd-party model buckets - as one aligned row per tier: tier name, remaining percentage, reset countdown, and the pace conclusion that tells you whether that tier lasts through its reset.

## Install and bind `prefix+u`

Requirements: Herdr 0.7.3 or newer, Node.js 22.19 or newer, npm, and at least one supported provider's official app or CLI signed in locally.

1. Install the plugin:

   ```bash
   herdr plugin install kvkenyon/herdr-quota
   ```

2. Add this command binding to `~/.config/herdr/config.toml`:

   ```toml
   [[keys.command]]
   key = "prefix+u"
   type = "plugin_action"
   command = "herdr-quota.open-dashboard"
   description = "AI quota"
   ```

3. Load the new binding into the running Herdr server:

   ```bash
   herdr server reload-config
   ```

Now press your configured prefix (for example `ctrl+b`) followed by `u`. The first press opens and focuses the right sidebar; the next press toggles it closed.

Herdr currently reads custom command bindings only from `config.toml`; plugin manifests cannot install them. The exact old failure was subtle: Herdr's plugin schema has actions and panes but no keybinding collection, while its TOML parser tolerates unknown top-level tables. A manifest `[[keys.command]]` therefore parsed but was silently absent from `herdr plugin list` and never registered an action log when pressed. The plugin now omits that false declaration and never rewrites your configuration during installation.

If the key does not respond, invoke the same action directly to distinguish a binding problem from a plugin problem:

```bash
herdr plugin action invoke herdr-quota.open-dashboard
```

Then run `herdr config check` and repeat `herdr server reload-config` after correcting any reported configuration issue.

To update, run the install command again. To remove the plugin:

```bash
herdr plugin uninstall herdr-quota
```

Remove the `[[keys.command]]` block from `config.toml` and reload configuration when you no longer want the binding.

## Use

| Control      | Action                                         |
| ------------ | ---------------------------------------------- |
| `prefix+u`   | Toggle the quota sidebar                       |
| `r`          | Refresh while keeping the last reading visible |
| `q` / Escape | Close and restore the prior layout             |

The sidebar targets 36 terminal cells on ordinary wide screens and scales down when the tab is narrow: labels compact first, then the pace column steps aside while percentages and resets stay. The normal four-provider tier set fits at ordinary terminal height without scrolling; a shorter pane cuts whole rows and says how many are hidden. Restrained color marks at-risk tiers, but the words carry the meaning without color. Unknown readings stay `--` rather than becoming a misleading zero.

Tier rows follow each provider's own quota model:

- **Claude** - session, week, optional Opus and per-model weekly windows (for example `Fable week`), and extra usage with its spend.
- **OpenAI Codex** - account session/week windows, per-model windows such as `Spark week`, and code-review windows. When no code-review window is returned, an explicit `Code review -- not reported` row says so instead of pretending review shares the base quota.
- **Cursor** - `Included`, `Auto`, and `3rd-party models` (Cursor's "API usage" bucket, which meters third-party model calls, not your own API keys).
- **Kimi** - session, week, and any additional limits Kimi describes, in provider order.

Grok and GitHub Copilot are neither queried nor rendered. Only the four providers above are part of this product.

### When a provider is signed out

A provider that needs authentication shows `signed out` and a single recovery command instead of numbers, so an unavailable reading is never mistaken for zero quota:

| Provider     | Sign in with            |
| ------------ | ----------------------- |
| Claude       | `claude`, then `/login` |
| OpenAI Codex | `codex login`           |
| Cursor       | `cursor-agent login`    |
| Kimi         | `kimi login`            |

The sidebar itself never starts an interactive login and never touches credentials; sign in with the provider's own CLI, then press `r`.

## Data and privacy

The plugin delegates provider access to its plugin-local `quota-axi@~0.1.29` executable and consumes schema version 5 from `quota-axi --json --full`. It does not duplicate provider authentication, inspect browser databases itself, or implement vendor refresh-token flows.

- Credentials remain in stores owned by official provider tools.
- Provider requests go directly from `quota-axi` to first-party endpoints.
- The normalized response remains in sidebar memory and is not persisted.
- Raw responses, account identifiers, credentials, and credential paths are never logged.
- Refreshes have a 12-second process deadline and 2 MiB output limit.
- Child-process and provider errors are bounded and sanitized; one failed provider does not hide the others.

See [Data sources and maintenance](docs/data-sources.md) for the provider boundary and known uncertainties.

### macOS Keychain approval

Claude Code and Cursor may keep live access tokens in the login Keychain. `quota-axi` avoids prompting during an ordinary sidebar refresh. If the sidebar reports that approval is required, run this once in a terminal and choose **Always Allow**:

```bash
npx --yes quota-axi@0.1.29 --allow-keychain-prompt --provider claude,cursor
```

The approval and non-secret access marker belong to `quota-axi`; this plugin never receives or stores the credential.

The Keychain grant only lets `quota-axi` read a credential that already works. Expired or missing credentials still require signing in with the owning CLI (`claude` then `/login`, or `cursor-agent login`); no Keychain approval can refresh them.

## Troubleshooting

**The direct action works but `prefix+u` does nothing.** The plugin is installed, but the binding is missing or has not been reloaded. Add the exact config block above, run `herdr config check`, then `herdr server reload-config`.

**A provider shows `signed out`.** Run the recovery command the sidebar shows for that provider (see the table above). On macOS, Claude or Cursor may also need the one-time Keychain approval above - but only a real sign-in fixes expired credentials.

**A reading is stale or unknown.** The sidebar preserves truthful state when a provider cannot be reached. Press `r` after connectivity or authentication recovers.

**Refresh times out.** The sidebar terminates `quota-axi` after 12 seconds so an unavailable vendor cannot strand the pane. Independent provider results remain visible when available.

**The plugin does not build.** Confirm `node --version` is at least 22.19 and `npm --version` works, then rerun installation. Herdr retains the previous managed install when an update build fails.

## Develop

```bash
npm ci
npm run check
npm run preview
```

`npm run check` formats, lints, type-checks, tests, and regenerates the sanitized preview. The runtime provider boundary lives in `src/schema.ts` and [docs/data-sources.md](docs/data-sources.md); provider credential or endpoint changes belong upstream in `quota-axi`.

## License and references

MIT licensed. See [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) for the open-source Herdr plugins reviewed during design.
