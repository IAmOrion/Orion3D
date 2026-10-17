//! Floating inspector (top-right of the viewport), like Tinkercad's shape panel.

use crate::app::OrionApp;
use crate::doc::{Content, FilletKind, GroupOp, PALETTE, Rgb, Shape};
use crate::icons::{self, Icon};
use crate::ops;
use egui::{Color32, DragValue, Rect, RichText, Sense, Stroke, vec2};
use glam::DVec3;

fn row(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui) -> bool) -> bool {
    let mut changed = false;
    ui.label(RichText::new(label).color(Color32::from_gray(80)));
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        changed = add(ui);
    });
    ui.add_space(2.0);
    changed
}

fn slider_mm(v: &mut f64, range: std::ops::RangeInclusive<f64>) -> egui::Slider<'_> {
    egui::Slider::new(v, range).max_decimals(2).clamping(egui::SliderClamping::Never)
}

fn slider_n(v: &mut u32, range: std::ops::RangeInclusive<u32>) -> egui::Slider<'_> {
    egui::Slider::new(v, range)
}

/// Shape-specific properties. Overall dimensions are edited through the
/// Size fields (or the handles); these are the parameters that are not.
fn shape_props(ui: &mut egui::Ui, s: &mut Shape) -> bool {
    let mut ch = false;
    match s {
        Shape::Box { size, radius, steps } => {
            let max_r = size.iter().cloned().fold(f64::MAX, f64::min) * 0.5;
            ch |= row(ui, "Radius", |ui| ui.add(slider_mm(radius, 0.0..=max_r)).changed());
            *radius = radius.clamp(0.0, max_r);
            ch |= row(ui, "Steps", |ui| ui.add(slider_n(steps, 1..=32)).changed());
        }
        Shape::Cylinder { sides, .. } | Shape::Polygon { sides, .. } => {
            ch |= row(ui, "Sides", |ui| ui.add(slider_n(sides, 3..=128)).changed());
        }
        Shape::Sphere { steps, .. } | Shape::HalfSphere { steps, .. } => {
            ch |= row(ui, "Steps", |ui| ui.add(slider_n(steps, 4..=128)).changed());
        }
        Shape::Cone { base_radius, top_radius, sides, .. } => {
            ch |= row(ui, "Base radius", |ui| ui.add(slider_mm(base_radius, 0.1..=100.0)).changed());
            ch |= row(ui, "Top radius", |ui| ui.add(slider_mm(top_radius, 0.0..=100.0)).changed());
            ch |= row(ui, "Sides", |ui| ui.add(slider_n(sides, 3..=128)).changed());
        }
        Shape::Tube { radius, wall, sides, .. } => {
            let r = *radius;
            ch |= row(ui, "Wall", |ui| ui.add(slider_mm(wall, 0.1..=r)).changed());
            ch |= row(ui, "Sides", |ui| ui.add(slider_n(sides, 3..=128)).changed());
        }
        Shape::Torus { radius, tube, sides, steps } => {
            ch |= row(ui, "Radius", |ui| ui.add(slider_mm(radius, 0.5..=100.0)).changed());
            let r = *radius;
            ch |= row(ui, "Tube", |ui| ui.add(slider_mm(tube, 0.1..=r)).changed());
            ch |= row(ui, "Sides", |ui| ui.add(slider_n(sides, 3..=64)).changed());
            ch |= row(ui, "Steps", |ui| ui.add(slider_n(steps, 3..=128)).changed());
        }
        Shape::Pyramid { sides, .. } => {
            ch |= row(ui, "Sides", |ui| ui.add(slider_n(sides, 3..=64)).changed());
        }
        Shape::Roof { .. } | Shape::Wedge { .. } => {}
        Shape::MetaFillet { radius, height, kind, segments } => {
            ch |= row(ui, "Radius", |ui| {
                ui.add(slider_mm(radius, 1.0..=100.0)).changed()
            });
            ch |= row(ui, "Height", |ui| {
                ui.add(slider_mm(height, 0.5..=100.0)).changed()
            });
            ch |= row(ui, "Type", |ui| {
                let mut c = false;
                egui::ComboBox::from_id_salt("fillet_kind")
                    .selected_text(match kind {
                        FilletKind::Fillet => "Fillet",
                        FilletKind::Chamfer => "Chamfer",
                    })
                    .show_ui(ui, |ui| {
                        c |= ui.selectable_value(kind, FilletKind::Fillet, "Fillet").changed();
                        c |= ui.selectable_value(kind, FilletKind::Chamfer, "Chamfer").changed();
                    });
                c
            });
            if *kind == FilletKind::Fillet {
                ch |= row(ui, "Segments", |ui| ui.add(slider_n(segments, 3..=64)).changed());
            }
            *radius = radius.max(0.1);
            *height = height.max(0.1);
        }
    }
    ch
}

