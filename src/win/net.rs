//! Stand-in for `tokio::net::{UnixListener, UnixStream}`.
//!
//! Binding and connecting fail with `Unsupported`, which every caller already
//! treats as "no OSD feed". The types are uninhabited: no value can exist, so the
//! I/O trait impls are unreachable by construction.

use std::io;
use std::path::Path;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

fn unsupported() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "Unix sockets are not available on Windows",
    )
}

pub enum UnixListener {}

impl UnixListener {
    pub fn bind(_path: impl AsRef<Path>) -> io::Result<Self> {
        Err(unsupported())
    }

    pub async fn accept(&self) -> io::Result<(UnixStream, ())> {
        match *self {}
    }
}

pub enum UnixStream {}

impl UnixStream {
    pub async fn connect(_path: impl AsRef<Path>) -> io::Result<Self> {
        Err(unsupported())
    }
}

impl AsyncRead for UnixStream {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match *self {}
    }
}

impl AsyncWrite for UnixStream {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match *self {}
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match *self {}
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match *self {}
    }
}
