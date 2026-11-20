//! 设置 → 定时任务：跨 Bot 读取 Mode automation，启停 / 立即运行 / 删除。
//!
//! 数据面直接复用 `automation-repo`（与壳同一个 `{data_root}/v2/tasks-index.sqlite`），
//! 不新造 schema、不复制调度状态机。`automation_scheduler` 在 GPUI 内启动
//! `automation-runtime`：立即运行只写 manual run，调度器约 20 秒认领并执行。

use std::path::{Path, PathBuf};

use automation_repo::{Automation, AutomationRepo, AutomationRun, AutomationScope};
use automation_schedule::{IntervalUnit, MonthlyMode, ScheduleRule};

use crate::time;

/// 归属匹配用的 Bot 快照（避免数据层依赖 UI 的 Roster 类型）。
pub struct BotRef<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub directory: &'a str,
}

/// 一条定时任务 + 归属 Bot。
pub struct Entry {
    pub automation: Automation,
    pub bot_name: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunNowOutcome {
    /// 已写入 manual run，等待调度器认领。
    Queued,
    /// 同一任务已有排队/运行中的 manual run（single-flight）。
    Duplicate,
    /// 任务不存在或不属于当前工作区。
    NotFound,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusKind {
    Active,
    Paused,
    Completed,
    Failed,
}

/// `{data_root}/v2/tasks-index.sqlite`（与壳 `automation.rs` 同口径）。
pub fn db_path(data_root: &Path) -> PathBuf {
    data_root.join("v2").join("tasks-index.sqlite")
}

fn open(db_path: &Path) -> Result<AutomationRepo, String> {
    let repo = AutomationRepo::with_busy_timeout(db_path, 5_000);
    repo.ensure_ready().map_err(|error| error.to_string())?;
    Ok(repo)
}

/// 读全部定时任务并映射到 Bot、按面板顺序排序。
pub fn load(db_path: &Path, bots: &[BotRef<'_>]) -> Result<Vec<Entry>, String> {
    let repo = open(db_path)?;
    let automations = repo
        .list(&AutomationScope::default())
        .map_err(|error| error.to_string())?;
    repo.close();
    let mut entries: Vec<Entry> = automations
        .into_iter()
        .map(|automation| {
            let bot = bot_for(&automation, bots);
            Entry {
                bot_name: bot.map(|bot| bot.name.to_string()),
                automation,
            }
        })
        .collect();
    sort_entries(&mut entries);
    Ok(entries)
}

/// 设置页「+ 新建任务」：写一条与壳同 schema 的 automation（scope 归当前 Bot）。
pub fn create_task(
    db_path: &Path,
    workspace_path: &str,
    workspace_identity: Option<&str>,
    title: &str,
    cron_expr: &str,
    prompt: &str,
    recurring: bool,
) -> Result<String, String> {
    let repo = AutomationRepo::with_busy_timeout(db_path, 5_000);
    let service = automation_service::AutomationService::new(
        repo,
        automation_service::AutomationServiceOptions {
            offset_secs: crate::time::local_offset_secs() as i32,
        },
    );
    let scope = AutomationScope {
        workspace_path: Some(workspace_path.to_string()),
        workspace_identity: workspace_identity.map(str::to_string),
    };
    let created = service.create(
        &automation_service::CreateParams {
            title: title.to_string(),
            cron_expr: cron_expr.to_string(),
            relative_delay_minutes: None,
            prompt: prompt.to_string(),
            model_selection: None,
            mode: None,
            target_task_id: None,
            bot_delivery_target: None,
            recurring: Some(recurring),
            max_runs: None,
            end_at: None,
            interval_unit: None,
            interval: None,
            schedule_rule: None,
        },
        &scope,
        crate::time::now_ms() as i64,
    )?;
    Ok(created.automation_id)
}

pub fn set_enabled(
    db_path: &Path,
    automation_id: &str,
    enabled: bool,
    workspace_key: &str,
) -> Result<(), String> {
    let repo = open(db_path)?;
    let exists = repo
        .get(automation_id, Some(workspace_key))
        .map_err(|error| error.to_string())?;
    if exists.is_none() {
        repo.close();
        return Err("任务不存在或不属于当前工作区".to_string());
    }
    let result = repo
        .set_enabled(automation_id, enabled, Some(workspace_key))
        .map_err(|error| error.to_string());
    repo.close();
    result
}

pub fn run_now(
    db_path: &Path,
    automation_id: &str,
    workspace_key: &str,
) -> Result<RunNowOutcome, String> {
    let repo = open(db_path)?;
    let result = repo
        .enqueue_manual_run(automation_id, workspace_key, time::now_ms() as i64)
        .map_err(|error| error.to_string());
    repo.close();
    Ok(match result? {
        automation_repo::EnqueueManualRunOutcome::Queued => RunNowOutcome::Queued,
        automation_repo::EnqueueManualRunOutcome::Duplicate => RunNowOutcome::Duplicate,
        automation_repo::EnqueueManualRunOutcome::NotFound => RunNowOutcome::NotFound,
    })
}

pub fn delete(
    db_path: &Path,
    automation_id: &str,
    workspace_key: &str,
) -> Result<bool, String> {
    let repo = open(db_path)?;
    let result = repo
        .delete(automation_id, Some(workspace_key))
        .map_err(|error| error.to_string());
    repo.close();
    result
}

pub fn runs(
    db_path: &Path,
    automation_id: &str,
    workspace_key: &str,
    limit: usize,
) -> Result<Vec<AutomationRun>, String> {
    let repo = open(db_path)?;
    let result = repo
        .list_runs(automation_id, Some(workspace_key))
        .map_err(|error| error.to_string());
    repo.close();
    let mut runs = result?;
    runs.truncate(limit);
    Ok(runs)
}

/// 状态归类（与 WebView `resolveAutomationStatusKind` 同口径）。
pub fn status_kind(automation: &Automation) -> StatusKind {
    if automation.lifecycle_status == "failed" {
        StatusKind::Failed
    } else if automation.lifecycle_status == "completed" {
        StatusKind::Completed
    } else if automation.lifecycle_status == "paused" || !automation.enabled {
        StatusKind::Paused
    } else {
        StatusKind::Active
    }
}

pub fn status_label(automation: &Automation) -> &'static str {
    match status_kind(automation) {
        StatusKind::Active => "已启用",
        StatusKind::Paused => "已暂停",
        StatusKind::Completed => "已完成",
        StatusKind::Failed => "失败",
    }
}

/// 还能继续调度的任务（已启用 / 已暂停）；终态只能删除。
pub fn is_live(automation: &Automation) -> bool {
    matches!(
        status_kind(automation),
        StatusKind::Active | StatusKind::Paused
    )
}

/// 调度摘要：优先结构化 `scheduleRule`，缺规则才回退 cron 原文。
pub fn describe_schedule(automation: &Automation) -> String {
    if !automation.recurring && automation.max_runs.unwrap_or(1) <= 1 {
        return "一次性".to_string();
    }
    let Some(rule) = parse_rule(automation) else {
        return format!("自定义（cron {}）", automation.cron_expr.trim());
    };
    let interval = rule.interval.max(1.0).floor() as i64;
    let time = format!(
        "{:02}:{:02}",
        rule.hour.clamp(0, 23),
        rule.minute.clamp(0, 59)
    );
    match rule.unit {
        IntervalUnit::Minute => {
            if interval == 1 {
                "每分钟".to_string()
            } else {
                format!("每 {interval} 分钟")
            }
        }
        IntervalUnit::Hourly => {
            let minute = if rule.minute == 0 {
                String::new()
            } else {
                format!("第 {:02} 分", rule.minute.clamp(0, 59))
            };
            if interval == 1 {
                format!("每小时{minute}")
            } else {
                format!("每 {interval} 小时{minute}")
            }
        }
        IntervalUnit::Daily => {
            if interval == 1 {
                format!("每天 {time}")
            } else {
                format!("每 {interval} 天 {time}")
            }
        }
        IntervalUnit::Weekly => {
            let days = rule
                .weekdays
                .unwrap_or_default()
                .into_iter()
                .map(weekday_label)
                .collect::<Vec<_>>()
                .join("、");
            let prefix = if interval == 1 {
                "每".to_string()
            } else {
                format!("每 {interval} 周")
            };
            if days.is_empty() {
                format!("每周 {time}")
            } else {
                format!("{prefix}{days} {time}")
            }
        }
        IntervalUnit::Monthly => {
            let prefix = if interval == 1 {
                "每月".to_string()
            } else {
                format!("每 {interval} 个月")
            };
            if rule.monthly_mode == Some(MonthlyMode::Weekday) {
                match rule
                    .weekdays
                    .as_deref()
                    .and_then(|days| days.first())
                    .copied()
                {
                    Some(weekday) => {
                        format!("{prefix}第一个{} {time}", weekday_label(weekday))
                    }
                    _ => format!("{prefix} {time}"),
                }
            } else {
                let days = rule
                    .month_days
                    .unwrap_or_default()
                    .into_iter()
                    .map(|day| day.to_string())
                    .collect::<Vec<_>>()
                    .join("、");
                if days.is_empty() {
                    format!("{prefix} {time}")
                } else {
                    format!("{prefix} {days} 日 {time}")
                }
            }
        }
        IntervalUnit::Yearly => {
            let prefix = if interval == 1 {
                "每年".to_string()
            } else {
                format!("每 {interval} 年")
            };
            let months = rule
                .months
                .unwrap_or_default()
                .into_iter()
                .map(|month| format!("{month} 月"))
                .collect::<Vec<_>>()
                .join("、");
            let days = rule
                .month_days
                .unwrap_or_default()
                .into_iter()
                .map(|day| format!("{day} 日"))
                .collect::<Vec<_>>()
                .join("、");
            let date = match (months.is_empty(), days.is_empty()) {
                (true, true) => String::new(),
                (false, true) => months,
                (true, false) => days,
                (false, false) => format!("{months} {days}"),
            };
            if date.is_empty() {
                format!("{prefix} {time}")
            } else {
                format!("{prefix} {date} {time}")
            }
        }
    }
}

/// 面板排序：已启用 → 已暂停 → 终态；同组按下次运行升序（无则排后），再按更新时间降序。
pub fn sort_entries(entries: &mut [Entry]) {
    entries.sort_by(|left, right| {
        status_group(&left.automation)
            .cmp(&status_group(&right.automation))
            .then_with(|| {
                left.automation
                    .next_run_at
                    .unwrap_or(i64::MAX)
                    .cmp(&right.automation.next_run_at.unwrap_or(i64::MAX))
            })
            .then_with(|| {
                right
                    .automation
                    .updated_at
                    .cmp(&left.automation.updated_at)
            })
            .then_with(|| {
                left.automation
                    .automation_id
                    .cmp(&right.automation.automation_id)
            })
    });
}

fn status_group(automation: &Automation) -> u8 {
    match status_kind(automation) {
        StatusKind::Active => 0,
        StatusKind::Paused => 1,
        StatusKind::Completed | StatusKind::Failed => 2,
    }
}

/// 归属：先按 `mode-bot:teammate:<id>` 身份，再按工作目录（Windows 大小写不敏感）。
pub fn bot_for<'a>(automation: &Automation, bots: &'a [BotRef<'a>]) -> Option<&'a BotRef<'a>> {
    if let Some(identity) = automation.workspace_identity.as_deref()
        && let Some(id) = identity.strip_prefix("mode-bot:teammate:")
        && let Some(bot) = bots.iter().find(|bot| bot.id == id)
    {
        return Some(bot);
    }
    bots.iter().find(|bot| same_path(&automation.workspace_path, bot.directory))
}

fn same_path(left: &str, right: &str) -> bool {
    fn trim(path: &str) -> &str {
        path.trim().trim_end_matches(['\\', '/'])
    }
    trim(left).eq_ignore_ascii_case(trim(right))
}

fn parse_rule(automation: &Automation) -> Option<ScheduleRule> {
    serde_json::from_value(automation.schedule_rule.clone()?).ok()
}

fn weekday_label(day: i64) -> &'static str {
    match day.rem_euclid(7) {
        0 => "周日",
        1 => "周一",
        2 => "周二",
        3 => "周三",
        4 => "周四",
        5 => "周五",
        _ => "周六",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use automation_repo::{AutomationCreateOptions, AutomationCreateParams};
    use std::fs;

    fn sample_automation(
        id: &str,
        enabled: bool,
        lifecycle: &str,
        next_run_at: Option<i64>,
        updated_at: i64,
    ) -> Automation {
        Automation {
            automation_id: id.to_string(),
            title: format!("任务 {id}"),
            cron_expr: "0 9 * * *".to_string(),
            prompt: "写简报".to_string(),
            model_selection: None,
            mode: None,
            workspace_key: "C:\\ws".to_string(),
            workspace_path: "C:\\ws".to_string(),
            workspace_identity: None,
            target_task_id: None,
            location_kind: "local".to_string(),
            recurring: true,
            max_runs: None,
            end_at: None,
            schedule_rule: None,
            schedule_edited_by_user: None,
            run_count: 0,
            enabled,
            lifecycle_status: lifecycle.to_string(),
            next_run_at,
            last_run_at: None,
            dispatch_status: "idle".to_string(),
            dispatch_attempts: 0,
            retry_at: None,
            last_error: None,
            created_at: 1,
            updated_at,
        }
    }

    fn entry(automation: Automation) -> Entry {
        Entry {
            automation,
            bot_name: None,
        }
    }

    fn rule(value: serde_json::Value) -> Option<serde_json::Value> {
        Some(value)
    }

    #[test]
    fn status_labels_follow_lifecycle_and_enabled() {
        let active = sample_automation("a", true, "active", None, 1);
        let paused = sample_automation("b", false, "active", None, 1);
        let completed = sample_automation("c", true, "completed", None, 1);
        let failed = sample_automation("d", true, "failed", None, 1);
        assert_eq!(status_label(&active), "已启用");
        assert_eq!(status_label(&paused), "已暂停");
        assert_eq!(status_label(&completed), "已完成");
        assert_eq!(status_label(&failed), "失败");
        assert!(is_live(&active));
        assert!(is_live(&paused));
        assert!(!is_live(&completed));
        assert!(!is_live(&failed));
    }

    #[test]
    fn one_shot_is_described_as_one_shot() {
        let mut automation = sample_automation("a", true, "active", None, 1);
        automation.recurring = false;
        automation.max_runs = Some(1);
        assert_eq!(describe_schedule(&automation), "一次性");
    }

    #[test]
    fn interval_minute_is_described() {
        let mut automation = sample_automation("a", true, "active", None, 1);
        automation.schedule_rule = rule(serde_json::json!({
            "unit": "minute", "interval": 30, "hour": 0, "minute": 0, "anchorAt": 0
        }));
        assert_eq!(describe_schedule(&automation), "每 30 分钟");
    }

    #[test]
    fn daily_rule_is_described_with_time() {
        let mut automation = sample_automation("a", true, "active", None, 1);
        automation.schedule_rule = rule(serde_json::json!({
            "unit": "daily", "interval": 1, "hour": 9, "minute": 5, "anchorAt": 0
        }));
        assert_eq!(describe_schedule(&automation), "每天 09:05");
    }

    #[test]
    fn weekly_rule_lists_weekdays() {
        let mut automation = sample_automation("a", true, "active", None, 1);
        automation.schedule_rule = rule(serde_json::json!({
            "unit": "weekly", "interval": 1, "hour": 8, "minute": 0,
            "weekdays": [1, 3], "anchorAt": 0
        }));
        assert_eq!(describe_schedule(&automation), "每周一、周三 08:00");
    }

    #[test]
    fn monthly_date_rule_lists_days() {
        let mut automation = sample_automation("a", true, "active", None, 1);
        automation.schedule_rule = rule(serde_json::json!({
            "unit": "monthly", "interval": 1, "hour": 7, "minute": 30,
            "monthDays": [1, 15], "anchorAt": 0
        }));
        assert_eq!(describe_schedule(&automation), "每月 1、15 日 07:30");
    }

    #[test]
    fn monthly_weekday_rule_is_described() {
        let mut automation = sample_automation("a", true, "active", None, 1);
        automation.schedule_rule = rule(serde_json::json!({
            "unit": "monthly", "interval": 1, "hour": 7, "minute": 30,
            "weekdays": [1], "monthDays": [2], "monthlyMode": "weekday", "anchorAt": 0
        }));
        assert_eq!(describe_schedule(&automation), "每月第一个周一 07:30");
    }

    #[test]
    fn hourly_rule_includes_minute() {
        let mut automation = sample_automation("a", true, "active", None, 1);
        automation.schedule_rule = rule(serde_json::json!({
            "unit": "hourly", "interval": 1, "hour": 0, "minute": 15, "anchorAt": 0
        }));
        assert_eq!(describe_schedule(&automation), "每小时第 15 分");
    }

    #[test]
    fn yearly_rule_includes_interval() {
        let mut automation = sample_automation("a", true, "active", None, 1);
        automation.schedule_rule = rule(serde_json::json!({
            "unit": "yearly", "interval": 2, "hour": 10, "minute": 0,
            "months": [1], "monthDays": [1], "anchorAt": 0
        }));
        assert_eq!(describe_schedule(&automation), "每 2 年 1 月 1 日 10:00");
    }

    #[test]
    fn missing_rule_falls_back_to_cron() {
        let automation = sample_automation("a", true, "active", None, 1);
        assert_eq!(describe_schedule(&automation), "自定义（cron 0 9 * * *）");
    }

    #[test]
    fn sort_puts_active_first_then_paused_then_terminal() {
        let mut entries = vec![
            entry(sample_automation("failed", true, "failed", None, 9)),
            entry(sample_automation("paused", false, "paused", Some(50), 8)),
            entry(sample_automation("active-late", true, "active", Some(300), 7)),
            entry(sample_automation("active-early", true, "active", Some(100), 6)),
            entry(sample_automation("active-none", true, "active", None, 5)),
        ];
        sort_entries(&mut entries);
        let ids: Vec<&str> = entries
            .iter()
            .map(|entry| entry.automation.automation_id.as_str())
            .collect();
        assert_eq!(
            ids,
            vec!["active-early", "active-late", "active-none", "paused", "failed"]
        );
    }

    #[test]
    fn bot_for_matches_identity_before_directory() {
        let bots = [
            BotRef {
                id: "b1",
                name: "娜娜",
                directory: "C:\\other",
            },
            BotRef {
                id: "b2",
                name: "小北",
                directory: "C:\\ws",
            },
        ];
        let mut automation = sample_automation("a", true, "active", None, 1);
        automation.workspace_path = "C:\\ws".to_string();
        assert_eq!(bot_for(&automation, &bots).map(|bot| bot.id), Some("b2"));
        automation.workspace_identity = Some("mode-bot:teammate:b1".to_string());
        assert_eq!(bot_for(&automation, &bots).map(|bot| bot.id), Some("b1"));
        automation.workspace_identity = Some("someone-else".to_string());
        assert_eq!(bot_for(&automation, &bots).map(|bot| bot.id), Some("b2"));
    }

    struct TempDb {
        dir: PathBuf,
        db: PathBuf,
    }

    impl TempDb {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!(
                "bot-gpui-automations-{}",
                uuid::Uuid::new_v4()
            ));
            fs::create_dir_all(&dir).expect("create temp dir");
            let db = dir.join("tasks-index.sqlite");
            Self { dir, db }
        }

        fn seed(&self, title: &str, identity: Option<&str>, path: &str) -> String {
            let repo = AutomationRepo::with_busy_timeout(&self.db, 5_000);
            repo.ensure_ready().expect("ensure db");
            let automation = repo
                .create(
                    &AutomationCreateParams {
                        title: title.to_string(),
                        cron_expr: "0 9 * * *".to_string(),
                        prompt: "写简报".to_string(),
                        workspace_path: path.to_string(),
                        workspace_identity: identity.map(str::to_string),
                        recurring: true,
                        ..Default::default()
                    },
                    &AutomationCreateOptions {
                        next_run_at: Some(1_700_000_000_000),
                        lifecycle_status: None,
                    },
                )
                .expect("create automation");
            let id = automation.automation_id;
            repo.close();
            id
        }
    }

