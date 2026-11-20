//! 窗口几何持久化（框架级）：下次启动回到上次的位置/大小/最大化状态。
//!
//! 为什么自己存：gpui 这版没有「窗口 bounds 变化」的观察者，`platform_window.on_resize`
//! 是内部实现。这里的做法是——渲染时把当前几何记进进程内共享槽（几乎零成本），
//! 后台线程每 5 秒比对一次，有变化就原子写盘；关窗/退出时立即写一次。
//!
//! 落盘位置与试点状态文件同目录（`BOT_PILOT_STATE_FILE` 一并隔离），文件
//! `window.json`：`{ x, y, width, height, maximized, offset_x, offset_y }`。
//! 坐标做基本兜底（有限值、绝对值不超过 20000、尺寸有下限），避免显示器变化后窗口
//! 跑到屏幕外——超出可见区时回落到默认居中位置。**负坐标是合法值**：多显示器时
//! 左侧/上方屏幕的原点就是负的，早期版本按「非负」校验会把窗口位置重置。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// 默认窗口尺寸（与 `main()` 的初始值一致）。
pub const DEFAULT_WIDTH: f32 = 1100.0;
pub const DEFAULT_HEIGHT: f32 = 720.0;
/// 窗口最小尺寸（gpui `window_min_size` 同值）。
pub const MIN_WIDTH: f32 = 900.0;
pub const MIN_HEIGHT: f32 = 600.0;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Geometry {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    #[serde(default)]
    pub maximized: bool,
    /// 恢复补偿：请求原点到实际原点的差（`请求 - 实际`）。
    ///
    /// 为什么需要：gpui-windows 用 `GetWindowPlacement` + border offset 反算 client
    /// 原点，本机 DWM 的不可见边框不对称（左右 8、上 0、下 8），它却按 1/2 均分，
    /// 结果每次启动 y 上移 4px、反复重启会一路爬到屏幕顶。这里在首帧实测一次
    /// 「请求 - 实际」，下次恢复时把差值加回去，位置就固定了。
    #[serde(default)]
    pub offset_x: f32,
    #[serde(default)]
    pub offset_y: f32,
}

/// 修复量上限：只兜这种几个像素的边框误差，配置损坏时不许把窗口甩出屏幕。
const MAX_OFFSET: f32 = 64.0;

static SHARED: OnceLock<Mutex<Option<Geometry>>> = OnceLock::new();
/// 首帧实测的恢复补偿（`calibrate` 写入，`remember` 带上）。
static CALIBRATION: Mutex<Option<(f32, f32)>> = Mutex::new(None);
/// 本次启动请求的原点（首帧校准后清空）。
static PENDING_REQUEST: Mutex<Option<(f32, f32)>> = Mutex::new(None);
/// 后台线程是否已经起过（只起一次）。
static AUTOSAVE_STARTED: AtomicBool = AtomicBool::new(false);

fn shared() -> &'static Mutex<Option<Geometry>> {
    SHARED.get_or_init(|| Mutex::new(None))
}

fn path() -> PathBuf {
    crate::instance::state_dir().join("window.json")
}

/// 读上次保存的几何（非法/缺失返回 None）。
pub fn load() -> Option<Geometry> {
    let bytes = std::fs::read(path()).ok()?;
    let geometry: Geometry = serde_json::from_slice(&bytes).ok()?;
    Some(sanitize(geometry))
}

/// 规范化：尺寸不低于最小值；位置为负（窗口在屏幕左侧/上方之外）时回落到默认。
fn sanitize(geometry: Geometry) -> Geometry {
    let width = if geometry.width.is_finite() && geometry.width >= MIN_WIDTH {
        geometry.width
    } else {
        DEFAULT_WIDTH
    };
    let height = if geometry.height.is_finite() && geometry.height >= MIN_HEIGHT {
        geometry.height
    } else {
        DEFAULT_HEIGHT
    };
    // 多显示器允许负坐标（副屏在主屏左侧/上方时原点为负）；只挡非有限值与离谱值。
    let sane = |value: f32| value.is_finite() && value.abs() <= 20_000.0;
    let (x, y) = if sane(geometry.x) && sane(geometry.y) {
        (geometry.x, geometry.y)
    } else {
        (120.0, 90.0)
    };
    let clamp_offset = |value: f32| {
        if value.is_finite() {
            value.clamp(-MAX_OFFSET, MAX_OFFSET)
        } else {
            0.0
        }
    };
    Geometry {
        x,
        y,
        width,
        height,
        maximized: geometry.maximized,
        offset_x: clamp_offset(geometry.offset_x),
        offset_y: clamp_offset(geometry.offset_y),
    }
}

/// 首帧用：把「请求原点 - 实际原点」记下来（窗口创建后调用一次）。
pub fn calibrate(requested: (f32, f32), achieved: (f32, f32)) {
    let clamp = |value: f32| {
        if value.is_finite() {
            value.clamp(-MAX_OFFSET, MAX_OFFSET)
        } else {
            0.0
        }
    };
    if let Ok(mut slot) = CALIBRATION.lock() {
        *slot = Some((clamp(requested.0 - achieved.0), clamp(requested.1 - achieved.1)));
    }
}

