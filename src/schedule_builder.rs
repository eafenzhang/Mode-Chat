//! 定时任务创建向导的纯逻辑（v0.5.0 W8，契约 §2.7）。
//!
//! 把「一次 / 每天 / 每周 / 每隔」四种选择器翻译成
//! `(cron, scheduleRule, recurring)` 三元组；cron 只用于展示与兼容，
//! 真正的调度权威是 `scheduleRule`（与壳同口径，见 `crates/automation-schedule`）。
//!
//! 时区：`scheduleRule.anchorAt` 与 `hour/minute` 都按**本机本地偏移**算，
//! 与 `src/automations.rs:91` 传的 `offset_secs` 一致。
//!
//! 契约 §2.7：`When` / `to_request` / `describe` 是给面板的固定接口，
//! UI 侧本期还没接线，所以对未接线项放开 `dead_code`。

#![allow(dead_code)]

use serde_json::json;

/// 向导里的四种时间选择。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum When {
    /// 一次性：`at_ms` 必须是未来时刻（毫秒时间戳）。
    Once { at_ms: i64 },
    /// 每天 `hour:minute`。
    Daily { hour: u32, minute: u32 },
    /// 每周指定几天（1=周一 … 7=周日）的 `hour:minute`。
    Weekly {
        weekdays: Vec<u32>,
        hour: u32,
        minute: u32,
    },
    /// 每隔 `minutes` 分钟（1–10080 = 7 天）。
    Every { minutes: u32 },
}

/// 交给 `automations::create_task` 的请求。
#[derive(Debug, Clone, PartialEq)]
pub struct ScheduleRequest {
    /// 兼容展示用 cron（调度权威是 `schedule_rule`）。
    pub cron: String,
    pub recurring: bool,
    pub schedule_rule: Option<serde_json::Value>,
}

const EVERY_MAX_MINUTES: u32 = 10_080;

fn check_time(hour: u32, minute: u32) -> Result<(), String> {
    if hour > 23 {
        return Err("小时必须在 0–23 之间".to_string());
    }
    if minute > 59 {
        return Err("分钟必须在 0–59 之间".to_string());
    }
    Ok(())
}

/// `When` → `(cron, schedule_rule, recurring)`。所有非法输入返回中文错误串。
pub fn to_request(when: &When, now_ms: i64, offset_secs: i32) -> Result<ScheduleRequest, String> {
    match when {
        When::Once { at_ms } => {
            if *at_ms <= now_ms {
                return Err("选一个未来的时间".to_string());
            }
            let local = local_parts(*at_ms, offset_secs);
            Ok(ScheduleRequest {
                cron: format!("{} {} {} {} *", local.1, local.0, local.2, local.3),
                recurring: false,
                schedule_rule: None,
            })
        }
        When::Daily { hour, minute } => {
            check_time(*hour, *minute)?;
            Ok(ScheduleRequest {
                cron: format!("{minute} {hour} * * *"),
                recurring: true,
                schedule_rule: Some(json!({
                    "unit": "daily",
                    "interval": 1,
                    "hour": hour,
                    "minute": minute,
                    "anchorAt": now_ms,
                })),
            })
        }
        When::Weekly {
            weekdays,
            hour,
            minute,
        } => {
            check_time(*hour, *minute)?;
            let mut days: Vec<u32> = weekdays
                .iter()
                .copied()
                .filter(|day| (1..=7).contains(day))
                .collect();
            days.sort_unstable();
            days.dedup();
            if days.is_empty() {
                return Err("至少选一天".to_string());
            }
            // cron 与 scheduleRule 的星期都是 0=周日 … 6=周六，向导用 1=周一…7=周日。
            let cron_days: Vec<u32> = days.iter().map(|day| day % 7).collect();
            let rule_days: Vec<i64> = cron_days.iter().map(|day| i64::from(*day)).collect();
            Ok(ScheduleRequest {
                cron: format!(
                    "{minute} {hour} * * {}",
                    cron_days
                        .iter()
                        .map(u32::to_string)
                        .collect::<Vec<_>>()
                        .join(",")
                ),
                recurring: true,
                schedule_rule: Some(json!({
                    "unit": "weekly",
                    "interval": 1,
                    "hour": hour,
                    "minute": minute,
                    "anchorAt": now_ms,
                    "weekdays": rule_days,
                })),
            })
        }
        When::Every { minutes } => {
            if *minutes < 1 || *minutes > EVERY_MAX_MINUTES {
                return Err("1 到 10080 分钟".to_string());
            }
            // 间隔型任务没有单一「几点」：用「每 N 分钟」的 cron 表达展示语义。
            let cron = if *minutes < 60 && 60 % *minutes == 0 {
                format!("*/{minutes} * * * *")
            } else if *minutes % 60 == 0 {
                let hours = (*minutes / 60).min(23);
                format!("0 */{hours} * * *")
            } else {
                // 非整小时：cron 无法精确表达，落成每分钟一次的展示值，
                // 真正触发按 scheduleRule 的 minute 间隔算。
                "* * * * *".to_string()
            };
            Ok(ScheduleRequest {
                cron,
                recurring: true,
                schedule_rule: Some(json!({
                    "unit": "minute",
                    "interval": minutes,
                    "hour": 0,
                    "minute": 0,
                    "anchorAt": now_ms,
                })),
            })
        }
    }
}

