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
    was_visible: bool,
}

/// Bounds of the display holding the mouse pointer, as (x, y, width, height)
/// in the global point space egui positions windows in. `CGDisplayBounds` is
/// already in points, like `ViewportCommand::OuterPosition`, so the two agree
/// without conversion. Falls back to the main display if the pointer cannot
/// be read.
fn cursor_display_bounds() -> (f32, f32, f32, f32) {
    use core_graphics::display::CGDisplay;
    use core_graphics::event::CGEvent;
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

    // An event created from a null-ish source carries the current pointer
    // location — the standard way to ask CoreGraphics where the mouse is.
    let cursor = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .ok()
        .and_then(|source| CGEvent::new(source).ok())
        .map(|event| event.location());

    let under_cursor =
        cursor
            .zip(CGDisplay::active_displays().ok())
            .and_then(|(point, displays)| {
                displays
                    .iter()
                    .map(|id| CGDisplay::new(*id).bounds())
                    .find(|b| {
                        point.x >= b.origin.x
                            && point.x < b.origin.x + b.size.width
                            && point.y >= b.origin.y
                            && point.y < b.origin.y + b.size.height
                    })
            });

    let bounds = under_cursor.unwrap_or_else(|| CGDisplay::main().bounds());
    (
        bounds.origin.x as f32,
        bounds.origin.y as f32,
        bounds.size.width as f32,
        bounds.size.height as f32,
    )
}

/// Top-left corner for the card on a display given as (x, y, width, height):
/// centred horizontally, `top_margin` down, never past the bottom edge. The
/// display's own origin is added, which is what makes a non-primary display
/// work — the card used to be laid out as though every display began at
/// (0, 0).
fn card_origin(
    display: (f32, f32, f32, f32),
    config: &crate::osd::config::OsdConfig,
) -> (f32, f32) {
    let (dx, dy, dw, dh) = display;
    let w = config.width_px as f32 + 2.0 * GLOW_PAD;
    let h = config.height_px as f32 + 2.0 * GLOW_PAD;
    let x = dx + (dw - w) / 2.0;
    let y = dy
        + (dh * config.top_margin.clamp(0.0, 1.0) - GLOW_PAD).min(dh - h - config.margin_px as f32);
    (x, y)
}

impl eframe::App for CardApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        let visible = self.model.poll();

        // Place the card when it is about to appear, not once at startup: the
        // pointer may have moved to another display between two dictations,
        // and `monitor_size` alone cannot say where that display begins.
        // Moving the viewport takes effect next frame, so skip drawing this
        // one — otherwise the card flashes at its previous spot.
        if visible && !self.was_visible {
            self.was_visible = true;
            let (x, y) = card_origin(cursor_display_bounds(), &self.model.config);
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(x, y)));
            ctx.request_repaint();
            return;
        }
        self.was_visible = visible;

        if !visible {
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
        // On-screen from the start: macOS reports no monitor for a window
        // outside every display, so an off-screen card never learns where to
        // go. It is transparent until there is something to show.
        .with_position([0.0, 0.0])
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
                was_visible: false,
            }))
        }),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}

#[cfg(test)]
mod tests {
    use super::{card_origin, GLOW_PAD};
    use crate::osd::config::OsdConfig;

    /// The real displays of a docked MacBook, as CoreGraphics reports them.
    const MACBOOK: (f32, f32, f32, f32) = (0.0, 0.0, 2056.0, 1329.0);
    const ABOVE: (f32, f32, f32, f32) = (0.0, -1440.0, 2560.0, 1440.0);
    const LEFT_PORTRAIT: (f32, f32, f32, f32) = (-1440.0, -1782.0, 1440.0, 2560.0);

    fn config() -> OsdConfig {
        OsdConfig::default()
    }

    #[test]
    fn centred_on_the_primary_display() {
        let cfg = config();
        let (x, _) = card_origin(MACBOOK, &cfg);
        let card_width = cfg.width_px as f32 + 2.0 * GLOW_PAD;
        assert_eq!(x, (2056.0 - card_width) / 2.0);
    }

    #[test]
    fn a_displays_origin_is_added() {
        // The regression this guards: a card laid out as though every display
        // began at (0, 0) lands on the primary one, whatever screen is in use.
        let cfg = config();
        let (x_above, y_above) = card_origin(ABOVE, &cfg);
        assert!(y_above < 0.0, "a display above the primary has negative y");
        assert!(
            y_above >= -1440.0 && y_above < 0.0,
            "y={y_above} should sit inside the display's own band"
        );

        let (x_left, y_left) = card_origin(LEFT_PORTRAIT, &cfg);
        assert!(x_left < 0.0, "a display left of the primary has negative x");
        assert!(
            y_left >= -1782.0 && y_left < 778.0,
            "y={y_left} should sit inside the portrait display"
        );

        // Each display centres the card on its own width, so the three differ.
        let (x_main, _) = card_origin(MACBOOK, &cfg);
        assert_ne!(x_main, x_above);
        assert_ne!(x_main, x_left);
    }

    #[test]
    fn never_pushed_past_the_bottom_edge() {
        let mut cfg = config();
        cfg.top_margin = 1.0;
        let (_, y) = card_origin(ABOVE, &cfg);
        let card_height = cfg.height_px as f32 + 2.0 * GLOW_PAD;
        let bottom = -1440.0 + 1440.0;
        assert!(
            y + card_height <= bottom,
            "y={y} plus the card height must stay above {bottom}"
        );
    }
}
