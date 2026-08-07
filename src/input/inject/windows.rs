//! Windows 键盘输入注入。
//!
//! M2.2 实现：使用官方 `SendInput` API + `KEYBDINPUT`。
//! 事件 → HID → Windows VK → `SendInput`。
//!
//! 不识别的 HID 键码（`keycodes::hid_to_vk` 返回 None）会被 log warn 并丢弃。

use anyhow::{anyhow, Result};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
    VIRTUAL_KEY,
};

use crate::input::event::{InputEvent, KeyState};
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
    }
}
