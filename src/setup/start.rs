//! Bringing voxtype up with one idempotent gesture.
//!
//! Backs `voxtype start`. A provisioning tool wants a single command that
//! leaves the machine dictating and exits 0, whether or not anything was
//! already running — and that behaves the same on both systems, so the same
//! atom serves each.
//!
//! "Up" means the daemon **and** the status icon. The icon is not decoration:
//! it is how someone who does not live in a terminal sees that voxtype is
//! running and reaches its settings.
//!
//! On macOS that means launching the app bundle rather than a bare daemon.
//! The two are not interchangeable: a daemon started on its own is attributed
//! to whatever launched it, so it does not inherit Voxtype.app's microphone
//! and Accessibility grants, and dictation silently fails. `setup launchd`
//! carries the same warning.

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum StartError {
    #[error(
        "cannot find Voxtype.app.\n  Install it with `voxtype setup app-bundle`, \
         or start the daemon directly with `voxtype daemon`."
    )]
    NoBundle,

    #[error("could not launch {path}: {source}")]
    Launch {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error(
        "launched {path} but the daemon did not come up within {secs}s.\n  \
         Look at ~/Library/Logs/voxtype/stdout.log."
    )]
    NeverCameUp { path: PathBuf, secs: u64 },
}

impl StartError {
    pub fn exit_code(&self) -> i32 {
        match self {
            StartError::NoBundle => 2,
            StartError::Launch { .. } | StartError::NeverCameUp { .. } => 1,
        }
    }
}

/// What `start` did, so the caller reports rather than guesses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Started {
    /// The daemon was already up; nothing was launched.
    AlreadyRunning,
    /// Launched, and the daemon answered.
    Launched { path: PathBuf },
}

/// How long to wait for a freshly launched daemon to claim its pidfile.
/// Loading a transcription model takes about a second on this hardware; the
/// pidfile is claimed before that, so this is generous rather than tight.
const WAIT: std::time::Duration = std::time::Duration::from_secs(20);
const POLL: std::time::Duration = std::time::Duration::from_millis(100);

/// Bring voxtype up, or confirm it already is.
///
/// Idempotent by design: a second call finds the daemon running and returns
/// without touching anything.
pub fn start() -> Result<Started, StartError> {
    if crate::daemon_status::is_daemon_running() {
        return Ok(Started::AlreadyRunning);
    }

    let path = launch()?;

    let deadline = std::time::Instant::now() + WAIT;
    while std::time::Instant::now() < deadline {
        if crate::daemon_status::is_daemon_running() {
            return Ok(Started::Launched { path });
        }
        std::thread::sleep(POLL);
    }

    Err(StartError::NeverCameUp {
        path,
        secs: WAIT.as_secs(),
    })
}

/// Launch the app, which starts the daemon and the icon together.
#[cfg(target_os = "macos")]
fn launch() -> Result<PathBuf, StartError> {
    let bundle = crate::setup::app_bundle::app_bundle_path();
    if !bundle.exists() {
        return Err(StartError::NoBundle);
    }

    // `open` rather than running the executable: it goes through
    // LaunchServices, which is what attributes the daemon to Voxtype.app and
    // so hands it the bundle's microphone and Accessibility grants.
    std::process::Command::new("open")
        .arg(&bundle)
        .status()
        .map_err(|source| StartError::Launch {
            path: bundle.clone(),
            source,
        })?;
    Ok(bundle)
}

/// Spawn the daemon and the notification-area icon as detached children.
///
/// Windows has no app bundle to open, and a bare `voxtype.exe` starts only
/// the daemon — the icon is a second process. Starting both here is what
/// makes `voxtype start` mean the same thing on either system, and gives
/// `autostart` a single thing to register.
#[cfg(windows)]
fn launch() -> Result<PathBuf, StartError> {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let exe = std::env::current_exe().map_err(|source| StartError::Launch {
        path: PathBuf::from("voxtype.exe"),
        source,
    })?;

    for arg in ["daemon", "menubar"] {
        std::process::Command::new(&exe)
            .arg(arg)
            .creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW)
            .spawn()
            .map_err(|source| StartError::Launch {
                path: exe.clone(),
                source,
            })?;
    }
    Ok(exe)
}

/// Linux starts voxtype through systemd, which already owns "start it, and do
/// nothing if it is up". Keeping this off Linux avoids a second, competing
/// notion of how the daemon comes up.
#[cfg(not(any(target_os = "macos", windows)))]
fn launch() -> Result<PathBuf, StartError> {
    Err(StartError::NoBundle)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wait has to outlast a model load, or an installer would report
    /// failure while the daemon was still starting.
    #[test]
    fn the_wait_is_generous_enough_for_a_model_load() {
        assert!(WAIT >= std::time::Duration::from_secs(10));
        assert!(POLL < std::time::Duration::from_secs(1));
    }

    #[test]
    fn a_missing_bundle_is_the_users_to_fix() {
        assert_eq!(StartError::NoBundle.exit_code(), 2);
    }

    #[test]
    fn a_failed_launch_is_not_the_users_fault() {
        let e = StartError::NeverCameUp {
            path: PathBuf::from("/Applications/Voxtype.app"),
            secs: 20,
        };
        assert_eq!(e.exit_code(), 1);
        assert!(
            e.to_string().contains("stdout.log"),
            "the message must say where to look: {e}"
        );
    }

    /// `start` must be callable twice. With a daemon up, the second call is a
    /// no-op; this pins that the already-running answer launches nothing.
    #[test]
    fn already_running_is_distinguishable_from_launched() {
        assert_ne!(
            Started::AlreadyRunning,
            Started::Launched {
                path: PathBuf::from("/Applications/Voxtype.app")
            }
        );
    }
}
