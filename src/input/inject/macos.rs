//! macOS 输入注入（键盘 + 鼠标），基于 Core Graphics `CGEvent` / `CGEventPost`。
//!
//! 捕获端发来的 `InputEvent` 在此重建为原生 `CGEvent` 并 post 到 HID 事件流。
//! 需要「辅助功能 / 输入监控」授权，否则 `CGEventCreate*` 在权限不足时返回 NULL
//!（这里会返回 `Err` 并 log）。
//!
//! 注意：本模块仅在 `cfg(target_os = "macos")` 下编译；沙箱无 Mac，仅做类型检查。

use std::sync::Mutex;

use anyhow::{anyhow, Result};
use core_graphics::event::{CGEvent, CGEventTapLocation, CGEventType, CGKeyCode, CGMouseButton};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::CGPoint;

use crate::input::event::{InputEvent, KeyState, MouseButton};
use crate::input::keycodes;

/// 客户端本地光标位置（由收到的相对位移累积）。绝对坐标用于 button 事件定位。
static CURSOR: Mutex<CGPoint> = Mutex::new(CGPoint { x: 0.0, y: 0.0 });

/// 创建事件源（每次使用新建，避免 move 问题；开销极小）。
fn make_source() -> Result<CGEventSource> {
    CGEventSource::new(CGEventSourceStateID::CombinedSessionState)
        .map_err(|_| anyhow!("macOS: failed to create CGEventSource (需要辅助功能/输入监控授权)"))
}

/// 注入一条输入事件。返回 `Err` 表示系统调用失败（通常是权限不足）。
pub fn inject_event(ev: InputEvent) -> Result<()> {
    match ev {
        InputEvent::Key(k) => {
            let kc = keycodes::mac::hid_to_mac_keycode(k.hid)
                .ok_or_else(|| anyhow!("inject: unknown HID key 0x{:04X}", k.hid))?;
            let source = make_source()?;
            let cg = CGEvent::new_keyboard_event(source, kc as CGKeyCode, matches!(k.state, KeyState::Pressed))
                .map_err(|_| anyhow!("macOS: CGEventCreateKeyboardEvent failed (权限?)"))?;
            cg.post(CGEventTapLocation::HID);
            Ok(())
        }
        InputEvent::Mouse(m) => {
            // 鼠标按键（按下 / 释放）
            if let (Some(btn), Some(st)) = (m.button, m.state) {
                let (mt, mb) = match (btn, st) {
                    (MouseButton::Left, KeyState::Pressed) => (CGEventType::LeftMouseDown, CGMouseButton::Left),
                    (MouseButton::Left, KeyState::Released) => (CGEventType::LeftMouseUp, CGMouseButton::Left),
                    (MouseButton::Right, KeyState::Pressed) => (CGEventType::RightMouseDown, CGMouseButton::Right),
                    (MouseButton::Right, KeyState::Released) => (CGEventType::RightMouseUp, CGMouseButton::Right),
                    (MouseButton::Middle, KeyState::Pressed) => (CGEventType::OtherMouseDown, CGMouseButton::Center),
                    (MouseButton::Middle, KeyState::Released) => (CGEventType::OtherMouseUp, CGMouseButton::Center),
                };
                let pos = *CURSOR.lock().unwrap();
                let source = make_source()?;
                let cg = CGEvent::new_mouse_event(source, mt, pos, mb)
                    .map_err(|_| anyhow!("macOS: CGEventCreateMouseEvent failed (权限?)"))?;
                cg.post(CGEventTapLocation::HID);
            }

            // 相对位移（累积到本地光标位置后再以绝对坐标 post）
            if m.dx != 0 || m.dy != 0 {
                let mut pos = CURSOR.lock().unwrap();
                pos.x += m.dx as f64;
                pos.y += m.dy as f64;
                let source = make_source()?;
                let cg = CGEvent::new_mouse_event(source, CGEventType::MouseMoved, *pos, CGMouseButton::Left)
                    .map_err(|_| anyhow!("macOS: CGEventCreateMouseEvent (move) failed (权限?)"))?;
                cg.post(CGEventTapLocation::HID);
            }
            Ok(())
        }
    }
}
