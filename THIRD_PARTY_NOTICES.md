# Third-party notices

The following MIT-licensed projects were reviewed as design references on 2026-08-18:

- [`senna-lang/herdr-agent-usage`](https://github.com/senna-lang/herdr-agent-usage) at commit [`0eb3abc`](https://github.com/senna-lang/herdr-agent-usage/tree/0eb3abc607ff7486891a4d1e028a0709b0d278f8). It demonstrates Herdr plugin pane/action packaging and account-limit presentation for Claude, Codex, OpenCode Go, and Grok. Its current provider coverage does not supply Cursor or Kimi subscription windows.
- [`Davidcreador/herdr-token-dashboard`](https://github.com/Davidcreador/herdr-token-dashboard) at commit [`8450615`](https://github.com/Davidcreador/herdr-token-dashboard/tree/845061589bfef53df4312f67a1e7336e5e47cd8c). It demonstrates a keyboard-opened Herdr TUI, but measures per-session token use and estimated spend rather than provider subscription quota.
- [`chmarax/herdr-nvim`](https://github.com/chmarax/herdr-nvim) at commit [`40aadea`](https://github.com/chmarax/herdr-nvim/tree/40aadeab3cef3702ef5e05069181c7168084794f). Its supported-pane evacuation and rebuild pattern informed the full-height right-side toggle design.

No source code or assets were copied from these projects. Their public interaction and layout patterns informed this independently implemented plugin. Each upstream repository retains its own copyright and MIT license.

The runtime dependency [`quota-axi`](https://github.com/kunchenguid/quota-axi) is MIT-licensed. npm installs its license alongside the plugin-local package; its normalized JSON contract is consumed without vendoring its source.
