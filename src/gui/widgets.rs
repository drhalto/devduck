//! Small painted building blocks shared by the views.

use eframe::egui::{
    Align2, Color32, CornerRadius, CursorIcon, FontId, Id, Mesh, Painter, Pos2, Rect, Response, Sense, Shape,
    Stroke, StrokeKind, Ui, Vec2, WidgetInfo, WidgetType, pos2, vec2,
};
use eframe::epaint::text::{LayoutJob, TextFormat, TextWrapping};

use super::icons::{self, Icon};
use super::theme::*;

/// Single-line text truncated with an ellipsis to `max_width`. Returns where it landed.
pub fn text(p: &Painter, pos: Pos2, align: Align2, s: &str, font: FontId, color: Color32, max_width: f32) -> Rect {
    let mut job = LayoutJob::single_section(s.to_string(), TextFormat::simple(font, color));
    job.wrap = TextWrapping { max_width: max_width.max(8.0), max_rows: 1, break_anywhere: true, overflow_character: Some('\u{2026}') };
    let galley = p.layout_job(job);
    let rect = align.anchor_size(pos, galley.size());
    p.galley(rect.min, galley, color);
    rect
}

pub fn text_width(p: &Painter, s: &str, font: FontId) -> f32 {
    p.layout_no_wrap(s.to_string(), font, TEXT).size().x
}

/// Small-caps style section label ("DEV SERVERS").
pub fn caps(p: &Painter, pos: Pos2, s: &str, color: Color32) -> Rect {
    let fmt = TextFormat { font_id: semibold(10.5), color, extra_letter_spacing: 1.1, ..Default::default() };
    let galley = p.layout_job(LayoutJob::single_section(s.to_uppercase(), fmt));
    let rect = Align2::LEFT_CENTER.anchor_size(pos, galley.size());
    p.galley(rect.min, galley, color);
    rect
}

pub fn focus_ring(p: &Painter, rect: Rect, radius: f32, resp: &Response) {
    if resp.has_focus() {
        p.rect_stroke(rect.expand(2.0), radius + 2.0, Stroke::new(2.0, ACCENT), StrokeKind::Outside);
    }
}

fn hover_t(ui: &Ui, id: Id, on: bool) -> f32 {
    ui.ctx().animate_bool_with_time(id.with("hover-anim"), on, 0.12)
}

/// A square icon-only button at `rect`.
pub fn icon_button(ui: &mut Ui, id: Id, rect: Rect, icon: Icon, tip: &str, danger: bool, enabled: bool) -> Response {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let resp = ui.interact(rect, id, sense);
    let resp = if enabled { resp.on_hover_cursor(CursorIcon::PointingHand) } else { resp };
    let tip_owned = tip.to_string();
    resp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, &tip_owned));
    let t = hover_t(ui, id, enabled && (resp.hovered() || resp.has_focus()));
    let p = ui.painter();
    let fill = if danger { mix(SURFACE_2, RED_BG, t) } else { mix(SURFACE_2, BORDER_HI, t) };
    p.rect_filled(rect, 8.0, if enabled { fill } else { tint(SURFACE_2, 0.5) });
    let color = if !enabled {
        tint(TEXT_3, 0.5)
    } else if danger {
        mix(TEXT_2, RED, t.max(0.35))
    } else {
        mix(TEXT_2, TEXT, t)
    };
    icons::paint(p, Rect::from_center_size(rect.center(), vec2(15.0, 15.0)), icon, color);
    focus_ring(p, rect, 8.0, &resp);
    resp.on_hover_text(tip)
}

/// Favorite toggle: an amber filled star when on, an outline that brightens on hover when off.
pub fn star_button(ui: &mut Ui, id: Id, rect: Rect, on: bool) -> Response {
    let tip = if on { "Remove from favorites" } else { "Add to favorites" };
    let resp = ui.interact(rect, id, Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
    resp.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, true, on, "Favorite"));
    let t = hover_t(ui, id, resp.hovered() || resp.has_focus());
    let p = ui.painter();
    if t > 0.0 {
        p.rect_filled(rect, 7.0, tint(SURFACE_2, t));
    }
    let icon_rect = Rect::from_center_size(rect.center(), vec2(15.0, 15.0));
    if on {
        icons::paint(p, icon_rect, Icon::StarFilled, AMBER);
    } else {
        icons::paint(p, icon_rect, Icon::Star, mix(TEXT_3, AMBER, t));
    }
    focus_ring(p, rect, 7.0, &resp);
    resp.on_hover_text(tip)
}

