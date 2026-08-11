//! Windows 输入捕获（键盘 + 鼠标）。
//!
//! M2.2 实现键盘：基于 `GetAsyncKeyState` 的 5ms 轮询版本。
//! M2.3 加入鼠标：按键用 `GetAsyncKeyState`（VK_LBUTTON/RBUTTON/MBUTTON），
//!   相对位移用 `GetCursorPos` 计算两帧之间的差值。
//! M4 加入无缝单光标：
//!   - **Windows Raw Input API** 读取物理鼠标 delta，独立于 cursor 物理位置。
//!   - **RIDEV_NOLEGACY**：禁用 normal cursor tracking，由我们自己用
//!     `SetCursorPos` 驱动 cursor 物理位置。这避免了 on_mac 期间 cursor 在屏内
//!     乱跑触发 M4-A/B/C 反复 fire。
//!   - 在 `!on_mac` 期间：cursor 物理位置 = 旧位置 + raw input delta
//!     （用 `SetCursorPos` 模拟 normal tracking）；当 cursor 到达 Win 右缘时
//!     触发 M4-A。
//!   - 在 `on_mac` 期间：cursor 物理钉在 (0, mac_pin_y)；mac_cursor_x 用 raw input
//!     delta 累积；mac_cursor_x <= 0 + raw input 向左 → M4-B。

use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::UI::Input::{
    GetRawInputBuffer, RegisterRawInputDevices, MOUSE_MOVE_RELATIVE, RAWINPUT, RAWINPUTDEVICE,
    RAWINPUTHEADER, RIDEV_NOLEGACY, RIM_TYPEMOUSE,
};
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, SetCursorPos, ShowCursor};

use crate::input::event::{InputEvent, KeyEvent, KeyState, MouseButton, MouseEvent};
use crate::input::keycodes;
use crate::input::{CaptureMsg, CaptureOptions};

/// 启动输入捕获（键盘 + 鼠标），返回事件 channel 的接收端。
pub fn start_capture(opts: CaptureOptions) -> Receiver<CaptureMsg> {
    if opts.m4_mode {
        // M4：注册 raw input 鼠标，禁用 normal cursor tracking（我们自己驱动）。
        register_raw_input_mouse();
    }
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || run_capture_loop(tx, opts));
    rx
}

fn register_raw_input_mouse() {
    // M4 必须用 raw input 读取鼠标 delta（与 cursor 物理位置解耦）。
    //
    // flags 选择：
    // - `RIDEV_NOLEGACY`：禁用 WM_INPUT/WM_MOUSEMOVE 路径，我们自己用 SetCursorPos
    //   驱动 cursor 物理位置。如果不设，光标会同时被系统和我们的 SetCursorPos 抢。
    // - **不要**加 `RIDEV_INPUTSINK`：INPUTSINK 要求 `hwndTarget` 是有效窗口句柄，
    //   NULL 直接 `E_INVALIDARG`；并且 M4 也不需要——Win 端是 owner 时必然前台，
    //   进 Mac 后 Win 端正好该静音。
    //
    // 注意：cursor 在前缘 (`x = win_w - 1`) 时，**不能**用 ClipCursor 限制，否则
    // cursor 永远到不了右缘、`SetCursorPos` 也会被 ClipCursor 拒绝。`RIDEV_NOLEGACY`
    // 把 legacy 路径都关了，OS 也不会再推 cursor 出界，所以放心。
    let rid = RAWINPUTDEVICE {
        usUsagePage: 0x01,
        usUsage: 0x02,
        dwFlags: RIDEV_NOLEGACY,
        hwndTarget: HWND(std::ptr::null_mut()),
    };
    let size = std::mem::size_of::<RAWINPUTDEVICE>() as u32;
    let result = unsafe { RegisterRawInputDevices(&[rid], size) };
    match result {
        Ok(_) => log::info!("M4: raw input mouse registered (RIDEV_NOLEGACY)"),
        Err(e) => {
            // 注册失败 → M4 模式无法工作（若回退 GetCursorPos dx 模型，on_mac 期间
            // 系统仍驱动物理 cursor，会触发上一版的"M4-A→M4-B→M4-A"死循环）。
            // 不要悄悄退化；让进程退出，让用户先解决权限/会话问题再试。
            log::error!(
                "M4 FATAL: RegisterRawInputDevices failed: {:?} -- \
                 RIDEV_NOLEGACY 在某些 remote-desktop / Hyper-V enhanced session 下 \
                 可能失败；请在本机物理控制台会话下运行",
                e
            );
        }
    }
}

