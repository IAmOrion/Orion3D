//! 3D viewport: rendering, picking, Tinkercad-style handles and dragging.

use crate::app::OrionApp;
use crate::doc::{NodeId, ShapeKind};
use crate::geom;
use crate::ops;
use crate::render::{DrawItem, SceneParams};
use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2, pos2, vec2};
use glam::{DMat4, DVec3, Vec3};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HandleKind {
    /// Bottom corner: scale X and Y. Signs give which corner.
    Corner(i8, i8),
    /// Bottom edge midpoint: scale one axis (0 = X, 1 = Y) on one side.
    Side(usize, i8),
    /// Top centre: scale Z.
    Top,
    /// Black cone above the object: move up/down.
    Lift,
    /// Rotation arc around an axis.
    Rotate(usize),
}

pub struct Handle {
    kind: HandleKind,
    pos: Pos2,
    arc: Vec<Pos2>,
}

pub enum Drag {
    Orbit,
    Pan,
    Marquee { start: Pos2, additive: bool },
    Move {
        originals: ops::Originals,
        plane_z: f64,
        start_hit: DVec3,
        press: Pos2,
        moved: bool,
        clicked: Option<NodeId>,
        delta: DVec3,
    },
    Lift { originals: ops::Originals, origin: DVec3, start_s: f64, delta: f64 },
    Scale { originals: ops::Originals, kind: HandleKind, lo: DVec3, hi: DVec3, new_size: DVec3 },
    Rotate {
        originals: ops::Originals,
        axis: usize,
        center: DVec3,
        radius: f64,
        last: f64,
        total: f64,
        applied: f64,
    },
}

struct View {
    rect: Rect,
    aspect: f32,
}

impl View {
    fn uv(&self, p: Pos2) -> [f32; 2] {
        [(p.x - self.rect.min.x) / self.rect.width(), (p.y - self.rect.min.y) / self.rect.height()]
    }
}

fn axis_basis(axis: usize) -> (DVec3, DVec3, DVec3) {
    // (normal, u, v) with u x v = normal.
    match axis {
        0 => (DVec3::X, DVec3::Y, DVec3::Z),
        1 => (DVec3::Y, DVec3::Z, DVec3::X),
        _ => (DVec3::Z, DVec3::X, DVec3::Y),
    }
}

fn ray_plane(o: DVec3, d: DVec3, p0: DVec3, n: DVec3) -> Option<DVec3> {
    let den = d.dot(n);
    if den.abs() < 1e-9 {
        return None;
    }
    let t = (p0 - o).dot(n) / den;
    (t > 0.0).then(|| o + d * t)
}

/// Parameter s of the point on line a + s*u closest to the ray o + t*d.
fn line_param(a: DVec3, u: DVec3, o: DVec3, d: DVec3) -> Option<f64> {
    let w0 = a - o;
    let b = u.dot(d);
    let den = 1.0 - b * b;
    if den.abs() < 1e-6 {
        return None;
    }
    Some((b * d.dot(w0) - u.dot(w0)) / den)
}