/// Green play button for starting something.
pub fn go_button(ui: &mut Ui, id: Id, rect: Rect, tip: &str) -> Response {
    let resp = ui.interact(rect, id, Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
    let tip_owned = tip.to_string();
    resp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, &tip_owned));
    let t = hover_t(ui, id, resp.hovered() || resp.has_focus());
    let p = ui.painter();
    p.rect_filled(rect, 8.0, mix(Color32::from_rgb(0x13, 0x2B, 0x22), Color32::from_rgb(0x18, 0x40, 0x2F), t));
    p.rect_stroke(rect, 8.0, Stroke::new(1.0, tint(GREEN, 0.35 + 0.25 * t)), StrokeKind::Inside);
    icons::paint(p, Rect::from_center_size(rect.center() + vec2(0.5, 0.0), vec2(12.0, 12.0)), Icon::Play, GREEN);
    focus_ring(p, rect, 8.0, &resp);
    resp.on_hover_text(tip)
}

/// Three pulsing dots ("working on it"). `time` is the UI clock in seconds.
pub fn spinner(p: &Painter, center: Pos2, time: f32) {
    for i in 0..3 {
        let a = 0.3 + 0.7 * ((time * 4.0 - i as f32 * 0.7).sin() * 0.5 + 0.5);
        p.circle_filled(center + vec2(-7.0 + i as f32 * 7.0, 0.0), 2.6, tint(TEXT, a));
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Normal,
    Primary,
    Danger,
    Quiet,
}

pub fn button_size(p: &Painter, icon: Option<Icon>, label: &str) -> Vec2 {
    vec2(text_width(p, label, medium(13.0)) + 26.0 + if icon.is_some() { 22.0 } else { 0.0 }, 32.0)
}

/// Icon + label button sized to its text, in the normal layout flow.
pub fn button(ui: &mut Ui, icon: Option<Icon>, label: &str, tone: Tone, enabled: bool) -> Response {
    let size = button_size(ui.painter(), icon, label);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let id = ui.id().with(("button", label));
    button_at(ui, rect, id, icon, label, tone, enabled)
}

/// Icon + label button painted at an exact rect.
#[allow(clippy::too_many_arguments)]
pub fn button_at(ui: &mut Ui, rect: Rect, id: Id, icon: Option<Icon>, label: &str, tone: Tone, enabled: bool) -> Response {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let resp = ui.interact(rect, id, sense);
    let resp = if enabled { resp.on_hover_cursor(CursorIcon::PointingHand) } else { resp };
    resp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, label));
    let t = hover_t(ui, id, enabled && (resp.hovered() || resp.has_focus()));
    let (fill, stroke, fg) = match tone {
        Tone::Normal => (mix(SURFACE_2, BORDER_HI, t), Stroke::NONE, TEXT),
        Tone::Primary => (mix(Color32::from_rgb(0x13, 0x2B, 0x22), Color32::from_rgb(0x17, 0x3A, 0x2B), t), Stroke::new(1.0, tint(GREEN, 0.45)), GREEN),
        Tone::Danger => (mix(RED_BG, Color32::from_rgb(0x52, 0x22, 0x2D), t), Stroke::new(1.0, tint(RED, 0.35)), Color32::from_rgb(0xFF, 0x9A, 0xA2)),
        Tone::Quiet => (mix(SURFACE, SURFACE_2, t), Stroke::new(1.0, BORDER_HI), TEXT_2),
    };
    let p = ui.painter();
    let alpha = if enabled { 1.0 } else { 0.45 };
    p.rect_filled(rect, 8.0, fill.gamma_multiply(alpha));
    if stroke != Stroke::NONE {
        p.rect_stroke(rect, 8.0, Stroke::new(stroke.width, stroke.color.gamma_multiply(alpha)), StrokeKind::Inside);
    }
    let mut x = rect.left() + 13.0;
    if let Some(icon) = icon {
        icons::paint(p, Rect::from_center_size(pos2(x + 7.0, rect.center().y), vec2(14.0, 14.0)), icon, fg.gamma_multiply(alpha));
        x += 22.0;
    }
    p.text(pos2(x, rect.center().y), Align2::LEFT_CENTER, label, medium(13.0), fg.gamma_multiply(alpha));
    focus_ring(p, rect, 8.0, &resp);
    resp
}

