//! Line icons drawn with the painter (no image files, no emoji), styled
//! after Tinkercad's toolbar.

use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, Vec2, pos2, vec2};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Copy,
    Paste,
    Duplicate,
    Delete,
    Undo,
    Redo,
    Bulb,
    BulbOff,
    Bundle,
    Union,
    Intersect,
    Ungroup,
    Align,
    Mirror,
    Measure,
    LockOpen,
    LockClosed,
    ChevronUp,
    ChevronDown,
    Search,
    Home,
    Fit,
}

/// Map unit coordinates (0..1, y down) into the rect.
struct U {
    r: Rect,
}

impl U {
    fn p(&self, x: f32, y: f32) -> Pos2 {
        pos2(self.r.min.x + x * self.r.width(), self.r.min.y + y * self.r.height())
    }
    fn s(&self, v: f32) -> f32 {
        v * self.r.width()
    }
    fn rect(&self, x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
        Rect::from_min_max(self.p(x0, y0), self.p(x1, y1))
    }
}

fn poly(p: &Painter, pts: Vec<Pos2>, st: Stroke) {
    p.add(Shape::line(pts, st));
}

fn arc(u: &U, cx: f32, cy: f32, r: f32, a0: f32, a1: f32) -> Vec<Pos2> {
    (0..=20)
        .map(|i| {
            let a = a0 + (a1 - a0) * i as f32 / 20.0;
            u.p(cx + r * a.cos(), cy + r * a.sin())
        })
        .collect()
}

fn arrow_head(p: &Painter, tip: Pos2, dir: Vec2, size: f32, col: Color32) {
    let d = dir.normalized();
    let side = vec2(-d.y, d.x);
    p.add(Shape::convex_polygon(
        vec![tip + d * size * 0.6, tip - d * size * 0.5 + side * size * 0.6, tip - d * size * 0.5 - side * size * 0.6],
        col,
        Stroke::NONE,
    ));
}

