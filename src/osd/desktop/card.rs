//! The recording card, drawn on the CPU.
//!
//! Reproduces the Quickshell card from Omarchy: dark rounded card, border in
//! the daemon state's colour with a glow that swells with the voice, a
//! microphone glyph, the waveform and a peak meter beneath it. The result is a
//! premultiplied RGBA pixmap that each platform presenter puts on screen.

use tiny_skia::{FillRule, LineCap, Paint, Path, PathBuilder, Pixmap, Rect, Stroke, Transform};

use crate::osd::visual::{peak_meter_fraction, Color, EnvelopeColumn, MeterZone, Palette};

/// Room around the card for the glow, in logical pixels.
pub const GLOW_PAD: f32 = 20.0;
const CORNER_RADIUS: f32 = 12.0;
const PADDING: f32 = 14.0;
const ICON_W: f32 = 28.0;
const BORDER_W: f32 = 2.0;
pub const METER_FLOOR_DBFS: f32 = -60.0;

pub const RECORDING: Color = Color::rgb(
    0xe0 as f32 / 255.0,
    0x6c as f32 / 255.0,
    0x75 as f32 / 255.0,
);
pub const STREAMING: Color = Color::rgb(
    0x61 as f32 / 255.0,
    0xaf as f32 / 255.0,
    0xef as f32 / 255.0,
);
pub const TRANSCRIBING: Color = Color::rgb(
    0xe5 as f32 / 255.0,
    0xc0 as f32 / 255.0,
    0x7b as f32 / 255.0,
);
pub const IDLE: Color = Color::rgb(
    0xab as f32 / 255.0,
    0xb2 as f32 / 255.0,
    0xbf as f32 / 255.0,
);

/// Everything one frame of the card depends on.
pub struct Scene<'a> {
    /// Card size in logical pixels, without the glow padding.
    pub width: f32,
    pub height: f32,
    pub palette: &'a Palette,
    pub state_color: Color,
    /// Background opacity multiplier from `[osd] opacity`.
    pub opacity: f32,
    /// Voice level 0..=1 driving the glow; 0 hides it.
    pub energy: f32,
    pub glow: bool,
    pub envelope: &'a [EnvelopeColumn],
    pub waveform_gain: f32,
    pub peak_dbfs: f32,
    pub held_dbfs: f32,
}

/// Pixel size of the pixmap for a card at `scale` (physical / logical).
pub fn pixel_size(width: f32, height: f32, scale: f32) -> (u32, u32) {
    (
        ((width + 2.0 * GLOW_PAD) * scale).ceil() as u32,
        ((height + 2.0 * GLOW_PAD) * scale).ceil() as u32,
    )
}

/// Number of waveform columns for a card of this width.
pub fn waveform_columns(width: f32) -> usize {
    (width - 2.0 * PADDING - ICON_W - 10.0).max(32.0) as usize
}

fn paint(c: Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(
        (c.r.clamp(0.0, 1.0) * 255.0) as u8,
        (c.g.clamp(0.0, 1.0) * 255.0) as u8,
        (c.b.clamp(0.0, 1.0) * 255.0) as u8,
        (c.a.clamp(0.0, 1.0) * 255.0) as u8,
    );
    p.anti_alias = true;
    p
}

pub(crate) fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<Path> {
    let r = r.min(w / 2.0).min(h / 2.0);
    // Cubic approximation of a quarter circle.
    let k = r * 0.552_284_8;
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish()
}

pub fn render(scene: &Scene, scale: f32) -> Option<Pixmap> {
    let (pw, ph) = pixel_size(scene.width, scene.height, scale);
    let mut pixmap = Pixmap::new(pw, ph)?;
    let t = Transform::from_scale(scale, scale);

    let (cx, cy, cw, ch) = (GLOW_PAD, GLOW_PAD, scene.width, scene.height);

    if scene.glow {
        draw_glow(&mut pixmap, t, scene, cx, cy, cw, ch);
    }

    // Card body and border.
    let mut bg = scene.palette.background;
    bg.a = (bg.a * scene.opacity / 0.95).clamp(0.0, 1.0);
    if let Some(body) = rounded_rect(cx, cy, cw, ch, CORNER_RADIUS) {
        pixmap.fill_path(&body, &paint(bg), FillRule::Winding, t, None);
    }
    let half = BORDER_W / 2.0;
    if let Some(border) = rounded_rect(
        cx + half,
        cy + half,
        cw - BORDER_W,
        ch - BORDER_W,
        CORNER_RADIUS - half,
    ) {
        let stroke = Stroke {
            width: BORDER_W,
            ..Stroke::default()
        };
        pixmap.stroke_path(&border, &paint(scene.state_color), &stroke, t, None);
    }

    draw_microphone(
        &mut pixmap,
        t,
        cx + PADDING + ICON_W / 2.0,
        cy + ch / 2.0,
        scene.state_color,
    );

    // Waveform above, meter below, in the column right of the icon.
    let left = cx + PADDING + ICON_W + 10.0;
    let width = cx + cw - PADDING - left;
    let content_h = (ch - 2.0 * PADDING).max(24.0);
    let top = cy + ch / 2.0 - content_h / 2.0;
    let meter_h = 4.0;
    let wave_h = content_h - meter_h - 4.0;
    draw_waveform(
        &mut pixmap,
        t,
        left,
        top,
        width,
        wave_h,
        scene.envelope,
        scene.waveform_gain,
        scene.palette.accent,
    );
    draw_meter(
        &mut pixmap,
        t,
        left,
        top + wave_h + 4.0,
        width,
        meter_h,
        scene,
    );

    Some(pixmap)
}

