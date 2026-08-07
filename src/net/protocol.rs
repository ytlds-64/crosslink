use serde::{Deserialize, Serialize};

use crate::input::event::InputEvent;

/// 应用层消息（握手完成后全部经 AES-GCM 加密传输）。
///
/// M1 实现连通性与心跳；M2 扩展输入事件转发（先键盘，后续加入鼠标/滚轮）。
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum Message {
    /// 握手后首条消息：告知对端本节点名称与平台
    Hello { name: String, platform: String },
    /// 心跳（携带发送时刻毫秒时间戳，用于测量 RTT）
    Heartbeat { t: u64 },
    /// 心跳应答（回显原始时间戳）
    HeartbeatAck { t: u64 },
    /// 输入事件（M2：捕获端→注入端）
    Input(InputEvent),
}
