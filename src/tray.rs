//! 托盘图标（框架级）：常驻通知区 + 左键唤起 + 右键菜单（显示/退出）。
//!
//! 为什么不用 gpui 的托盘能力：gpui-pre/gpui-kit 没有托盘 API。这里用 Win32
//! `Shell_NotifyIconW` 自研，并且**不改 gpui 的 wndproc**：单独起一个线程，创建一个
//! 隐藏的普通窗口（从不 ShowWindow，不上任务栏）接收托盘回调，再对主窗口做
//! `ShowWindow` / `SetForegroundWindow` / `PostMessage(WM_CLOSE)`。
//!
//! 已知限制：点窗口的最小化按钮仍是普通最小化（拦 SC_MINIMIZE 需要主窗口 wndproc，
//! 在 gpui 内部）；托盘只提供「唤起 / 退出」。进程退出时图标由系统在下次悬停时清理。

#![cfg(windows)]

use std::ffi::c_void;
use std::sync::atomic::{AtomicPtr, Ordering};

type Hwnd = *mut c_void;
type Hicon = *mut c_void;
type Hmenu = *mut c_void;
type Hinstance = *mut c_void;

const NIM_ADD: u32 = 0;
const NIM_MODIFY: u32 = 1;
const NIF_MESSAGE: u32 = 0x1;
const NIF_ICON: u32 = 0x2;
const NIF_TIP: u32 = 0x4;
const NIF_INFO: u32 = 0x10;
const NIIF_INFO: u32 = 0x1;
const WM_APP: u32 = 0x8000;
/// 托盘回调消息（WM_APP + 1）。
const WM_TRAY: u32 = WM_APP + 1;
const WM_LBUTTONUP: usize = 0x0202;
const WM_RBUTTONUP: usize = 0x0205;
const WM_CLOSE: u32 = 0x0010;
const WM_NULL: u32 = 0;
const SW_HIDE: i32 = 0;
const SW_SHOW: i32 = 5;
const SW_RESTORE: i32 = 9;
const MF_STRING: u32 = 0x0000;
const TPM_RETURNCMD: u32 = 0x0100;
const TPM_RIGHTBUTTON: u32 = 0x0002;
const TPM_NONOTIFY: u32 = 0x0080;
const IMAGE_ICON: u32 = 1;
const LR_LOADFROMFILE: u32 = 0x0010;
const LR_DEFAULTSIZE: u32 = 0x0040;

#[repr(C)]
struct NotifyIconDataW {
    cb_size: u32,
    hwnd: Hwnd,
    uid: u32,
    flags: u32,
    callback_message: u32,
    hicon: Hicon,
    tip: [u16; 128],
    state: u32,
    state_mask: u32,
    info: [u16; 256],
    timeout_or_version: u32,
    info_title: [u16; 64],
    info_flags: u32,
    guid_item: [u8; 16],
    balloon_icon: Hicon,
}

#[repr(C)]
struct WndClassW {
    style: u32,
    wnd_proc: Option<unsafe extern "system" fn(Hwnd, u32, usize, isize) -> isize>,
    cls_extra: i32,
    wnd_extra: i32,
    instance: Hinstance,
    icon: Hicon,
    cursor: *mut c_void,
    background: *mut c_void,
    menu_name: *const u16,
    class_name: *const u16,
}

#[repr(C)]
#[derive(Default)]
struct Point {
    x: i32,
    y: i32,
}

#[repr(C)]
#[derive(Default)]
struct Msg {
    hwnd: Hwnd,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    pt: Point,
    private: u32,
}

#[link(name = "user32")]
unsafe extern "system" {
    fn RegisterClassW(class: *const WndClassW) -> u16;
    fn CreateWindowExW(
        ex_style: u32,
        class_name: *const u16,
        window_name: *const u16,
        style: u32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        parent: Hwnd,
        menu: Hmenu,
        instance: Hinstance,
        param: *mut c_void,
    ) -> Hwnd;
    fn DefWindowProcW(hwnd: Hwnd, message: u32, wparam: usize, lparam: isize) -> isize;
    fn GetMessageW(msg: *mut Msg, hwnd: Hwnd, min: u32, max: u32) -> i32;
    fn TranslateMessage(msg: *const Msg) -> i32;
    fn DispatchMessageW(msg: *const Msg) -> isize;
    fn FindWindowW(class: *const u16, title: *const u16) -> Hwnd;
    fn LoadImageW(
        instance: Hinstance,
        name: *const u16,
        kind: u32,
        cx: i32,
        cy: i32,
        flags: u32,
    ) -> Hicon;
    fn ShowWindow(hwnd: Hwnd, cmd: i32) -> i32;
    fn GetForegroundWindow() -> Hwnd;
    fn SetForegroundWindow(hwnd: Hwnd) -> i32;
    fn PostMessageW(hwnd: Hwnd, message: u32, wparam: usize, lparam: isize) -> i32;
    fn CreatePopupMenu() -> Hmenu;
    fn AppendMenuW(menu: Hmenu, flags: u32, id: usize, text: *const u16) -> i32;
    fn DestroyMenu(menu: Hmenu) -> i32;
    fn TrackPopupMenu(
        menu: Hmenu,
        flags: u32,
        x: i32,
        y: i32,
        reserved: i32,
        hwnd: Hwnd,
        rect: *const c_void,
    ) -> i32;
    fn GetCursorPos(point: *mut Point) -> i32;
}

