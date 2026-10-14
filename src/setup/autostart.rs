//! Starting voxtype at login, without asking anybody anything.
//!
//! Backs `voxtype autostart enable | disable | status`, the third thing an
//! unattended installer needs after dropping files and writing config.
//!
//! What it registers is `voxtype start`, which brings up the daemon *and* the
//! status icon and is idempotent. Registering the daemon alone would be a
//! trap: it would not inherit Voxtype.app's microphone and Accessibility
//! grants, so the machine would boot into a voxtype that looks alive and
//! cannot dictate.
//!
//! **Why not Login Items.** `app_bundle::add_to_login_items` drives System
//! Events over AppleScript, which needs Automation consent — macOS would put
//! a dialog in front of an installer that has nobody to answer it, and the
//! Apply would hang. A LaunchAgent is a plain file plus `launchctl`: no
//! consent, no prompt, and removing the file is the whole of `disable`.
//!
//! On Windows the same role is played by a single `HKCU\…\Run` value, which
//! needs no administrator rights. It is set through `reg.exe` rather than a
//! registry crate, to add no dependency for three invocations.

#[cfg(target_os = "macos")]
use std::path::PathBuf;

/// Whether autostart is registered, for callers that want the answer rather
/// than the printed report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Enabled,
    Disabled,
}

#[derive(Debug, thiserror::Error)]
pub enum AutostartError {
    #[error("could not determine the home directory")]
    NoHome,

    #[error("{context}: {source}")]
    Io {
        context: String,
        source: std::io::Error,
    },

    #[error("autostart is not supported on this platform")]
    Unsupported,
}

impl AutostartError {
    pub fn exit_code(&self) -> i32 {
        match self {
            AutostartError::Unsupported => 2,
            _ => 1,
        }
    }
}

// ---------------------------------------------------------------------------
// macOS: a LaunchAgent that runs `voxtype start`
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
pub const LABEL: &str = "io.voxtype.autostart";

#[cfg(target_os = "macos")]
fn plist_path() -> Result<PathBuf, AutostartError> {
    dirs::home_dir()
        .map(|h| {
            h.join("Library/LaunchAgents")
                .join(format!("{LABEL}.plist"))
        })
        .ok_or(AutostartError::NoHome)
}

/// `RunAtLoad` with no `KeepAlive`: `voxtype start` is a short-lived command
/// that launches the app and exits, so launchd must not treat its exit as a
/// crash and restart it forever.
#[cfg(target_os = "macos")]
fn plist(exe: &std::path::Path) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{}</string>
        <string>start</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
</dict>
</plist>
"#,
        exe.display()
    )
}

#[cfg(target_os = "macos")]
pub fn enable() -> Result<PathBuf, AutostartError> {
    let path = plist_path()?;
    let exe = std::env::current_exe().map_err(|source| AutostartError::Io {
        context: "finding this executable".to_string(),
        source,
    })?;

    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|source| AutostartError::Io {
            context: format!("creating {}", dir.display()),
            source,
        })?;
    }
    std::fs::write(&path, plist(&exe)).map_err(|source| AutostartError::Io {
        context: format!("writing {}", path.display()),
        source,
    })?;

    // Replace any previous registration rather than adding a second one, so
    // enabling twice leaves one agent. Failures are not fatal: the plist is
    // on disk and will be picked up at the next login either way.
    let _ = std::process::Command::new("launchctl")
        .args(["unload", &path.to_string_lossy()])
        .output();
    let _ = std::process::Command::new("launchctl")
        .args(["load", &path.to_string_lossy()])
        .output();

    Ok(path)
}

#[cfg(target_os = "macos")]
pub fn disable() -> Result<bool, AutostartError> {
    let path = plist_path()?;
    if !path.exists() {
        return Ok(false);
    }
    let _ = std::process::Command::new("launchctl")
        .args(["unload", &path.to_string_lossy()])
        .output();
    std::fs::remove_file(&path).map_err(|source| AutostartError::Io {
        context: format!("removing {}", path.display()),
        source,
    })?;
    Ok(true)
}

