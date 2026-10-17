//! 3D viewport: rendering, picking, Tinkercad-style handles, dragging and
//! the Align / Mirror / Measure tools.

use crate::app::OrionApp;
use crate::doc::{NodeId, ShapeKind};
use crate::geom::{self, Item};
use crate::icons;
use crate::measure::{self, Feature};
use crate::ops;
use crate::render::{DrawItem, SceneParams};
use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2, pos2, vec2};
use glam::{DMat4, DVec3, Vec3};
use std::collections::HashMap;
use std::sync::Arc;

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
    /// Rotation arrow around an axis.
    Rotate(usize),
    /// Align tool dot: axis, 0 = min / 1 = centre / 2 = max.
    AlignDot(usize, usize),
    /// Mirror tool arrow for an axis.
    MirrorArrow(usize),
}

pub struct Handle {
    kind: HandleKind,
    pos: Pos2,
    arc: Vec<Pos2>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Tool {
    Select,
    Align { key: Option<NodeId> },
    Mirror,
    Measure,
}

#[derive(Default)]
pub struct MeasureState {
    pub first: Option<Feature>,
    pub second: Option<Feature>,
    hover: Option<Feature>,
    cache: HashMap<(u64, [u64; 16]), Arc<Vec<Feature>>>,
}

impl MeasureState {
    pub fn clear(&mut self) {
        self.first = None;
        self.second = None;
        self.hover = None;
    }
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
    Scale { originals: ops::Originals, kind: HandleKind, lo: DVec3, hi: DVec3 },
    Rotate {
        originals: ops::Originals,
        axis: usize,
        /// Pivot (centre of the selection).
        pivot: DVec3,
        /// Centre of the plane the arrow sits in.
        ring: DVec3,
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

struct Hit {
    node: NodeId,
    point: DVec3,
    normal: DVec3,
    t: f64,
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
    if pts.len() == 1 {
        return pts[0].distance(p);
    }
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
pub const ACCENT: Color32 = Color32::from_rgb(0x2b, 0x7c, 0xe9);
const MEASURE: Color32 = Color32::from_rgb(0xf2, 0x7a, 0x12);
const ARROW_GREY: Color32 = Color32::from_rgb(0x6b, 0x70, 0x78);

impl OrionApp {
    fn project(&self, v: &View, p: DVec3) -> Option<Pos2> {
        self.camera
            .project(p.as_vec3(), v.aspect)
            .map(|uv| pos2(v.rect.min.x + uv[0] * v.rect.width(), v.rect.min.y + uv[1] * v.rect.height()))
    }

    fn ray(&self, v: &View, p: Pos2) -> (DVec3, DVec3) {
        let (o, d) = self.camera.ray(v.uv(p), v.aspect);
        (o.as_dvec3(), d.as_dvec3())
    }

    /// Drawable items of all visible top-level nodes.
    fn visible_items(&mut self) -> Vec<Item> {
        let mut items = Vec::new();
        for i in 0..self.doc.nodes.len() {
            if self.doc.nodes[i].hidden {
                continue;
            }
            let n = &self.doc.nodes[i];
            self.cache.node_items(n.id, n, DMat4::IDENTITY, false, &mut items);
        }
        items
    }

    /// Closest visible surface under the pointer.
    fn pick_items(&self, items: &[Item], v: &View, p: Pos2) -> Option<Hit> {
        let (o, d) = self.ray(v, p);
        let mut best: Option<Hit> = None;
        for it in items {
            let inv = it.model.inverse();
            let lo = inv.transform_point3(o).as_vec3();
            let ld = inv.transform_vector3(d).as_vec3();
            if let Some((t, tri)) = geom::ray_hit(&it.mesh, lo, ld) {
                let t = t as f64;
                if best.as_ref().is_none_or(|b| t < b.t) {
                    let tp = &it.mesh.positions[tri * 3..tri * 3 + 3];
                    let ln = (tp[1] - tp[0]).cross(tp[2] - tp[0]).as_dvec3();
                    let normal = inv.transpose().transform_vector3(ln).normalize_or_zero();
                    best = Some(Hit { node: it.node, point: o + d * t, normal, t });
                }
            }
        }
        best
    }

    fn handle_gap(&self) -> f64 {
        self.camera.dist as f64 * 0.045
    }

    fn selection_locked(&self) -> bool {
        self.selection.iter().any(|id| self.doc.get(*id).is_some_and(|n| n.locked))
    }

    /// For an axis' rotation arrow: the centre of rotation projected into
    /// the arrow's plane, and the arrow's anchor point. The arrows sit at
    /// fixed places around the selection (like Tinkercad), so orbiting
    /// changes only the angle you see them from.
    fn ring(&self, axis: usize, lo: DVec3, hi: DVec3) -> (DVec3, f64) {
        let (center, anchor) = self.arrow_anchor(axis, lo, hi);
        (center, anchor.distance(center))
    }

    fn arrow_anchor(&self, axis: usize, lo: DVec3, hi: DVec3) -> (DVec3, DVec3) {
        let c = (lo + hi) * 0.5;
        let g = self.handle_gap();
        match axis {
            0 => (DVec3::new(lo.x - g * 0.6, c.y, c.z), DVec3::new(lo.x - g * 0.6, lo.y - g * 0.2, hi.z + g * 0.9)),
            1 => (DVec3::new(c.x, lo.y, c.z), DVec3::new(hi.x + g * 0.4, lo.y, hi.z + g * 0.8)),
            _ => (DVec3::new(c.x, c.y, lo.z), DVec3::new(c.x, lo.y - g * 1.3, lo.z)),
        }
    }

    /// Curved double-arrow glyph points (screen space) for an axis.
    fn arrow_glyph(&self, v: &View, axis: usize, lo: DVec3, hi: DVec3) -> Vec<Pos2> {
        let (center, anchor) = self.arrow_anchor(axis, lo, hi);
        let (n, _, _) = axis_basis(axis);
        let r = (anchor - center).normalize_or(DVec3::X);
        let t = n.cross(r);
        let eye = self.camera.eye().as_dvec3();
        let wpp = 2.0 * eye.distance(anchor) * (self.camera.fov_y as f64 * 0.5).tan() / v.rect.height().max(1.0) as f64;
        let rg = 15.0 * wpp;
        let q = anchor - r * rg;
        (0..=14)
            .filter_map(|i| {
                let th = (-62.0 + 124.0 * i as f64 / 14.0f64).to_radians();
                self.project(v, q + (r * th.cos() + t * th.sin()) * rg)
            })
            .collect()
    }

    fn compute_handles(&self, v: &View, lo: DVec3, hi: DVec3) -> Vec<Handle> {
        let mut out = Vec::new();
        let c = (lo + hi) * 0.5;
        let mut add = |kind, p: DVec3| {
            if let Some(pos) = self.project(v, p) {
                out.push(Handle { kind, pos, arc: Vec::new() });
            }
        };
        let g = self.handle_gap();
        match &self.tool {
            Tool::Select | Tool::Measure => {
                if matches!(self.tool, Tool::Measure) || self.selection_locked() {
                    return out;
                }
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
                add(HandleKind::Lift, DVec3::new(c.x, c.y, hi.z + g * 1.4));
                for axis in 0..3 {
                    let pts = self.arrow_glyph(v, axis, lo, hi);
                    if pts.len() > 2 {
                        out.push(Handle { kind: HandleKind::Rotate(axis), pos: pts[pts.len() / 2], arc: pts });
                    }
                }
            }
            Tool::Align { .. } => {
                for k in 0..3 {
                    let x = [lo.x, c.x, hi.x][k];
                    add(HandleKind::AlignDot(0, k), DVec3::new(x, lo.y - g * 1.6, lo.z));
                    let y = [lo.y, c.y, hi.y][k];
                    add(HandleKind::AlignDot(1, k), DVec3::new(hi.x + g * 1.6, y, lo.z));
                    let z = [lo.z, c.z, hi.z][k];
                    add(HandleKind::AlignDot(2, k), DVec3::new(lo.x - g * 1.2, lo.y - g * 1.2, z));
                }
            }
            Tool::Mirror => {
                let spots = [
                    DVec3::new(hi.x + g * 1.6, c.y, lo.z),
                    DVec3::new(c.x, lo.y - g * 1.6, lo.z),
                    DVec3::new(c.x, c.y, hi.z + g * 1.6),
                ];
                for (axis, at) in spots.into_iter().enumerate() {
                    let mut dir = DVec3::ZERO;
                    dir[axis] = g * 0.6;
                    if let (Some(pos), Some(a), Some(b)) =
                        (self.project(v, at), self.project(v, at - dir), self.project(v, at + dir))
                    {
                        out.push(Handle { kind: HandleKind::MirrorArrow(axis), pos, arc: vec![a, b] });
                    }
                }
            }
        }
        out
    }

    fn hit_handle(handles: &[Handle], p: Pos2) -> Option<HandleKind> {
        for h in handles {
            if !matches!(h.kind, HandleKind::Rotate(_)) && h.pos.distance(p) <= HANDLE_R + 5.0 {
                return Some(h.kind);
            }
        }
        handles
            .iter()
            .filter(|h| matches!(h.kind, HandleKind::Rotate(_)))
            .map(|h| (dist_to_polyline(p, &h.arc), h.kind))
            .filter(|(d, _)| *d < 9.0)
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

        // Leave tools that no longer apply.
        match &self.tool {
            Tool::Align { .. } if self.selection.len() < 2 => self.tool = Tool::Select,
            Tool::Mirror if self.selection.is_empty() => self.tool = Tool::Select,
            _ => {}
        }

        let mut items = self.visible_items();
        let mut bounds = ops::bounds_of(&mut self.cache, &self.doc, &self.selection);
        let mut handles = bounds.map(|(lo, hi)| self.compute_handles(&v, lo, hi)).unwrap_or_default();

        // ------------------------------------------------------------------
        // Input
        // ------------------------------------------------------------------
        let (pressed, released, primary_down, secondary_down, middle_down, mods) = ctx.input(|i| {
            (
                i.pointer.any_pressed(),
                i.pointer.any_released(),
                i.pointer.primary_down(),
                i.pointer.secondary_down(),
                i.pointer.middle_down(),
                i.modifiers,
            )
        });
        let hovered_handle = if response.hovered() { pointer.and_then(|p| Self::hit_handle(&handles, p)) } else { None };

        // Drop from the shape library.
        if let Some(entry) = response.dnd_release_payload::<(ShapeKind, bool)>() {
            if let Some(p) = pointer {
                let at = self.drop_point(&items, &v, p);
                self.add_shape(entry.0, entry.1, Some(at));
                items = self.visible_items();
            }
        }

        if pressed && response.hovered() && self.drag.is_none() {
            if let Some(p) = pointer {
                if (secondary_down && mods.shift) || middle_down || (primary_down && mods.command && mods.shift) {
                    self.drag = Some(Drag::Pan);
                } else if secondary_down || (primary_down && mods.command) {
                    self.drag = Some(Drag::Orbit);
                } else if primary_down {
                    match self.tool.clone() {
                        Tool::Measure => self.measure_click(&items, &v, p),
                        Tool::Align { .. } | Tool::Mirror => self.tool_click(&items, &v, p, hovered_handle),
                        Tool::Select => self.begin_primary(&items, &v, p, hovered_handle, bounds, mods),
                    }
                }
            }
        }

        // Wheel zoom towards the cursor.
        if response.hovered() {
            let scroll = ctx.input(|i| i.smooth_scroll_delta.y);
            if scroll.abs() > 0.0 {
                let towards = pointer.map(|p| self.zoom_point(&items, &v, p));
                self.camera.zoom((-scroll * 0.0025).exp(), towards);
            }
        }

        let delta = ctx.input(|i| i.pointer.delta());
        if let Some(p) = pointer {
            self.update_drag(&v, p, delta, mods, snap);
        }

        let any_down = primary_down || secondary_down || middle_down;
        if self.drag.is_some() && (!any_down || released && !any_down) {
            self.end_drag(&items, &v, pointer, mods.shift);
        }

        // Recompute after edits so drawing reflects this frame.
        if self.drag.is_some() || pressed || released {
            items = self.visible_items();
            bounds = ops::bounds_of(&mut self.cache, &self.doc, &self.selection);
            handles = bounds.map(|(lo, hi)| self.compute_handles(&v, lo, hi)).unwrap_or_default();
        }

        // Hover.
        let can_hover = self.drag.is_none() && response.hovered() && hovered_handle.is_none();
        self.hovered = if can_hover && self.tool != Tool::Measure {
            pointer.and_then(|p| self.pick_items(&items, &v, p)).map(|h| h.node)
        } else {
            None
        };
        if self.tool == Tool::Measure {
            self.measure.hover = if can_hover { pointer.and_then(|p| self.measure_pick(&items, &v, p)) } else { None };
        }

        self.set_cursor(&ctx, hovered_handle);

        // ------------------------------------------------------------------
        // Render
        // ------------------------------------------------------------------
        self.render_scene(ui, frame, rect, &items, bounds);

        // ------------------------------------------------------------------
        // Overlays
        // ------------------------------------------------------------------
        let painter = ui.painter_at(rect);
        if let Some((lo, hi)) = bounds {
            if self.tool != Tool::Measure {
                self.draw_selection_frame(&painter, &v, lo, hi);
            }
            let rotating = matches!(self.drag, Some(Drag::Rotate { .. }));
            if !rotating {
                for h in &handles {
                    draw_handle(&painter, h, hovered_handle == Some(h.kind));
                }
            }
            if let Tool::Align { .. } = self.tool {
                self.draw_align_guides(&painter, &v, lo, hi, hovered_handle);
            }
            self.draw_drag_feedback(&painter, &v, lo, hi);
        }
        if self.tool == Tool::Measure {
            self.draw_measure(&painter, &v);
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
        self.tool_banner(ui, rect);

        self.view_controls(ui, rect);
        if self.tool != Tool::Measure {
            self.inspector(ui, rect);
        }

        if self.drag.is_some() {
            ctx.request_repaint();
        }
    }

    fn set_cursor(&self, ctx: &egui::Context, hovered_handle: Option<HandleKind>) {
        let active = hovered_handle.or(match &self.drag {
            Some(Drag::Scale { kind, .. }) => Some(*kind),
            Some(Drag::Rotate { axis, .. }) => Some(HandleKind::Rotate(*axis)),
            Some(Drag::Lift { .. }) => Some(HandleKind::Lift),
            _ => None,
        });
        let icon = match active {
            Some(HandleKind::Lift | HandleKind::Top) => egui::CursorIcon::ResizeVertical,
            Some(HandleKind::Rotate(_)) => egui::CursorIcon::Grab,
            Some(HandleKind::AlignDot(..) | HandleKind::MirrorArrow(_)) => egui::CursorIcon::PointingHand,
            Some(_) => egui::CursorIcon::Crosshair,
            None => match &self.drag {
                Some(Drag::Move { moved: true, .. }) => egui::CursorIcon::Grabbing,
                Some(Drag::Orbit) => egui::CursorIcon::AllScroll,
                Some(Drag::Pan) => egui::CursorIcon::Move,
                _ if self.tool == Tool::Measure => egui::CursorIcon::Crosshair,
                _ if self.hovered.is_some() => egui::CursorIcon::PointingHand,
                _ => return,
            },
        };
        ctx.set_cursor_icon(icon);
    }

    fn zoom_point(&self, items: &[Item], v: &View, p: Pos2) -> Vec3 {
        if let Some(h) = self.pick_items(items, v, p) {
            return h.point.as_vec3();
        }
        let (o, d) = self.ray(v, p);
        if let Some(h) = ray_plane(o, d, DVec3::ZERO, DVec3::Z) {
            if h.distance(o) < self.camera.dist as f64 * 6.0 {
                return h.as_vec3();
            }
        }
        self.camera.target
    }

    fn drop_point(&self, items: &[Item], v: &View, p: Pos2) -> DVec3 {
        if let Some(h) = self.pick_items(items, v, p) {
            return h.point;
        }
        let (o, d) = self.ray(v, p);
        ray_plane(o, d, DVec3::ZERO, DVec3::Z).unwrap_or_else(|| {
            let t = self.camera.target.as_dvec3();
            DVec3::new(t.x, t.y, 0.0)
        })
    }

    // ----------------------------------------------------------------------
    // Select tool
    // ----------------------------------------------------------------------

    fn begin_primary(
        &mut self,
        items: &[Item],
        v: &View,
        p: Pos2,
        handle: Option<HandleKind>,
        bounds: Option<(DVec3, DVec3)>,
        mods: egui::Modifiers,
    ) {
        if let (Some(kind), Some((lo, hi))) = (handle, bounds) {
            let originals = ops::originals(&self.doc, &self.selection);
            let (o, d) = self.ray(v, p);
            match kind {
                HandleKind::Lift => {
                    let c = (lo + hi) * 0.5;
                    let origin = DVec3::new(c.x, c.y, hi.z);
                    let start_s = line_param(origin, DVec3::Z, o, d).unwrap_or(0.0);
                    self.drag = Some(Drag::Lift { originals, origin, start_s, delta: 0.0 });
                }
                HandleKind::Rotate(axis) => {
                    let (ring, radius) = self.ring(axis, lo, hi);
                    let start = self.ring_angle(v, p, axis, ring).unwrap_or(0.0);
                    if mods.alt {
                        self.duplicate();
                    }
                    let originals = ops::originals(&self.doc, &self.selection);
                    self.drag = Some(Drag::Rotate {
                        originals,
                        axis,
                        pivot: (lo + hi) * 0.5,
                        ring,
                        radius,
                        last: start,
                        total: 0.0,
                        applied: 0.0,
                    });
                }
                _ => self.drag = Some(Drag::Scale { originals, kind, lo, hi }),
            }
            return;
        }

        if let Some(hit) = self.pick_items(items, v, p) {
            let id = hit.node;
            let mut clicked = None;
            if mods.shift {
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
            if self.selection_locked() {
                return;
            }
            // Alt-drag drags a copy, like Tinkercad.
            if mods.alt {
                self.duplicate();
                clicked = None;
            }
            self.drag = Some(Drag::Move {
                originals: ops::originals(&self.doc, &self.selection),
                plane_z: hit.point.z,
                start_hit: hit.point,
                press: p,
                moved: false,
                clicked,
                delta: DVec3::ZERO,
            });
        } else {
            self.drag = Some(Drag::Marquee { start: p, additive: mods.shift });
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
        // Plane seen edge-on: fall back to the screen-space angle.
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
            Drag::Scale { originals, kind, lo, hi } => {
                let (o, d) = self.ray(v, p);
                let size = *hi - *lo;
                let center = (*lo + *hi) * 0.5;
                let min = if snap > 0.0 { snap } else { 0.1 };
                // Alt scales about the centre: the dragged side moves twice
                // as far relative to the size.
                let k = if mods.alt { 2.0 } else { 1.0 };
                let fit = |raw: f64| ops::snap(raw, snap).max(min);
                let mut s = DVec3::ONE;
                let mut anchor = *lo;
                match *kind {
                    HandleKind::Corner(sx, sy) => {
                        if let Some(h) = ray_plane(o, d, *lo, DVec3::Z) {
                            anchor.x = if sx > 0 { lo.x } else { hi.x };
                            anchor.y = if sy > 0 { lo.y } else { hi.y };
                            let ax = if mods.alt { center.x } else { anchor.x };
                            let ay = if mods.alt { center.y } else { anchor.y };
                            s.x = fit((h.x - ax) * sx as f64 * k) / size.x.max(1e-9);
                            s.y = fit((h.y - ay) * sy as f64 * k) / size.y.max(1e-9);
                            if mods.shift {
                                s = DVec3::splat(s.x.max(s.y));
                            }
                        }
                    }
                    HandleKind::Side(axis, sign) => {
                        if let Some(h) = ray_plane(o, d, *lo, DVec3::Z) {
                            anchor[axis] = if sign > 0 { lo[axis] } else { hi[axis] };
                            anchor[1 - axis] = center[1 - axis];
                            let a = if mods.alt { center[axis] } else { anchor[axis] };
                            s[axis] = fit((h[axis] - a) * sign as f64 * k) / size[axis].max(1e-9);
                            if mods.shift {
                                s = DVec3::splat(s[axis]);
                            }
                        }
                    }
                    HandleKind::Top => {
                        anchor = DVec3::new(center.x, center.y, lo.z);
                        let base = if mods.alt { center } else { anchor };
                        if let Some(t) = line_param(base, DVec3::Z, o, d) {
                            s.z = fit(t * k) / size.z.max(1e-9);
                            if mods.shift {
                                s = DVec3::splat(s.z);
                            }
                        }
                    }
                    _ => {}
                }
                if mods.alt {
                    anchor = center;
                } else if mods.shift {
                    // Uniform scale keeps the object on the plane.
                    anchor.z = lo.z;
                }
                ops::apply(&mut self.doc, originals, ops::scale_about(anchor, s), true);
            }
            Drag::Rotate { originals, axis, pivot, ring, last, total, applied, .. } => {
                let (n, _, _) = axis_basis(*axis);
                if let Some(a) = self.ring_angle(v, p, *axis, *ring) {
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
                        ops::apply(&mut self.doc, originals, ops::rotate_about(*pivot, n, snapped), true);
                    }
                }
            }
        }
        self.drag = Some(drag);
    }

    fn end_drag(&mut self, items: &[Item], v: &View, pointer: Option<Pos2>, shift: bool) {
        let Some(drag) = self.drag.take() else { return };
        match drag {
            Drag::Marquee { start, additive } => {
                let end = pointer.unwrap_or(start);
                if start.distance(end) < 3.0 {
                    if !additive {
                        self.selection.clear();
                        if matches!(self.tool, Tool::Align { .. } | Tool::Mirror) {
                            self.tool = Tool::Select;
                        }
                    }
                    return;
                }
                let r = Rect::from_two_pos(start, end);
                if !additive {
                    self.selection.clear();
                }
                // Select objects whose whole bounding box is inside.
                let mut per_node: HashMap<NodeId, (DVec3, DVec3)> = HashMap::new();
                for it in items {
                    if let Some((lo, hi)) = geom::world_bounds(&it.mesh, &it.model) {
                        per_node
                            .entry(it.node)
                            .and_modify(|b| {
                                b.0 = b.0.min(lo);
                                b.1 = b.1.max(hi);
                            })
                            .or_insert((lo, hi));
                    }
                }
                for n in &self.doc.nodes {
                    let Some((lo, hi)) = per_node.get(&n.id) else { continue };
                    let inside = (0..8).all(|k| {
                        let c = DVec3::new(
                            if k & 1 == 0 { lo.x } else { hi.x },
                            if k & 2 == 0 { lo.y } else { hi.y },
                            if k & 4 == 0 { lo.z } else { hi.z },
                        );
                        self.project(v, c).is_some_and(|s| r.contains(s))
                    });
                    if inside && !self.selection.contains(&n.id) {
                        self.selection.push(n.id);
                    }
                }
            }
            Drag::Move { moved: false, clicked: Some(id), .. } if !shift => {
                // Plain click on an already-selected object selects just it.
                self.selection = vec![id];
            }
            _ => {}
        }
    }

    // ----------------------------------------------------------------------
    // Align and Mirror tools
    // ----------------------------------------------------------------------

    fn tool_click(&mut self, items: &[Item], v: &View, p: Pos2, handle: Option<HandleKind>) {
        match handle {
            Some(HandleKind::AlignDot(axis, which)) => {
                self.align(axis, which);
                return;
            }
            Some(HandleKind::MirrorArrow(axis)) => {
                self.mirror(axis);
                return;
            }
            _ => {}
        }
        match self.pick_items(items, v, p) {
            Some(h) if self.selection.contains(&h.node) => {
                if let Tool::Align { key } = &mut self.tool {
                    // Clicking a selected object makes it the one the others
                    // align to (click again to clear).
                    *key = if *key == Some(h.node) { None } else { Some(h.node) };
                }
            }
            _ => {
                self.tool = Tool::Select;
                self.drag = Some(Drag::Marquee { start: p, additive: false });
            }
        }
    }

    /// Align every selected object on `axis` to min / centre / max of the
    /// selection (or of the key object).
    pub fn align(&mut self, axis: usize, which: usize) {
        let key = if let Tool::Align { key } = self.tool { key } else { None };
        let reference = match key {
            Some(k) => ops::bounds_of(&mut self.cache, &self.doc, &[k]),
            None => ops::bounds_of(&mut self.cache, &self.doc, &self.selection),
        };
        let Some((rlo, rhi)) = reference else { return };
        let target = [rlo[axis], (rlo[axis] + rhi[axis]) * 0.5, rhi[axis]][which];
        for id in self.selection.clone() {
            if Some(id) == key {
                continue;
            }
            let Some(n) = self.doc.get(id).cloned() else { continue };
            if n.locked {
                continue;
            }
            let (lo, hi) = ops::node_bounds(&mut self.cache, &n);
            let cur = [lo[axis], (lo[axis] + hi[axis]) * 0.5, hi[axis]][which];
            let mut d = DVec3::ZERO;
            d[axis] = target - cur;
            if let Some(n) = self.doc.get_mut(id) {
                n.transform.0 = DMat4::from_translation(d) * n.transform.0;
            }
        }
    }

    fn draw_align_guides(&self, p: &egui::Painter, v: &View, lo: DVec3, hi: DVec3, hot: Option<HandleKind>) {
        let g = self.handle_gap();
        let c = (lo + hi) * 0.5;
        let stroke = Stroke::new(1.0, Color32::from_gray(120));
        let line = |a: DVec3, b: DVec3| {
            if let (Some(pa), Some(pb)) = (self.project(v, a), self.project(v, b)) {
                p.extend(egui::Shape::dashed_line(&[pa, pb], stroke, 4.0, 3.0));
            }
        };
        line(DVec3::new(lo.x, lo.y - g * 1.6, lo.z), DVec3::new(hi.x, lo.y - g * 1.6, lo.z));
        line(DVec3::new(hi.x + g * 1.6, lo.y, lo.z), DVec3::new(hi.x + g * 1.6, hi.y, lo.z));
        line(DVec3::new(lo.x - g * 1.2, lo.y - g * 1.2, lo.z), DVec3::new(lo.x - g * 1.2, lo.y - g * 1.2, hi.z));
        // Preview of the alignment plane for the hovered dot.
        if let Some(HandleKind::AlignDot(axis, which)) = hot {
            let val = [lo[axis], c[axis], hi[axis]][which];
            let (_, u, w) = axis_basis(axis);
            let mut q = c;
            q[axis] = val;
            let e = (hi - lo) * 0.5 + DVec3::splat(g * 0.5);
            let corners: Vec<Pos2> = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0), (-1.0, -1.0)]
                .iter()
                .filter_map(|(a, b)| self.project(v, q + u * a * e.dot(u).abs() + w * b * e.dot(w).abs()))
                .collect();
            if corners.len() == 5 {
                p.add(egui::Shape::line(corners, Stroke::new(1.5, ACCENT)));
            }
        }
        if let Tool::Align { key: Some(k) } = self.tool {
            if let Some(n) = self.doc.get(k) {
                let _ = n;
                if let Some(pos) = self.project(v, DVec3::new(c.x, c.y, hi.z)) {
                    p.text(pos + vec2(0.0, -30.0), Align2::CENTER_BOTTOM, "aligning to the highlighted object", FontId::proportional(12.0), Color32::from_gray(80));
                }
            }
        }
    }

    fn tool_banner(&self, ui: &mut egui::Ui, rect: Rect) {
        let text = match &self.tool {
            Tool::Select => return,
            Tool::Align { key: None } => "Align: click a dot. Click one of the selected objects first to align the others to it. Esc to finish.",
            Tool::Align { key: Some(_) } => "Align: aligning to the chosen object. Click a dot. Esc to finish.",
            Tool::Mirror => "Mirror: click an arrow to flip the selection along that axis. Esc to finish.",
            Tool::Measure => "Measure: click an edge, circle or face, then a second one. Right-drag to orbit. Esc to finish.",
        };
        let p = ui.painter_at(rect);
        let font = FontId::proportional(13.0);
        let galley = p.layout_no_wrap(text.to_string(), font, Color32::WHITE);
        let r = Rect::from_center_size(rect.center_bottom() - vec2(0.0, 26.0), galley.size() + vec2(20.0, 10.0));
        p.rect_filled(r, 6.0, Color32::from_rgba_unmultiplied(30, 42, 58, 230));
        p.galley(r.min + vec2(10.0, 5.0), galley, Color32::WHITE);
    }

    // ----------------------------------------------------------------------
    // Measure tool
    // ----------------------------------------------------------------------

    fn features_for(&mut self, it: &Item) -> Arc<Vec<Feature>> {
        let k = (it.key, it.model.to_cols_array().map(|f| f.to_bits()));
        if let Some(f) = self.measure.cache.get(&k) {
            return f.clone();
        }
        if self.measure.cache.len() > 256 {
            self.measure.cache.clear();
        }
        let f = Arc::new(measure::extract(&it.mesh, &it.model));
        self.measure.cache.insert(k, f.clone());
        f
    }

    /// Edge (or circle) near the cursor that is not hidden behind geometry,
    /// otherwise the flat face under the cursor.
    fn measure_pick(&mut self, items: &[Item], v: &View, p: Pos2) -> Option<Feature> {
        let hit = self.pick_items(items, v, p);
        let eye = self.camera.eye().as_dvec3();
        let tol = self.camera.dist as f64 * 0.01 + 0.05;
        let mut best: Option<(f32, Feature)> = None;
        for it in items {
            // Cheap reject: bounds must be near the cursor on screen.
            let Some((lo, hi)) = geom::world_bounds(&it.mesh, &it.model) else { continue };
            let corners: Vec<Pos2> = (0..8)
                .filter_map(|k| {
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
            if corners.is_empty() || !Rect::from_points(&corners).expand(10.0).contains(p) {
                continue;
            }
            for f in self.features_for(it).iter() {
                let pts3 = f.polyline();
                let pts: Vec<(Pos2, DVec3)> = pts3.iter().filter_map(|w| self.project(v, *w).map(|s| (s, *w))).collect();
                if pts.len() < 2 {
                    continue;
                }
                let screen: Vec<Pos2> = pts.iter().map(|x| x.0).collect();
                let d = dist_to_polyline(p, &screen);
                if d > 8.0 || best.as_ref().is_some_and(|b| b.0 <= d) {
                    continue;
                }
                // Visible? Compare the nearest feature point's depth with
                // the surface under the cursor.
                let near = pts.iter().min_by(|a, b| a.0.distance(p).total_cmp(&b.0.distance(p))).unwrap().1;
                if let Some(h) = &hit {
                    if near.distance(eye) > h.t + tol && h.point.distance(near) > tol {
                        continue;
                    }
                }
                best = Some((d, f.clone()));
            }
        }
        if let Some((_, f)) = best {
            return Some(f);
        }
        hit.map(|h| Feature::Face { point: h.point, normal: h.normal })
    }

    fn measure_click(&mut self, items: &[Item], v: &View, p: Pos2) {
        let f = self.measure_pick(items, v, p);
        match f {
            None => self.measure.clear(),
            Some(f) => {
                if self.measure.first.is_none() || self.measure.second.is_some() {
                    self.measure.first = Some(f);
                    self.measure.second = None;
                } else {
                    self.measure.second = Some(f);
                }
            }
        }
    }

    fn draw_feature(&self, p: &egui::Painter, v: &View, f: &Feature, col: Color32, width: f32) {
        match f {
            Feature::Face { point, normal } => {
                // A small square in the face plane.
                let n = normal.normalize_or_zero();
                let u = n.any_orthonormal_vector();
                let w = n.cross(u);
                let s = self.camera.dist as f64 * 0.02;
                let pts: Vec<Pos2> = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0), (-1.0, -1.0)]
                    .iter()
                    .filter_map(|(a, b)| self.project(v, *point + u * (a * s) + w * (b * s)))
                    .collect();
                p.add(egui::Shape::line(pts, Stroke::new(width, col)));
                if let Some(c) = self.project(v, *point) {
                    p.circle_filled(c, 3.0, col);
                }
            }
            Feature::Circle { center, .. } => {
                let pts: Vec<Pos2> = f.polyline().iter().filter_map(|x| self.project(v, *x)).collect();
                p.add(egui::Shape::line(pts, Stroke::new(width, col)));
                if let Some(c) = self.project(v, *center) {
                    p.line_segment([c - vec2(5.0, 0.0), c + vec2(5.0, 0.0)], Stroke::new(1.5, col));
                    p.line_segment([c - vec2(0.0, 5.0), c + vec2(0.0, 5.0)], Stroke::new(1.5, col));
                }
            }
            _ => {
                let pts: Vec<Pos2> = f.polyline().iter().filter_map(|x| self.project(v, *x)).collect();
                p.add(egui::Shape::line(pts, Stroke::new(width, col)));
            }
        }
    }

    fn draw_measure(&self, p: &egui::Painter, v: &View) {
        if let Some(h) = &self.measure.hover {
            self.draw_feature(p, v, h, Color32::from_rgba_unmultiplied(242, 122, 18, 150), 3.0);
        }
        if let Some(f) = &self.measure.first {
            self.draw_feature(p, v, f, MEASURE, 3.0);
        }
        if let Some(f) = &self.measure.second {
            self.draw_feature(p, v, f, MEASURE, 3.0);
        }

        let mut lines: Vec<String> = Vec::new();
        if let Some(f1) = &self.measure.first {
            lines.push(format!("1: {}", f1.describe()));
            if let Some(f2) = &self.measure.second {
                lines.push(format!("2: {}", f2.describe()));
                let r = measure::measure(f1, f2);
                if let (Some(a), Some(b)) = (self.project(v, r.from), self.project(v, r.to)) {
                    p.line_segment([a, b], Stroke::new(2.0, Color32::from_rgb(20, 30, 45)));
                    p.circle_filled(a, 3.5, Color32::from_rgb(20, 30, 45));
                    p.circle_filled(b, 3.5, Color32::from_rgb(20, 30, 45));
                    let text = measure::mm(r.from.distance(r.to));
                    let font = FontId::proportional(14.0);
                    let galley = p.layout_no_wrap(text, font, Color32::WHITE);
                    let at = Rect::from_center_size(a.lerp(b, 0.5), galley.size() + vec2(12.0, 6.0));
                    p.rect_filled(at, 4.0, MEASURE);
                    p.galley(at.min + vec2(6.0, 3.0), galley, Color32::WHITE);
                }
                lines.extend(r.lines);
            }
        } else if let Some(h) = &self.measure.hover {
            lines.push(h.describe());
        }
        if lines.is_empty() {
            return;
        }
        // Results panel, bottom-left.
        let font = FontId::proportional(13.0);
        let galleys: Vec<_> = lines.into_iter().map(|l| p.layout_no_wrap(l, font.clone(), Color32::from_gray(30))).collect();
        let w = galleys.iter().map(|g| g.size().x).fold(0.0, f32::max) + 24.0;
        let h = galleys.iter().map(|g| g.size().y + 3.0).sum::<f32>() + 16.0;
        let r = Rect::from_min_size(pos2(v.rect.min.x + 12.0, v.rect.max.y - h - 60.0), vec2(w, h));
        p.rect_filled(r, 6.0, Color32::from_rgba_unmultiplied(255, 255, 255, 240));
        p.rect_stroke(r, 6.0, Stroke::new(1.0, Color32::from_gray(200)), egui::StrokeKind::Inside);
        let mut y = r.min.y + 8.0;
        for g in galleys {
            let gh = g.size().y;
            p.galley(pos2(r.min.x + 12.0, y), g, Color32::from_gray(30));
            y += gh + 3.0;
        }
    }

    // ----------------------------------------------------------------------
    // Drawing
    // ----------------------------------------------------------------------

    fn render_scene(
        &mut self,
        ui: &mut egui::Ui,
        frame: &mut eframe::Frame,
        rect: Rect,
        items: &[Item],
        bounds: Option<(DVec3, DVec3)>,
    ) {
        let Some(rs) = frame.wgpu_render_state() else { return };
        let ppp = ui.ctx().pixels_per_point();
        let size = [(rect.width() * ppp).round() as u32, (rect.height() * ppp).round() as u32];
        if size[0] < 2 || size[1] < 2 {
            return;
        }

        // One outline group per top-level node.
        let mut groups: HashMap<NodeId, u32> = HashMap::new();
        let draw: Vec<DrawItem> = items
            .iter()
            .map(|it| {
                let n = groups.len() as u32;
                let group = *groups.entry(it.node).or_insert(n);
                let c = egui::Rgba::from(Color32::from_rgb(it.color.0[0], it.color.0[1], it.color.0[2]));
                DrawItem {
                    key: it.key,
                    mesh: it.mesh.clone(),
                    model: it.model.as_mat4(),
                    color: [c.r(), c.g(), c.b(), 1.0],
                    hole: it.hole,
                    selected: self.tool != Tool::Measure && self.selection.contains(&it.node),
                    hovered: self.hovered == Some(it.node),
                    group,
                }
            })
            .collect();

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
                items: &draw,
                workplane: Some([wp[0] as f32, wp[1] as f32]),
                clear: [bg.r() as f64, bg.g() as f64, bg.b() as f64, 1.0],
                extra_lines: &[],
                fine_grid: self.camera.dist < 400.0,
                grid_fade: self.camera.dist * 0.9,
                ppp,
                footprint: bounds.filter(|_| self.tool != Tool::Measure).map(|(lo, hi)| {
                    let z = lo.z as f32;
                    [
                        Vec3::new(lo.x as f32, lo.y as f32, z),
                        Vec3::new(hi.x as f32, lo.y as f32, z),
                        Vec3::new(hi.x as f32, hi.y as f32, z),
                        Vec3::new(lo.x as f32, hi.y as f32, z),
                    ]
                }),
            },
        );
        ui.painter().image(tex, rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    }

    /// Footprint on the floor, dashed outline and centre line, like Tinkercad.
    fn draw_selection_frame(&self, painter: &egui::Painter, v: &View, lo: DVec3, hi: DVec3) {
        let c = (lo + hi) * 0.5;
        let foot: Vec<Pos2> = [(lo.x, lo.y), (hi.x, lo.y), (hi.x, hi.y), (lo.x, hi.y)]
            .iter()
            .filter_map(|(x, y)| self.project(v, DVec3::new(*x, *y, lo.z)))
            .collect();
        if foot.len() == 4 {
            let stroke = Stroke::new(1.2, Color32::from_rgba_unmultiplied(30, 40, 55, 200));
            let mut ring = foot.clone();
            ring.push(foot[0]);
            painter.extend(egui::Shape::dashed_line(&ring, stroke, 5.0, 4.0));
        }
        if self.tool == Tool::Select && !self.selection_locked() {
            if let (Some(a), Some(b)) = (
                self.project(v, DVec3::new(c.x, c.y, lo.z)),
                self.project(v, DVec3::new(c.x, c.y, hi.z)),
            ) {
                painter.extend(egui::Shape::dashed_line(
                    &[a, b],
                    Stroke::new(1.0, Color32::from_rgba_unmultiplied(30, 40, 55, 160)),
                    4.0,
                    4.0,
                ));
            }
        }
        if self.selection_locked() {
            if let Some(pos) = self.project(v, DVec3::new(c.x, c.y, hi.z)) {
                let r = Rect::from_center_size(pos - vec2(0.0, 22.0), vec2(18.0, 18.0));
                icons::paint(painter, r, icons::Icon::LockClosed, Color32::from_gray(40), Color32::from_rgb(0xb7, 0xd4, 0xf5));
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
            Some(Drag::Rotate { axis, ring, radius, applied, .. }) => {
                // Protractor: ring with 22.5 degree ticks.
                let (_, u, w) = axis_basis(*axis);
                let c = *ring;
                let pts: Vec<Pos2> = (0..=96)
                    .filter_map(|i| {
                        let a = i as f64 / 96.0 * std::f64::consts::TAU;
                        self.project(v, c + (u * a.cos() + w * a.sin()) * *radius)
                    })
                    .collect();
                painter.add(egui::Shape::line(pts, Stroke::new(1.5, ACCENT)));
                for i in 0..16 {
                    let a = i as f64 / 16.0 * std::f64::consts::TAU;
                    let dir = u * a.cos() + w * a.sin();
                    let len = if i % 4 == 0 { 0.12 } else { 0.06 };
                    if let (Some(p0), Some(p1)) = (
                        self.project(v, c + dir * *radius),
                        self.project(v, c + dir * *radius * (1.0 - len)),
                    ) {
                        painter.line_segment([p0, p1], Stroke::new(1.2, ACCENT));
                    }
                }
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
                egui::Frame::popup(ui.style()).inner_margin(4).show(ui, |ui| {
                    ui.spacing_mut().item_spacing = vec2(2.0, 2.0);
                    ui.vertical(|ui| {
                        if icons::button(ui, icons::Icon::Home, true, "Home view").clicked() {
                            self.camera = crate::render::camera::Camera::home();
                        }
                        if icons::button(ui, icons::Icon::Fit, true, "Fit selection, or everything (F)").clicked() {
                            self.fit_all();
                        }
                        if ui.add_sized([34.0, 22.0], egui::Button::new("+").frame(false)).on_hover_text("Zoom in (+)").clicked() {
                            self.camera.zoom(0.8, None);
                        }
                        if ui.add_sized([34.0, 22.0], egui::Button::new("-").frame(false)).on_hover_text("Zoom out (-)").clicked() {
                            self.camera.zoom(1.25, None);
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
                            if ui.add_sized([34.0, 20.0], egui::Button::new(egui::RichText::new(name).size(10.0)).frame(false)).on_hover_text("Look from this side").clicked() {
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
    let hot_fill = ACCENT;
    match h.kind {
        HandleKind::Corner(..) | HandleKind::Top => {
            let r = Rect::from_center_size(h.pos, Vec2::splat(HANDLE_R * 2.0));
            p.rect_filled(r, 1.0, if hot { hot_fill } else { Color32::WHITE });
            p.rect_stroke(r, 1.0, Stroke::new(1.4, Color32::from_gray(60)), egui::StrokeKind::Middle);
        }
        HandleKind::Side(..) => {
            let r = Rect::from_center_size(h.pos, Vec2::splat(HANDLE_R * 1.5));
            p.rect_filled(r, 1.0, if hot { hot_fill } else { Color32::from_gray(30) });
        }
        HandleKind::Lift => {
            let c = h.pos;
            let pts = vec![c + vec2(0.0, -9.0), c + vec2(6.5, 6.0), c + vec2(-6.5, 6.0)];
            p.add(egui::Shape::convex_polygon(pts, if hot { hot_fill } else { Color32::from_gray(25) }, Stroke::NONE));
        }
        HandleKind::Rotate(_) => {
            let col = if hot { hot_fill } else { ARROW_GREY };
            icons::curved_double_arrow(p, &h.arc, col, if hot { 2.6 } else { 2.0 });
        }
        HandleKind::AlignDot(_, which) => {
            let r = if which == 1 { 6.0 } else { 5.0 };
            p.circle_filled(h.pos, r + 1.5, Color32::WHITE);
            p.circle_filled(h.pos, r, if hot { hot_fill } else { Color32::from_gray(35) });
        }
        HandleKind::MirrorArrow(_) => {
            let col = if hot { hot_fill } else { Color32::from_gray(35) };
            p.circle_filled(h.pos, 15.0, Color32::from_rgba_unmultiplied(255, 255, 255, 230));
            p.circle_stroke(h.pos, 15.0, Stroke::new(1.2, col));
            // Double arrow along the projected axis, fixed on-screen length.
            let mut dir = h.arc[1] - h.arc[0];
            if dir.length() < 1e-3 {
                dir = vec2(1.0, 0.0);
            }
            let d = dir.normalized() * 8.0;
            icons::curved_double_arrow(p, &[h.pos - d, h.pos + d], col, 2.0);
        }
    }
}
