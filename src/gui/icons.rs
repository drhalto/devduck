//! A small line-icon set drawn with the painter, so every icon is crisp at any DPI and
//! takes its colour from context. Each draws into a square `rect` (16-20 px works best).

use std::f32::consts::{PI, TAU};

use eframe::egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, pos2, vec2};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon {
    Open,
    Copy,
    Folder,
    Code,
    Terminal,
    Restart,
    Stop,
    Play,
    Settings,
    Close,
    Search,
    Grid,
    List,
    Pause,
    Globe,
    Refresh,
}

pub fn paint(p: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let s = rect.width().min(rect.height());
    let r = Rect::from_center_size(rect.center(), vec2(s, s));
    let u = s / 16.0; // design grid: 16 units
    let at = |x: f32, y: f32| pos2(r.left() + x * u, r.top() + y * u);
    let stroke = Stroke::new((1.5 * u).max(1.2), color);
    let line = |pts: &[(f32, f32)]| {
        p.add(Shape::line(pts.iter().map(|&(x, y)| at(x, y)).collect(), stroke));
    };
    match icon {
        Icon::Open => {
            line(&[(9.0, 2.5), (13.5, 2.5), (13.5, 7.0)]);
            line(&[(13.5, 2.5), (7.5, 8.5)]);
            line(&[(11.5, 9.5), (11.5, 13.5), (2.5, 13.5), (2.5, 4.5), (6.5, 4.5)]);
        }
        Icon::Copy => {
            p.rect_stroke(Rect::from_min_max(at(5.5, 5.5), at(13.5, 13.5)), 2.0 * u, stroke, eframe::egui::StrokeKind::Middle);
            line(&[(10.5, 3.0), (10.5, 2.5), (2.5, 2.5), (2.5, 10.5), (3.0, 10.5)]);
        }
        Icon::Folder => {
            line(&[(2.0, 4.0), (6.0, 4.0), (7.5, 5.5), (14.0, 5.5), (14.0, 13.0), (2.0, 13.0), (2.0, 4.0)]);
        }
        Icon::Code => {
            line(&[(5.0, 4.5), (1.5, 8.0), (5.0, 11.5)]);
            line(&[(11.0, 4.5), (14.5, 8.0), (11.0, 11.5)]);
            line(&[(9.5, 3.0), (6.5, 13.0)]);
        }
        Icon::Terminal => {
            p.rect_stroke(Rect::from_min_max(at(1.5, 2.5), at(14.5, 13.5)), 2.0 * u, stroke, eframe::egui::StrokeKind::Middle);
            line(&[(4.5, 6.0), (6.5, 8.0), (4.5, 10.0)]);
            line(&[(8.5, 10.5), (11.5, 10.5)]);
        }
        Icon::Restart | Icon::Refresh => {
            let c = at(8.0, 8.0);
            let rad = 5.5 * u;
            let (a0, a1) = (-PI * 0.30, TAU - PI * 0.55);
            let pts: Vec<Pos2> = (0..=24).map(|i| {
                let a = a0 + (a1 - a0) * i as f32 / 24.0;
                c + vec2(a.cos(), a.sin()) * rad
            }).collect();
            let end = c + vec2(a0.cos(), a0.sin()) * rad;
            p.add(Shape::line(pts, stroke));
            // arrowhead at the start of the arc, pointing along the turn
            let tip = end;
            p.add(Shape::line(vec![tip + vec2(-3.2, -0.6) * u, tip, tip + vec2(-0.2, 3.2) * u], stroke));
        }
        Icon::Stop => {
            p.rect_filled(Rect::from_center_size(r.center(), vec2(9.0 * u, 9.0 * u)), 2.0 * u, color);
        }
        Icon::Play => {
            p.add(Shape::convex_polygon(vec![at(4.5, 2.5), at(13.5, 8.0), at(4.5, 13.5)], color, Stroke::NONE));
        }
        Icon::Settings => {
            for (y, knob) in [(4.0, 10.5), (8.0, 5.0), (12.0, 9.0)] {
                line(&[(2.0, y), (14.0, y)]);
                p.circle_filled(at(knob, y), 2.2 * u, color);
            }
        }
        Icon::Close => {
            line(&[(3.5, 3.5), (12.5, 12.5)]);
            line(&[(12.5, 3.5), (3.5, 12.5)]);
        }
        Icon::Search => {
            p.circle_stroke(at(7.0, 7.0), 4.5 * u, stroke);
            line(&[(10.5, 10.5), (14.0, 14.0)]);
        }
        Icon::Grid => {
            for (x, y) in [(2.0, 2.0), (9.0, 2.0), (2.0, 9.0), (9.0, 9.0)] {
                p.rect_stroke(Rect::from_min_size(at(x, y), vec2(5.0 * u, 5.0 * u)), 1.2 * u, stroke, eframe::egui::StrokeKind::Middle);
            }
        }
        Icon::List => {
            for y in [3.5, 8.0, 12.5] {
                p.circle_filled(at(2.5, y), 1.1 * u, color);
                line(&[(5.5, y), (14.0, y)]);
            }
        }
        Icon::Pause => {
            p.rect_filled(Rect::from_min_max(at(4.0, 3.0), at(6.8, 13.0)), 1.0 * u, color);
            p.rect_filled(Rect::from_min_max(at(9.2, 3.0), at(12.0, 13.0)), 1.0 * u, color);
        }
        Icon::Globe => {
            p.circle_stroke(at(8.0, 8.0), 6.0 * u, stroke);
            line(&[(2.0, 8.0), (14.0, 8.0)]);
            let pts: Vec<Pos2> = (0..=20).map(|i| {
                let a = -PI / 2.0 + PI * i as f32 / 20.0;
                at(8.0 + 2.6 * a.cos(), 8.0 + 6.0 * a.sin())
            }).collect();
            p.add(Shape::line(pts.clone(), stroke));
            p.add(Shape::line(pts.into_iter().map(|q| pos2(2.0 * at(8.0, 0.0).x - q.x, q.y)).collect(), stroke));
        }
    }
}

/// The Dev Ports mark: a tile with three coloured rows (dev, docker, app).
pub fn logo(p: &Painter, rect: Rect) {
    use super::theme::*;
    p.rect_filled(rect, rect.width() * 0.26, SURFACE_2);
    let h = rect.height();
    for (i, c) in [GREEN, CYAN, TEXT].into_iter().enumerate() {
        let y = rect.top() + h * (0.31 + 0.19 * i as f32);
        let t = h * 0.065;
        p.rect_filled(Rect::from_min_max(pos2(rect.left() + h * 0.20, y - t), pos2(rect.left() + h * 0.33, y + t)), t, c);
        p.rect_filled(Rect::from_min_max(pos2(rect.left() + h * 0.41, y - t), pos2(rect.left() + h * 0.80, y + t)), t, c);
    }
}