/// Paint `icon` filling `rect`. `col` is the line colour, `accent` the fill
/// used for highlighted parts (e.g. the union's merged shape).
pub fn paint(p: &Painter, rect: Rect, icon: Icon, col: Color32, accent: Color32) {
    let u = U { r: rect };
    let st = Stroke::new(u.s(0.075).max(1.3), col);
    let thin = Stroke::new(u.s(0.05).max(1.0), col);
    let rr = u.s(0.06);
    match icon {
        Icon::Copy => {
            p.rect_stroke(u.rect(0.3, 0.12, 0.88, 0.7), rr, st, egui::StrokeKind::Middle);
            p.rect_filled(u.rect(0.12, 0.3, 0.7, 0.88), rr, Color32::WHITE);
            p.rect_stroke(u.rect(0.12, 0.3, 0.7, 0.88), rr, st, egui::StrokeKind::Middle);
        }
        Icon::Paste => {
            p.rect_stroke(u.rect(0.18, 0.18, 0.82, 0.9), rr, st, egui::StrokeKind::Middle);
            p.rect_filled(u.rect(0.35, 0.08, 0.65, 0.26), rr, Color32::WHITE);
            p.rect_stroke(u.rect(0.35, 0.08, 0.65, 0.26), rr, st, egui::StrokeKind::Middle);
            p.line_segment([u.p(0.32, 0.5), u.p(0.68, 0.5)], thin);
            p.line_segment([u.p(0.32, 0.66), u.p(0.68, 0.66)], thin);
        }
        Icon::Duplicate => {
            p.rect_stroke(u.rect(0.36, 0.1, 0.9, 0.64), rr, st, egui::StrokeKind::Middle);
            poly(p, vec![u.p(0.26, 0.2), u.p(0.26, 0.74), u.p(0.8, 0.74)], st);
            poly(p, vec![u.p(0.12, 0.34), u.p(0.12, 0.88), u.p(0.66, 0.88)], st);
        }
        Icon::Delete => {
            p.line_segment([u.p(0.14, 0.24), u.p(0.86, 0.24)], st);
            poly(p, vec![u.p(0.38, 0.24), u.p(0.38, 0.12), u.p(0.62, 0.12), u.p(0.62, 0.24)], st);
            poly(p, vec![u.p(0.22, 0.24), u.p(0.28, 0.9), u.p(0.72, 0.9), u.p(0.78, 0.24)], st);
            for x in [0.42, 0.58] {
                p.line_segment([u.p(x, 0.4), u.p(x, 0.76)], thin);
            }
        }
        Icon::Undo | Icon::Redo => {
            let flip = icon == Icon::Redo;
            let fx = |x: f32| if flip { 1.0 - x } else { x };
            poly(
                p,
                vec![u.p(fx(0.18), 0.45), u.p(fx(0.45), 0.2), u.p(fx(0.45), 0.36), u.p(fx(0.86), 0.36)],
                st,
            );
            poly(p, vec![u.p(fx(0.18), 0.45), u.p(fx(0.45), 0.7), u.p(fx(0.45), 0.54), u.p(fx(0.86), 0.54)], st);
        }
        Icon::Bulb | Icon::BulbOff => {
            let pts = arc(&u, 0.5, 0.4, 0.26, std::f32::consts::PI * 0.75, std::f32::consts::PI * 2.25);
            poly(p, pts, st);
            poly(p, vec![u.p(0.32, 0.58), u.p(0.38, 0.72), u.p(0.62, 0.72), u.p(0.68, 0.58)], st);
            p.line_segment([u.p(0.4, 0.82), u.p(0.6, 0.82)], st);
            p.line_segment([u.p(0.44, 0.92), u.p(0.56, 0.92)], st);
            if icon == Icon::BulbOff {
                p.line_segment([u.p(0.12, 0.08), u.p(0.88, 0.92)], st);
            }
        }
        Icon::Bundle => {
            // Dotted frame around a square and a circle, unmerged.
            let dots = Stroke::new(u.s(0.06).max(1.2), col);
            for (x, y) in [(0.08, 0.08), (0.92, 0.08), (0.08, 0.92), (0.92, 0.92), (0.5, 0.08), (0.5, 0.92), (0.08, 0.5), (0.92, 0.5)] {
                p.circle_filled(u.p(x, y), u.s(0.045), dots.color);
            }
            p.rect_stroke(u.rect(0.2, 0.22, 0.56, 0.58), rr, st, egui::StrokeKind::Middle);
            p.circle_stroke(u.p(0.6, 0.62), u.s(0.2), st);
        }
        Icon::Union => {
            p.rect_filled(u.rect(0.12, 0.12, 0.62, 0.62), rr, accent);
            p.circle_filled(u.p(0.62, 0.62), u.s(0.26), accent);
            p.rect_stroke(u.rect(0.12, 0.12, 0.62, 0.62), rr, st, egui::StrokeKind::Middle);
            p.circle_stroke(u.p(0.62, 0.62), u.s(0.26), st);
        }
        Icon::Intersect => {
            p.rect_stroke(u.rect(0.12, 0.12, 0.62, 0.62), rr, thin, egui::StrokeKind::Middle);
            p.circle_stroke(u.p(0.62, 0.62), u.s(0.26), thin);
            // The overlapping quarter.
            let mut pts = vec![u.p(0.62, 0.36), u.p(0.62, 0.62), u.p(0.36, 0.62)];
            pts.extend(arc(&u, 0.62, 0.62, 0.26, std::f32::consts::PI, std::f32::consts::PI * 1.5));
            p.add(Shape::convex_polygon(pts.clone(), accent, Stroke::NONE));
            p.add(Shape::closed_line(pts, st));
        }
        Icon::Ungroup => {
            p.rect_stroke(u.rect(0.1, 0.1, 0.5, 0.5), rr, st, egui::StrokeKind::Middle);
            p.circle_stroke(u.p(0.7, 0.7), u.s(0.2), st);
            for (x, y) in [(0.62, 0.22), (0.76, 0.12), (0.84, 0.32), (0.18, 0.78), (0.3, 0.88)] {
                p.circle_filled(u.p(x, y), u.s(0.04), col);
            }
        }
        Icon::Align => {
            p.line_segment([u.p(0.14, 0.08), u.p(0.14, 0.92)], st);
            p.rect_stroke(u.rect(0.24, 0.18, 0.86, 0.42), rr, st, egui::StrokeKind::Middle);
            p.rect_stroke(u.rect(0.24, 0.58, 0.62, 0.82), rr, st, egui::StrokeKind::Middle);
        }
        Icon::Mirror => {
            p.add(Shape::closed_line(vec![u.p(0.42, 0.14), u.p(0.42, 0.86), u.p(0.1, 0.86)], st));
            p.add(Shape::closed_line(vec![u.p(0.58, 0.14), u.p(0.58, 0.86), u.p(0.9, 0.86)], st));
        }
        Icon::Measure => {
            // Tape measure: round case with tape and tick marks.
            p.circle_stroke(u.p(0.38, 0.5), u.s(0.28), st);
            p.circle_stroke(u.p(0.38, 0.5), u.s(0.08), thin);
            poly(p, vec![u.p(0.38, 0.78), u.p(0.92, 0.78), u.p(0.92, 0.62)], st);
            for x in [0.6, 0.72, 0.84] {
                p.line_segment([u.p(x, 0.78), u.p(x, 0.7)], thin);
            }
        }
        Icon::LockOpen | Icon::LockClosed => {
            p.rect_filled(u.rect(0.18, 0.46, 0.82, 0.9), rr, accent);
            p.rect_stroke(u.rect(0.18, 0.46, 0.82, 0.9), rr, st, egui::StrokeKind::Middle);
            let (x0, x1) = (0.32, 0.68);
            if icon == Icon::LockClosed {
                let mut pts = vec![u.p(x0, 0.46)];
                pts.extend(arc(&u, 0.5, 0.3, 0.18, std::f32::consts::PI, std::f32::consts::TAU));
                pts.push(u.p(x1, 0.46));
                poly(p, pts, st);
            } else {
                let mut pts = vec![u.p(x1 + 0.14, 0.4)];
                pts.extend(arc(&u, 0.68, 0.26, 0.18, std::f32::consts::TAU, std::f32::consts::PI));
                pts.push(u.p(x1 - 0.36, 0.46));
                poly(p, pts.into_iter().rev().collect(), st);
            }
            p.circle_filled(u.p(0.5, 0.66), u.s(0.05), col);
        }
        Icon::ChevronUp => poly(p, vec![u.p(0.2, 0.65), u.p(0.5, 0.35), u.p(0.8, 0.65)], st),
        Icon::ChevronDown => poly(p, vec![u.p(0.2, 0.35), u.p(0.5, 0.65), u.p(0.8, 0.35)], st),
        Icon::Search => {
            p.circle_stroke(u.p(0.42, 0.42), u.s(0.26), st);
            p.line_segment([u.p(0.62, 0.62), u.p(0.88, 0.88)], st);
        }
        Icon::Home => {
            poly(p, vec![u.p(0.12, 0.5), u.p(0.5, 0.14), u.p(0.88, 0.5)], st);
            poly(p, vec![u.p(0.24, 0.42), u.p(0.24, 0.86), u.p(0.76, 0.86), u.p(0.76, 0.42)], st);
        }
        Icon::Fit => {
            for (x, y, dx, dy) in [(0.1, 0.1, 1.0, 1.0), (0.9, 0.1, -1.0, 1.0), (0.1, 0.9, 1.0, -1.0), (0.9, 0.9, -1.0, -1.0)] {
                poly(p, vec![u.p(x + dx * 0.25, y), u.p(x, y), u.p(x, y + dy * 0.25)], st);
            }
            p.rect_stroke(u.rect(0.34, 0.34, 0.66, 0.66), rr, thin, egui::StrokeKind::Middle);
        }
    }
    let _ = arrow_head;
}

