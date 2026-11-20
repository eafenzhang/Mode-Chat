//! 一次性验证工具：往 mode-bot:team 写一条团队记忆（验证注入后删除）。
//! 运行：cargo run --manifest-path apps/bot-gpui-pilot/Cargo.toml --bin seed-team-memory
fn main() {
    let storage = std::env::var("MODE_STORAGE_DIR").expect("MODE_STORAGE_DIR");
    let root = bots_service::memory_store::resolve_memory_root(
        Some(&storage),
        "mode-bot:team",
    )
    .expect("memory root");
    println!("root = {}", root.display());
    let id = bots_service::memory_store::write_memory(
        &root,
        bots_service::memory_store::WriteRequest {
            identity: "mode-bot:team",
            file: None,
            title: "试点注入验证",
            description: "GPUI 试点团队记忆注入验证条目（验证后可删除）",
            kind: "note",
            body: "团队约定：所有回复以「试点确认」开头，用于验证团队记忆注入是否生效。",
            end: None,
            pin: false,
        },
    )
    .expect("write");
    println!("written = {id}");
    let context =
        bots_service::memory_store::memory_context(&root, 4_000);
    println!("---- context ----\n{context}");
}