fn dist_to_polyline(p: Pos2, pts: &[Pos2]) -> f32 {
    let mut best = f32::MAX;
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let ab = b - a;
        let t = ((p - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
        best = best.min((a + ab * t).distance(p));
    }
    best
}

fn wrap_angle(a: f64) -> f64 {
    let mut a = a;
    while a > std::f64::consts::PI {
        a -= std::f64::consts::TAU;
    }
    while a < -std::f64::consts::PI {
        a += std::f64::consts::TAU;
    }
    a
}

const HANDLE_R: f32 = 5.0;
const ACCENT: Color32 = Color32::from_rgb(0x2b, 0x7c, 0xe9);

impl OrionApp {
    fn project(&self, v: &View, p: DVec3) -> Option<Pos2> {
        self.camera.project(p.as_vec3(), v.aspect).map(|uv| {
            pos2(v.rect.min.x + uv[0] * v.rect.width(), v.rect.min.y + uv[1] * v.rect.height())
        })
    }

    fn ray(&self, v: &View, p: Pos2) -> (DVec3, DVec3) {
        let (o, d) = self.camera.ray(v.uv(p), v.aspect);
        (o.as_dvec3(), d.as_dvec3())
    }

    /// Closest top-level node under the pointer and the hit point.
    fn pick(&mut self, v: &View, p: Pos2) -> Option<(NodeId, DVec3)> {
        let (o, d) = self.ray(v, p);
        let mut best: Option<(f64, NodeId)> = None;
        for i in 0..self.doc.nodes.len() {
            let n = &self.doc.nodes[i];
            let inv = n.transform.0.inverse();
            let lo = inv.transform_point3(o).as_vec3();
            let ld = inv.transform_vector3(d).as_vec3();
            let id = n.id;
            let (_, mesh) = self.cache.content_mesh(&self.doc.nodes[i].content);
            if let Some(t) = geom::ray_hit(&mesh, lo, ld) {
                let t = t as f64;
                if best.is_none_or(|b| t < b.0) {
                    best = Some((t, id));
                }
            }
        }
        best.map(|(t, id)| (id, o + d * t))
    }

    fn handle_gap(&self) -> f64 {
        self.camera.dist as f64 * 0.045
    }

    fn compute_handles(&self, v: &View, lo: DVec3, hi: DVec3) -> Vec<Handle> {
        let mut out = Vec::new();
        let c = (lo + hi) * 0.5;
        let mut add = |kind, p: DVec3| {
            if let Some(pos) = self.project(v, p) {
                out.push(Handle { kind, pos, arc: Vec::new() });
            }
        };
        for (sx, sy) in [(-1i8, -1i8), (1, -1), (1, 1), (-1, 1)] {
            let x = if sx < 0 { lo.x } else { hi.x };
            let y = if sy < 0 { lo.y } else { hi.y };
            add(HandleKind::Corner(sx, sy), DVec3::new(x, y, lo.z));
        }
        add(HandleKind::Side(0, -1), DVec3::new(lo.x, c.y, lo.z));
        add(HandleKind::Side(0, 1), DVec3::new(hi.x, c.y, lo.z));
        add(HandleKind::Side(1, -1), DVec3::new(c.x, lo.y, lo.z));
        add(HandleKind::Side(1, 1), DVec3::new(c.x, hi.y, lo.z));
        add(HandleKind::Top, DVec3::new(c.x, c.y, hi.z));
        add(HandleKind::Lift, DVec3::new(c.x, c.y, hi.z + self.handle_gap() * 1.4));

        for axis in 0..3 {
            let (center, radius) = self.ring(axis, lo, hi);
            let (_, u, w) = axis_basis(axis);
            let eye = self.camera.eye().as_dvec3();
            let to_eye = eye - center;
            let (pu, pw) = (to_eye.dot(u), to_eye.dot(w));
            let mid = if pu.abs() + pw.abs() < 1e-6 { 0.0 } else { pw.atan2(pu) };
            // Spread the three arcs so they do not overlap.
            let mid = mid + [0.55, -0.55, 0.0][axis];
            let span = 0.5;
            let pts: Vec<Pos2> = (0..=16)
                .filter_map(|i| {
                    let a = mid - span + span * 2.0 * i as f64 / 16.0;
                    self.project(v, center + (u * a.cos() + w * a.sin()) * radius)
                })
                .collect();
            if pts.len() > 2 {
                let pos = pts[pts.len() / 2];
                out.push(Handle { kind: HandleKind::Rotate(axis), pos, arc: pts });
            }
        }
        out
    }

    fn ring(&self, axis: usize, lo: DVec3, hi: DVec3) -> (DVec3, f64) {
        let c = (lo + hi) * 0.5;
        let e = hi - lo;
        let gap = self.handle_gap();
        match axis {
            0 => (c, 0.5 * (e.y * e.y + e.z * e.z).sqrt() + gap),
            1 => (c, 0.5 * (e.x * e.x + e.z * e.z).sqrt() + gap),
            _ => (DVec3::new(c.x, c.y, lo.z), 0.5 * (e.x * e.x + e.y * e.y).sqrt() + gap),
        }
    }

    fn hit_handle(handles: &[Handle], p: Pos2) -> Option<HandleKind> {
        // Squares and the lift cone take priority over arcs.
        for h in handles {
            if !matches!(h.kind, HandleKind::Rotate(_)) && h.pos.distance(p) <= HANDLE_R + 4.0 {
                return Some(h.kind);
            }
        }
        handles
            .iter()
            .filter(|h| matches!(h.kind, HandleKind::Rotate(_)))
            .map(|h| (dist_to_polyline(p, &h.arc), h.kind))
            .filter(|(d, _)| *d < 10.0)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, k)| k)
    }

    pub fn viewport(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let rect = ui.max_rect();
        let response = ui.allocate_rect(rect, Sense::click_and_drag());
        let v = View { rect, aspect: rect.width() / rect.height().max(1.0) };
        let ctx = ui.ctx().clone();
        let pointer = ctx.input(|i| i.pointer.hover_pos());
        let snap = self.doc.settings.snap;

        // Selection bounds and handles (as of before this frame's input).
        let mut bounds = ops::bounds_of(&mut self.cache, &self.doc, &self.selection);
        let mut handles = bounds.map(|(lo, hi)| self.compute_handles(&v, lo, hi)).unwrap_or_default();

        // ------------------------------------------------------------------
        // Input
        // ------------------------------------------------------------------
        let (pressed, released, primary_down, secondary_down, middle_down, shift, mods) = ctx.input(|i| {
            (
                i.pointer.any_pressed(),
                i.pointer.any_released(),
                i.pointer.primary_down(),
                i.pointer.secondary_down(),
                i.pointer.middle_down(),
                i.modifiers.shift,
                i.modifiers,
            )
        });

        let hovered_handle = pointer.and_then(|p| Self::hit_handle(&handles, p));

        // Drop from the shape library.
        if let Some(kind) = response.dnd_release_payload::<ShapeKind>() {
            if let Some(p) = pointer {
                let at = self.drop_point(&v, p);
                self.add_shape(*kind, Some(at));
            }
        }

        if pressed && response.hovered() && self.drag.is_none() {
            if let Some(p) = pointer {
                if secondary_down && !shift {
                    self.drag = Some(Drag::Orbit);
                } else if middle_down || (secondary_down && shift) {
                    self.drag = Some(Drag::Pan);
                } else if primary_down {
                    self.begin_primary(&v, p, hovered_handle, bounds, shift);
                }
            }
        }

        // Camera wheel zoom towards the cursor.
        if response.hovered() {
            let scroll = ctx.input(|i| i.smooth_scroll_delta.y);
            if scroll.abs() > 0.0 {
                let towards = pointer.map(|p| self.zoom_point(&v, p));
                self.camera.zoom((-scroll * 0.0025).exp(), towards);
            }
        }

        let delta = ctx.input(|i| i.pointer.delta());
        if let Some(p) = pointer {
            self.update_drag(&v, p, delta, mods, snap);
        }

        let any_down = primary_down || secondary_down || middle_down;
        if (released && !any_down) || (self.drag.is_some() && !any_down) {
            self.end_drag(&v, pointer, shift);
        }

        // Recompute after edits so drawing reflects this frame.
        if self.drag.is_some() || pressed || released {
            bounds = ops::bounds_of(&mut self.cache, &self.doc, &self.selection);
            handles = bounds.map(|(lo, hi)| self.compute_handles(&v, lo, hi)).unwrap_or_default();
        }

        // Hover highlight.
        self.hovered = if self.drag.is_none() && response.hovered() && hovered_handle.is_none() {
            pointer.and_then(|p| self.pick(&v, p)).map(|(id, _)| id)
        } else {
            None
        };

        // Cursor feedback.
        if let Some(k) = hovered_handle.or(match &self.drag {
            Some(Drag::Scale { kind, .. }) => Some(*kind),
            Some(Drag::Rotate { axis, .. }) => Some(HandleKind::Rotate(*axis)),
            Some(Drag::Lift { .. }) => Some(HandleKind::Lift),
            _ => None,
        }) {
            ctx.set_cursor_icon(match k {
                HandleKind::Lift | HandleKind::Top => egui::CursorIcon::ResizeVertical,
                HandleKind::Rotate(_) => egui::CursorIcon::Grab,
                _ => egui::CursorIcon::Crosshair,
            });
        } else if matches!(self.drag, Some(Drag::Move { moved: true, .. })) {
            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        } else if self.hovered.is_some() {
            ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
        }

        // ------------------------------------------------------------------
        // Render
        // ------------------------------------------------------------------
        self.render_scene(ui, frame, rect);

        // ------------------------------------------------------------------
        // Overlays
        // ------------------------------------------------------------------
        let painter = ui.painter_at(rect);
        if let Some((lo, hi)) = bounds {
            self.draw_bounds(&painter, &v, lo, hi);
            let dragging_rotate = matches!(self.drag, Some(Drag::Rotate { .. }));
            if !dragging_rotate {
                for h in &handles {
                    draw_handle(&painter, h, hovered_handle == Some(h.kind));
                }
            }
            self.draw_drag_feedback(&painter, &v, lo, hi);
        }
        if let (Some(Drag::Marquee { start, .. }), Some(p)) = (&self.drag, pointer) {
            let r = Rect::from_two_pos(*start, p);
            painter.rect_filled(r, 0.0, Color32::from_rgba_unmultiplied(43, 124, 233, 30));
            painter.rect_stroke(r, 0.0, Stroke::new(1.0, ACCENT), egui::StrokeKind::Inside);
        }
        if self.doc.nodes.is_empty() {
            painter.text(
                rect.center_top() + vec2(0.0, 40.0),
                Align2::CENTER_TOP,
                "Drag a shape from the right onto the workplane to begin",
                FontId::proportional(15.0),
                Color32::from_gray(120),
            );
        }

        self.view_controls(ui, rect);
        self.inspector(ui, rect);

        if self.drag.is_some() {
            ctx.request_repaint();
        }
    }

    fn zoom_point(&mut self, v: &View, p: Pos2) -> Vec3 {
        if let Some((_, hit)) = self.pick(v, p) {
            return hit.as_vec3();
        }
        let (o, d) = self.ray(v, p);
        if let Some(h) = ray_plane(o, d, DVec3::ZERO, DVec3::Z) {
            if h.distance(o) < self.camera.dist as f64 * 6.0 {
                return h.as_vec3();
            }
        }
        self.camera.target
    }

    fn drop_point(&mut self, v: &View, p: Pos2) -> DVec3 {
        if let Some((_, hit)) = self.pick(v, p) {
            return hit;
        }
        let (o, d) = self.ray(v, p);
        ray_plane(o, d, DVec3::ZERO, DVec3::Z).unwrap_or_else(|| {
            let t = self.camera.target.as_dvec3();
            DVec3::new(t.x, t.y, 0.0)
        })
    }

    fn begin_primary(
        &mut self,
        v: &View,
        p: Pos2,
        handle: Option<HandleKind>,
        bounds: Option<(DVec3, DVec3)>,
        shift: bool,
    ) {
        let originals = ops::originals(&self.doc, &self.selection);
        if let (Some(kind), Some((lo, hi))) = (handle, bounds) {
            let (o, d) = self.ray(v, p);
            match kind {
                HandleKind::Lift => {
                    let c = (lo + hi) * 0.5;
                    let origin = DVec3::new(c.x, c.y, hi.z);
                    let start_s = line_param(origin, DVec3::Z, o, d).unwrap_or(0.0);
                    self.drag = Some(Drag::Lift { originals, origin, start_s, delta: 0.0 });
                }
                HandleKind::Rotate(axis) => {
                    let (center, radius) = self.ring(axis, lo, hi);
                    let start = self.ring_angle(v, p, axis, center).unwrap_or(0.0);
                    // Rotate about the true centre of the selection.
                    let center = if axis == 2 { (lo + hi) * 0.5 } else { center };
                    self.drag = Some(Drag::Rotate {
                        originals,
                        axis,
                        center,
                        radius,
                        last: start,
                        total: 0.0,
                        applied: 0.0,
                    });
                }
                _ => {
                    self.drag = Some(Drag::Scale { originals, kind, lo, hi, new_size: hi - lo });
                }
            }
            return;
        }

        if let Some((id, hit)) = self.pick(v, p) {
            let mut clicked = None;
            if shift {
                if let Some(i) = self.selection.iter().position(|x| *x == id) {
                    self.selection.remove(i);
                    return;
                }
                self.selection.push(id);
            } else if !self.selection.contains(&id) {
                self.selection = vec![id];
            } else {
                clicked = Some(id);
            }
            self.drag = Some(Drag::Move {
                originals: ops::originals(&self.doc, &self.selection),
                plane_z: hit.z,
                start_hit: hit,
                press: p,
                moved: false,
                clicked,
                delta: DVec3::ZERO,
            });
        } else {
            self.drag = Some(Drag::Marquee { start: p, additive: shift });
        }
    }

    fn ring_angle(&self, v: &View, p: Pos2, axis: usize, center: DVec3) -> Option<f64> {
        let (n, u, w) = axis_basis(axis);
        let (o, d) = self.ray(v, p);
        if d.dot(n).abs() > 0.08 {
            let hit = ray_plane(o, d, center, n)?;
            let r = hit - center;
            return Some(r.dot(w).atan2(r.dot(u)));
        }
        // Ring seen edge-on: fall back to screen-space angle.
        let c = self.project(v, center)?;
        let a = (p.y - c.y).atan2(p.x - c.x) as f64;
        let toward = (self.camera.eye().as_dvec3() - center).dot(n).signum();
        Some(-a * toward)
    }

    fn update_drag(&mut self, v: &View, p: Pos2, delta: Vec2, mods: egui::Modifiers, snap: f64) {
        let Some(mut drag) = self.drag.take() else { return };
        match &mut drag {
            Drag::Orbit => self.camera.orbit(delta.x, delta.y),
            Drag::Pan => self.camera.pan(delta.x, delta.y, v.rect.height()),
            Drag::Marquee { .. } => {}
            Drag::Move { originals, plane_z, start_hit, press, moved, delta: dv, .. } => {
                if !*moved && p.distance(*press) > 3.0 {
                    *moved = true;
                }
                if *moved {
                    let (o, d) = self.ray(v, p);
                    if let Some(h) = ray_plane(o, d, DVec3::new(0.0, 0.0, *plane_z), DVec3::Z) {
                        let raw = h - *start_hit;
                        let mut dd = DVec3::new(ops::snap(raw.x, snap), ops::snap(raw.y, snap), 0.0);
                        // Keep wild jumps (ray near horizon) in check.
                        let limit = self.camera.dist as f64 * 4.0;
                        dd = dd.clamp(DVec3::splat(-limit), DVec3::splat(limit));
                        *dv = dd;
                        ops::apply(&mut self.doc, originals, DMat4::from_translation(dd), false);
                    }
                }
            }
            Drag::Lift { originals, origin, start_s, delta: dz } => {
                let (o, d) = self.ray(v, p);
                if let Some(s) = line_param(*origin, DVec3::Z, o, d) {
                    *dz = ops::snap(s - *start_s, snap);
                    ops::apply(&mut self.doc, originals, DMat4::from_translation(DVec3::Z * *dz), false);
                }
            }
            Drag::Scale { originals, kind, lo, hi, new_size } => {
                let (o, d) = self.ray(v, p);
                let size = *hi - *lo;
                let min = if snap > 0.0 { snap } else { 0.1 };
                let fit = |raw: f64| ops::snap(raw, snap).max(min);
                let mut s = DVec3::ONE;
                let mut anchor = *lo;
                match *kind {
                    HandleKind::Corner(sx, sy) => {
                        if let Some(h) = ray_plane(o, d, *lo, DVec3::Z) {
                            anchor.x = if sx > 0 { lo.x } else { hi.x };
                            anchor.y = if sy > 0 { lo.y } else { hi.y };
                            let nx = fit((h.x - anchor.x) * sx as f64);
                            let ny = fit((h.y - anchor.y) * sy as f64);
                            s.x = nx / size.x.max(1e-9);
                            s.y = ny / size.y.max(1e-9);
                            if mods.shift {
                                let k = s.x.max(s.y);
                                s = DVec3::splat(k);
                            }
                        }
                    }
                    HandleKind::Side(axis, sign) => {
                        if let Some(h) = ray_plane(o, d, *lo, DVec3::Z) {
                            anchor[axis] = if sign > 0 { lo[axis] } else { hi[axis] };
                            anchor[1 - axis] = (lo[1 - axis] + hi[1 - axis]) * 0.5;
                            let n = fit((h[axis] - anchor[axis]) * sign as f64);
                            s[axis] = n / size[axis].max(1e-9);
                            if mods.shift {
                                s = DVec3::splat(s[axis]);
                            }
                        }
                    }
                    HandleKind::Top => {
                        let c = (*lo + *hi) * 0.5;
                        anchor = DVec3::new(c.x, c.y, lo.z);
                        if let Some(t) = line_param(anchor, DVec3::Z, o, d) {
                            s.z = fit(t) / size.z.max(1e-9);
                            if mods.shift {
                                s = DVec3::splat(s.z);
                            }
                        }
                    }
                    _ => {}
                }
                // Anchor uniform scales at the bottom so things stay on the plane.
                if mods.shift {
                    anchor.z = lo.z;
                }
                *new_size = size * s;
                ops::apply(&mut self.doc, originals, ops::scale_about(anchor, s), true);
            }
            Drag::Rotate { originals, axis, center, last, total, applied, .. } => {
                let (n, _, _) = axis_basis(*axis);
                let ring_c = match *axis {
                    2 => {
                        let b = ops::bounds_of(&mut self.cache, &self.doc, &self.selection);
                        DVec3::new(center.x, center.y, b.map(|b| b.0.z).unwrap_or(center.z))
                    }
                    _ => *center,
                };
                if let Some(a) = self.ring_angle(v, p, *axis, ring_c) {
                    *total += wrap_angle(a - *last);
                    *last = a;
                    let step = if mods.command {
                        1.0
                    } else if mods.shift {
                        45.0
                    } else {
                        22.5
                    };
                    let snapped = ops::snap(total.to_degrees(), step).to_radians();
                    if (snapped - *applied).abs() > 1e-9 {
                        *applied = snapped;
                        ops::apply(&mut self.doc, originals, ops::rotate_about(*center, n, snapped), true);
                    }
                }
            }
        }
        self.drag = Some(drag);
    }

    fn end_drag(&mut self, v: &View, pointer: Option<Pos2>, shift: bool) {
        let Some(drag) = self.drag.take() else { return };
        match drag {
            Drag::Marquee { start, additive } => {
                let end = pointer.unwrap_or(start);
                if start.distance(end) < 3.0 {
                    if !additive {
                        self.selection.clear();
                    }
                    return;
                }
                let r = Rect::from_two_pos(start, end);
                if !additive {
                    self.selection.clear();
                }
                for i in 0..self.doc.nodes.len() {
                    let n = self.doc.nodes[i].clone();
                    let (lo, hi) = ops::node_bounds(&mut self.cache, &n);
                    let mut inside = true;
                    for k in 0..8 {
                        let c = DVec3::new(
                            if k & 1 == 0 { lo.x } else { hi.x },
                            if k & 2 == 0 { lo.y } else { hi.y },
                            if k & 4 == 0 { lo.z } else { hi.z },
                        );
                        if !self.project(v, c).is_some_and(|s| r.contains(s)) {
                            inside = false;
                            break;
                        }
                    }
                    if inside && !self.selection.contains(&n.id) {
                        self.selection.push(n.id);
                    }
                }
            }
            Drag::Move { moved: false, clicked: Some(id), .. } if !shift => {
                // Plain click on an already-selected object selects just it.
                self.selection = vec![id];
            }
            Drag::Rotate { .. } => {
                // Rotation can push parts below the plane; keep the bottom
                // where it was, like Tinkercad does.
            }
            _ => {}
        }
    }

    fn render_scene(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame, rect: Rect) {
        let Some(rs) = frame.wgpu_render_state() else { return };
        let ppp = ui.ctx().pixels_per_point();
        let size = [(rect.width() * ppp).round() as u32, (rect.height() * ppp).round() as u32];
        if size[0] < 2 || size[1] < 2 {
            return;
        }

        let mut items = Vec::with_capacity(self.doc.nodes.len());
        for i in 0..self.doc.nodes.len() {
            let n = &self.doc.nodes[i];
            let model = n.transform.0.as_mat4();
            let c = egui::Rgba::from(Color32::from_rgb(n.color.0[0], n.color.0[1], n.color.0[2]));
            let (hole, id) = (n.hole, n.id);
            let (key, mesh) = self.cache.content_mesh(&self.doc.nodes[i].content);
            items.push(DrawItem {
                key,
                mesh,
                model,
                color: [c.r(), c.g(), c.b(), 1.0],
                hole,
                selected: self.selection.contains(&id),
                hovered: self.hovered == Some(id),
            });
        }

        let bg = egui::Rgba::from(Color32::from_rgb(0xf3, 0xf5, 0xf8));
        let aspect = rect.width() / rect.height();
        let wp = self.doc.settings.workplane;
        let renderer = self.renderer.as_mut().unwrap();
        if self.view_target.is_none() {
            self.view_target = Some(renderer.create_target(&rs.device, size));
        }
        let tex = renderer.render(
            rs,
            self.view_target.as_mut().unwrap(),
            size,
            &SceneParams {
                view_proj: self.camera.view_proj(aspect),
                eye: self.camera.eye(),
                items: &items,
                workplane: Some([wp[0] as f32, wp[1] as f32]),
                clear: [bg.r() as f64, bg.g() as f64, bg.b() as f64, 1.0],
                extra_lines: &[],
                fine_grid: self.camera.dist < 400.0,
                grid_fade: self.camera.dist * 0.9,
            },
        );
        ui.painter().image(
            tex,
            rect,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    }

    fn draw_bounds(&self, painter: &egui::Painter, v: &View, lo: DVec3, hi: DVec3) {
        let corners: Vec<Option<Pos2>> = (0..8)
            .map(|k| {
                self.project(
                    v,
                    DVec3::new(
                        if k & 1 == 0 { lo.x } else { hi.x },
                        if k & 2 == 0 { lo.y } else { hi.y },
                        if k & 4 == 0 { lo.z } else { hi.z },
                    ),
                )
            })
            .collect();
        let edges = [
            (0, 1), (1, 3), (3, 2), (2, 0),
            (4, 5), (5, 7), (7, 6), (6, 4),
            (0, 4), (1, 5), (2, 6), (3, 7),
        ];
        let stroke = Stroke::new(1.0, Color32::from_rgba_unmultiplied(40, 60, 90, 150));
        for (a, b) in edges {
            if let (Some(pa), Some(pb)) = (corners[a], corners[b]) {
                painter.extend(egui::Shape::dashed_line(&[pa, pb], stroke, 5.0, 4.0));
            }
        }
    }

    fn draw_drag_feedback(&self, painter: &egui::Painter, v: &View, lo: DVec3, hi: DVec3) {
        let label = |painter: &egui::Painter, at: Pos2, text: String| {
            let font = FontId::proportional(13.0);
            let galley = painter.layout_no_wrap(text, font, Color32::WHITE);
            let r = Rect::from_center_size(at, galley.size() + vec2(12.0, 6.0));
            painter.rect_filled(r, 4.0, Color32::from_rgba_unmultiplied(20, 30, 45, 220));
            painter.galley(r.min + vec2(6.0, 3.0), galley, Color32::WHITE);
        };
        let size = hi - lo;
        match &self.drag {
            Some(Drag::Scale { .. }) => {
                if let Some(p) = self.project(v, DVec3::new(hi.x, (lo.y + hi.y) * 0.5, lo.z)) {
                    label(painter, p + vec2(0.0, 26.0), format!("{:.2} x {:.2} x {:.2} mm", size.x, size.y, size.z));
                }
            }
            Some(Drag::Move { moved: true, delta, .. }) => {
                if let Some(p) = self.project(v, DVec3::new((lo.x + hi.x) * 0.5, (lo.y + hi.y) * 0.5, lo.z)) {
                    label(painter, p + vec2(0.0, 28.0), format!("X {:+.2}  Y {:+.2}", delta.x, delta.y));
                }
            }
            Some(Drag::Lift { delta, .. }) => {
                if let Some(p) = self.project(v, DVec3::new((lo.x + hi.x) * 0.5, (lo.y + hi.y) * 0.5, hi.z)) {
                    label(painter, p + vec2(40.0, -20.0), format!("Z {:+.2}  (bottom {:.2})", delta, lo.z));
                }
            }
            Some(Drag::Rotate { axis, center, radius, applied, .. }) => {
                let (_, u, w) = axis_basis(*axis);
                let c = if *axis == 2 { DVec3::new(center.x, center.y, lo.z) } else { *center };
                let pts: Vec<Pos2> = (0..=72)
                    .filter_map(|i| {
                        let a = i as f64 / 72.0 * std::f64::consts::TAU;
                        self.project(v, c + (u * a.cos() + w * a.sin()) * *radius)
                    })
                    .collect();
                painter.add(egui::Shape::line(pts, Stroke::new(1.5, ACCENT)));
                if let Some(p) = self.project(v, c) {
                    label(painter, p, format!("{:.1} deg", applied.to_degrees()));
                }
            }
            _ => {}
        }
    }

    fn view_controls(&mut self, ui: &mut egui::Ui, rect: Rect) {
        egui::Area::new(egui::Id::unique("view_controls"))
            .fixed_pos(rect.left_top() + vec2(12.0, 12.0))
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).inner_margin(6).show(ui, |ui| {
                    ui.vertical(|ui| {
                        let b = |ui: &mut egui::Ui, t: &str, tip: &str| {
                            ui.add_sized([62.0, 24.0], egui::Button::new(t)).on_hover_text(tip).clicked()
                        };
                        if b(ui, "Home", "Default view") {
                            self.camera = crate::render::camera::Camera::home();
                        }
                        if b(ui, "Fit", "F - fit selection (or everything)") {
                            self.fit_all();
                        }
                        ui.separator();
                        let views = [
                            ("Top", -90f32, 89.9f32),
                            ("Front", -90.0, 0.0),
                            ("Right", 0.0, 0.0),
                            ("Back", 90.0, 0.0),
                            ("Left", 180.0, 0.0),
                        ];
                        for (name, yaw, pitch) in views {
                            if b(ui, name, "Look from this side") {
                                self.camera.yaw = yaw.to_radians();
                                self.camera.pitch = pitch.to_radians();
                            }
                        }
                    });
                });
            });
    }
}

