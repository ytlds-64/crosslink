//! Windows 输入捕获（键盘 + 鼠标）。
//!
//! M2.2 实现键盘：基于 `GetAsyncKeyState` 的 5ms 轮询版本。
//! M2.3 加入鼠标：按键用 `GetAsyncKeyState`（VK_LBUTTON/RBUTTON/MBUTTON），
//! 相对位移用 `GetCursorPos` 计算两帧之间的差值。
//!
//! 局限（后续升级 Raw Input 时解决）：
//! - 当焦点切换到低权限窗口时，0x8000 位可能滞后一个轮询周期；
//! - 同一时刻 0x0001（"曾按下"）位会被忽略——但 0x8000 已足够区分 press/release 转换。
//!
//! Raw Input 升级要点：HWND_MESSAGE 隐藏窗口 + WM_INPUT 消息循环 + RID_INPUT，
//! 能拿到 MakeCode 精确区分左右修饰键与 E0/E1 扩展键，对游戏兼容性更好。

use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use windows::Win32::Foundation::POINT;
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

use crate::input::event::{InputEvent, KeyEvent, KeyState, MouseButton, MouseEvent};
use crate::input::keycodes;

/// 启动输入捕获（键盘 + 鼠标），返回事件 channel 的接收端。
///
/// 调用方在独立线程（`tokio::task::spawn_blocking`）里调用 `.recv()` 即可。
pub fn start_capture() -> Receiver<InputEvent> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || run_capture_loop(tx));
    rx
}

fn run_capture_loop(tx: mpsc::Sender<InputEvent>) {
    log::info!("input capture: started (Windows GetAsyncKeyState + GetCursorPos, 5ms poll)");
    let mut prev_keys: [bool; 256] = [false; 256];
    // 鼠标按键：0=Left, 1=Right, 2=Middle
    let mut prev_btn = [false; 3];
    let mut last_pos = POINT { x: 0, y: 0 };
    let mut have_pos = false;

    loop {
        // ---- 键盘 ----
        for vk in 0u16..256 {
            // GetAsyncKeyState 返回 SHORT(i16)；最高位(0x8000)= 当前是否按下
            let state = unsafe { GetAsyncKeyState(vk as i32) };
            let down = (state as u16 & 0x8000) != 0;
            let idx = vk as usize;
            if down != prev_keys[idx] {
                prev_keys[idx] = down;
                if let Some(hid) = keycodes::vk_to_hid(vk) {
                    let ev = InputEvent::Key(KeyEvent {
                        hid,
                        state: if down {
                            KeyState::Pressed
                        } else {
                            KeyState::Released
                        },
                    });
                    if tx.send(ev).is_err() {
                        log::info!("input capture: receiver dropped, stopping");
                        return;
                    }
                } else if down {
                    log::trace!("input capture: unmapped VK 0x{:02X} (release skipped)", vk);
                }
            }
        }

        // ---- 鼠标按键 ----
        const BTN_VK: [i32; 3] = [
            0x01, // VK_LBUTTON
            0x02, // VK_RBUTTON
            0x04, // VK_MBUTTON
        ];
        for (i, &vk) in BTN_VK.iter().enumerate() {
            let state = unsafe { GetAsyncKeyState(vk) };
            let down = (state as u16 & 0x8000) != 0;
            if down != prev_btn[i] {
                prev_btn[i] = down;
                let button = match i {
                    0 => MouseButton::Left,
                    1 => MouseButton::Right,
                    _ => MouseButton::Middle,
                };
                let ev = InputEvent::Mouse(MouseEvent {
                    dx: 0,
                    dy: 0,
                    button: Some(button),
                    state: Some(if down {
                        KeyState::Pressed
                    } else {
                        KeyState::Released
                    }),
                });
                if tx.send(ev).is_err() {
                    log::info!("input capture: receiver dropped, stopping");
                    return;
                }
            }
        }

        // ---- 鼠标相对位移 ----
        let mut p = POINT { x: 0, y: 0 };
        if unsafe { GetCursorPos(&mut p) }.is_ok() {
            if have_pos {
                let dx = (p.x - last_pos.x) as i16;
                let dy = (p.y - last_pos.y) as i16;
                if dx != 0 || dy != 0 {
                    let ev = InputEvent::Mouse(MouseEvent {
                        dx,
                        dy,
                        button: None,
                        state: None,
                    });
                    if tx.send(ev).is_err() {
                        log::info!("input capture: receiver dropped, stopping");
                        return;
                    }
                }
            }
            last_pos = p;
            have_pos = true;
        }

        thread::sleep(Duration::from_millis(5));
    }
}
