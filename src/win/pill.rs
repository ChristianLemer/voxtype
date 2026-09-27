//! Floating status pill for Windows.
//!
//! A small rounded window above the taskbar, centred, shown while recording or
//! transcribing and hidden otherwise. It must never take focus: voxtype types
//! into whatever window is active, so the pill is a non-activating, click-through
//! tool window (`WS_EX_NOACTIVATE | WS_EX_TRANSPARENT`, no taskbar entry).
//!
//! It is created on the menubar's event-loop thread, whose message loop also
//! dispatches the pill's paint messages.

use std::sync::atomic::{AtomicU8, Ordering};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreateRoundRectRgn, CreateSolidBrush, DeleteObject, DrawTextW,
    Ellipse, EndPaint, FillRect, GetStockObject, InvalidateRect, SelectObject, SetBkMode,
    SetTextColor, SetWindowRgn, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DT_LEFT,
    DT_SINGLELINE, DT_VCENTER, FF_SWISS, FW_SEMIBOLD, NULL_PEN, OUT_DEFAULT_PRECIS, PAINTSTRUCT,
    TRANSPARENT, VARIABLE_PITCH,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, RegisterClassW, SetLayeredWindowAttributes, SetWindowPos,
    ShowWindow, SystemParametersInfoW, HWND_TOPMOST, LWA_ALPHA, MA_NOACTIVATE, SPI_GETWORKAREA,
    SWP_NOACTIVATE, SWP_SHOWWINDOW, SW_HIDE, WM_MOUSEACTIVATE, WM_PAINT, WNDCLASSW, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

const WIDTH: i32 = 190;
const HEIGHT: i32 = 44;
/// Gap between the pill and the top of the taskbar.
const MARGIN: i32 = 48;
const OPACITY: u8 = 235;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PillState {
    Hidden = 0,
    Recording = 1,
    Transcribing = 2,
}

/// Read by the window procedure, which has no other way to reach the pill.
static SHOWN: AtomicU8 = AtomicU8::new(PillState::Hidden as u8);

pub struct Pill {
    hwnd: HWND,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    r as u32 | (g as u32) << 8 | (b as u32) << 16
}

impl Pill {
    /// Create the (hidden) pill window, or `None` if Windows refuses.
    pub fn new() -> Option<Self> {
        let class = wide("VoxtypeStatusPill");
        // SAFETY: plain Win32 window creation; every pointer passed outlives
        // the call and the class name is NUL-terminated.
        unsafe {
            let instance = GetModuleHandleW(std::ptr::null());
            let wc = WNDCLASSW {
                style: 0,
                lpfnWndProc: Some(window_proc),
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
                WS_EX_LAYERED
                    | WS_EX_TOPMOST
                    | WS_EX_TOOLWINDOW
                    | WS_EX_NOACTIVATE
                    | WS_EX_TRANSPARENT,
                class.as_ptr(),
                class.as_ptr(),
                WS_POPUP,
                0,
                0,
                WIDTH,
                HEIGHT,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                instance,
                std::ptr::null(),
            );
            if hwnd.is_null() {
                return None;
            }
            SetLayeredWindowAttributes(hwnd, 0, OPACITY, LWA_ALPHA);
            // The window takes ownership of the region.
            let region = CreateRoundRectRgn(0, 0, WIDTH + 1, HEIGHT + 1, HEIGHT, HEIGHT);
            SetWindowRgn(hwnd, region, 0);
            Some(Self { hwnd })
        }
    }

    pub fn set(&self, state: PillState) {
        SHOWN.store(state as u8, Ordering::SeqCst);
        // SAFETY: `self.hwnd` is a live window owned by this thread.
        unsafe {
            if state == PillState::Hidden {
                ShowWindow(self.hwnd, SW_HIDE);
                return;
            }
            // Re-read the work area each time: the taskbar or resolution may
            // have changed since the last recording.
            let mut area: RECT = std::mem::zeroed();
            SystemParametersInfoW(
                SPI_GETWORKAREA,
                0,
                &mut area as *mut RECT as *mut core::ffi::c_void,
                0,
            );
            let x = area.left + (area.right - area.left - WIDTH) / 2;
            let y = area.bottom - HEIGHT - MARGIN;
            SetWindowPos(
                self.hwnd,
                HWND_TOPMOST,
                x,
                y,
                WIDTH,
                HEIGHT,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            InvalidateRect(self.hwnd, std::ptr::null(), 1);
        }
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_PAINT => {
            // SAFETY: called by the system for our own window during WM_PAINT.
            unsafe { paint(hwnd) };
            0
        }
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        // SAFETY: default handling for everything else.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

unsafe fn paint(hwnd: HWND) {
    let (dot, label) = match SHOWN.load(Ordering::SeqCst) {
        1 => (rgb(0xe0, 0x30, 0x30), "Recording…"),
        2 => (rgb(0xf0, 0xa0, 0x20), "Transcribing…"),
        _ => (rgb(0x80, 0x80, 0x80), ""),
    };

    // SAFETY: every GDI object created here is selected out and deleted
    // before returning; BeginPaint/EndPaint are paired.
    unsafe {
        let mut ps: PAINTSTRUCT = std::mem::zeroed();
        let hdc = BeginPaint(hwnd, &mut ps);

        let background = CreateSolidBrush(rgb(0x1f, 0x1f, 0x1f));
        let full = RECT {
            left: 0,
            top: 0,
            right: WIDTH,
            bottom: HEIGHT,
        };
        FillRect(hdc, &full, background);
        DeleteObject(background);

        let dot_brush = CreateSolidBrush(dot);
        let old_brush = SelectObject(hdc, dot_brush);
        let old_pen = SelectObject(hdc, GetStockObject(NULL_PEN));
        let d = 14;
        let cy = HEIGHT / 2;
        Ellipse(hdc, 18, cy - d / 2, 18 + d, cy + d / 2);
        SelectObject(hdc, old_pen);
        SelectObject(hdc, old_brush);
        DeleteObject(dot_brush);

        let face = wide("Segoe UI");
        let font = CreateFontW(
            -17,
            0,
            0,
            0,
            FW_SEMIBOLD as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_DEFAULT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            CLEARTYPE_QUALITY as u32,
            (VARIABLE_PITCH | FF_SWISS) as u32,
            face.as_ptr(),
        );
        let old_font = SelectObject(hdc, font);
        SetBkMode(hdc, TRANSPARENT as _);
        SetTextColor(hdc, rgb(0xf2, 0xf2, 0xf2));
        let mut text_rect = RECT {
            left: 44,
            top: 0,
            right: WIDTH - 12,
            bottom: HEIGHT,
        };
        let text = wide(label);
        DrawTextW(
            hdc,
            text.as_ptr(),
            -1,
            &mut text_rect,
            DT_SINGLELINE | DT_VCENTER | DT_LEFT,
        );
        SelectObject(hdc, old_font);
        DeleteObject(font);

        EndPaint(hwnd, &ps);
    }
}
