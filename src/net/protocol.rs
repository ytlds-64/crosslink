use serde::{Deserialize, Serialize};

use crate::input::event::InputEvent;

/// 指针移交（M3：当前 owner 把指针交给对端）。
///
/// `entry_x` / `entry_y` 为**对端本地坐标系**下的落点
/// （即接收方应在其「接缝边」的该位置落下光标并接管）。
#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub struct Transfer {
    pub entry_x: u32,
    pub entry_y: u32,
}

/// 逻辑光标状态（M4 无缝单光标：Win server 通知 Mac 端光标应在哪显示）。
///
/// Win 与 Mac 在逻辑上是一块扩展桌面；只有一个逻辑光标，由 Win 鼠标驱动。
/// Win 根据 Win 鼠标位置决定光标在 Win 屏还是 Mac 屏，通过此消息告诉 Mac 端
/// 应显示还是隐藏光标，并在显示时把光标 warp 到指定位置（Mac 本地坐标系）。
#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub struct CursorState {
    /// `true` = 光标在 Mac 区域（Mac 显示光标并 warp 到 (x, y)，Win 隐藏）；
    /// `false` = 光标在 Win 区域（Mac 隐藏光标，Win 显示）。
    pub on_mac: bool,
    /// Mac 本地坐标系的落点（仅 `on_mac=true` 有意义）。
    pub x: u32,
    pub y: u32,
}

/// 应用层消息（握手完成后全部经 AES-GCM 加密传输）。
///
/// M1 实现连通性与心跳；M2 扩展输入事件转发（先键盘，后续加入鼠标/滚轮）；
/// M3 扩展边缘切换（指针漫游）：`Hello` 携带屏幕几何，`Transfer` 用于跨机指针移交。
/// M4 扩展无缝单光标：`CursorState` 让 Win 通知 Mac 端光标显隐与位置。
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum Message {
    /// 握手后首条消息：告知对端本节点名称、平台与主屏分辨率。
    /// 屏幕几何用于 M3 边缘切换 / M4 无缝单光标的坐标映射。
    Hello {
        name: String,
        platform: String,
        screen_w: u32,
        screen_h: u32,
    },
    /// 心跳（携带发送时刻毫秒时间戳，用于测量 RTT）
    Heartbeat { t: u64 },
    /// 心跳应答（回显原始时间戳）
    HeartbeatAck { t: u64 },
    /// 输入事件（M2：捕获端 → 注入端；M4 同样适用，仅在 `on_mac` 时注入）
    Input(InputEvent),
    /// 指针移交（M3：当前 owner 把指针交给对端）。
    Transfer(Transfer),
    /// 逻辑光标状态（M4 无缝单光标模式专用，Win → Mac）。
    CursorState(CursorState),
}
