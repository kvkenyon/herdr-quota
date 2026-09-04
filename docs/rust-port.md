# Rust port checklist

This file records the Rust cutover scope. It is not executable comparison
tooling. Keep TypeScript tests for the live TypeScript product. Do not use
them as a Rust oracle.

## Required verification rule

Do not build a TypeScript parity oracle, golden-output or snapshot harness, or
side-by-side or differential verification layer. Ship one feature at a time
with ordinary Rust unit tests. Once a feature is reachable through the Rust
dashboard, also run the applicable manual dashboard smoke checks.

## Persistence preparation

Rust program PR 3 reserved closed schema versions before provider work. Quota
history and transition state use v2. Rust settings use v4 after PR 20 added the
finite `overview` or `details` startup view; schemas v1-v3 default it to
`overview` in memory. The six provider IDs do not change. Stores do not write
during a load; the next Rust settings save writes schema v4.

Herdr Quota 0.3.x sees settings v4 as a future version and preserves its bytes.
History and transition state keep their schema-v2 paths through the cutover.

Each later Rust ship brief must include this rule in substance. Once its
feature is reachable through the Rust dashboard, it must also state the
applicable manual smoke evidence.

## Feature inventory owners

“Owner” is the planned Rust PR that retires the item. A slash means that the
item has separate parts. The later PR is the completion owner.

| Item                                            | Planned Rust owner PR | Cutover record                                     |
| ----------------------------------------------- | --------------------- | -------------------------------------------------- |
| FI-01 Terminal lifetime, open refresh, and exit | 8 / 9 / **15**        | Collector, scheduler, then terminal loop and guard |
| FI-02 Schema-v5 input and collector boundary    | 7 / **8**             | Adaptation and sanitation, then bounded collection |
| FI-03 Title and decision line                   | **13**                | View model                                         |
| FI-04 Attention decision                        | **13**                | View model                                         |
| FI-05 Provider tiers                            | **13**                | Tier mapping and view model                        |
| FI-06 Remaining/used gauges                     | **13**                | Gauge and view model                               |
| FI-07 Change evidence                           | **11**                | Bounded history and evidence                       |
| FI-08 Provider failure states                   | **13**                | Tier mapping and view model                        |
| FI-09 Whole-check failure state                 | **8**                 | Bounded collector                                  |
| FI-10 Scrolling and pinned rows                 | **14**                | Ratatui renderer                                   |
| FI-11 All-hidden recovery state                 | **14**                | Ratatui renderer                                   |
| FI-12 Preferences                               | **15**                | Terminal loop and Preferences                      |
| FI-13 Transition cues and review                | **12**                | Transition state and reducer                       |
| FI-14 Sidebar layout and restore                | **16**                | Sidebar action and state                           |
| FI-15 Safe deterministic text                   | 7 / **14**            | Input sanitation, then renderer                    |
| FI-16 Semantic display order                    | **14**                | Ratatui renderer                                   |
| PF-01 Preferences file                          | **10**                | Safe atomic settings store                         |
| PF-02 Quota-history file                        | **11**                | Bounded history store                              |
| PF-03 Transition-audit file                     | **12**                | Transition store                                   |
| PF-04 Sidebar coordination file                 | **16**                | Versioned sidebar state                            |

The post-PR 23 marketed-roster and readiness/provenance contract supersedes the
earlier FI-10/FI-11 hidden-provider summary. Its authoritative boundary is
documented in [data-sources.md](data-sources.md#readiness-and-provenance-boundary).

The v0.4.0 manifest cutover retires all listed items in the released product.
Installation keeps `npm ci` only for the plugin-local `quota-axi`, builds the
locked Rust release binary, and routes both production entrypoints to it. The
TypeScript implementation remains development history, not a production
settings writer. Rust alone migrates schemas v1-v3 in memory and writes schema
v4 on the next explicit save.

## Scope limits

- Keep the plugin-local `quota-axi --json --full` schema-v5 boundary.
- Keep persisted data in its stated finite allow-lists.
- Keep both production entrypoints on the Rust binary after the v0.4.0 cutover.
- Do not add a fixture generator, a golden output, a snapshot comparison, or a
  differential test layer to this program.

See [rust-dashboard-smoke.md](rust-dashboard-smoke.md) for the required manual
smoke record.
