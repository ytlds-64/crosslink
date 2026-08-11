//! Windows 输入捕获（键盘 + 鼠标）。
//!
//! M2.2 键盘：基于 `GetAsyncKeyState` 的 5ms 轮询版本。
//! M2.3 鼠标：按键用 `GetAsyncKeyState`，相对位移用 `GetCursorPos` 帧差。
//! M4 无缝单光标（Win 主控 Mac）：
//!   - **低级别 hook（WH_KEYBOARD_LL + WH_MOUSE_LL）**：on_mac 期间吞掉
//!     Win 侧按键 / 鼠标点击 / 滚轮（Win 前台程序不再响应），并把它们转发到 Mac。
//!   - **鼠标位移**仍用 `GetCursorPos` 帧差（用户机器 raw input 环境不可用时的
//!     已验证路径）：on_mac 期间让 Win 光标自由游走（不 pin、不 wrap），
//!     dx/dy 反映真实物理移动。
//!   - **光标隐藏**：用 `SetSystemCursor` 换成 1x1 全透明光标（替代无效的
//!     `ShowCursor(FALSE)`），进入 Mac 时隐藏、返回 Win 时恢复。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HINSTANCE, HWND, LRESULT, POINT, WPARAM, LPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CreateCursor, DispatchMessageW, GetCursorPos, GetMessageW, HHOOK, HCURSOR,
    IDC_ARROW, KBDLLHOOKSTRUCT, LoadCursorW, MSG, OCR_NORMAL, SetCursorPos, SetSystemCursor,
    SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL, WH_MOUSE_LL,
    WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEWHEEL,
    WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SYSKEYDOWN,
};

use crate::input::event::{InputEvent, KeyEvent, KeyState, MouseButton, MouseEvent};
use crate::input::keycodes;
use crate::input::{CaptureMsg, CaptureOptions};

// ── M4 低级别 hook 共享状态 ────────────────────────────────────────────────
// 低级别 hook（WH_KEYBOARD_LL / WH_MOUSE_LL）在独立线程运行消息泵，回调在本进程
// 上下文被调用。它们与 capture 主循环通过这些 static 通信：
// - `ON_MAC_HOOK`：当前是否在 Mac 侧。hook 回调据此决定是否吞掉/转发输入；
//   capture 主循环（run_capture_loop）在状态迁移时写入它。
// - `HOOK_TX`：键盘/鼠标事件转发给 Mac 的发送端。
// - `HOOKS_OK`：hook 是否安装成功（失败则降级：不吞事件，Win 会响应）。
static ON_MAC_HOOK: AtomicBool = AtomicBool::new(false);
static HOOKS_OK: AtomicBool = AtomicBool::new(false);
static HOOK_TX: Mutex<Option<mpsc::Sender<CaptureMsg>>> = Mutex::new(None);

// 透明光标隐藏状态（on_mac 期间隐藏 Win 系统光标）
static CURSOR_HIDDEN: AtomicBool = AtomicBool::new(false);
/// HCURSOR 内部是 `*mut c_void`（非 Send/Sync），包一层以便放进 static。
struct SendCursor(HCURSOR);
unsafe impl Send for SendCursor {}
unsafe impl Sync for SendCursor {}
static ORIG_ARROW: Mutex<Option<SendCursor>> = Mutex::new(None);
/// 1x1 全透明光标：AND 掩码全 1（透明），XOR 掩码全 0。
const TRANSPARENT_AND: [u8; 2] = [0xFF, 0x00];
const TRANSPARENT_XOR: [u8; 2] = [0x00, 0x00];

unsafe extern "system" fn low_level_kb_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 && ON_MAC_HOOK.load(Ordering::Relaxed) {
        let kbhs = *(lparam.0 as *const KBDLLHOOKSTRUCT);
        if let Some(hid) = keycodes::vk_to_hid(kbhs.vkCode as u16) {
            let is_down = matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
            let state = if is_down { KeyState::Pressed } else { KeyState::Released };
            if let Ok(g) = HOOK_TX.lock() {
                if let Some(tx) = g.as_ref() {
                    let _ = tx.send(CaptureMsg::Input(InputEvent::Key(KeyEvent { hid, state })));
                }
            }
        }
        return LRESULT(1); // 吞掉：Win 前台程序不收此键
    }
    CallNextHookEx(HHOOK(std::ptr::null_mut()), code, wparam, lparam)
}

