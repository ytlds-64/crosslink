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

/// 应用层消息（握手完成后全部经 AES-GCM 加密传输）。
///
/// M1 实现连通性与心跳；M2 扩展输入事件转发（先键盘，后续加入鼠标/滚轮）；
/// M3 扩展边缘切换（指针漫游）：`Hello` 携带屏幕几何，`Transfer` 用于跨机指针移交。
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum Message {
    /// 握手后首条消息：告知对端本节点名称、平台与主屏分辨率。
    /// 屏幕几何用于 M3 边缘切换时计算指针在对端屏幕的落点。
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
    /// 输入事件（M2：捕获端 → 注入端）
    Input(InputEvent),
    /// 指针移交（M3：当前 owner 把指针交给对端）。
    Transfer(Transfer),
}