#[link(name = "shell32")]
unsafe extern "system" {
    fn Shell_NotifyIconW(message: u32, data: *const NotifyIconDataW) -> i32;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetModuleHandleW(name: *const u16) -> Hinstance;
}

/// Win32 句柄按值传给托盘线程用（句柄本身由系统持有，线程间传递安全）。
struct SendHandle(Hicon);
// SAFETY: HICON 是不透明内核句柄，不指向 Rust 内存；只在托盘线程里用于 NIM_ADD。
unsafe impl Send for SendHandle {}

impl SendHandle {
    /// 取回句柄（走方法调用，闭包才会捕获整个 Send 包装而不是裸指针字段）。
    fn take(self) -> Hicon {
        self.0
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 托盘隐藏窗口句柄（install 线程创建后写入；notify 从这里取）。
static TRAY_HWND: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

fn main_window() -> Hwnd {
    let class = wide("Zed::Window");
    let title = wide("Mode Chat");
    // SAFETY: 两个指针都指向 NUL 结尾的宽字符串。
    unsafe { FindWindowW(class.as_ptr(), title.as_ptr()) }
}

fn load_icon() -> Hicon {
    let path = crate::instance::state_dir().join("app.ico");
    // 总是覆盖，避免沿用上一代图标的缓存文件。
    let _ = std::fs::write(&path, crate::window_style::icon_bytes());
    let wide_path = wide(&path.to_string_lossy());
    // SAFETY: 路径是 NUL 结尾宽字符串；失败返回空指针（下面判空）。
    unsafe {
        LoadImageW(
            std::ptr::null_mut(),
            wide_path.as_ptr(),
            IMAGE_ICON,
            0,
            0,
            LR_LOADFROMFILE | LR_DEFAULTSIZE,
        )
    }
}

unsafe extern "system" fn tray_wnd_proc(
    hwnd: Hwnd,
    message: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    if message == WM_TRAY {
        let event = (lparam as usize) & 0xffff;
        if event == WM_LBUTTONUP {
            let main = main_window();
            if !main.is_null() {
                // SAFETY: main 是 FindWindowW 找到的真实窗口句柄。
                unsafe {
                    ShowWindow(main, SW_RESTORE);
                    ShowWindow(main, SW_SHOW);
                    SetForegroundWindow(main);
                }
            }
            return 0;
        }
        if event == WM_RBUTTONUP {
            let main = main_window();
            let show = wide("显示主窗口");
            let quit = wide("退出");
            // SAFETY: 菜单/句柄都由本线程创建，立即销毁；菜单命令用 TPM_RETURNCMD 同步取回。
            unsafe {
                let menu = CreatePopupMenu();
                if menu.is_null() {
                    return 0;
                }
                AppendMenuW(menu, MF_STRING, 1, show.as_ptr());
                AppendMenuW(menu, MF_STRING, 2, quit.as_ptr());
                let mut pt = Point::default();
                GetCursorPos(&mut pt);
                // 菜单要把 hwnd 设到前台才收得到命令（经典托盘写法）。
                SetForegroundWindow(hwnd);
                let command = TrackPopupMenu(
                    menu,
                    TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY,
                    pt.x,
                    pt.y,
                    0,
                    hwnd,
                    std::ptr::null(),
                );
                let _ = PostMessageW(hwnd, WM_NULL, 0, 0);
                DestroyMenu(menu);
                if command == 1 && !main.is_null() {
                    ShowWindow(main, SW_RESTORE);
                    ShowWindow(main, SW_SHOW);
                    SetForegroundWindow(main);
                } else if command == 2 && !main.is_null() {
                    PostMessageW(main, WM_CLOSE, 0, 0);
                }
            }
            return 0;
        }
        return 0;
    }
    // SAFETY: 其余消息交给系统默认处理。
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

/// 安装托盘图标（只装一次；失败返回 false，不影响主窗口）。
pub fn install() -> bool {
    let hicon = SendHandle(load_icon());
    if hicon.0.is_null() {
        eprintln!("[tray] 图标加载失败，跳过托盘");
        return false;
    }
    std::thread::spawn(move || {
        let hicon = hicon.take();
        let class_name = wide("ModeBotPilotTray");
        let title = wide("Mode Chat Tray");
        // SAFETY: 类名/标题都是 NUL 结尾宽字符串；窗口从不 ShowWindow，不会上任务栏。
        unsafe {
            let instance = GetModuleHandleW(std::ptr::null());
            let class = WndClassW {
                style: 0,
                wnd_proc: Some(tray_wnd_proc),
                cls_extra: 0,
                wnd_extra: 0,
                instance,
                icon: hicon,
                cursor: std::ptr::null_mut(),
                background: std::ptr::null_mut(),
                menu_name: std::ptr::null(),
                class_name: class_name.as_ptr(),
            };
            if RegisterClassW(&class) == 0 {
                eprintln!("[tray] RegisterClassW 失败");
                return;
            }
            let hwnd = CreateWindowExW(
                0,
                class_name.as_ptr(),
                title.as_ptr(),
                0,
                0,
                0,
                0,
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                instance,
                std::ptr::null_mut(),
            );
            if hwnd.is_null() {
                eprintln!("[tray] 创建托盘窗口失败");
                return;
            }
            let mut tip = [0u16; 128];
            for (slot, ch) in tip.iter_mut().zip("Mode Chat · 点击唤起".encode_utf16()) {
                *slot = ch;
            }
            let data = NotifyIconDataW {
                cb_size: std::mem::size_of::<NotifyIconDataW>() as u32,
                hwnd,
                uid: 1,
                flags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
                callback_message: WM_TRAY,
                hicon,
                tip,
                state: 0,
                state_mask: 0,
                info: [0; 256],
                timeout_or_version: 0,
                info_title: [0; 64],
                info_flags: 0,
                guid_item: [0; 16],
                balloon_icon: std::ptr::null_mut(),
            };
            if Shell_NotifyIconW(NIM_ADD, &data) == 0 {
                eprintln!("[tray] Shell_NotifyIconW(NIM_ADD) 失败");
                return;
            }
            TRAY_HWND.store(hwnd, Ordering::Relaxed);
            let mut msg = Msg::default();
            while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    });
    true
}


/// 关闭按钮：隐藏主窗口而不是退出（托盘仍可唤起）。
pub fn hide_main_window() {
    let main = main_window();
    if !main.is_null() {
        // SAFETY: main 是 FindWindowW 找到的真实窗口句柄。
        unsafe {
            ShowWindow(main, SW_HIDE);
        }
    }
}

/// 主窗口当前是否在前台（后台时审批/提问才弹系统通知）。
pub fn main_foreground() -> bool {
    let main = main_window();
    // SAFETY: 纯查询；句柄来自 main_window()。
    !main.is_null() && unsafe { GetForegroundWindow() == main }
}

fn copy_wide<const N: usize>(dest: &mut [u16; N], text: &str) {
    for (slot, ch) in dest
        .iter_mut()
        .zip(text.encode_utf16().take(N.saturating_sub(1)))
    {
        *slot = ch;
    }
}

/// 托盘气泡通知（后台审批/提问用；标题最多 63 字、正文最多 255 字）。
pub fn notify(title: &str, body: &str) {
    let hwnd = TRAY_HWND.load(Ordering::Relaxed);
    if hwnd.is_null() {
        return;
    }
    let mut info = [0u16; 256];
    let mut info_title = [0u16; 64];
    copy_wide(&mut info, body);
    copy_wide(&mut info_title, title);
    let data = NotifyIconDataW {
        cb_size: std::mem::size_of::<NotifyIconDataW>() as u32,
        hwnd,
        uid: 1,
        flags: NIF_INFO,
        callback_message: WM_TRAY,
        hicon: std::ptr::null_mut(),
        tip: [0; 128],
        state: 0,
        state_mask: 0,
        info,
        timeout_or_version: 5_000,
        info_title,
        info_flags: NIIF_INFO,
        guid_item: [0; 16],
        balloon_icon: std::ptr::null_mut(),
    };
    // SAFETY: data 的 hwnd 是 install 线程登记的真实托盘窗口。
    unsafe {
        Shell_NotifyIconW(NIM_MODIFY, &data);
    }
}