unsafe extern "system" fn low_level_mouse_proc(
    code: i32,
    wparam: WPARAM,
    _lparam: LPARAM,
) -> LRESULT {
    if code >= 0 && ON_MAC_HOOK.load(Ordering::Relaxed) {
        let wp = wparam.0 as u32;
        match wp {
            WM_LBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONDOWN | WM_RBUTTONUP
            | WM_MBUTTONDOWN | WM_MBUTTONUP => {
                let button = match wp {
                    WM_LBUTTONDOWN | WM_LBUTTONUP => MouseButton::Left,
                    WM_RBUTTONDOWN | WM_RBUTTONUP => MouseButton::Right,
                    _ => MouseButton::Middle,
                };
                let down = matches!(wp, WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN);
                let state = if down { KeyState::Pressed } else { KeyState::Released };
                if let Ok(g) = HOOK_TX.lock() {
                    if let Some(tx) = g.as_ref() {
                        let _ = tx.send(CaptureMsg::Input(InputEvent::Mouse(MouseEvent {
                            dx: 0,
                            dy: 0,
                            button: Some(button),
                            state: Some(state),
                        })));
                    }
                }
                return LRESULT(1); // 吞掉：Win 不响应点击
            }
            WM_MOUSEWHEEL => {
                // 滚轮暂不转发（MouseEvent 暂无 wheel 字段）；吞掉避免 Win 侧滚动。
                return LRESULT(1);
            }
            _ => {}
        }
    }
    CallNextHookEx(HHOOK(std::ptr::null_mut()), code, wparam, _lparam)
}

/// 运行低级别 hook 的线程：安装 hook + 泵消息（hook 回调需要线程有消息队列）。
fn hook_thread(tx: mpsc::Sender<CaptureMsg>) {
    *HOOK_TX.lock().unwrap() = Some(tx);
    HOOKS_OK.store(false, Ordering::Relaxed);
    unsafe {
        let hinst: HINSTANCE = match GetModuleHandleW(None) {
            Ok(h) => h.into(),
            Err(e) => {
                log::error!("M4 hook: GetModuleHandleW failed: {:?}", e);
                set_system_cursor_hidden(false);
                return;
            }
        };
        let kb = SetWindowsHookExW(WH_KEYBOARD_LL, Some(low_level_kb_proc), hinst, 0);
        let mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(low_level_mouse_proc), hinst, 0);
        match (&kb, &mouse) {
            (Ok(_), Ok(_)) => {
                HOOKS_OK.store(true, Ordering::Relaxed);
                log::info!("M4: low-level input hooks installed (WH_KEYBOARD_LL + WH_MOUSE_LL)");
            }
            _ => {
                log::error!(
                    "M4 FATAL: low-level hook install failed: kb={:?} mouse={:?} \
                     -- 点击/按键可能仍透传到 Win",
                    kb.as_ref().err(),
                    mouse.as_ref().err()
                );
            }
        }
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, HWND(std::ptr::null_mut()), 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            let _ = DispatchMessageW(&msg);
        }
        if let Ok(k) = kb {
            let _ = UnhookWindowsHookEx(k);
        }
        if let Ok(m) = mouse {
            let _ = UnhookWindowsHookEx(m);
        }
        HOOKS_OK.store(false, Ordering::Relaxed);
        // 退出时恢复系统光标（若仍隐藏）
        set_system_cursor_hidden(false);
    }
}

