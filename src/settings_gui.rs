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
    let result = open(eframe::Renderer::Glow, path.clone(), config.clone());
    // Some Windows machines, VMs among them, have no OpenGL 2 driver; wgpu
    // goes through Direct3D there.
    #[cfg(windows)]
    let result = result.or_else(|e| {
        tracing::warn!("OpenGL unavailable ({e}), trying Direct3D");
        open(eframe::Renderer::Wgpu, path, config)
    });
    result.map_err(|e| anyhow::anyhow!("{e}"))
}

fn open(renderer: eframe::Renderer, path: PathBuf, config: Config) -> eframe::Result {
    let viewport = egui::ViewportBuilder::default()
        .with_title("Voxtype Settings")
        .with_app_id("voxtype-settings")
        .with_inner_size([900.0, 640.0])
        .with_min_inner_size([640.0, 420.0]);
    let options = eframe::NativeOptions {
        viewport,
        renderer,
        ..Default::default()
    };
    eframe::run_native(
        "voxtype-settings",
        options,
        Box::new(move |cc| {
            style(&cc.egui_ctx);
            Ok(Box::new(SettingsApp::new(path, config)))
        }),
    )
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
    /// Focus the search field on the first frame: type to find a setting.
    focus_search: bool,
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
            focus_search: true,
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
        ui.add_space(14.0);
        let search = ui.add(
            egui::TextEdit::singleline(&mut self.search)
                .hint_text("🔍 Search")
                .margin(egui::Margin::symmetric(8, 6))
                .desired_width(f32::INFINITY),
        );
        if std::mem::take(&mut self.focus_search) {
            search.request_focus();
            // Launched from a menu bar icon, the window may open behind
            // whatever app had focus.
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        ui.add_space(12.0);
        for section in schema::SECTIONS {
            if self.keys_in(section).is_empty() {
                continue;
            }
            let selected = self.search.is_empty() && self.section == *section;
            let text = format!("{}   {}", section_icon(section), section);
            let button = egui::Button::selectable(selected, text)
                .min_size(egui::vec2(ui.available_width(), 30.0))
                .corner_radius(6);
            if ui.add(button).clicked() {
                self.section = section;
                self.search.clear();
            }
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if self.restart_pending {
                ui.label("Some changes take effect after a restart.");
                let accent = ui.visuals().selection.bg_fill;
                let restart = egui::Button::new(
                    egui::RichText::new("Restart Voxtype").color(egui::Color32::WHITE),
                )
                .fill(accent)
                .corner_radius(6);
                if ui.add(restart).clicked() {
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
        let searching = !self.search.is_empty();
        let (title, keys) = if searching {
            let search = self.search.clone();
            (format!("Results for “{search}”"), self.matching(&search))
        } else {
            (self.section.to_string(), self.keys_in(self.section))
        };
        ui.add_space(14.0);
        ui.heading(egui::RichText::new(title).strong());
        ui.add_space(10.0);
        if keys.is_empty() {
            ui.weak("Nothing here.");
            return;
        }

        // One card per config table, in schema order: "Feedback" under
        // Audio, "Diarization" under Meeting, one card per engine.
        let mut groups: Vec<(String, Vec<&'static KeySpec>)> = Vec::new();
        for spec in keys {
            let name = if searching {
                spec.section.to_string()
            } else {
                spec.table.to_string()
            };
            match groups.last_mut() {
                Some((last, specs)) if *last == name => specs.push(spec),
                _ => groups.push((name, vec![spec])),
            }
        }

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_max_width(ui.available_width() - 16.0);
                for (i, (name, specs)) in groups.into_iter().enumerate() {
                    if searching || i > 0 {
                        let title = if searching { name } else { humanize(&name) };
                        ui.add_space(6.0);
                        ui.label(egui::RichText::new(title).strong().weak());
                        ui.add_space(4.0);
                    }
                    card(ui).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        let last = specs.len() - 1;
                        for (j, spec) in specs.into_iter().enumerate() {
                            self.row(ui, spec);
                            if j != last {
                                ui.separator();
                            }
                        }
                    });
                    ui.add_space(14.0);
                }
            });
    }

    fn row(&mut self, ui: &mut egui::Ui, spec: &'static KeySpec) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            // Leave the control a fixed column so labels wrap before it.
            let text_width = (ui.available_width() - 250.0).max(160.0);
            ui.vertical(|ui| {
                ui.set_width(text_width);
                let strong = ui.visuals().strong_text_color();
                let label = ui.label(egui::RichText::new(spec.label).size(14.5).color(strong));
                if spec.restart_required {
                    label.on_hover_text("Takes effect after Voxtype restarts");
                }
                ui.label(egui::RichText::new(spec.description).size(12.0).weak());
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.control(ui, spec);
            });
        });
        ui.add_space(6.0);
    }

    fn control(&mut self, ui: &mut egui::Ui, spec: &'static KeySpec) {
        let current = self.value(spec);
        match spec.ty {
            KeyType::Bool => {
                let mut on = current == "true";
                if toggle(ui, &mut on).changed() {
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
                    .width(200.0)
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
        let r = ui.add(egui::TextEdit::singleline(draft).desired_width(180.0));
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
        let sidebar_fill = sidebar_color(ui.visuals());
        if self.restart_pending || self.message.is_some() {
            egui::Panel::bottom("status")
                .show_separator_line(true)
                .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(16, 8)))
                .show_inside(ui, |ui| self.status_bar(ui));
        }
        egui::Panel::left("sections")
            .resizable(false)
            .exact_size(200.0)
            .show_separator_line(false)
            .frame(
                egui::Frame::new()
                    .fill(sidebar_fill)
                    .inner_margin(egui::Margin::symmetric(12, 0)),
            )
            .show_inside(ui, |ui| self.sidebar(ui));
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(content_color(ui.visuals()))
                    .inner_margin(egui::Margin {
                        left: 24,
                        right: 8,
                        top: 0,
                        bottom: 0,
                    }),
            )
            .show_inside(ui, |ui| self.pane(ui));
    }
}