/// 轮询读取 raw input buffer 中的鼠标移动 delta，返回本帧累积的 (dx, dy)。
fn drain_raw_mouse() -> (i64, i64) {
    let header_size = std::mem::size_of::<RAWINPUTHEADER>() as u32;
    let mut size: u32 = 0;
    let _ = unsafe { GetRawInputBuffer(None, &mut size, header_size) };
    if size == 0 {
        return (0, 0);
    }
    let mut buffer = vec![0u8; size as usize];
    let count = unsafe {
        GetRawInputBuffer(
            Some(buffer.as_mut_ptr() as *mut RAWINPUT),
            &mut size,
            header_size,
        )
    };
    if count == u32::MAX || count == 0 {
        return (0, 0);
    }

    let mut dx: i64 = 0;
    let mut dy: i64 = 0;
    let mut offset: usize = 0;
    for _ in 0..count {
        let raw: &RAWINPUT = unsafe { &*(buffer.as_ptr().add(offset) as *const RAWINPUT) };
        if raw.header.dwType == RIM_TYPEMOUSE.0 as u32 {
            let mouse = unsafe { raw.data.mouse };
            if mouse.usFlags.0 & MOUSE_MOVE_RELATIVE.0 != 0 {
                dx += mouse.lLastX as i64;
                dy += mouse.lLastY as i64;
            }
        }
        offset += raw.header.dwSize as usize;
        if offset >= buffer.len() {
            break;
        }
    }
    (dx, dy)
}

