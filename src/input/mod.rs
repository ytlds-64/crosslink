/// 当前运行平台标识（用于 Hello 消息与后续平台相关分支）。
pub fn platform() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "unknown"
    }
}

/// 输入后端状态占位（M2 实现）。
///
/// 后续里程碑将在此处加入平台特定的：
/// - Windows：Raw Input 捕获 + `SendInput` 注入
/// - macOS：`CGEventTap` 捕获 + `CGEventPost` 注入（需辅助功能权限）
/// 以及跨平台键码（HID Usage / Windows VK / macOS keycode）翻译模块。
#[allow(dead_code)]
pub fn backend_status() -> &'static str {
    "stub (M2)"
}
