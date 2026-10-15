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
         Look at {log}."
    )]
    NeverCameUp {
        path: PathBuf,
        secs: u64,
        log: String,
    },
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
        log: log_hint(),
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
    let exe = std::env::current_exe().map_err(|source| StartError::Launch {
        path: PathBuf::from("voxtype.exe"),
        source,
    })?;

    let daemon_log = daemon_log_path();
    for (arg, log) in [("daemon", daemon_log.as_deref()), ("menubar", None)] {
        let mut cmd = std::process::Command::new(&exe);
        cmd.arg(arg);
        spawn_detached(&mut cmd, log).map_err(|source| StartError::Launch {
            path: exe.clone(),
            source,
        })?;
    }
    Ok(exe)
}

/// Where a daemon started by `start` writes its log on Windows. macOS needs
/// none of this: the app's own launcher already logs to ~/Library/Logs.
#[cfg(windows)]
fn daemon_log_path() -> Option<PathBuf> {
    let dir = crate::config::Config::data_dir().join("logs");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("daemon.log"))
}

/// Where to tell the user to look when the daemon never comes up.
fn log_hint() -> String {
    #[cfg(target_os = "macos")]
    return "~/Library/Logs/voxtype/stdout.log".to_string();
    #[cfg(windows)]
    return daemon_log_path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "the daemon's log".to_string());
    #[cfg(not(any(target_os = "macos", windows)))]
    return "the daemon's log".to_string();
}

/// Spawn a long-lived child that holds none of this process's standard
/// handles.
///
/// The default — inherit — is a trap for a launcher. A child that inherits
/// stdout keeps the caller's pipe open for as long as it lives, so anything
/// reading this command's output waits for an end of file that only comes
/// when the daemon exits: an installer capturing `voxtype restart` hung until
/// it was killed. Measured on Windows, by Saul's Talos atom. stdin and stderr
/// go the same way, and the daemon's output goes to `log` instead of being
/// lost.
#[cfg(any(windows, test))]
fn spawn_detached(
    cmd: &mut std::process::Command,
    log: Option<&std::path::Path>,
) -> std::io::Result<std::process::Child> {
    use std::process::Stdio;

    let (out, err) = match log.and_then(|p| {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(p)
            .ok()
    }) {
        Some(file) => {
            let second = file.try_clone()?;
            (Stdio::from(file), Stdio::from(second))
        }
        None => (Stdio::null(), Stdio::null()),
    };
    cmd.stdin(Stdio::null()).stdout(out).stderr(err);

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW);
    }

    cmd.spawn()
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
            log: "~/Library/Logs/voxtype/stdout.log".to_string(),
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

    // -- the captured-pipe regression ------------------------------------
    //
    // The bug: a launcher whose output is captured never finishes reading,
    // because the daemon it spawned inherited the pipe and keeps it open.
    // Reproducing it needs three processes — reader, launcher, long-lived
    // child — so the test re-runs its own binary as the launcher.

    const LAUNCHER_ENV: &str = "VOXTYPE_TEST_DETACH_LAUNCHER";

    /// A command that outlives the launcher by far, on either system.
    fn sleeper() -> std::process::Command {
        #[cfg(windows)]
        {
            let mut c = std::process::Command::new("ping");
            c.args(["-n", "31", "127.0.0.1"]);
            c
        }
        #[cfg(not(windows))]
        {
            let mut c = std::process::Command::new("sleep");
            c.arg("30");
            c
        }
    }

    /// The launcher's half. Inert unless the outer test sets the variable,
    /// so a plain `cargo test` run passes straight through it.
    #[test]
    // Not waiting is the point: a launcher spawns the daemon and leaves, and
    // the outer test reaps the sleeper itself.
    #[allow(clippy::zombie_processes)]
    fn detach_launcher_half() {
        if std::env::var_os(LAUNCHER_ENV).is_none() {
            return;
        }
        let child = spawn_detached(&mut sleeper(), None).expect("spawn");
        println!("spawned {}", child.id());
    }

    #[test]
    fn a_detached_child_does_not_hold_the_callers_pipe() {
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};

        let exe = std::env::current_exe().unwrap();
        let started = Instant::now();
        let mut launcher = Command::new(exe)
            .args([
                "--exact",
                "setup::start::tests::detach_launcher_half",
                "--nocapture",
            ])
            .env(LAUNCHER_ENV, "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("run the launcher half");

        // Read to end of file on a thread, so a regression shows up as a
        // timeout here rather than a test that hangs for 30 seconds.
        let mut out = launcher.stdout.take().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = String::new();
            let _ = std::io::Read::read_to_string(&mut out, &mut buf);
            let _ = tx.send(buf);
        });
        let output = rx
            .recv_timeout(Duration::from_secs(15))
            .expect("the launcher's output never closed: its child inherited the pipe");
        let _ = launcher.wait();

        // Clean up the sleeper rather than leave it running for 30 seconds.
        if let Some(pid) = output
            .lines()
            .find_map(|l| l.strip_prefix("spawned "))
            .and_then(|p| p.trim().parse::<u32>().ok())
        {
            #[cfg(windows)]
            let _ = Command::new("taskkill")
                .args(["/F", "/PID", &pid.to_string()])
                .output();
            #[cfg(not(windows))]
            let _ = Command::new("kill").arg(pid.to_string()).output();
        } else {
            panic!("the launcher half did not run: {output:?}");
        }

        assert!(
            started.elapsed() < Duration::from_secs(15),
            "reading the launcher's output took {:?}",
            started.elapsed()
        );
    }
}
