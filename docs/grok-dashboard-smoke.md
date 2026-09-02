# Grok dashboard smoke record

Date: 2026-09-02. Platform: macOS arm64. Runtime: TypeScript dashboard in a
real POSIX PTY. Input: a sanitized schema-v5 Grok consumer-quota fixture. It
contains no account or credential data.

| PTY size | Steps                     | Result                                                                                                                                               |
| -------- | ------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------- |
| 36 × 23  | Open, refresh, move, quit | The `Consumer` compact tier label, percentage, reset, and pace text were readable. Refresh and quit succeeded. No terminal control text was visible. |
| 20 × 12  | Open, refresh, move, quit | The compact Grok tier remained readable. Refresh and quit succeeded. No terminal control text was visible.                                           |

The smoke uses one TypeScript dashboard. It does not compare dashboards.
