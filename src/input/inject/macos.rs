//! macOS 输入注入（键盘 + 鼠标），基于 Core Graphics `CGEvent` / `CGEventPost`
//! + `osascript`（System Events）双通道。
//!
//! M5/Tahoe 上 `CGEventPost` 对键盘/鼠标点击可能被系统安全层静默丢弃；
//! `osascript` 走 AppleScript `key code` / `click at` 绕过这个限制。
//! 需要「辅助功能 / 输入监控」授权（`System Events` 依赖它）。
//!
//! 注意：本模块仅在 `cfg(target_os = "macos")` 下编译；沙箱无 Mac，仅做类型检查。

use std::ffi::c_void;
use std::process::Command;
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
/// M5/Tahoe 上 `CombinedSessionState` 产生的合成事件可能被系统沙箱拦截；
/// `HIDSystemState` 生成的 HID 系统事件被视为“真实硬件外设”，能绕过更严的过滤。
fn make_source() -> Result<CGEventSource> {
    CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow!("macOS: failed to create CGEventSource (需要辅助功能/输入监控授权)"))
}

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

/// 注入一条输入事件。返回 `Err` 表示系统调用失败（通常是权限不足）。
/// 按键 / 鼠标点击走 `osascript`（`key code` / `click at`），光标移动保留 CGEventPost。
pub fn inject_event(ev: InputEvent) -> Result<()> {
    match ev {
        InputEvent::Key(k) => {
            let kc = keycodes::mac::hid_to_mac_keycode(k.hid)
                .ok_or_else(|| anyhow!("inject: unknown HID key 0x{:04X}", k.hid))?;
            if matches!(k.state, KeyState::Pressed) {
                // key code <N> 在 AppleScript 里做一次完整击键（down+up），所以
                // 只响应 Pressed、忽略 Released。这样也自然处理了按住不放的重复。
                let cmd = format!("tell application \"System Events\" to key code {}", kc);
                log::trace!("osa: {} (mac_keycode={})", cmd, kc);
                osa(&cmd);
            }
            Ok(())
        }
        InputEvent::Mouse(m) => {
            // 鼠标按键 — 用 osascript click at {X,Y}（会自动在最前面窗口点击）
            if let (Some(_btn), Some(st)) = (m.button, m.state) {
                if matches!(st, KeyState::Pressed) {
                    let pos = *CURSOR.lock().unwrap();
                    let cmd = format!(
                        "tell application \"System Events\" to click at {{{}, {}}}",
                        pos.x, pos.y
                    );
                    log::trace!("osa: click at ({}, {})", pos.x, pos.y);
                    osa(&cmd);
                }
            }

            // 相对位移（累积到本地光标位置；M4 期间不作为独立事件投递）
            if m.dx != 0 || m.dy != 0 {
                let mut pos = CURSOR.lock().unwrap();
                pos.x += m.dx as f64;
                pos.y += m.dy as f64;
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
        *CURSOR.lock().unwrap() = p;
        // CGEventPost + osascript 双路：CGEventPost 可能在 Tahoe 上被弃，再补一条 osascript
        let source = make_source()?;
        let cg = CGEvent::new_mouse_event(source, CGEventType::MouseMoved, p, CGMouseButton::Left)
            .map_err(|_| anyhow!("macOS: CGEventCreateMouseEvent(MouseMoved) failed (权限?)"))?;
        cg.post(CGEventTapLocation::HID);
        // osascript 兜底路径上轮被 `mouse 没有定义` 失败——AppleScript 里
        // `System Events` 没有可写的 `mouse` 属性。光标定位保留 CGEventPost，
        // 不在这里再调 osascript。
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
