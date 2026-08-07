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

/// 鼠标按键（与 HID Button 一致：左/右/中）。
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// 鼠标事件。
///
/// 采用「相对位移 + 按键边沿」表示，与 HID 鼠标报告一致：
/// - `dx` / `dy`：自上次事件以来的相对移动（像素，有符号）；`0, 0` 表示无移动。
/// - `button` + `state`：按键按下/释放边沿；`None` 表示本条事件不含按键变化。
///
/// 注：屏幕边缘切换逻辑（M3）尚未加入——M2.3 会无条件转发所有鼠标事件，
/// 仅用于端到端链路验证。
#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub struct MouseEvent {
    pub dx: i16,
    pub dy: i16,
    pub button: Option<MouseButton>,
    pub state: Option<KeyState>,
}

/// 输入事件（M2.2 键盘；M2.3 加入鼠标；后续里程碑扩展滚轮 / 拖拽）。
#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub enum InputEvent {
    Key(KeyEvent),
    Mouse(MouseEvent),
}

#[cfg(test)]
mod tests {
    use super::*;
    use bincode::{deserialize, serialize};

    #[test]
    fn mouse_event_bincode_roundtrip() {
        let ev = InputEvent::Mouse(MouseEvent {
            dx: 12,
            dy: -7,
            button: Some(MouseButton::Left),
            state: Some(KeyState::Pressed),
        });
        let bytes = serialize(&ev).expect("serialize");
        let back: InputEvent = deserialize(&bytes).expect("deserialize");
        match back {
            InputEvent::Mouse(m) => {
                assert_eq!(m.dx, 12);
                assert_eq!(m.dy, -7);
                assert_eq!(m.button, Some(MouseButton::Left));
                assert_eq!(m.state, Some(KeyState::Pressed));
            }
            _ => panic!("wrong variant"),
        }
    }
}
