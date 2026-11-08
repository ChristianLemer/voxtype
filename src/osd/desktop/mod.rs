//! The recording card for Windows and macOS (`voxtype osd-card`).
//!
//! The same card as the Quickshell frontend on Omarchy, for desktops without
//! Wayland. It is drawn on the CPU (`card`), so it needs no GPU and runs in
//! virtual machines and remote sessions; a small presenter per platform puts
//! the image on screen in a window that never takes focus, since voxtype types
//! into whatever window has it.
//!
//! It runs in its own process, which the daemon's OSD supervisor starts from
//! the voxtype executable itself: a crash here never stops dictation, and
//! there is still a single file to ship.
//!
//! Audio frames arrive over the daemon's level socket (a named pipe on
//! Windows); the daemon state comes from its state file. While the daemon is
//! idle the card is hidden.

pub(crate) mod card;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::audio::levels::{AudioFrame, FRAME_HZ};
use crate::osd::config::OsdConfig;
use crate::osd::ipc::{resolve_socket_path, run_ipc_loop, FrameRing};
use crate::osd::theme::load_palette;
use crate::osd::visual::{peak_meter_fraction, Color, Palette, PeakHold};

/// How often the state file is re-read.
pub const STATE_POLL: Duration = Duration::from_millis(80);

/// Pause between reconnection attempts to the daemon's level socket.
const RECONNECT_SECS: f32 = 1.0;

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
        .or_else(crate::config::Config::default_path);
    let partial: Partial = path
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default();
    let state_file = match partial.state_file.as_deref() {
        None | Some("auto") => Some(crate::config::Config::runtime_dir().join("state")),
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
    /// Smoothed waveform level per column, 0..=1 before gain.
    wave: Vec<f32>,
    /// Seconds of animation, driving the waveform's ripple.
    phase: f32,
    rendered_at: Instant,
}

/// How fast a waveform column climbs to a louder level and falls back, per
/// second. The values of Omarchy's Quickshell card (`RecipeRenderer.qml`):
/// quick to rise, slower to fall, so the trace pulses with the voice instead
/// of jumping between 10 ms blocks.
const WAVE_RISE: f32 = 30.0;
const WAVE_FALL: f32 = 14.0;

/// Exponential approach of `current` toward `target`.
fn approach(current: f32, target: f32, stiffness: f32, dt: f32) -> f32 {
    current + (target - current) * (1.0 - (-stiffness.max(0.01) * dt).exp())
}

/// Average peak of the frames under each of `columns` columns, the newest
/// frame at the right edge and the window's left part silent while history
/// is still filling.
fn column_targets(peaks: &[f32], columns: usize, window_frames: usize) -> Vec<f32> {
    let capacity = peaks.len().max(window_frames).max(1);
    let pad = capacity - peaks.len();
    (0..columns)
        .map(|i| {
            let start = (i * capacity / columns).saturating_sub(pad);
            let end = (((i + 1) * capacity / columns).saturating_sub(pad)).max(start + 1);
            let bucket = &peaks[start.min(peaks.len())..end.min(peaks.len())];
            if bucket.is_empty() {
                0.0
            } else {
                bucket.iter().sum::<f32>() / bucket.len() as f32
            }
        })
        .collect()
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
        let dt = now
            .saturating_duration_since(self.rendered_at)
            .as_secs_f32()
            .min(0.1);
        self.rendered_at = now;
        self.phase += dt;
        let width = self.config.width_px as f32;
        let height = self.config.height_px as f32;
        let columns = card::waveform_columns(width);
        let (targets, peak_dbfs) = {
            let ring = self.feed.ring.lock().unwrap();
            let peaks: Vec<f32> = ring
                .iter()
                .map(|f: AudioFrame| f.min.abs().max(f.max.abs()).min(1.0))
                .collect();
            let window = (self.config.waveform_window_secs * FRAME_HZ as f32) as usize;
            (
                column_targets(&peaks, columns, window),
                ring.latest().map(|f| f.peak_dbfs).unwrap_or(-120.0),
            )
        };
        self.wave.resize(columns, 0.0);
        for (level, target) in self.wave.iter_mut().zip(targets) {
            let stiffness = if target > *level {
                WAVE_RISE
            } else {
                WAVE_FALL
            };
            *level = approach(*level, target, stiffness, dt);
        }
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
                wave: &self.wave,
                phase: self.phase,
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

/// Show the card until the process is stopped. `config` overrides the
/// default config file.
pub fn run(config: Option<&std::path::Path>) -> anyhow::Result<()> {
    let (config, state_file) = load_config(config);
    if !config.enabled {
        tracing::info!("OSD disabled in config; exiting");
        return Ok(());
    }

    let feed = Arc::new(Feed {
        ring: Mutex::new(FrameRing::for_window(config.waveform_window_secs)),
        peak_hold: Mutex::new(PeakHold::new(config.peak_decay_db_per_sec)),
        last_frame: Mutex::new(Instant::now() - Duration::from_secs(10)),
    });
    spawn_ipc(feed.clone(), resolve_socket_path(None), RECONNECT_SECS);

    let model = Model {
        feed,
        config,
        palette: load_palette(),
        state_file,
        state: String::new(),
        state_read_at: Instant::now() - STATE_POLL,
        energy: 0.0,
        wave: Vec::new(),
        phase: 0.0,
        rendered_at: Instant::now(),
    };

    #[cfg(windows)]
    return windows::run(model);
    #[cfg(target_os = "macos")]
    return macos::run(model);
}
