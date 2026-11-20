# Mode Chat

Rust + GPUI desktop client for Mode Bot (the successor to the WebView shell).

This repository was extracted from the Mode monorepo as a standalone workspace.
It builds without Node or the TypeScript agent bundle; the agent engine is being
replaced by an embedded [pi-rust](https://github.com/bigfish1913/pi-rust) runtime.

## Status

- Standalone workspace builds (`cargo check` / `cargo test` green).
- The embedded pi-rust engine is the default for private chat, channels, IM turns
  and automations; sessions persist as JSONL under `~/.mode-bot/.mode/v2/mode-chat-sessions`.
- Node/TS agent runtime is no longer bundled. `MODE_CHAT_ENGINE=v4` keeps the old
  Mode Agent path as a migration escape hatch.
- Known gaps until the host-side MCP bridge lands: plugin market, connectors, and
  non-image attachments are unavailable in the embedded engine.

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