/// 隐藏 / 恢复系统箭头光标（on_mac 期间让 Win 光标不可见）。
fn set_system_cursor_hidden(hidden: bool) {
    if hidden == CURSOR_HIDDEN.load(Ordering::Relaxed) {
        return;
    }
    unsafe {
        if hidden {
            let hinst: HINSTANCE = match GetModuleHandleW(None) {
                Ok(h) => h.into(),
                Err(e) => {
                    log::error!("M4: GetModuleHandleW failed (cursor hide): {:?}", e);
                    return;
                }
            };
            match LoadCursorW(None, IDC_ARROW) {
                Ok(orig) => match CreateCursor(
                    hinst,
                    0,
                    0,
                    1,
                    1,
                    TRANSPARENT_AND.as_ptr() as *const core::ffi::c_void,
                    TRANSPARENT_XOR.as_ptr() as *const core::ffi::c_void,
                ) {
                    Ok(trans) => {
                        if SetSystemCursor(trans, OCR_NORMAL).is_ok() {
                            *ORIG_ARROW.lock().unwrap() = Some(SendCursor(orig));
                            CURSOR_HIDDEN.store(true, Ordering::Relaxed);
                            log::info!("M4: system cursor hidden (transparent) while on Mac");
                        } else {
                            log::error!("M4: SetSystemCursor(hide) failed");
                        }
                    }
                    Err(e) => {
                        log::error!("M4: CreateCursor failed: {:?}", e);
                    }
                },
                Err(e) => {
                    log::error!("M4: LoadCursorW(arrow) failed: {:?}", e);
                }
            }
        } else {
            let orig = ORIG_ARROW
                .lock()
                .unwrap()
                .take()
                .map(|c| c.0)
                .unwrap_or_else(|| LoadCursorW(None, IDC_ARROW).unwrap_or(HCURSOR(std::ptr::null_mut())));
            let _ = SetSystemCursor(orig, OCR_NORMAL);
            CURSOR_HIDDEN.store(false, Ordering::Relaxed);
            log::info!("M4: system cursor restored");
        }
    }
}

/// 启动输入捕获（键盘 + 鼠标），返回事件 channel 的接收端。
pub fn start_capture(opts: CaptureOptions) -> Receiver<CaptureMsg> {
    let (tx, rx) = mpsc::channel();
    if opts.m4_mode {
        // M4：hook 线程吞掉 on_mac 期间的 Win 输入并转发；主循环用 GetCursorPos
        // 计算位移 + 驱动逻辑光标。--m4-fallback 不再需要单独处理（统一走此模型）。
        let tx_hook = tx.clone();
        thread::spawn(move || hook_thread(tx_hook));
    }
    thread::spawn(move || run_capture_loop(tx, opts));
    rx
}

/// 把 `delta` 从 `from` 分辨率等比映射到 `to` 分辨率（0 时退化为 1:1）。
fn map_axis(delta: i64, from: u32, to: u32) -> i64 {
    if from == 0 || to == 0 {
        return delta;
    }
    ((delta as f64) * (to as f64) / (from as f64)) as i64
}

