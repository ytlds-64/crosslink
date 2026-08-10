//! Windows 输入捕获（键盘 + 鼠标）。
//!
//! M2.2 实现键盘：基于 `GetAsyncKeyState` 的 5ms 轮询版本。
//! M2.3 加入鼠标：按键用 `GetAsyncKeyState`（VK_LBUTTON/RBUTTON/MBUTTON），
//! 相对位移用 `GetCursorPos` 计算两帧之间的差值。
//! M4 加入无缝单光标：Win 鼠标推到 Win 右缘时 warp 到左缘（光标已隐藏、用户不可见），
//! 切到 Mac 区域并通知 Mac 显示光标；推到左缘时反向切回。
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
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState,
};
use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, SetCursorPos, ShowCursor};

use crate::input::event::{InputEvent, KeyEvent, KeyState, MouseButton, MouseEvent};
use crate::input::keycodes;
use crate::input::{CaptureMsg, CaptureOptions};

/// 启动输入捕获（键盘 + 鼠标），返回事件 channel 的接收端。
///
/// 调用方在独立线程（`tokio::task::spawn_blocking`）里调用 `.recv()` 即可。
pub fn start_capture(opts: CaptureOptions) -> Receiver<CaptureMsg> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || run_capture_loop(tx, opts));
    rx
}

/// 切换 Win 光标的可见性。
///
/// `ShowCursor` 是引用计数的：TRUE 递增、FALSE 递减，计数 < 0 时光标隐藏。
/// 为避免多次切换产生不平衡，循环调到目标状态为止。
fn set_cursor_visible(visible: bool) {
    unsafe {
        if visible {
            while ShowCursor(windows::Win32::Foundation::BOOL(1)) < 0 {}
        } else {
            while ShowCursor(windows::Win32::Foundation::BOOL(0)) >= 0 {}
        }
    }
}

/// Win y → Mac y 线性映射。`mac_h=0`（未知）时退化为恒等。
fn map_y(win_y: i64, win_h: u32, mac_h: u32) -> i64 {
    if mac_h == 0 || win_h == 0 {
        return win_y;
    }
    ((win_y as f64 / win_h as f64) * mac_h as f64) as i64
}

