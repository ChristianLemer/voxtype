//! `voxtype settings`: a settings window generated from the config schema.
//!
//! Every pane comes from [`schema::scalar_keys`], the same allowlist behind
//! `voxtype config set` and `config schema --json`, so a new config key shows
//! up here without touching this file. Changes are written through
//! [`config_set::set_key`], which keeps the file's comments, the moment a
//! control settles. The window is its own process: closing it ends it.

use std::collections::HashMap;
use std::path::PathBuf;

use eframe::egui;
use serde_json::Value as Json;

use crate::config::schema::{self, KeySpec, KeyType};
use crate::config::{load_config, Config};
use crate::config_set;

pub fn run(path: PathBuf) -> anyhow::Result<()> {
    let config = load_config(Some(&path))?;
    let viewport = egui::ViewportBuilder::default()
        .with_title("Voxtype Settings")
        .with_app_id("voxtype-settings")
        .with_inner_size([820.0, 600.0])
        .with_min_inner_size([560.0, 360.0]);
    let options = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "voxtype-settings",
        options,
        Box::new(move |_cc| Ok(Box::new(SettingsApp::new(path, config)))),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}

struct SettingsApp {
    path: PathBuf,
    config: Config,
    section: &'static str,
    search: String,
    /// Text being typed into a field, not yet written.
    drafts: HashMap<&'static str, String>,
    /// Choices for dynamic enums, by source, fetched once.
    sources: HashMap<&'static str, Vec<Choice>>,
    restart_pending: bool,
    message: Option<(bool, String)>,
}

struct Choice {
    name: String,
    engine: Option<String>,
    installed: bool,
}

impl SettingsApp {
    fn new(path: PathBuf, config: Config) -> Self {
        Self {
            path,
            config,
            section: schema::SECTIONS[0],
            search: String::new(),
            drafts: HashMap::new(),
            sources: HashMap::new(),
            restart_pending: false,
            message: None,
        }
    }

    /// Keys this binary honors under the engine currently configured.
    fn visible(&self, spec: &KeySpec) -> bool {
        spec.compiled() && spec.engine.is_none_or(|e| e == self.config.engine.name())
    }

