//! macOS 输入注入（键盘 + 鼠标）。
//!
//! **Deskflow 源码分析（OSXScreen.mm）的结论**：
//! `CGEventCreateKeyboardEvent(nullptr, ...)` 和 `CGEventCreateMouseEvent(nullptr, ...)`
//! —— 用 **NULL source** 创建事件。NULL 事件源让系统把它视作**本地原生事件**而非合成事件，
//! 不会被 M5/Tahoe 的合成事件过滤器静默丢弃。我们的旧代码用 `CGEventSource::new(...)`
//! 创建特定源的事件——这就是被沙箱阻挡的根因。
//!
//! 本模块现在走两条路：① NULL-source CGEvent（主路径，Deskflow 验证）② osascript（备用）。
//!
//! 注意：本模块仅在 `cfg(target_os = "macos")` 下编译；沙箱无 Mac，仅做类型检查。

use std::ffi::c_void;
use std::process::Command;
use std::sync::Mutex;

use anyhow::{anyhow, Result};
use core_graphics::display::CGDisplay;
use core_graphics::event::{CGEventTapLocation, CGEventType, CGMouseButton};
use core_graphics::geometry::CGPoint;

use crate::input::event::{InputEvent, KeyState, MouseButton};
use crate::input::keycodes;

// Deskflow 的核心发现：用 NULL source 创建的事件被视为"本地原生事件"，
// 不受合成事件过滤器影响（M5/Tahoe 上 `CGEventPost` 静默失败的原因）。
//
// `source: *const c_void` 传 `std::ptr::null()` → 等价于 Deskflow 的 `CGEventCreateXxx(nullptr, ...)`。
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventCreateKeyboardEvent(
        source: *const c_void,
        virtualKey: u16,
        keyDown: bool,
    ) -> *mut c_void;

    fn CGEventCreateMouseEvent(
        source: *const c_void,
        mouseType: u32,
        mouseCursorPosition: CGPoint,
        mouseButton: u32,
    ) -> *mut c_void;

    fn CGEventPost(tap: u32, event: *mut c_void);
    fn CFRelease(cf: *mut c_void);
    fn CGWarpMouseCursorPosition(newCursorPosition: CGPoint) -> i32;
}

/// kCGHIDEventTap = 0（Deskflow 的 `CGEventPost(kCGHIDEventTap, ...)`）。
const K_CG_HID_EVENT_TAP: u32 = 0;

/// 客户端本地光标位置（由收到的相对位移累积）。绝对坐标用于 button 事件定位。
static CURSOR: Mutex<CGPoint> = Mutex::new(CGPoint { x: 0.0, y: 0.0 });

/// M4：当前光标是否显示在 Mac 上。
static CURSOR_SHOWN: Mutex<bool> = Mutex::new(false);

/// 通过 `osascript` + `System Events` 注入按键/点击。
/// macOS Tahoe/M5 上 `CGEventPost(HID)` 被安全层静默丢弃——这段绕过它。
fn osa(script: &str) {
    match Command::new("osascript").arg("-e").arg(script).output() {
        Ok(o) if o.status.success() => {}
        Ok(o) => log::warn!(
            "osascript fail: {} {}",
            String::from_utf8_lossy(&o.stdout).trim(),
            String::from_utf8_lossy(&o.stderr).trim(),
        ),
        Err(e) => log::warn!("osascript spawn fail: {:?}", e),
    }
}

/// 注入一条输入事件（Deskflow NULL-source CGEvent 主路径 + osascript 备用）。
pub fn inject_event(ev: InputEvent) -> Result<()> {
    unsafe {
        match ev {
            InputEvent::Key(k) => {
                let kc = keycodes::mac::hid_to_mac_keycode(k.hid)
                    .ok_or_else(|| anyhow!("inject: unknown HID key 0x{:04X}", k.hid))?;
                let pressed = matches!(k.state, KeyState::Pressed);
                // NULL-source CGEvent：系统视作原生事件（Deskflow 验证路径）
                let raw = CGEventCreateKeyboardEvent(std::ptr::null(), kc, pressed);
                if !raw.is_null() {
                    CGEventPost(K_CG_HID_EVENT_TAP, raw);
                    CFRelease(raw);
                    log::trace!("cgevent: Key hid=0x{:04X} kc={} {} → NULL-source+HID",
                        k.hid, kc, if pressed { "down" } else { "up" });
                } else {
                    log::warn!("cgevent: CGEventCreateKeyboardEvent returned NULL");
                }
                // osascript 备用
                if pressed {
                    let cmd = format!("tell application \"System Events\" to key code {}", kc);
                    osa(&cmd);
                }
                Ok(())
            }
            InputEvent::Mouse(m) => {
                // 鼠标按键
                if let (Some(btn), Some(st)) = (m.button, m.state) {
                    let pressed = matches!(st, KeyState::Pressed);
                    let pos = *CURSOR.lock().unwrap();
                    let (mtype, mbtn) = match (btn, pressed) {
                        (MouseButton::Left, true) => (1u32, 0u32),   // kCGEventLeftMouseDown, kCGMouseButtonLeft
                        (MouseButton::Left, false) => (2u32, 0u32),  // kCGEventLeftMouseUp
                        (MouseButton::Right, true) => (3u32, 1u32),  // kCGEventRightMouseDown, kCGMouseButtonRight
                        (MouseButton::Right, false) => (4u32, 1u32),
                        (MouseButton::Middle, true) => (25u32, 2u32), // kCGEventOtherMouseDown, kCGMouseButtonCenter
                        (MouseButton::Middle, false) => (26u32, 2u32),
                    };
                    let raw = CGEventCreateMouseEvent(std::ptr::null(), mtype, pos, mbtn);
                    if !raw.is_null() {
                        CGEventPost(K_CG_HID_EVENT_TAP, raw);
                        CFRelease(raw);
                        log::trace!("cgevent: Mouse {:?}/{:?} at ({:.0},{:.0}) → NULL-source+HID",
                            btn, st, pos.x, pos.y);
                    } else {
                        log::warn!("cgevent: CGEventCreateMouseEvent returned NULL");
                    }
                    // osascript 备用（click at）
                    if pressed {
                        let cmd = format!("tell application \"System Events\" to click at {{{}, {}}}",
                            pos.x, pos.y);
                        osa(&cmd);
                    }
                }

                // 相对位移（累积到本地光标位置）
                if m.dx != 0 || m.dy != 0 {
                    let mut pos = CURSOR.lock().unwrap();
                    pos.x += m.dx as f64;
                    pos.y += m.dy as f64;
                }
                Ok(())
            }
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
        *CURSOR.lock().unwrap() = p;
        unsafe {
            // Deskflow 的方法：CGWarpMouseCursorPosition + NULL-source MouseMoved
            CGWarpMouseCursorPosition(p);
            let raw = CGEventCreateMouseEvent(std::ptr::null(), 5u32, p, 0u32); // kCGEventMouseMoved=5, kCGMouseButtonLeft=0
            if !raw.is_null() {
                CGEventPost(K_CG_HID_EVENT_TAP, raw);
                CFRelease(raw);
            }
        }
        let _ = CGDisplay::main().show_cursor();
        log::trace!("m4: cursor shown on Mac at ({}, {})", x, y);
    } else {
        // 系统级隐藏光标（CGDisplayHideCursor 是引用计数的、不需要 TCC）
        // 主显示器 id 即可——跨显示器会被 macOS 路由
        let _ = CGDisplay::main().hide_cursor();
        log::trace!("m4 inject: cursor hidden on Mac (in Win region)");
    }
    Ok(())
}
