//! Palette, type and egui style. Graphite surfaces with signal colours: green = a dev
//! server answering, cyan = Docker, amber = needs a look, red = stop.

use std::sync::Arc;

use eframe::egui::{self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle, vec2};

pub const BG: Color32 = Color32::from_rgb(0x0D, 0x0F, 0x14);
pub const RAIL: Color32 = Color32::from_rgb(0x11, 0x13, 0x1A);
pub const SURFACE: Color32 = Color32::from_rgb(0x14, 0x17, 0x1F);
pub const SURFACE_HI: Color32 = Color32::from_rgb(0x1A, 0x1E, 0x28);
pub const SURFACE_2: Color32 = Color32::from_rgb(0x22, 0x27, 0x34);
pub const BORDER: Color32 = Color32::from_rgb(0x22, 0x27, 0x34);
pub const BORDER_HI: Color32 = Color32::from_rgb(0x33, 0x3A, 0x4B);
pub const TEXT: Color32 = Color32::from_rgb(0xE7, 0xEA, 0xF3);
pub const TEXT_2: Color32 = Color32::from_rgb(0xA4, 0xAC, 0xC2);
pub const TEXT_3: Color32 = Color32::from_rgb(0x7A, 0x84, 0xA0);
pub const GREEN: Color32 = Color32::from_rgb(0x3D, 0xDC, 0x84);
pub const CYAN: Color32 = Color32::from_rgb(0x4C, 0xC2, 0xFF);
pub const VIOLET: Color32 = Color32::from_rgb(0xA7, 0x8B, 0xFA);
pub const AMBER: Color32 = Color32::from_rgb(0xF5, 0xB4, 0x4A);
pub const RED: Color32 = Color32::from_rgb(0xF0, 0x61, 0x6D);
pub const RED_BG: Color32 = Color32::from_rgb(0x3A, 0x1A, 0x22);
pub const ACCENT: Color32 = Color32::from_rgb(0x7C, 0x9C, 0xFF);

pub fn rgb(c: Color32) -> [u8; 3] {
    [c.r(), c.g(), c.b()]
}

/// `c` at `alpha` (0-1) over nothing, for tinted fills.
pub fn tint(c: Color32, alpha: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (alpha * 255.0) as u8)
}

pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t.clamp(0.0, 1.0)) as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

pub fn framework_color(name: &str) -> Color32 {
    match name {
        "Next.js" | "Expo" | "Flask" | "Deno" | "React Router" => TEXT,
        "Vite" => VIOLET,
        "Remotion" => Color32::from_rgb(0x4C, 0x8D, 0xFF),
        "Astro" => Color32::from_rgb(0xFF, 0x7A, 0x45),
        "Nuxt" => Color32::from_rgb(0x00, 0xDC, 0x82),
        "SvelteKit" => Color32::from_rgb(0xFF, 0x6A, 0x33),
        "Storybook" => Color32::from_rgb(0xFF, 0x5C, 0x9A),
        "Wrangler" => Color32::from_rgb(0xF6, 0x82, 0x1F),
        "Uvicorn" | "FastAPI" => Color32::from_rgb(0x2D, 0xD4, 0xBF),
        "Django" => Color32::from_rgb(0x44, 0xB7, 0x8B),
        "Python" | "Python http" => Color32::from_rgb(0xF7, 0xCB, 0x4D),
        "Jupyter" => Color32::from_rgb(0xF3, 0x76, 0x26),
        "Supabase" => Color32::from_rgb(0x3E, 0xCF, 0x8E),
        "Webpack" => Color32::from_rgb(0x8E, 0xD6, 0xFB),
        "Node" | "Express" | "Fastify" | "Hono" | "NestJS" => Color32::from_rgb(0x7C, 0xC3, 0x6E),
        "Bun" => Color32::from_rgb(0xFB, 0xF0, 0xDF),
        ".NET" => Color32::from_rgb(0xA0, 0x7C, 0xFF),
        "Electron" => Color32::from_rgb(0x9F, 0xEA, 0xF9),
        "Angular" | "Remix" => Color32::from_rgb(0xF0, 0x46, 0x5A),
        "postgres" => Color32::from_rgb(0x6E, 0xA8, 0xFF),
        "redis" => Color32::from_rgb(0xFF, 0x6B, 0x5E),
        "mysql" | "mariadb" => Color32::from_rgb(0x4F, 0xA3, 0xD1),
        "mongo" => Color32::from_rgb(0x4D, 0xB3, 0x3D),
        _ => TEXT_2,
    }
}

