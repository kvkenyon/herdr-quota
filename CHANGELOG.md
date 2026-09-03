# Changelog

## Unreleased

### Added

- First-run readiness now summarizes trustworthy live providers and explicit sign-in needs, while the overview and Preferences keep every marketed provider visible with finite `live`, aged `stale`, `auth`, `partial`, `quota unavailable`, or `unsupported` copy. No account, credential path, raw source/error, or inferred local estimate enters the UI.
- The README hero and committed 36-cell preview now use one purpose-built sanitized launch fixture that shows provider comparison, the next quota consequence, readiness, and the keyboard path from the first frame.
- Selected-provider detail can show one bounded NO_COLOR-safe history trace with concise material consequence text. Only current, same-reset-cycle allow-listed facts participate; unsafe samples break the trace visibly, and narrow panes elide it before decision or tier text.

### Changed

- The Rust overview/detail dashboard now refreshes in place: age changes to
  `refreshing` or `↻` in the title slot, rapid manual refreshes coalesce to one
  replacement attempt, and finite first-load or last-good failures remain
  actionable without shifting provider rows.
- Settings now use schema v3. Quota history and transition state now use schema v2. The stores migrate the previous versions in memory and write the new version on the next save.
- GitHub Copilot joins Claude, OpenAI Codex, Cursor, and Kimi in the complete provider set. Existing settings add Copilot to the provider order in memory and do not change on disk until the next save.
- Herdr Quota 0.3.x treats the new documents as future schemas and preserves their bytes.

## 0.3.0 — 2026-08-20

### Added

- Quiet opt-in transition cues add only two finite Preferences controls: a remaining-capacity threshold (`off`, `25%`, `10%`, or `5%`) and an independent established forecast-before-reset toggle. Defaults and migrated v0.2.1 settings remain cues-off.
- Fresh visible authoritative limits now produce one deduped downward threshold or forecast-enter event and one later same-cycle recovery. A transient title marker and `a alert` footer open a text-first in-pane review; acknowledgement removes the cue while retaining bounded audit/dedupe state.
- A separately confirmed Preferences action clears transition history without writing or deleting quota history or provider settings. Exact state-machine, gap, persistence, no-color render, input-fragmentation, responsive PTY, acknowledgement, and clear-history regressions cover the complete policy lifecycle.

### Changed

- Settings schema v2 adds only the finite transition policy fields. Schema-v1 documents migrate in memory and are not rewritten until an explicit save. Enabling or changing a policy and hiding/showing a provider establish a baseline, so the current sample, policy changes, and presentation changes cannot synthesize an event.
- The existing serialized pane refresh evaluates transitions while open. Next-open catch-up compares retained trustworthy samples without claiming continuous monitoring; reset identities segment state, and dedupe survives pane reopen and Herdr restart.
- Herdr's general notification command is intentionally not used because delivery can leave the pane and is not plugin-scoped. There is no daemon, watcher, OS notification, sound, badge, toast, telemetry, account, sync, or cloud service.

### Security and privacy

- `${XDG_STATE_HOME:-~/.local/state}/herdr-quota/transitions-v1.json` is a private atomic schema-v1 allow-list bounded to 256 events or 30 days. It stores only marketed provider/scope/limit identity, finite policy, reset-cycle identity, transition/baseline kind, event timestamp, and optional acknowledgement timestamp—never percentages, raw payloads/errors, accounts, credentials, paths, tokens, or arbitrary provider fields.
- Stale, signed-out, auth-required, unavailable, rate-limited, error, unknown-semantics, early-projection, hidden-provider, missing-percentage, and whole-check failure states create no threshold, forecast, or recovery event. Unsupported, malformed, read-only, foreign-owned, symlink, interrupted, and clock-rollback persistence cases fail closed without fabricating or repeating alerts.

## 0.2.1 — 2026-08-20

### Added

- An in-pane, keyboard-only Preferences surface (`p`) can hide supported providers, order visible provider sections, and select remaining or used meters without hand-editing a file. Save, cancel, and confirmed reset remain usable at 20/24/36 columns and 6/8/12/23 rows.
- A small schema-v1 settings document persists only provider order, hidden providers, and meter mode under the XDG config directory. Private atomic writes, malformed-file quarantine, incompatible-schema preservation, non-regular-target refusal, and failure containment protect both settings and live refresh.
- PTY, render, settings, and input regressions cover defaults, each hidden provider, all-hidden recovery, order-independent constraint selection, bounded meter complements, immediate rerender without collector overlap, fragmented Escape sequences, pane restoration, and ANSI/NO_COLOR accessibility.

