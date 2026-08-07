//! 平台特定输入后端分发。
//!
//! M2.2 实现 Windows 子集；macOS / Linux 在后续里程碑接入。

pub mod event;
pub mod keycodes;

#[cfg(target_os = "windows")]
pub mod capture {
    pub mod windows;
    pub use windows::start_keyboard_capture;
}

#[cfg(target_os = "windows")]
pub mod inject {
    pub mod windows;
    pub use windows::inject as inject_event;
}

/// 当前运行平台标识（用于 Hello 消息与后续平台相关分支）。
pub fn platform() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "unknown"
    }
}
