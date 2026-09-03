# AI Quota for Herdr

**See the next Claude, OpenAI Codex, Cursor, Kimi, Grok, or GitHub Copilot limit before it stops your work—without leaving Herdr.**

```bash
herdr plugin install kvkenyon/herdr-quota --yes
```

![AI Quota open in Herdr from the first frame, preserving the limiting provider and exhaustion time while a compact title marker and keyboard footer expose one new opt-in transition cue](docs/readme-demo.gif)

[![CI](https://github.com/kvkenyon/herdr-quota/actions/workflows/ci.yml/badge.svg)](https://github.com/kvkenyon/herdr-quota/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/kvkenyon/herdr-quota)](https://github.com/kvkenyon/herdr-quota/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

A slim, full-height sidebar leads with the known provider tier most likely to block work first, then explains actionable established change before making every trustworthy tier reachable with the allowance still remaining, reset countdown, and pace conclusion. Press `p` to keep only the providers you care about, put their sections in your order, show used instead of remaining percentage, or opt into one-shot capacity and forecast transition cues. It refreshes while it is open, keeps the current tab and split arrangement intact, and restores the prior layout when closed. Unknown readings stay honest, and provider access remains read-only through the plugin-local `quota-axi`.

## Bind `prefix+u`

Requirements: Herdr 0.7.3 or newer, Node.js 22.19 or newer, npm, and at least one supported provider's official app or CLI signed in locally.

1. Add this command binding to `~/.config/herdr/config.toml`:

   ```toml
   [[keys.command]]
   key = "prefix+u"
   type = "plugin_action"
   command = "herdr-quota.open-dashboard"
   description = "AI quota"
   ```

2. Load the new binding into the running Herdr server:

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

| Control                | Action                                        |
| ---------------------- | --------------------------------------------- |
| `prefix+u`             | Toggle the quota sidebar                      |
| `j` / `k` or Down / Up | Move the provider/detail window one whole row |
| Page Down / Page Up    | Move one visible page                         |
| `p`                    | Open in-pane Preferences                      |
| `a`                    | Review and acknowledge a new transition cue   |
| `r`                    | Refresh now and reset automatic scheduling    |
| `q` / Escape           | Close and restore the prior layout            |

The sidebar refreshes immediately, then five minutes after each completed attempt without overlapping collectors. A whole-collector failure keeps the last good reading visible and retries after 10, 20, then at most 30 minutes. Pressing `r` preempts the current attempt, refreshes immediately, and resets that backoff.

The sidebar targets 36 terminal cells on ordinary wide screens and scales down when the tab is narrow: labels compact first, then the gauge gives up its cells, then the pace column steps aside while percentages and resets stay. The normal six-provider tier set fits at ordinary terminal height. In shorter panes, `j`/`k` and Page Down/Page Up scroll only whole provider/detail rows. The title and freshness, limiting-capacity attention line, position (`Rows 5–8 of 16`), and keyboard footer stay pinned. Actionable history gets one compact change line when the pane is at least 10 rows tall; at 6 or 8 rows it steps aside entirely for live data. Decorative provider gaps disappear before scrolling is needed and never inflate that row count. The full default footer remains `j/k · PgUp/PgDn · p prefs · r · q/esc`; only a new unacknowledged transition temporarily adds the discoverable `a alert` action and a compact `!` title marker, without adding a content row or replacing the limiting answer.

Essential labels, percentages, resets, history, and state text use the terminal's default foreground so they remain legible in Herdr's light and dark themes. Bold weight is reserved for titles, provider headers, the limiting tier, focus, and severity—not every row. `!`, `=`, `?`, arrows, checkboxes, focus markers, and explicit state words carry all meaning without color; optional color only reinforces them. Unknown readings stay `--` rather than becoming a misleading zero.

The committed validation sheets exercise the real ANSI renderer against Herdr's [light](docs/theme-preview-light.svg) and [dark](docs/theme-preview-dark.svg) Rosé Pine terminal palettes at 20/24/36 columns and normal/short heights; the same matrix also runs in a real PTY and with `NO_COLOR` during `npm run check`.

### Preferences

Press `p` inside the pane. Use `j`/`k` or Up/Down to move focus, Space or Enter to show/hide a provider, `u`/`d` to move a visible provider, and Left/Right or Space to change the meter or transition controls. `Capacity cue` cycles through `off`, `25%`, `10%`, and `5%`; `Forecast cue` independently turns the established forecast before reset policy on or off. Press `s` to save or `c`/Escape to cancel without changing the live dashboard. `x` opens reset confirmation; `y` resets the draft to all providers in the original order with remaining meters and transition cues off, but an explicit save is still required. `Clear transition history` has its own `y` confirmation and never deletes quota history or provider settings. The focused row and every action remain reachable in 20/24/36-column panes, including short heights.

Provider order changes provider sections only. Attention still selects the most restrictive visible evidence using the established safety semantics. Hidden providers disappear from detail, attention, and the change line. The compact hidden count reports unavailable marketed provider conditions; unsupported providers remain filtered at the schema boundary, and a deliberate Preferences choice is not relabeled as hidden. If all six are hidden, the pane says `No providers shown` and gives the exact recovery instruction `Press p for Preferences` (compact `Press p for prefs` at 20 columns); it never claims that everything is on pace.

### Transition cues

Transition cues are off by default. Enabling or changing a policy establishes a baseline from the current trustworthy sample and never alerts from that sample alone. Changing thresholds, hiding/showing/reordering providers, or changing the meter cannot synthesize a crossing. While the pane is open, evaluation runs after the existing non-overlapping refresh; no watcher, daemon, schedule, or background process is added. On the next open, retained trustworthy samples can reveal that a crossing happened between those samples, and the wording intentionally makes no claim of continuous monitoring.

- A visible authoritative effective limit crossing downward from above to at or below `25%`, `10%`, or `5%` produces one capacity event. A later same-cycle fresh recovery above that threshold produces one recovery event.
- With `Forecast cue` enabled, an established runway changing from through-reset to projected exhaustion or exhausted-now produces one forecast event. Returning to through-reset produces one recovery event. Early projections never qualify.
- Stale, signed-out, auth-required, unavailable, rate-limited, error, unknown-semantics, hidden-provider, missing-percentage, and whole-check failure states produce no threshold, forecast, or recovery event. A trustworthy later sample may bridge a same-cycle data gap; a changed reset identity starts a new baseline instead of calling replenishment a recovery.
- Events dedupe across refresh, pane reopen, and Herdr restart. Press `a` when the transient title marker/footer appears to review text such as `Codex Week crossed 25% · 23% left`, `Cursor 3rd-party now exhausts before reset`, or `Codex Week recovered above 25%`; press `a` or Enter again to acknowledge. Acknowledgement removes the cue but retains the bounded sanitized event record.

Herdr's general notification command is intentionally not used: its configured delivery may leave the pane or reach an OS notification surface, and it is not isolated as plugin-owned attention. Transition cues stay inside this pane with no sound, badge, toast, Notification Center item, email, webhook, telemetry, account, sync, or cloud service.

### Reading the attention line

The first content line is a decision summary built only from `quota-axi`'s schema-v5 effective availability, pace, and runway—not a locally inferred cap:

| Marker | Meaning                                                                |
| ------ | ---------------------------------------------------------------------- |
| `!`    | The earliest established exhaustion, or a tier already spent.          |
| `=`    | Every current limit with known pace is expected to last through reset. |
| `?`    | Some provider or pace data is not current enough for a safe answer.    |

An established projection shows when capacity runs out; an early projection remains `ahead` on its tier row without pretending its time is precise. A spent tier shows its reset when known. Signed-out, stale, unavailable, and error providers never contribute a precise forecast. When a stronger current constraint exists it remains the summary, while unreadable providers retain their own explicit status below. Unknown limiting IDs fall back to the provider name and are never exposed.

### Reading the change line

The optional second content line compares only consecutive, trustworthy samples from the same reset cycle. It never joins a post-reset series to the prior cycle and never signals from a single sample, signed-out provider, stale/unavailable/error state, unknown semantics, schema mismatch, or whole-check failure.

| Marker    | Established local evidence                                                              |
| --------- | --------------------------------------------------------------------------------------- |
| `↻`       | The reset time changed and remaining capacity materially replenished.                   |
| `↓` / `↑` | Remaining capacity dropped/gained meaningfully, or authoritative pace got worse/better. |
| `↘` / `↗` | An established exhaustion projection moved materially earlier/later.                    |
| `~`       | A finite local-history recovery or availability note.                                   |

Exact unchanged samples, ordinary same-cycle drift, and values that differ underneath but round to the same displayed integer leave this prime line empty. The pane never shows a confusing `N→N%`. Meaningful capacity drops or gains require at least 10 percentage points; pace reserve changes require at least 10 points; exhaustion projections must move by at least two hours. Material same-cycle gain, replenishment after a reset, authoritative pace improvement, and materially later exhaustion remain positive evidence. These thresholds avoid fabricating precision from routine refresh noise while preserving actionable drop, gain, reset, pace, forecast, and gap semantics.

### Reading the gauges

By default each tier draws a small gauge of what is **still remaining**, exactly as v0.2.0 did. In Preferences, `used` changes only this presentation: for a known bounded percentage it shows `100 - remaining`, clamped to 0–100. The title adds `· used` so a screenshot and no-color output cannot be misread. Unknown, unavailable, stale, signed-out, error, and non-percentage evidence never turn into zero or 100, and reset, pace, runway, history, and attention calculations continue using the original upstream data.

| Gauge  | Meaning                                                  |
| ------ | -------------------------------------------------------- |
| `████` | Exactly 100% in the selected meter mode.                 |
| `██──` | Half of the selected remaining/used measure.             |
| `▏───` | Near zero; any positive selected measure keeps a sliver. |
| `────` | Exactly 0% in the selected meter mode.                   |
| blank  | No reading. Nothing is drawn, so empty is never implied. |

Gauges are drawn to an eighth of a cell, so tiers a few points apart stay apart on screen. The filled part and percentage use the same presentation mode, and the tier that limits the provider is shown in bold.

Tier rows follow each provider's own quota model:

- **Claude** - session, week, optional Opus and per-model weekly windows (for example `Fable week`), and extra usage with its spend.
- **OpenAI Codex** - account session/week windows, per-model windows such as `Spark week`, and code-review windows. When no code-review window is returned, an explicit `Code review -- not reported` row says so instead of pretending review shares the base quota.
- **Cursor** - `Included`, `Auto`, and `3rd-party models` (Cursor's "API usage" bucket, which meters third-party model calls, not your own API keys).
- **Kimi** - session, week, and any additional limits Kimi describes, in provider order.
- **Grok** - `Consumer quota` and any product limits Grok reports. A usable Grok CLI or Pi `xai` login can have no consumer quota reading. The pane says `consumer quota unavailable` in this case.
- **GitHub Copilot** - Chat, Completions, Premium, and other reported windows. Copilot does not report trusted window relationships. The pane shows each window and does not make a combined quota claim.

Unknown providers are not queried or rendered. The six providers above are part of this product. Pi has no provider card.

### When a provider is signed out

A provider that needs authentication shows `signed out` and a single recovery command instead of numbers, so an unavailable reading is never mistaken for zero quota:

| Provider       | Sign in with                    |
| -------------- | ------------------------------- |
| Claude         | `claude`, then `/login`         |
| OpenAI Codex   | `codex login`                   |
| Cursor         | `cursor-agent login`            |
| Kimi           | `kimi login`                    |
| Grok           | `grok`                          |
| GitHub Copilot | `github-copilot-cli auth login` |

The sidebar itself never starts an interactive login and never touches credentials; sign in with the provider's own CLI, then press `r`.

### When the whole quota check fails

The last good provider details remain visible. A short, allow-listed failure line shows what class of check failed and the countdown to the scheduled automatic retry; press `r` to retry immediately and restart backoff at 10 minutes.

| Sidebar failure          | What to do                                                                    |
| ------------------------ | ----------------------------------------------------------------------------- |
| `Quota check timed out`  | Check connectivity or vendor availability; press `r` to try again now.        |
| `quota-axi missing`      | Reinstall or update the Herdr plugin so its local dependencies are rebuilt.   |
| `Incompatible output`    | Update the plugin; its pinned schema adapter does not accept this output.     |
| `Network/process failed` | Check connectivity, then press `r`; reinstall if the local process stays bad. |

Provider-level signed-out, stale, rate-limited, unavailable, and error states stay inside their provider sections. They do not become a whole-check failure and do not hide healthy siblings.

## Data and privacy

The plugin delegates provider access to its plugin-local `quota-axi@~0.1.29` executable and consumes schema version 5 from `quota-axi --json --full`. It does not duplicate provider authentication, inspect browser databases itself, or implement vendor refresh-token flows. History is local-only and never changes that credential boundary.

- Credentials remain in stores owned by official provider tools.
- Provider requests go directly from `quota-axi` to first-party endpoints.
- The full normalized response remains in sidebar memory. History persists only its separate schema-v2 allow-list: collection timestamp, marketed provider/scope/limit display identity, effective remaining percentage, reset time, authoritative pace state/reserve, authoritative runway state/projection, and finite data-health/auth eligibility.
- History never stores credentials, environment values, account identifiers, filesystem paths, source or plan content, raw child output, arbitrary errors, tokens, response bodies, or other provider fields.
- The single compact JSON document lives at `${XDG_STATE_HOME:-~/.local/state}/herdr-quota/history-v1.json`, uses private directory/file modes, and is replaced atomically through a sibling temporary file.
- Retention is bounded to 512 snapshots or 30 days, whichever removes data first. Equivalent samples are stored at most once every 15 minutes; real normalized changes may be recorded sooner.
- Only a usable successful collection can write history. Ineligible providers keep finite health/auth gap markers but no quota facts, and an entirely stale, signed-out, unavailable, error, or unknown-semantics report produces no write.
- Corrupt/truncated history restarts safely; schema mismatch, permission failure, clock rollback, and interrupted writes preserve the live dashboard and collapse to a finite local-history note. No local error or file content becomes display copy.
- Preferences persist separately at `${XDG_CONFIG_HOME:-~/.config}/herdr-quota/settings.json`. Its complete schema-v3 content is the schema version, supported-provider order, hidden-provider IDs, `remaining`/`used` meter mode, finite `off`/`25`/`10`/`5` capacity threshold, and forecast-before-reset boolean. Schema-v1 and schema-v2 files migrate in memory. A load does not rewrite the file. The next save writes schema v3.
- Settings never store account IDs, quota payloads, credentials, authentication facts, raw errors, history samples, provider-output paths, or arbitrary future fields. The writer serializes only its finite schema, uses a private `0700` directory and `0600` file, syncs a private sibling temporary file, then atomically replaces the target.
- A malformed current settings file is quarantined with private permissions before defaults recover; an unsupported future schema is preserved in place. Unknown fields and provider IDs are ignored. Symlink/non-regular targets, read-only or unwritable paths, and interrupted replacement fail closed without crashing refresh or replacing the last valid file.
- Saving Preferences rerenders the existing last-good report immediately. It does not launch another collector, reset retry backoff, erase history, alter credentials, or require a Herdr server restart.
- Transition audit/dedupe state is separate at `${XDG_STATE_HOME:-~/.local/state}/herdr-quota/transitions-v1.json`. Its schema is v2. Schema-v1 files migrate in memory. A load does not rewrite the file. The next state write uses schema v2. The file is bounded to 256 events or 30 days and contains only marketed provider/scope/limit identity, finite policy, reset-cycle identity, transition kind, event timestamp, and optional acknowledgement timestamp. It uses the same private `0700` directory, `0600` file, no-symlink, owner, atomic sibling-replacement boundary as settings. It never contains percentages, quota payloads, account IDs, credentials, paths, raw output/errors, tokens, source text, or arbitrary provider fields; event wording recovers the displayed percentage from the separately sanitized quota-history sample when retained.
- Schema-v1 history files migrate in memory. A load does not rewrite the file. The next history write uses schema v2. The `history-v1.json` and `transitions-v1.json` file names do not change.
- Herdr Quota 0.3.x sees settings v3, history v2, and transition state v2 as future schemas. It does not replace or delete these files. The six provider IDs are `claude`, `codex`, `cursor`, `kimi`, `grok`, and `copilot`.
- Clearing transition history is separately confirmed inside Preferences. It neither writes nor deletes quota history or provider settings, and unsupported/read-only settings documents remain untouched. A fresh baseline is established before future evaluation resumes.
- Automatic refresh exists only while the pane process is alive: five minutes after success, with bounded 10/20/30-minute whole-collector failure backoff.
- Raw responses, account identifiers, credentials, and credential paths are never logged.
- Refreshes have a 12-second process deadline and 2 MiB output limit.
- Arbitrary child output is drained but never retained for display. Whole-check failures collapse to timeout, missing executable, incompatible output, or generic network/process state before rendering.
- Provider errors remain bounded and sanitized; one failed provider does not hide the others.

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

**A reading is stale or unknown.** The sidebar preserves truthful state when a provider cannot be reached and keeps retrying while open. Press `r` to retry immediately after connectivity or authentication recovers.

**History restarted or is unavailable.** `History restarted` means a corrupt/truncated file or clock rollback was isolated and a new safe segment began. `History unavailable` means the local schema is newer/incompatible or the file could not be read or atomically replaced. Live quota and refresh continue normally; check permissions on `${XDG_STATE_HOME:-~/.local/state}/herdr-quota` or update the plugin for a newer schema.

**Preferences reset or cannot save.** A recovered malformed file starts with safe defaults; an unsupported future schema remains untouched. `Save failed` means the settings target could not be replaced safely—check that `${XDG_CONFIG_HOME:-~/.config}/herdr-quota/settings.json` is an ordinary file owned by you and its directory is writable. The live quota pane keeps refreshing, and cancel still leaves the active settings unchanged. Never replace the target with a symlink.

**A transition did not appear.** Confirm `Capacity cue` or `Forecast cue` is enabled in Preferences. Enabling/changing a policy, showing a provider, or entering a new reset cycle intentionally establishes a baseline; it cannot alert from the current sample alone. Stale, signed-out, unavailable, error, unknown, hidden, early-projection, and whole-check failure states never become quota transitions.

**A transition cue stays visible.** Press `a`, review the text-first event, then press `a` or Enter to acknowledge it. If acknowledgement or clear reports failure, check that `${XDG_STATE_HOME:-~/.local/state}/herdr-quota/transitions-v1.json` is an ordinary writable file owned by you, not a symlink or unsupported future schema.

**Every provider is hidden.** Press the `p` key named by the empty state, show at least one provider with Space, and press `s`. You can also choose `Reset defaults`, confirm with `y`, then save.

**The position says more rows exist.** Use `j`/`k`, Down/Up, or Page Down/Page Up. The position counts provider headers and tier/recovery rows, not decorative blank lines.

**The whole quota check fails.** `Timed out` and `Network/process failed` usually call for checking connectivity and pressing `r`. `quota-axi missing` calls for reinstalling the plugin. `Incompatible output` calls for updating the plugin. The displayed countdown is the next automatic retry; previous good detail stays available meanwhile.

**Refresh times out.** The sidebar terminates `quota-axi` after 12 seconds so an unavailable vendor cannot strand the pane. Provider-level results from a valid report remain visible; a whole-check timeout keeps the previous good report.

**The plugin does not build.** Confirm `node --version` is at least 22.19 and `npm --version` works, then rerun installation. Herdr retains the previous managed install when an update build fails.

## Develop

```bash
npm ci
npm run check
npm run preview
```

`npm run check` formats, lints, type-checks, runs the selector/refresh/layout/history/settings/transition suites, drives the real app through a POSIX PTY, and regenerates the sanitized no-color preview. Exact responsive input/render fixtures cover 20/24/36 columns and 6/8/12/23 rows, every reachable row, pinned context, fragmented Escape sequences, Preferences save/cancel/reset/order/visibility/meter/policy/clear controls, immediate rerender without collector overlap, transition review/acknowledgement, light/dark ANSI safety, all-hidden recovery, settings failure containment, dynamic scroll clamping, safe whole-check failures, retry timing, current Codex model session/week labels, bounded atomic local files, privacy allow-listing, reset segmentation, auth/data gaps, and deterministic actionable change/transition timelines. The runtime provider boundary lives in `src/schema.ts` and [docs/data-sources.md](docs/data-sources.md); provider credential or endpoint changes belong upstream in `quota-axi`.

## License and references

MIT licensed. See [CHANGELOG.md](CHANGELOG.md) for release notes and [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) for the open-source Herdr plugins reviewed during design.
