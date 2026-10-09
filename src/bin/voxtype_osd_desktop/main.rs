//! `voxtype-osd-desktop` — the recording card for Windows and macOS.
//!
//! The same card as the Quickshell frontend on Omarchy, for desktops without
//! Wayland. The card is drawn on the CPU (`card`), so it needs no GPU and runs
//! in virtual machines and remote sessions; a small presenter per platform puts
//! the image on screen in a window that never takes focus, since voxtype types
//! into whatever window has it.
//!
//! Audio frames arrive over the daemon's level socket (a named pipe on
//! Windows); the daemon state comes from its state file. While the daemon is
//! idle the card is hidden.

mod card;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use clap::Parser;

use voxtype::audio::levels::{AudioFrame, FRAME_HZ};
use voxtype::osd::config::OsdConfig;
use voxtype::osd::ipc::{resolve_socket_path, run_ipc_loop, FrameRing};
use voxtype::osd::theme::load_palette;
use voxtype::osd::visual::{peak_meter_fraction, project_envelope, Color, Palette, PeakHold};

/// How often the state file is re-read.
pub const STATE_POLL: Duration = Duration::from_millis(80);

#[derive(Parser, Debug)]
#[command(
    name = "voxtype-osd-desktop",
    version,
    about = "Voxtype recording card for Windows and macOS"
)]
struct Args {
    #[arg(long, env = "VOXTYPE_CONFIG")]
    config: Option<PathBuf>,

    #[arg(long, env = "VOXTYPE_OSD_SOCKET")]
    socket: Option<PathBuf>,

    #[arg(long, default_value = "1.0", env = "VOXTYPE_OSD_RECONNECT_SECS")]
    reconnect_secs: f32,
}

fn load_config(explicit: Option<&std::path::Path>) -> (OsdConfig, Option<PathBuf>) {
    #[derive(serde::Deserialize, Default)]
    struct Partial {
        #[serde(default)]
        osd: Option<OsdConfig>,
        #[serde(default)]
        state_file: Option<String>,
    }
    let path = explicit
        .map(std::path::Path::to_path_buf)
        .or_else(voxtype::config::Config::default_path);
    let partial: Partial = path
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default();
    let state_file = match partial.state_file.as_deref() {
        None | Some("auto") => Some(voxtype::config::Config::runtime_dir().join("state")),
        Some("disabled" | "none" | "off" | "false") => None,
        Some(p) => Some(PathBuf::from(p)),
    };
    (partial.osd.unwrap_or_default(), state_file)
}

/// Written by the IPC thread, read by the presenter.
struct Feed {
    ring: Mutex<FrameRing>,
    peak_hold: Mutex<PeakHold>,
    last_frame: Mutex<Instant>,
}

/// What the card shows, independent of how it reaches the screen.
pub struct Model {
    feed: Arc<Feed>,
    pub config: OsdConfig,
    palette: Palette,
    state_file: Option<PathBuf>,
    state: String,
    state_read_at: Instant,
    energy: f32,
}

impl Model {
    /// Re-read the daemon state at most every [`STATE_POLL`]. Returns whether
    /// the card should be on screen.
    pub fn poll(&mut self) -> bool {
        if self.state_read_at.elapsed() >= STATE_POLL {
            self.state_read_at = Instant::now();
            if let Some(path) = &self.state_file {
                let state = std::fs::read_to_string(path)
                    .unwrap_or_default()
                    .trim()
                    .to_lowercase();
                if state != self.state {
                    if state == "recording" {
                        self.feed.ring.lock().unwrap().clear();
                    }
                    self.state = state;
                }
            }
        }
        !matches!(self.state.as_str(), "" | "idle" | "stopped")
    }

    fn state_color(&self) -> Color {
        match self.state.as_str() {
            "recording" => card::RECORDING,
            "streaming" => card::STREAMING,
            "transcribing" => card::TRANSCRIBING,
            _ => card::IDLE,
        }
    }

    /// Draw the current frame at `scale` physical pixels per logical pixel.
    pub fn render(&mut self, scale: f32) -> Option<tiny_skia::Pixmap> {
        let now = Instant::now();
        let width = self.config.width_px as f32;
        let height = self.config.height_px as f32;
        let (envelope, peak_dbfs) = {
            let ring = self.feed.ring.lock().unwrap();
            let frames: Vec<AudioFrame> = ring.iter().collect();
            let envelope = project_envelope(
                &frames,
                card::waveform_columns(width),
                self.config.waveform_window_secs as f64 * FRAME_HZ as f64,
                ring.scroll_offset(now),
            );
            (
                envelope,
                ring.latest().map(|f| f.peak_dbfs).unwrap_or(-120.0),
            )
        };
        let held_dbfs = self.feed.peak_hold.lock().unwrap().held_dbfs;

        let recording = self.state == "recording";
        let live = self.feed.last_frame.lock().unwrap().elapsed() < Duration::from_millis(300);
        let target = if recording && live {
            peak_meter_fraction(peak_dbfs, card::METER_FLOOR_DBFS)
        } else {
            0.0
        };
        // Ease toward the voice level so the glow breathes rather than flickers.
        self.energy += (target - self.energy) * 0.25;

        card::render(
            &card::Scene {
                width,
                height,
                palette: &self.palette,
                state_color: self.state_color(),
                opacity: self.config.opacity,
                energy: self.energy,
                glow: recording,
                envelope: &envelope,
                waveform_gain: self.config.waveform_gain,
                peak_dbfs,
                held_dbfs,
            },
            scale,
        )
    }
}

fn spawn_ipc(feed: Arc<Feed>, socket_path: PathBuf, reconnect_secs: f32) {
    std::thread::spawn(move || {
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                tracing::error!("Cannot start the OSD IPC runtime: {}", e);
                return;
            }
        };
        let mut last = Instant::now();
        runtime.block_on(run_ipc_loop(socket_path, reconnect_secs, move |frame| {
            let now = Instant::now();
            let dt = now.duration_since(last).as_secs_f32();
            last = now;
            feed.ring.lock().unwrap().push(frame);
            feed.peak_hold.lock().unwrap().update(frame.peak_dbfs, dt);
            *feed.last_frame.lock().unwrap() = now;
        }));
    });
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let args = Args::parse();
    let (config, state_file) = load_config(args.config.as_deref());
    if !config.enabled {
        tracing::info!("OSD disabled in config; exiting");
        return Ok(());
    }

    let feed = Arc::new(Feed {
        ring: Mutex::new(FrameRing::for_window(config.waveform_window_secs)),
        peak_hold: Mutex::new(PeakHold::new(config.peak_decay_db_per_sec)),
        last_frame: Mutex::new(Instant::now() - Duration::from_secs(10)),
    });
    spawn_ipc(
        feed.clone(),
        resolve_socket_path(args.socket),
        args.reconnect_secs,
    );

    let model = Model {
        feed,
        config,
        palette: load_palette(),
        state_file,
        state: String::new(),
        state_read_at: Instant::now() - STATE_POLL,
        energy: 0.0,
    };

    #[cfg(windows)]
    return windows::run(model);
    #[cfg(target_os = "macos")]
    return macos::run(model);
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = model;
        anyhow::bail!("voxtype-osd-desktop runs on Windows and macOS; on Linux use voxtype-osd")
    }
}
