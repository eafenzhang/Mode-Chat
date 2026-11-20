//! 测试夹具：在指定 tasks-index.sqlite 里创建一个 automation 并入队一次 manual run。
//!
//! ```bash
//! cargo run --example automation-seed -- <db> <workspace> <identity>
//! ```
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use automation_repo::{
    AutomationCreateOptions, AutomationCreateParams, AutomationRepo, ModelSelection, SelectionOptions,
};

fn main() {
    let mut args = std::env::args().skip(1);
    let db = args.next().expect("db path");
    let workspace = args.next().expect("workspace path");
    let identity = args.next().expect("workspace identity");
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0);
    let repo = AutomationRepo::with_busy_timeout(PathBuf::from(db), 5_000);
    let params = AutomationCreateParams {
        title: "GPUI scheduler E2E".to_string(),
        cron_expr: "0 0 1 1 *".to_string(),
        prompt: "只回复两个字：收到".to_string(),
        model_selection: Some(ModelSelection {
            provider_id: "deepseek".to_string(),
            model_id: "deepseek-flash".to_string(),
            options: Some(SelectionOptions { reasoning_level: Some("max".to_string()) }),
        }),
        mode: None,
        workspace_path: workspace,
        workspace_identity: Some(identity),
        target_task_id: None,
        bot_delivery_target: None,
        recurring: true,
        max_runs: None,
        end_at: None,
        schedule_rule: None,
    };
    let automation = repo
        .create(
            &params,
            &AutomationCreateOptions {
                next_run_at: None,
                lifecycle_status: None,
            },
        )
        .expect("create automation");
    let outcome = repo
        .enqueue_manual_run(&automation.automation_id, &automation.workspace_key, now)
        .expect("enqueue manual run");
    println!(
        "automation={} workspace_key={} enqueue={outcome:?}",
        automation.automation_id, automation.workspace_key
    );
}
