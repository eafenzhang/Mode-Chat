//! 时间格式化（UI 用）：只做「毫秒时间戳 → 本地日期/时分」，不引 chrono。
//!
//! 本地时区偏移在进程启动后取一次：`GetLocalTime` 给出本地墙上时间，
//! `SystemTime::now()` 给出 UTC，两者的差就是偏移（到分钟取整，DST 切换当天
//! 最多差一小时，对这个用途足够）。

use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

/// 当前 UTC 毫秒时间戳。
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// 本地时区相对 UTC 的偏移（秒），进程内取一次。
pub fn local_offset_secs() -> i64 {
    static OFFSET: OnceLock<i64> = OnceLock::new();
    *OFFSET.get_or_init(|| {
        let utc_now = now_ms() as i64 / 1000;
        let local = local_wall_secs();
        // 取整到分钟，避免秒级抖动导致负偏移被截断。
        let diff = local.unwrap_or(utc_now) - utc_now;
        (diff as f64 / 60.0).round() as i64 * 60
    })
}

/// `GetLocalTime` 的本地墙上时间（自 epoch 起的秒）。
#[cfg(windows)]
fn local_wall_secs() -> Option<i64> {
    #[repr(C)]
    struct SystemTimeLocal {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetLocalTime(out: *mut SystemTimeLocal);
    }
    let mut time = SystemTimeLocal {
        year: 0,
        month: 0,
        day_of_week: 0,
        day: 0,
        hour: 0,
        minute: 0,
        second: 0,
        milliseconds: 0,
    };
    // SAFETY: 传入的是本栈上的有效结构体指针。
    unsafe { GetLocalTime(&mut time) };
    if time.year < 1970 {
        return None;
    }
    let days = days_from_civil(time.year as i64, time.month as u32, time.day as u32);
    Some(days * 86_400 + i64::from(time.hour) * 3600 + i64::from(time.minute) * 60 + i64::from(time.second))
}

#[cfg(not(windows))]
fn local_wall_secs() -> Option<i64> {
    None
}

/// 天数（1970-01-01 起）转年月日（Howard Hinnant 算法）。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// 年月日 → 天数（`civil_from_days` 的逆）。
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = month as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn local_parts(ms: u64) -> (i64, u32, u32, u32, u32) {
    let secs = (ms as i64 / 1000) + local_offset_secs();
    let days = secs.div_euclid(86_400);
    let of_day = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    (
        year,
        month,
        day,
        (of_day / 3600) as u32,
        ((of_day % 3600) / 60) as u32,
    )
}

/// `2026-11-14`
pub fn format_day(ms: u64) -> String {
    let (year, month, day, _, _) = local_parts(ms);
    format!("{year:04}-{month:02}-{day:02}")
}

/// `11:28`
pub fn format_hm(ms: u64) -> String {
    let (_, _, _, hour, minute) = local_parts(ms);
    format!("{hour:02}:{minute:02}")
}

/// 日期分隔标题：今天 / 昨天 / 2026-11-12。
pub fn day_label(ms: u64) -> String {
    day_label_at(ms, now_ms())
}

fn day_label_at(ms: u64, now: u64) -> String {
    let day = format_day(ms);
    if day == format_day(now) {
        return crate::i18n::t("今天").to_string();
    }
    if format_day(now.saturating_sub(86_400_000)) == day {
        return crate::i18n::t("昨天").to_string();
    }
    day
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn day_label_uses_now_for_yesterday() {
        let now = now_ms();
        assert_eq!(day_label_at(now, now), "今天");
        assert_eq!(day_label_at(now.saturating_sub(86_400_000), now), "昨天");
    }

    #[test]
    fn civil_roundtrip() {
        for (year, month, day) in [
            (1970, 1, 1),
            (2000, 2, 29),
            (2026, 11, 14),
            (2038, 1, 19),
        ] {
            let days = days_from_civil(year, month, day);
            assert_eq!(civil_from_days(days), (year, month, day));
        }
    }

    #[test]
    fn day_and_hm_are_stable_for_known_instant() {
        // 2026-11-14T00:00:00Z 的毫秒；本地时区不同，只断言形状与一致性。
        let ms = 1_794_528_000_000u64;
        let day = format_day(ms);
        assert_eq!(day.len(), 10, "日期形如 YYYY-MM-DD：{day}");
        let hm = format_hm(ms);
        assert_eq!(hm.len(), 5, "时间形如 HH:MM：{hm}");
        assert_eq!(format_day(ms), format_day(ms));
    }

    #[test]
    fn yesterday_label_detected() {
        let now = now_ms();
        assert_eq!(day_label(now), "今天");
        if let Some(yesterday) = now.checked_sub(86_400_000) {
            // 跨月/跨年也应是「昨天」或日期字符串，不应 panic。
            let label = day_label(yesterday);
            assert!(label == "昨天" || label.len() == 10, "昨天标签异常：{label}");
        }
    }
}