fn style(ctx: &egui::Context) {
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(10.0, 4.0);
        style.spacing.interact_size.y = 26.0;
        for w in [
            &mut style.visuals.widgets.inactive,
            &mut style.visuals.widgets.hovered,
            &mut style.visuals.widgets.active,
            &mut style.visuals.widgets.open,
        ] {
            w.corner_radius = egui::CornerRadius::same(6);
        }
        style.visuals.widgets.noninteractive.corner_radius = egui::CornerRadius::same(6);
        style.visuals.window_corner_radius = egui::CornerRadius::same(10);
        style.visuals.menu_corner_radius = egui::CornerRadius::same(8);
        if let Some(body) = style.text_styles.get_mut(&egui::TextStyle::Body) {
            body.size = 14.0;
        }
        if let Some(button) = style.text_styles.get_mut(&egui::TextStyle::Button) {
            button.size = 14.0;
        }
        if let Some(heading) = style.text_styles.get_mut(&egui::TextStyle::Heading) {
            heading.size = 22.0;
        }
    });
}

/// A rounded card, a shade apart from the window, like a grouped form.
fn card(ui: &egui::Ui) -> egui::Frame {
    let visuals = ui.visuals();
    let (fill, stroke) = if visuals.dark_mode {
        (egui::Color32::from_gray(38), egui::Color32::from_gray(52))
    } else {
        (egui::Color32::WHITE, egui::Color32::from_gray(222))
    };
    egui::Frame::new()
        .fill(fill)
        .stroke(egui::Stroke::new(1.0_f32, stroke))
        .corner_radius(10)
        .inner_margin(egui::Margin::symmetric(16, 6))
}

fn content_color(visuals: &egui::Visuals) -> egui::Color32 {
    if visuals.dark_mode {
        egui::Color32::from_gray(30)
    } else {
        egui::Color32::from_gray(246)
    }
}

/// An on/off switch, the way macOS and Windows settings show a boolean.
fn toggle(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    let size = egui::vec2(38.0, 22.0);
    let (rect, mut response) = ui.allocate_exact_size(size, egui::Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    if ui.is_rect_visible(rect) {
        let t = ui.ctx().animate_bool_responsive(response.id, *on);
        let visuals = ui.visuals();
        let off_fill = if visuals.dark_mode {
            egui::Color32::from_gray(70)
        } else {
            egui::Color32::from_gray(200)
        };
        let fill = off_fill.lerp_to_gamma(visuals.selection.bg_fill, t);
        let radius = rect.height() / 2.0;
        ui.painter().rect_filled(rect, radius, fill);
        let x = egui::lerp((rect.left() + radius)..=(rect.right() - radius), t);
        let knob = egui::pos2(x, rect.center().y);
        ui.painter()
            .circle_filled(knob, radius - 2.5, egui::Color32::WHITE);
    }
    response
}

fn sidebar_color(visuals: &egui::Visuals) -> egui::Color32 {
    if visuals.dark_mode {
        egui::Color32::from_gray(24)
    } else {
        egui::Color32::from_gray(236)
    }
}

fn section_icon(section: &str) -> &'static str {
    match section {
        "Engine" => "⚙",
        "Hotkey" => "⌨",
        "Audio" => "🎤",
        "Output" => "✏",
        "Text" => "🔤",
        "VAD" => "〰",
        "Meeting" => "👥",
        "Notifications" => "🔔",
        "OSD" => "💬",
        "Status" => "📊",
        _ => "🔧",
    }
}

/// `meeting.diarization` → "Diarization", `output.post_process` → "Post process".
fn humanize(table: &str) -> String {
    let last = table.rsplit('.').next().unwrap_or(table).replace('_', " ");
    let mut chars = last.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
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
