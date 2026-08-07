//! 键码翻译：Windows Virtual-Key ↔ USB HID Keyboard Usage。
//!
//! 两端各自走自己平台的本地键码（HID 仅作 wire 内部表示）：
//! - 捕获：本地 VK → HID（`vk_to_hid`）
//! - 注入：HID → 本地 VK（`hid_to_vk`）
//!
//! 不识别的键返回 `None`，由调用方 log warn 后丢弃（不阻塞后续事件）。
//!
//! 完整键表见 USB HID Usage Tables 1.5 §10 (Keyboard/Keypad Page, 0x07)。

/// USB HID Keyboard/Keypad Page (0x07) 常量。
#[allow(dead_code)]
pub mod hid {
    // 字母 a–z
    pub const A: u16 = 0x04;
    pub const Z: u16 = 0x1D;
    // 数字 1–9、0
    pub const _1: u16 = 0x1E;
    pub const _2: u16 = 0x1F;
    pub const _3: u16 = 0x20;
    pub const _4: u16 = 0x21;
    pub const _5: u16 = 0x22;
    pub const _6: u16 = 0x23;
    pub const _7: u16 = 0x24;
    pub const _8: u16 = 0x25;
    pub const _9: u16 = 0x26;
    pub const _0: u16 = 0x27;
    // 常用控制
    pub const ENTER: u16 = 0x28;
    pub const ESC: u16 = 0x29;
    pub const BACKSPACE: u16 = 0x2A;
    pub const TAB: u16 = 0x2B;
    pub const SPACE: u16 = 0x2C;
    // 标点
    pub const MINUS: u16 = 0x2D;
    pub const EQUAL: u16 = 0x2E;
    pub const LBRACKET: u16 = 0x2F;
    pub const RBRACKET: u16 = 0x30;
    pub const BACKSLASH: u16 = 0x31;
    pub const SEMICOLON: u16 = 0x33;
    pub const APOSTROPHE: u16 = 0x34;
    pub const GRAVE: u16 = 0x35;
    pub const COMMA: u16 = 0x36;
    pub const PERIOD: u16 = 0x37;
    pub const SLASH: u16 = 0x38;
    // 锁定 / 功能
    pub const CAPSLOCK: u16 = 0x39;
    pub const F1: u16 = 0x3A;
    pub const F2: u16 = 0x3B;
    pub const F3: u16 = 0x3C;
    pub const F4: u16 = 0x3D;
    pub const F5: u16 = 0x3E;
    pub const F6: u16 = 0x3F;
    pub const F7: u16 = 0x40;
    pub const F8: u16 = 0x41;
    pub const F9: u16 = 0x42;
    pub const F10: u16 = 0x43;
    pub const F11: u16 = 0x44;
    pub const F12: u16 = 0x45;
    // 编辑 / 方向
    pub const DELETE: u16 = 0x4C;
    pub const RIGHT: u16 = 0x4F;
    pub const LEFT: u16 = 0x50;
    pub const DOWN: u16 = 0x51;
    pub const UP: u16 = 0x52;
    // 修饰键（左/右独立）
    pub const LCTRL: u16 = 0xE0;
    pub const LSHIFT: u16 = 0xE1;
    pub const LALT: u16 = 0xE2;
    pub const LGUI: u16 = 0xE3;
    pub const RCTRL: u16 = 0xE4;
    pub const RSHIFT: u16 = 0xE5;
    pub const RALT: u16 = 0xE6;
    pub const RGUI: u16 = 0xE7;
}

