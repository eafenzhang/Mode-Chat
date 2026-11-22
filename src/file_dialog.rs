//! 原生文件选择（Win32 `GetOpenFileNameW`，不引第三方对话框依赖）。
//!
//! 只做「多选现有文件」这一件事：附件上限 20MiB 在调用方校验。对话框是模态的，
//! 直接在当前线程调用（原生对话框返回后 gpui 继续跑事件循环）。

#![cfg(windows)]

use std::ffi::c_void;

type Hwnd = *mut c_void;

const OFN_ALLOWMULTISELECT: u32 = 0x0000_0200;
const OFN_EXPLORER: u32 = 0x0008_0000;
const OFN_FILEMUSTEXIST: u32 = 0x0000_1000;
const OFN_PATHMUSTEXIST: u32 = 0x0000_0800;
const OFN_NOCHANGEDIR: u32 = 0x0000_0008;
const OFN_OVERWRITEPROMPT: u32 = 0x0000_0002;

/// `OPENFILENAMEW`（字段顺序必须与 SDK 完全一致，否则 GetOpenFileNameW 读错布局）。
#[repr(C)]
struct OpenFileNameW {
    struct_size: u32,
    owner: Hwnd,
    instance: *mut c_void,
    filter: *const u16,
    custom_filter: *mut u16,
    max_custom_filter: u32,
    filter_index: u32,
    file: *mut u16,
    max_file: u32,
    file_title: *mut u16,
    max_file_title: u32,
    initial_dir: *const u16,
    title: *const u16,
    flags: u32,
    file_offset: u16,
    file_extension: u16,
    default_extension: *const u16,
    custom_data: isize,
    hook: *mut c_void,
    template_name: *const u16,
    reserved_ptr: *mut c_void,
    reserved: u32,
    flags_ex: u32,
}

#[link(name = "comdlg32")]
unsafe extern "system" {
    fn GetOpenFileNameW(dialog: *mut OpenFileNameW) -> i32;
    fn GetSaveFileNameW(dialog: *mut OpenFileNameW) -> i32;
}

#[link(name = "user32")]
unsafe extern "system" {
    fn FindWindowW(class: *const u16, title: *const u16) -> Hwnd;
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 弹出文件选择框，返回选中的绝对路径（取消返回空）。
pub fn pick_files() -> Vec<String> {
    // 32KiB 缓冲：多选时是「目录\0文件1\0文件2\0\0」。
    let mut buffer = vec![0u16; 16 * 1024];
    let filter: Vec<u16> = "所有文件\0*.*\0图片与文档\0*.png;*.jpg;*.jpeg;*.gif;*.webp;*.pdf;*.md;*.txt\0"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let title = wide("选择附件");
    let class = wide("Zed::Window");
    let window_title = wide("Mode Chat");
    // SAFETY: 指针都指向本函数栈上/堆上、以 NUL 结尾的宽字符串或缓冲；缓冲足够大。
    unsafe {
        let owner = FindWindowW(class.as_ptr(), window_title.as_ptr());
        let mut dialog = OpenFileNameW {
            struct_size: std::mem::size_of::<OpenFileNameW>() as u32,
            owner,
            instance: std::ptr::null_mut(),
            filter: filter.as_ptr(),
            custom_filter: std::ptr::null_mut(),
            max_custom_filter: 0,
            filter_index: 1,
            file: buffer.as_mut_ptr(),
            max_file: buffer.len() as u32,
            file_title: std::ptr::null_mut(),
            max_file_title: 0,
            initial_dir: std::ptr::null(),
            title: title.as_ptr(),
            flags: OFN_ALLOWMULTISELECT
                | OFN_EXPLORER
                | OFN_FILEMUSTEXIST
                | OFN_PATHMUSTEXIST
                | OFN_NOCHANGEDIR,
            file_offset: 0,
            file_extension: 0,
            default_extension: std::ptr::null(),
            custom_data: 0,
            hook: std::ptr::null_mut(),
            template_name: std::ptr::null(),
            reserved_ptr: std::ptr::null_mut(),
            reserved: 0,
            flags_ex: 0,
        };
        if GetOpenFileNameW(&mut dialog) == 0 {
            return Vec::new();
        }
    }
    parse_multi(&buffer)
}

/// 导出 Markdown：弹出保存框，返回目标路径（取消返回 None）。
pub fn save_markdown(default_name: &str) -> Option<String> {
    let mut buffer = vec![0u16; 1024];
    let name: Vec<u16> = default_name.encode_utf16().collect();
    let copy = name.len().min(buffer.len() - 1);
    buffer[..copy].copy_from_slice(&name[..copy]);
    let filter: Vec<u16> = "Markdown\0*.md\0所有文件\0*.*\0"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let title = wide("导出会话");
    let class = wide("Zed::Window");
    let window_title = wide("Mode Chat");
    let extension = wide("md");
    // SAFETY: 指针都指向本函数栈上/堆上、以 NUL 结尾的宽字符串或缓冲；缓冲足够大。
    unsafe {
        let owner = FindWindowW(class.as_ptr(), window_title.as_ptr());
        let mut dialog = OpenFileNameW {
            struct_size: std::mem::size_of::<OpenFileNameW>() as u32,
            owner,
            instance: std::ptr::null_mut(),
            filter: filter.as_ptr(),
            custom_filter: std::ptr::null_mut(),
            max_custom_filter: 0,
            filter_index: 1,
            file: buffer.as_mut_ptr(),
            max_file: buffer.len() as u32,
            file_title: std::ptr::null_mut(),
            max_file_title: 0,
            initial_dir: std::ptr::null(),
            title: title.as_ptr(),
            flags: OFN_EXPLORER
                | OFN_PATHMUSTEXIST
                | OFN_OVERWRITEPROMPT
                | OFN_NOCHANGEDIR,
            file_offset: 0,
            file_extension: 0,
            default_extension: extension.as_ptr(),
            custom_data: 0,
            hook: std::ptr::null_mut(),
            template_name: std::ptr::null(),
            reserved_ptr: std::ptr::null_mut(),
            reserved: 0,
            flags_ex: 0,
        };
        if GetSaveFileNameW(&mut dialog) == 0 {
            return None;
        }
    }
    let end = buffer.iter().position(|value| *value == 0).unwrap_or(0);
    if end == 0 {
        return None;
    }
    let mut path = String::from_utf16_lossy(&buffer[..end]);
    if std::path::Path::new(&path).extension().is_none() {
        path.push_str(".md");
    }
    Some(path)
}

/// 解析 `GetOpenFileNameW` 的多选缓冲：目录 + 若干文件名；单选时就是一整条路径。
fn parse_multi(buffer: &[u16]) -> Vec<String> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    for index in 0..buffer.len() {
        if buffer[index] == 0 {
            if index == start {
                break;
            }
            parts.push(String::from_utf16_lossy(&buffer[start..index]));
            start = index + 1;
        }
    }
    if parts.is_empty() {
        return Vec::new();
    }
    if parts.len() == 1 {
        return parts;
    }
    let directory = parts[0].clone();
    parts[1..]
        .iter()
        .map(|name| {
            std::path::Path::new(&directory)
                .join(name)
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

/// 常见 MIME（按扩展名；未知回落 octet-stream）。
pub fn mime_for(path: &str) -> &'static str {
    let extension = std::path::Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        "md" => "text/markdown",
        "txt" => "text/plain",
        "json" => "application/json",
        "csv" => "text/csv",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}
