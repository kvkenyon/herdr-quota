# Copilot dashboard smoke record

Date: 2026-09-02. Platform: macOS arm64. Runtime: TypeScript dashboard in a
real POSIX PTY. Input: sanitized schema-v5 Copilot fixture with Chat,
Completions, and Premium windows. No account or credential data was used.

| PTY size | Steps                                  | Result                                                                                             |
| -------- | -------------------------------------- | -------------------------------------------------------------------------------------------------- |
| 36 × 23  | Open, refresh, move, quit              | Copilot labels, percentages, resets, and unknown pace stayed readable. Quit restored the terminal. |
| 20 × 12  | Open, move through rows, refresh, quit | Compact labels stayed readable. All rows were reachable. Quit restored the terminal.               |

The smoke uses one dashboard only. It does not compare TypeScript output with
another program.
