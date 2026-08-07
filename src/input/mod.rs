//! 平台特定输入后端分发。
//!
//! - Windows：M2.2 键盘 + M2.3 鼠标，基于 `GetAsyncKeyState` / `GetCursorPos` 捕获
//!   与 `SendInput` 注入。
//! - macOS：M2.4 计划用 Core Graphics `CGEventTap` 捕获 + `CGEvent` 注入。
//! - 其它平台（含 Linux CI 构建）：提供 no-op 实现，保证 `cargo build` 通过，
//!   运行时不做任何输入转发（冒烟测试仅验证连通性与心跳）。

pub mod event;
pub mod keycodes;

#[cfg(target_os = "windows")]
pub mod capture {
    pub mod windows;
    pub use windows::start_capture;
}

#[cfg(target_os = "windows")]
pub mod inject {
    pub mod windows;
    pub use windows::inject as inject_event;
}

// macOS 捕获 / 注入（M2.4）
#[cfg(target_os = "macos")]
pub mod capture {
    pub mod macos;
    pub use macos::start_capture;
}

#[cfg(target_os = "macos")]
pub mod inject {
    pub mod macos;
    pub use macos::inject as inject_event;
}

// 非 Windows / 非 macOS 平台：no-op 实现，保证可编译与可被冒烟测试链接。
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub mod capture {
    use std::sync::mpsc::Receiver;

    use crate::input::event::InputEvent;

    pub fn start_capture() -> Receiver<InputEvent> {
        log::warn!("input capture: not implemented on this platform (no-op)");
        let (tx, rx) = std::sync::mpsc::channel();
        drop(tx); // 立即关闭，不会有任何事件产生
        rx
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub mod inject {
    use anyhow::Result;

    use crate::input::event::InputEvent;

    pub fn inject_event(_ev: InputEvent) -> Result<()> {
        // no-op：非目标平台不注入
        Ok(())
    }
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
