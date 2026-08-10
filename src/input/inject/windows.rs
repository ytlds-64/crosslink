//! Windows 键盘输入注入。
//!
//! M2.2 实现：使用官方 `SendInput` API + `KEYBDINPUT`。
//! 事件 → HID → Windows VK → `SendInput`。
//!
//! 不识别的 HID 键码（`keycodes::hid_to_vk` 返回 None）会被 log warn 并丢弃。

use anyhow::{anyhow, Result};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_KEYUP, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN,
    MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
    MOUSEINPUT, MOUSE_EVENT_FLAGS, VIRTUAL_KEY,
};

use crate::input::event::{InputEvent, KeyState, MouseButton};
use crate::input::keycodes;

/// 注入一条输入事件。返回 `Err` 表示系统调用失败。
pub fn inject(ev: InputEvent) -> Result<()> {
    match ev {
        InputEvent::Key(k) => {
            let vk = keycodes::hid_to_vk(k.hid)
                .ok_or_else(|| anyhow!("inject: unknown HID key 0x{:04X}", k.hid))?;
            log::info!("inject: hid=0x{:04X} -> VK 0x{:02X} state={:?}", k.hid, vk, k.state);

            let mut flags: KEYBD_EVENT_FLAGS = KEYBD_EVENT_FLAGS(0);
            if matches!(k.state, KeyState::Released) {
                flags |= KEYEVENTF_KEYUP;
            }

            let input = INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VIRTUAL_KEY(vk),
                        wScan: 0,
                        dwFlags: flags,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            };

            // cbSize 用 i32 是 SendInput 的 Win32 签名要求
            let sent = unsafe {
                SendInput(
                    &[input],
                    std::mem::size_of::<INPUT>() as i32,
                )
            };
            if sent == 0 {
                let err = std::io::Error::last_os_error();
                return Err(anyhow!("SendInput failed for VK 0x{:02X}: {}", vk, err));
            }
            Ok(())
        }
        InputEvent::Mouse(m) => {
            // 按键边沿（按下 / 释放）
            if let (Some(btn), Some(st)) = (m.button, m.state) {
                let (down, up) = match btn {
                    MouseButton::Left => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
                    MouseButton::Right => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
                    MouseButton::Middle => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
                };
                // 按下用 down 标志，释放用 up 标志（二者互斥，取其一即可）
                let flags: MOUSE_EVENT_FLAGS = if matches!(st, KeyState::Released) {
                    up
                } else {
                    down
                };
                let input = INPUT {
                    r#type: INPUT_MOUSE,
                    Anonymous: INPUT_0 {
                        mi: MOUSEINPUT {
                            dx: 0,
                            dy: 0,
                            mouseData: 0,
                            dwFlags: flags,
                            time: 0,
                            dwExtraInfo: 0,
                        },
                    },
                };
                let sent = unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) };
                if sent == 0 {
                    let err = std::io::Error::last_os_error();
                    return Err(anyhow!("SendInput (mouse button) failed: {}", err));
                }
                log::info!("inject: mouse button {:?} {:?}", btn, st);
            }

            // 相对位移（MOUSEEVENTF_MOVE 不带绝对标志 = 相对移动）
            if m.dx != 0 || m.dy != 0 {
                let input = INPUT {
                    r#type: INPUT_MOUSE,
                    Anonymous: INPUT_0 {
                        mi: MOUSEINPUT {
                            dx: m.dx as i32,
                            dy: m.dy as i32,
                            mouseData: 0,
                            dwFlags: MOUSEEVENTF_MOVE,
                            time: 0,
                            dwExtraInfo: 0,
                        },
                    },
                };
                let sent = unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) };
                if sent == 0 {
                    let err = std::io::Error::last_os_error();
                    return Err(anyhow!("SendInput (mouse move) failed: {}", err));
                }
                log::info!("inject: mouse move dx={} dy={}", m.dx, m.dy);
            }
            Ok(())
        }
    }
}

/// M4：Windows 端是 master（Win 控制 Mac），不会收到 `CursorState`（那是 Win 发给 Mac 的）。
/// 为 API 统一提供 no-op 占位；真正的实现只在 `inject/macos.rs`。
pub fn handle_cursor_state(_on_mac: bool, _x: u32, _y: u32) -> Result<()> {
    Ok(())
}
