# Mode Chat

Rust + GPUI desktop client for Mode Bot (the successor to the WebView shell).

This repository was extracted from the Mode monorepo as a standalone workspace.
It builds without Node or the TypeScript agent bundle; the agent engine is being
replaced by an embedded [pi-rust](https://github.com/bigfish1913/pi-rust) runtime.

## Status

- Standalone workspace builds (`cargo check` / `cargo test` green).
- The embedded pi-rust engine is the only engine for private chat, channels, IM
  turns and automations; sessions persist as JSONL under
  `~/.mode-bot/.mode/v2/mode-chat-sessions`.
- The legacy v4 client and its crates (`agent-process`, `agent-protocol`,
  `mode-protocol-client`) have been removed; no Node/TS runtime is bundled.
- Known gaps until the host-side MCP bridge lands: plugin market, connectors, and
  non-image attachments are unavailable in the embedded engine.

## Capabilities

- **Steering (插话)** — during a running turn `Ctrl+Enter` (or the composer's
  steer button) pushes the text into the engine's steering queue instead of
  queueing it for later; plain `Enter` still queues. Only the embedded engine has
  a steering channel — external ACP sessions fall back to queueing.
- **Side chat (`/side`)** — a throwaway session that is neither written to the
  main transcript nor shown in history or global search; its JSONL lands under
  `mode-chat-sessions/.side`. Open it from the composer (`/side`), the `⋯` menu,
  or the command palette. Attachments, question cards and forms are not
  supported there yet.
- **Private bots** — a private bot keeps its sessions under
  `mode-chat-sessions/.private/<botId>`, is excluded from global search, cannot
  join a group, and cannot be the team Chief.
- **Team Chief** — only the single `isChief` bot (enforced on create and update)
  gets the team tools in a group: `team_roster`, `team_goal_show|create|stage`,
  `team_task_list`, `team_assign`. `team_assign` writes a child task onto the
  group task ledger and dispatches the target member's turn; re-dispatch of a
  member already running in the same turn is rejected, and one turn dispatches
  at most 4 tasks.

## Build

```powershell
cargo build --release
```

The binary is `target/release/bot-gpui-pilot.exe` (rename pending).

## Data

Same data root as Mode Bot: `~/.mode-bot/.mode` (roster, groups, sessions,
credentials, automations). Set `MODE_BOT_DATA_ROOT` to isolate a test instance.

## Layout

- `src/` — GPUI app (Windows).
- `crates/` — pure-Rust services extracted from the Mode monorepo
  (roster/identity, groups/memory/IM, automations, connectors config, credentials).
- `assets/` — icons and fonts.
- `examples/` — developer probes.

See `docs/pilot-notes.md` for the original pilot implementation notes.
