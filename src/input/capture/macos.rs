//! macOS 输入捕获（键盘 + 鼠标），基于 Core Graphics `CGEventTap`。
//!
//! 通过 `CGEventTap::new` 在事件流上安装一个 tap，回调里读取按键 / 鼠标事件，
//! 经 std mpsc 转发给 tokio 桥。tap 需要「辅助功能 / 输入监控」授权（TCC），
//! 否则 `CGEventTapCreate` 返回 NULL（`CGEventTap::new` 返回 `Err`），此时仅 log 错误。
//!
//! 设计：tap 为 **被动监听**（Passive / ListenOnly 也行，这里用 Default 但回调返回
//! `None` 始终透传事件，不拦截输入）。修饰键在 macOS 上产生 `FlagsChanged` 而非
//! KeyDown/KeyUp，需对比前后 flags 推断按下/释放。
//!
//! 注意：本模块仅在 `cfg(target_os = "macos")` 下编译；沙箱无 Mac，仅做类型检查。

use std::cell::Cell;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use core_foundation::runloop::{CFRunLoop, kCFRunLoopCommonModes};
use core_graphics::event::{
    CGEvent, CGEventTap, CGEventTapLocation, CGEventTapPlacement, CGEventTapOptions, CGEventType,
    EventField,
};

use crate::input::event::{InputEvent, KeyEvent, KeyState, MouseButton, MouseEvent};
use crate::input::keycodes;

// 跟踪上一次的修饰键 flags（用于从 `FlagsChanged` 事件推断按下/释放）。
thread_local! {
    static PREV_FLAGS: Cell<u64> = const { Cell::new(0) };
}

/// macOS 修饰键 keycode → 对应 `CGEventFlag` 位（与 kCGEventFlagMask* 一致）。
fn flag_bit_for(kc: u16) -> Option<u64> {
    match kc {
        0x37 => Some(0x100000), // Command
        0x3A => Some(0x80000),  // Option / Alt
        0x3B => Some(0x40000),  // Control
        0x38 | 0x3C => Some(0x20000), // Shift（左右共用同一 flag 位）
        0x39 => Some(0x10000),  // Caps Lock
        _ => None,
    }
}

/// 启动 macOS 输入捕获，返回事件 channel 的接收端。
pub fn start_capture() -> Receiver<InputEvent> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || run_capture(tx));
    rx
}

fn run_capture(tx: Sender<InputEvent>) {
    log::info!("input capture: starting macOS CGEventTap");

    let events_of_interest = vec![
        CGEventType::KeyDown,
        CGEventType::KeyUp,
        CGEventType::FlagsChanged,
        CGEventType::LeftMouseDown,
        CGEventType::LeftMouseUp,
        CGEventType::RightMouseDown,
        CGEventType::RightMouseUp,
        CGEventType::OtherMouseDown,
        CGEventType::OtherMouseUp,
        CGEventType::MouseMoved,
        CGEventType::LeftMouseDragged,
        CGEventType::RightMouseDragged,
        CGEventType::OtherMouseDragged,
    ];

    let tap = match CGEventTap::new(
        CGEventTapLocation::HID,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::Default,
        events_of_interest,
        move |_proxy, event_type, event| {
            translate(event_type, event, &tx);
            None // 始终透传原始事件，不拦截
        },
    ) {
        Ok(tap) => tap,
        Err(_) => {
            log::error!(
                "input capture: CGEventTapCreate failed (需要「辅助功能 / 输入监控」授权；在「系统设置 → 隐私与安全性」中允许本程序)"
            );
            return;
        }
    };

    let current = CFRunLoop::get_current();
    let loop_source = match tap.mach_port.create_runloop_source(0) {
        Ok(src) => src,
        Err(_) => {
            log::error!("input capture: failed to create run loop source");
            return;
        }
    };
    current.add_source(&loop_source, unsafe { kCFRunLoopCommonModes });
    tap.enable();
    log::info!("input capture: CGEventTap enabled, entering run loop");
    CFRunLoop::run_current();
}

/// 把一条 CGEvent 翻译为内部 `InputEvent` 并发送；返回 `None` 由上层透传。
fn translate(et: CGEventType, ev: &CGEvent, tx: &Sender<InputEvent>) {
    match et {
        CGEventType::KeyDown | CGEventType::KeyUp => {
            let kc = ev.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
            if let Some(hid) = keycodes::mac::mac_keycode_to_hid(kc) {
                let state = match et {
                    CGEventType::KeyDown => KeyState::Pressed,
                    _ => KeyState::Released, // 本分支只可能是 KeyUp
                };
                let _ = tx.send(InputEvent::Key(KeyEvent { hid, state }));
            }
        }
        CGEventType::FlagsChanged => {
            let kc = ev.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
            if let (Some(hid), Some(bit)) = (keycodes::mac::mac_keycode_to_hid(kc), flag_bit_for(kc)) {
                let prev = PREV_FLAGS.with(|p| p.get());
                let now: u64 = ev.get_flags().bits();
                let was_down = (prev & bit) != 0;
                let is_down = (now & bit) != 0;
                if is_down != was_down {
                    let _ = tx.send(InputEvent::Key(KeyEvent {
                        hid,
                        state: if is_down {
                            KeyState::Pressed
                        } else {
                            KeyState::Released
                        },
                    }));
                }
                PREV_FLAGS.with(|p| p.set(now));
            }
        }
        CGEventType::LeftMouseDown
        | CGEventType::LeftMouseUp
        | CGEventType::RightMouseDown
        | CGEventType::RightMouseUp
        | CGEventType::OtherMouseDown
        | CGEventType::OtherMouseUp => {
            let btn = match et {
                CGEventType::LeftMouseDown | CGEventType::LeftMouseUp => MouseButton::Left,
                CGEventType::RightMouseDown | CGEventType::RightMouseUp => MouseButton::Right,
                _ => MouseButton::Middle,
            };
            let state = match et {
                CGEventType::LeftMouseDown
                | CGEventType::RightMouseDown
                | CGEventType::OtherMouseDown => KeyState::Pressed,
                _ => KeyState::Released,
            };
            let _ = tx.send(InputEvent::Mouse(MouseEvent {
                dx: 0,
                dy: 0,
                button: Some(btn),
                state: Some(state),
            }));
        }
        CGEventType::MouseMoved
        | CGEventType::LeftMouseDragged
        | CGEventType::RightMouseDragged
        | CGEventType::OtherMouseDragged => {
            let dx = ev.get_integer_value_field(EventField::MOUSE_EVENT_DELTA_X) as i16;
            let dy = ev.get_integer_value_field(EventField::MOUSE_EVENT_DELTA_Y) as i16;
            if dx != 0 || dy != 0 {
                let _ = tx.send(InputEvent::Mouse(MouseEvent {
                    dx,
                    dy,
                    button: None,
                    state: None,
                }));
            }
        }
        _ => {}
    }
}
