//! M3 边缘切换（Universal Control 式指针漫游）控制器。
//!
//! 设计要点（对称式，契合「光标越屏边缘切到另一台，同时只控一台」）：
//! - 任一时刻指针只属于一台机器（owner）。owner 用本机物理键鼠**原生**操作本机，
//!   无需注入；非 owner 一方光标停在接缝边、忽略本地输入。
//! - owner 的光标推到与对端共享的「接缝边」时，立即把指针交给对端：
//!   对端在其接缝边对应位置落下光标并变身 owner；本端降级为非 owner。
//! - 稳态下**不转发任何鼠标/键盘**，跨机只发极小的 `Transfer` 消息，
//!   彻底绕开跨平台「输入抑制」难题。
//!
//! 拓扑：通过 `Side` 声明「对端相对本机的位置」。服务端默认 `Right`
//! （对端在右），客户端默认 `Left`（对端在左），二者一致即构成水平拼接。
//! 也支持 `Top` / `Bottom`（垂直拼接）。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use tokio::sync::mpsc;

use crate::input::screen;
use crate::net::protocol::Transfer;

/// 对端相对本机的位置（决定「接缝边」在哪一侧）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Right,
    Left,
    Top,
    Bottom,
}

impl Side {
    /// 本机与对端共享的「接缝边」（指针从本机穿越到对端所经过的屏幕边）。
    fn seam(self) -> Edge {
        match self {
            Side::Right => Edge::Right,
            Side::Left => Edge::Left,
            Side::Top => Edge::Top,
            Side::Bottom => Edge::Bottom,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Edge {
    Right,
    Left,
    Top,
    Bottom,
}

/// 距接缝边多少像素内视为「已到达」（触发穿越）。
const EDGE: i32 = 4;
/// 离开接缝边多少像素后才重新允许下一次穿越（防抖动 / 防启动乒乓）。
const REARM: i32 = 12;

struct State {
    /// 本机当前是否持有指针。
    owning: AtomicBool,
    /// 是否已「武装」——仅在离开接缝边再回来后才允许穿越，避免立即回弹。
    armed: AtomicBool,
    side: Side,
    my_w: u32,
    my_h: u32,
    /// 对端屏幕几何（Hello 交换后回填，初始 (0,0) → 监控空转）。
    peer: Mutex<(u32, u32)>,
    /// 当前指针在「本机本地坐标」下的位置（owner 时由监控线程同步）。
    cursor: Mutex<(i32, i32)>,
}

/// 边缘切换控制器。持有一个后台监控线程（仅 owner 时积极工作）。
pub struct Switch {
    state: Arc<State>,
    /// 把「穿越」事件发给会话循环，由会话负责加密送出。
    tx: mpsc::UnboundedSender<Transfer>,
}

impl Switch {
    /// `side` 对端相对本机的位置；`initial_owner` 本机是否初始持有指针
    /// （服务端 = true，客户端 = false）。
    pub fn new(
        side: Side,
        initial_owner: bool,
        my_w: u32,
        my_h: u32,
        tx: mpsc::UnboundedSender<Transfer>,
    ) -> Self {
        let state = Arc::new(State {
            owning: AtomicBool::new(initial_owner),
            armed: AtomicBool::new(initial_owner),
            side,
            my_w,
            my_h,
            peer: Mutex::new((0, 0)),
            cursor: Mutex::new((0, 0)),
        });

        // 非持有方：把真实光标停在接缝边中点，避免在本机屏幕里乱飘干扰用户。
        if !initial_owner {
            let (cx, cy) = seam_park_pos(side, my_w, my_h);
            screen::set_cursor_pos(cx, cy);
            *state.cursor.lock().unwrap() = (cx, cy);
        }

        Self { state, tx }
    }

    /// 收到对端的 `Transfer`：把指针落到本机接缝边的 `entry` 位置并接管。
    pub fn on_receive(&self, entry_x: u32, entry_y: u32) {
        let (x, y) = (entry_x as i32, entry_y as i32);
        log::info!("switch: pointer handed to me at ({}, {})", x, y);
        screen::set_cursor_pos(x, y);
        *self.state.cursor.lock().unwrap() = (x, y);
        self.state.owning.store(true, Ordering::SeqCst);
        // 刚接管时指针就在接缝边，需先向屏幕内移动再允许回去，避免立即回弹。
        self.state.armed.store(false, Ordering::SeqCst);
    }

    /// 启动后台监控线程。
    pub fn start_monitor(&self) {
        log::info!("switch: monitor starting (side = {:?})", self.state.side);
        let state = self.state.clone();
        let tx = self.tx.clone();
        thread::spawn(move || run_monitor(state, tx));
    }

    /// 回填对端屏幕几何（来自对端 Hello）。
    pub fn set_peer_geom(&self, w: u32, h: u32) {
        *self.state.peer.lock().unwrap() = (w, h);
        log::info!("switch: peer screen geometry {}x{}", w, h);
    }
}

fn run_monitor(state: Arc<State>, tx: mpsc::UnboundedSender<Transfer>) {
    log::info!("switch monitor: thread running");
    loop {
        if state.owning.load(Ordering::SeqCst) {
            let (mw, mh) = (state.my_w, state.my_h);
            let (pw, ph) = *state.peer.lock().unwrap();
            // 几何未知（尚未收到对端 Hello，或非目标平台）→ 空转。
            if mw == 0 || mh == 0 || pw == 0 || ph == 0 {
                thread::sleep(Duration::from_millis(50));
                continue;
            }

            let (x, y) = screen::get_cursor_pos();
            {
                let mut c = state.cursor.lock().unwrap();
                *c = (x, y);
            }

            let seam = state.side.seam();
            let dist = dist_to_seam(seam, x, y, mw, mh);

            if !state.armed.load(Ordering::SeqCst) {
                // 已触发过穿越：等指针明显离开接缝边（向屏幕内）再重新武装。
                if dist > REARM {
                    state.armed.store(true, Ordering::SeqCst);
                }
            } else if dist <= EDGE {
                // 已武装且抵达接缝边 → 触发穿越。
                if let Some((clamp_x, clamp_y, ex, ey)) =
                    compute_transfer(seam, x, y, mw, mh, pw, ph)
                {
                    screen::set_cursor_pos(clamp_x, clamp_y);
                    state.owning.store(false, Ordering::SeqCst);
                    state.armed.store(false, Ordering::SeqCst);
                    let _ = tx.send(Transfer {
                        entry_x: ex,
                        entry_y: ey,
                    });
                    log::info!("switch: handed pointer to peer at ({}, {})", ex, ey);
                }
            }
        }
        thread::sleep(Duration::from_millis(5));
    }
}

/// 指针距「接缝边」的像素距离（>=0；越靠近越小）。
fn dist_to_seam(seam: Edge, x: i32, y: i32, mw: u32, mh: u32) -> i32 {
    let mw = mw as i32;
    let mh = mh as i32;
    match seam {
        Edge::Right => (mw - 1 - x).max(0),
        Edge::Left => x.max(0),
        Edge::Top => y.max(0),
        Edge::Bottom => (mh - 1 - y).max(0),
    }
}

/// 非持有方初始停泊点：接缝边中点。
fn seam_park_pos(side: Side, mw: u32, mh: u32) -> (i32, i32) {
    let mw = mw as i32;
    let mh = mh as i32;
    match side.seam() {
        Edge::Right => (mw - 1, mh / 2),
        Edge::Left => (0, mh / 2),
        Edge::Top => (mw / 2, 0),
        Edge::Bottom => (mw / 2, mh - 1),
    }
}

/// 计算穿越：本端把光标夹在接缝边，并算出对端本地坐标下的落点。
///
/// 返回 `(夹住的本端x, 夹住的本端y, 对端落点x, 对端落点y)`。
/// 对端落点始终落在**对端的接缝边**：本端 Right → 对端 Left 边、本端 Left →
/// 对端 Right 边，以此类推；y/x 按双方分辨率比例映射。
fn compute_transfer(
    seam: Edge,
    x: i32,
    y: i32,
    mw: u32,
    mh: u32,
    pw: u32,
    ph: u32,
) -> Option<(i32, i32, u32, u32)> {
    let mw = mw as i32;
    let mh = mh as i32;
    let pw = pw as i32;
    let ph = ph as i32;

    let (clamp_x, clamp_y, ex, ey) = match seam {
        Edge::Right => {
            let ex = 0; // 对端 Left 边
            let ey = map(y, mh, ph);
            (mw - 1, y, ex, ey)
        }
        Edge::Left => {
            let ex = pw - 1; // 对端 Right 边
            let ey = map(y, mh, ph);
            (0, y, ex, ey)
        }
        Edge::Top => {
            let ex = map(x, mw, pw);
            let ey = ph - 1; // 对端 Bottom 边
            (x, 0, ex, ey)
        }
        Edge::Bottom => {
            let ex = map(x, mw, pw);
            let ey = 0; // 对端 Top 边
            (x, mh - 1, ex, ey)
        }
    };

    Some((
        clamp_x,
        clamp_y,
        ex.max(0) as u32,
        ey.max(0) as u32,
    ))
}

/// 按分辨率比例把一维坐标 `v`（范围 [0, from)）映射到 [0, to)。
fn map(v: i32, from: i32, to: i32) -> i32 {
    if from <= 0 || to <= 0 {
        return 0;
    }
    let r = (v as f64 * to as f64 / from as f64).round() as i32;
    r.clamp(0, to - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seam_mapping_right() {
        // 本端 1920x1080，对端 1440x900，对端在右。
        // 本端光标在右边缘 y=540（中点）→ 对端落点应在其 Left 边 (0, ~450)。
        let (cx, _cy, ex, ey) = compute_transfer(Edge::Right, 1919, 540, 1920, 1080, 1440, 900).unwrap();
        assert_eq!(cx, 1919);
        assert_eq!(ex, 0);
        assert_eq!(ey, 450);
    }

    #[test]
    fn seam_mapping_left() {
        // 本端 1440x900，对端在左 1920x1080。本端光标在左边缘 y=450 →
        // 对端落点在其 Right 边 (1919, ~540)。
        let (cx, _cy, ex, ey) = compute_transfer(Edge::Left, 0, 450, 1440, 900, 1920, 1080).unwrap();
        assert_eq!(cx, 0);
        assert_eq!(ex, 1919);
        assert_eq!(ey, 540);
    }

    #[test]
    fn dist_to_seam_right() {
        assert_eq!(dist_to_seam(Edge::Right, 1919, 0, 1920, 1080), 0);
        assert_eq!(dist_to_seam(Edge::Right, 1000, 0, 1920, 1080), 919);
    }

    #[test]
    fn map_clamps() {
        assert_eq!(map(0, 1920, 1440), 0);
        assert_eq!(map(1919, 1920, 1440), 1439);
        assert_eq!(map(100, 0, 100), 0); // 除零保护
    }
}
