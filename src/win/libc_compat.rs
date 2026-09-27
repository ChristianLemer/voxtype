//! The few `libc` items the CLI uses to signal the daemon, for Windows.
//!
//! Brought into scope as `libc` so the Unix call sites compile unchanged.
//! `kill` reports "not supported" until the named-pipe control channel lands;
//! the built-in hotkey is the way to drive the daemon meanwhile.

#![allow(non_camel_case_types)]

pub type c_int = i32;

pub const SIGUSR1: c_int = 10;
pub const SIGUSR2: c_int = 12;

/// # Safety
/// Always safe; `unsafe` only mirrors the libc signature.
pub unsafe fn kill(_pid: i32, _sig: c_int) -> c_int {
    windows_sys::Win32::Foundation::SetLastError(
        windows_sys::Win32::Foundation::ERROR_NOT_SUPPORTED,
    );
    -1
}

/// There is no root on Windows; report an ordinary user.
///
/// # Safety
/// Always safe; `unsafe` only mirrors the libc signature.
pub unsafe fn getuid() -> u32 {
    1000
}