fn run_capture_loop(tx: mpsc::Sender<CaptureMsg>, opts: CaptureOptions) {
    log::info!(
        "input capture: started (Windows, 5ms poll, m4={}, win={}x{})",
        opts.m4_mode,
        opts.win_w,
        opts.win_h,
    );
    let mut prev_keys: [bool; 256] = [false; 256];
    // 鼠标按键：0=Left, 1=Right, 2=Middle
    let mut prev_btn = [false; 3];
    let mut last_pos = POINT { x: 0, y: 0 };
    let mut have_pos = false;

    // M4 状态
    let mut on_mac = false; // false=Win 区域，true=Mac 区域
    let mut mac_cursor_x: i64 = 0;
    let mut mac_cursor_y: i64 = 0;

    loop {
        // ---- 键盘 ----
        for vk in 0u16..256 {
            let state = unsafe { GetAsyncKeyState(vk as i32) };
            let down = (state as u16 & 0x8000) != 0;
            let idx = vk as usize;
            if down != prev_keys[idx] {
                prev_keys[idx] = down;
                if let Some(hid) = keycodes::vk_to_hid(vk) {
                    let state = if down {
                        KeyState::Pressed
                    } else {
                        KeyState::Released
                    };
                    // M4：仅在光标位于 Mac 区域（on_mac）时把键盘转发给 Mac；
                    // 光标回到 Win 区域时 Win 原生处理，不再转发（避免双投）。
                    if !(opts.m4_mode && !on_mac) {
                        let ev = InputEvent::Key(KeyEvent { hid, state });
                        if tx.send(CaptureMsg::Input(ev)).is_err() {
                            log::info!("input capture: receiver dropped, stopping");
                            return;
                        }
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
                let state = if down {
                    KeyState::Pressed
                } else {
                    KeyState::Released
                };
                // M4：仅在光标位于 Mac 区域时转发鼠标按键（与键盘同理）。
                if !(opts.m4_mode && !on_mac) {
                    let ev = InputEvent::Mouse(MouseEvent {
                        dx: 0,
                        dy: 0,
                        button: Some(button),
                        state: Some(state),
                    });
                    if tx.send(CaptureMsg::Input(ev)).is_err() {
                        log::info!("input capture: receiver dropped, stopping");
                        return;
                    }
                }
            }
        }

        // ---- M4: 在 Mac 区域期间，确保 Win 光标保持隐藏 ----
        // 系统/其它进程可能通过 ShowCursor(TRUE) 把计数拉回 0+，每帧检查一次
        // （计数 < 0 视为隐藏）。第一次 ShowCursor(FALSE) 总是自减 1；
        // 如果减完 cnt < 0 就 break；否则继续减直到 cnt < 0。
        // 不能用 ClipCursor：物理钳制会让 dx=0，mac_cursor_x 不再累积 → Mac 光标卡住。
        if opts.m4_mode && on_mac {
            unsafe {
                loop {
                    let n = ShowCursor(windows::Win32::Foundation::BOOL(0));
                    if n < 0 {
                        break;
                    }
                }
            }
        }

        // ---- 鼠标相对位移 ----
        let mut p = POINT { x: 0, y: 0 };
        if unsafe { GetCursorPos(&mut p) }.is_ok() {
            if have_pos {
                let dx = p.x - last_pos.x;
                let dy = p.y - last_pos.y;

                if opts.m4_mode {
                    let win_w_i = opts.win_w as i32;
                    // 每帧从共享原子读取最新对端几何（Hello 到达后会更新）
                    use std::sync::atomic::Ordering;
                    let mac_w = opts.mac_w.load(Ordering::Relaxed);
                    let mac_h = opts.mac_h.load(Ordering::Relaxed);

                    // M4-A: 检测进入 Mac 区域（Win 光标到右缘 + **真实**向右推）
                    // 必须用 dx > 0（严格）：用 dx >= 0 会在 SetCursorPos(0,p.y) 后下一帧
                    // dx=0 时立刻把光标当成「还在右缘」自触发，导致和 M4-B 来回弹跳
                    // （Win 物理光标在 Mac 区域时被隐藏并不动）。
                    if !on_mac && p.x >= win_w_i - 1 && dx > 0 {
                        // **先隐藏再 warp**：避免 warp 瞬间在 (0, p.y) 闪一帧。
                        //   不能用 ClipCursor：物理钳制会让 GetCursorPos 的 dx 恒为 0，
                        //   mac_cursor_x 不再累积，Mac 光标立刻卡住。
                        set_cursor_visible(false);
                        let _ = unsafe { SetCursorPos(0, p.y) };
                        on_mac = true;
                        mac_cursor_x = 0;
                        mac_cursor_y = map_y(p.y as i64, opts.win_h, mac_h);
                        let _ = tx.send(CaptureMsg::CursorState {
                            on_mac: true,
                            x: 0,
                            y: mac_cursor_y.clamp(0, mac_h as i64) as u32,
                        });
                        log::info!("m4: enter Mac region at win_y={}, mapped mac_y={}", p.y, mac_cursor_y);
                        last_pos = POINT { x: 0, y: p.y };
                        have_pos = true;
                        continue;
                    }

                    // M4-B: 检测返回 Win 区域。
                    // 严格 dx < 0：dx = 0（光标未动）时不能当成「推左」触发；否则会和
                    // M4-A 来回弹（M4-A 把光标 warp 到左缘后 dx=0，立即触发 M4-B）。
                    // 同时也用 mac_cursor_x <= 0（之前累计已经达到 Mac 左缘且还在继续左推），
                    // 而非 Win 物理 p.x <= 0——后者在 Mac 区域时永远成立，会误触发。
                    if on_mac && mac_cursor_x <= 0 && dx < 0 {
                        let _ = unsafe { SetCursorPos(win_w_i - 1, p.y) };
                        set_cursor_visible(true);
                        on_mac = false;
                        let _ = tx.send(CaptureMsg::CursorState {
                            on_mac: false,
                            x: 0,
                            y: 0,
                        });
                        log::info!("m4: return to Win region at win_y={}", p.y);
                        last_pos = POINT { x: win_w_i - 1, y: p.y };
                        have_pos = true;
                        continue;
                    }

                    // M4-C: Mac 区域内循环 wrap（允许无限向右移动）
                    //   Win 光标在 Mac 区域到达 Win 物理右缘 → 静默 warp 到 Win 物理左缘
                    //   不切换区域、不改光标可见性、不发送 CursorState（Mac 光标连续平滑移动）
                    //   严格 dx > 0（只对真实右推做 wrap）
                    if on_mac && p.x >= win_w_i - 1 && dx > 0 {
                        let _ = unsafe { SetCursorPos(0, p.y) };
                        last_pos = POINT { x: 0, y: p.y };
                        have_pos = true;
                        continue;
                    }

                    // M4-D: 正常 delta 转发（仅 Mac 区域）。
                    // 位置只通过 CursorState（绝对坐标）驱动 Mac 光标，避免在 Mac
                    // 上既 warp 又注入相对位移造成双重移动/漂移。相对位移不再经
                    // Message::Input 发送（macOS inject 只在 M2 模式才会用到 delta）。
                    if dx != 0 || dy != 0 {
                        if on_mac {
                            mac_cursor_x += dx as i64;
                            mac_cursor_y += map_y(dy as i64, opts.win_h, mac_h);
                            let _ = tx.send(CaptureMsg::CursorState {
                                on_mac: true,
                                x: mac_cursor_x.clamp(0, mac_w as i64) as u32,
                                y: mac_cursor_y.clamp(0, mac_h as i64) as u32,
                            });
                            // 低频调试日志（~每秒 1 次）方便排查 Mac 光标是否跟随
                            log::trace!(
                                "m4 stream: dx={} dy={} mac=({}, {})",
                                dx,
                                dy,
                                mac_cursor_x.clamp(0, mac_w as i64),
                                mac_cursor_y.clamp(0, mac_h as i64)
                            );
                        }
                        // Win 区域：Mac 光标隐藏，不转发
                    }
                } else {
                    // M2/M3 默认：原样转发相对位移
                    let dx16 = dx as i16;
                    let dy16 = dy as i16;
                    if dx16 != 0 || dy16 != 0 {
                        let ev = InputEvent::Mouse(MouseEvent {
                            dx: dx16,
                            dy: dy16,
                            button: None,
                            state: None,
                        });
                        if tx.send(CaptureMsg::Input(ev)).is_err() {
                            log::info!("input capture: receiver dropped, stopping");
                            return;
                        }
                    }
                }
            }
            last_pos = p;
            have_pos = true;
        }

        thread::sleep(Duration::from_millis(5));
    }
}