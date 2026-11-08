//! The control surface: the Projects view (what's running as cards with their main
//! action up front, everything else as rows with Start), each item's page (Overview,
//! Output, Details), stacks in the sidebar, the command palette, and the stack and
//! port-conflict dialogs.

use std::cmp::Reverse;

use eframe::egui::{
    self, Align2, Color32, CursorIcon, Frame, Id, Key, Margin, Modifiers, Rect, RichText, ScrollArea, Sense, Stroke, StrokeKind,
    TextEdit, Ui, WidgetInfo, WidgetType, pos2, vec2,
};

use super::icons::{self, Icon};
use super::model::Item;
use super::theme::*;
use super::views::{cpu_text, health_color};
use super::widgets::{self, Tone};
use super::{Action, Chip, DevDuck, Health, RunState, StackDraft, Tab};
use crate::launch;
use crate::scan::{Exposure, Kind};
use crate::store::{self, FavoriteKind, Member, Stack};

const CARD_H: f32 = 142.0;
const CARD_MIN_W: f32 = 300.0;
const GAP: f32 = 12.0;
const ROW_H: f32 = 44.0;
const TILE_H: f32 = 96.0;
const TILE_MIN_W: f32 = 200.0;
/// The Start control's width, chevron or not, so every row's right edge lines up.
const CHEV_W: f32 = 28.0;
const PALETTE_ID: &str = "palette-input";

/// The Projects view in its three bands.
pub(crate) struct Lists<'a> {
    pub live: Vec<Item<'a>>,
    /// Pick up where you left off: crashed, then the most recently run, then starred.
    pub recent: Vec<Item<'a>>,
    pub rest: Vec<Item<'a>>,
}

impl Lists<'_> {
    pub(crate) fn ids(&self) -> Vec<String> {
        self.live.iter().chain(&self.recent).chain(&self.rest).map(|i| i.id.clone()).collect()
    }
}

fn crashed(it: &Item<'_>) -> bool {
    matches!(it.state, RunState::Exited(c) if c != Some(0))
}

/// The main action for an item: (label, icon, action, tone). No action = shown disabled.
pub(crate) struct Primary {
    pub label: String,
    pub icon: Icon,
    pub action: Option<Action>,
    pub tone: Tone,
}

impl DevDuck {
    // ------------------------------------------------------------ lists --

    fn item_matches(&self, it: &Item<'_>, needle: &str) -> bool {
        let has = |s: &str| s.to_lowercase().contains(needle);
        has(&it.name)
            || has(&it.folder)
            || it.framework.as_deref().is_some_and(has)
            || it.container.as_deref().is_some_and(has)
            || it.scripts().iter().any(|s| has(&s.name))
            || self.git_of(it).is_some_and(|g| has(&g.branch))
            || it.row.is_some_and(|r| has(&r.ports_text) || self.page_title(r).is_some_and(has))
            || it.recent.is_some_and(|e| e.ports.iter().any(|p| p.to_string().contains(needle)))
    }

