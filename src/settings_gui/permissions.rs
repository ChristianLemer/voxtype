//! The Permissions pane (macOS): what dictation needs from System Settings.
//!
//! Read by `voxtype permissions --json` in a fresh process each time
//! ([`crate::setup::permissions::check_fresh`]): a process keeps the answer
//! macOS first gave it, so re-reading in this long-lived window could show
//! "Not allowed" after the switch was turned on. The window runs as a child
//! of Voxtype.app, so those processes read the app's grants. The pane
//! re-reads every few seconds while open, and Refresh reads at once; the
//! time of the last reading is shown so a stale state cannot pass for a
//! fresh one.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use eframe::egui;

use super::{card, SettingsApp};
use crate::setup::permissions::{self, Report};

const RECHECK: Duration = Duration::from_secs(3);

#[derive(Default)]
pub(super) struct State {
    /// When the last reading finished, what it said, and its wall-clock time.
    checked: Option<(Instant, Report, String)>,
    /// A reading in progress, in a background thread.
    reading: Option<mpsc::Receiver<Report>>,
    /// The repair button was pressed once and waits for a confirmation.
    confirming: bool,
    /// Why the last repair failed, kept on screen with the manual steps.
    repair_failed: Option<String>,
}

enum Repair {
    Ask,
    Confirm,
    Cancel,
}

struct Grant {
    name: &'static str,
    purpose: &'static str,
    /// `None` when macOS gives no way to read it without prompting.
    granted: Option<bool>,
    /// The Privacy & Security anchor in System Settings.
    anchor: &'static str,
}

