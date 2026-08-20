# Changelog

## 0.1.2 — unreleased

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
