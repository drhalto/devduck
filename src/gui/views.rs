//! Drawing: header, the server cards and rows, recently stopped projects, the detail
//! drawer, dialogs and toasts. Views read state and push `Action`s; nothing here
//! mutates processes directly.

use eframe::egui::{
    self, Align2, Color32, CursorIcon, Frame, Id, Key, Margin, Modifiers, Pos2, Rect, RichText, ScrollArea, Sense,
    Stroke, StrokeKind, TextEdit, Ui, UiBuilder, WidgetInfo, WidgetType, pos2, vec2,
};

use super::icons::{self, Icon};
use super::theme::*;
use super::widgets::{self, Tone};
use super::{Action, Chip, DevPorts, Dialog, Health};
use crate::launch;
use crate::probe::Probe;
use crate::scan::{Exposure, Kind, PortApp, PortInfo};
use crate::store::{self, Editor, Recent, ViewMode};

const CARD_H: f32 = 190.0;
const CARD_MIN_W: f32 = 300.0;
const GAP: f32 = 14.0;
const ROW_H: f32 = 46.0;
const SEARCH_ID: &str = "search";

pub(crate) struct Visible<'a> {
    dev: Vec<&'a PortApp>,
    docker: Vec<&'a PortApp>,
    apps: Vec<&'a PortApp>,
    system: Vec<&'a PortApp>,
    recent: Vec<&'a Recent>,
}

impl Visible<'_> {
    fn is_empty(&self) -> bool {
        self.dev.is_empty() && self.docker.is_empty() && self.apps.is_empty() && self.system.is_empty() && self.recent.is_empty()
    }

    fn keys(&self) -> Vec<String> {
        self.dev.iter().chain(&self.docker).chain(&self.apps).chain(&self.system).map(|r| r.key.clone()).collect()
    }
}

fn health_color(h: Health, kind: Kind) -> (Color32, bool) {
    match h {
        Health::Probing => (TEXT_3, true),
        Health::Ok(..) => (GREEN, false),
        Health::ClientError(_) | Health::Silent => (AMBER, false),
        Health::ServerError(_) => (RED, false),
        Health::NotHttp | Health::Internal => (TEXT_3, false),
        Health::Docker => (CYAN, false),
        Health::Plain => match kind {
            Kind::Dev => (GREEN, false),
            Kind::App => (VIOLET, false),
            _ => (TEXT_3, false),
        },
    }
}

fn exposure_text(p: &PortInfo) -> &'static str {
    match p.exposure() {
        Exposure::All => "all interfaces (LAN)",
        Exposure::Loopback => "localhost only",
        Exposure::Specific => "a specific address",
    }
}