fn run_capture_loop(tx: mpsc::Sender<CaptureMsg>, opts: CaptureOptions) {
    log::info!(
        "input capture: started (Windows, m4={}, fallback_flag={}, win={}x{})",
        opts.m4_mode,
        opts.m4_fallback,
        opts.win_w,
        opts.win_h,
    );

    // M2/M3 模式状态
    let mut prev_keys: [bool; 256] = [false; 256];
    let mut prev_btn = [false; 3];
    let mut last_pos = POINT { x: 0, y: 0 };
    let mut have_pos = false;

    // M4 状态
    let mut on_mac = false;
    let mut mac_cursor_x: i64 = 0;
    let mut mac_cursor_y: i64 = 0;
    let mut last_sent_x: i64 = -1;
    let mut last_sent_y: i64 = -1;
    let mut last_stream = Instant::now();

    let win_w_i = opts.win_w as i32;

    let mut hooks_warned = false;

    loop {
        // ---- 键盘 / 鼠标按键（仅非 M4 模式转发；M4 的 on_mac 转发由 hook 负责）----
        if !opts.m4_mode {
            for vk in 0u16..256 {
                let state = unsafe { GetAsyncKeyState(vk as i32) };
                let down = (state as u16 & 0x8000) != 0;
                let idx = vk as usize;
                if down != prev_keys[idx] {
                    prev_keys[idx] = down;
                    if let Some(hid) = keycodes::vk_to_hid(vk) {
                        let st = if down { KeyState::Pressed } else { KeyState::Released };
                        let ev = InputEvent::Key(KeyEvent { hid, state: st });
                        if tx.send(CaptureMsg::Input(ev)).is_err() {
                            log::info!("input capture: receiver dropped, stopping");
                            return;
                        }
                    } else if down {
                        log::trace!("input capture: unmapped VK 0x{:02X} (release skipped)", vk);
                    }
                }
            }

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
                    let st = if down { KeyState::Pressed } else { KeyState::Released };
                    let ev = InputEvent::Mouse(MouseEvent {
                        dx: 0,
                        dy: 0,
                        button: Some(button),
                        state: Some(st),
                    });
                    if tx.send(CaptureMsg::Input(ev)).is_err() {
                        log::info!("input capture: receiver dropped, stopping");
                        return;
                    }
                }
            }
        }

        if opts.m4_mode {
            if !HOOKS_OK.load(Ordering::Relaxed) && !hooks_warned {
                log::warn!("M4: input hooks not installed -- Win 点击/按键可能仍透传到 Win");
                hooks_warned = true;
            }
            let mac_w = opts.mac_w.load(Ordering::Relaxed);
            let mac_h = opts.mac_h.load(Ordering::Relaxed);

            let mut p = POINT { x: 0, y: 0 };
            let _ = unsafe { GetCursorPos(&mut p) };

            if !on_mac {
                let last_x = last_pos.x;
                let dx_fb = p.x - last_x;
                // M4-A: 光标进入 Win 最右列（且之前不在最右列）→ 切到 Mac
                if have_pos && p.x >= win_w_i - 1 && dx_fb > 0 {
                    set_system_cursor_hidden(true);
                    let _ = unsafe { SetCursorPos(0, p.y) };
                    on_mac = true;
                    ON_MAC_HOOK.store(true, Ordering::Relaxed);
                    mac_cursor_x = 0;
                    mac_cursor_y = map_axis(p.y as i64, opts.win_h, mac_h);
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
                    last_pos = POINT { x: 0, y: p.y };
                    have_pos = true;
                    last_sent_x = -1;
                    last_sent_y = -1;
                    last_stream = Instant::now();
                    continue;
                }
            } else {
                let last_x = if have_pos { last_pos.x } else { p.x };
                let last_y = if have_pos { last_pos.y } else { p.y };
                let dx_fb = p.x - last_x;
                let dy_fb = p.y - last_y;

                // M4-B: mac 光标在左缘且继续向左 → 回到 Win
                if mac_cursor_x <= 0 && dx_fb < 0 {
                    set_system_cursor_hidden(false);
                    let _ = unsafe { SetCursorPos(win_w_i - 1, p.y) };
                    on_mac = false;
                    ON_MAC_HOOK.store(false, Ordering::Relaxed);
                    let _ = tx.send(CaptureMsg::CursorState {
                        on_mac: false,
                        x: 0,
                        y: 0,
                    });
                    log::info!("m4: return to Win region at win_y={}", p.y);
                    last_pos = POINT { x: win_w_i - 1, y: p.y };
                    have_pos = true;
                    last_sent_x = -1;
                    last_sent_y = -1;
                    last_stream = Instant::now();
                    continue;
                }

                // M4-D: 累积 mac 光标（GetCursorPos 反映真实相对位移）
                if dx_fb != 0 || dy_fb != 0 {
                    mac_cursor_x += map_axis(dx_fb as i64, opts.win_w, mac_w);
                    mac_cursor_y += map_axis(dy_fb as i64, opts.win_h, mac_h);
                    let new_x = mac_cursor_x.clamp(0, mac_w as i64);
                    let new_y = mac_cursor_y.clamp(0, mac_h as i64);
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

                    let now = Instant::now();
                    if (new_x != last_sent_x || new_y != last_sent_y)
                        && now.duration_since(last_stream) >= Duration::from_millis(16)
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
                            "m4 stream: dx={} dy={} mac=({}, {})",
                            dx_fb,
                            dy_fb,
                            new_x,
                            new_y
                        );
                    }
                }
                last_pos = p;
                have_pos = true;
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