/// The plist's presence is the answer. `launchctl list` would also report an
/// agent loaded in this session, but the question autostart answers is "will
/// it come up next login", and that is the file.
#[cfg(target_os = "macos")]
pub fn state() -> Result<(State, PathBuf), AutostartError> {
    let path = plist_path()?;
    let state = if path.exists() {
        State::Enabled
    } else {
        State::Disabled
    };
    Ok((state, path))
}

// ---------------------------------------------------------------------------
// Windows: one HKCU Run value
// ---------------------------------------------------------------------------

#[cfg(windows)]
pub const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
#[cfg(windows)]
pub const VALUE_NAME: &str = "Voxtype";

#[cfg(windows)]
pub fn enable() -> Result<std::path::PathBuf, AutostartError> {
    let exe = std::env::current_exe().map_err(|source| AutostartError::Io {
        context: "finding this executable".to_string(),
        source,
    })?;
    // Quoted, because Program Files and a user profile both contain spaces.
    let command = format!("\"{}\" start", exe.display());
    std::process::Command::new("reg")
        .args(["add", RUN_KEY, "/v", VALUE_NAME, "/t", "REG_SZ", "/d"])
        .arg(&command)
        .arg("/f")
        .output()
        .map_err(|source| AutostartError::Io {
            context: "running reg add".to_string(),
            source,
        })?;
    Ok(exe)
}

#[cfg(windows)]
pub fn disable() -> Result<bool, AutostartError> {
    let out = std::process::Command::new("reg")
        .args(["delete", RUN_KEY, "/v", VALUE_NAME, "/f"])
        .output()
        .map_err(|source| AutostartError::Io {
            context: "running reg delete".to_string(),
            source,
        })?;
    Ok(out.status.success())
}

#[cfg(windows)]
pub fn state() -> Result<(State, std::path::PathBuf), AutostartError> {
    let out = std::process::Command::new("reg")
        .args(["query", RUN_KEY, "/v", VALUE_NAME])
        .output()
        .map_err(|source| AutostartError::Io {
            context: "running reg query".to_string(),
            source,
        })?;
    let state = if out.status.success() {
        State::Enabled
    } else {
        State::Disabled
    };
    Ok((state, std::path::PathBuf::from(RUN_KEY)))
}

#[cfg(not(any(target_os = "macos", windows)))]
pub fn enable() -> Result<std::path::PathBuf, AutostartError> {
    Err(AutostartError::Unsupported)
}

#[cfg(not(any(target_os = "macos", windows)))]
pub fn disable() -> Result<bool, AutostartError> {
    Err(AutostartError::Unsupported)
}

#[cfg(not(any(target_os = "macos", windows)))]
pub fn state() -> Result<(State, std::path::PathBuf), AutostartError> {
    Err(AutostartError::Unsupported)
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn the_agent_runs_start_and_not_the_bare_daemon() {
        let body = plist(std::path::Path::new(
            "/Applications/Voxtype.app/Contents/MacOS/voxtype-bin",
        ));
        assert!(body.contains("<string>start</string>"), "{body}");
        assert!(
            !body.contains("<string>daemon</string>"),
            "registering the daemon alone loses the bundle's permissions: {body}"
        );
    }

    /// `voxtype start` launches the app and exits. KeepAlive would read that
    /// exit as a crash and relaunch it in a loop.
    #[test]
    fn the_agent_does_not_keep_alive() {
        let body = plist(std::path::Path::new("/tmp/voxtype-bin"));
        assert!(body.contains("<key>RunAtLoad</key>"));
        assert!(!body.contains("KeepAlive"), "{body}");
    }

    #[test]
    fn the_plist_is_well_formed_and_labelled() {
        let body = plist(std::path::Path::new("/tmp/voxtype-bin"));
        assert!(body.starts_with("<?xml"));
        assert!(body.contains(LABEL));
        // Distinct from the daemon agent `setup launchd` installs, so the two
        // can coexist without one overwriting the other.
        assert_ne!(LABEL, "io.voxtype.daemon");
    }

    #[test]
    fn the_executable_path_is_embedded_verbatim() {
        let exe = std::path::Path::new("/Applications/Voxtype.app/Contents/MacOS/voxtype-bin");
        assert!(plist(exe).contains(&exe.display().to_string()));
    }
}
