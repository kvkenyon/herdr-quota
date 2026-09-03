# Rust dashboard smoke protocol

Use this manual protocol after a Rust dashboard feature becomes runnable. It
checks the Rust dashboard directly. It does not compare Rust output with the
TypeScript dashboard.

## Record for each run

Record the date, OS and architecture, Rust PR, binary version or commit,
terminal program, terminal size, input data state, steps, and result. Record a
failure with the visible safe text and the reproduction steps. Do not record
tokens, paths, raw collector output, or provider account data.

## Direct dashboard runs

Run the Rust `dashboard` subcommand in a real PTY. Set the PTY to each exact
size before launch:

| Run    | PTY size             | Required checks                                              |
| ------ | -------------------- | ------------------------------------------------------------ |
| Wide   | 36 columns × 12 rows | Open, first render, feature path, navigation, and quit       |
| Narrow | 20 columns × 12 rows | Open, first render, feature path, row reachability, and quit |

For both runs, check these points that apply to the shipped feature:

1. The dashboard opens in a TTY. It renders without terminal control text.
2. The title, decision text, position, and controls stay readable. No row
   wraps or clips in a way that changes its meaning.
3. Use `j`/`k` and Page Up/Page Down when rows can scroll. Check that each row
   is reachable and the pinned context remains visible.
4. Use `r` when collection is available. Check the feature's refresh or
   failure state. Do not expose raw output.
5. Exercise the keys and modal path added or changed by the PR. For PR 15,
   include Preferences, confirmation, fragmented Escape, rapid refresh, and
   signal/quit cleanup.
6. Quit with `q` and check that the alternate screen, cursor, and terminal
   input mode are restored.

Run the direct smoke on native macOS and Linux release artifacts before the
cutover. A feature PR records the applicable direct runs; it does not need a
Herdr lifecycle run.

## Guarded cutover lab run

Only the manifest cutover PR performs this run. Generate its ship brief with
the required `--herdr-lab` guard before any Herdr lifecycle action. Use a named
non-default Herdr lab session.

In that guarded lab, verify this sequence:

1. Open the sidebar from a tab that has an existing layout.
2. Check first refresh and the visible dashboard state.
3. Close the sidebar.
4. Check that the original layout and focus return and that no sidebar state
   residue remains.

The lab proves lifecycle behavior only. It does not prove display width: run
the direct PTY checks at 36 × 12 and 20 × 12 as well. Do not run server-global
Herdr lifecycle commands, and do not add a lifecycle command to an unguarded
brief.

## Handoff statement

Each Rust PR handoff states the two direct-run results, the platform, and the
feature path checked. The cutover handoff also states the guarded named-lab
result. The evidence is a manual completion record, not a comparison harness.

## PR 14 renderer record

Date: 2026-09-02. Platform: macOS arm64 (Darwin 25.1.0). Runtime: Rust
`herdr-quota` at `501a750` plus this renderer worktree, run directly through a
native `script` PTY using the sanitized `complete.json` schema-v5 fixture.

| PTY size | Steps                                                    | Result                                                                                                              |
| -------- | -------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------- |
| 36 × 23  | Opened `dashboard`, moved down twice, then quit with `q` | Title, attention, position, tier rows, and full controls stayed visible; the dashboard exited and restored the PTY. |
| 20 × 12  | Opened `dashboard`, moved down, then quit with `q`       | Compact controls and pinned context stayed readable; the dashboard exited and restored the PTY.                     |

The static fixture has no collector, so refresh is intentionally unavailable.
Semantic-row reachability and cell-width bounds are
covered by ordinary Rust unit tests; this record is a direct dashboard smoke,
not a comparison against the TypeScript runtime.

## PR 15 terminal-loop record

Date: 2026-09-02. Platform: macOS arm64 (Darwin 25.1.0). Runtime: Rust
`herdr-quota` at `8b2c762` plus the PR 15 worktree, run directly in a native
POSIX PTY with `NO_COLOR=1`, temporary schema-v3/v2 local state, and the
plugin-local schema-v5 collector.

| PTY path                  | Steps                                                                                                                                                                       | Result                                                                                                                                                                |
| ------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 36 × 23 → 20 × 12         | Opened `dashboard`; reviewed and acknowledged a bounded transition; exercised fragmented Down, save/cancel/reset/clear confirmations, and rapid `r`; resized; quit with `q` | Both sizes remained interactive; fragmented Escape input stayed in Preferences; the transition was acknowledged; and raw mode, cursor, and alternate screen restored. |
| 20 × 12, SIGINT / SIGTERM | Opened `dashboard`, delivered each signal in a separate direct run, and inspected terminal state and emitted terminal teardown                                              | Both signals produced a clean exit and restored canonical/echo/signal input flags, the cursor, and the primary screen.                                                |

The run used no Herdr lifecycle command. The same paths are covered by the
ordinary Rust POSIX-PTY integration test with a bounded fake collector so rapid
refresh and teardown remain deterministic in the test suite.

## PR 20 overview/detail record

Date: 2026-09-02. Platform: macOS arm64 (Darwin 25.1.0). Runtime: Rust
`herdr-quota` at `8b2c762` plus this overview/detail worktree, run directly in
an exact-size PTY with `NO_COLOR` and the sanitized `complete.json` schema-v5
fixture.

| PTY size | Steps                                                                                                                                                           | Result                                                                                                                                                        |
| -------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 36 × 12  | Opened overview, moved the provider cursor, entered and escaped detail, changed the startup-view preference in an isolated config directory, then quit with `q` | Four provider summaries, decision, evidence, and controls remained visible; detail and Preferences opened locally; the dashboard exited and restored the PTY. |
| 20 × 12  | Opened overview, selected provider 4, entered and escaped detail, opened and cancelled Preferences, then quit with `q`                                          | Bars dropped while markers, providers, percentages/states, compact dates, and controls remained readable; the dashboard exited and restored the PTY.          |

The static fixture has no collector, so refresh is intentionally unavailable.
The provider-number/detail two-key reachability matrix and transition-review
Enter locality are covered by ordinary Rust unit tests. No Herdr lifecycle
behavior was driven.
