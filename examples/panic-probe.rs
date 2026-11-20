//! 崩溃落盘探针：验证 panic hook 真能在试点目录写出 crash-*.log。
//!
//! 用法：`cargo run --example panic-probe`（故意 panic，退出码非零）。
//! 断言方式：stdout 打印日志路径，外部脚本读该文件核对内容。

// 只用到 install_panic_hook；其余单实例能力在示例里是死代码，明确放行。
#[allow(dead_code)]
#[path = "../src/instance.rs"]
mod instance;

fn main() {
    instance::install_panic_hook(&instance::state_dir());
    // 先确认目录可写，再制造 panic（避免「目录不存在」伪装成落盘失败）。
    let dir = instance::state_dir();
    println!("[probe] 崩溃日志目录：{}", dir.display());
    panic!("panic-probe：这是预期内的崩溃，用来验证落盘链路");
}