/// 只为一处展示算本地时间：返回 `(hour, minute, day, month)`。
fn local_parts(ms: i64, offset_secs: i32) -> (u32, u32, u32, u32) {
    // 与 automation-schedule 的 `to_local` 同口径：秒级偏移 + 民用历换算。
    let total_secs = (ms / 1000) + i64::from(offset_secs);
    let days = total_secs.div_euclid(86_400);
    let second_of_day = total_secs.rem_euclid(86_400);
    let hour = (second_of_day / 3_600) as u32;
    let minute = ((second_of_day % 3_600) / 60) as u32;
    let (year, month, day) = civil_from_days(days);
    let _ = year;
    (hour, minute, day, month)
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// 人类可读的一句话（UI 预览 + toast）。
pub fn describe(when: &When) -> String {
    let weekday_names = ["一", "二", "三", "四", "五", "六", "日"];
    match when {
        When::Once { .. } => "一次".to_string(),
        When::Daily { hour, minute } => format!("每天 {hour:02}:{minute:02}"),
        When::Weekly {
            weekdays,
            hour,
            minute,
        } => {
            let days = weekdays
                .iter()
                .filter(|day| (1..=7).contains(*day))
                .map(|day| format!("周{}", weekday_names[(day - 1) as usize]))
                .collect::<Vec<_>>()
                .join("、");
            format!("每 {days} {hour:02}:{minute:02}")
        }
        When::Every { minutes } => format!("每隔 {minutes} 分钟"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daily_rule_round_trip() {
        let request = to_request(
            &When::Daily {
                hour: 9,
                minute: 5,
            },
            1_700_000_000_000,
            28_800,
        )
        .unwrap();
        assert_eq!(request.cron, "5 9 * * *");
        assert!(request.recurring);
        let rule = request.schedule_rule.unwrap();
        assert_eq!(rule["unit"], "daily");
        assert_eq!(rule["hour"], 9);
        assert_eq!(rule["minute"], 5);
    }

    #[test]
    fn weekly_maps_sunday_to_zero() {
        let request = to_request(
            &When::Weekly {
                weekdays: vec![1, 3, 7],
                hour: 9,
                minute: 0,
            },
            1_700_000_000_000,
            28_800,
        )
        .unwrap();
        assert_eq!(request.cron, "0 9 * * 1,3,0");
        let rule = request.schedule_rule.unwrap();
        assert_eq!(rule["weekdays"], json!([1, 3, 0]));
    }

    #[test]
    fn weekly_requires_a_day() {
        let error = to_request(
            &When::Weekly {
                weekdays: vec![],
                hour: 9,
                minute: 0,
            },
            0,
            0,
        )
        .unwrap_err();
        assert!(error.contains("至少选一天"));
        // 越界的天被过滤后同样报错。
        assert!(
            to_request(
                &When::Weekly {
                    weekdays: vec![0, 9],
                    hour: 9,
                    minute: 0,
                },
                0,
                0
            )
            .is_err()
        );
    }

    #[test]
    fn every_accepts_minutes_in_range() {
        let request = to_request(&When::Every { minutes: 15 }, 0, 0).unwrap();
        assert!(request.recurring);
        assert_eq!(request.cron, "*/15 * * * *");
        assert_eq!(request.schedule_rule.unwrap()["interval"], 15);
        assert!(to_request(&When::Every { minutes: 0 }, 0, 0).is_err());
        assert!(to_request(&When::Every { minutes: 10_081 }, 0, 0).is_err());
    }

    #[test]
    fn once_requires_future() {
        assert!(to_request(&When::Once { at_ms: 1_000 }, 2_000, 0).is_err());
        let request = to_request(&When::Once { at_ms: 3_000 }, 2_000, 0).unwrap();
        assert!(!request.recurring);
        assert!(request.schedule_rule.is_none());
        assert_eq!(request.cron.split_whitespace().count(), 5);
    }

    #[test]
    fn describe_is_readable() {
        assert_eq!(
            describe(&When::Daily {
                hour: 9,
                minute: 0
            }),
            "每天 09:00"
        );
        assert_eq!(
            describe(&When::Weekly {
                weekdays: vec![1, 3],
                hour: 9,
                minute: 30
            }),
            "每 周一、周三 09:30"
        );
        assert_eq!(describe(&When::Every { minutes: 45 }), "每隔 45 分钟");
    }
}
