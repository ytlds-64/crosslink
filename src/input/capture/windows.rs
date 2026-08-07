//! Windows 键盘输入捕获。
//!
//! M2.2 实现：基于 `GetAsyncKeyState` 的 5ms 轮询版本。
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

use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;

use crate::input::event::{InputEvent, KeyEvent, KeyState};
use crate::input::keycodes;

/// 启动键盘捕获，返回事件 channel 的接收端。
///
/// 调用方在独立线程（`tokio::task::spawn_blocking`）里调用 `.recv()` 即可。
pub fn start_keyboard_capture() -> Receiver<InputEvent> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || run_capture_loop(tx));
    rx
}

fn run_capture_loop(tx: mpsc::Sender<InputEvent>) {
    log::info!("input capture: started (Windows GetAsyncKeyState, 5ms poll)");
    let mut prev: [bool; 256] = [false; 256];
    loop {
        for vk in 0u16..256 {
            // GetAsyncKeyState 返回 SHORT(i16)；最高位(0x8000)= 当前是否按下
            // 在 Windows 上返回类型实际是 i16。我们只关心高字节。
            let state = unsafe { GetAsyncKeyState(vk as i32) };
            let down = (state as u16 & 0x8000) != 0;
            let idx = vk as usize;
            if down != prev[idx] {
                prev[idx] = down;
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
                    // 只在按下时 log warn，避免释放时噪音
                    log::trace!("input capture: unmapped VK 0x{:02X} (release skipped)", vk);
                }
            }
        }
        thread::sleep(Duration::from_millis(5));
    }
}
