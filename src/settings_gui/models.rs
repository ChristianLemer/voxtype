//! The Models pane: the current engine's models, downloadable in place.
//!
//! The list comes from `voxtype info models --json`, which also checks each
//! installed model's files, and a download runs `voxtype setup --download
//! --progress-format json` in the background, its progress events driving the
//! bar. Downloading never selects a model; "Use" does, through the same
//! `<engine>.model` key the Engine pane edits.

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use eframe::egui;
use serde_json::Value as Json;

use super::{card, SettingsApp};
use crate::config::schema::{self, KeySpec, KeyType};

#[derive(Default)]
pub(super) struct State {
    /// The engine the list was read for, and the list.
    listed: Option<(String, Vec<Model>)>,
    download: Arc<Mutex<Option<Download>>>,
}

struct Model {
    name: String,
    download_arg: String,
    downloadable: bool,
    installed: bool,
}

struct Download {
    model: String,
    file: String,
    pct: Option<f32>,
    finished: Option<Result<(), String>>,
}

impl SettingsApp {
    pub(super) fn models_pane(&mut self, ui: &mut egui::Ui) {
        let engine = self.config.engine.name().to_string();
        self.collect_finished_download();
        if self
            .models
            .listed
            .as_ref()
            .is_none_or(|(e, _)| *e != engine)
        {
            self.models.listed = Some((engine.clone(), list_models(&engine)));
        }
        let spec = model_spec(&engine);
        let current = spec.map(|s| self.value(s)).unwrap_or_default();
        let busy = self
            .models
            .download
            .lock()
            .map(|d| d.is_some())
            .unwrap_or(false);
        if busy {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(200));
        }

        ui.add_space(14.0);
        ui.heading(egui::RichText::new("Models").strong());
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(format!(
                "Models for the {engine} engine. Pick another engine under Engine to see its models."
            ))
            .weak(),
        );
        ui.add_space(10.0);

        let models = self
            .models
            .listed
            .as_ref()
            .map(|(_, m)| m.as_slice())
            .unwrap_or_default();
        if models.is_empty() {
            ui.weak("This engine has no models to download.");
            return;
        }

        let mut pick: Option<String> = None;
        let mut fetch: Option<String> = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_max_width(ui.available_width() - 16.0);
                card(ui).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let last = models.len() - 1;
                    for (i, m) in models.iter().enumerate() {
                        ui.add_space(6.0);
                        let in_use = m.name == current;
                        let status = if in_use {
                            "In use"
                        } else if m.installed {
                            "Downloaded"
                        } else {
                            "Not downloaded"
                        };
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                let strong = ui.visuals().strong_text_color();
                                ui.label(egui::RichText::new(&m.name).size(14.5).color(strong));
                                ui.label(egui::RichText::new(status).size(12.0).weak());
                            });
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if in_use {
                                        ui.weak("✔");
                                    } else if m.installed {
                                        if ui.button("Use").clicked() {
                                            pick = Some(m.name.clone());
                                        }
                                    } else if m.downloadable {
                                        let button =
                                            ui.add_enabled(!busy, egui::Button::new("Download"));
                                        if button.clicked() {
                                            fetch = Some(m.download_arg.clone());
                                        }
                                    }
                                },
                            );
                        });
                        self.download_progress(ui, &m.download_arg);
                        ui.add_space(6.0);
                        if i != last {
                            ui.separator();
                        }
                    }
                });
            });

        if let (Some(name), Some(spec)) = (pick, spec) {
            self.write(spec, &name);
        }
        if let Some(arg) = fetch {
            self.start_download(arg, ui.ctx().clone());
        }
    }

    fn download_progress(&self, ui: &mut egui::Ui, model: &str) {
        let Ok(slot) = self.models.download.lock() else {
            return;
        };
        let Some(d) = slot.as_ref().filter(|d| d.model == model) else {
            return;
        };
        let bar = match d.pct {
            Some(pct) => {
                egui::ProgressBar::new(pct / 100.0).text(format!("{}  {:.0}%", d.file, pct))
            }
            None => egui::ProgressBar::new(0.0)
                .animate(true)
                .text(if d.file.is_empty() {
                    "Starting…".to_string()
                } else {
                    d.file.clone()
                }),
        };
        ui.add_space(4.0);
        ui.add(bar.desired_height(18.0));
    }

    /// Report a download that ended since the last frame, and re-read the
    /// list so the model shows as downloaded.
    fn collect_finished_download(&mut self) {
        let finished = {
            let Ok(mut slot) = self.models.download.lock() else {
                return;
            };
            match slot.as_ref().and_then(|d| d.finished.clone()) {
                Some(result) => {
                    let model = slot.take().map(|d| d.model).unwrap_or_default();
                    Some((model, result))
                }
                None => None,
            }
        };
        if let Some((model, result)) = finished {
            self.message = Some(match result {
                Ok(()) => (false, format!("Downloaded {model}")),
                Err(e) => (true, format!("Could not download {model}: {e}")),
            });
            self.models.listed = None;
            self.sources.remove("models");
        }
    }

    fn start_download(&mut self, model: String, ctx: egui::Context) {
        let slot = self.models.download.clone();
        if let Ok(mut s) = slot.lock() {
            *s = Some(Download {
                model: model.clone(),
                file: String::new(),
                pct: None,
                finished: None,
            });
        }
        let path = self.path.clone();
        std::thread::spawn(move || {
            let result = run_download(&path, &model, &slot, &ctx);
            if let Ok(mut s) = slot.lock() {
                if let Some(d) = s.as_mut() {
                    d.finished.get_or_insert(result);
                }
            }
            ctx.request_repaint();
        });
    }
}

