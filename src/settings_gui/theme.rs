//! The settings window in the colours of the desktop.
//!
//! With an Omarchy theme, the window takes the theme's background,
//! foreground and accent (the palette the OSD already reads through
//! [`crate::osd::theme`]) and follows a theme switch while it is open.
//! Without one, on any system, it keeps egui's light or dark look with the
//! neutral greys it always had.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use eframe::egui::{self, Color32};

use crate::osd::theme::{self as omarchy, ThemeWatchHandle};
use crate::osd::visual::{Color, Palette};

/// The window's own surfaces, which egui's visuals have no names for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Surfaces {
    pub sidebar: Color32,
    pub content: Color32,
    pub card: Color32,
    pub card_stroke: Color32,
    pub toggle_off: Color32,
}

impl Surfaces {
    /// The greys used when no desktop theme applies.
    pub fn neutral(dark: bool) -> Self {
        if dark {
            Self {
                sidebar: Color32::from_gray(24),
                content: Color32::from_gray(30),
                card: Color32::from_gray(38),
                card_stroke: Color32::from_gray(52),
                toggle_off: Color32::from_gray(70),
            }
        } else {
            Self {
                sidebar: Color32::from_gray(236),
                content: Color32::from_gray(246),
                card: Color32::WHITE,
                card_stroke: Color32::from_gray(222),
                toggle_off: Color32::from_gray(200),
            }
        }
    }
}

/// The surfaces in effect: the desktop theme's when one was applied, the
/// neutral greys otherwise.
pub fn surfaces(ui: &egui::Ui) -> Surfaces {
    ui.ctx()
        .data(|d| d.get_temp::<Surfaces>(egui::Id::NULL))
        .unwrap_or_else(|| Surfaces::neutral(ui.visuals().dark_mode))
}

/// Paint the window in the active Omarchy theme. Returns `false`, leaving
/// egui's look untouched, when there is no Omarchy theme.
pub fn apply_omarchy(ctx: &egui::Context) -> bool {
    let has_theme = omarchy::omarchy_theme_dirs()
        .unwrap_or_default()
        .iter()
        .any(|dir| dir.join("colors.toml").is_file());
    if !has_theme {
        ctx.data_mut(|d| d.remove::<Surfaces>(egui::Id::NULL));
        return false;
    }
    let (visuals, surfaces) = from_palette(&omarchy::load_palette());
    let theme = if visuals.dark_mode {
        egui::Theme::Dark
    } else {
        egui::Theme::Light
    };
    ctx.set_theme(theme);
    ctx.set_visuals_of(theme, visuals);
    ctx.data_mut(|d| d.insert_temp(egui::Id::NULL, surfaces));
    true
}

/// Watches the Omarchy theme while the window is open.
pub struct Follow {
    changed: Arc<AtomicBool>,
    _watch: Option<ThemeWatchHandle>,
}

impl Follow {
    pub fn start(ctx: &egui::Context) -> Self {
        let changed = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&changed);
        let ctx = ctx.clone();
        let watch = omarchy::watch_current_theme(move || {
            flag.store(true, Ordering::SeqCst);
            ctx.request_repaint();
        });
        Self {
            changed,
            _watch: watch,
        }
    }

    /// True once after each theme switch.
    pub fn changed(&self) -> bool {
        self.changed.swap(false, Ordering::SeqCst)
    }
}