fn draw_handle(p: &egui::Painter, h: &Handle, hot: bool) {
    let fill_hot = ACCENT;
    match h.kind {
        HandleKind::Corner(..) | HandleKind::Top => {
            let r = Rect::from_center_size(h.pos, Vec2::splat(HANDLE_R * 2.0));
            p.rect_filled(r, 1.0, if hot { fill_hot } else { Color32::WHITE });
            p.rect_stroke(r, 1.0, Stroke::new(1.2, Color32::from_gray(40)), egui::StrokeKind::Middle);
        }
        HandleKind::Side(..) => {
            let r = Rect::from_center_size(h.pos, Vec2::splat(HANDLE_R * 1.5));
            p.rect_filled(r, 1.0, if hot { fill_hot } else { Color32::from_gray(30) });
        }
        HandleKind::Lift => {
            // Small upward-pointing cone.
            let c = h.pos;
            let pts = vec![c + vec2(0.0, -9.0), c + vec2(7.0, 6.0), c + vec2(-7.0, 6.0)];
            p.add(egui::Shape::convex_polygon(pts, if hot { fill_hot } else { Color32::from_gray(25) }, Stroke::NONE));
        }
        HandleKind::Rotate(_) => {
            let col = if hot { fill_hot } else { Color32::from_gray(45) };
            let w = if hot { 3.0 } else { 2.2 };
            p.add(egui::Shape::line(h.arc.clone(), Stroke::new(w, col)));
            // Arrow heads at both ends.
            for (tip, prev) in [(h.arc[0], h.arc[1]), (h.arc[h.arc.len() - 1], h.arc[h.arc.len() - 2])] {
                let dir = (tip - prev).normalized();
                let side = vec2(-dir.y, dir.x);
                let pts = vec![tip + dir * 6.0, tip - dir * 2.0 + side * 5.0, tip - dir * 2.0 - side * 5.0];
                p.add(egui::Shape::convex_polygon(pts, col, Stroke::NONE));
            }
        }
    }
}
