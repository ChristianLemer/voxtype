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