/// Visuals and surfaces for a palette. Every shade is the background mixed
/// with the foreground, so a light theme works the same way as a dark one.
pub fn from_palette(p: &Palette) -> (egui::Visuals, Surfaces) {
    let bg = color32(p.background);
    let fg = color32(p.foreground);
    let accent = color32(p.accent);
    let dark = luminance(bg) < 0.5;
    let shade = |t: f32| bg.lerp_to_gamma(fg, t);
    let strong = fg.lerp_to_gamma(if dark { Color32::WHITE } else { Color32::BLACK }, 0.4);

    let surfaces = Surfaces {
        sidebar: bg,
        content: shade(0.04),
        card: shade(0.08),
        card_stroke: shade(0.16),
        toggle_off: shade(0.30),
    };

    let mut v = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    v.panel_fill = surfaces.content;
    v.window_fill = surfaces.card;
    v.window_stroke.color = surfaces.card_stroke;
    v.faint_bg_color = surfaces.card;
    v.extreme_bg_color = bg;
    v.text_edit_bg_color = Some(bg);
    v.code_bg_color = surfaces.card;
    v.hyperlink_color = accent;
    v.warn_fg_color = color32(p.meter_mid);
    v.error_fg_color = color32(p.meter_high);
    v.selection.bg_fill = accent;
    v.selection.stroke.color = if luminance(accent) > 0.5 {
        Color32::BLACK
    } else {
        Color32::WHITE
    };

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = surfaces.content;
    w.noninteractive.weak_bg_fill = surfaces.content;
    w.noninteractive.bg_stroke.color = surfaces.card_stroke;
    w.noninteractive.fg_stroke.color = fg;
    for (state, t) in [
        (&mut w.inactive, 0.12),
        (&mut w.hovered, 0.18),
        (&mut w.active, 0.24),
        (&mut w.open, 0.12),
    ] {
        state.bg_fill = shade(t);
        state.weak_bg_fill = shade(t);
        state.fg_stroke.color = fg;
    }
    w.hovered.bg_stroke.color = shade(0.30);
    w.hovered.fg_stroke.color = strong;
    w.active.fg_stroke.color = strong;

    (v, surfaces)
}

fn color32(c: Color) -> Color32 {
    let byte = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgb(byte(c.r), byte(c.g), byte(c.b))
}

/// Relative luminance, close enough to sort light from dark.
fn luminance(c: Color32) -> f32 {
    (0.2126 * f32::from(c.r()) + 0.7152 * f32::from(c.g()) + 0.0722 * f32::from(c.b())) / 255.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Color {
        let n = u32::from_str_radix(s.trim_start_matches('#'), 16).unwrap();
        let ch = |shift: u32| ((n >> shift) & 0xff) as f32 / 255.0;
        Color::rgb(ch(16), ch(8), ch(0))
    }

    fn palette(background: &str, foreground: &str, accent: &str) -> Palette {
        Palette {
            // The OSD keeps its background translucent; the window must not.
            background: hex(background).with_alpha(0.85),
            foreground: hex(foreground),
            accent: hex(accent),
            ..Palette::fallback()
        }
    }

    #[test]
    fn dark_theme_gives_dark_visuals_in_its_colours() {
        let (v, s) = from_palette(&palette("#000000", "#a9b1d6", "#7aa2f7"));
        assert!(v.dark_mode);
        assert_eq!(s.sidebar, Color32::from_rgb(0, 0, 0));
        assert_eq!(v.selection.bg_fill, Color32::from_rgb(0x7a, 0xa2, 0xf7));
        assert_eq!(v.hyperlink_color, v.selection.bg_fill);
        assert_eq!(v.panel_fill, s.content);
    }

    #[test]
    fn light_theme_gives_light_visuals() {
        let (v, s) = from_palette(&palette("#faf4ed", "#575279", "#d7827e"));
        assert!(!v.dark_mode);
        assert!(luminance(s.sidebar) > luminance(s.card_stroke));
    }

    #[test]
    fn surfaces_step_from_background_towards_foreground() {
        let (_, s) = from_palette(&palette("#1a1b26", "#c0caf5", "#7aa2f7"));
        let steps = [s.sidebar, s.content, s.card, s.card_stroke, s.toggle_off];
        for pair in steps.windows(2) {
            assert!(luminance(pair[0]) < luminance(pair[1]), "{pair:?}");
        }
    }

    #[test]
    fn selected_text_stays_readable_on_the_accent() {
        let (light_accent, _) = from_palette(&palette("#000000", "#ffffff", "#e0e0a0"));
        assert_eq!(light_accent.selection.stroke.color, Color32::BLACK);
        let (dark_accent, _) = from_palette(&palette("#000000", "#ffffff", "#203060"));
        assert_eq!(dark_accent.selection.stroke.color, Color32::WHITE);
    }

    #[test]
    fn neutral_surfaces_keep_the_previous_greys() {
        let dark = Surfaces::neutral(true);
        assert_eq!(dark.content, Color32::from_gray(30));
        assert_eq!(dark.card, Color32::from_gray(38));
        let light = Surfaces::neutral(false);
        assert_eq!(light.card, Color32::WHITE);
        assert_eq!(light.sidebar, Color32::from_gray(236));
    }
}