    /// What the Projects view shows. The recent band only appears without a search or a
    /// stack, and holds as many tiles as fit in one line.
    pub(crate) fn project_lists(&self) -> Lists<'_> {
        let needle = self.search.trim().to_lowercase();
        let stack = match self.chip {
            Chip::Stack(i) => self.state.stacks.get(i),
            _ => None,
        };
        let mut items: Vec<Item<'_>> = self
            .items()
            .into_iter()
            .filter(|it| stack.is_none_or(|s| it.member().is_some_and(|m| s.members.iter().any(|x| x.is(m.kind, &m.target)))))
            .filter(|it| needle.is_empty() || self.item_matches(it, &needle))
            .collect();
        items.sort_by_key(|i| (Reverse(i.last_used), i.favorite.is_none(), i.name.to_lowercase()));
        let (live, idle): (Vec<_>, Vec<_>) = items.into_iter().partition(|i| i.state.is_live());
        if !needle.is_empty() || stack.is_some() {
            return Lists { live, recent: Vec::new(), rest: idle };
        }
        let slots = self.recent_slots.get();
        let mut picks: Vec<usize> = (0..idle.len()).filter(|&i| crashed(&idle[i]) || idle[i].last_used > 0 || idle[i].favorite.is_some()).collect();
        picks.sort_by_key(|&i| (!crashed(&idle[i]), idle[i].last_used == 0, Reverse(idle[i].last_used)));
        picks.truncate(slots);
        let (mut recent, mut rest) = (Vec::new(), Vec::new());
        for (i, it) in idle.into_iter().enumerate() {
            if picks.contains(&i) { recent.push((picks.iter().position(|&p| p == i), it)) } else { rest.push(it) }
        }
        recent.sort_by_key(|(pos, _)| *pos);
        Lists { live, recent: recent.into_iter().map(|(_, it)| it).collect(), rest }
    }

    pub(crate) fn primary(&self, it: &Item<'_>) -> Primary {
        let p = |label: String, icon, action: Option<Action>, tone| Primary { label, icon, action, tone };
        match (it.state, it.row) {
            (RunState::Busy(l), _) => p(format!("{l}\u{2026}"), Icon::Refresh, None, Tone::Normal),
            (RunState::Starting, _) => p("Starting\u{2026}".into(), Icon::Play, None, Tone::Primary),
            (RunState::Running, Some(r)) => match r.kind {
                Kind::Docker => p("Logs".into(), Icon::Terminal, Some(Action::Show(it.id.clone(), Tab::Output)), Tone::Normal),
                Kind::Dev => match self.health(r) {
                    Health::ServerError(_) => p("Restart".into(), Icon::Restart, Some(Action::Restart(r.key.clone())), Tone::Primary),
                    Health::Internal | Health::NotHttp | Health::Silent => p("Output".into(), Icon::Terminal, Some(Action::Show(it.id.clone(), Tab::Output)), Tone::Normal),
                    _ => p(format!("Open :{}", self.main_port(r)), Icon::Open, Some(Action::OpenMain(r.key.clone())), Tone::Primary),
                },
                _ => p(format!("Open :{}", self.main_port(r)), Icon::Open, Some(Action::OpenMain(r.key.clone())), Tone::Normal),
            },
            _ => match &it.container {
                Some(c) => p("Start".into(), Icon::Play, Some(Action::StartContainer(c.clone())), Tone::Primary),
                None if self.can_start(&it.folder) => p("Start".into(), Icon::Play, Some(Action::Run(it.folder.clone(), None)), Tone::Primary),
                None => p("Start".into(), Icon::Play, None, Tone::Primary),
            },
        }
    }

    pub(crate) fn git_of(&self, it: &Item<'_>) -> Option<&crate::git::GitInfo> {
        if it.folder.is_empty() {
            return None;
        }
        self.git.get(&it.folder.to_lowercase()).and_then(Option::as_ref)
    }

    /// The project's own icon, or its initial on a plain tile; a small dot on the corner
    /// when it's running, starting or crashed. `bg` is what's behind it, for the dot's ring.
    fn avatar(&self, ui: &Ui, rect: Rect, it: &Item<'_>, bg: Color32) {
        let p = ui.painter();
        let radius = rect.width() * 0.24;
        match self.icons.get(&it.folder.to_lowercase()).filter(|_| it.is_project()) {
            Some(tex) => {
                egui::Image::new(egui::load::SizedTexture::new(tex.id(), rect.size())).corner_radius(radius).paint_at(ui, rect);
            }
            None => {
                p.rect_filled(rect, radius, SURFACE_2);
                let letter = it.name.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().to_string()).unwrap_or_default();
                p.text(rect.center() + vec2(0.0, 0.5), Align2::CENTER_CENTER, letter, semibold(rect.height() * 0.5), TEXT_2);
            }
        }
        if it.state.is_live() || crashed(it) {
            let (dc, _) = self.item_dot(it);
            let c = rect.right_bottom() - vec2(1.0, 1.0);
            p.circle_filled(c, 5.0, bg);
            p.circle_filled(c, 3.5, dc);
        }
    }

    fn item_dot(&self, it: &Item<'_>) -> (Color32, bool) {
        match (it.state, it.row) {
            (RunState::Running | RunState::Busy(_), Some(r)) => health_color(self.health(r), r.kind),
            (RunState::Starting | RunState::Busy(_), _) => (AMBER, false),
            (RunState::Exited(Some(0)), _) => (TEXT_3, true),
            (RunState::Exited(_), _) => (RED, false),
            _ => (TEXT_3, true),
        }
    }

    fn toggle_favorite_action(&self, it: &Item<'_>) -> Option<Action> {
        if it.is_project() {
            Some(Action::ToggleFavoriteRecent(it.folder.clone()))
        } else if it.is_container() {
            match (it.favorite, it.row) {
                (Some(f), _) => Some(Action::RemoveFavorite(f.id())),
                (None, Some(r)) => Some(Action::ToggleFavorite(r.key.clone())),
                _ => None,
            }
        } else {
            None
        }
    }

    // ------------------------------------------------------------- body --

    pub(crate) fn projects_body(&self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let avail = ui.available_width() - 14.0;
        self.recent_slots.set((((avail + GAP) / (TILE_MIN_W + GAP)).floor() as usize).clamp(2, 6));
        let lists = self.project_lists();
        let searching = !self.search.trim().is_empty();
        let stack = match self.chip {
            Chip::Stack(i) => self.state.stacks.get(i).map(|s| (i, s)),
            _ => None,
        };
        let mut scroll = ScrollArea::vertical().auto_shrink([false, false]);
        if let Some(offset) = self.shot.as_ref().and_then(|s| s.main_scroll) {
            scroll = scroll.vertical_scroll_offset(offset);
        }
        scroll.show(ui, |ui| {
            ui.set_max_width(ui.available_width() - 14.0);
            ui.spacing_mut().item_spacing.y = 0.0;
            if let Some((i, s)) = stack {
                self.stack_header(ui, i, s, actions);
            }
            if self.scanned.is_none() {
                self.empty(ui, "Scanning ports\u{2026}", "");
                return;
            }
            if searching && lists.live.is_empty() && lists.rest.is_empty() {
                self.empty(ui, &format!("Nothing matches \u{201C}{}\u{201D}", self.search.trim()), "Search looks at names, folders, frameworks, scripts, ports and page titles.");
                return;
            }

            if !lists.live.is_empty() || !searching {
                self.section(ui, "Running", lists.live.len(), None);
                if lists.live.is_empty() {
                    let hint = if stack.is_some() { "Start the stack above, or its members one by one below." } else { "Start something below, or press Ctrl K and type its name." };
                    self.duck_placeholder(ui, "Nothing running", hint);
                } else {
                    self.project_cards(ui, &lists.live, actions);
                }
            }

            if !lists.recent.is_empty() {
                self.section(ui, "Pick up where you left off", 0, None);
                self.recent_tiles(ui, &lists.recent, actions);
                ui.add_space(6.0);
            }

            if !lists.rest.is_empty() || (!searching && stack.is_none()) {
                let title = if stack.is_some() { "Stopped" } else if lists.recent.is_empty() { "Projects" } else { "All projects" };
                self.section(ui, title, lists.rest.len(), None);
                if lists.rest.is_empty() {
                    if self.projects_loading {
                        self.placeholder(ui, "Looking for projects\u{2026}", "Reading package.json, deno.json and manage.py files under your project folders.");
                    } else if stack.is_none() {
                        let roots = self.state.roots().iter().map(|r| widgets::abbreviate_home(r)).collect::<Vec<_>>().join(", ");
                        self.placeholder(ui, "No other projects found", &format!("DevDuck looks under {roots} for projects with a dev script. Add your code folder in Settings."));
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            if widgets::button(ui, Some(Icon::Folder), "Add a project folder", Tone::Quiet, true).clicked() {
                                actions.push(Action::OpenSettings);
                            }
                        });
                    }
                } else {
                    self.port_note(ui, lists.recent.iter().chain(&lists.rest));
                    for it in &lists.rest {
                        self.project_row(ui, it, actions);
                    }
                }
            }
            ui.add_space(28.0);
        });
    }

    /// The mascot beside a title and a hint, for the empty Running band.
    fn duck_placeholder(&self, ui: &mut Ui, title: &str, hint: &str) {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 84.0), Sense::hover());
        let p = ui.painter();
        p.rect_filled(rect, 14.0, tint(SURFACE, 0.6));
        let duck = Rect::from_center_size(pos2(rect.left() + 46.0, rect.center().y), vec2(44.0, 44.0));
        p.image(self.logo.id(), duck, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        p.text(pos2(duck.right() + 18.0, rect.center().y - 10.0), Align2::LEFT_CENTER, title, medium(14.0), TEXT_2);
        widgets::text(p, pos2(duck.right() + 18.0, rect.center().y + 12.0), Align2::LEFT_CENTER, hint, sans(12.5), TEXT_3, rect.right() - duck.right() - 36.0);
    }

    /// One quiet line above the list instead of a warning on every row: the port most of
    /// these projects want, and who has it.
    fn port_note<'a>(&self, ui: &mut Ui, items: impl Iterator<Item = &'a Item<'a>>) {
        let mut groups: Vec<(u16, String, usize)> = Vec::new();
        for it in items.filter(|it| it.is_project()) {
            if let Some((port, holder)) = self.conflict(&it.folder) {
                match groups.iter_mut().find(|g| g.0 == port) {
                    Some(g) => g.2 += 1,
                    None => groups.push((port, holder.app.clone(), 1)),
                }
            }
        }
        groups.sort_by_key(|g| Reverse(g.2));
        let Some((port, holder, n)) = groups.first().cloned() else { return };
        let more = groups.iter().skip(1).map(|g| g.2).sum::<usize>();
        let who = if n == 1 { "1 project wants".to_string() } else { format!("{n} projects want") };
        let mut text = format!("{who} :{port}, which {holder} is using. Start will offer the next free port.");
        if more > 0 {
            text.push_str(&format!(" ({more} more want other busy ports.)"));
        }
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::hover());
        let p = ui.painter();
        p.rect_filled(rect, 8.0, tint(AMBER, 0.07));
        icons::paint(p, Rect::from_center_size(pos2(rect.left() + 18.0, rect.center().y), vec2(13.0, 13.0)), Icon::Alert, AMBER);
        widgets::text(p, pos2(rect.left() + 34.0, rect.center().y), Align2::LEFT_CENTER, &text, sans(12.5), mix(AMBER, TEXT, 0.35), rect.width() - 46.0);
        ui.add_space(6.0);
    }

    fn stack_header(&self, ui: &mut Ui, i: usize, s: &Stack, actions: &mut Vec<Action>) {
        ui.add_space(14.0);
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 64.0), Sense::hover());
        let p = ui.painter().clone();
        p.rect_filled(rect, 14.0, SURFACE);
        p.rect_stroke(rect, 14.0, Stroke::new(1.0, BORDER), StrokeKind::Inside);
        icons::paint(&p, Rect::from_center_size(pos2(rect.left() + 26.0, rect.center().y), vec2(18.0, 18.0)), Icon::Layers, ACCENT);
        let (up, total) = self.stack_status(i);
        let name = widgets::text(&p, pos2(rect.left() + 48.0, rect.center().y - 9.0), Align2::LEFT_CENTER, &s.name, semibold(17.0), TEXT, rect.width() * 0.4);
        let _ = name;
        let busy = s.members.iter().any(|m| self.busy.contains_key(&format!("fav:container:{}", m.target))) || !self.stack_queue.is_empty();
        let sub = if busy { "Starting\u{2026}".to_string() } else { format!("{up} of {total} running \u{00B7} containers start first") };
        widgets::text(&p, pos2(rect.left() + 48.0, rect.center().y + 12.0), Align2::LEFT_CENTER, &sub, sans(12.5), TEXT_3, rect.width() * 0.45);

        let mut x = rect.right() - 14.0;
        let mut add = |ui: &mut Ui, label: &str, icon: Icon, tone: Tone, enabled: bool, id: &str| -> bool {
            let size = widgets::button_size(&p, Some(icon), label);
            let r = Rect::from_min_size(pos2(x - size.x, rect.center().y - size.y / 2.0), size);
            x = r.left() - 8.0;
            widgets::button_at(ui, r, Id::new((id, i)), Some(icon), label, tone, enabled).clicked()
        };
        if add(ui, "Edit", Icon::Settings, Tone::Quiet, true, "stack-edit") {
            actions.push(Action::EditStack(i));
        }
        if up > 0 && add(ui, "Stop all", Icon::Stop, Tone::Danger, true, "stack-stop") {
            actions.push(Action::StopStack(i));
        }
        if up < total && add(ui, "Start all", Icon::Play, Tone::Primary, !busy, "stack-start") {
            actions.push(Action::StartStack(i));
        }
    }

    // ------------------------------------------------------------ cards --

    fn project_cards(&self, ui: &mut Ui, items: &[Item<'_>], actions: &mut Vec<Action>) {
        let avail = ui.available_width();
        let n = (((avail + GAP) / (CARD_MIN_W + GAP)).floor() as usize).clamp(1, 5);
        let w = (avail - GAP * (n - 1) as f32) / n as f32;
        for chunk in items.chunks(n) {
            let (line, _) = ui.allocate_exact_size(vec2(avail, CARD_H), Sense::hover());
            for (i, it) in chunk.iter().enumerate() {
                let rect = Rect::from_min_size(line.min + vec2(i as f32 * (w + GAP), 0.0), vec2(w, CARD_H));
                self.project_card(ui, rect, it, actions);
            }
            ui.add_space(GAP);
        }
    }

    /// What a live item is doing, in words: (text, color).
    pub(crate) fn plain_status(&self, it: &Item<'_>) -> (String, Color32) {
        match (it.state, it.row) {
            (RunState::Starting, _) => ("Starting\u{2026}".into(), AMBER),
            (RunState::Busy(l), _) => (format!("{l}\u{2026}"), AMBER),
            (RunState::Running, Some(r)) => match r.kind {
                Kind::Docker => (format!("Up {} \u{00B7} {}", r.uptime, r.docker_image), TEXT_2),
                Kind::Dev => {
                    let titled = |lead: &str| match self.page_title(r) {
                        Some(t) => format!("{lead} \u{00B7} {t}"),
                        None => lead.to_string(),
                    };
                    match self.health(r) {
                        Health::Probing => ("Checking\u{2026}".into(), TEXT_2),
                        Health::Ok(..) if self.redirect(r).is_some() => (format!("Ready \u{00B7} sends you to {}", self.redirect(r).unwrap_or_default()), TEXT_2),
                        Health::Ok(..) => (titled("Ready"), TEXT_2),
                        Health::ClientError(404) => (titled("Ready \u{00B7} no page at /"), TEXT_2),
                        Health::ClientError(s) => (titled(&format!("Ready \u{00B7} / answers {s}")), TEXT_2),
                        Health::ServerError(s) => (format!("Server error on / ({s})"), RED),
                        Health::Silent => ("Not answering yet \u{00B7} still compiling?".into(), AMBER),
                        Health::NotHttp => ("Listening, but not a web server".into(), TEXT_2),
                        Health::Internal => ("Only internal ports so far".into(), TEXT_2),
                        Health::Docker | Health::Plain => (format!("Running \u{00B7} {}", r.process), TEXT_2),
                    }
                }
                _ => (r.process.clone(), TEXT_2),
            },
            _ => (String::new(), TEXT_3),
        }
    }

    fn project_card(&self, ui: &mut Ui, rect: Rect, it: &Item<'_>, actions: &mut Vec<Action>) {
        let id = Id::new(("pcard", &it.id));
        let resp = ui.interact(rect, id, Sense::click());
        resp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, format!("{}, {}", it.name, state_word(it.state))));
        let hovered = ui.rect_contains_pointer(rect);
        let selected = self.drawer && self.selected.as_deref() == Some(it.id.as_str());
        let t = ui.ctx().animate_bool_with_time(id.with("h"), hovered, 0.15);
        let glow = it.row.and_then(|r| self.live.get(&r.key)).map_or(0.0, |l| 1.0 - (l.first_seen.elapsed().as_secs_f32() / 2.5).min(1.0));
        let p = ui.painter().clone();
        let starting = matches!(it.state, RunState::Starting);
        let base_border = if starting { tint(AMBER, 0.45) } else { mix(BORDER, BORDER_HI, t) };
        let border = if selected { ACCENT } else { mix(base_border, GREEN, glow * 0.85) };
        widgets::paint_card(&p, rect, mix(SURFACE, SURFACE_HI, t), border, if selected { 1.5 } else { 1.0 });
        if glow > 0.0 {
            p.rect_stroke(rect.expand(3.0), 17.0, Stroke::new(3.0, tint(GREEN, 0.18 * glow)), StrokeKind::Outside);
        }
        widgets::focus_ring(&p, rect, 14.0, &resp);

        let (x0, x1) = (rect.left() + 18.0, rect.right() - 18.0);
        let y = rect.top() + 26.0;

        // name, then framework (plain words) and the star on the right
        self.avatar(ui, Rect::from_center_size(pos2(x0 + 11.0, y), vec2(22.0, 22.0)), it, mix(SURFACE, SURFACE_HI, t));
        let mut right_edge = x1;
        if let Some(fw) = it.framework.as_deref() {
            let r = p.text(pos2(x1, y), Align2::RIGHT_CENTER, fw, sans(12.0), TEXT_3);
            right_edge = r.left() - 8.0;
        }
        if let Some(fav_action) = self.toggle_favorite_action(it) {
            let fav = it.favorite.is_some();
            let star = Rect::from_min_size(pos2(right_edge - 24.0, y - 12.0), vec2(24.0, 24.0));
            if (fav || hovered) && widgets::star_button(ui, id.with("star"), star, fav).clicked() {
                actions.push(fav_action);
            }
            right_edge -= 28.0;
        }
        widgets::text(&p, pos2(x0 + 32.0, y), Align2::LEFT_CENTER, &it.name, semibold(15.0), TEXT, right_edge - x0 - 38.0);

        // what it's doing, in words; then the last thing it printed (or where it's listening)
        let (status, color) = self.plain_status(it);
        widgets::text(&p, pos2(x0, y + 28.0), Align2::LEFT_CENTER, &status, sans(13.0), color, x1 - x0);
        let last = self.last_lines.get(&it.folder.to_lowercase()).map(|(_, l)| l.as_str()).filter(|l| !l.is_empty());
        match (last, it.row) {
            (Some(line), _) => {
                widgets::text(&p, pos2(x0, y + 50.0), Align2::LEFT_CENTER, line, mono(11.0), TEXT_3, x1 - x0);
            }
            (None, Some(r)) if r.kind != Kind::Docker => {
                let mut bits = vec![format!(":{}", self.main_port(r))];
                if !r.uptime.is_empty() {
                    bits.push(format!("up {}", r.uptime));
                }
                if let Some((_, ms)) = match self.health(r) { Health::Ok(s, ms) => Some((s, ms)), _ => None } {
                    bits.push(format!("{ms} ms"));
                }
                widgets::text(&p, pos2(x0, y + 50.0), Align2::LEFT_CENTER, &bits.join("  \u{00B7}  "), sans(12.0), TEXT_3, x1 - x0);
            }
            (None, None) if starting => {
                widgets::text(&p, pos2(x0, y + 50.0), Align2::LEFT_CENTER, "Waiting for it to listen on a port\u{2026}", sans(12.0), TEXT_3, x1 - x0);
            }
            _ => {}
        }

        // the main action, then restart / stop / output
        let by = rect.bottom() - 16.0 - 32.0;
        let pr = self.primary(it);
        let size = widgets::button_size(&p, Some(pr.icon), &pr.label);
        let prect = Rect::from_min_size(pos2(x0, by), size);
        let presp = widgets::button_at(ui, prect, id.with("primary"), Some(pr.icon), &pr.label, pr.tone, pr.action.is_some());
        if presp.clicked()
            && let Some(a) = pr.action.clone()
        {
            actions.push(a);
        }
        let mut x = prect.right() + 6.0;
        let mut icon = |ui: &mut Ui, ic: Icon, tip: &str, danger: bool, n: usize| -> bool {
            let r = Rect::from_min_size(pos2(x, by), vec2(32.0, 32.0));
            x = r.right() + 6.0;
            widgets::icon_button(ui, id.with(("act", n)), r, ic, tip, danger, true).clicked()
        };
        match (it.state, it.row) {
            (RunState::Running, Some(r)) => {
                if self.can_restart(r) && !matches!(self.health(r), Health::ServerError(_)) && icon(ui, Icon::Restart, "Restart", false, 0) {
                    actions.push(Action::Restart(r.key.clone()));
                }
                if (r.kill_pid > 4 || r.kind == Kind::Docker) && icon(ui, Icon::Stop, "Stop (Del)", true, 1) {
                    actions.push(Action::AskStop(r.key.clone()));
                }
                if r.kind == Kind::Dev && icon(ui, Icon::Terminal, "Output", false, 2) {
                    actions.push(Action::Show(it.id.clone(), Tab::Output));
                }
            }
            (RunState::Starting, _) => {
                if icon(ui, Icon::Stop, "Stop", true, 1) {
                    actions.push(Action::StopItem(it.id.clone()));
                }
                if icon(ui, Icon::Terminal, "Output", false, 2) {
                    actions.push(Action::Show(it.id.clone(), Tab::Output));
                }
            }
            _ => {}
        }

        if let RunState::Busy(label) = it.state {
            p.rect_filled(rect, 14.0, tint(BG, 0.55));
            widgets::spinner(&p, rect.center() - vec2(0.0, 10.0), ui.input(|i| i.time) as f32);
            p.text(rect.center() + vec2(0.0, 14.0), Align2::CENTER_CENTER, format!("{label}\u{2026}"), medium(13.0), TEXT_2);
        }

        if resp.double_clicked() {
            if let Some(a) = pr.action {
                actions.push(a);
            }
        } else if resp.clicked() {
            actions.push(Action::Select(it.id.clone()));
        }
        self.item_menu(&resp, it, actions);
    }

    // ------------------------------------------------------------ tiles --

    fn recent_tiles(&self, ui: &mut Ui, items: &[Item<'_>], actions: &mut Vec<Action>) {
        let avail = ui.available_width();
        let n = self.recent_slots.get().max(1);
        let w = (avail - GAP * (n - 1) as f32) / n as f32;
        let (line, _) = ui.allocate_exact_size(vec2(avail, TILE_H), Sense::hover());
        for (i, it) in items.iter().take(n).enumerate() {
            let rect = Rect::from_min_size(line.min + vec2(i as f32 * (w + GAP), 0.0), vec2(w, TILE_H));
            self.recent_tile(ui, rect, it, actions);
        }
        ui.add_space(GAP);
    }

    /// A stopped project worth starting again: name, when it last ran (or how it ended),
    /// and Start.
    fn recent_tile(&self, ui: &mut Ui, rect: Rect, it: &Item<'_>, actions: &mut Vec<Action>) {
        let id = Id::new(("tile", &it.id));
        let resp = ui.interact(rect, id, Sense::click());
        resp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, format!("{}, {}", it.name, state_word(it.state))));
        let hovered = ui.rect_contains_pointer(rect);
        let selected = self.drawer && self.selected.as_deref() == Some(it.id.as_str());
        let t = ui.ctx().animate_bool_with_time(id.with("h"), hovered, 0.12);
        let p = ui.painter().clone();
        let bad = crashed(it);
        let fill = if selected { tint(ACCENT, 0.10) } else { mix(tint(SURFACE, 0.75), SURFACE_HI, t) };
        p.rect_filled(rect, 12.0, fill);
        let border = if selected { tint(ACCENT, 0.7) } else if bad { tint(RED, 0.45) } else { tint(BORDER, 0.5 + 0.5 * t) };
        p.rect_stroke(rect, 12.0, Stroke::new(1.0, border), StrokeKind::Inside);
        widgets::focus_ring(&p, rect, 12.0, &resp);

        let (x0, x1) = (rect.left() + 14.0, rect.right() - 14.0);
        let y = rect.top() + 22.0;
        self.avatar(ui, Rect::from_center_size(pos2(x0 + 10.0, y), vec2(20.0, 20.0)), it, fill);
        let mut right = x1;
        if let Some(fav_action) = self.toggle_favorite_action(it) {
            let fav = it.favorite.is_some();
            let star = Rect::from_min_size(pos2(x1 - 22.0, y - 11.0), vec2(22.0, 22.0));
            if (fav || hovered) && widgets::star_button(ui, id.with("star"), star, fav).clicked() {
                actions.push(fav_action);
            }
            right -= 26.0;
        }
        widgets::text(&p, pos2(x0 + 28.0, y), Align2::LEFT_CENTER, &it.name, medium(14.0), TEXT, right - x0 - 30.0);

        let lc = it.folder.to_lowercase();
        let (sub, color) = match it.state {
            RunState::Exited(code) => {
                let ago = self.exits.get(&lc).map(|e| store::ago(e.at)).unwrap_or_default();
                match code {
                    Some(0) => (format!("finished {ago}"), TEXT_3),
                    Some(c) => (format!("exited with code {c} {ago}"), RED),
                    None => (format!("stopped unexpectedly {ago}"), RED),
                }
            }
            _ => {
                let mut bits = Vec::new();
                bits.push(if it.last_used > 0 { store::ago(it.last_used) } else { "starred".into() });
                if let Some(g) = self.git_of(it) {
                    bits.push(g.summary());
                } else if let Some(fw) = &it.framework {
                    bits.push(fw.clone());
                }
                (bits.join(" \u{00B7} "), TEXT_3)
            }
        };
        widgets::text(&p, pos2(x0, y + 22.0), Align2::LEFT_CENTER, &sub, sans(12.0), color, x1 - x0);

        let by = rect.bottom() - 12.0 - 28.0;
        let pr = self.primary(it);
        let tone = if hovered || selected { Tone::Primary } else { Tone::Normal };
        let size = widgets::button_size(&p, Some(Icon::Play), &pr.label);
        let brect = Rect::from_min_size(pos2(x0 - 2.0, by), vec2(size.x, 28.0));
        if widgets::button_at(ui, brect, id.with("go"), Some(Icon::Play), &pr.label, tone, pr.action.is_some()).clicked()
            && let Some(a) = pr.action.clone()
        {
            actions.push(a);
        }
        if bad {
            let orect = Rect::from_min_size(pos2(brect.right() + 6.0, by), vec2(28.0, 28.0));
            if widgets::icon_button(ui, id.with("out"), orect, Icon::Terminal, "See its output", false, true).clicked() {
                actions.push(Action::Show(it.id.clone(), Tab::Output));
            }
        }

        if resp.double_clicked() {
            if let Some(a) = pr.action {
                actions.push(a);
            }
        } else if resp.clicked() {
            actions.push(Action::Select(it.id.clone()));
        }
        self.item_menu(&resp, it, actions);
    }

    // ------------------------------------------------------------- rows --

    /// A quiet row: name, framework, where it lives. Start appears on hover, on the
    /// selected row and on the focused one.
    fn project_row(&self, ui: &mut Ui, it: &Item<'_>, actions: &mut Vec<Action>) {
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
        let id = resp.id;
        resp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, format!("{}, {}", it.name, state_word(it.state))));
        let hovered = ui.rect_contains_pointer(rect);
        let selected = self.drawer && self.selected.as_deref() == Some(it.id.as_str());
        let active = hovered || selected || resp.has_focus() || self.selected.as_deref() == Some(it.id.as_str());
        let t = ui.ctx().animate_bool_with_time(id.with("h"), hovered, 0.12);
        let p = ui.painter().clone();
        if selected {
            p.rect_filled(rect, 8.0, tint(ACCENT, 0.10));
        } else if t > 0.0 {
            p.rect_filled(rect, 8.0, tint(SURFACE_HI, t));
        }
        if !selected {
            p.line_segment([rect.left_bottom() + vec2(12.0, -0.5), rect.right_bottom() + vec2(-12.0, -0.5)], Stroke::new(1.0, tint(BORDER, 0.8 * (1.0 - t))));
        }
        widgets::focus_ring(&p, rect, 8.0, &resp);

        let cy = rect.center().y;
        self.avatar(ui, Rect::from_center_size(pos2(rect.left() + 22.0, cy), vec2(20.0, 20.0)), it, BG);
        let wide = rect.width() > 760.0;
        let name = widgets::text(&p, pos2(rect.left() + 42.0, cy), Align2::LEFT_CENTER, &it.name, medium(13.5), TEXT, if wide { 230.0 } else { 180.0 });
        let mut x = if wide { rect.left() + 280.0 } else { name.right() + 12.0 };
        if let Some(fw) = it.framework.as_deref() {
            let r = widgets::text(&p, pos2(x.max(name.right() + 12.0), cy), Align2::LEFT_CENTER, fw, sans(12.0), TEXT_3, 110.0);
            x = if wide { rect.left() + 400.0 } else { r.right() + 14.0 };
        } else if wide {
            x = rect.left() + 400.0;
        }

        // right side, reserved whether or not it's showing so nothing jumps on hover
        let start_w = widgets::button_size(&p, Some(Icon::Play), "Start").x + CHEV_W;
        let info_right = rect.right() - 10.0 - start_w - 8.0 - 30.0 - 10.0;
        if active && !matches!(it.state, RunState::Busy(_)) {
            let left = self.start_split(ui, rect.right() - 10.0, cy, it, actions);
            if let Some(fav_action) = self.toggle_favorite_action(it) {
                let star = Rect::from_min_size(pos2(left - 8.0 - 30.0, cy - 15.0), vec2(30.0, 30.0));
                if widgets::star_button(ui, id.with("star"), star, it.favorite.is_some()).clicked() {
                    actions.push(fav_action);
                }
            }
        } else if let RunState::Busy(label) = it.state {
            p.text(pos2(rect.right() - 16.0, cy), Align2::RIGHT_CENTER, format!("{label}\u{2026}"), medium(12.5), AMBER);
        } else if it.favorite.is_some()
            && let Some(fav_action) = self.toggle_favorite_action(it)
        {
            let star = Rect::from_min_size(pos2(rect.right() - 10.0 - start_w - 8.0 - 30.0, cy - 15.0), vec2(30.0, 30.0));
            if widgets::star_button(ui, id.with("star"), star, true).clicked() {
                actions.push(fav_action);
            }
        }

        // the middle: how it ended, or where it lives and when it last ran
        if info_right - x > 60.0 {
            let lc = it.folder.to_lowercase();
            let (text, color) = if let RunState::Exited(code) = it.state {
                let ago = self.exits.get(&lc).map(|e| store::ago(e.at)).unwrap_or_default();
                match code {
                    Some(0) => (format!("finished {ago}"), TEXT_3),
                    Some(c) => (format!("exited with code {c} {ago} \u{00B7} see output"), RED),
                    None => (format!("stopped unexpectedly {ago}"), RED),
                }
            } else {
                let mut parts = Vec::new();
                if let Some(g) = self.git_of(it) {
                    parts.push(g.summary());
                }
                if it.is_project() {
                    parts.push(widgets::abbreviate_home(&it.folder));
                } else if let Some(c) = &it.container {
                    parts.push(format!("container {c}"));
                }
                if it.last_used > 0 {
                    parts.push(store::ago(it.last_used));
                }
                (parts.join("  \u{00B7}  "), TEXT_3)
            };
            widgets::text(&p, pos2(x, cy), Align2::LEFT_CENTER, &text, sans(12.5), color, info_right - x);
        }

        if resp.double_clicked() {
            if let Some(a) = self.primary(it).action {
                actions.push(a);
            }
        } else if resp.clicked() {
            actions.push(Action::Select(it.id.clone()));
        }
        self.item_menu(&resp, it, actions);
    }

    /// [▶ Start | ▾] right-aligned at `right`, always the same width; returns its left edge.
    fn start_split(&self, ui: &mut Ui, right: f32, cy: f32, it: &Item<'_>, actions: &mut Vec<Action>) -> f32 {
        let p = ui.painter().clone();
        let id = Id::new(("start", &it.id));
        let pr = self.primary(it);
        let script = if it.is_project() { self.start_label(&it.folder) } else { None };
        let has_menu = it.scripts().len() > 1 || (script.is_none() && !it.scripts().is_empty());
        let main_size = widgets::button_size(&p, Some(Icon::Play), "Start");
        let total = main_size.x + CHEV_W;
        let rect = Rect::from_min_size(pos2(right - total, cy - 15.0), vec2(total, 30.0));
        let main = if has_menu { Rect::from_min_max(rect.min, pos2(rect.left() + main_size.x, rect.bottom())) } else { rect };
        let mut tip = match (&pr.action, &script) {
            (None, _) => "DevDuck doesn't know how to start this yet".to_string(),
            (_, Some(s)) => it.project.and_then(|p| p.script(s)).map_or_else(|| format!("Runs {s}"), |sc| format!("Runs {}  ({})", sc.command, sc.detail)),
            (_, None) if it.is_container() => "docker start".to_string(),
            (_, None) => self.run_spec(&it.folder, None).map(|s| format!("Runs {}", s.command)).unwrap_or_default(),
        };
        if it.is_project()
            && let Some((port, holder)) = self.conflict(&it.folder)
        {
            tip.push_str(&format!("\n:{port} is in use by {}, so DevDuck will offer another port", holder.app));
        }
        let resp = widgets::button_at(ui, main, id, Some(Icon::Play), "Start", Tone::Primary, pr.action.is_some()).on_hover_text(tip);
        if resp.clicked()
            && let Some(a) = pr.action.clone()
        {
            actions.push(a);
        }
        if has_menu {
            let chev = Rect::from_min_max(pos2(main.right(), rect.top()), rect.max);
            let cid = id.with("pick");
            let cresp = ui.interact(chev, cid, Sense::click()).on_hover_cursor(CursorIcon::PointingHand).on_hover_text("Run a different script");
            cresp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, "Pick a script to run"));
            let ht = ui.ctx().animate_bool_with_time(cid.with("h"), cresp.hovered(), 0.12);
            p.rect_filled(chev, 8.0, mix(Color32::from_rgb(0x13, 0x2B, 0x22), Color32::from_rgb(0x17, 0x3A, 0x2B), ht));
            p.rect_stroke(chev, 8.0, Stroke::new(1.0, tint(GREEN, 0.45)), StrokeKind::Inside);
            p.line_segment([pos2(chev.left(), chev.top() + 7.0), pos2(chev.left(), chev.bottom() - 7.0)], Stroke::new(1.0, tint(GREEN, 0.35)));
            icons::paint(&p, Rect::from_center_size(chev.center(), vec2(12.0, 12.0)), Icon::Chevron, GREEN);
            widgets::focus_ring(&p, chev, 8.0, &cresp);
            egui::Popup::menu(&cresp).show(|ui| {
                ui.set_min_width(260.0);
                self.script_menu(ui, it, actions);
            });
        }
        rect.left()
    }

    fn script_menu(&self, ui: &mut Ui, it: &Item<'_>, actions: &mut Vec<Action>) {
        let current = self.start_label(&it.folder);
        for s in it.scripts() {
            let mut label = RichText::new(&s.name).font(mono_medium(12.5));
            if Some(&s.name) == current.as_ref() {
                label = label.color(GREEN);
            }
            let detail = if s.detail.chars().count() > 34 { format!("{}\u{2026}", s.detail.chars().take(33).collect::<String>()) } else { s.detail.clone() };
            if ui.add(egui::Button::new(label).shortcut_text(RichText::new(detail).font(mono(11.0)).color(TEXT_3))).on_hover_text(&s.command).clicked() {
                actions.push(Action::Run(it.folder.clone(), Some(s.name.clone())));
                ui.close();
            }
        }
    }

    /// Right-click menu shared by cards and rows.
    fn item_menu(&self, resp: &egui::Response, it: &Item<'_>, actions: &mut Vec<Action>) {
        resp.context_menu(|ui| {
            ui.set_min_width(210.0);
            let pr = self.primary(it);
            if let Some(a) = pr.action.clone()
                && ui.button(&pr.label).clicked()
            {
                actions.push(a);
                ui.close();
            }
            if it.is_project() && !it.state.is_live() && it.scripts().len() > 1 {
                ui.menu_button("Run script", |ui| self.script_menu(ui, it, actions));
            }
            if let Some(r) = it.row.filter(|_| it.state == RunState::Running) {
                if self.can_restart(r) && ui.button("Restart").clicked() {
                    actions.push(Action::Restart(r.key.clone()));
                    ui.close();
                }
                if ui.button("Stop").clicked() {
                    actions.push(Action::AskStop(r.key.clone()));
                    ui.close();
                }
            }
            if (it.is_project() || it.is_container()) && ui.button(if it.is_container() { "Logs" } else { "Output" }).clicked() {
                actions.push(Action::Show(it.id.clone(), Tab::Output));
                ui.close();
            }
            if it.is_project() {
                ui.separator();
                if ui.button("Open folder").clicked() {
                    actions.push(Action::Folder(it.folder.clone()));
                    ui.close();
                }
                if ui.button(format!("Open in {}", launch::editor_name(self.state.settings.editor))).clicked() {
                    actions.push(Action::Editor(it.folder.clone()));
                    ui.close();
                }
                if ui.button("Terminal here").clicked() {
                    actions.push(Action::Terminal(it.folder.clone()));
                    ui.close();
                }
            }
            if let Some(m) = it.member() {
                ui.separator();
                ui.menu_button("Stacks", |ui| self.stack_menu(ui, &m, actions));
                if let Some(a) = self.toggle_favorite_action(it)
                    && ui.button(if it.favorite.is_some() { "Remove from favorites" } else { "Add to favorites" }).clicked()
                {
                    actions.push(a);
                    ui.close();
                }
            }
            if let Some(e) = it.recent.filter(|_| !it.state.is_live())
                && ui.button("Forget last run").on_hover_text("Clears the remembered command and output").clicked()
            {
                actions.push(Action::Forget(e.folder.clone()));
                ui.close();
            }
        });
    }

    fn stack_menu(&self, ui: &mut Ui, m: &Member, actions: &mut Vec<Action>) {
        ui.set_min_width(180.0);
        for (i, s) in self.state.stacks.iter().enumerate() {
            let mut on = s.members.iter().any(|x| x.is(m.kind, &m.target));
            if ui.checkbox(&mut on, &s.name).clicked() {
                actions.push(Action::ToggleInStack(i, m.clone()));
            }
        }
        if !self.state.stacks.is_empty() {
            ui.separator();
        }
        if ui.button("New stack\u{2026}").clicked() {
            actions.push(Action::NewStack);
            ui.close();
        }
    }

    // ----------------------------------------------------------- drawer --

    pub(crate) fn item_drawer(&self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let Some(it) = self.selected.as_ref().and_then(|id| self.item(id)) else { return };
        egui::Panel::right("drawer")
            .resizable(true)
            .default_size(440.0)
            .size_range(360.0..=760.0)
            .frame(Frame::new().fill(RAIL).inner_margin(Margin { left: 22, right: 14, top: 18, bottom: 12 }))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                ui.set_width(ui.available_width() - 8.0);
                self.drawer_head(ui, &it, actions);
                let mut tabs = Vec::new();
                if it.is_project() || it.is_container() {
                    tabs.extend([Tab::Overview, Tab::Output]);
                }
                if it.row.is_some() {
                    tabs.push(Tab::Details);
                }
                let tab = if tabs.contains(&self.tab) { self.tab } else { tabs.first().copied().unwrap_or(Tab::Details) };
                if tabs.len() > 1 {
                    self.tab_bar(ui, &tabs, tab, actions);
                }
                let scroll = |salt: &str| {
                    let mut s = ScrollArea::vertical().id_salt(salt.to_string()).auto_shrink([false, false]);
                    if let Some(offset) = self.shot.as_ref().and_then(|s| s.drawer_scroll) {
                        s = s.vertical_scroll_offset(offset);
                    }
                    s
                };
                match tab {
                    Tab::Overview => {
                        scroll("overview").show(ui, |ui| {
                            self.overview(ui, &it, actions);
                            ui.add_space(20.0);
                        });
                    }
                    Tab::Output => self.output(ui, &it, actions),
                    Tab::Details => {
                        if let Some(r) = it.row {
                            scroll("details").show(ui, |ui| {
                                ui.set_width(ui.available_width() - 8.0);
                                self.details_body(ui, r, actions);
                                ui.add_space(20.0);
                            });
                        }
                    }
                }
            });
    }

    fn drawer_head(&self, ui: &mut Ui, it: &Item<'_>, actions: &mut Vec<Action>) {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
        let p = ui.painter().clone();
        self.avatar(ui, Rect::from_center_size(pos2(rect.left() + 13.0, rect.center().y), vec2(26.0, 26.0)), it, RAIL);
        widgets::text(&p, pos2(rect.left() + 36.0, rect.center().y), Align2::LEFT_CENTER, &it.name, semibold(19.0), TEXT, rect.width() - 76.0);
        let close = Rect::from_min_size(pos2(rect.right() - 30.0, rect.center().y - 15.0), vec2(30.0, 30.0));
        if widgets::icon_button(ui, Id::new("close-drawer"), close, Icon::Close, "Close (Esc)", false, true).clicked() {
            actions.push(Action::CloseDrawer);
        }
        let sub = match (it.row, &it.container) {
            (Some(r), Some(_)) => r.docker_image.clone(),
            (_, Some(c)) => format!("container {c}"),
            _ if it.is_project() => widgets::abbreviate_home(&it.folder),
            (Some(r), None) => r.process.clone(),
            _ => String::new(),
        };
        ui.add(egui::Label::new(RichText::new(sub).font(mono(12.0)).color(TEXT_3)).truncate());

        // badges
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let mut badges: Vec<(String, Color32)> = Vec::new();
            if let Some(fw) = it.framework.as_deref() {
                badges.push((fw.to_string(), framework_color(fw)));
            }
            let (word, color) = match it.state {
                RunState::Running => ("running", GREEN),
                RunState::Starting => ("starting", AMBER),
                RunState::Busy(_) => ("working", AMBER),
                RunState::Exited(Some(0)) => ("finished", TEXT_3),
                RunState::Exited(_) => ("crashed", RED),
                RunState::Stopped => ("stopped", TEXT_3),
            };
            badges.push((word.into(), color));
            if let Some(r) = it.row
                && r.ports.iter().any(|p| p.exposure() != Exposure::Loopback)
                && r.kind != Kind::System
            {
                badges.push(("LAN".into(), AMBER));
            }
            if let Some(m) = it.member() {
                for i in self.stacks_of(&m) {
                    badges.push((self.state.stacks[i].name.clone(), ACCENT));
                }
            }
            for (text, color) in badges {
                let size = widgets::badge_size(ui.painter(), &text);
                let (b, _) = ui.allocate_exact_size(size, Sense::hover());
                widgets::badge(ui.painter(), b, &text, color);
            }
        });

        // the actions that matter
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            let pr = self.primary(it);
            let is_start = !it.state.is_live();
            let label = match (is_start, it.is_project().then(|| self.start_label(&it.folder)).flatten()) {
                (true, Some(s)) => format!("Start {s}"),
                _ => pr.label.clone(),
            };
            if widgets::button(ui, Some(pr.icon), &label, pr.tone, pr.action.is_some()).clicked()
                && let Some(a) = pr.action.clone()
            {
                actions.push(a);
            }
            if is_start && it.scripts().len() > 1 {
                let resp = widgets::button(ui, Some(Icon::Chevron), "Other script", Tone::Quiet, true);
                egui::Popup::menu(&resp).show(|ui| {
                    ui.set_min_width(260.0);
                    self.script_menu(ui, it, actions);
                });
            }
            match (it.state, it.row) {
                (RunState::Running, Some(r)) => {
                    let busy = self.busy.contains_key(&r.key);
                    let restart_is_primary = matches!(self.primary(it).action, Some(Action::Restart(_)));
                    if self.can_restart(r) && !restart_is_primary && widgets::button(ui, Some(Icon::Restart), "Restart", Tone::Normal, !busy).clicked() {
                        actions.push(Action::Restart(r.key.clone()));
                    }
                    let stoppable = r.kind != Kind::System && (r.kind == Kind::Docker || r.kill_pid > 4);
                    if stoppable && widgets::button(ui, Some(Icon::Stop), "Stop", Tone::Danger, !busy).clicked() {
                        actions.push(Action::AskStop(r.key.clone()));
                    }
                }
                (RunState::Starting, _) if widgets::button(ui, Some(Icon::Stop), "Stop", Tone::Danger, true).clicked() => {
                    actions.push(Action::StopItem(it.id.clone()));
                }
                _ => {}
            }
        });
        // jump-offs, quieter
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(2.0, 2.0);
            ui.spacing_mut().button_padding = vec2(6.0, 3.0);
            let quiet = |ui: &mut Ui, icon: Icon, label: &str, enabled: bool| {
                ui.add_enabled(enabled, egui::Button::new(RichText::new(label).font(medium(12.0)).color(TEXT_2)).fill(Color32::TRANSPARENT).corner_radius(7))
                    .on_hover_cursor(CursorIcon::PointingHand)
                    .clicked()
                    .then_some(icon)
            };
            if it.is_project() {
                if quiet(ui, Icon::Folder, "Folder", true).is_some() {
                    actions.push(Action::Folder(it.folder.clone()));
                }
                let editor = self.state.settings.editor;
                if quiet(ui, Icon::Code, launch::editor_name(editor), launch::editor_installed(editor)).is_some() {
                    actions.push(Action::Editor(it.folder.clone()));
                }
                if quiet(ui, Icon::Terminal, "Terminal", true).is_some() {
                    actions.push(Action::Terminal(it.folder.clone()));
                }
            }
            if let Some(r) = it.row.filter(|r| r.kind != Kind::System) {
                let url = format!("http://localhost:{}", self.main_port(r));
                if quiet(ui, Icon::Copy, "Copy URL", true).is_some() {
                    actions.push(Action::Copy(url.clone(), url));
                }
            }
            if let Some(a) = self.toggle_favorite_action(it)
                && quiet(ui, Icon::Star, if it.favorite.is_some() { "\u{2605} Favorite" } else { "\u{2606} Favorite" }, true).is_some()
            {
                actions.push(a);
            }
        });
    }

    fn tab_bar(&self, ui: &mut Ui, tabs: &[Tab], current: Tab, actions: &mut Vec<Action>) {
        ui.add_space(4.0);
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::hover());
        let p = ui.painter().clone();
        p.line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, BORDER));
        let mut x = rect.left();
        for &tab in tabs {
            let label = match tab {
                Tab::Overview => "Overview",
                Tab::Output => {
                    if self.selected.as_deref().is_some_and(|s| s.starts_with("c:")) {
                        "Logs"
                    } else {
                        "Output"
                    }
                }
                Tab::Details => "Details",
            };
            let w = widgets::text_width(&p, label, medium(13.0)) + 24.0;
            let r = Rect::from_min_size(pos2(x, rect.top()), vec2(w, rect.height()));
            let resp = ui.interact(r, Id::new(("tab", label)), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
            resp.widget_info(|| WidgetInfo::selected(WidgetType::SelectableLabel, true, tab == current, label));
            let on = tab == current;
            let fg = if on { TEXT } else if resp.hovered() { TEXT_2 } else { TEXT_3 };
            p.text(r.center() - vec2(0.0, 1.0), Align2::CENTER_CENTER, label, medium(13.0), fg);
            if on {
                p.line_segment([pos2(r.left() + 8.0, r.bottom() - 1.0), pos2(r.right() - 8.0, r.bottom() - 1.0)], Stroke::new(2.0, ACCENT));
            }
            widgets::focus_ring(&p, r.shrink(2.0), 6.0, &resp);
            if resp.clicked() {
                actions.push(Action::SetTab(tab));
            }
            x = r.right() + 2.0;
        }
        ui.add_space(6.0);
    }

    fn overview(&self, ui: &mut Ui, it: &Item<'_>, actions: &mut Vec<Action>) {
        let lc = it.folder.to_lowercase();
        // what's going on
        let (headline, color, sub): (String, Color32, Option<String>) = match (it.state, it.row) {
            (RunState::Running, Some(r)) if r.kind == Kind::Docker => (format!("Running \u{00B7} up {}", r.uptime), CYAN, None),
            (RunState::Running, Some(r)) => {
                let (h, c) = match self.health(r) {
                    Health::Probing => ("Running \u{00B7} checking HTTP\u{2026}".to_string(), TEXT_2),
                    Health::Ok(_, ms) if self.redirect(r).is_some() => (format!("Running \u{00B7} sends you to {} \u{00B7} {ms} ms", self.redirect(r).unwrap_or_default()), GREEN),
                    Health::Ok(s, ms) => (format!("Running \u{00B7} HTTP {s} \u{00B7} {ms} ms"), GREEN),
                    Health::ClientError(s) => (format!("Running \u{00B7} HTTP {s} at /"), AMBER),
                    Health::ServerError(s) => (format!("Server error \u{00B7} HTTP {s} \u{2014} restart it, or check Output"), RED),
                    Health::Silent => ("Running, but not answering yet \u{2014} still compiling?".to_string(), AMBER),
                    Health::NotHttp => ("Running \u{00B7} listening, not HTTP".to_string(), TEXT_2),
                    Health::Internal => ("Running \u{00B7} only internal ports".to_string(), TEXT_2),
                    _ => (format!("Running \u{00B7} up {}", r.uptime), GREEN),
                };
                (h, c, self.page_title(r).map(|t| format!("\u{201C}{t}\u{201D}")))
            }
            (RunState::Starting, _) => (
                "Starting\u{2026}".into(),
                AMBER,
                self.last_lines.get(&lc).map(|(_, l)| l.clone()).filter(|l| !l.is_empty()),
            ),
            (RunState::Busy(l), _) => (format!("{l}\u{2026}"), AMBER, None),
            (RunState::Exited(Some(0)), _) => ("Finished".into(), TEXT_2, None),
            (RunState::Exited(Some(c)), _) => (format!("Exited with code {c}"), RED, None),
            (RunState::Exited(None), _) => ("Stopped unexpectedly".into(), RED, None),
            _ => (
                "Stopped".into(),
                TEXT_2,
                it.recent.filter(|e| e.last_seen > 0).map(|e| format!("Last ran {}", store::ago(e.last_seen))),
            ),
        };
        ui.add_space(6.0);
        if crashed(it) {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(46.0, 46.0), Sense::hover());
                let duck = egui::Image::new(egui::load::SizedTexture::new(self.logo.id(), vec2(40.0, 40.0))).rotate(-0.28, vec2(0.5, 0.5));
                duck.paint_at(ui, Rect::from_center_size(r.center() + vec2(0.0, 3.0), vec2(40.0, 40.0)));
                ui.painter().text(r.right_top() + vec2(-2.0, 6.0), Align2::CENTER_CENTER, "?", semibold(17.0), AMBER);
                ui.vertical(|ui| {
                    ui.add_space(4.0);
                    ui.label(RichText::new(&headline).font(medium(14.5)).color(color));
                    ui.label(RichText::new("Something went wrong. The last lines it printed are below.").font(sans(12.5)).color(TEXT_2));
                });
            });
        } else {
            ui.label(RichText::new(&headline).font(medium(14.5)).color(color));
        }
        if let Some(sub) = sub {
            ui.add(egui::Label::new(RichText::new(sub).font(sans(12.5)).color(TEXT_2)).truncate());
        }
        if it.is_project()
            && matches!(it.state, RunState::Stopped | RunState::Exited(_))
            && let Some((port, holder)) = self.conflict(&it.folder)
        {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(14.0, 16.0), Sense::hover());
                icons::paint(ui.painter(), r, Icon::Alert, AMBER);
                ui.label(RichText::new(format!(":{port} is in use by {}. Start will offer the next free port.", holder.app)).font(sans(12.5)).color(AMBER));
            });
        }
        if let RunState::Exited(code) = it.state {
            if code != Some(0)
                && let Some(log) = self.logs.as_ref().filter(|l| Some(&l.key) == self.selected.as_ref())
            {
                let lines: Vec<&str> = log.text.trim_end().lines().collect();
                let tail = lines[lines.len().saturating_sub(10)..].join("\n");
                if !tail.trim().is_empty() {
                    self.code_box(ui, &tail);
                }
            }
            ui.horizontal(|ui| {
                if widgets::button(ui, Some(Icon::Terminal), "All output", Tone::Quiet, true).clicked() {
                    actions.push(Action::SetTab(Tab::Output));
                }
                if widgets::button(ui, None, "Dismiss", Tone::Quiet, true).clicked() {
                    actions.push(Action::DismissExit(it.folder.clone()));
                }
            });
        }

        // the numbers, in one line
        if let Some(r) = it.row {
            let mut facts = Vec::new();
            if r.kind != Kind::Docker {
                facts.push(format!("{} \u{00B7} {} CPU", r.mem, cpu_text(r.cpu)));
            }
            facts.push(format!("up {}", r.uptime));
            facts.push(format!(":{}", self.main_port(r)));
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(facts.join("  \u{00B7}  ")).font(mono(11.5)).color(TEXT_3));
                if ui.add(egui::Button::new(RichText::new("Details \u{2192}").font(medium(12.0)).color(ACCENT)).frame(false)).on_hover_cursor(CursorIcon::PointingHand).clicked() {
                    actions.push(Action::SetTab(Tab::Details));
                }
            });
        }

        // git
        if let Some(g) = self.git_of(it) {
            self.drawer_section(ui, "Git");
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(&g.branch).font(mono_medium(12.5)).color(TEXT));
                let mut bits = vec![match g.changed {
                    0 => "nothing uncommitted".to_string(),
                    1 => "1 uncommitted file".to_string(),
                    n => format!("{n} uncommitted files"),
                }];
                match (g.ahead, g.behind) {
                    (Some(a), Some(b)) if a + b > 0 => {
                        if a > 0 {
                            bits.push(format!("{a} to push"));
                        }
                        if b > 0 {
                            bits.push(format!("{b} to pull"));
                        }
                    }
                    (Some(_), _) => bits.push("in sync with its remote".into()),
                    _ => bits.push("not tracking a remote branch".into()),
                }
                ui.label(RichText::new(bits.join(" \u{00B7} ")).font(sans(12.5)).color(TEXT_2));
            });
            if !g.last_subject.is_empty() {
                ui.add(egui::Label::new(RichText::new(format!("Last commit {}: {}", store::ago(g.last_time), g.last_subject)).font(sans(12.5)).color(TEXT_3)).truncate());
            }
            if let Some(web) = &g.web {
                let host = web.trim_start_matches("https://").split('/').next().unwrap_or_default();
                let label = match host {
                    "github.com" => "Open on GitHub".to_string(),
                    "gitlab.com" => "Open on GitLab".to_string(),
                    "bitbucket.org" => "Open on Bitbucket".to_string(),
                    h => format!("Open on {h}"),
                };
                ui.horizontal(|ui| {
                    if widgets::button(ui, Some(Icon::Open), &label, Tone::Quiet, true).on_hover_text(web.as_str()).clicked() {
                        actions.push(Action::OpenWeb(web.clone()));
                    }
                });
            }
        }

        // scripts
        if !it.scripts().is_empty() {
            self.drawer_section(ui, "Scripts");
            ui.spacing_mut().item_spacing.y = 2.0;
            let current = self.start_label(&it.folder);
            let running_script = it.recent.and_then(|e| e.script.clone()).filter(|_| it.state.is_live());
            for s in it.scripts() {
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
                let p = ui.painter().clone();
                if resp.hovered() {
                    p.rect_filled(rect, 8.0, SURFACE_HI);
                }
                let name = p.text(pos2(rect.left() + 8.0, rect.center().y), Align2::LEFT_CENTER, &s.name, mono_medium(12.5), if s.serves() { TEXT } else { TEXT_2 });
                let right_w = 92.0;
                widgets::text(&p, pos2(name.right().max(rect.left() + 96.0) + 10.0, rect.center().y), Align2::LEFT_CENTER, &s.detail, mono(11.0), TEXT_3, rect.right() - right_w - name.right().max(rect.left() + 96.0) - 14.0);
                let btn = Rect::from_min_size(pos2(rect.right() - 30.0, rect.center().y - 14.0), vec2(28.0, 28.0));
                if running_script.as_deref() == Some(s.name.as_str()) {
                    let size = widgets::badge_size(&p, "running");
                    widgets::badge(&p, Rect::from_min_size(pos2(rect.right() - 6.0 - size.x, rect.center().y - 10.0), size), "running", GREEN);
                } else if !it.state.is_live() {
                    if current.as_deref() == Some(s.name.as_str()) {
                        p.text(pos2(btn.left() - 8.0, rect.center().y), Align2::RIGHT_CENTER, "Start", sans(11.0), TEXT_3);
                    }
                    if widgets::go_button(ui, Id::new(("run-script", &it.id, &s.name)), btn, &format!("Run {}", s.command)).clicked() {
                        actions.push(Action::Run(it.folder.clone(), Some(s.name.clone())));
                    }
                }
            }
            ui.spacing_mut().item_spacing.y = 8.0;
            if it.state.is_live() {
                ui.label(RichText::new("Stop it to run a different script.").font(sans(11.5)).color(TEXT_3));
            }
        } else if let Some(e) = it.recent.filter(|e| !e.command.is_empty()) {
            self.drawer_section(ui, "Start runs");
            self.code_box(ui, &e.command);
        }

        // stacks
        if let Some(m) = it.member() {
            self.drawer_section(ui, "Stacks");
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                for i in self.stacks_of(&m) {
                    let name = &self.state.stacks[i].name;
                    if widgets::button(ui, Some(Icon::Layers), name, Tone::Quiet, true).on_hover_text("Show this stack").clicked() {
                        actions.push(Action::SetChip(Chip::Stack(i)));
                    }
                }
                let resp = widgets::button(ui, Some(Icon::Plus), if self.state.stacks.is_empty() { "New stack" } else { "Add to stack" }, Tone::Quiet, true);
                if self.state.stacks.is_empty() {
                    if resp.clicked() {
                        actions.push(Action::NewStack);
                    }
                } else {
                    egui::Popup::menu(&resp).show(|ui| self.stack_menu(ui, &m, actions));
                }
            });
            if self.stacks_of(&m).is_empty() {
                ui.label(RichText::new("A stack starts and stops a set of projects and containers together.").font(sans(11.5)).color(TEXT_3));
            }
        }

        if it.is_project() {
            self.drawer_section(ui, "Folder");
            ui.add(egui::Label::new(RichText::new(&it.folder).font(mono(11.5)).color(TEXT_2)).wrap().selectable(true));
        }
    }

    fn output(&self, ui: &mut Ui, it: &Item<'_>, actions: &mut Vec<Action>) {
        let log = self.logs.as_ref().filter(|l| Some(&l.key) == self.selected.as_ref());
        ui.horizontal(|ui| {
            let source = log.map_or("", |l| l.source);
            ui.label(RichText::new(source).font(sans(12.0)).color(TEXT_3));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(path) = log.and_then(|l| l.path.clone())
                    && ui.add(egui::Button::new(RichText::new("Open log file").font(medium(12.0)).color(TEXT_2)).fill(SURFACE_2).corner_radius(7)).clicked()
                {
                    actions.push(Action::Folder(path));
                }
            });
        });
        let running_elsewhere = log.is_some_and(|l| !l.ours) && it.row.is_some();
        if running_elsewhere {
            Frame::new().fill(SURFACE).stroke(Stroke::new(1.0, BORDER)).corner_radius(10).inner_margin(Margin::same(14)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new("Started outside DevDuck").font(medium(13.5)).color(TEXT));
                ui.label(
                    RichText::new("Its output goes to the terminal that started it, so DevDuck can't show it. Restart it here and DevDuck runs it with its output captured.")
                        .font(sans(12.5))
                        .color(TEXT_2),
                );
                if let Some(r) = it.row
                    && self.can_restart(r)
                    && widgets::button(ui, Some(Icon::Restart), "Restart here", Tone::Primary, !self.busy.contains_key(&r.key)).clicked()
                {
                    actions.push(Action::Restart(r.key.clone()));
                }
            });
            ui.add_space(6.0);
            if log.is_none_or(|l| l.text.trim().is_empty()) {
                return;
            }
            ui.label(RichText::new("Its last run from DevDuck:").font(sans(12.0)).color(TEXT_3));
        }
        let text = log.map(|l| l.text.trim_end().to_string()).unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        let tail = lines[lines.len().saturating_sub(500)..].join("\n");
        let empty_hint = match it.state {
            RunState::Starting | RunState::Busy(_) => "Waiting for output\u{2026}",
            _ if it.is_container() => "No logs.",
            _ if it.state.is_live() => "(no output yet)",
            _ => "Nothing yet. Start it and its output streams here.",
        };
        Frame::new().fill(BG).stroke(Stroke::new(1.0, BORDER)).corner_radius(10).inner_margin(Margin::same(10)).show(ui, |ui| {
            ScrollArea::vertical().id_salt(("output", &it.id)).stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
                ui.set_width(ui.available_width());
                if tail.trim().is_empty() {
                    ui.label(RichText::new(empty_hint).font(mono(11.5)).color(TEXT_3));
                } else {
                    ui.add(egui::Label::new(RichText::new(&tail).font(mono(11.5)).color(TEXT_2)).wrap().selectable(true));
                }
            });
        });
    }

    // ---------------------------------------------------------- sidebar --

    pub(crate) fn sidebar_stacks(&self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::hover());
        widgets::caps(ui.painter(), pos2(rect.left() + 6.0, rect.center().y), "Stacks", TEXT_3);
        let label = "+ New";
        let w = widgets::text_width(ui.painter(), label, medium(12.0)) + 14.0;
        let r = Rect::from_min_size(pos2(rect.right() - w, rect.top()), vec2(w, 24.0));
        let resp = ui.interact(r, Id::new("new-stack"), Sense::click()).on_hover_cursor(CursorIcon::PointingHand).on_hover_text("Group projects and containers you run together");
        resp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, "New stack"));
        if resp.hovered() {
            ui.painter().rect_filled(r, 6.0, tint(ACCENT, 0.12));
        }
        ui.painter().text(r.center(), Align2::CENTER_CENTER, label, medium(12.0), ACCENT);
        widgets::focus_ring(ui.painter(), r, 6.0, &resp);
        if resp.clicked() {
            actions.push(Action::NewStack);
        }
        ui.add_space(4.0);
        if self.state.stacks.is_empty() {
            ui.add(egui::Label::new(RichText::new("Start a project and its database with one click.").font(sans(12.0)).color(TEXT_3)).wrap());
            return;
        }
        for (i, s) in self.state.stacks.iter().enumerate() {
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 46.0), Sense::click());
            let id = resp.id;
            let (up, total) = self.stack_status(i);
            let selected = self.chip == Chip::Stack(i);
            let hovered = ui.rect_contains_pointer(rect);
            resp.widget_info(|| WidgetInfo::selected(WidgetType::SelectableLabel, true, selected, format!("Stack {}, {up} of {total} running", s.name)));
            let p = ui.painter().clone();
            if selected {
                p.rect_filled(rect, 9.0, SURFACE_2);
            } else if hovered {
                p.rect_filled(rect, 9.0, tint(SURFACE_2, 0.55));
            }
            widgets::focus_ring(&p, rect, 9.0, &resp);
            let (dc, hollow) = match (up, total) {
                (0, _) => (TEXT_3, true),
                (u, t) if u == t => (GREEN, false),
                _ => (AMBER, false),
            };
            widgets::dot(&p, pos2(rect.left() + 14.0, rect.top() + 16.0), dc, hollow);
            let text_w = rect.width() - 28.0 - 42.0;
            widgets::text(&p, pos2(rect.left() + 28.0, rect.top() + 16.0), Align2::LEFT_CENTER, &s.name, medium(13.5), TEXT, text_w);
            let busy = s.members.iter().any(|m| self.busy.contains_key(&format!("fav:container:{}", m.target)))
                || s.members.iter().any(|m| m.kind == FavoriteKind::Project && self.stack_queue.iter().any(|f| f.eq_ignore_ascii_case(&m.target)));
            let sub = if busy { "starting\u{2026}".to_string() } else { format!("{up} of {total} running") };
            widgets::text(&p, pos2(rect.left() + 28.0, rect.top() + 33.0), Align2::LEFT_CENTER, &sub, sans(11.5), TEXT_3, text_w);
            let button = Rect::from_min_size(pos2(rect.right() - 36.0, rect.center().y - 15.0), vec2(30.0, 30.0));
            if busy {
                widgets::spinner(&p, button.center(), ui.input(|i| i.time) as f32);
            } else if up < total {
                if widgets::go_button(ui, id.with("start"), button, &format!("Start {}", s.name)).clicked() {
                    actions.push(Action::StartStack(i));
                }
            } else if total > 0 && widgets::icon_button(ui, id.with("stop"), button, Icon::Stop, &format!("Stop {}", s.name), true, true).clicked() {
                actions.push(Action::StopStack(i));
            }
            if resp.clicked() {
                actions.push(Action::SetChip(Chip::Stack(i)));
            }
            resp.context_menu(|ui| {
                ui.set_min_width(160.0);
                if up < total && ui.button("Start all").clicked() {
                    actions.push(Action::StartStack(i));
                    ui.close();
                }
                if up > 0 && ui.button("Stop all").clicked() {
                    actions.push(Action::StopStack(i));
                    ui.close();
                }
                if ui.button("Edit\u{2026}").clicked() {
                    actions.push(Action::EditStack(i));
                    ui.close();
                }
                ui.separator();
                if ui.button("Delete stack").clicked() {
                    actions.push(Action::DeleteStack(i));
                    ui.close();
                }
            });
        }
    }

    // ---------------------------------------------------------- palette --

    pub(crate) fn palette_ui(&mut self, actions: &mut Vec<Action>) {
        let Some(query) = self.palette.as_ref().map(|p| p.query.clone()) else { return };
        let cmds = self.commands(&query);
        let ctx = self.ctx.clone();
        let (up, down, enter, esc) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_key(Modifiers::NONE, Key::Enter),
                i.consume_key(Modifiers::NONE, Key::Escape),
            )
        });
        if esc {
            self.palette = None;
            return;
        }
        let shown = cmds.len().min(9);
        let pal = self.palette.as_mut().unwrap();
        if shown > 0 {
            if down {
                pal.selected = (pal.selected + 1) % shown;
            }
            if up {
                pal.selected = (pal.selected + shown - 1) % shown;
            }
            pal.selected = pal.selected.min(shown - 1);
        }
        if enter {
            if let Some(c) = cmds.get(pal.selected) {
                actions.push(c.action.clone());
            }
            self.palette = None;
            return;
        }
        let mut chosen = None;
        let mut close = false;
        let frame = Frame::new().fill(SURFACE_HI).stroke(Stroke::new(1.0, BORDER_HI)).corner_radius(14).inner_margin(Margin::same(10));
        let modal = egui::Modal::new(Id::new("palette")).frame(frame).backdrop_color(Color32::from_black_alpha(130)).show(&ctx, |ui| {
            ui.set_width(580.0);
            ui.spacing_mut().item_spacing = vec2(0.0, 2.0);
            let (row, _) = ui.allocate_exact_size(vec2(580.0, 42.0), Sense::hover());
            icons::paint(ui.painter(), Rect::from_center_size(pos2(row.left() + 18.0, row.center().y), vec2(16.0, 16.0)), Icon::Command, ACCENT);
            let inner = Rect::from_min_max(pos2(row.left() + 40.0, row.center().y - 11.0), pos2(row.right() - 10.0, row.center().y + 11.0));
            let edit = TextEdit::singleline(&mut pal.query)
                .id(Id::new(PALETTE_ID))
                .frame(Frame::NONE)
                .margin(Margin::ZERO)
                .font(sans(16.0))
                .text_color(TEXT)
                .hint_text(RichText::new("Start, stop, open or run anything\u{2026}").color(TEXT_3).font(sans(16.0)))
                .desired_width(inner.width());
            let resp = ui.put(inner, edit);
            if !pal.focused || !resp.has_focus() {
                resp.request_focus();
                pal.focused = true;
            }
            if resp.changed() {
                pal.selected = 0;
            }
            let sep = ui.allocate_exact_size(vec2(580.0, 9.0), Sense::hover()).0;
            ui.painter().line_segment([pos2(sep.left() - 10.0, sep.center().y), pos2(sep.right() + 10.0, sep.center().y)], Stroke::new(1.0, BORDER_HI));
            if cmds.is_empty() {
                let (r, _) = ui.allocate_exact_size(vec2(580.0, 48.0), Sense::hover());
                ui.painter().text(r.center(), Align2::CENTER_CENTER, "No matching commands", sans(13.0), TEXT_3);
            }
            for (i, c) in cmds.iter().take(9).enumerate() {
                let (r, resp) = ui.allocate_exact_size(vec2(580.0, 38.0), Sense::click());
                let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
                resp.widget_info(|| WidgetInfo::selected(WidgetType::Button, true, i == pal.selected, &c.label));
                if resp.hovered() && ui.input(|inp| inp.pointer.delta() != egui::Vec2::ZERO) {
                    pal.selected = i;
                }
                let p = ui.painter();
                if i == pal.selected {
                    p.rect_filled(r, 8.0, SURFACE_2);
                }
                let tone = match c.icon {
                    Icon::Play => GREEN,
                    Icon::Stop => RED,
                    _ => TEXT_2,
                };
                icons::paint(p, Rect::from_center_size(pos2(r.left() + 18.0, r.center().y), vec2(14.0, 14.0)), c.icon, tone);
                let hint_w = if c.hint.is_empty() { 0.0 } else { widgets::text_width(p, &c.hint, mono(11.5)).min(220.0) + 14.0 };
                widgets::text(p, pos2(r.left() + 40.0, r.center().y), Align2::LEFT_CENTER, &c.label, medium(13.5), TEXT, r.width() - 52.0 - hint_w);
                if !c.hint.is_empty() {
                    widgets::text(p, pos2(r.right() - 12.0, r.center().y), Align2::RIGHT_CENTER, &c.hint, mono(11.5), TEXT_3, 220.0);
                }
                if resp.clicked() {
                    chosen = Some(c.action.clone());
                }
            }
            ui.add_space(6.0);
            let (f, _) = ui.allocate_exact_size(vec2(580.0, 22.0), Sense::hover());
            let more = cmds.len().saturating_sub(9);
            let foot = if more > 0 { format!("\u{2191}\u{2193} move \u{00B7} Enter runs \u{00B7} Esc closes \u{00B7} {more} more, keep typing") } else { "\u{2191}\u{2193} move \u{00B7} Enter runs \u{00B7} Esc closes".to_string() };
            ui.painter().text(pos2(f.left() + 10.0, f.center().y), Align2::LEFT_CENTER, foot, sans(11.5), TEXT_3);
        });
        if modal.should_close() {
            close = true;
        }
        if let Some(a) = chosen {
            actions.push(a);
            close = true;
        }
        if close {
            self.palette = None;
        }
    }
}

