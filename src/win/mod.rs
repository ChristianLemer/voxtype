//! Windows stand-ins for the Unix APIs the rest of the crate is written against.
//!
//! Each submodule mirrors the shape of the Unix type or trait it replaces, so a
//! caller only swaps its `use` line under `#[cfg(windows)]` and keeps its body.
//! Unix sockets become named pipes (`net`), and the two user signals travel
//! over a control pipe (`signal`).

pub mod fs;
pub mod libc_compat;
pub mod net;
pub mod pill;
pub mod process;
pub mod signal;