    fn keys_in(&self, section: &str) -> Vec<&'static KeySpec> {
        schema::scalar_keys()
            .filter(|s| s.section == section && self.visible(s))
            .collect()
    }

    fn matching(&self, needle: &str) -> Vec<&'static KeySpec> {
        let needle = needle.to_lowercase();
        schema::scalar_keys()
            .filter(|s| self.visible(s))
            .filter(|s| {
                s.key.to_lowercase().contains(&needle)
                    || s.label.to_lowercase().contains(&needle)
                    || s.description.to_lowercase().contains(&needle)
            })
            .collect()
    }

    fn value(&self, spec: &KeySpec) -> String {
        match schema::resolve(spec.key, &self.config) {
            None | Some(Json::Null) => String::new(),
            Some(Json::String(s)) => s,
            Some(other) => other.to_string(),
        }
    }

    fn write(&mut self, spec: &KeySpec, raw: &str) {
        let result = if raw.is_empty() {
            config_set::unset_key(self.path.clone(), spec.key)
        } else {
            config_set::set_key(self.path.clone(), spec.key, raw)
        };
        match result {
            Ok(outcome) => {
                self.restart_pending |= outcome.restart_required;
                self.message = Some((false, format!("Saved {}", spec.label)));
                match load_config(Some(&self.path)) {
                    Ok(config) => self.config = config,
                    Err(e) => self.message = Some((true, e.to_string())),
                }
            }
            Err(e) => self.message = Some((true, e.to_string())),
        }
    }

    fn choices(&mut self, source: &'static str) -> &[Choice] {
        self.sources
            .entry(source)
            .or_insert_with(|| fetch_choices(source))
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.add(
            egui::TextEdit::singleline(&mut self.search)
                .hint_text("Search")
                .desired_width(f32::INFINITY),
        );
        ui.add_space(8.0);
        for section in schema::SECTIONS {
            if self.keys_in(section).is_empty() {
                continue;
            }
            let selected = self.search.is_empty() && self.section == *section;
            if ui.selectable_label(selected, *section).clicked() {
                self.section = section;
                self.search.clear();
            }
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if self.restart_pending {
                ui.label("Some changes take effect after a restart.");
                if ui.button("Restart Voxtype").clicked() {
                    match restart_daemon() {
                        Ok(()) => {
                            self.restart_pending = false;
                            self.message = Some((false, "Voxtype restarted".into()));
                        }
                        Err(e) => self.message = Some((true, e)),
                    }
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some((is_error, text)) = &self.message {
                    let color = if *is_error {
                        ui.visuals().error_fg_color
                    } else {
                        ui.visuals().weak_text_color()
                    };
                    ui.colored_label(color, text);
                }
            });
        });
    }

    fn pane(&mut self, ui: &mut egui::Ui) {
        let (title, keys) = if self.search.is_empty() {
            (self.section.to_string(), self.keys_in(self.section))
        } else {
            let search = self.search.clone();
            (format!("Results for “{search}”"), self.matching(&search))
        };
        ui.heading(title);
        ui.add_space(4.0);
        if keys.is_empty() {
            ui.weak("Nothing here.");
        }
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for spec in keys {
                    self.row(ui, spec);
                    ui.separator();
                }
            });
    }

    fn row(&mut self, ui: &mut egui::Ui, spec: &'static KeySpec) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.strong(spec.label);
            if spec.restart_required {
                ui.weak("· restart")
                    .on_hover_text("Takes effect after a restart");
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.control(ui, spec);
            });
        });
        let help = if self.search.is_empty() {
            spec.description.to_string()
        } else {
            format!("{} — {}", spec.section, spec.description)
        };
        ui.label(egui::RichText::new(help).weak());
        ui.add_space(4.0);
    }

    fn control(&mut self, ui: &mut egui::Ui, spec: &'static KeySpec) {
        let current = self.value(spec);
        match spec.ty {
            KeyType::Bool => {
                let mut on = current == "true";
                if ui.checkbox(&mut on, "").changed() {
                    self.write(spec, if on { "true" } else { "false" });
                }
            }
            KeyType::Int { min, max } => {
                let mut n: i64 = current.parse().unwrap_or(min.max(0));
                let r = ui.add(egui::DragValue::new(&mut n).range(min..=max));
                if settled(&r) && n.to_string() != current {
                    self.write(spec, &n.to_string());
                }
            }
            KeyType::Float { min, max } => {
                let mut x: f64 = current.parse().unwrap_or(min.max(0.0));
                let r = ui.add(
                    egui::DragValue::new(&mut x)
                        .range(min..=max)
                        .speed(0.01)
                        .max_decimals(3),
                );
                if settled(&r) && x.to_string() != current {
                    self.write(spec, &x.to_string());
                }
            }
            KeyType::Enum {
                choices,
                open: false,
            } => {
                let mut picked = current.clone();
                egui::ComboBox::from_id_salt(spec.key)
                    .selected_text(if picked.is_empty() { "—" } else { &picked })
                    .show_ui(ui, |ui| {
                        for c in choices {
                            ui.selectable_value(&mut picked, c.to_string(), *c);
                        }
                    });
                if picked != current {
                    self.write(spec, &picked);
                }
            }
            KeyType::Enum {
                choices,
                open: true,
            } => {
                let list: Vec<(String, String)> = choices
                    .iter()
                    .map(|c| (c.to_string(), c.to_string()))
                    .collect();
                self.editable_choice(ui, spec, &current, &list);
            }
            KeyType::DynamicEnum { source } => {
                let engine = spec
                    .engine
                    .map(str::to_string)
                    .unwrap_or_else(|| self.config.engine.name().to_string());
                let list: Vec<(String, String)> = self
                    .choices(source)
                    .iter()
                    .filter(|c| c.engine.as_deref().is_none_or(|e| e == engine))
                    .map(|c| {
                        let shown = if c.installed {
                            format!("{} (installed)", c.name)
                        } else {
                            c.name.clone()
                        };
                        (c.name.clone(), shown)
                    })
                    .collect();
                self.editable_choice(ui, spec, &current, &list);
            }
            KeyType::String => {
                self.text_field(ui, spec, &current);
            }
            KeyType::MapString => {}
        }
    }

    /// A text field with a menu of suggestions beside it.
    fn editable_choice(
        &mut self,
        ui: &mut egui::Ui,
        spec: &'static KeySpec,
        current: &str,
        list: &[(String, String)],
    ) {
        // An empty combo box draws only its arrow: the suggestions menu.
        let mut picked = current.to_string();
        egui::ComboBox::from_id_salt(spec.key)
            .selected_text("")
            .width(0.0)
            .show_ui(ui, |ui| {
                for (value, shown) in list {
                    ui.selectable_value(&mut picked, value.clone(), shown);
                }
            });
        if picked != current {
            self.drafts.remove(spec.key);
            self.write(spec, &picked);
            return;
        }
        self.text_field(ui, spec, current);
    }

    fn text_field(&mut self, ui: &mut egui::Ui, spec: &'static KeySpec, current: &str) {
        let draft = self
            .drafts
            .entry(spec.key)
            .or_insert_with(|| current.to_string());
        let r = ui.add(egui::TextEdit::singleline(draft).desired_width(220.0));
        if r.lost_focus() {
            let text = draft.trim().to_string();
            self.drafts.remove(spec.key);
            if text != current {
                self.write(spec, &text);
            }
        } else if !r.has_focus() && *draft != current {
            // Written elsewhere (a menu pick, a reload): follow the file.
            *draft = current.to_string();
        }
    }
}