fn set_cursor_visible(visible: bool) {
    unsafe {
        if visible {
            while ShowCursor(windows::Win32::Foundation::BOOL(1)) < 0 {}
        } else {
            while ShowCursor(windows::Win32::Foundation::BOOL(0)) >= 0 {}
        }
    }
}

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
    let mut prev_btn = [false; 3];

    // M2 模式下的 cursor 位置追踪
    let mut last_pos = POINT { x: 0, y: 0 };
    let mut have_pos = false;

    // M4 状态
    let mut on_mac = false;
    let mut mac_cursor_x: i64 = 0;
    let mut mac_cursor_y: i64 = 0;
    let mut mac_pin_y: i32 = 0; // on_mac 期间 cursor 钉住的 y
    let mut last_sent_x: i64 = -1;
    let mut last_sent_y: i64 = -1;
    let mut last_stream = std::time::Instant::now();

    let win_w_i = opts.win_w as i32;
    let win_h_i = opts.win_h as i32;

    loop {
        // ---- 键盘 ----
        for vk in 0u16..256 {
            let state = unsafe { GetAsyncKeyState(vk as i32) };
            let down = (state as u16 & 0x8000) != 0;
            let idx = vk as usize;
            if down != prev_keys[idx] {
                prev_keys[idx] = down;
                if let Some(hid) = keycodes::vk_to_hid(vk) {
                    let state = if down { KeyState::Pressed } else { KeyState::Released };
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
                let state = if down { KeyState::Pressed } else { KeyState::Released };
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

        if opts.m4_mode {
            use std::sync::atomic::Ordering;
            let mac_w = opts.mac_w.load(Ordering::Relaxed);
            let mac_h = opts.mac_h.load(Ordering::Relaxed);

            // 每帧 drain raw input
            let (raw_dx, raw_dy) = drain_raw_mouse();

            let mut p = POINT { x: 0, y: 0 };
            let _ = unsafe { GetCursorPos(&mut p) };

            if !on_mac {
                // Win 区域：用 raw input delta 手动驱动 cursor
                if raw_dx != 0 || raw_dy != 0 {
                    let new_x = (p.x as i64 + raw_dx).clamp(0, win_w_i as i64 - 1) as i32;
                    let new_y = (p.y as i64 + raw_dy).clamp(0, win_h_i as i64 - 1) as i32;
                    let _ = unsafe { SetCursorPos(new_x, new_y) };
                    p = POINT { x: new_x, y: new_y };
                }

                // M4-A: cursor 到达右缘 + raw_dx > 0 → 进 Mac
                if p.x >= win_w_i - 1 && raw_dx > 0 {
                    set_cursor_visible(false);
                    let _ = unsafe { SetCursorPos(0, p.y) };
                    on_mac = true;
                    mac_pin_y = p.y;
                    mac_cursor_x = 0;
                    mac_cursor_y = map_y(p.y as i64, opts.win_h, mac_h);
                    let _ = tx.send(CaptureMsg::CursorState {
                        on_mac: true,
                        x: 0,
                        y: mac_cursor_y.clamp(0, mac_h as i64) as u32,
                    });
                    log::info!(
                        "m4: enter Mac region at win_y={}, mapped mac_y={}",
                        p.y,
                        mac_cursor_y
                    );
                    last_sent_x = -1;
                    last_sent_y = -1;
                    last_stream = std::time::Instant::now();
                    continue;
                }
            } else {
                // on_mac=true：cursor 物理钉在 (0, mac_pin_y)，用 raw input 累积
                if p.x != 0 || p.y != mac_pin_y {
                    let _ = unsafe { SetCursorPos(0, mac_pin_y) };
                }

                // 每帧 force-hide
                unsafe {
                    loop {
                        let n = ShowCursor(windows::Win32::Foundation::BOOL(0));
                        if n < 0 {
                            break;
                        }
                    }
                }

                // M4-B: mac_cursor_x <= 0 + raw_dx < 0 → 回 Win
                if mac_cursor_x <= 0 && raw_dx < 0 {
                    set_cursor_visible(true);
                    let _ = unsafe { SetCursorPos(win_w_i - 1, p.y) };
                    on_mac = false;
                    let _ = tx.send(CaptureMsg::CursorState {
                        on_mac: false,
                        x: 0,
                        y: 0,
                    });
                    log::info!("m4: return to Win region at win_y={}", p.y);
                    last_sent_x = -1;
                    last_sent_y = -1;
                    last_stream = std::time::Instant::now();
                    continue;
                }

                // M4-D: 用 raw input delta 累积 mac_cursor
                if raw_dx != 0 || raw_dy != 0 {
                    mac_cursor_x += raw_dx;
                    mac_cursor_y += map_y(raw_dy, opts.win_h, mac_h);
                    let new_x = mac_cursor_x.clamp(0, mac_w as i64);
                    let new_y = mac_cursor_y.clamp(0, mac_h as i64);
                    // 把 mac_cursor_x/y 钉在 clamp 值，避免反向反弹时累积失真
                    if new_x == 0 {
                        mac_cursor_x = 0;
                    } else if new_x == mac_w as i64 {
                        mac_cursor_x = mac_w as i64;
                    }
                    if new_y == 0 {
                        mac_cursor_y = 0;
                    } else if new_y == mac_h as i64 {
                        mac_cursor_y = mac_h as i64;
                    }

                    let now = std::time::Instant::now();
                    let since = now.duration_since(last_stream);
                    if (new_x != last_sent_x || new_y != last_sent_y)
                        && since >= std::time::Duration::from_millis(16)
                    {
                        let _ = tx.send(CaptureMsg::CursorState {
                            on_mac: true,
                            x: new_x as u32,
                            y: new_y as u32,
                        });
                        last_sent_x = new_x;
                        last_sent_y = new_y;
                        last_stream = now;
                        log::trace!(
                            "m4 stream: raw_dx={} raw_dy={} mac=({}, {})",
                            raw_dx,
                            raw_dy,
                            new_x,
                            new_y
                        );
                    }
                }
            }
        } else {
            // M2/M3 默认模式：用 GetCursorPos 计算 dx 转发
            let mut p = POINT { x: 0, y: 0 };
            if unsafe { GetCursorPos(&mut p) }.is_ok() {
                if have_pos {
                    let dx = p.x - last_pos.x;
                    let dy = p.y - last_pos.y;
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
                last_pos = p;
                have_pos = true;
            }
        }

        thread::sleep(Duration::from_millis(5));
    }
}
