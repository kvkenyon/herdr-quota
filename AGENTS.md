# Project agent memory

This file is the project's committed home for project-intrinsic agent knowledge: build, test, release, architecture, and sharp-edge notes that should travel with the code.

- Runtime/provider boundary: consume only the plugin-local `quota-axi --json --full` schema-v5 output. Keep credential and endpoint implementation upstream; see `docs/data-sources.md`.
- Rust refresh lifecycle and display-only age ticks are isolated in `rust/herdr-quota/src/scheduler.rs`; the collector-facing `RefreshWorker` fence and completion-relative timing are covered by paused-time `cargo test --workspace` tests. Product-owned live state reduces through `rust/herdr-quota/src/app_state.rs`; pane lifetime, input, Preferences, and terminal restoration are owned by `rust/herdr-quota/src/ui/{runtime,keys,preferences,terminal}.rs`. Sidebar layout/API orchestration and its private schema-v1 coordination boundary live in `rust/herdr-quota/src/sidebar/` and `rust/herdr-quota/src/store/sidebar_state.rs`.
- Local history is the schema-v2 bounded allow-list in `src/history.ts`; the Rust port is split between `rust/herdr-quota/src/domain/history_evidence.rs` and `rust/herdr-quota/src/store/history.rs`. Never widen either implementation to raw collector/provider fields. Timeline and failure contracts live in `test/history.test.js` and `test/fixtures/history-timelines.json`.
- Preferences persist only the finite schema in `src/settings.ts`. Visibility/order/meter mode and opt-in transition policy are local inputs: do not let them alter collection, retry, upstream quota semantics, or quota-history storage.
- Transition audit/dedupe is the bounded schema-v2 allow-list in `src/transitions.ts` and `rust/herdr-quota/src/store/transitions.rs`; their contracts live in `test/transitions*.test.js`, the PTY suite, and `rust/herdr-quota/tests/transitions_*.rs`. Never route cues through Herdr's general notification delivery, which can leave the pane.
- Essential TUI evidence uses the terminal default foreground; severity and selection must remain explicit through weight, markers, and text. ANSI/NO_COLOR contrast contracts live in `test/render.test.js` and `test/personalization-render.test.js`.
- The full local gate is `npm run check`. It regenerates `docs/dashboard-preview.svg` from sanitized fixtures, so commit intentional preview changes. That preview is plain text with ANSI stripped, so anything the sidebar encodes visually has to survive without colour.
- Herdr lifecycle testing must use a named non-default session. Follow the task environment's lab helper contract; never run server-global lifecycle commands directly.
- `herdr plugin link/install` mutate a plugin registry shared across all sessions, including the live default one. After lab-testing a worktree via `plugin link`, unlink it and reinstall the original GitHub source at its previous commit.
- A headless lab session never resizes a plugin pane's PTY, so the width in `herdr pane read` is not the width users see. Use the lab for lifecycle (open, refresh, close, layout restore, residue) and check layout at an exact width by running `dist/index.js` in a PTY sized by the harness.
- Herdr plugin manifests do not own keybindings. Document actions as explicit `[[keys.command]]` entries in `config.toml`, followed by `herdr server reload-config`.
- Settings migration uses one active runtime. Rust-only storage locks do not coordinate with the TypeScript writer; add shared coordination at cutover before both runtimes can write concurrently.

## Maintaining this file

Keep this file for knowledge useful to almost every future agent session in this project.
Do not repeat what the codebase already shows; point to the authoritative file or command instead.
Prefer rewriting or pruning existing entries over appending new ones.
When updating this file, preserve this bar for all agents and keep entries concise.