impl eframe::App for SettingsApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::bottom("status")
            .show_separator_line(true)
            .show_inside(ui, |ui| {
                ui.add_space(4.0);
                self.status_bar(ui);
                ui.add_space(2.0);
            });
        egui::Panel::left("sections")
            .resizable(false)
            .exact_size(180.0)
            .show_inside(ui, |ui| self.sidebar(ui));
        egui::CentralPanel::default().show_inside(ui, |ui| self.pane(ui));
    }
}

/// A drag or a typed number is done: write it once, not on every frame.
fn settled(r: &egui::Response) -> bool {
    r.drag_stopped() || (r.changed() && !r.dragged())
}

/// Choices for a dynamic enum, from `voxtype info <source> --json`.
fn fetch_choices(source: &str) -> Vec<Choice> {
    let Ok(exe) = std::env::current_exe() else {
        return Vec::new();
    };
    let Ok(out) = std::process::Command::new(exe)
        .args(["info", source, "--json"])
        .output()
    else {
        return Vec::new();
    };
    let Ok(doc) = serde_json::from_slice::<Json>(&out.stdout) else {
        return Vec::new();
    };
    let name_of = |v: &Json| v.get("name").and_then(Json::as_str).map(str::to_string);
    match &doc {
        Json::Object(map) if map.contains_key("engines") => map["engines"]
            .as_object()
            .into_iter()
            .flatten()
            .flat_map(|(engine, e)| {
                e.get("models")
                    .and_then(Json::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(move |m| {
                        Some(Choice {
                            name: name_of(m)?,
                            engine: Some(engine.clone()),
                            installed: m.get("installed").and_then(Json::as_bool).unwrap_or(false),
                        })
                    })
            })
            .collect(),
        Json::Array(items) => items
            .iter()
            .filter_map(|v| {
                Some(Choice {
                    name: name_of(v)?,
                    engine: None,
                    installed: false,
                })
            })
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(any(target_os = "macos", windows))]
fn restart_daemon() -> Result<(), String> {
    crate::setup::start::restart()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(not(any(target_os = "macos", windows)))]
fn restart_daemon() -> Result<(), String> {
    let status = std::process::Command::new("systemctl")
        .args(["--user", "restart", "voxtype"])
        .status()
        .map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err("systemctl --user restart voxtype failed".into())
    }
}