fn state_word(s: RunState) -> &'static str {
    match s {
        RunState::Running => "running",
        RunState::Starting => "starting",
        RunState::Busy(_) => "busy",
        RunState::Exited(Some(0)) => "finished",
        RunState::Exited(_) => "crashed",
        RunState::Stopped => "stopped",
    }
}

// ------------------------------------------------------------------ dialogs --

/// The stack editor's body. Candidates are (member, name, detail).
pub(crate) fn stack_editor(ui: &mut Ui, draft: &mut StackDraft, candidates: &[(Member, String, String)], actions: &mut Vec<Action>, close: &mut bool) {
    ui.set_width(480.0);
    let title = if draft.index.is_some() { "Edit stack" } else { "New stack" };
    ui.label(RichText::new(title).font(semibold(17.0)).color(TEXT));
    ui.label(RichText::new("Start and stop these together. Containers start first, so the database is up before the app.").font(sans(12.5)).color(TEXT_2));
    ui.add_space(6.0);
    ui.label(RichText::new("Name").font(medium(12.5)).color(TEXT_2));
    let name = ui.add(TextEdit::singleline(&mut draft.name).hint_text("Acme").font(sans(14.0)).desired_width(f32::INFINITY));
    if draft.index.is_none() && draft.name.is_empty() && !name.has_focus() && ui.ctx().memory(|m| m.focused().is_none()) {
        name.request_focus();
    }
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("Members ({})", draft.members.len())).font(medium(12.5)).color(TEXT_2));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add(TextEdit::singleline(&mut draft.filter).hint_text("Filter").font(sans(12.5)).desired_width(160.0));
        });
    });
    let needle = draft.filter.trim().to_lowercase();
    Frame::new().fill(BG).stroke(Stroke::new(1.0, BORDER)).corner_radius(10).inner_margin(Margin::same(6)).show(ui, |ui| {
        ScrollArea::vertical().max_height(300.0).auto_shrink([false, true]).show(ui, |ui| {
            ui.set_width(ui.available_width());
            for (m, name, detail) in candidates.iter().filter(|(_, n, d)| needle.is_empty() || n.to_lowercase().contains(&needle) || d.to_lowercase().contains(&needle)) {
                let mut on = draft.members.iter().any(|x| x.is(m.kind, &m.target));
                ui.horizontal(|ui| {
                    if ui.checkbox(&mut on, RichText::new(name).font(medium(13.0)).color(TEXT)).changed() {
                        if on {
                            draft.members.push(m.clone());
                        } else {
                            draft.members.retain(|x| !x.is(m.kind, &m.target));
                        }
                    }
                    let (icon, color) = if m.kind == FavoriteKind::Container { (Icon::Cube, CYAN) } else { (Icon::Code, TEXT_3) };
                    let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                    icons::paint(ui.painter(), r, icon, color);
                    ui.add(egui::Label::new(RichText::new(detail).font(mono(11.0)).color(TEXT_3)).truncate());
                });
            }
        });
    });
    ui.add_space(8.0);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let ok = !draft.members.is_empty();
        if widgets::button(ui, None, if draft.index.is_some() { "Save" } else { "Create stack" }, Tone::Primary, ok).clicked() {
            actions.push(Action::SaveStack(draft.clone()));
            *close = true;
        }
        if widgets::button(ui, None, "Cancel", Tone::Quiet, true).clicked() {
            *close = true;
        }
        if let Some(i) = draft.index
            && widgets::button(ui, None, "Delete", Tone::Danger, true).clicked()
        {
            actions.push(Action::DeleteStack(i));
            *close = true;
        }
    });
}