    impl Drop for TempDb {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn load_maps_workspace_to_bot() {
        let temp = TempDb::new();
        temp.seed("每日简报", Some("mode-bot:teammate:b1"), "C:\\ws");
        let bots = [BotRef {
            id: "b1",
            name: "娜娜",
            directory: "C:\\ws",
        }];
        let entries = load(&temp.db, &bots).expect("load");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].automation.title, "每日简报");
        assert_eq!(entries[0].bot_name.as_deref(), Some("娜娜"));
    }

    #[test]
    fn set_enabled_writes_paused_state() {
        let temp = TempDb::new();
        let id = temp.seed("每日简报", Some("mode-bot:teammate:b1"), "C:\\ws");
        let key = "mode-bot:teammate:b1";
        set_enabled(&temp.db, &id, false, key).expect("disable");
        let entries = load(&temp.db, &[]).expect("load");
        assert!(!entries[0].automation.enabled);
        assert_eq!(status_label(&entries[0].automation), "已暂停");
        set_enabled(&temp.db, &id, true, key).expect("enable");
        let entries = load(&temp.db, &[]).expect("load");
        assert!(entries[0].automation.enabled);
        assert_eq!(status_label(&entries[0].automation), "已启用");
    }

    #[test]
    fn run_now_queues_once_and_reports_duplicate() {
        let temp = TempDb::new();
        let id = temp.seed("每日简报", Some("mode-bot:teammate:b1"), "C:\\ws");
        let key = "mode-bot:teammate:b1";
        assert_eq!(
            run_now(&temp.db, &id, key).expect("first run"),
            RunNowOutcome::Queued
        );
        assert_eq!(
            run_now(&temp.db, &id, key).expect("second run"),
            RunNowOutcome::Duplicate
        );
        let history = runs(&temp.db, &id, key, 10).expect("history");
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].trigger, "manual");
        assert_eq!(history[0].dispatch_status, "claimed");
        // 关键：入队不能占用 automation 的 claim，宿主下一次轮询要能立刻认领。
        let repo = AutomationRepo::with_busy_timeout(&temp.db, 5_000);
        repo.ensure_ready().expect("ensure");
        let claimed = repo
            .claim_manual_runs(time::now_ms() as i64)
            .expect("claim manual");
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].run.trigger, "manual");
        repo.close();
    }

    #[test]
    fn wrong_workspace_key_is_a_no_op() {
        let temp = TempDb::new();
        let id = temp.seed("每日简报", Some("mode-bot:teammate:b1"), "C:\\ws");
        let wrong = "mode-bot:teammate:someone-else";
        assert!(set_enabled(&temp.db, &id, false, wrong).is_err());
        assert!(load(&temp.db, &[]).expect("load")[0].automation.enabled);
        assert_eq!(
            run_now(&temp.db, &id, wrong).expect("run"),
            RunNowOutcome::NotFound
        );
        assert!(!delete(&temp.db, &id, wrong).expect("delete"));
        assert!(runs(&temp.db, &id, wrong, 10).expect("runs").is_empty());
        assert_eq!(
            run_now(&temp.db, "missing-id", wrong).expect("run missing"),
            RunNowOutcome::NotFound
        );
    }

    #[test]
    fn delete_removes_definition() {
        let temp = TempDb::new();
        let id = temp.seed("每日简报", Some("mode-bot:teammate:b1"), "C:\\ws");
        let key = "mode-bot:teammate:b1";
        assert!(delete(&temp.db, &id, key).expect("delete"));
        assert!(load(&temp.db, &[]).expect("load").is_empty());
        assert!(!delete(&temp.db, &id, key).expect("delete again"));
    }

    #[test]
    fn runs_respect_limit_zero() {
        let temp = TempDb::new();
        let id = temp.seed("每日简报", Some("mode-bot:teammate:b1"), "C:\\ws");
        let key = "mode-bot:teammate:b1";
        let _ = run_now(&temp.db, &id, key).expect("run");
        let history = runs(&temp.db, &id, key, 0).expect("history");
        assert!(history.is_empty());
    }

    #[test]
    fn parse_rule_keeps_unit_and_interval() {
        let mut automation = sample_automation("a", true, "active", None, 1);
        automation.schedule_rule = rule(serde_json::json!({
            "unit": "hourly", "interval": 2, "hour": 0, "minute": 15, "anchorAt": 0
        }));
        let parsed = parse_rule(&automation).expect("parse");
        assert_eq!(parsed.unit, IntervalUnit::Hourly);
        assert_eq!(parsed.interval, 2.0);
        let _ = MonthlyMode::Date;
    }
}
