//! Stand-in for `tokio::signal::unix`.
//!
//! `terminate` maps to Ctrl+C / console close. The two user-defined signals
//! carry `voxtype record start|stop` on Unix; Windows has no equivalent, so they
//! never fire until the named-pipe control channel feeds them.

use std::io;

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
        Kind::User1 | Kind::User2 => (None, None),
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
            _ => std::future::pending().await,
        }
    }
}
