//! macOS first-launch auto-setup. Only compiled when the binary is launched as
//! a `.app` bundle on macOS.

use voxtype::{config, setup};

/// First-launch auto-setup for macOS app bundle launches.
///
/// Detects if this is the first launch by checking for a config file and downloaded model.
/// If either is missing, creates default config and downloads the recommended model
/// so the user can start recording immediately after granting permissions.
pub(crate) async fn first_launch_setup(_config: &config::Config) {
    // Check if config file exists
    let config_exists = config::Config::default_path()
        .map(|p| p.exists())
        .unwrap_or(false);

    // Check if any model is already downloaded
    let models_dir = config::Config::models_dir();
    let has_model = models_dir.exists()
        && std::fs::read_dir(&models_dir)
            .map(|entries| {
                entries.filter_map(|e| e.ok()).any(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    // Whisper models (ggml-*.bin) or ONNX model dirs. Any .onnx
                    // file counts: the int8 Parakeet a fleet installs ships
                    // encoder-model.int8.onnx, and requiring encoder-model.onnx
                    // re-ran the model download at every launch, behind a
                    // firewall that blocks it.
                    (name.starts_with("ggml-") && name.ends_with(".bin"))
                        || (e.path().is_dir() && dir_has_onnx(&e.path()))
                })
            })
            .unwrap_or(false);

    if config_exists && has_model {
        return; // Not first launch
    }

    // Create default config if missing
    if !config_exists {
        if let Some(config_path) = config::Config::default_path() {
            if let Some(parent) = config_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let content = config::default_config_content();
            if std::fs::write(&config_path, &content).is_ok() {
                tracing::info!("Created default config: {:?}", config_path);
            }
        }
    }

    // Download default model if none present
    if !has_model {
        // Detect system language to choose the right model
        let is_english = tokio::process::Command::new("defaults")
            .args(["read", "NSGlobalDomain", "AppleLanguages"])
            .output()
            .await
            .map(|o| {
                let s = String::from_utf8_lossy(&o.stdout);
                s.lines()
                    .find(|l| l.trim().starts_with('"'))
                    .map(|l| {
                        l.trim()
                            .trim_matches(|c| c == '"' || c == ',')
                            .starts_with("en")
                    })
                    .unwrap_or(true)
            })
            .unwrap_or(true);

        // Show notification that model is downloading
        let _ = std::process::Command::new("osascript")
            .args([
                "-e",
                "display notification \"Downloading speech model (this may take a minute)...\" with title \"Voxtype\"",
            ])
            .status();

        #[cfg(feature = "parakeet")]
        let download_result = if is_english {
            tracing::info!("First launch: downloading Parakeet model");
            setup::model::download_parakeet_model("parakeet-tdt-0.6b-v3-int8")
                .and_then(|_| setup::model::set_parakeet_config("parakeet-tdt-0.6b-v3-int8"))
        } else {
            tracing::info!("First launch: downloading Whisper base model");
            setup::model::download_model("base")
                .and_then(|_| setup::model::set_model_config("base"))
        };

        #[cfg(not(feature = "parakeet"))]
        let download_result = {
            let model = if is_english { "base.en" } else { "base" };
            tracing::info!("First launch: downloading Whisper {} model", model);
            setup::model::download_model(model).and_then(|_| setup::model::set_model_config(model))
        };

        match download_result {
            Ok(_) => {
                let _ = std::process::Command::new("osascript")
                    .args([
                        "-e",
                        "display notification \"Ready! Press fn to start recording.\" with title \"Voxtype\"",
                    ])
                    .status();
            }
            Err(e) => {
                tracing::error!("Failed to download model: {}", e);
                let msg = format!(
                    "display notification \"Model download failed: {}. Run 'voxtype setup model' to try again.\" with title \"Voxtype\"",
                    e.to_string().replace('"', "'")
                );
                let _ = std::process::Command::new("osascript")
                    .args(["-e", &msg])
                    .status();
            }
        }
    }
}

fn dir_has_onnx(dir: &std::path::Path) -> bool {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .any(|e| e.path().extension().is_some_and(|x| x == "onnx"))
        })
        .unwrap_or(false)
}

/// Set on the daemon Voxtype.app starts: the pid of the app.
pub(crate) const APP_PID_VAR: &str = "VOXTYPE_APP_PID";

/// Stop this process (the daemon, or a settings window) when the app that
/// started it is gone.
///
/// When Input Monitoring is switched on, macOS offers to quit and reopen the
/// app. It quits the app's main process, the menu bar, and the daemon it
/// started lived on, orphaned. macOS then counts that daemon as Voxtype.app
/// still running, so the reopen did nothing, and neither did `open -a`: the
/// user was left with dictation but no menu bar (#7). With the daemon gone
/// as soon as its app is, the reopen starts a whole new app.
///
/// A settings window the app opened has the same property and goes the same
/// way. Only for processes the app started (`VOXTYPE_APP_PID` set): a
/// daemon run from a terminal or a LaunchAgent keeps running on its own.
pub(crate) fn exit_with_app() {
    let Some(app) = std::env::var(APP_PID_VAR)
        .ok()
        .and_then(|p| p.parse::<u32>().ok())
    else {
        return;
    };
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_millis(300));
        if std::os::unix::process::parent_id() != app {
            tracing::info!("Voxtype.app (pid {app}) is gone; stopping");
            // The daemon's own SIGTERM handling shuts it down cleanly;
            // exit regardless if that takes too long.
            unsafe {
                libc::kill(std::process::id() as libc::pid_t, libc::SIGTERM);
            }
            std::thread::sleep(std::time::Duration::from_secs(3));
            std::process::exit(0);
        }
    });
}
