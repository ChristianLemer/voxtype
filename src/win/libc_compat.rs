//! The few `libc` items the CLI uses to signal the daemon, for Windows.
//!
//! Brought into scope as `libc` so the Unix call sites compile unchanged.
//! `kill` delivers the two user signals over the daemon's control pipe.

#![allow(non_camel_case_types)]

use super::signal::{send_user_signal, CONTROL_USER1, CONTROL_USER2};

pub type c_int = i32;

pub const SIGUSR1: c_int = CONTROL_USER1 as c_int;
pub const SIGUSR2: c_int = CONTROL_USER2 as c_int;

/// # Safety
/// Always safe; `unsafe` only mirrors the libc signature.
pub unsafe fn kill(_pid: i32, sig: c_int) -> c_int {
    use windows_sys::Win32::Foundation::{SetLastError, ERROR_NOT_SUPPORTED};
    let result = match sig {
        SIGUSR1 | SIGUSR2 => send_user_signal(sig as u8),
        _ => {
            // SAFETY: setting the thread's last-error value.
            unsafe { SetLastError(ERROR_NOT_SUPPORTED) };
            return -1;
        }
    };
    match result {
        Ok(()) => 0,
        Err(e) => {
            if let Some(code) = e.raw_os_error() {
                // SAFETY: setting the thread's last-error value, which the
                // caller reports through io::Error::last_os_error.
                unsafe { SetLastError(code as u32) };
            }
            -1
        }
    }
}

/// There is no root on Windows; report an ordinary user.
///
/// # Safety
/// Always safe; `unsafe` only mirrors the libc signature.
pub unsafe fn getuid() -> u32 {
    1000
}
