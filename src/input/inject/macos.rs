//! macOS 输入注入（键盘 + 鼠标），基于 Core Graphics `CGEvent` / `CGEventPost`。
//!
//! 捕获端发来的 `InputEvent` 在此重建为原生 `CGEvent` 并 post 到 HID 事件流。
//! 需要「辅助功能 / 输入监控」授权，否则 `CGEventCreate*` 在权限不足时返回 NULL
//!（这里会返回 `Err` 并 log）。
//!
//! 注意：本模块仅在 `cfg(target_os = "macos")` 下编译；沙箱无 Mac，仅做类型检查。

use std::ffi::c_void;
use std::sync::Mutex;

use anyhow::{anyhow, Result};
use core_graphics::display::CGDisplay;
use core_graphics::event::{CGEvent, CGEventTapLocation, CGEventType, CGKeyCode, CGMouseButton};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::CGPoint;

use crate::input::event::{InputEvent, KeyState, MouseButton};
use crate::input::keycodes;

/// 客户端本地光标位置（由收到的相对位移累积）。绝对坐标用于 button 事件定位。
static CURSOR: Mutex<CGPoint> = Mutex::new(CGPoint { x: 0.0, y: 0.0 });

/// M4：当前光标是否显示在 Mac 上。`false` 时 inject_event 仍会累计位置但不实际
/// 投递鼠标移动（避免在 Mac 隐藏光标的情况下还动它）。
static CURSOR_SHOWN: Mutex<bool> = Mutex::new(false);

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
                // M4：光标在 Mac 区域时才投递鼠标移动事件（光标隐藏时不投递）
                let shown = *CURSOR_SHOWN.lock().unwrap();
                if shown {
                    let source = make_source()?;
                    let cg = CGEvent::new_mouse_event(source, CGEventType::MouseMoved, *pos, CGMouseButton::Left)
                        .map_err(|_| anyhow!("macOS: CGEventCreateMouseEvent (move) failed (权限?)"))?;
                    cg.post(CGEventTapLocation::HID);
                }
            }
            Ok(())
        }
    }
}

// ---- M4：通过 Cocoa NSCursor 控制光标显隐 ----
//
// 与 screen.rs 里读 [NSEvent mouseLocation] 同样的 objc_msgSend 套路。
// 这里只发无返回值的消息（hide/unhide），不需要 ABI 复杂的返回类型。

#[link(name = "AppKit", kind = "framework")]
extern "C" {
    fn objc_getClass(name: *const u8) -> *const c_void;
    fn sel_registerName(name: *const u8) -> *const c_void;
    fn objc_msgSend(receiver: *const c_void, sel: *const c_void);
}

unsafe fn set_nscursor_hidden(hidden: bool) {
    let cls = objc_getClass(b"NSCursor\0".as_ptr());
    let sel = sel_registerName(if hidden { b"hide\0".as_ptr() } else { b"unhide\0".as_ptr() });
    objc_msgSend(cls, sel);
}

/// M4 无缝单光标：处理 Win server 发来的 `CursorState`。
///
/// - `on_mac=true`：在 Mac 端通过 `CGEventPost(MouseMoved)` 把光标移到 `(x, y)` 并显示；
/// - `on_mac=false`：系统级隐藏 Mac 光标（光标在 Win 区域时）。
///
/// **为什么不用 `CGWarpMouseCursorPosition`：** 该 API 在 macOS 上需要「输入监控」TCC
/// 授权，未授权时**静默失败**（不报错），会造成「Mac 端 log 显示收到坐标但屏幕光标
/// 不动」的诡异现象。改用 `CGEventPost(MouseMoved)` 走和键盘/按键同样的通路（只需
/// 辅助功能授权），并同步更新本地 `CURSOR` 让后续 button 事件在正确位置投递。
///
/// 显示/隐藏也改用 `CGDisplayShowCursor` / `CGDisplayHideCursor`（系统级、不需要
/// TCC），不再用 `NSCursor hide/unhide`（其作用范围仅限当前 app，对跨 app 全屏显示
/// 不可靠）。
pub fn handle_cursor_state(on_mac: bool, x: u32, y: u32) -> Result<()> {
    *CURSOR_SHOWN.lock().unwrap() = on_mac;
    if on_mac {
        let p = CGPoint { x: x as f64, y: y as f64 };
        // 更新本地累积位置，让后续 button 事件在正确位置投递
        *CURSOR.lock().unwrap() = p;
        // 强制移动 Mac 光标：post 一个 MouseMoved 事件到 HID 系统事件流
        // （CGEventPost 需要辅助功能权限；现在和正常 inject 路径一致）
        let source = make_source()?;
        let cg = CGEvent::new_mouse_event(source, CGEventType::MouseMoved, p, CGMouseButton::Left)
            .map_err(|_| anyhow!("macOS: CGEventCreateMouseEvent(MouseMoved) failed (权限?)"))?;
        cg.post(CGEventTapLocation::HID);
        // 系统级显示光标（CGDisplayShowCursor 是引用计数的、不需要 TCC）
        // 主显示器 id 即可——跨显示器会被 macOS 路由
        let _ = CGDisplay::main().show_cursor();
        log::trace!("m4 inject: cursor shown on Mac at ({}, {})", x, y);
    } else {
        // 系统级隐藏光标（CGDisplayHideCursor 是引用计数的、不需要 TCC）
        // 主显示器 id 即可——跨显示器会被 macOS 路由
        let _ = CGDisplay::main().hide_cursor();
        log::trace!("m4 inject: cursor hidden on Mac (in Win region)");
    }
    Ok(())
}