fn card(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(Color32::WHITE)
        .corner_radius(8)
        .inner_margin(12)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
}

/// Round Solid / Hole swatch. Returns true when clicked.
fn swatch(ui: &mut egui::Ui, label: &str, fill: Option<Color32>, active: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(96.0, 84.0), Sense::click());
    let c = rect.center_top() + vec2(0.0, 32.0);
    let p = ui.painter();
    if active {
        p.circle_stroke(c, 27.0, Stroke::new(2.0, Color32::from_rgb(0x8e, 0xb6, 0xee)));
    }
    match fill {
        Some(col) => {
            p.circle_filled(c, 23.0, col);
        }
        None => {
            // Striped grey circle for "Hole".
            p.circle_filled(c, 23.0, Color32::from_rgb(0xc4, 0xc7, 0xcb));
            let st = Stroke::new(3.0, Color32::from_rgb(0x9a, 0x9e, 0xa4));
            for k in -3..=3 {
                let o = k as f32 * 8.0;
                // Clip the stripe to the circle.
                let r = 22.0f32;
                let d = o / std::f32::consts::SQRT_2;
                let half = (r * r - d * d).max(0.0).sqrt();
                let dir = vec2(1.0, -1.0).normalized();
                let n = vec2(1.0, 1.0).normalized();
                let mid = c + n * d;
                p.line_segment([mid - dir * half, mid + dir * half], st);
            }
        }
    }
    let col = if active { Color32::from_rgb(0x5c, 0x8f, 0xd6) } else { Color32::from_gray(50) };
    p.text(rect.center_bottom() - vec2(0.0, 6.0), egui::Align2::CENTER_BOTTOM, label, egui::FontId::proportional(15.0), col);
    resp.on_hover_text(if label == "Hole" { "Hole (H)" } else { "Solid colour (S)" }).clicked()
}

fn small_icon(ui: &mut egui::Ui, icon: Icon, tip: &str) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, 4.0, Color32::from_rgb(0xe3, 0xec, 0xf7));
    }
    icons::paint(
        ui.painter(),
        Rect::from_center_size(rect.center(), vec2(20.0, 20.0)),
        icon,
        Color32::from_rgb(0x46, 0x50, 0x5c),
        Color32::from_rgb(0xb7, 0xd4, 0xf5),
    );
    resp.on_hover_text(tip).clicked()
}

impl OrionApp {
    pub fn inspector(&mut self, ui: &mut egui::Ui, view: Rect) {
        if self.selection.is_empty() {
            self.palette_open = false;
            return;
        }
        let ctx = ui.ctx().clone();
        egui::Area::new(egui::Id::unique("inspector"))
            .fixed_pos(view.right_top() + vec2(-12.0, 12.0))
            .pivot(egui::Align2::RIGHT_TOP)
            .show(&ctx, |ui| {
                egui::Frame::new()
                    .fill(Color32::from_rgb(0xf3, 0xf4, 0xf6))
                    .corner_radius(8)
                    .shadow(egui::Shadow { offset: [0, 2], blur: 10, spread: 0, color: Color32::from_black_alpha(40) })
                    .stroke(Stroke::new(1.0, Color32::from_gray(222)))
                    .inner_margin(10)
                    .show(ui, |ui| {
                        ui.set_width(300.0);
                        self.inspector_contents(ui);
                    });
            });
    }

