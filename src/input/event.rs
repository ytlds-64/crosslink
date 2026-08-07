//! 跨平台输入事件（内部表示，Wire 格式）。
//!
//! 键码采用 **USB HID Usage Tables** 中 Keyboard/Keypad Page (0x07) 的 Usage ID：
//! - 0x04–0x1D: 字母 a–z
//! - 0x1E–0x27: 数字 1–9、0
//! - 0x28–0x38: 回车 / Esc / 退格 / Tab / 空格 / 符号键
//! - 0x39–0x45: Caps Lock / F1–F12
//! - 0x4F–0x52: 方向键
//! - 0xE0–0xE7: 左/右 Ctrl / Shift / Alt / GUI（Win/Cmd）
//!
//! 选择 HID 作为内部表示：跨平台统一（M2.4 macOS 接入零额外成本），
//! 与 Barrier / Deskflow / Universal Control 一致。

use serde::{Deserialize, Serialize};

/// USB HID Keyboard/Keypad Usage ID。
pub type HidKey = u16;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyState {
    Pressed,
    Released,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub struct KeyEvent {
    pub hid: HidKey,
    pub state: KeyState,
}

/// 输入事件（M2.2 仅键盘；后续里程碑扩展鼠标 / 滚轮）。
#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub enum InputEvent {
    Key(KeyEvent),
}
