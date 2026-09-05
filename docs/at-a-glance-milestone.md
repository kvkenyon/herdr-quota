# Current quota first

This milestone starts from `99dcc73590ecb0197de99dac656743132cebbc07`.
The shipped Rust renderer now puts current numeric windows before cached account
readings. Stale/error accounts use selectable secondary rows; focusing a repeated
account reveals its safe display label, and Enter retains its last values and
individual resets. No collection, credential, cache, or quota semantics changed.

## Design: two passes

The audience is an engineer checking remaining subscription capacity while working.
The signature is the compact window ledger: window, remaining bar, exact number,
reset. The terminal owns the monospace font. Bold provider/account headings form
the display role; normal window labels and fixed-width numbers form the body and
utility roles. Preview tokens come from the existing Rosé Pine palette: background
`#191724`, evidence `#e0def4`, capacity cyan `#9ccfd8`, warning gold `#f6c177`, and
critical rose `#eb6f92`. Production uses the terminal default foreground and its
ANSI accents, preserving NO_COLOR evidence.

The first pass compared keeping cached meters in the main ledger with a compact
secondary roster. Current quota wins the primary space; cached evidence belongs
in detail. Alignment yields before a reset wraps, so a short window can retain
its bar, number, and countdown on one 20-cell line.

```text
Claude                 Claude
 Research Team          Personal
 cached meters          current windows
 Personal              !Claude #1 · stale
 current windows        [focus: Research Team; Enter: last values]
```

Self-critique found that the old scroll heuristic inferred account boundaries from
text/style and could count secondary rows as part of a selected account. That
could scroll away the provider heading. Explicit account row ranges now keep the
selected group in view. Long windows still wrap at 20 cells; retaining the exact
number and reset is more useful than forcing every window onto one line.

## Evidence and limits

The same synthetic multi-account fixture is replayed by the installed baseline
and candidate renderer: [before, 36 cells](at-a-glance-before.svg),
[after, 36 cells](at-a-glance-wide.svg), and
[after, 20 cells](at-a-glance-narrow.svg). These are exact plain-text Rust renderer
outputs, visually inspected after SVG rasterization. They are not live account
captures. The fixture separates two accounts with overlapping weekly window IDs,
a fractional current value, cached values, multiple resets, and an unknown window.

On 2026-09-05 UTC, guarded session
`fm-lab-hq-at-a-glance-n-37119-16196` was used for installed/candidate lifecycle
verification (the session identifier is recorded in the local lab log).
The installed registry stayed at the baseline commit. Its dashboard manifest
opened successfully with app-only XDG settings/state overrides. The candidate
`target/release/herdr-quota dashboard` then ran in the lab with its worktree
`HERDR_PLUGIN_ROOT`. Both collected and refreshed; candidate Preferences showed
`Identity: logo + name -> names`. Closing restored the one-pane layout, and the
helper teardown verified the default fleet state unchanged. No registry link or
install was performed; firstmate owns final installation after merge. Headless
Herdr pane dimensions do not prove PTY widths: exact 20/24/36-cell tests and direct
PTY tests provide that evidence.

The installed client and server report stable Herdr **0.8.2**, protocol 20. Its
bundled API schema exposes pane graphics, and the installed binary includes the
explicit rejection `pane graphics require experimental.kitty_graphics`.
No safe stable provider-logo surface was established. All identity modes retain
readable names and the explicit Preferences fallback; no glyph logos or graphics
configuration changes were introduced.

The Codex path was traced through bundled quota-axi 0.1.29
`normalizeCodexUsage`, schema-v5 `adapt_provider`, `provider_tiers`,
`provider_section`, and overview/detail rendering. The previously collected null
review container normalizes to no review window. Current main already preserves
that absence correctly. Existing observed-data and real-review counterfactual
regressions remain green; this milestone makes no new availability claim.

## Validation

Focused coverage exercises stale/error/rate-limited/unavailable/auth records beside
current values, fresh records marked stale, missing values, fractional remaining
bars in both meter modes, all three identity modes, account/reset separation,
short frames, and narrow inline resets. A real PTY selects the cached account and
opens its last reading/reset at 20 and 36 cells. Existing identity persistence,
NO_COLOR, terminal restoration, and null-review regressions remain in the gates.

Local gates: `npm run check`, `cargo fmt --all -- --check`,
`cargo check --workspace --all-targets`, `cargo test --workspace`, and
`cargo +nightly clippy --workspace --all-targets -- -D warnings`. The default
stable toolchain lacks Clippy; the already-installed nightly supplies it without
a toolchain change. No no-mistakes pipeline was used.
