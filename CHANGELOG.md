# Changelog

## 0.1.1 — unreleased

### Added

- A color-independent attention line selects the earliest trustworthy effective constraint across Claude, Codex, Cursor, and Kimi, including established exhaustion timing, spent-tier resets, low remaining capacity, all-known-on-pace, and incomplete-data states.
- Automatic completion-relative refresh every five minutes, with no collector overlap and bounded 10/20/30-minute backoff after whole-collector failures.
- Deterministic selector, scheduler, current-schema Codex model-window, and no-color responsive coverage at 20/24/36 columns and 12/23/30 rows.

### Changed

- Manual refresh now preempts the active collector and resets automatic scheduling while preserving the last good report.
- Decorative provider gaps are removed before data rows at short heights, keeping the limiting answer visible and hidden-row counts truthful.
- Codex model session/week labels compact semantically; genuinely unknown truncation uses an ellipsis instead of a prompt-like `>`.
- README media shows the live sidebar and attention answer from its first frame.

### Security and privacy

- The credential boundary is unchanged: the plugin consumes only plugin-local `quota-axi --json --full` schema-v5 output and adds no account, daemon, persistence, credential parsing, vendor endpoint, or telemetry.
