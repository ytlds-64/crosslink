//! 平台特定输入后端分发。
//!
//! - Windows：M2.2 键盘 + M2.3 鼠标，基于 `GetAsyncKeyState` / `GetCursorPos` 捕获
//!   与 `SendInput` 注入。
//! - macOS：M2.4 基于 Core Graphics `CGEventTap` 捕获 + `CGEvent` 注入（需 TCC 授权）。
//! - 其它平台（含 Linux CI 构建）：提供 no-op 实现，保证 `cargo build` 通过，
//!   运行时不做任何输入转发（冒烟测试仅验证连通性与心跳）。

pub mod event;
pub mod keycodes;
pub mod screen;

use std::sync::atomic::AtomicU32;
use std::sync::Arc;

/// 捕获线程发出的消息（M2 输入事件 + M4 逻辑光标状态）。
///
/// M2 仅产生 `Input`；M4 还会产生 `CursorState` 用于通知 Mac 端光标应在哪显示。
#[derive(Debug, Clone, Copy)]
pub enum CaptureMsg {
    Input(event::InputEvent),
    CursorState { on_mac: bool, x: u32, y: u32 },
}

/// 启动捕获时的配置（屏幕几何 + 模式开关）。
///
/// M2 只用 `win_w/h`（M4 预留）；M4 用全部字段。
/// `mac_w/h` 是 `Arc<AtomicU32>`：初始 0，transport 收到对端 Hello 后写入。
/// 这样 capture 线程每次 poll 都能读到最新值，无需重启捕获。
#[derive(Clone)]
pub struct CaptureOptions {
    pub win_w: u32,
    pub win_h: u32,
    pub mac_w: Arc<AtomicU32>,
    pub mac_h: Arc<AtomicU32>,
    pub m4_mode: bool,
}

#[cfg(target_os = "windows")]
pub mod capture {
    pub mod windows;
    pub use windows::start_capture;
}

#[cfg(target_os = "windows")]
pub mod inject {
    pub mod windows;
    pub use windows::{handle_cursor_state, inject as inject_event};
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
    pub use macos::{handle_cursor_state, inject_event};
}

// 非 Windows / 非 macOS 平台：no-op 实现，保证可编译与可被冒烟测试链接。
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub mod capture {
    use std::sync::mpsc::Receiver;

    use crate::input::{CaptureMsg, CaptureOptions};

    pub fn start_capture(_opts: CaptureOptions) -> Receiver<CaptureMsg> {
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

    pub fn handle_cursor_state(_on_mac: bool, _x: u32, _y: u32) -> Result<()> {
        // no-op：非目标平台
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