/// Filter chip: "Dev  4".
pub fn chip(ui: &mut Ui, label: &str, count: usize, selected: bool) -> Response {
    let font = medium(12.5);
    let count_s = count.to_string();
    let w = text_width(ui.painter(), label, font.clone()) + text_width(ui.painter(), &count_s, mono(11.5)) + 34.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 30.0), Sense::click());
    let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
    resp.widget_info(|| WidgetInfo::selected(WidgetType::SelectableLabel, true, selected, label));
    let t = hover_t(ui, resp.id, resp.hovered() || resp.has_focus());
    let p = ui.painter();
    if selected {
        p.rect_filled(rect, 15.0, SURFACE_2);
        p.rect_stroke(rect, 15.0, Stroke::new(1.0, BORDER_HI), StrokeKind::Inside);
    } else {
        p.rect_filled(rect, 15.0, tint(SURFACE_2, 0.6 * t));
    }
    let fg = if selected { TEXT } else { mix(TEXT_2, TEXT, t) };
    let r = p.text(pos2(rect.left() + 13.0, rect.center().y), Align2::LEFT_CENTER, label, font, fg);
    p.text(pos2(r.right() + 7.0, rect.center().y + 0.5), Align2::LEFT_CENTER, count_s, mono(11.5), if selected { TEXT_2 } else { TEXT_3 });
    focus_ring(p, rect, 15.0, &resp);
    resp
}

pub fn badge_size(p: &Painter, s: &str) -> Vec2 {
    vec2(text_width(p, s, medium(11.0)) + 16.0, 20.0)
}

/// Tinted pill ("Next.js", "LAN").
pub fn badge(p: &Painter, rect: Rect, s: &str, color: Color32) {
    p.rect_filled(rect, 10.0, tint(color, 0.13));
    p.rect_stroke(rect, 10.0, Stroke::new(1.0, tint(color, 0.22)), StrokeKind::Inside);
    p.text(rect.center() + vec2(0.0, 0.5), Align2::CENTER_CENTER, s, medium(11.0), color);
}

/// Status light with a soft halo.
pub fn dot(p: &Painter, center: Pos2, color: Color32, hollow: bool) {
    if hollow {
        p.circle_stroke(center, 3.6, Stroke::new(1.4, color));
    } else {
        p.circle_filled(center, 7.0, tint(color, 0.13));
        p.circle_filled(center, 3.8, color);
    }
}

/// Area sparkline. `zero_floor` = Some(min top-of-scale) draws from zero (CPU %);
/// None scales to the data's own range so memory drift is visible.
pub fn sparkline(p: &Painter, rect: Rect, values: &[f32], color: Color32, zero_floor: Option<f32>) {
    if values.len() < 2 {
        p.line_segment([pos2(rect.left(), rect.bottom() - 0.5), pos2(rect.right(), rect.bottom() - 0.5)], Stroke::new(1.0, tint(color, 0.35)));
        return;
    }
    let hi = values.iter().copied().fold(f32::MIN, f32::max);
    let lo = values.iter().copied().fold(f32::MAX, f32::min);
    let (min, max) = match zero_floor {
        Some(floor) => (0.0, hi.max(floor)),
        None => {
            let pad = ((hi - lo) * 0.25).max(hi.abs() * 0.02).max(0.5);
            (lo - pad, hi + pad)
        }
    };
    let n = values.len();
    let pts: Vec<Pos2> = values
        .iter()
        .enumerate()
        .map(|(i, &v)| {
            let x = rect.left() + rect.width() * i as f32 / (n - 1) as f32;
            let y = rect.bottom() - (v - min) / (max - min).max(1e-3) * rect.height();
            pos2(x, y.clamp(rect.top(), rect.bottom()))
        })
        .collect();
    let mut mesh = Mesh::default();
    let top = tint(color, 0.28);
    let bottom = tint(color, 0.0);
    for (i, pt) in pts.iter().enumerate() {
        mesh.colored_vertex(*pt, top);
        mesh.colored_vertex(pos2(pt.x, rect.bottom()), bottom);
        if i > 0 {
            let b = (i as u32) * 2;
            mesh.add_triangle(b - 2, b - 1, b);
            mesh.add_triangle(b - 1, b + 1, b);
        }
    }
    p.add(Shape::mesh(mesh));
    p.add(Shape::line(pts, Stroke::new(1.5, color)));
}

