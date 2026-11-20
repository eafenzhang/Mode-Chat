//! Mode Bot 的 teammate / 群组领域层。
//!
//! 边界：只放纯数据模型与纯逻辑（校验、身份键、@提及、群 prompt），不做文件 IO、
//! 不依赖 Tauri / services / agent runtime。落盘与命令接线留在 `apps/bot-tauri`，
//! 这样同一份规则可以被 Tauri command、未来的 Rust 服务频道和测试共用。
//!
//! 行为真源：本 crate 的表驱动测试与子 spec `mode-bot-rust-migration.md`；
//! renderer 只保留 `groups/types.ts` / `groups/mentions.ts` 的渲染侧契约。
//! 2026-11-09 W0 起 renderer 回退执行器（`turn.ts`/`orchestration.ts`/`v4Driver.ts`）
//! 已删除，群编排唯一执行体是壳侧 `orchestrator.rs`。

pub mod approval;
pub mod capabilities;
pub mod context_packet;
pub mod goal;
pub mod group;
pub mod group_state;
pub mod idle_guard;
pub mod identity;
pub mod mentions;
pub mod orchestration;
pub mod plugin;
pub mod prompt;
pub mod roster_status;
pub mod tasks;
pub mod team_file;
pub mod teammate;
