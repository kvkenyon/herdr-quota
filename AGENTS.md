# Project agent memory

This file is the project's committed home for project-intrinsic agent knowledge: build, test, release, architecture, and sharp-edge notes that should travel with the code.

- Runtime/provider boundary: consume only the plugin-local `quota-axi --json --full` schema-v5 output. Keep credential and endpoint implementation upstream; see `docs/data-sources.md`.
- Local history is the schema-v1 bounded allow-list in `src/history.ts`; never widen it to raw collector/provider fields. Timeline and failure contracts live in `test/history.test.js` and `test/fixtures/history-timelines.json`.
- The full local gate is `npm run check`. It regenerates `docs/dashboard-preview.svg` from sanitized fixtures, so commit intentional preview changes. That preview is plain text with ANSI stripped, so anything the sidebar encodes visually has to survive without colour.
- Herdr lifecycle testing must use a named non-default session. Follow the task environment's lab helper contract; never run server-global lifecycle commands directly.
- `herdr plugin link/install` mutate a plugin registry shared across all sessions, including the live default one. After lab-testing a worktree via `plugin link`, unlink it and reinstall the original GitHub source at its previous commit.
- A headless lab session never resizes a plugin pane's PTY, so the width in `herdr pane read` is not the width users see. Use the lab for lifecycle (open, refresh, close, layout restore, residue) and check layout at an exact width by running `dist/index.js` in a PTY sized by the harness.
- Herdr plugin manifests do not own keybindings. Document actions as explicit `[[keys.command]]` entries in `config.toml`, followed by `herdr server reload-config`.

## Maintaining this file

Keep this file for knowledge useful to almost every future agent session in this project.
Do not repeat what the codebase already shows; point to the authoritative file or command instead.
Prefer rewriting or pruning existing entries over appending new ones.
When updating this file, preserve this bar for all agents and keep entries concise.