/// 启动时记下本次请求的原点；首帧拿到真实 bounds 后 `calibrate_if_pending` 用。
pub fn set_restore_request(origin: (f32, f32)) {
    if let Ok(mut slot) = PENDING_REQUEST.lock() {
        *slot = Some(origin);
    }
}

/// 首帧调用：如果启动时有恢复请求，用实际原点算一次补偿（只算一次）。
pub fn calibrate_if_pending(achieved: (f32, f32)) {
    let pending = PENDING_REQUEST.lock().ok().and_then(|mut slot| slot.take());
    if let Some(requested) = pending {
        calibrate(requested, achieved);
    }
}

/// 渲染时调用：把当前几何记进共享槽（只写内存，不碰磁盘）。
pub fn remember(mut geometry: Geometry) {
    // 带上首帧实测的补偿，下次恢复才对得上同一处视觉位置。
    if let Some((offset_x, offset_y)) = CALIBRATION.lock().ok().and_then(|slot| *slot) {
        geometry.offset_x = offset_x;
        geometry.offset_y = offset_y;
    }
    if let Ok(mut slot) = shared().lock() {
        let changed = slot.is_none_or(|previous| previous != geometry);
        if changed {
            *slot = Some(geometry);
        }
    }
}

/// 恢复用原点：存储值 + 首帧实测补偿（见 `Geometry::offset_y` 的说明）。
pub fn restore_origin(geometry: &Geometry) -> (f32, f32) {
    (geometry.x + geometry.offset_x, geometry.y + geometry.offset_y)
}

/// 立即落盘（关窗/退出时调用）。
pub fn save_now() {
    let geometry = shared().lock().ok().and_then(|slot| *slot);
    let Some(geometry) = geometry else {
        return;
    };
    write(&geometry);
}

/// 起一个 5 秒轮询的后台线程（只起一次）：共享槽变化就写盘。
pub fn spawn_autosave() {
    if AUTOSAVE_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || {
        let mut last_written: Option<Geometry> = load();
        loop {
            std::thread::sleep(Duration::from_secs(5));
            let current = shared().lock().ok().and_then(|slot| *slot);
            let Some(current) = current else {
                continue;
            };
            if Some(current) == last_written {
                continue;
            }
            if write(&current) {
                last_written = Some(current);
            }
        }
    });
}

fn write(geometry: &Geometry) -> bool {
    let path = path();
    if let Some(parent) = path.parent()
        && std::fs::create_dir_all(parent).is_err()
    {
        return false;
    }
    let Ok(value) = serde_json::to_value(geometry) else {
        return false;
    };
    // 这是「下次启动偏好」类数据：抢不到锁就跳过这一轮，不阻塞 UI。
    atomic_file::write_json_atomic(
        &path,
        &value,
        &atomic_file::WriteOptions {
            lock_max_wait_ms: 200,
            ..Default::default()
        },
    )
    .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_clamps_bad_geometry() {
        let fixed = sanitize(Geometry {
            x: f32::NAN,
            y: 10.0,
            width: 10.0,
            height: 10.0,
            maximized: false,
            offset_x: f32::NAN,
            offset_y: 9_000.0,
        });
        assert_eq!(fixed.width, DEFAULT_WIDTH);
        assert_eq!(fixed.height, DEFAULT_HEIGHT);
        assert_eq!(fixed.x, 120.0, "非有限坐标回落到默认");
        // 多显示器：负坐标是合法位置，不能被重置。
        let secondary = sanitize(Geometry {
            x: -1400.0,
            y: -200.0,
            width: 1280.0,
            height: 800.0,
            maximized: false,
            offset_x: 0.0,
            offset_y: 0.0,
        });
        assert_eq!(restore_origin(&secondary), (-1400.0, -200.0));
        let absurd = sanitize(Geometry {
            x: 99_999.0,
            y: 0.0,
            width: 1280.0,
            height: 800.0,
            maximized: false,
            offset_x: 0.0,
            offset_y: 0.0,
        });
        assert_eq!(absurd.x, 120.0, "离谱坐标回落到默认");
        assert_eq!(fixed.offset_x, 0.0, "非有限补偿归零");
        assert_eq!(fixed.offset_y, MAX_OFFSET, "过大的补偿要截断");

        let ok = sanitize(Geometry {
            x: 200.0,
            y: 120.0,
            width: 1280.0,
            height: 800.0,
            maximized: true,
            offset_x: 1.0,
            offset_y: -4.0,
        });
        assert_eq!(ok.width, 1280.0);
        assert_eq!(ok.height, 800.0);
        assert!(ok.maximized);
        assert_eq!(restore_origin(&ok), (201.0, 116.0));
    }
}