### Changed

- Default behavior remains v0.2.0-compatible: all four providers appear in the established order, meters mean remaining, the most-restrictive visible limit still wins, and no setup prompt interrupts first run.
- Essential labels, percentages, resets, history, and state text now use the terminal foreground instead of faint or fixed bright 256-color values. Weight, markers, and text carry hierarchy and severity in Herdr light mode, dark mode, and without color.
- The top history line is reserved for actionable change. Exact unchanged samples and underlying differences that round to the same displayed integer no longer render neutral `N→N%` evidence; material drop/gain, reset, pace, projection, gap, privacy, and same-cycle boundaries are unchanged.

### Security and privacy

- Settings never contain account IDs, quota payloads, credentials, authentication facts, raw errors, history samples, or paths from provider output. Unknown fields are ignored, unsafe targets are refused, and a failed replacement preserves the last valid document.

## 0.2.0 — 2026-08-20

### Added

- A bounded schema-v1 local history document records only normalized display-safe quota facts after usable successful collections, with 512-snapshot/30-day retention, 15-minute equivalent-sample cadence, and atomic private-file replacement.
- A compact color-independent change line distinguishes meaningful remaining-capacity drops, authoritative pace deterioration/improvement, reset replenishment, materially earlier/later established exhaustion projections, and neutral same-cycle sparklines.
- Deterministic storage and timeline fixtures cover retention, cadence, privacy allow-listing, atomic interruption, corruption/schema/clock/permission recovery, reset segmentation, first-run insufficiency, provider/auth gaps, and exact 20/24/36-column by 6/8/12/23-row rendering.

### Changed

- Reset-time changes start a new comparison segment, so replenishment is never presented as improved consumption and post-reset sparklines never connect to the prior cycle.
- History steps aside in 6- and 8-row panes; live limiting capacity, current rows, navigation position, and every v0.1.2 input/refresh/layout contract remain dominant and reachable.

### Security and privacy

- History excludes credentials, environment values, account identifiers, paths, raw output, arbitrary errors, tokens, source/plan content, and response bodies. Stale, signed-out, unavailable, error, and unknown-semantics providers contribute no quota facts; entirely unusable reports produce no write.
- Corrupt/truncated data restarts locally, while schema mismatch, permission failure, clock rollback, and interrupted writes degrade to finite safe notes without disturbing live or last-good quota.

## 0.1.2 — 2026-08-20

### Added

- Whole-row `j`/`k`, arrow, Page Down, and Page Up navigation makes every provider and tier reachable in short panes while keeping the title/freshness, limiting-capacity attention, position, and footer pinned.
- A truthful `Rows n–m of total` indicator replaces the hidden-row dead end and clamps after refresh, resize, provider disappearance, and authentication-state changes.
- Top-level timeout, missing-executable, incompatible-output, and generic network/process failures now use finite safe copy with the scheduled automatic retry countdown.

### Changed

- Whole-collector failures retain last-good detail without retaining or displaying arbitrary child stderr; manual `r` still retries immediately and restarts backoff at 10 minutes.
- Responsive coverage now exercises 20/24/36 columns at 6/8/12/23 rows, navigation/fragmented input, dynamic scroll clamping, pinned content, hostile failure output, retry timing, and line-width invariants.

### Security and privacy

- The credential boundary is unchanged. Collector output can no longer become top-level display copy: payloads, paths, credentials, accounts, terminal controls, and arbitrary process output are discarded behind an allow-list.

## 0.1.1 — 2026-08-20

### Added

- A color-independent attention line selects the earliest trustworthy effective constraint across Claude, Codex, Cursor, and Kimi, including established exhaustion timing, spent-tier resets, all-known-on-pace, and incomplete-data states.
- Automatic completion-relative refresh every five minutes, with no collector overlap and bounded 10/20/30-minute backoff after whole-collector failures.
- Deterministic selector, scheduler, current-schema Codex model-window, and no-color responsive coverage at 20/24/36 columns and 12/23/30 rows.

### Changed

- Manual refresh now preempts the active collector and resets automatic scheduling while preserving the last good report.
- Decorative provider gaps are removed before data rows at short heights, keeping the limiting answer visible and hidden-row counts truthful.
- Codex model session/week labels compact semantically; genuinely unknown truncation uses an ellipsis instead of a prompt-like `>`.
- README media shows the live sidebar and attention answer from its first frame.

### Security and privacy

- The credential boundary is unchanged: the plugin consumes only plugin-local `quota-axi --json --full` schema-v5 output and adds no account, daemon, persistence, credential parsing, vendor endpoint, or telemetry.
