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
        // 关键：core-graphics 0.22 没有 `CGEvent::mouseLocation()` 静态方法。
        // `CGEvent::new(source)` 强制要 source，源码注释也写明返回的是「新事件，
        // 默认 location (0, 0)」——所以之前用它读到的一直是 (0, 0)，Mac 监控线程
        // 永远不解锁 armed，M3 真机只能往返一次。
        //
        // 正确做法：直接调 C API `CGEventCreate(NULL)`，传 NULL 时 CoreGraphics
        // 会捕获**当前系统状态**，事件里的 location 就是真实光标位置。
        extern "C" {
            fn CGEventCreate(source: *const std::ffi::c_void) -> *const std::ffi::c_void;
            fn CGEventGetLocation(event: *const std::ffi::c_void) -> CGPoint;
            fn CFRelease(cf: *const std::ffi::c_void);
        }
        unsafe {
            let event = CGEventCreate(std::ptr::null());
            if event.is_null() {
                return (0, 0);
            }
            let p = CGEventGetLocation(event);
            // CGEventCreate 返回的是 +1 retain，必须手动释放。
            CFRelease(event);
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
