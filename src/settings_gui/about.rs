//! The About pane: which build this is, and where its files live.
//!
//! The version is the one `voxtype --version` prints, including the commit
//! (`1.1.1+g218d29d`), so a colleague can say exactly what they run.

use std::path::Path;

use eframe::egui;

use super::{card, SettingsApp};

const RELEASES: &str = "https://github.com/ChristianLemer/voxtype/releases";
const PROJECT: &str = "https://github.com/ChristianLemer/voxtype";

impl SettingsApp {
    pub(super) fn about_pane(&mut self, ui: &mut egui::Ui) {
        let version = crate::cli::VERSION;
        ui.add_space(14.0);
        ui.heading(egui::RichText::new("About").strong());
        ui.add_space(10.0);

        let mut copied = false;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_max_width(ui.available_width() - 16.0);
                card(ui).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            let strong = ui.visuals().strong_text_color();
                            ui.label(egui::RichText::new("Voxtype").size(20.0).color(strong));
                            if let Some(tag) = crate::cli::RELEASE_TAG {
                                ui.label(egui::RichText::new(format!("Release {tag}")).size(14.5));
                            }
                            ui.label(egui::RichText::new(format!("Version {version}")).size(14.5));
                            ui.label(
                                egui::RichText::new(
                                    "Desktop build of voxtype for Windows and macOS, \
                                     from the ChristianLemer/voxtype fork.",
                                )
                                .size(12.0)
                                .weak(),
                            );
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Copy version").clicked() {
                                ui.ctx().copy_text(match crate::cli::RELEASE_TAG {
                                    Some(tag) => format!("voxtype {version} ({tag})"),
                                    None => format!("voxtype {version}"),
                                });
                                copied = true;
                            }
                        });
                    });
                    ui.add_space(8.0);
                });

                ui.add_space(14.0);
                card(ui).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let config = self.path.clone();
                    row(ui, "Configuration file", &config, "Open", || open(&config));
                    ui.separator();
                    let logs = logs_dir();
                    row(ui, "Logs", &logs, "Open folder", || open(&logs));
                    ui.separator();
                    link_row(ui, "Releases", RELEASES);
                    ui.separator();
                    link_row(ui, "Project", PROJECT);
                });
            });

        if copied {
            self.message = Some((false, format!("Copied voxtype {version}")));
        }
    }
}

fn row(ui: &mut egui::Ui, name: &str, path: &Path, action: &str, on_click: impl FnOnce()) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            let strong = ui.visuals().strong_text_color();
            ui.label(egui::RichText::new(name).size(14.5).color(strong));
            ui.label(
                egui::RichText::new(path.display().to_string())
                    .size(12.0)
                    .weak(),
            );
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button(action).clicked() {
                on_click();
            }
        });
    });
    ui.add_space(6.0);
}

fn link_row(ui: &mut egui::Ui, name: &str, url: &str) {
    row(ui, name, Path::new(url), "Open", || open_url(url));
}

/// Where the daemon writes its log on this system.
fn logs_dir() -> std::path::PathBuf {
    #[cfg(target_os = "macos")]
    {
        dirs::home_dir()
            .map(|h| h.join("Library/Logs/voxtype"))
            .unwrap_or_else(crate::config::Config::runtime_dir)
    }
    #[cfg(windows)]
    {
        crate::config::Config::data_dir().join("logs")
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        crate::config::Config::runtime_dir()
    }
}

fn open(path: &Path) {
    open_url(&path.to_string_lossy());
}

/// Hand a path or URL to the system's default handler.
fn open_url(target: &str) {
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(target).spawn();
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer").arg(target).spawn();
    #[cfg(not(any(target_os = "macos", windows)))]
    let _ = std::process::Command::new("xdg-open").arg(target).spawn();
}