    fn inspector_contents(&mut self, ui: &mut egui::Ui) {
        let single = (self.selection.len() == 1).then(|| self.selection[0]);
        let locked = self.selection.iter().any(|id| self.doc.get(*id).is_some_and(|n| n.locked));

        // Header: collapse, name, lock, hide.
        ui.horizontal(|ui| {
            let chev = if self.inspector_open { Icon::ChevronUp } else { Icon::ChevronDown };
            if small_icon(ui, chev, if self.inspector_open { "Collapse" } else { "Expand" }) {
                self.inspector_open = !self.inspector_open;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if small_icon(ui, Icon::BulbOff, "Hide (Ctrl+H)") {
                    self.hide_selection();
                }
                let lock_icon = if locked { Icon::LockClosed } else { Icon::LockOpen };
                if small_icon(ui, lock_icon, if locked { "Unlock (Ctrl+L)" } else { "Lock (Ctrl+L)" }) {
                    self.toggle_lock();
                }
                if let Some(id) = single {
                    if let Some(n) = self.doc.get_mut(id) {
                        ui.add_enabled(
                            !locked,
                            egui::TextEdit::singleline(&mut n.name)
                                .font(egui::FontId::proportional(18.0))
                                .frame(egui::Frame::NONE)
                                .desired_width(f32::INFINITY),
                        );
                    }
                } else {
                    ui.label(RichText::new(format!("{} shapes", self.selection.len())).size(18.0));
                }
            });
        });
        if self.selection.is_empty() || !self.inspector_open {
            return;
        }
        ui.add_space(6.0);