fn draw_glow(pixmap: &mut Pixmap, t: Transform, scene: &Scene, x: f32, y: f32, w: f32, h: f32) {
    // Widening, fading outlines approximate the Quickshell shadow blur.
    let base = 0.18 + scene.energy * 0.34;
    for step in 1..=8 {
        let spread = step as f32 * (1.6 + scene.energy * 1.0);
        let alpha = base * (1.0 - step as f32 / 9.0).powf(1.5) * 0.55;
        let Some(path) = rounded_rect(
            x - spread,
            y - spread,
            w + 2.0 * spread,
            h + 2.0 * spread,
            CORNER_RADIUS + spread,
        ) else {
            continue;
        };
        let stroke = Stroke {
            width: 2.2,
            ..Stroke::default()
        };
        pixmap.stroke_path(
            &path,
            &paint(scene.state_color.with_alpha(alpha)),
            &stroke,
            t,
            None,
        );
    }
}

fn draw_microphone(pixmap: &mut Pixmap, t: Transform, cx: f32, cy: f32, color: Color) {
    let fill = paint(color);
    if let Some(capsule) = rounded_rect(cx - 4.0, cy - 11.0, 8.0, 14.0, 4.0) {
        pixmap.fill_path(&capsule, &fill, FillRule::Winding, t, None);
    }
    let stroke = Stroke {
        width: 2.2,
        line_cap: LineCap::Round,
        ..Stroke::default()
    };
    // Cradle: a half circle under the capsule.
    let mut pb = PathBuilder::new();
    for i in 0..=20 {
        let a = std::f32::consts::PI * i as f32 / 20.0;
        let (px, py) = (cx - 7.5 * a.cos(), cy - 3.0 + 7.5 * a.sin());
        if i == 0 {
            pb.move_to(px, py);
        } else {
            pb.line_to(px, py);
        }
    }
    pb.move_to(cx, cy + 4.5);
    pb.line_to(cx, cy + 9.0);
    pb.move_to(cx - 4.5, cy + 9.0);
    pb.line_to(cx + 4.5, cy + 9.0);
    if let Some(path) = pb.finish() {
        pixmap.stroke_path(&path, &fill, &stroke, t, None);
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_waveform(
    pixmap: &mut Pixmap,
    t: Transform,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    envelope: &[EnvelopeColumn],
    gain: f32,
    color: Color,
) {
    if envelope.is_empty() {
        return;
    }
    let n = envelope.len();
    let col_w = w / n as f32;
    let mid = y + h / 2.0;
    let half = h / 2.0;
    // Never thinner than a hairline, so silence still reads as a line.
    let top = |c: &EnvelopeColumn| (mid - (c.max * gain).clamp(-1.0, 1.0) * half).min(mid - 0.6);
    let bottom = |c: &EnvelopeColumn| (mid - (c.min * gain).clamp(-1.0, 1.0) * half).max(mid + 0.6);

    let mut pb = PathBuilder::new();
    for (i, c) in envelope.iter().enumerate() {
        let px = x + (i as f32 + 0.5) * col_w;
        if i == 0 {
            pb.move_to(px, top(c));
        } else {
            pb.line_to(px, top(c));
        }
    }
    for (i, c) in envelope.iter().enumerate().rev() {
        pb.line_to(x + (i as f32 + 0.5) * col_w, bottom(c));
    }
    pb.close();
    if let Some(path) = pb.finish() {
        pixmap.fill_path(&path, &paint(color), FillRule::Winding, t, None);
    }
}

fn draw_meter(pixmap: &mut Pixmap, t: Transform, x: f32, y: f32, w: f32, h: f32, scene: &Scene) {
    let palette = scene.palette;
    if let Some(track) = rounded_rect(x, y, w, h, 2.0) {
        pixmap.fill_path(
            &track,
            &paint(palette.foreground.with_alpha(0.10)),
            FillRule::Winding,
            t,
            None,
        );
    }

    let lit = peak_meter_fraction(scene.peak_dbfs, METER_FLOOR_DBFS);
    if lit > 0.0 {
        let zone = MeterZone::from_dbfs(scene.peak_dbfs);
        if let Some(fill) = rounded_rect(x, y, (w * lit).max(h), h, 2.0) {
            pixmap.fill_path(
                &fill,
                &paint(zone.color(palette)),
                FillRule::Winding,
                t,
                None,
            );
        }
    }

    let held = peak_meter_fraction(scene.held_dbfs, METER_FLOOR_DBFS);
    if held > 0.0 {
        if let Some(tick) = Rect::from_xywh(x + w * held - 1.5, y - 1.0, 3.0, h + 2.0) {
            pixmap.fill_rect(tick, &paint(palette.foreground), t, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_a_card_with_transparent_corners_and_opaque_body() {
        let palette = Palette::fallback();
        let envelope = vec![
            EnvelopeColumn {
                min: -0.05,
                max: 0.05
            };
            waveform_columns(400.0)
        ];
        let scene = Scene {
            width: 400.0,
            height: 48.0,
            palette: &palette,
            state_color: RECORDING,
            opacity: 0.95,
            energy: 0.5,
            glow: true,
            envelope: &envelope,
            waveform_gain: 10.0,
            peak_dbfs: -20.0,
            held_dbfs: -12.0,
        };
        let pixmap = render(&scene, 1.0).unwrap();
        assert_eq!(
            (pixmap.width(), pixmap.height()),
            pixel_size(400.0, 48.0, 1.0)
        );
        // Far corner of the padding: fully transparent.
        assert_eq!(pixmap.pixel(0, 0).unwrap().alpha(), 0);
        // Centre of the card body: opaque enough to read as a card.
        let centre = pixmap
            .pixel(pixmap.width() / 2, (GLOW_PAD + 6.0) as u32)
            .unwrap();
        assert!(centre.alpha() > 200);
    }
}