/// A toolbar-style icon button. Returns the response.
pub fn button(ui: &mut egui::Ui, icon: Icon, enabled: bool, tip: &str) -> egui::Response {
    let size = vec2(34.0, 30.0);
    let (rect, resp) = ui.allocate_exact_size(size, if enabled { egui::Sense::click() } else { egui::Sense::hover() });
    let hovered = enabled && resp.hovered();
    if hovered {
        ui.painter().rect_filled(rect, 4.0, Color32::from_rgb(0xe3, 0xec, 0xf7));
    }
    let col = if enabled { Color32::from_rgb(0x2e, 0x3a, 0x4a) } else { Color32::from_rgb(0xb8, 0xbf, 0xc8) };
    let accent = if enabled { Color32::from_rgb(0xb7, 0xd4, 0xf5) } else { Color32::from_rgb(0xe6, 0xea, 0xee) };
    let r = Rect::from_center_size(rect.center(), vec2(22.0, 22.0));
    paint(ui.painter(), r, icon, col, accent);
    resp.on_hover_text(tip)
}

/// Draw a curved double-headed arrow along `pts` (a polyline).
pub fn curved_double_arrow(p: &Painter, pts: &[Pos2], col: Color32, width: f32) {
    if pts.len() < 2 {
        return;
    }
    p.add(Shape::line(pts.to_vec(), Stroke::new(width, col)));
    let n = pts.len();
    arrow_head(p, pts[0], pts[0] - pts[1], width * 4.2, col);
    arrow_head(p, pts[n - 1], pts[n - 1] - pts[n - 2], width * 4.2, col);
}
