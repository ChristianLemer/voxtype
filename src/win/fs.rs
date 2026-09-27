//! Stand-in for the `std::os::unix::fs` items used by the Linux setup and
//! output-driver code.
//!
//! Those code paths manage `/usr/lib/voxtype` symlinks, FIFOs and sockets that
//! do not exist on Windows: file-type probes answer "no", permission bits are
//! ignored, and symlink creation fails instead of pretending to succeed.

use std::io;
use std::path::Path;

pub fn symlink(_original: impl AsRef<Path>, _link: impl AsRef<Path>) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "symlink management is Linux-only",
    ))
}

pub trait FileTypeExt {
    fn is_fifo(&self) -> bool;
    fn is_socket(&self) -> bool;
}

impl FileTypeExt for std::fs::FileType {
    fn is_fifo(&self) -> bool {
        false
    }

    fn is_socket(&self) -> bool {
        false
    }
}

pub trait PermissionsExt {
    fn set_mode(&mut self, mode: u32);
}

impl PermissionsExt for std::fs::Permissions {
    fn set_mode(&mut self, _mode: u32) {}
}
