//! macOS presenter: an eframe window showing the CPU-drawn card.
//!
//! Every Mac has OpenGL, so eframe's glow renderer is enough. The window is
//! transparent, borderless, floating above other windows and transparent to
//! the mouse; it is created inactive so voxtype keeps typing into the
//! focused application.

use eframe::egui;

use super::card::{pixel_size, GLOW_PAD};
use super::{Model, STATE_POLL};

struct CardApp {
    model: Model,
    texture: Option<egui::TextureHandle>,
    positioned: bool,
}

impl eframe::App for CardApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        if !self.positioned {
            if let Some(monitor) = ctx.input(|i| i.viewport().monitor_size) {
                let w = self.model.config.width_px as f32 + 2.0 * GLOW_PAD;
                let h = self.model.config.height_px as f32 + 2.0 * GLOW_PAD;
                let x = (monitor.x - w) / 2.0;
                let y = (monitor.y * self.model.config.top_margin.clamp(0.0, 1.0) - GLOW_PAD)
                    .min(monitor.y - h - self.model.config.margin_px as f32);
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(x, y)));
                self.positioned = true;
            }
        }

        if !self.model.poll() {
            ctx.request_repaint_after(STATE_POLL);
            return;
        }
        ctx.request_repaint();

        let scale = ctx.pixels_per_point();
        let Some(pixmap) = self.model.render(scale) else {
            return;
        };
        let image = egui::ColorImage::from_rgba_premultiplied(
            [pixmap.width() as usize, pixmap.height() as usize],
            pixmap.data(),
        );
        if let Some(texture) = &mut self.texture {
            texture.set(image, egui::TextureOptions::LINEAR);
        } else {
            self.texture =
                Some(ctx.load_texture("voxtype-card", image, egui::TextureOptions::LINEAR));
        }
        let Some(texture) = &self.texture else {
            return;
        };
        let rect = egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(
                pixmap.width() as f32 / scale,
                pixmap.height() as f32 / scale,
            ),
        );
        ui.painter().image(
            texture.id(),
            rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    }
}

pub fn run(model: Model) -> anyhow::Result<()> {
    let (w, h) = pixel_size(
        model.config.width_px as f32,
        model.config.height_px as f32,
        1.0,
    );
    let viewport = egui::ViewportBuilder::default()
        .with_title("Voxtype")
        .with_inner_size([w as f32, h as f32])
        // Off-screen until the monitor size is known, so nothing flashes.
        .with_position([-10_000.0, -10_000.0])
        .with_decorations(false)
        .with_transparent(true)
        .with_resizable(false)
        .with_always_on_top()
        .with_taskbar(false)
        .with_active(false)
        .with_has_shadow(false)
        .with_mouse_passthrough(true);
    let options = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "voxtype-osd-desktop",
        options,
        Box::new(move |_cc| {
            Ok(Box::new(CardApp {
                model,
                texture: None,
                positioned: false,
            }))
        }),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}
