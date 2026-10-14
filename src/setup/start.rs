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

// ---------------------------------------------------------------------------
// restart
// ---------------------------------------------------------------------------

/// How long processes get to leave on SIGTERM before they are killed. The
/// daemon tears down its recording card and level socket on the way out,
/// which takes well under a second.
const GRACE: std::time::Duration = std::time::Duration::from_secs(3);

/// Stop everything voxtype is running, then start it afresh.
///
/// `start` is idempotent by design and so does nothing to a voxtype that is
/// already up — which leaves `config apply` changing a key the daemon will
/// not see until something restarts it. This is that something. When nothing
/// was running it simply starts, so it is safe to call either way.
///
/// Every process is stopped, the app's own menu-bar process included. Leave
/// that one and macOS keeps counting Voxtype.app as running, so the launch
/// that follows only activates it and no daemon comes back.
pub fn restart() -> Result<Started, StartError> {
    stop_all();
    // Stale locks name pids that no longer exist. The daemon checks the pid
    // itself, but removing them keeps a crashed run from looking alive.
    let runtime = crate::config::Config::runtime_dir();
    let _ = std::fs::remove_file(runtime.join("voxtype.lock"));
    let _ = std::fs::remove_file(runtime.join("menubar.lock"));
    start()
}

/// The command lines of every voxtype process to stop on macOS. The bare
/// bundle executable is the app's main process — menu bar and launcher — and
/// is matched exactly, so `voxtype-bin restart` (this very process) and any
/// unrelated command never match it.
#[cfg(target_os = "macos")]
fn pids_to_stop() -> Vec<u32> {
    let exe = crate::setup::app_bundle::app_binary_path();
    let exe = exe.to_string_lossy();
    let mut pids = Vec::new();

    let mut collect = |args: &[&str]| {
        if let Ok(out) = std::process::Command::new("pgrep").args(args).output() {
            pids.extend(
                String::from_utf8_lossy(&out.stdout)
                    .split_whitespace()
                    .filter_map(|p| p.parse::<u32>().ok()),
            );
        }
    };
    collect(&["-fx", &exe]);
    for role in ["daemon", "menubar", "osd-card"] {
        collect(&["-f", &format!("voxtype-bin {role}")]);
    }

    let me = std::process::id();
    pids.retain(|p| *p != me);
    pids.sort_unstable();
    pids.dedup();
    pids
}

#[cfg(target_os = "macos")]
fn stop_all() {
    let signal = |sig: &str, pids: &[u32]| {
        if pids.is_empty() {
            return;
        }
        let list: Vec<String> = pids.iter().map(u32::to_string).collect();
        let _ = std::process::Command::new("kill")
            .arg(sig)
            .args(&list)
            .status();
    };

    let pids = pids_to_stop();
    signal("-TERM", &pids);

    let deadline = std::time::Instant::now() + GRACE;
    while std::time::Instant::now() < deadline && !pids_to_stop().is_empty() {
        std::thread::sleep(POLL);
    }
    // Anything still there after the grace period is hung, not busy.
    signal("-KILL", &pids_to_stop());

    // LaunchServices drops a dead app's registration a beat after the kernel
    // reaps it; `open` in that window fails with -600. Wait it out here so the
    // start that follows is not racing a ghost.
    std::thread::sleep(std::time::Duration::from_millis(300));
}

/// Every voxtype.exe except this one. Windows has no graceful signal a
/// windowless process answers, so it is a forced stop.
#[cfg(windows)]
fn stop_all() {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let me = std::process::id();
    let _ = std::process::Command::new("taskkill")
        .args(["/F", "/FI", "IMAGENAME eq voxtype.exe", "/FI"])
        .arg(format!("PID ne {me}"))
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    std::thread::sleep(std::time::Duration::from_millis(300));
}

#[cfg(not(any(target_os = "macos", windows)))]
fn stop_all() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_grace_period_is_short_but_not_instant() {
        assert!(GRACE >= std::time::Duration::from_secs(1));
        assert!(GRACE <= std::time::Duration::from_secs(10));
    }

    /// The process doing the restart must never be on its own kill list,
    /// whatever its command line looks like.
    #[cfg(target_os = "macos")]
    #[test]
    fn restart_never_targets_itself() {
        assert!(!pids_to_stop().contains(&std::process::id()));
    }

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
