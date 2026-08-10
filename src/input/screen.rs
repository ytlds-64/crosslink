//! 屏幕几何与绝对光标位置（跨平台）。M3 边缘切换（指针漫游）所需。
//!
//! - `screen_size`：主屏分辨率（像素）。
//! - `get_cursor_pos` / `set_cursor_pos`：读取 / 设置绝对光标位置，
//!   用于边界检测，以及指针穿越后在对端屏幕的落点定位。
//!
//! 非 Windows / 非 macOS 平台提供 no-op（返回 0 或空操作），保证可编译；
//! 这些平台不运行 `--switch` 模式（监控线程会因几何为 0 而自动空转）。

#[cfg(target_os = "windows")]
mod imp {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetCursorPos, GetSystemMetrics, SetCursorPos, SM_CXSCREEN, SM_CYSCREEN,
    };

    pub fn screen_size() -> (u32, u32) {
        let w = unsafe { GetSystemMetrics(SM_CXSCREEN) } as u32;
        let h = unsafe { GetSystemMetrics(SM_CYSCREEN) } as u32;
        (w, h)
    }

    pub fn get_cursor_pos() -> (i32, i32) {
        let mut p = POINT { x: 0, y: 0 };
        if unsafe { GetCursorPos(&mut p) }.is_ok() {
            (p.x, p.y)
        } else {
            (0, 0)
        }
    }

    pub fn set_cursor_pos(x: i32, y: i32) {
        unsafe {
            let _ = SetCursorPos(x, y);
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use core_graphics::display::CGDisplay;
    use core_graphics::geometry::CGPoint;

    pub fn screen_size() -> (u32, u32) {
        let disp = CGDisplay::main();
        let b = disp.bounds();
        (b.size.width as u32, b.size.height as u32)
    }

    pub fn get_cursor_pos() -> (i32, i32) {
        // Apple 官方姿势：从 Cocoa 的 `[NSEvent mouseLocation]` 读全局鼠标位置。
        //
        // 走过的两个坑（教训已写入项目记忆）：
        //  1. core-graphics 0.22 的 `CGEvent::new(source).location()`：source 参数
        //     强制要，所以总是返回「新建事件」、location 默认 (0, 0)，不是真实光标。
        //  2. 裸 C API `CGEventCreate(NULL).location`：Apple 文档明确说
        //     "The location of a newly created event is (0, 0)"，和 (1) 同病。
        //     b9e7d57 的修复就是这条错路。
        //
        // 正确方法：`[NSEvent mouseLocation]`（AppKit，Cocoa 类方法），不需要
        // NSApplication 也不需要任何 TCC 授权。需要在 extern 块上挂
        // `#[link(name = "AppKit", kind = "framework")]` 让链接器拉入 AppKit。
        // CGPoint (16 字节，#[repr(C)] { f64, f64 }) 在 x86_64 SysV 是 SSE 类返回
        // 在 xmm0/xmm1、在 arm64 是寄存器返回，均与 Rust extern "C" 返回 CGPoint
        // 的 ABI 一致，无需 objc_msgSend_stret。
        #[link(name = "AppKit", kind = "framework")]
        extern "C" {
            fn objc_getClass(name: *const i8) -> *const std::ffi::c_void;
            fn sel_registerName(name: *const i8) -> *const std::ffi::c_void;
            fn objc_msgSend(receiver: *const std::ffi::c_void, sel: *const std::ffi::c_void) -> CGPoint;
        }
        unsafe {
            let class = objc_getClass(b"NSEvent\0".as_ptr() as *const i8);
            if class.is_null() {
                return (0, 0);
            }
            let sel = sel_registerName(b"mouseLocation\0".as_ptr() as *const i8);
            if sel.is_null() {
                return (0, 0);
            }
            let p = objc_msgSend(class, sel);
            (p.x as i32, p.y as i32)
        }
    }

    pub fn set_cursor_pos(x: i32, y: i32) {
        let p = CGPoint::new(x as f64, y as f64);
        // CGWarpMouseCursorPosition 需要辅助功能/输入监控授权（TCC）；
        // 未授权时返回非 0 错误码，这里忽略。
        let _ = CGDisplay::warp_mouse_cursor_position(p);
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
mod imp {
    pub fn screen_size() -> (u32, u32) {
        (0, 0)
    }
    pub fn get_cursor_pos() -> (i32, i32) {
        (0, 0)
    }
    pub fn set_cursor_pos(_x: i32, _y: i32) {}
}

pub use imp::*;