impl SettingsApp {
    /// Start a reading in a fresh process, unless one is already running.
    fn read_permissions(&mut self, ctx: &egui::Context) {
        if self.permissions.reading.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let report = permissions::check_fresh().unwrap_or_else(permissions::check);
            let _ = tx.send(report);
            ctx.request_repaint();
        });
        self.permissions.reading = Some(rx);
    }

    pub(super) fn permissions_pane(&mut self, ui: &mut egui::Ui) {
        if let Some(rx) = &self.permissions.reading {
            if let Ok(report) = rx.try_recv() {
                let at = chrono::Local::now().format("%H:%M:%S").to_string();
                self.permissions.checked = Some((Instant::now(), report, at));
                self.permissions.reading = None;
            }
        }
        let due = self
            .permissions
            .checked
            .as_ref()
            .is_none_or(|(at, _, _)| at.elapsed() >= RECHECK);
        if due {
            self.read_permissions(ui.ctx());
        }
        ui.ctx().request_repaint_after(RECHECK);
        let Some((_, report, checked_at)) = self.permissions.checked.clone() else {
            ui.add_space(14.0);
            ui.heading(egui::RichText::new("Permissions").strong());
            ui.add_space(10.0);
            ui.weak("Reading what macOS allows…");
            return;
        };
        let reading = self.permissions.reading.is_some();
        let marker = permissions::reset_marker();
        let just_reset = marker.exists();
        if just_reset && report.usable() {
            let _ = std::fs::remove_file(&marker);
        }

        let grants = [
            Grant {
                name: "Input Monitoring",
                purpose: "Lets Voxtype see the hotkey.",
                granted: Some(report.listen_event),
                anchor: "Privacy_ListenEvent",
            },
            Grant {
                name: "Accessibility",
                purpose: "Lets Voxtype type the transcribed text.",
                granted: Some(report.post_event),
                anchor: "Privacy_Accessibility",
            },
            Grant {
                name: "Microphone",
                purpose: "Lets Voxtype hear you. macOS asks the first time you dictate.",
                granted: None,
                anchor: "Privacy_Microphone",
            },
        ];

        ui.add_space(14.0);
        let mut refresh = false;
        ui.horizontal(|ui| {
            ui.heading(egui::RichText::new("Permissions").strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(16.0);
                if ui
                    .add_enabled(!reading, egui::Button::new("Refresh"))
                    .clicked()
                {
                    refresh = true;
                }
                let status = if reading {
                    "Checking…".to_string()
                } else {
                    format!("Checked at {checked_at}")
                };
                ui.weak(status);
            });
        });
        if refresh {
            self.read_permissions(ui.ctx());
        }
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(
                "macOS asks you, not Voxtype, to allow these. Open System Settings, \
                 turn Voxtype on, and this page updates by itself.",
            )
            .weak(),
        );
        ui.add_space(10.0);
        if just_reset && !report.usable() {
            card(ui).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.add_space(6.0);
                ui.label(egui::RichText::new("Voxtype was just updated").strong());
                ui.label(
                    egui::RichText::new(
                        "macOS ties these permissions to the exact build, so Voxtype cleared \
                         them and macOS asks again. Open Settings below, switch Voxtype on, then restart it.",
                    )
                    .weak(),
                );
                ui.add_space(6.0);
            });
            ui.add_space(10.0);
        }

        let mut repair: Option<Repair> = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_max_width(ui.available_width() - 16.0);
                card(ui).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let last = grants.len() - 1;
                    for (i, g) in grants.iter().enumerate() {
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                let strong = ui.visuals().strong_text_color();
                                ui.label(egui::RichText::new(g.name).size(14.5).color(strong));
                                ui.label(egui::RichText::new(g.purpose).size(12.0).weak());
                            });
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.button("Open Settings").clicked() {
                                        open_privacy_pane(g.anchor);
                                    }
                                    ui.add_space(8.0);
                                    status_chip(ui, g.granted);
                                },
                            );
                        });
                        ui.add_space(6.0);
                        if i != last {
                            ui.separator();
                        }
                    }
                });

                ui.add_space(14.0);
                let (title, text) = match report.remedy() {
                    None => (
                        "Ready",
                        "Voxtype has what it needs to see the hotkey and type.",
                    ),
                    Some(advice) => ("Dictation cannot work yet", advice),
                };
                let confirming = self.permissions.confirming;
                let failed = self.permissions.repair_failed.clone();
                card(ui).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new(title).strong());
                    ui.label(egui::RichText::new(text).weak());
                    if report.usable() {
                        ui.add_space(6.0);
                        return;
                    }
                    ui.add_space(8.0);
                    if confirming {
                        ui.label(
                            "This removes Voxtype from Input Monitoring and Accessibility, \
                             then asks macOS for both again. Allow Voxtype when it asks.",
                        );
                        ui.horizontal(|ui| {
                            if ui.button("Repair").clicked() {
                                repair = Some(Repair::Confirm);
                            }
                            if ui.button("Cancel").clicked() {
                                repair = Some(Repair::Cancel);
                            }
                        });
                    } else if ui.button("Repair permissions…").clicked() {
                        repair = Some(Repair::Ask);
                    }
                    if let Some(e) = failed {
                        ui.add_space(6.0);
                        let red = ui.visuals().error_fg_color;
                        ui.colored_label(red, format!("macOS did not reset the permissions: {e}"));
                        ui.label(
                            "Remove Voxtype with − under Input Monitoring and under \
                             Accessibility, then add it back with +.",
                        );
                    }
                    ui.add_space(6.0);
                });
            });

        match repair {
            Some(Repair::Ask) => self.permissions.confirming = true,
            Some(Repair::Cancel) => self.permissions.confirming = false,
            Some(Repair::Confirm) => {
                self.permissions.confirming = false;
                match permissions::reset() {
                    Ok(()) => {
                        self.permissions.repair_failed = None;
                        permissions::request();
                        // The daemon opened its event tap at startup; it
                        // needs a restart to use the new grants.
                        self.restart_pending = true;
                        self.message = Some((
                            false,
                            "Permissions reset. Allow Voxtype when macOS asks, then restart it."
                                .into(),
                        ));
                    }
                    Err(e) => {
                        self.permissions.repair_failed = Some(e);
                        open_privacy_pane("Privacy_ListenEvent");
                    }
                }
                self.permissions.checked = None;
                self.read_permissions(ui.ctx());
            }
            None => {}
        }
    }
}

fn status_chip(ui: &mut egui::Ui, granted: Option<bool>) {
    let (color, text) = match granted {
        Some(true) => (egui::Color32::from_rgb(52, 199, 89), "Allowed"),
        Some(false) => (egui::Color32::from_rgb(255, 69, 58), "Not allowed"),
        None => (ui.visuals().weak_text_color(), "Asked on first use"),
    };
    ui.label(egui::RichText::new(text).color(color));
    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.5, color);
}

fn open_privacy_pane(anchor: &str) {
    let url = format!("x-apple.systempreferences:com.apple.preference.security?{anchor}");
    let _ = std::process::Command::new("open").arg(url).spawn();
}