/// "Port :3000 is taken": stop the other thing and start, start anyway, or cancel.
#[allow(clippy::too_many_arguments)]
pub(crate) fn conflict_dialog(ui: &mut Ui, name: &str, holder_name: &str, holder_is_system: bool, folder: &str, script: &Option<String>, port: u16, holder: &str, free: u16, actions: &mut Vec<Action>, close: &mut bool) {
    ui.set_width(460.0);
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(18.0, 22.0), Sense::hover());
        icons::paint(ui.painter(), r, Icon::Alert, AMBER);
        ui.label(RichText::new(format!("Port :{port} is taken")).font(semibold(17.0)).color(TEXT));
    });
    ui.label(RichText::new(format!("{holder_name} is using :{port}, which {name} usually takes. :{free} is free, so DevDuck can run {name} there and leave {holder_name} alone.")).font(sans(13.0)).color(TEXT_2));
    ui.add_space(8.0);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if widgets::button(ui, Some(Icon::Play), &format!("Start on :{free}"), Tone::Primary, true).on_hover_text(format!("Runs {name} on :{free}")).clicked() {
            actions.push(Action::RunOnPort(folder.to_string(), script.clone(), free));
            *close = true;
        }
        if !holder_is_system
            && widgets::button(ui, None, &format!("Stop {holder_name} first"), Tone::Quiet, true).on_hover_text(format!("Stops {holder_name}, waits for :{port} to free up, then starts {name} there")).clicked()
        {
            actions.push(Action::RunForce(folder.to_string(), script.clone(), Some(holder.to_string())));
            *close = true;
        }
        if widgets::button(ui, None, "Cancel", Tone::Quiet, true).clicked() {
            *close = true;
        }
    });
}
