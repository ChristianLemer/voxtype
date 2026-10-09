//! Stand-in for `tokio::signal::unix`.
//!
//! `terminate` maps to Ctrl+C / console close. The two user-defined signals,
//! which carry `voxtype record start|stop` on Unix, arrive over a control
//! named pipe instead: the CLI writes one byte (see [`send_user_signal`]) and
//! the daemon's control server wakes the matching [`Signal`].

use std::io;
use std::path::PathBuf;
use std::sync::Once;
use tokio::io::AsyncReadExt;
use tokio::sync::Notify;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignalKind(Kind);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    User1,
    User2,
    Terminate,
}

impl SignalKind {
    pub fn user_defined1() -> Self {
        Self(Kind::User1)
    }

    pub fn user_defined2() -> Self {
        Self(Kind::User2)
    }

    pub fn terminate() -> Self {
        Self(Kind::Terminate)
    }
}

/// Wire values of the control pipe, matching `libc_compat::SIGUSR1/2`.
pub const CONTROL_USER1: u8 = 10;
pub const CONTROL_USER2: u8 = 12;

static USER1: Notify = Notify::const_new();
static USER2: Notify = Notify::const_new();
static CONTROL_SERVER: Once = Once::new();

fn control_path() -> PathBuf {
    crate::config::Config::runtime_dir().join("control")
}

/// Serve the control pipe for the life of the daemon.
fn start_control_server() {
    CONTROL_SERVER.call_once(|| {
        let listener = match super::net::UnixListener::bind(control_path()) {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!(
                    "Could not open the control pipe; 'voxtype record' commands will not reach this daemon: {}",
                    e
                );
                return;
            }
        };
        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((mut stream, _)) => {
                        let mut byte = [0u8; 1];
                        if stream.read_exact(&mut byte).await.is_ok() {
                            match byte[0] {
                                CONTROL_USER1 => USER1.notify_one(),
                                CONTROL_USER2 => USER2.notify_one(),
                                other => tracing::debug!("Ignoring control byte {}", other),
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Control pipe accept error: {}", e);
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }
                }
            }
        });
    });
}

/// Deliver a user signal to the running daemon. Blocking; used by the CLI.
pub fn send_user_signal(value: u8) -> io::Result<()> {
    use std::io::Write;
    let mut pipe = super::net::connect_blocking(&control_path())?;
    pipe.write_all(&[value])
}

pub struct Signal {
    kind: Kind,
    ctrl_c: Option<tokio::signal::windows::CtrlC>,
    ctrl_close: Option<tokio::signal::windows::CtrlClose>,
}

pub fn signal(kind: SignalKind) -> io::Result<Signal> {
    let (ctrl_c, ctrl_close) = match kind.0 {
        Kind::Terminate => (
            Some(tokio::signal::windows::ctrl_c()?),
            Some(tokio::signal::windows::ctrl_close()?),
        ),
        Kind::User1 | Kind::User2 => {
            start_control_server();
            (None, None)
        }
    };
    Ok(Signal {
        kind: kind.0,
        ctrl_c,
        ctrl_close,
    })
}

impl Signal {
    pub async fn recv(&mut self) -> Option<()> {
        match (self.kind, &mut self.ctrl_c, &mut self.ctrl_close) {
            (Kind::Terminate, Some(c), Some(close)) => tokio::select! {
                r = c.recv() => r,
                r = close.recv() => r,
            },
            (Kind::User1, _, _) => {
                USER1.notified().await;
                Some(())
            }
            (Kind::User2, _, _) => {
                USER2.notified().await;
                Some(())
            }
            _ => std::future::pending().await,
        }
    }
}