fn run_download(
    path: &std::path::Path,
    model: &str,
    slot: &Mutex<Option<Download>>,
    ctx: &egui::Context,
) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(exe);
    cmd.arg("--config")
        .arg(path)
        .args(["setup", "--download", "--no-post-install", "--model", model])
        .args(["--progress-format", "json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    let mut error: Option<String> = None;
    if let Some(stdout) = child.stdout.take() {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let Ok(event) = serde_json::from_str::<Json>(&line) else {
                continue;
            };
            match event.get("event").and_then(Json::as_str) {
                Some("progress") => {
                    if let Ok(mut s) = slot.lock() {
                        if let Some(d) = s.as_mut() {
                            d.file = event
                                .get("file")
                                .and_then(Json::as_str)
                                .unwrap_or_default()
                                .to_string();
                            d.pct = event.get("pct").and_then(Json::as_f64).map(|p| p as f32);
                        }
                    }
                    ctx.request_repaint();
                }
                Some("error") => {
                    error = event
                        .get("message")
                        .and_then(Json::as_str)
                        .map(str::to_string);
                }
                _ => {}
            }
        }
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    match (status.success(), error) {
        (true, None) => Ok(()),
        (_, Some(message)) => Err(message),
        (false, None) => Err(format!("setup exited with {status}")),
    }
}

/// The `<engine>.model` key, which names the model the daemon loads.
fn model_spec(engine: &str) -> Option<&'static KeySpec> {
    schema::scalar_keys().find(|s| {
        s.engine == Some(engine)
            && s.field == "model"
            && matches!(s.ty, KeyType::DynamicEnum { source: "models" })
    })
}

fn list_models(engine: &str) -> Vec<Model> {
    let Ok(exe) = std::env::current_exe() else {
        return Vec::new();
    };
    let mut cmd = Command::new(exe);
    cmd.args(["info", "models", "--json", "--engine", engine]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let Ok(out) = cmd.output() else {
        return Vec::new();
    };
    let Ok(doc) = serde_json::from_slice::<Json>(&out.stdout) else {
        return Vec::new();
    };
    doc.pointer(&format!("/engines/{engine}/models"))
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let name = m.get("name")?.as_str()?.to_string();
            Some(Model {
                download_arg: m
                    .get("download_arg")
                    .and_then(Json::as_str)
                    .unwrap_or(&name)
                    .to_string(),
                downloadable: m
                    .get("downloadable")
                    .and_then(Json::as_bool)
                    .unwrap_or(false),
                installed: m.get("installed").and_then(Json::as_bool).unwrap_or(false),
                name,
            })
        })
        .collect()
}