/// A scannable QR code: dark modules on a light tile with a quiet zone.
pub fn qr(p: &Painter, rect: Rect, data: &str) {
    let Ok(code) = qrcodegen::QrCode::encode_text(data, qrcodegen::QrCodeEcc::Medium) else { return };
    let n = code.size();
    let quiet = 3;
    let module = rect.width() / (n + quiet * 2) as f32;
    p.rect_filled(rect, CornerRadius::same(10), Color32::from_rgb(0xF4, 0xF6, 0xFB));
    let ink = Color32::from_rgb(0x0D, 0x0F, 0x14);
    for y in 0..n {
        for x in 0..n {
            if code.get_module(x, y) {
                let min = rect.min + vec2((x + quiet) as f32 * module, (y + quiet) as f32 * module);
                p.rect_filled(Rect::from_min_size(min, vec2(module + 0.4, module + 0.4)), 0.0, ink);
            }
        }
    }
}

/// iOS-style switch with a label to its right. Returns the response (clicked = toggled).
pub fn toggle(ui: &mut Ui, on: &mut bool, label: &str, help: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
    }
    resp.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, true, *on, label));
    let t = ui.ctx().animate_bool_with_time(resp.id.with("switch"), *on, 0.15);
    let p = ui.painter();
    let track = Rect::from_min_size(pos2(rect.left(), rect.center().y - 10.0), vec2(36.0, 20.0));
    p.rect_filled(track, 10.0, mix(SURFACE_2, tint(GREEN, 0.75), t));
    let knob = pos2(track.left() + 10.0 + 16.0 * t, track.center().y);
    p.circle_filled(knob, 7.5, mix(TEXT_2, Color32::WHITE, t));
    p.text(pos2(track.right() + 12.0, rect.center().y - 8.0), Align2::LEFT_CENTER, label, medium(13.0), TEXT);
    text(p, pos2(track.right() + 12.0, rect.center().y + 9.0), Align2::LEFT_CENTER, help, sans(12.0), TEXT_3, rect.width() - 60.0);
    focus_ring(p, track, 10.0, &resp);
    resp
}

/// Segmented control. Returns the newly picked index, if any.
pub fn segmented(ui: &mut Ui, id: Id, options: &[&str], selected: usize) -> Option<usize> {
    let font = medium(12.5);
    let widths: Vec<f32> = options.iter().map(|o| text_width(ui.painter(), o, font.clone()) + 24.0).collect();
    let total: f32 = widths.iter().sum::<f32>() + 6.0;
    let (rect, _) = ui.allocate_exact_size(vec2(total, 32.0), Sense::hover());
    ui.painter().rect_filled(rect, 9.0, SURFACE);
    ui.painter().rect_stroke(rect, 9.0, Stroke::new(1.0, BORDER), StrokeKind::Inside);
    let mut x = rect.left() + 3.0;
    let mut picked = None;
    for (i, (opt, w)) in options.iter().zip(&widths).enumerate() {
        let r = Rect::from_min_size(pos2(x, rect.top() + 3.0), vec2(*w, 26.0));
        let resp = ui.interact(r, id.with(i), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
        resp.widget_info(|| WidgetInfo::selected(WidgetType::RadioButton, true, i == selected, *opt));
        if resp.clicked() && i != selected {
            picked = Some(i);
        }
        let p = ui.painter();
        if i == selected {
            p.rect_filled(r, 7.0, SURFACE_2);
        } else if resp.hovered() {
            p.rect_filled(r, 7.0, tint(SURFACE_2, 0.5));
        }
        p.text(r.center(), Align2::CENTER_CENTER, *opt, font.clone(), if i == selected { TEXT } else { TEXT_2 });
        focus_ring(p, r, 7.0, &resp);
        x += w;
    }
    picked
}

pub fn abbreviate_home(path: &str) -> String {
    match std::env::var("USERPROFILE") {
        Ok(home) if !home.is_empty() && path.len() >= home.len() && path[..home.len()].eq_ignore_ascii_case(&home) => {
            format!("~{}", &path[home.len()..])
        }
        _ => path.to_string(),
    }
}

pub fn paint_card(p: &Painter, rect: Rect, fill: Color32, border: Color32, width: f32) {
    p.rect_filled(rect, 14.0, fill);
    p.rect_stroke(rect, 14.0, Stroke::new(width, border), StrokeKind::Inside);
}