/// Windows Virtual-Key Code → HID Usage ID。
pub fn vk_to_hid(vk: u16) -> Option<u16> {
    use hid::*;
    match vk {
        // 字母
        0x41..=0x5A => Some((vk - 0x41) + A),
        // 数字
        0x31 => Some(_1),
        0x32 => Some(_2),
        0x33 => Some(_3),
        0x34 => Some(_4),
        0x35 => Some(_5),
        0x36 => Some(_6),
        0x37 => Some(_7),
        0x38 => Some(_8),
        0x39 => Some(_9),
        0x30 => Some(_0),
        // 控制
        0x0D => Some(ENTER),
        0x1B => Some(ESC),
        0x08 => Some(BACKSPACE),
        0x09 => Some(TAB),
        0x20 => Some(SPACE),
        // 标点
        0xBD => Some(MINUS),
        0xBB => Some(EQUAL),
        0xDB => Some(LBRACKET),
        0xDD => Some(RBRACKET),
        0xDC => Some(BACKSLASH),
        0xBA => Some(SEMICOLON),
        0xDE => Some(APOSTROPHE),
        0xC0 => Some(GRAVE),
        0xBC => Some(COMMA),
        0xBE => Some(PERIOD),
        0xBF => Some(SLASH),
        // 锁定 / 功能
        0x14 => Some(CAPSLOCK),
        0x70 => Some(F1),
        0x71 => Some(F2),
        0x72 => Some(F3),
        0x73 => Some(F4),
        0x74 => Some(F5),
        0x75 => Some(F6),
        0x76 => Some(F7),
        0x77 => Some(F8),
        0x78 => Some(F9),
        0x79 => Some(F10),
        0x7A => Some(F11),
        0x7B => Some(F12),
        // 编辑 / 方向
        0x2E => Some(DELETE),
        0x27 => Some(RIGHT),
        0x25 => Some(LEFT),
        0x28 => Some(DOWN),
        0x26 => Some(UP),
        // 修饰键
        0xA0 => Some(LSHIFT),
        0xA1 => Some(RSHIFT),
        0xA2 => Some(LCTRL),
        0xA3 => Some(RCTRL),
        0xA4 => Some(LALT),
        0xA5 => Some(RALT),
        0x5B => Some(LGUI),
        0x5C => Some(RGUI),
        _ => None,
    }
}

/// HID Usage ID → Windows Virtual-Key Code。
pub fn hid_to_vk(hid: u16) -> Option<u16> {
    use hid::*;
    // 连续区间用 if 守卫（match 模式不识别模块 const 作为范围端点）
    if (A..=Z).contains(&hid) {
        return Some((hid - A) + 0x41);
    }
    if (_1..=_9).contains(&hid) {
        return Some((hid - _1) + 0x31);
    }
    if (F1..=F12).contains(&hid) {
        return Some((hid - F1) + 0x70);
    }
    match hid {
        _0 => Some(0x30),
        // 控制
        ENTER => Some(0x0D),
        ESC => Some(0x1B),
        BACKSPACE => Some(0x08),
        TAB => Some(0x09),
        SPACE => Some(0x20),
        // 标点
        MINUS => Some(0xBD),
        EQUAL => Some(0xBB),
        LBRACKET => Some(0xDB),
        RBRACKET => Some(0xDD),
        BACKSLASH => Some(0xDC),
        SEMICOLON => Some(0xBA),
        APOSTROPHE => Some(0xDE),
        GRAVE => Some(0xC0),
        COMMA => Some(0xBC),
        PERIOD => Some(0xBE),
        SLASH => Some(0xBF),
        // 锁定
        CAPSLOCK => Some(0x14),
        // 编辑 / 方向
        DELETE => Some(0x2E),
        RIGHT => Some(0x27),
        LEFT => Some(0x25),
        DOWN => Some(0x28),
        UP => Some(0x26),
        // 修饰键
        LSHIFT => Some(0xA0),
        RSHIFT => Some(0xA1),
        LCTRL => Some(0xA2),
        RCTRL => Some(0xA3),
        LALT => Some(0xA4),
        RALT => Some(0xA5),
        LGUI => Some(0x5B),
        RGUI => Some(0x5C),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_letters() {
        for vk in 0x41u16..=0x5A {
            let hid = vk_to_hid(vk).expect("letter should map");
            let back = hid_to_vk(hid).expect("hid should map back");
            assert_eq!(vk, back, "round-trip letter VK 0x{:02X}", vk);
        }
    }

    #[test]
    fn round_trip_numbers() {
        for vk in [0x30u16, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39] {
            let hid = vk_to_hid(vk).expect("digit should map");
            let back = hid_to_vk(hid).expect("hid should map back");
            assert_eq!(vk, back, "round-trip digit VK 0x{:02X}", vk);
        }
    }

    #[test]
    fn round_trip_modifiers() {
        for vk in [0xA0u16, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5, 0x5B, 0x5C] {
            let hid = vk_to_hid(vk).expect("modifier should map");
            let back = hid_to_vk(hid).expect("hid should map back");
            assert_eq!(vk, back, "round-trip modifier VK 0x{:02X}", vk);
        }
    }

    #[test]
    fn round_trip_misc() {
        for vk in [0x0Du16, 0x1B, 0x08, 0x09, 0x20, 0x14, 0x2E, 0x27, 0x25, 0x28, 0x26] {
            let hid = vk_to_hid(vk).expect("misc should map");
            let back = hid_to_vk(hid).expect("hid should map back");
            assert_eq!(vk, back, "round-trip misc VK 0x{:02X}", vk);
        }
    }
}
