//! Process liveness without `kill(pid, 0)`.

use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};

pub fn is_alive(pid: i32) -> bool {
    let Ok(pid) = u32::try_from(pid) else {
        return false;
    };
    // SAFETY: the handle is checked for null and closed on every path.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        let mut code = 0u32;
        let ok = GetExitCodeProcess(handle, &mut code) != 0;
        CloseHandle(handle);
        ok && code == STILL_ACTIVE as u32
    }
}

/// Stand-in for `std::os::unix::process::CommandExt::exec`.
///
/// Windows cannot replace the current process image, so the child runs to
/// completion and this process exits with its code. Like `exec`, it only
/// returns on failure to start the child.
pub trait CommandExt {
    fn exec(&mut self) -> std::io::Error;
}

impl CommandExt for std::process::Command {
    fn exec(&mut self) -> std::io::Error {
        match self.status() {
            Ok(status) => std::process::exit(status.code().unwrap_or(1)),
            Err(e) => e,
        }
    }
}

/// Held for the life of the process to keep other instances out.
pub struct InstanceGuard(Option<windows_sys::Win32::Foundation::HANDLE>);

// SAFETY: a mutex handle is a process-wide kernel object reference.
unsafe impl Send for InstanceGuard {}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        if let Some(handle) = self.0 {
            // SAFETY: the handle came from CreateMutexW and is closed once.
            unsafe { CloseHandle(handle) };
        }
    }
}

/// Claim the per-session named mutex `name`, or `None` if another process
/// holds it.
///
/// The PID lockfile cannot see across elevation: a process started from an
/// administrator shell looks dead to one started from Explorer, so both ran and
/// typed every dictation twice. A mutex created elevated is visible to a
/// non-elevated process of the same session; opening it then fails with
/// access denied, which counts as taken too.
pub fn single_instance(name: &str) -> Option<InstanceGuard> {
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::CreateMutexW;

    let wide: Vec<u16> = format!("Local\\{}", name)
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: `wide` is NUL-terminated and outlives the call.
    unsafe {
        let handle = CreateMutexW(std::ptr::null(), 0, wide.as_ptr());
        let err = GetLastError();
        if handle.is_null() {
            if err == ERROR_ACCESS_DENIED {
                return None;
            }
            // Fall back to the lockfile alone rather than refuse to start.
            tracing::warn!("Could not create instance mutex {}: error {}", name, err);
            return Some(InstanceGuard(None));
        }
        if err == ERROR_ALREADY_EXISTS {
            CloseHandle(handle);
            return None;
        }
        Some(InstanceGuard(Some(handle)))
    }
}
