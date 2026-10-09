//! Windows presenter: a layered window fed from the CPU-drawn card.
//!
//! `UpdateLayeredWindow` takes a premultiplied BGRA bitmap and composites it
//! with per-pixel alpha, without OpenGL or Direct3D, so the card shows in
//! virtual machines and remote sessions too. The window is a non-activating,
//! click-through tool window: it never takes focus and stays off the taskbar.

use std::time::Duration;

use windows_sys::Win32::Foundation::{HWND, POINT, SIZE};
use windows_sys::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, GetMonitorInfoW,
    MonitorFromPoint, ReleaseDC, SelectObject, AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS, MONITORINFO, MONITOR_DEFAULTTOPRIMARY,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::{
    GetDpiForMonitor, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    MDT_EFFECTIVE_DPI,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, PeekMessageW, RegisterClassW, ShowWindow,
    TranslateMessage, UpdateLayeredWindow, MSG, PM_REMOVE, SW_HIDE, SW_SHOWNOACTIVATE, ULW_ALPHA,
    WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
    WS_POPUP,
};

use super::card::GLOW_PAD;
use super::{Model, STATE_POLL};

const FRAME: Duration = Duration::from_millis(16);

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Primary monitor rectangle (physical pixels) and its scale factor.
fn primary_monitor() -> (i32, i32, i32, i32, f32) {
    // SAFETY: plain queries with stack-allocated out parameters.
    unsafe {
        let monitor = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        GetMonitorInfoW(monitor, &mut info);
        let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
        GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
        let r = info.rcMonitor;
        (
            r.left,
            r.top,
            r.right - r.left,
            r.bottom - r.top,
            dpi_x as f32 / 96.0,
        )
    }
}

fn create_window() -> Option<HWND> {
    let class = wide("VoxtypeOsdCard");
    // SAFETY: Win32 window creation with NUL-terminated names that outlive
    // the calls.
    unsafe {
        let instance = GetModuleHandleW(std::ptr::null());
        let wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(DefWindowProcW),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance,
            hIcon: std::ptr::null_mut(),
            hCursor: std::ptr::null_mut(),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: class.as_ptr(),
        };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class.as_ptr(),
            class.as_ptr(),
            WS_POPUP,
            0,
            0,
            1,
            1,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        );
        (!hwnd.is_null()).then_some(hwnd)
    }
}

/// Hand a premultiplied RGBA pixmap to the layered window at `(x, y)`.
fn present(hwnd: HWND, pixmap: &tiny_skia::Pixmap, x: i32, y: i32) {
    let (w, h) = (pixmap.width() as i32, pixmap.height() as i32);
    // SAFETY: the DIB section is sized to the pixmap and written within
    // bounds; every GDI object is released before returning.
    unsafe {
        let screen = GetDC(std::ptr::null_mut());
        let mem = CreateCompatibleDC(screen);
        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h, // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            biSizeImage: 0,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let bitmap = CreateDIBSection(
            screen,
            &info,
            DIB_RGB_COLORS,
            &mut bits,
            std::ptr::null_mut(),
            0,
        );
        if !bitmap.is_null() && !bits.is_null() {
            // tiny-skia stores premultiplied RGBA; GDI wants premultiplied BGRA.
            let dst = std::slice::from_raw_parts_mut(bits as *mut u8, (w * h * 4) as usize);
            for (d, s) in dst.chunks_exact_mut(4).zip(pixmap.data().chunks_exact(4)) {
                d[0] = s[2];
                d[1] = s[1];
                d[2] = s[0];
                d[3] = s[3];
            }
            let old = SelectObject(mem, bitmap);
            let position = POINT { x, y };
            let size = SIZE { cx: w, cy: h };
            let origin = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            UpdateLayeredWindow(
                hwnd, screen, &position, &size, mem, &origin, 0, &blend, ULW_ALPHA,
            );
            SelectObject(mem, old);
            DeleteObject(bitmap);
        }
        DeleteDC(mem);
        ReleaseDC(std::ptr::null_mut(), screen);
    }
}

fn pump_messages() {
    // SAFETY: standard message pump for windows owned by this thread.
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

pub fn run(mut model: Model) -> anyhow::Result<()> {
    // Draw at the monitor's real resolution instead of letting Windows
    // stretch a 96-dpi bitmap.
    // SAFETY: process-wide setting made before any window exists.
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };

    let hwnd = create_window().ok_or_else(|| {
        anyhow::anyhow!(
            "cannot create the OSD window: {}",
            std::io::Error::last_os_error()
        )
    })?;
    let mut shown = false;

    loop {
        pump_messages();
        if !model.poll() {
            if shown {
                // SAFETY: hiding our own window.
                unsafe { ShowWindow(hwnd, SW_HIDE) };
                shown = false;
            }
            std::thread::sleep(STATE_POLL);
            continue;
        }

        // Re-read each frame: monitors and scaling can change under us.
        let (mx, my, mw, mh, scale) = primary_monitor();
        if let Some(pixmap) = model.render(scale) {
            let x = mx + (mw - pixmap.width() as i32) / 2;
            let top = model.config.top_margin.clamp(0.0, 1.0) * mh as f32 - GLOW_PAD * scale;
            let bottom_limit =
                mh as f32 - pixmap.height() as f32 - model.config.margin_px as f32 * scale;
            let y = my + top.min(bottom_limit) as i32;
            present(hwnd, &pixmap, x, y);
            if !shown {
                // SAFETY: showing our own window without activating it.
                unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
                shown = true;
            }
        }
        std::thread::sleep(FRAME);
    }
}