        ui.add_enabled_ui(!locked, |ui| {
            self.solid_hole_card(ui);
            ui.add_space(8.0);
            self.properties_card(ui, single);
        });
    }

    fn solid_hole_card(&mut self, ui: &mut egui::Ui) {
        let all_hole = self.selection.iter().all(|id| self.doc.get(*id).is_some_and(|n| n.hole));
        let any_hole = self.selection.iter().any(|id| self.doc.get(*id).is_some_and(|n| n.hole));
        let current = self.selection.iter().filter_map(|id| self.doc.get(*id)).find(|n| !n.hole).map(|n| n.color);
        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add_space(18.0);
                let col = current.map(|c| Color32::from_rgb(c.0[0], c.0[1], c.0[2])).unwrap_or(Color32::from_rgb(0xe1, 0x35, 0x2d));
                if swatch(ui, "Solid", Some(col), !any_hole) {
                    if any_hole {
                        self.set_hole(false);
                    }
                    self.palette_open = !self.palette_open;
                }
                ui.add_space(30.0);
                if swatch(ui, "Hole", None, all_hole) {
                    self.set_hole(true);
                    self.palette_open = false;
                }
            });
            if self.palette_open && !all_hole {
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                    for c in PALETTE {
                        let (rect, resp) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::click());
                        let col = Color32::from_rgb(c.0[0], c.0[1], c.0[2]);
                        ui.painter().circle_filled(rect.center(), 14.0, col);
                        if current == Some(c) {
                            ui.painter().circle_stroke(rect.center(), 16.0, Stroke::new(2.0, Color32::from_rgb(0x5c, 0x8f, 0xd6)));
                        }
                        if resp.clicked() {
                            self.set_color(c);
                        }
                    }
                    if let Some(cur) = current {
                        let mut rgb = cur.0;
                        if ui.color_edit_button_srgb(&mut rgb).on_hover_text("Custom colour").changed() {
                            self.set_color(Rgb(rgb));
                        }
                    }
                });
            }
        });
    }

    fn properties_card(&mut self, ui: &mut egui::Ui, single: Option<u64>) {
        card(ui, |ui| {
            ui.label(RichText::new("Properties").size(16.0).color(Color32::from_gray(40)));
            ui.add_space(4.0);
            if let Some(id) = single {
                let mut ungroup = false;
                let mut op_change = None;
                if let Some(n) = self.doc.get_mut(id) {
                    let mut baked = false;
                    match &mut n.content {
                        Content::Shape(s) => {
                            ui.label(RichText::new(s.kind().label()).color(Color32::from_gray(90)));
                            ui.add_space(2.0);
                            baked = shape_props(ui, s);
                        }
                        Content::Group(g) => {
                            ui.label(RichText::new(format!("Group type: {}", g.op.label())).color(Color32::from_gray(70)));
                            ui.horizontal(|ui| {
                                for (op, icon, tip) in [
                                    (GroupOp::Bundle, Icon::Bundle, "Bundle: keep parts and colours separate (Ctrl+B)"),
                                    (GroupOp::Union, Icon::Union, "Union: merge solids, holes cut (Ctrl+G)"),
                                    (GroupOp::Intersect, Icon::Intersect, "Intersect: keep only the overlap (Ctrl+I)"),
                                ] {
                                    let (rect, resp) = ui.allocate_exact_size(vec2(84.0, 48.0), Sense::click());
                                    let active = g.op == op;
                                    ui.painter().rect_filled(rect, 6.0, Color32::WHITE);
                                    ui.painter().rect_stroke(
                                        rect,
                                        6.0,
                                        Stroke::new(
                                            if active { 2.0 } else { 1.0 },
                                            if active { Color32::from_rgb(0x5c, 0x8f, 0xd6) } else { Color32::from_gray(215) },
                                        ),
                                        egui::StrokeKind::Inside,
                                    );
                                    icons::paint(
                                        ui.painter(),
                                        Rect::from_center_size(rect.center(), vec2(30.0, 30.0)),
                                        icon,
                                        Color32::from_rgb(0x2e, 0x3a, 0x4a),
                                        Color32::from_rgb(0xb7, 0xd4, 0xf5),
                                    );
                                    if resp.on_hover_text(tip).clicked() && !active {
                                        op_change = Some(op);
                                    }
                                }
                            });
                            let (mut solids, mut holes) = (0, 0);
                            for c in &g.children {
                                if c.hole { holes += 1 } else { solids += 1 }
                            }
                            ui.label(
                                RichText::new(format!("{} parts ({solids} solid, {holes} hole)", solids + holes))
                                    .size(12.0)
                                    .color(Color32::from_gray(110)),
                            );
                            if ui.button("Ungroup").on_hover_text("Ctrl+Shift+G").clicked() {
                                ungroup = true;
                            }
                        }
                        Content::Mesh(m) => {
                            ui.label(RichText::new("Imported mesh").color(Color32::from_gray(90)));
                            ui.label(
                                RichText::new(format!("{} triangles", m.indices.len() / 3))
                                    .size(12.0)
                                    .color(Color32::from_gray(110)),
                            );
                        }
                    }
                    if baked {
                        ops::bake_scale(n);
                    }
                }
                if let Some(op) = op_change {
                    self.group(op);
                }
                if ungroup {
                    self.ungroup();
                    return;
                }
            } else {
                ui.label(RichText::new("Group the selection as").color(Color32::from_gray(80)));
                ui.horizontal(|ui| {
                    if ui.button("Bundle").clicked() {
                        self.group(GroupOp::Bundle);
                    }
                    if ui.button("Union").clicked() {
                        self.group(GroupOp::Union);
                    }
                    if ui.button("Intersect").clicked() {
                        self.group(GroupOp::Intersect);
                    }
                });
            }
            ui.add_space(6.0);
            ui.separator();
            self.transform_fields(ui);
        });
    }

    fn transform_fields(&mut self, ui: &mut egui::Ui) {
        let Some((lo, hi)) = ops::bounds_of(&mut self.cache, &self.doc, &self.selection) else { return };
        let c = (lo + hi) * 0.5;
        let size = hi - lo;

        let mut p = [c.x, c.y, lo.z];
        let mut moved = false;
        let mut s = [size.x, size.y, size.z];
        let mut resized = None;
        egui::Grid::new("xform").num_columns(4).spacing([6.0, 4.0]).show(ui, |ui| {
            ui.label(RichText::new("Position").color(Color32::from_gray(80)));
            for v in p.iter_mut() {
                moved |= ui.add(DragValue::new(v).speed(0.1).max_decimals(2)).changed();
            }
            ui.end_row();
            ui.label(RichText::new("Size").color(Color32::from_gray(80)));
            for (i, v) in s.iter_mut().enumerate() {
                if ui.add(DragValue::new(v).speed(0.1).range(0.01..=10000.0).max_decimals(2)).changed() {
                    resized = Some(i);
                }
            }
            ui.end_row();
            ui.label("");
            for l in ["X / W", "Y / L", "Z / H"] {
                ui.label(RichText::new(l).size(11.0).color(Color32::from_gray(130)));
            }
            ui.end_row();
        });
        if moved {
            self.translate_selection(DVec3::new(p[0] - c.x, p[1] - c.y, p[2] - lo.z));
        }
        if let Some(i) = resized {
            let mut f = DVec3::ONE;
            if size[i] > 1e-9 {
                f[i] = s[i] / size[i];
            }
            let anchor = DVec3::new(c.x, c.y, lo.z);
            let ids: Vec<u64> = self.selection.clone();
            let orig = ops::originals(&self.doc, &ids);
            ops::apply(&mut self.doc, &orig, ops::scale_about(anchor, f), true);
        }
    }
}
