//! Stand-in for `tokio::net::{UnixListener, UnixStream}` over named pipes.
//!
//! Callers keep passing the filesystem path they would bind on Unix; the pipe
//! name is derived from it, so server and client agree without sharing
//! anything else, and the per-user runtime directory in the path keeps two
//! users' pipes apart. No file is created at that path.

use std::io;
use std::path::Path;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::windows::named_pipe::{
    ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions,
};

/// All pipe instances are busy, or the server is between two instances.
const ERROR_PIPE_BUSY: i32 = 231;
const ERROR_FILE_NOT_FOUND: i32 = 2;

/// Pipe name for a Unix-style socket path.
pub fn pipe_name(path: &Path) -> String {
    let flat: String = path
        .to_string_lossy()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    format!(r"\\.\pipe\voxtype{}", flat)
}

fn server_instance(name: &str, first: bool) -> io::Result<NamedPipeServer> {
    ServerOptions::new()
        .first_pipe_instance(first)
        .reject_remote_clients(true)
        .create(name)
}

pub struct UnixListener {
    name: String,
    /// The instance the next client will connect to.
    pending: tokio::sync::Mutex<NamedPipeServer>,
}

impl UnixListener {
    /// Fails if another live process already serves this name.
    pub fn bind(path: impl AsRef<Path>) -> io::Result<Self> {
        let name = pipe_name(path.as_ref());
        let pending = tokio::sync::Mutex::new(server_instance(&name, true)?);
        Ok(Self { name, pending })
    }

    pub async fn accept(&self) -> io::Result<(UnixStream, ())> {
        let mut pending = self.pending.lock().await;
        pending.connect().await?;
        // Open the next instance before handing this one out, so clients
        // arriving meanwhile find a pipe to connect to.
        let next = server_instance(&self.name, false)?;
        let connected = std::mem::replace(&mut *pending, next);
        Ok((UnixStream::Server(connected), ()))
    }
}

pub enum UnixStream {
    Server(NamedPipeServer),
    Client(NamedPipeClient),
}

impl UnixStream {
    pub async fn connect(path: impl AsRef<Path>) -> io::Result<Self> {
        let name = pipe_name(path.as_ref());
        let mut attempts = 0;
        loop {
            match ClientOptions::new().open(&name) {
                Ok(client) => return Ok(Self::Client(client)),
                Err(e)
                    if attempts < 40
                        && matches!(
                            e.raw_os_error(),
                            Some(ERROR_PIPE_BUSY) | Some(ERROR_FILE_NOT_FOUND)
                        ) =>
                {
                    attempts += 1;
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
                Err(e) => return Err(e),
            }
        }
    }
}

/// Blocking client connect for callers outside a Tokio runtime (the CLI).
/// Retries briefly while the server is between two instances.
pub fn connect_blocking(path: &Path) -> io::Result<std::fs::File> {
    let name = pipe_name(path);
    let mut attempts = 0;
    loop {
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&name)
        {
            Ok(file) => return Ok(file),
            Err(e)
                if attempts < 40
                    && matches!(
                        e.raw_os_error(),
                        Some(ERROR_PIPE_BUSY) | Some(ERROR_FILE_NOT_FOUND)
                    ) =>
            {
                attempts += 1;
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(e) => return Err(e),
        }
    }
}

impl AsyncRead for UnixStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Server(s) => Pin::new(s).poll_read(cx, buf),
            Self::Client(c) => Pin::new(c).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for UnixStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Server(s) => Pin::new(s).poll_write(cx, buf),
            Self::Client(c) => Pin::new(c).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Server(s) => Pin::new(s).poll_flush(cx),
            Self::Client(c) => Pin::new(c).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Server(s) => Pin::new(s).poll_shutdown(cx),
            Self::Client(c) => Pin::new(c).poll_shutdown(cx),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn pipe_name_is_flat_and_stable() {
        let a = pipe_name(Path::new(
            r"C:\Users\x\AppData\Local\voxtype\runtime\audio.sock",
        ));
        assert_eq!(
            a,
            r"\\.\pipe\voxtypec--users-x-appdata-local-voxtype-runtime-audio-sock"
        );
    }

    #[tokio::test]
    async fn round_trip_two_clients() {
        let path = std::env::temp_dir().join(format!("vt-test-{}", std::process::id()));
        let listener = UnixListener::bind(&path).unwrap();
        let server = tokio::spawn(async move {
            for _ in 0..2 {
                let (mut s, _) = listener.accept().await.unwrap();
                s.write_all(b"ping").await.unwrap();
            }
        });
        for _ in 0..2 {
            let mut c = UnixStream::connect(&path).await.unwrap();
            let mut buf = [0u8; 4];
            c.read_exact(&mut buf).await.unwrap();
            assert_eq!(&buf, b"ping");
        }
        server.await.unwrap();
    }
}