pub fn family(name: &str) -> FontFamily {
    FontFamily::Name(name.into())
}
pub fn sans(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}
pub fn medium(size: f32) -> FontId {
    FontId::new(size, family("medium"))
}
pub fn semibold(size: f32) -> FontId {
    FontId::new(size, family("semibold"))
}
pub fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}
pub fn mono_medium(size: f32) -> FontId {
    FontId::new(size, family("mono-medium"))
}

/// Geist + Geist Mono (SIL OFL, bundled), with egui's defaults behind them for any
/// glyph Geist lacks.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let faces: [(&str, &'static [u8]); 5] = [
        ("geist", include_bytes!("../../assets/fonts/Geist-Regular.ttf")),
        ("geist-medium", include_bytes!("../../assets/fonts/Geist-Medium.ttf")),
        ("geist-semibold", include_bytes!("../../assets/fonts/Geist-SemiBold.ttf")),
        ("geist-mono", include_bytes!("../../assets/fonts/GeistMono-Regular.ttf")),
        ("geist-mono-medium", include_bytes!("../../assets/fonts/GeistMono-Medium.ttf")),
    ];
    for (name, bytes) in faces {
        fonts.font_data.insert(name.into(), Arc::new(FontData::from_static(bytes)));
    }
    let fallback = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let with_fallback = |first: &str| std::iter::once(first.to_string()).chain(fallback.iter().cloned()).collect::<Vec<_>>();
    fonts.families.insert(FontFamily::Proportional, with_fallback("geist"));
    fonts.families.insert(family("medium"), with_fallback("geist-medium"));
    fonts.families.insert(family("semibold"), with_fallback("geist-semibold"));
    fonts.families.insert(FontFamily::Monospace, with_fallback("geist-mono"));
    fonts.families.insert(family("mono-medium"), with_fallback("geist-mono-medium"));
    ctx.set_fonts(fonts);
}

pub fn install_style(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    ctx.all_styles_mut(|s| {
        s.text_styles.insert(TextStyle::Body, sans(13.0));
        s.text_styles.insert(TextStyle::Button, medium(13.0));
        s.text_styles.insert(TextStyle::Monospace, mono(12.0));
        s.text_styles.insert(TextStyle::Small, sans(11.0));
        s.text_styles.insert(TextStyle::Heading, semibold(17.0));
        s.spacing.button_padding = vec2(12.0, 7.0);
        s.spacing.item_spacing = vec2(8.0, 8.0);
        s.spacing.scroll.bar_width = 8.0;
        s.spacing.scroll.floating = true;
        s.interaction.tooltip_delay = 0.3;
        s.interaction.selectable_labels = false;

        let v = &mut s.visuals;
        v.dark_mode = true;
        v.panel_fill = BG;
        v.window_fill = SURFACE_HI;
        v.window_stroke = Stroke::new(1.0, BORDER_HI);
        v.window_corner_radius = CornerRadius::same(12);
        v.menu_corner_radius = CornerRadius::same(10);
        v.extreme_bg_color = SURFACE;
        v.text_edit_bg_color = Some(SURFACE);
        v.faint_bg_color = SURFACE;
        v.code_bg_color = BG;
        v.selection.bg_fill = tint(ACCENT, 0.35);
        v.selection.stroke = Stroke::new(1.5, ACCENT);
        v.hyperlink_color = ACCENT;
        v.override_text_color = None;
        let w = &mut v.widgets;
        for (state, fill) in [
            (&mut w.noninteractive, SURFACE),
            (&mut w.inactive, SURFACE_2),
            (&mut w.hovered, BORDER_HI),
            (&mut w.active, BORDER_HI),
            (&mut w.open, BORDER_HI),
        ] {
            state.bg_fill = fill;
            state.weak_bg_fill = fill;
            state.fg_stroke = Stroke::new(1.5, TEXT);
            state.bg_stroke = Stroke::NONE;
            state.corner_radius = CornerRadius::same(8);
            state.expansion = 0.0;
        }
        w.noninteractive.fg_stroke.color = TEXT_2;
        w.noninteractive.bg_stroke = Stroke::new(1.0, BORDER); // separators
        w.active.bg_stroke = Stroke::new(1.5, ACCENT); // keyboard focus
    });
}