fn cpu_text(cpu: Option<f32>) -> String {
    match cpu {
        None => "\u{2014}".into(),
        Some(c) if c < 10.0 => format!("{c:.1}%"),
        Some(c) => format!("{c:.0}%"),
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

impl DevPorts {
    fn matches(&self, r: &PortApp, needle: &str) -> bool {
        needle.is_empty()
            || [&r.app, &r.ports_text, &r.process, &r.folder].iter().any(|f| f.to_lowercase().contains(needle))
            || r.framework.as_ref().is_some_and(|f| f.to_lowercase().contains(needle))
            || self.page_title(r).is_some_and(|t| t.to_lowercase().contains(needle))
    }

    fn visible(&self) -> Visible<'_> {
        let needle = self.search.trim().to_lowercase();
        let searching = !needle.is_empty();
        let show = |k: Kind| match self.chip {
            Chip::All => k != Kind::System || searching,
            Chip::Dev => k == Kind::Dev,
            Chip::Docker => k == Kind::Docker,
            Chip::Apps => k == Kind::App,
            Chip::System => k == Kind::System,
        };
        let mut v = Visible { dev: vec![], docker: vec![], apps: vec![], system: vec![], recent: vec![] };
        for r in self.rows.iter().filter(|r| show(r.kind) && self.matches(r, &needle)) {
            match r.kind {
                Kind::Dev => v.dev.push(r),
                Kind::Docker => v.docker.push(r),
                Kind::App => v.apps.push(r),
                Kind::System => v.system.push(r),
            }
        }
        if matches!(self.chip, Chip::All | Chip::Dev) {
            let running: Vec<&str> = self.rows.iter().filter(|r| r.kind == Kind::Dev).map(|r| r.folder.as_str()).collect();
            v.recent = self
                .state
                .recent
                .iter()
                .filter(|e| !running.iter().any(|f| f.eq_ignore_ascii_case(&e.folder)))
                .filter(|e| {
                    needle.is_empty()
                        || e.app.to_lowercase().contains(&needle)
                        || e.folder.to_lowercase().contains(&needle)
                        || e.ports.iter().any(|p| p.to_string().contains(&needle))
                })
                .take(8)
                .collect();
        }
        v
    }

    // ----------------------------------------------------------- keyboard --

    pub(crate) fn keyboard(&mut self, actions: &mut Vec<Action>) {
        let search_id = Id::new(SEARCH_ID);
        let focused = self.ctx.memory(|m| m.focused());
        let in_search = focused == Some(search_id);
        let free = focused.is_none() || in_search;
        let (find, refresh, esc, down, up, enter, del) = self.ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::COMMAND, Key::K) || i.consume_key(Modifiers::COMMAND, Key::F),
                i.consume_key(Modifiers::NONE, Key::F5) || i.consume_key(Modifiers::COMMAND, Key::R),
                i.consume_key(Modifiers::NONE, Key::Escape),
                free && i.consume_key(Modifiers::NONE, Key::ArrowDown),
                free && i.consume_key(Modifiers::NONE, Key::ArrowUp),
                free && i.consume_key(Modifiers::NONE, Key::Enter),
                !in_search && i.consume_key(Modifiers::NONE, Key::Delete),
            )
        });
        if find {
            self.focus_search = true;
        }
        if refresh {
            actions.push(Action::Refresh);
        }
        if esc {
            if in_search && !self.search.is_empty() {
                self.search.clear();
            } else if in_search {
                self.ctx.memory_mut(|m| m.surrender_focus(search_id));
            } else if self.drawer {
                actions.push(Action::CloseDrawer);
            } else {
                self.search.clear();
            }
        }
        if down || up {
            let keys = self.visible().keys();
            if !keys.is_empty() {
                let current = self.selected.as_ref().and_then(|k| keys.iter().position(|x| x == k));
                let next = match (current, down) {
                    (None, true) => 0,
                    (None, false) => keys.len() - 1,
                    (Some(i), true) => (i + 1).min(keys.len() - 1),
                    (Some(i), false) => i.saturating_sub(1),
                };
                actions.push(Action::Select(keys[next].clone()));
            }
        }
        if enter {
            let target = self.selected.clone().or_else(|| self.visible().keys().into_iter().next());
            if let Some(key) = target {
                actions.push(Action::OpenMain(key));
            }
        }
        if del && let Some(key) = self.selected.clone() {
            actions.push(Action::AskStop(key));
        }
    }

    // --------------------------------------------------------------- frame --

    pub(crate) fn draw(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        self.header(ui, actions);
        if self.drawer && self.selected.as_ref().is_some_and(|k| self.row(k).is_some()) {
            self.drawer_panel(ui, actions);
        }
        egui::CentralPanel::default()
            .frame(Frame::new().fill(BG).inner_margin(Margin { left: 22, right: 22, top: 4, bottom: 0 }))
            .show(ui, |ui| self.body(ui, actions));
        self.dialogs(actions);
        self.toasts_area();
    }

    fn header(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        egui::Panel::top("header")
            .exact_size(108.0)
            .frame(Frame::new().fill(BG).inner_margin(Margin { left: 22, right: 22, top: 16, bottom: 0 }))
            .show(ui, |ui| {
                let full = ui.max_rect();
                let p = ui.painter().clone();

                // Row 1: mark, name, summary ................ live, refresh, settings
                let r1 = Rect::from_min_size(full.min, vec2(full.width(), 34.0));
                icons::logo(&p, Rect::from_min_size(pos2(r1.left(), r1.center().y - 13.0), vec2(26.0, 26.0)));
                let title = p.text(pos2(r1.left() + 38.0, r1.center().y), Align2::LEFT_CENTER, "Dev Ports", semibold(17.0), TEXT);

                let mut x = r1.right();
                let settings = Rect::from_min_size(pos2(x - 34.0, r1.center().y - 17.0), vec2(34.0, 34.0));
                if widgets::icon_button(ui, Id::new("settings"), settings, Icon::Settings, "Settings", false, true).clicked() {
                    actions.push(Action::OpenSettings);
                }
                x = settings.left() - 8.0;
                let refresh = Rect::from_min_size(pos2(x - 34.0, r1.center().y - 17.0), vec2(34.0, 34.0));
                if widgets::icon_button(ui, Id::new("refresh"), refresh, Icon::Refresh, "Refresh now (F5)", false, true).clicked() {
                    actions.push(Action::Refresh);
                }
                x = refresh.left() - 8.0;
                let live_rect = self.live_pill(ui, &p, pos2(x, r1.center().y), actions);

                let summary = self.summary();
                widgets::text(&p, pos2(title.right() + 18.0, r1.center().y + 1.0), Align2::LEFT_CENTER, &summary, sans(13.0), TEXT_2, live_rect.left() - title.right() - 36.0);

                // Row 2: search, kind chips ........................ cards / list
                let y = r1.bottom() + 14.0;
                let search_w = (full.width() * 0.34).clamp(220.0, 380.0);
                let search = Rect::from_min_size(pos2(full.left(), y), vec2(search_w, 34.0));
                self.search_box(ui, &p, search);

                let view = self.state.settings.view;
                let toggle = Rect::from_min_size(pos2(full.right() - 74.0, y + 1.0), vec2(74.0, 32.0));
                p.rect_filled(toggle, 9.0, SURFACE);
                p.rect_stroke(toggle, 9.0, Stroke::new(1.0, BORDER), StrokeKind::Inside);
                for (i, (mode, icon, tip)) in [(ViewMode::Cards, Icon::Grid, "Cards"), (ViewMode::List, Icon::List, "List")].into_iter().enumerate() {
                    let r = Rect::from_min_size(pos2(toggle.left() + 3.0 + i as f32 * 34.0, toggle.top() + 3.0), vec2(34.0, 26.0));
                    let resp = ui.interact(r, Id::new(("view", i)), Sense::click()).on_hover_cursor(CursorIcon::PointingHand).on_hover_text(tip);
                    resp.widget_info(|| WidgetInfo::selected(WidgetType::RadioButton, true, view == mode, tip));
                    if view == mode {
                        p.rect_filled(r, 7.0, SURFACE_2);
                    } else if resp.hovered() {
                        p.rect_filled(r, 7.0, tint(SURFACE_2, 0.5));
                    }
                    icons::paint(&p, Rect::from_center_size(r.center(), vec2(14.0, 14.0)), icon, if view == mode { TEXT } else { TEXT_3 });
                    widgets::focus_ring(&p, r, 7.0, &resp);
                    if resp.clicked() {
                        actions.push(Action::SetView(mode));
                    }
                }

                let count = |k: Kind| self.rows.iter().filter(|r| r.kind == k).count();
                let all = self.rows.iter().filter(|r| r.kind != Kind::System).count();
                let chips = [
                    (Chip::All, "All", all),
                    (Chip::Dev, "Dev", count(Kind::Dev)),
                    (Chip::Docker, "Docker", count(Kind::Docker)),
                    (Chip::Apps, "Apps", count(Kind::App)),
                    (Chip::System, "System", count(Kind::System)),
                ];
                let chip_area = Rect::from_min_max(pos2(search.right() + 16.0, y + 2.0), pos2(toggle.left() - 12.0, y + 32.0));
                ui.scope_builder(UiBuilder::new().max_rect(chip_area).layout(egui::Layout::left_to_right(egui::Align::Center)), |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    for (chip, label, n) in chips {
                        if widgets::chip(ui, label, n, self.chip == chip).clicked() {
                            actions.push(Action::SetChip(chip));
                        }
                    }
                });
            });
    }

    fn summary(&self) -> String {
        if self.scanned.is_none() {
            return "Scanning\u{2026}".into();
        }
        let dev: Vec<&PortApp> = self.rows.iter().filter(|r| r.kind == Kind::Dev).collect();
        let docker = self.rows.iter().filter(|r| r.kind == Kind::Docker).count();
        let ports: usize = self.rows.iter().filter(|r| r.kind != Kind::System).map(|r| r.ports.len()).sum();
        let mut parts = Vec::new();
        if dev.is_empty() && docker == 0 {
            parts.push("No dev servers running".to_string());
        } else {
            if !dev.is_empty() {
                parts.push(plural(dev.len(), "dev server", "dev servers"));
            }
            if docker > 0 {
                parts.push(plural(docker, "container", "containers"));
            }
        }
        parts.push(plural(ports, "port", "ports"));
        let mem: u64 = dev.iter().map(|r| r.mem_bytes).sum();
        if mem > 0 {
            let cpu: f32 = dev.iter().filter_map(|r| r.cpu).sum();
            parts.push(format!("{} \u{00B7} {} CPU in dev", crate::scan::format_mem(mem), cpu_text(Some(cpu))));
        }
        parts.join("  \u{00B7}  ")
    }

    /// "● Live · 2s" / "❚❚ Paused" toggle, right-aligned at `right_center`.
    fn live_pill(&self, ui: &mut Ui, p: &egui::Painter, right_center: Pos2, actions: &mut Vec<Action>) -> Rect {
        let secs = self.state.settings.interval_secs;
        let label = if secs == 0 { "Paused".to_string() } else { format!("Live \u{00B7} {secs}s") };
        let w = widgets::text_width(p, &label, medium(12.5)) + 40.0;
        let rect = Rect::from_min_size(pos2(right_center.x - w, right_center.y - 16.0), vec2(w, 32.0));
        let tip = if secs == 0 { "Paused \u{2014} click to resume live refresh".to_string() } else { format!("Refreshing every {secs}s \u{2014} click to pause") };
        let resp = ui.interact(rect, Id::new("live"), Sense::click()).on_hover_cursor(CursorIcon::PointingHand).on_hover_text(tip);
        resp.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, true, secs != 0, "Live refresh"));
        let hov = ui.ctx().animate_bool_with_time(Id::new("live-h"), resp.hovered(), 0.12);
        p.rect_filled(rect, 16.0, mix(SURFACE, SURFACE_2, hov));
        p.rect_stroke(rect, 16.0, Stroke::new(1.0, BORDER), StrokeKind::Inside);
        let c = pos2(rect.left() + 16.0, rect.center().y);
        if secs == 0 {
            icons::paint(p, Rect::from_center_size(c, vec2(11.0, 11.0)), Icon::Pause, AMBER);
        } else {
            let t = self.scanned.map_or(1.0, |s| (s.elapsed().as_secs_f32() / 0.9).min(1.0));
            p.circle_filled(c, 4.0 + 6.0 * t, tint(GREEN, 0.35 * (1.0 - t)));
            p.circle_filled(c, 4.0, GREEN);
        }
        p.text(pos2(rect.left() + 28.0, rect.center().y), Align2::LEFT_CENTER, label, medium(12.5), if secs == 0 { AMBER } else { TEXT_2 });
        widgets::focus_ring(p, rect, 16.0, &resp);
        if resp.clicked() {
            actions.push(Action::ToggleLive);
        }
        rect
    }

    fn search_box(&mut self, ui: &mut Ui, p: &egui::Painter, rect: Rect) {
        let id = Id::new(SEARCH_ID);
        let focused = ui.memory(|m| m.has_focus(id));
        p.rect_filled(rect, 10.0, SURFACE);
        p.rect_stroke(rect, 10.0, Stroke::new(1.0, if focused { tint(ACCENT, 0.8) } else { BORDER }), StrokeKind::Inside);
        icons::paint(p, Rect::from_center_size(pos2(rect.left() + 18.0, rect.center().y), vec2(14.0, 14.0)), Icon::Search, TEXT_3);
        let inner = Rect::from_min_max(pos2(rect.left() + 36.0, rect.center().y - 9.5), pos2(rect.right() - 62.0, rect.center().y + 9.5));
        let edit = TextEdit::singleline(&mut self.search)
            .id(id)
            .frame(Frame::NONE)
            .margin(Margin::ZERO)
            .font(sans(13.5))
            .text_color(TEXT)
            .hint_text(RichText::new("Search apps, ports, folders").color(TEXT_3).font(sans(13.5)))
            .desired_width(inner.width());
        let resp = ui.put(inner, edit);
        if self.focus_search {
            resp.request_focus();
            self.focus_search = false;
        }
        if self.search.is_empty() {
            let kw = widgets::text_width(p, "Ctrl K", mono(10.5)) + 12.0;
            let k = Rect::from_min_size(pos2(rect.right() - 10.0 - kw, rect.center().y - 10.0), vec2(kw, 20.0));
            p.rect_stroke(k, 6.0, Stroke::new(1.0, BORDER_HI), StrokeKind::Inside);
            p.text(k.center(), Align2::CENTER_CENTER, "Ctrl K", mono(10.5), TEXT_3);
        } else {
            let x = Rect::from_min_size(pos2(rect.right() - 32.0, rect.center().y - 12.0), vec2(24.0, 24.0));
            if widgets::icon_button(ui, Id::new("clear-search"), x, Icon::Close, "Clear search (Esc)", false, true).clicked() {
                self.search.clear();
            }
        }
    }

    // ---------------------------------------------------------------- body --

    fn body(&self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let v = self.visible();
        let searching = !self.search.trim().is_empty();
        let cards = self.state.settings.view == ViewMode::Cards;
        ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.set_max_width(ui.available_width() - 14.0); // gutter for the floating scrollbar
            ui.spacing_mut().item_spacing.y = 0.0;
            if self.scanned.is_none() {
                self.empty(ui, "Scanning ports\u{2026}", "");
                return;
            }
            if v.is_empty() && searching {
                self.empty(ui, &format!("Nothing matches \u{201C}{}\u{201D}", self.search.trim()), "Search looks at app names, ports, folders, frameworks and page titles.");
                return;
            }

            if matches!(self.chip, Chip::All | Chip::Dev) && (!v.dev.is_empty() || !searching) {
                self.section(ui, "Dev servers", v.dev.len(), Some("double-click to open \u{00B7} Del to stop"));
                if v.dev.is_empty() {
                    let hint = if self.state.recent.is_empty() {
                        "Run npm run dev (or any dev server) and it appears here."
                    } else {
                        "Start one from Recently stopped below, or run your dev command as usual."
                    };
                    self.placeholder(ui, "No dev servers running", hint);
                } else if cards {
                    self.cards(ui, &v.dev, actions);
                } else {
                    self.rows_list(ui, &v.dev, actions);
                }
            }
            if !v.docker.is_empty() {
                self.section(ui, "Containers", v.docker.len(), None);
                if cards { self.cards(ui, &v.docker, actions) } else { self.rows_list(ui, &v.docker, actions) }
            } else if self.chip == Chip::Docker {
                self.section(ui, "Containers", 0, None);
                self.placeholder(ui, "No containers publishing ports", "Ports published by Docker Desktop containers show up here.");
            }
            if !v.recent.is_empty() {
                self.section(ui, "Recently stopped", v.recent.len(), Some("Start re-runs the same command in the same folder"));
                for e in &v.recent {
                    self.recent_row(ui, e, actions);
                }
            }
            if !v.apps.is_empty() {
                self.section(ui, "Other listeners", v.apps.len(), None);
                self.rows_list(ui, &v.apps, actions);
            }
            if !v.system.is_empty() {
                self.section(ui, "Windows", v.system.len(), Some("system services \u{2014} stopping these is rarely a good idea"));
                self.rows_list(ui, &v.system, actions);
            }
            ui.add_space(28.0);
        });
    }

    fn section(&self, ui: &mut Ui, title: &str, count: usize, hint: Option<&str>) {
        ui.add_space(18.0);
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
        let p = ui.painter();
        let r = widgets::caps(p, pos2(rect.left() + 2.0, rect.center().y), title, TEXT_3);
        if count > 0 {
            p.text(pos2(r.right() + 8.0, rect.center().y), Align2::LEFT_CENTER, count.to_string(), mono(11.0), TEXT_3);
        }
        if let Some(h) = hint {
            widgets::text(p, pos2(rect.right(), rect.center().y), Align2::RIGHT_CENTER, h, sans(12.0), tint(TEXT_3, 0.85), rect.width() * 0.5);
        }
        ui.add_space(8.0);
    }

    fn placeholder(&self, ui: &mut Ui, title: &str, hint: &str) {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 92.0), Sense::hover());
        let p = ui.painter();
        p.rect_filled(rect, 14.0, tint(SURFACE, 0.6));
        // dashed outline
        let pts = [rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom(), rect.left_top()];
        let r = rect.shrink(0.5);
        let _ = pts;
        p.add(egui::Shape::dashed_line(&[r.left_top() + vec2(14.0, 0.0), r.right_top() - vec2(14.0, 0.0)], Stroke::new(1.0, BORDER_HI), 6.0, 5.0));
        p.add(egui::Shape::dashed_line(&[r.left_bottom() + vec2(14.0, 0.0), r.right_bottom() - vec2(14.0, 0.0)], Stroke::new(1.0, BORDER_HI), 6.0, 5.0));
        p.text(rect.center() - vec2(0.0, 11.0), Align2::CENTER_CENTER, title, medium(14.0), TEXT_2);
        widgets::text(p, rect.center() + vec2(0.0, 13.0), Align2::CENTER_CENTER, hint, sans(12.5), TEXT_3, rect.width() - 40.0);
    }

    fn empty(&self, ui: &mut Ui, title: &str, hint: &str) {
        ui.add_space(80.0);
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 60.0), Sense::hover());
        let p = ui.painter();
        p.text(rect.center_top() + vec2(0.0, 12.0), Align2::CENTER_CENTER, title, medium(15.0), TEXT_2);
        widgets::text(p, rect.center_top() + vec2(0.0, 38.0), Align2::CENTER_CENTER, hint, sans(13.0), TEXT_3, rect.width() - 40.0);
    }

    // --------------------------------------------------------------- cards --

    fn cards(&self, ui: &mut Ui, rows: &[&PortApp], actions: &mut Vec<Action>) {
        let avail = ui.available_width();
        let n = (((avail + GAP) / (CARD_MIN_W + GAP)).floor() as usize).clamp(1, 6);
        let w = (avail - GAP * (n - 1) as f32) / n as f32;
        for chunk in rows.chunks(n) {
            let (line, _) = ui.allocate_exact_size(vec2(avail, CARD_H), Sense::hover());
            for (i, r) in chunk.iter().enumerate() {
                let rect = Rect::from_min_size(line.min + vec2(i as f32 * (w + GAP), 0.0), vec2(w, CARD_H));
                self.card(ui, rect, r, actions);
            }
            ui.add_space(GAP);
        }
    }

    fn card(&self, ui: &mut Ui, rect: Rect, r: &PortApp, actions: &mut Vec<Action>) {
        let id = Id::new(("card", &r.key));
        let resp = ui.interact(rect, id, Sense::click());
        let label = format!("{}, {}, port {}", r.app, r.framework.as_deref().unwrap_or(r.kind.as_str()), r.ports_text);
        resp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, &label));
        let hovered = ui.rect_contains_pointer(rect);
        let selected = self.selected.as_deref() == Some(r.key.as_str());
        let act_ids: Vec<Id> = (0..4).map(|i| id.with(("act", i))).collect();
        let child_focus = ui.memory(|m| m.focused()).is_some_and(|f| act_ids.contains(&f));
        let t = ui.ctx().animate_bool_with_time(id.with("h"), hovered, 0.15);
        let glow = self.live.get(&r.key).map_or(0.0, |l| 1.0 - (l.first_seen.elapsed().as_secs_f32() / 2.5).min(1.0));
        let p = ui.painter().clone();

        let border = if selected { ACCENT } else { mix(mix(BORDER, BORDER_HI, t), GREEN, glow * 0.85) };
        widgets::paint_card(&p, rect, mix(SURFACE, SURFACE_HI, t), border, if selected { 1.5 } else { 1.0 });
        if glow > 0.0 {
            p.rect_stroke(rect.expand(3.0), 17.0, Stroke::new(3.0, tint(GREEN, 0.18 * glow)), StrokeKind::Outside);
        }
        widgets::focus_ring(&p, rect, 14.0, &resp);

        let (x0, x1) = (rect.left() + 18.0, rect.right() - 18.0);
        let mut y = rect.top() + 18.0;

        // name + framework
        let (dot_color, hollow) = health_color(self.health(r), r.kind);
        widgets::dot(&p, pos2(x0 + 4.0, y + 10.0), dot_color, hollow);
        let mut name_max = x1 - x0 - 20.0;
        if let Some(fw) = r.framework.as_deref() {
            let size = widgets::badge_size(&p, fw);
            widgets::badge(&p, Rect::from_min_size(pos2(x1 - size.x, y), size), fw, framework_color(fw));
            name_max -= size.x + 10.0;
        }
        widgets::text(&p, pos2(x0 + 18.0, y + 10.0), Align2::LEFT_CENTER, &r.app, semibold(15.5), TEXT, name_max);
        y += 26.0;

        // folder / image (plus the root process when two rows share a name)
        let mut sub = if r.kind == Kind::Docker { r.docker_image.clone() } else { widgets::abbreviate_home(&r.folder) };
        if self.rows.iter().filter(|o| o.app == r.app).count() > 1 {
            sub = format!("{sub} \u{00B7} {}", r.process);
        }
        widgets::text(&p, pos2(x0, y + 7.0), Align2::LEFT_CENTER, &sub, mono(11.5), TEXT_3, x1 - x0);
        y += 24.0;

        // ports
        self.port_chips(ui, &p, pos2(x0, y), x1, r, actions);
        y += 46.0;

        // page title / status
        let (line, meta) = self.status_line(r);
        let meta_w = meta.as_ref().map_or(0.0, |m| widgets::text_width(&p, m, mono(11.0)) + 12.0);
        widgets::text(&p, pos2(x0, y + 8.0), Align2::LEFT_CENTER, &line, sans(13.0), TEXT_2, x1 - x0 - meta_w);
        if let Some(m) = &meta {
            p.text(pos2(x1, y + 8.0), Align2::RIGHT_CENTER, m, mono(11.0), TEXT_3);
        }

        // stats + sparkline, or actions while hovered
        let by = rect.bottom() - 18.0 - 30.0;
        let stats_right = self.stats(&p, pos2(x0, by), r);
        let show_actions = hovered || child_focus || selected;
        if show_actions && !self.busy.contains_key(&r.key) {
            self.card_actions(ui, x1, by, r, &act_ids, actions);
        } else if r.kind != Kind::Docker {
            let w = (x1 - stats_right - 14.0).min(116.0);
            if w > 44.0 {
                let values: Vec<f32> = self.live.get(&r.key).map(|l| l.mem.iter().copied().collect()).unwrap_or_default();
                let color = if r.kind == Kind::Dev { GREEN } else { VIOLET };
                widgets::sparkline(&p, Rect::from_min_max(pos2(x1 - w, by + 4.0), pos2(x1, by + 30.0)), &values, tint(color, 0.85), None);
            }
        }

        if let Some(label) = self.busy.get(&r.key) {
            p.rect_filled(rect, 14.0, tint(BG, 0.6));
            let phase = ui.input(|i| i.time) as f32;
            for i in 0..3 {
                let a = 0.35 + 0.65 * ((phase * 4.0 - i as f32 * 0.7).sin() * 0.5 + 0.5);
                p.circle_filled(rect.center() + vec2(-14.0 + i as f32 * 14.0, -10.0), 3.5, tint(TEXT, a));
            }
            p.text(rect.center() + vec2(0.0, 14.0), Align2::CENTER_CENTER, format!("{label}\u{2026}"), medium(13.0), TEXT_2);
        }

        if resp.double_clicked() {
            actions.push(Action::OpenMain(r.key.clone()));
        } else if resp.clicked() {
            actions.push(Action::Select(r.key.clone()));
        }
    }

    fn card_actions(&self, ui: &mut Ui, right: f32, top: f32, r: &PortApp, ids: &[Id], actions: &mut Vec<Action>) {
        let size = 30.0;
        let mut x = right - size;
        let restartable = self.can_restart(r);
        let buttons: [(Icon, &str, bool, bool); 4] = [
            (Icon::Stop, "Stop (Del)", true, r.kind != Kind::System && (r.kill_pid > 4 || r.kind == Kind::Docker)),
            (Icon::Restart, "Restart", false, restartable),
            (Icon::Copy, "Copy URL", false, true),
            (Icon::Open, "Open in browser (Enter)", false, true),
        ];
        for (i, (icon, tip, danger, enabled)) in buttons.into_iter().enumerate() {
            let rect = Rect::from_min_size(pos2(x, top), vec2(size, size));
            if widgets::icon_button(ui, ids[i], rect, icon, tip, danger, enabled).clicked() {
                actions.push(match icon {
                    Icon::Stop => Action::AskStop(r.key.clone()),
                    Icon::Restart => Action::Restart(r.key.clone()),
                    Icon::Copy => Action::Copy(format!("http://localhost:{}", self.main_port(r)), format!("http://localhost:{}", self.main_port(r))),
                    _ => Action::OpenMain(r.key.clone()),
                });
            }
            x -= size + 6.0;
        }
    }

    fn port_chips(&self, ui: &mut Ui, p: &egui::Painter, origin: Pos2, x1: f32, r: &PortApp, actions: &mut Vec<Action>) {
        let main = self.main_port(r);
        let mut ordered: Vec<&PortInfo> = r.ports.iter().filter(|p| p.port == main).collect();
        ordered.extend(r.ports.iter().filter(|p| p.port != main && !p.is_ephemeral()));
        let internal: Vec<&PortInfo> = r.ports.iter().filter(|p| p.port != main && p.is_ephemeral()).collect();
        let main_info = ordered[0];
        let lan = main_info.exposure() != Exposure::Loopback && r.kind != Kind::System;
        let lan_w = if lan { 50.0 } else { 0.0 };
        let mut x = origin.x;
        let mut overflow: Vec<u16> = internal.iter().map(|p| p.port).collect();
        for (i, info) in ordered.iter().enumerate() {
            let is_main = i == 0;
            let label = format!(":{}", info.port);
            let font = if is_main { mono_medium(15.0) } else { mono(12.5) };
            let h = if is_main { 32.0 } else { 26.0 };
            let w = widgets::text_width(p, &label, font.clone()) + if is_main { 24.0 } else { 16.0 };
            if !is_main && x + w > x1 - lan_w - 34.0 {
                overflow.extend(ordered[i..].iter().map(|p| p.port));
                break;
            }
            let rect = Rect::from_min_size(pos2(x, origin.y + (32.0 - h) / 2.0), vec2(w, h));
            let cid = Id::new(("port", &r.key, info.port));
            let resp = ui
                .interact(rect, cid, Sense::click())
                .on_hover_cursor(CursorIcon::PointingHand)
                .on_hover_text(format!("Open http://localhost:{} \u{00B7} {}", info.port, exposure_text(info)));
            resp.widget_info(|| WidgetInfo::labeled(WidgetType::Link, true, format!("Open port {}", info.port)));
            let ht = ui.ctx().animate_bool_with_time(cid.with("h"), resp.hovered() || resp.has_focus(), 0.12);
            p.rect_filled(rect, 8.0, mix(if is_main { BG } else { SURFACE }, SURFACE_2, ht));
            p.rect_stroke(rect, 8.0, Stroke::new(1.0, mix(BORDER_HI, ACCENT, ht)), StrokeKind::Inside);
            let fg = if is_main { TEXT } else { TEXT_2 };
            p.text(rect.center() + vec2(0.0, 0.5), Align2::CENTER_CENTER, &label, font, mix(fg, Color32::WHITE, ht * 0.5));
            widgets::focus_ring(p, rect, 8.0, &resp);
            if resp.clicked() {
                actions.push(Action::OpenPort(info.port));
            }
            x = rect.right() + 6.0;
        }
        if !overflow.is_empty() {
            overflow.sort_unstable();
            let label = format!("+{}", overflow.len());
            let rect = Rect::from_min_size(pos2(x + 2.0, origin.y + 4.0), vec2(widgets::text_width(p, &label, mono(12.0)) + 6.0, 24.0));
            let list = overflow.iter().map(|p| format!(":{p}")).collect::<Vec<_>>().join("  ");
            ui.interact(rect, Id::new(("more-ports", &r.key)), Sense::hover()).on_hover_text(format!("Also listening (internal): {list}"));
            p.text(rect.left_center(), Align2::LEFT_CENTER, label, mono(12.0), TEXT_3);
        }
        if lan {
            let c = pos2(x1, origin.y + 16.0);
            let text_rect = p.text(c, Align2::RIGHT_CENTER, "LAN", medium(11.0), TEXT_3);
            icons::paint(p, Rect::from_center_size(pos2(text_rect.left() - 9.0, c.y), vec2(12.0, 12.0)), Icon::Globe, TEXT_3);
            let hit = Rect::from_min_max(pos2(text_rect.left() - 16.0, text_rect.top() - 4.0), text_rect.right_bottom() + vec2(0.0, 4.0));
            let tip = match self.lan_ip {
                Some(ip) => format!("Reachable from your network at http://{ip}:{} (firewall permitting)", main_info.port),
                None => "Bound to all interfaces: reachable from your network".into(),
            };
            ui.interact(hit, Id::new(("lan", &r.key)), Sense::hover()).on_hover_text(tip);
        }
    }

    fn status_line(&self, r: &PortApp) -> (String, Option<String>) {
        match r.kind {
            Kind::Docker => {
                let main = r.ports.iter().find(|p| p.port == self.main_port(r)).unwrap_or(&r.ports[0]);
                (format!("Container \u{00B7} published on {}", exposure_text(main)), None)
            }
            Kind::App | Kind::System => (r.process.clone(), None),
            Kind::Dev => match self.health(r) {
                Health::Probing => ("Checking\u{2026}".into(), None),
                Health::Ok(status, ms) => (
                    self.page_title(r).map(str::to_string).unwrap_or_else(|| format!("HTTP {status}")),
                    Some(format!("{status} \u{00B7} {ms} ms")),
                ),
                Health::ClientError(status) => (
                    self.page_title(r).map(str::to_string).unwrap_or_else(|| "Responding (no page at /)".into()),
                    Some(status.to_string()),
                ),
                Health::ServerError(status) => ("Server error on /".into(), Some(status.to_string())),
                Health::Silent => ("Not answering yet \u{2014} still compiling?".into(), None),
                Health::NotHttp => ("Listening, but not an HTTP server".into(), None),
                Health::Internal => ("Only internal ports (build workers or IPC)".into(), None),
                Health::Docker | Health::Plain => (r.process.clone(), None),
            },
        }
    }

    /// value-over-label stats; returns the right edge.
    fn stats(&self, p: &egui::Painter, pos: Pos2, r: &PortApp) -> f32 {
        let items: Vec<(String, &str)> = match r.kind {
            Kind::Docker => vec![(r.uptime.clone(), "uptime")],
            _ => vec![(r.mem.clone(), "memory"), (cpu_text(r.cpu), "cpu"), (r.uptime.clone(), "uptime")],
        };
        let mut x = pos.x;
        for (value, label) in items {
            let a = p.text(pos2(x, pos.y), Align2::LEFT_TOP, &value, medium(13.5), TEXT);
            let b = p.text(pos2(x, pos.y + 19.0), Align2::LEFT_TOP, label, sans(11.0), TEXT_3);
            x = a.right().max(b.right()) + 16.0;
        }
        x - 16.0
    }

    // ---------------------------------------------------------------- rows --

    fn rows_list(&self, ui: &mut Ui, rows: &[&PortApp], actions: &mut Vec<Action>) {
        for r in rows {
            self.list_row(ui, r, actions);
            ui.add_space(4.0);
        }
    }

    fn list_row(&self, ui: &mut Ui, r: &PortApp, actions: &mut Vec<Action>) {
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
        let id = resp.id;
        resp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, format!("{}, port {}", r.app, r.ports_text)));
        let hovered = ui.rect_contains_pointer(rect);
        let selected = self.selected.as_deref() == Some(r.key.as_str());
        let act_ids: Vec<Id> = (0..4).map(|i| id.with(("act", i))).collect();
        let child_focus = ui.memory(|m| m.focused()).is_some_and(|f| act_ids.contains(&f));
        let t = ui.ctx().animate_bool_with_time(id.with("h"), hovered, 0.12);
        let glow = self.live.get(&r.key).map_or(0.0, |l| 1.0 - (l.first_seen.elapsed().as_secs_f32() / 2.5).min(1.0));
        let p = ui.painter().clone();
        let fill = if selected { tint(ACCENT, 0.10) } else { tint(SURFACE_HI, 0.35 + 0.65 * t) };
        p.rect_filled(rect, 10.0, fill);
        p.rect_stroke(rect, 10.0, Stroke::new(1.0, if selected { tint(ACCENT, 0.6) } else { mix(tint(BORDER, 0.6), GREEN, glow * 0.8) }), StrokeKind::Inside);
        widgets::focus_ring(&p, rect, 10.0, &resp);

        let cy = rect.center().y;
        let (dot_color, hollow) = health_color(self.health(r), r.kind);
        widgets::dot(&p, pos2(rect.left() + 18.0, cy), dot_color, hollow);
        let wide = rect.width() > 820.0;
        let name_w = 200.0;
        let name = widgets::text(&p, pos2(rect.left() + 34.0, cy), Align2::LEFT_CENTER, &r.app, medium(13.5), if r.kind == Kind::System { TEXT_2 } else { TEXT }, name_w - 10.0);
        let mut x = rect.left() + 34.0 + name_w;
        if let Some(fw) = r.framework.as_deref() {
            let size = widgets::badge_size(&p, fw);
            let bx = if wide { x } else { name.right() + 8.0 };
            if wide || bx + size.x < x + 90.0 {
                widgets::badge(&p, Rect::from_min_size(pos2(bx, cy - 10.0), size), fw, framework_color(fw));
            }
        }
        x += 100.0;
        let ports = r.ports.iter().map(|p| format!(":{}", p.port)).collect::<Vec<_>>().join("  ");
        widgets::text(&p, pos2(x, cy), Align2::LEFT_CENTER, &ports, mono(12.5), TEXT, 170.0);
        x += 180.0;

        let right_block = 70.0 + 80.0 + 60.0 + 24.0;
        let info_right = rect.right() - right_block;
        if info_right - x > 80.0 {
            let info = match r.kind {
                Kind::Dev => self.page_title(r).map(str::to_string).unwrap_or_else(|| widgets::abbreviate_home(&r.folder)),
                Kind::Docker => r.docker_image.clone(),
                _ => r.process.clone(),
            };
            widgets::text(&p, pos2(x, cy), Align2::LEFT_CENTER, &info, sans(12.5), TEXT_2, info_right - x - 12.0);
        }
        if (hovered || child_focus || selected) && !self.busy.contains_key(&r.key) {
            let strip = Rect::from_min_max(pos2(rect.right() - 4.0 * 36.0 - 14.0, rect.top() + 2.0), pos2(rect.right() - 2.0, rect.bottom() - 2.0));
            p.rect_filled(strip, 9.0, if selected { mix(BG, ACCENT, 0.10) } else { SURFACE_HI });
            self.card_actions(ui, rect.right() - 10.0, cy - 15.0, r, &act_ids, actions);
        } else if let Some(label) = self.busy.get(&r.key) {
            p.text(pos2(rect.right() - 16.0, cy), Align2::RIGHT_CENTER, format!("{label}\u{2026}"), medium(12.5), AMBER);
        } else {
            let mut rx = rect.right() - 16.0;
            let up = p.text(pos2(rx, cy), Align2::RIGHT_CENTER, &r.uptime, sans(12.5), TEXT_3);
            rx = up.left().min(rect.right() - 16.0 - 70.0) - 14.0;
            if r.kind != Kind::Docker {
                let mem = p.text(pos2(rx, cy), Align2::RIGHT_CENTER, &r.mem, mono(12.0), TEXT_2);
                rx = mem.left().min(rx - 70.0) - 14.0;
                if r.kind != Kind::System {
                    p.text(pos2(rx, cy), Align2::RIGHT_CENTER, cpu_text(r.cpu), mono(12.0), TEXT_3);
                }
            }
        }
        let resp = resp.on_hover_text(&r.cmd);
        if resp.double_clicked() {
            actions.push(Action::OpenMain(r.key.clone()));
        } else if resp.clicked() {
            actions.push(Action::Select(r.key.clone()));
        }
    }

    fn recent_row(&self, ui: &mut Ui, e: &Recent, actions: &mut Vec<Action>) {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
        let p = ui.painter().clone();
        let hovered = ui.rect_contains_pointer(rect);
        p.rect_filled(rect, 10.0, if hovered { tint(SURFACE_HI, 0.8) } else { tint(SURFACE, 0.55) });
        p.rect_stroke(rect, 10.0, Stroke::new(1.0, tint(BORDER, 0.7)), StrokeKind::Inside);
        let cy = rect.center().y;
        widgets::dot(&p, pos2(rect.left() + 18.0, cy), TEXT_3, true);
        let name = widgets::text(&p, pos2(rect.left() + 34.0, cy), Align2::LEFT_CENTER, &e.app, medium(13.5), TEXT_2, 190.0);
        let mut x = (rect.left() + 234.0).max(name.right() + 10.0);
        if let Some(fw) = e.framework.as_deref() {
            let size = widgets::badge_size(&p, fw);
            widgets::badge(&p, Rect::from_min_size(pos2(x, cy - 10.0), size), fw, mix(framework_color(fw), TEXT_3, 0.45));
            x += size.x.max(90.0) + 10.0;
        } else {
            x += 100.0;
        }
        let ports = e.ports.iter().map(|p| format!(":{p}")).collect::<Vec<_>>().join("  ");
        let pr = widgets::text(&p, pos2(x, cy), Align2::LEFT_CENTER, &ports, mono(12.5), TEXT_3, 170.0);
        x = (x + 180.0).max(pr.right() + 12.0);

        let busy_key = format!("start:{}", e.folder);
        let busy = self.busy.contains_key(&busy_key);
        let start_label = if busy { "Starting\u{2026}" } else { "Start" };
        let start_size = widgets::button_size(&p, Some(Icon::Play), start_label);
        let forget = Rect::from_min_size(pos2(rect.right() - 8.0 - 30.0, cy - 15.0), vec2(30.0, 30.0));
        let start = Rect::from_min_size(pos2(forget.left() - 8.0 - start_size.x, cy - 16.0), start_size);
        let info_right = start.left() - 14.0;
        if info_right - x > 60.0 {
            let info = format!("stopped {}  \u{00B7}  {}", store::ago(e.last_seen), widgets::abbreviate_home(&e.folder));
            widgets::text(&p, pos2(x, cy), Align2::LEFT_CENTER, &info, sans(12.5), TEXT_3, info_right - x);
        }
        let can_start = !e.command.is_empty() && !busy;
        let tip = if e.command.is_empty() { "Dev Ports never saw this project's command line".to_string() } else { format!("Runs: {}", e.command) };
        let resp = widgets::button_at(ui, start, Id::new(("start", &e.folder)), Some(Icon::Play), start_label, Tone::Primary, can_start).on_hover_text(tip);
        if resp.clicked() {
            actions.push(Action::Start(e.folder.clone()));
        }
        if widgets::icon_button(ui, Id::new(("forget", &e.folder)), forget, Icon::Close, "Forget this project", false, true).clicked() {
            actions.push(Action::Forget(e.folder.clone()));
        }
    }

    // -------------------------------------------------------------- drawer --

    fn drawer_panel(&self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let Some(r) = self.selected.as_ref().and_then(|k| self.row(k)) else { return };
        egui::Panel::right("drawer")
            .resizable(true)
            .default_size(400.0)
            .size_range(340.0..=620.0)
            .frame(Frame::new().fill(RAIL).inner_margin(Margin { left: 22, right: 14, top: 18, bottom: 0 }))
            .show(ui, |ui| {
                let mut scroll = ScrollArea::vertical().auto_shrink([false, false]);
                if let Some(offset) = self.shot.as_ref().and_then(|s| s.drawer_scroll) {
                    scroll = scroll.vertical_scroll_offset(offset);
                }
                scroll.show(ui, |ui| {
                    ui.set_width(ui.available_width() - 8.0);
                    self.drawer_body(ui, r, actions);
                    ui.add_space(24.0);
                });
            });
    }

    fn drawer_section(&self, ui: &mut Ui, title: &str) {
        ui.add_space(22.0);
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 16.0), Sense::hover());
        widgets::caps(ui.painter(), pos2(rect.left(), rect.center().y), title, TEXT_3);
        ui.add_space(10.0);
    }

    fn drawer_body(&self, ui: &mut Ui, r: &PortApp, actions: &mut Vec<Action>) {
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        let main = self.main_port(r);
        let main_info = r.ports.iter().find(|p| p.port == main).unwrap_or(&r.ports[0]);

        // Title
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
        let p = ui.painter().clone();
        let (dot_color, hollow) = health_color(self.health(r), r.kind);
        widgets::dot(&p, pos2(rect.left() + 5.0, rect.center().y), dot_color, hollow);
        widgets::text(&p, pos2(rect.left() + 20.0, rect.center().y), Align2::LEFT_CENTER, &r.app, semibold(19.0), TEXT, rect.width() - 60.0);
        let close = Rect::from_min_size(pos2(rect.right() - 30.0, rect.center().y - 15.0), vec2(30.0, 30.0));
        if widgets::icon_button(ui, Id::new("close-drawer"), close, Icon::Close, "Close (Esc)", false, true).clicked() {
            actions.push(Action::CloseDrawer);
        }
        let sub = if r.kind == Kind::Docker { r.docker_image.clone() } else if r.folder.is_empty() { r.process.clone() } else { widgets::abbreviate_home(&r.folder) };
        ui.add(egui::Label::new(RichText::new(sub).font(mono(12.0)).color(TEXT_3)).truncate());

        // Badges
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let mut badges: Vec<(String, Color32)> = Vec::new();
            if let Some(fw) = r.framework.as_deref() {
                badges.push((fw.to_string(), framework_color(fw)));
            }
            badges.push((r.kind.as_str().to_string(), match r.kind { Kind::Dev => GREEN, Kind::Docker => CYAN, Kind::App => VIOLET, Kind::System => TEXT_3 }));
            if main_info.exposure() != Exposure::Loopback {
                badges.push(("LAN".into(), AMBER));
            }
            if self.launched_log(r).is_some() {
                badges.push(("started here".into(), ACCENT));
            }
            for (text, color) in badges {
                let size = widgets::badge_size(ui.painter(), &text);
                let (b, _) = ui.allocate_exact_size(size, Sense::hover());
                widgets::badge(ui.painter(), b, &text, color);
            }
        });

        // Actions
        ui.add_space(10.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            let folder = if r.folder.is_empty() { None } else { Some(r.folder.clone()) };
            if widgets::button(ui, Some(Icon::Open), &format!("Open :{main}"), Tone::Normal, true).clicked() {
                actions.push(Action::OpenPort(main));
            }
            if widgets::button(ui, Some(Icon::Copy), "Copy URL", Tone::Normal, true).clicked() {
                actions.push(Action::Copy(format!("http://localhost:{main}"), format!("http://localhost:{main}")));
            }
            if let Some(f) = &folder {
                if widgets::button(ui, Some(Icon::Folder), "Folder", Tone::Normal, true).clicked() {
                    actions.push(Action::Folder(f.clone()));
                }
                let editor = self.state.settings.editor;
                let ok = launch::editor_installed(editor);
                if widgets::button(ui, Some(Icon::Code), launch::editor_name(editor), Tone::Normal, ok)
                    .on_hover_text(if ok { "Open the project folder in your editor" } else { "Not installed \u{2014} pick another editor in Settings" })
                    .clicked()
                {
                    actions.push(Action::Editor(f.clone()));
                }
                if widgets::button(ui, Some(Icon::Terminal), "Terminal", Tone::Normal, true).clicked() {
                    actions.push(Action::Terminal(f.clone()));
                }
            }
            let busy = self.busy.contains_key(&r.key);
            if self.can_restart(r) && widgets::button(ui, Some(Icon::Restart), "Restart", Tone::Normal, !busy).clicked() {
                actions.push(Action::Restart(r.key.clone()));
            }
            let stoppable = r.kind == Kind::Docker || r.kill_pid > 4;
            if stoppable && widgets::button(ui, Some(Icon::Stop), "Stop", Tone::Danger, !busy).clicked() {
                actions.push(Action::AskStop(r.key.clone()));
            }
        });

        // Status
        if r.kind == Kind::Dev {
            self.drawer_section(ui, "Status");
            let (headline, color) = match self.health(r) {
                Health::Probing => ("Checking HTTP\u{2026}".to_string(), TEXT_2),
                Health::Ok(s, ms) => (format!("Responding \u{00B7} HTTP {s} \u{00B7} {ms} ms"), GREEN),
                Health::ClientError(s) => (format!("Responding \u{00B7} HTTP {s} at /"), AMBER),
                Health::ServerError(s) => (format!("Server error \u{00B7} HTTP {s}"), RED),
                Health::Silent => ("Not answering yet \u{2014} still compiling?".to_string(), AMBER),
                Health::NotHttp => ("Listening, but not speaking HTTP".to_string(), TEXT_2),
                Health::Internal => ("Only internal ports \u{2014} no web server in this tree".to_string(), TEXT_2),
                _ => ("HTTP checks are off (Settings)".to_string(), TEXT_3),
            };
            ui.horizontal(|ui| {
                ui.label(RichText::new(headline).font(medium(13.5)).color(color));
                if self.state.settings.probe_http && !self.demo {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(egui::Button::new(RichText::new("Re-check").font(medium(12.0)).color(TEXT_2)).fill(SURFACE_2).corner_radius(7)).clicked() {
                            actions.push(Action::Reprobe(r.key.clone()));
                        }
                    });
                }
            });
            if let Some(title) = self.page_title(r) {
                ui.label(RichText::new(format!("\u{201C}{title}\u{201D}")).font(sans(13.0)).color(TEXT_2));
            }
        }

        // Ports
        self.drawer_section(ui, &plural(r.ports.len(), "Port", "Ports"));
        for info in &r.ports {
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click());
            let resp = resp.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(format!("Open http://localhost:{}", info.port));
            resp.widget_info(|| WidgetInfo::labeled(WidgetType::Link, true, format!("Open port {}", info.port)));
            let p = ui.painter();
            if resp.hovered() || resp.has_focus() {
                p.rect_filled(rect, 8.0, SURFACE_HI);
            }
            widgets::focus_ring(p, rect, 8.0, &resp);
            p.text(pos2(rect.left() + 8.0, rect.center().y), Align2::LEFT_CENTER, format!(":{}", info.port), mono_medium(13.5), if info.port == main { TEXT } else { TEXT_2 });
            let probe = self.probes.get(&r.key).and_then(|m| m.get(&info.port)).and_then(|e| e.result.as_ref());
            let (kind_text, kind_color) = match probe {
                Some(Probe::Http { status, html, .. }) => (format!("HTTP {status}{}", if *html { " \u{00B7} html" } else { "" }), if *status < 400 { GREEN } else { AMBER }),
                Some(Probe::NotHttp) => ("not HTTP".into(), TEXT_3),
                Some(Probe::Timeout) => ("no answer".into(), AMBER),
                None if r.kind == Kind::Dev && self.state.settings.probe_http && !info.is_ephemeral() => ("checking\u{2026}".into(), TEXT_3),
                None => (if info.is_ephemeral() { "internal".into() } else { String::new() }, TEXT_3),
            };
            p.text(pos2(rect.left() + 84.0, rect.center().y), Align2::LEFT_CENTER, kind_text, sans(12.5), kind_color);
            widgets::text(p, pos2(rect.right() - 30.0, rect.center().y), Align2::RIGHT_CENTER, &format!("{}  {}", info.addrs_text(), match info.exposure() { Exposure::All => "LAN", Exposure::Loopback => "local", Exposure::Specific => "" }), mono(11.0), TEXT_3, rect.width() - 220.0);
            icons::paint(p, Rect::from_center_size(pos2(rect.right() - 13.0, rect.center().y), vec2(13.0, 13.0)), Icon::Open, if resp.hovered() { TEXT } else { TEXT_3 });
            if resp.clicked() {
                actions.push(Action::OpenPort(info.port));
            }
        }

        // Phone
        if r.kind != Kind::System {
            self.drawer_section(ui, "On your phone");
            match (main_info.exposure(), self.lan_ip) {
                (Exposure::Loopback, _) => {
                    let hint = match r.framework.as_deref() {
                        Some("Vite") | Some("Astro") | Some("SvelteKit") => "Bound to localhost only. Start it with --host to reach it from your phone.",
                        Some("Next.js") => "Bound to localhost only. Start it with -H 0.0.0.0 to reach it from your phone.",
                        Some("Uvicorn") | Some("Flask") | Some("Django") | Some("Python http") => "Bound to localhost only. Bind 0.0.0.0 to reach it from your phone.",
                        _ => "Bound to localhost only, so other devices can't reach it.",
                    };
                    ui.label(RichText::new(hint).font(sans(12.5)).color(TEXT_3));
                }
                (_, Some(ip)) => {
                    let url = format!("http://{ip}:{main}");
                    ui.horizontal(|ui| {
                        let (qr, _) = ui.allocate_exact_size(vec2(132.0, 132.0), Sense::hover());
                        widgets::qr(ui.painter(), qr, &url);
                        ui.vertical(|ui| {
                            ui.add_space(6.0);
                            ui.label(RichText::new("Scan to open on a phone on the same network").font(sans(12.5)).color(TEXT_2));
                            ui.add(egui::Label::new(RichText::new(&url).font(mono(12.0)).color(TEXT)).selectable(true));
                            if widgets::button(ui, Some(Icon::Copy), "Copy LAN URL", Tone::Quiet, true).clicked() {
                                actions.push(Action::Copy(url.clone(), url.clone()));
                            }
                            ui.label(RichText::new("Windows Firewall may still block it.").font(sans(11.5)).color(TEXT_3));
                        });
                    });
                }
                (_, None) => {
                    ui.label(RichText::new("Reachable on all interfaces, but this PC has no LAN address right now.").font(sans(12.5)).color(TEXT_3));
                }
            }
        }

        // Resources
        if r.kind != Kind::Docker {
            self.drawer_section(ui, "Resources");
            let live = self.live.get(&r.key);
            let w = (ui.available_width() - 10.0) / 2.0;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                for (label, value, values, color, floor) in [
                    ("CPU", cpu_text(r.cpu), live.map(|l| l.cpu.iter().copied().collect::<Vec<_>>()).unwrap_or_default(), ACCENT, Some(5.0)),
                    ("Memory", r.mem.clone(), live.map(|l| l.mem.iter().copied().collect::<Vec<_>>()).unwrap_or_default(), GREEN, None),
                ] {
                    let (rect, _) = ui.allocate_exact_size(vec2(w, 78.0), Sense::hover());
                    let p = ui.painter();
                    p.rect_filled(rect, 10.0, SURFACE);
                    p.rect_stroke(rect, 10.0, Stroke::new(1.0, BORDER), StrokeKind::Inside);
                    p.text(rect.left_top() + vec2(12.0, 10.0), Align2::LEFT_TOP, label, sans(11.5), TEXT_3);
                    p.text(rect.right_top() + vec2(-12.0, 9.0), Align2::RIGHT_TOP, value, medium(13.0), TEXT);
                    widgets::sparkline(p, Rect::from_min_max(rect.left_top() + vec2(12.0, 34.0), rect.right_bottom() - vec2(12.0, 10.0)), &values, color, floor);
                }
            });
            if !r.tree.is_empty() {
                ui.label(RichText::new(format!("{} \u{00B7} Stop ends all of them", plural(r.tree.len(), "process in this tree", "processes in this tree"))).font(sans(12.0)).color(TEXT_3));
            }
        }

        // Process tree
        if r.tree.len() > 1 {
            self.drawer_section(ui, "Process tree");
            for t in r.tree.iter().take(40) {
                let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
                let p = ui.painter();
                let x = rect.left() + 4.0 + t.depth.min(8) as f32 * 14.0;
                if t.depth > 0 {
                    p.line_segment([pos2(x - 9.0, rect.top() - 4.0), pos2(x - 9.0, rect.center().y)], Stroke::new(1.0, BORDER_HI));
                    p.line_segment([pos2(x - 9.0, rect.center().y), pos2(x - 3.0, rect.center().y)], Stroke::new(1.0, BORDER_HI));
                }
                let n = p.text(pos2(x, rect.center().y), Align2::LEFT_CENTER, &t.name, sans(12.5), if t.depth == 0 { TEXT } else { TEXT_2 });
                p.text(pos2(n.right() + 8.0, rect.center().y), Align2::LEFT_CENTER, t.pid.to_string(), mono(11.0), TEXT_3);
                p.text(pos2(rect.right(), rect.center().y), Align2::RIGHT_CENTER, crate::scan::format_mem(t.mem), mono(11.0), TEXT_3);
            }
            if r.tree.len() > 40 {
                ui.label(RichText::new(format!("+{} more", r.tree.len() - 40)).font(sans(12.0)).color(TEXT_3));
            }
        }

        // Command
        if r.kind != Kind::Docker {
            self.drawer_section(ui, "Command");
            let cmd = if r.root_cmd.is_empty() { r.cmd.clone() } else { r.root_cmd.clone() };
            self.code_box(ui, &cmd);
            ui.horizontal(|ui| {
                if let Some(cwd) = &r.cwd {
                    ui.label(RichText::new(format!("in {}", widgets::abbreviate_home(cwd))).font(mono(11.5)).color(TEXT_3));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(egui::Button::new(RichText::new("Copy").font(medium(12.0)).color(TEXT_2)).fill(SURFACE_2).corner_radius(7)).clicked() {
                        actions.push(Action::Copy(cmd.clone(), "command line".into()));
                    }
                });
            });
        }

        // Logs
        if matches!(r.kind, Kind::Dev | Kind::Docker) {
            self.drawer_section(ui, "Logs");
            match self.logs.as_ref().filter(|l| l.key == r.key) {
                Some(log) => {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(log.source).font(sans(12.0)).color(TEXT_3));
                        if let Some(path) = &log.path {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.add(egui::Button::new(RichText::new("Open log file").font(medium(12.0)).color(TEXT_2)).fill(SURFACE_2).corner_radius(7)).clicked() {
                                    actions.push(Action::Folder(path.clone()));
                                }
                            });
                        }
                    });
                    // The newest 60 lines; "Open log file" has the rest. (No scroll area nested
                    // inside the scrolling drawer.)
                    let lines: Vec<&str> = log.text.trim_end().lines().collect();
                    let tail = lines[lines.len().saturating_sub(60)..].join("\n");
                    let text = if tail.trim().is_empty() { "(no output yet)".to_string() } else { tail };
                    self.code_box(ui, &text);
                }
                None if r.kind == Kind::Dev => {
                    ui.label(
                        RichText::new("This server was started outside Dev Ports, so its output isn't visible here. Restart it from Dev Ports to capture its logs.")
                            .font(sans(12.5))
                            .color(TEXT_3),
                    );
                }
                None => {
                    ui.label(RichText::new("Loading\u{2026}").font(sans(12.5)).color(TEXT_3));
                }
            }
        }
    }

    fn code_box(&self, ui: &mut Ui, text: &str) {
        Frame::new().fill(BG).stroke(Stroke::new(1.0, BORDER)).corner_radius(10).inner_margin(Margin::same(12)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(egui::Label::new(RichText::new(text).font(mono(11.5)).color(TEXT_2)).wrap().selectable(true));
        });
    }

    // ------------------------------------------------------------- dialogs --

    pub(crate) fn dialogs(&mut self, actions: &mut Vec<Action>) {
        let Some(dialog) = &self.dialog else { return };
        let ctx = self.ctx.clone();
        let mut close = false;
        let frame = Frame::new().fill(SURFACE_HI).stroke(Stroke::new(1.0, BORDER_HI)).corner_radius(14).inner_margin(Margin::same(24));
        let modal = egui::Modal::new(Id::new("dialog")).frame(frame).backdrop_color(Color32::from_black_alpha(150)).show(&ctx, |ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            match dialog {
                Dialog::ConfirmStop(key) => {
                    ui.set_width(420.0);
                    let Some(r) = self.rows.iter().find(|r| &r.key == key) else {
                        close = true;
                        return;
                    };
                    ui.label(RichText::new(format!("Stop {}?", r.app)).font(semibold(17.0)).color(TEXT));
                    let body = if r.kind == Kind::Docker {
                        format!("Stops the Docker container {}. Ports {} are freed.", r.docker_name, r.ports_text)
                    } else {
                        format!(
                            "Ends {} and {}. Ports {} are freed.",
                            r.process,
                            if r.tree.len() > 1 { plural(r.tree.len() - 1, "process under it", "processes under it") } else { "nothing else".into() },
                            r.ports_text
                        )
                    };
                    ui.label(RichText::new(body).font(sans(13.0)).color(TEXT_2));
                    if r.kind == Kind::System {
                        ui.label(RichText::new("This looks like a Windows system process.").font(medium(13.0)).color(AMBER));
                    }
                    ui.add_space(8.0);
                    let mut dont_ask = !self.state.settings.confirm_stop;
                    if widgets::toggle(ui, &mut dont_ask, "Don't ask again", "You can turn confirmations back on in Settings").clicked() {
                        self.state.settings.confirm_stop = !dont_ask;
                    }
                    ui.add_space(8.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if widgets::button(ui, Some(Icon::Stop), "Stop", Tone::Danger, true).clicked() {
                            actions.push(Action::Stop(key.clone()));
                            close = true;
                        }
                        if widgets::button(ui, None, "Cancel", Tone::Quiet, true).clicked() {
                            close = true;
                        }
                    });
                }
                Dialog::Error(title, body) => {
                    ui.set_width(440.0);
                    ui.label(RichText::new(title).font(semibold(17.0)).color(TEXT));
                    ui.label(RichText::new(body).font(sans(13.0)).color(TEXT_2));
                    ui.add_space(8.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if widgets::button(ui, None, "OK", Tone::Normal, true).clicked() {
                            close = true;
                        }
                    });
                }
                Dialog::Settings => {
                    ui.set_width(460.0);
                    ui.label(RichText::new("Settings").font(semibold(17.0)).color(TEXT));
                    ui.add_space(6.0);
                    let s = &mut self.state.settings;

                    ui.label(RichText::new("Live refresh").font(medium(13.0)).color(TEXT));
                    let intervals = [0u32, 1, 2, 5, 10];
                    let current = intervals.iter().position(|&i| i == s.interval_secs).unwrap_or(2);
                    if let Some(i) = widgets::segmented(ui, Id::new("interval"), &["Off", "1s", "2s", "5s", "10s"], current) {
                        s.interval_secs = intervals[i];
                        if intervals[i] > 0 {
                            self.last_interval = intervals[i];
                        }
                        let _ = self.worker.send(super::WorkerCmd::Interval(intervals[i]));
                    }
                    ui.add_space(10.0);

                    ui.label(RichText::new("Editor").font(medium(13.0)).color(TEXT));
                    let editors = [Editor::VsCode, Editor::Cursor];
                    let labels: Vec<String> = editors
                        .iter()
                        .map(|&e| if launch::editor_installed(e) { launch::editor_name(e).to_string() } else { format!("{} (not found)", launch::editor_name(e)) })
                        .collect();
                    let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
                    let current = editors.iter().position(|&e| e == s.editor).unwrap_or(0);
                    if let Some(i) = widgets::segmented(ui, Id::new("editor"), &label_refs, current) {
                        s.editor = editors[i];
                    }
                    ui.add_space(10.0);

                    widgets::toggle(ui, &mut s.probe_http, "Check dev servers over HTTP", "Requests / once per port to show status, latency and the page title");
                    widgets::toggle(ui, &mut s.confirm_stop, "Confirm before stopping", "Ask before ending a process tree or container");

                    ui.add_space(8.0);
                    let dir = store::data_dir();
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("Data and logs: {}", widgets::abbreviate_home(&dir.to_string_lossy()))).font(mono(11.5)).color(TEXT_3));
                    });
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if widgets::button(ui, Some(Icon::Folder), "Open data folder", Tone::Quiet, true).clicked() {
                            let _ = std::fs::create_dir_all(&dir);
                            actions.push(Action::Folder(dir.to_string_lossy().into_owned()));
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if widgets::button(ui, None, "Done", Tone::Normal, true).clicked() {
                                close = true;
                            }
                        });
                    });
                    ui.add_space(4.0);
                    ui.label(RichText::new(concat!("Dev Ports ", env!("CARGO_PKG_VERSION"))).font(sans(11.5)).color(TEXT_3));
                }
            }
        });
        if modal.should_close() {
            close = true;
        }
        if close {
            if matches!(self.dialog, Some(Dialog::Settings) | Some(Dialog::ConfirmStop(_))) {
                self.save_now();
            }
            self.dialog = None;
        }
    }

    pub(crate) fn toasts_area(&self) {
        if self.toasts.is_empty() {
            return;
        }
        egui::Area::new(Id::new("toasts"))
            .anchor(Align2::RIGHT_BOTTOM, vec2(-18.0, -18.0))
            .order(egui::Order::Foreground)
            .interactable(false)
            .show(&self.ctx, |ui| {
                for t in &self.toasts {
                    let age = t.at.elapsed().as_secs_f32();
                    let alpha = (age / 0.15).min(1.0) * (1.0 - ((age - 3.2) / 0.4).clamp(0.0, 1.0));
                    ui.scope(|ui| {
                        ui.multiply_opacity(alpha);
                        Frame::new()
                            .fill(SURFACE_HI)
                            .stroke(Stroke::new(1.0, BORDER_HI))
                            .corner_radius(10)
                            .inner_margin(Margin::symmetric(14, 10))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    let (d, _) = ui.allocate_exact_size(vec2(10.0, 16.0), Sense::hover());
                                    ui.painter().circle_filled(d.center(), 3.5, if t.ok { GREEN } else { AMBER });
                                    ui.label(RichText::new(&t.text).font(medium(13.0)).color(TEXT));
                                });
                            });
                    });
                    ui.add_space(6.0);
                }
            });
    }
}
