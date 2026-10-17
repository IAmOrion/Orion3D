//! Floating inspector (top-right of the viewport), like Tinkercad's shape panel.

use crate::app::OrionApp;
use crate::doc::{Content, FilletKind, PALETTE, Rgb, Shape};
use crate::ops;
use egui::{Color32, DragValue, Rect, RichText, Sense, vec2};
use glam::DVec3;

fn mm(v: &mut f64, range: std::ops::RangeInclusive<f64>) -> DragValue<'_> {
    DragValue::new(v).speed(0.1).range(range).suffix(" mm").max_decimals(2)
}

fn count(v: &mut u32, range: std::ops::RangeInclusive<u32>) -> DragValue<'_> {
    DragValue::new(v).speed(0.2).range(range)
}

fn row(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui) -> bool) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.add_sized([96.0, 20.0], egui::Label::new(RichText::new(label).color(Color32::from_gray(80))));
        changed = add(ui);
    });
    changed
}

/// Shape-specific properties. Overall dimensions are edited through the
/// Size fields (or the handles); these are the parameters that are not.
fn shape_props(ui: &mut egui::Ui, s: &mut Shape) -> bool {
    let mut ch = false;
    match s {
        Shape::Box { size, radius, steps } => {
            let max_r = size.iter().cloned().fold(f64::MAX, f64::min) * 0.5;
            ch |= row(ui, "Radius", |ui| ui.add(mm(radius, 0.0..=max_r)).changed());
            ch |= row(ui, "Steps", |ui| ui.add(count(steps, 1..=32)).changed());
        }
        Shape::Cylinder { sides, .. } | Shape::Polygon { sides, .. } => {
            ch |= row(ui, "Sides", |ui| ui.add(count(sides, 3..=128)).changed());
        }
        Shape::Sphere { steps, .. } | Shape::HalfSphere { steps, .. } => {
            ch |= row(ui, "Steps", |ui| ui.add(count(steps, 4..=128)).changed());
        }
        Shape::Cone { base_radius, top_radius, sides, .. } => {
            ch |= row(ui, "Base radius", |ui| ui.add(mm(base_radius, 0.1..=1000.0)).changed());
            ch |= row(ui, "Top radius", |ui| ui.add(mm(top_radius, 0.0..=1000.0)).changed());
            ch |= row(ui, "Sides", |ui| ui.add(count(sides, 3..=128)).changed());
        }
        Shape::Tube { radius, wall, sides, .. } => {
            let r = *radius;
            ch |= row(ui, "Wall", |ui| ui.add(mm(wall, 0.1..=r)).changed());
            ch |= row(ui, "Sides", |ui| ui.add(count(sides, 3..=128)).changed());
        }
        Shape::Torus { radius, tube, sides, steps } => {
            ch |= row(ui, "Radius", |ui| ui.add(mm(radius, 0.5..=1000.0)).changed());
            let r = *radius;
            ch |= row(ui, "Tube", |ui| ui.add(mm(tube, 0.1..=r)).changed());
            ch |= row(ui, "Sides", |ui| ui.add(count(sides, 3..=64)).changed());
            ch |= row(ui, "Steps", |ui| ui.add(count(steps, 3..=128)).changed());
        }
        Shape::Pyramid { sides, .. } => {
            ch |= row(ui, "Sides", |ui| ui.add(count(sides, 3..=64)).changed());
        }
        Shape::Roof { .. } | Shape::Wedge { .. } => {}
        Shape::MetaFillet { radius, height, kind, segments } => {
            ch |= row(ui, "Radius", |ui| {
                ui.add(egui::Slider::new(radius, 1.0..=100.0).suffix(" mm").max_decimals(2).clamping(egui::SliderClamping::Never))
                    .changed()
            });
            ch |= row(ui, "Height", |ui| {
                ui.add(egui::Slider::new(height, 0.5..=100.0).suffix(" mm").max_decimals(2).clamping(egui::SliderClamping::Never))
                    .changed()
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
                ch |= row(ui, "Segments", |ui| ui.add(count(segments, 3..=64)).changed());
            }
            *radius = radius.max(0.1);
            *height = height.max(0.1);
        }
    }
    ch
}

impl OrionApp {
    pub fn inspector(&mut self, ui: &mut egui::Ui, view: Rect) {
        if self.selection.is_empty() {
            return;
        }
        let ctx = ui.ctx().clone();
        egui::Area::new(egui::Id::unique("inspector"))
            .fixed_pos(view.right_top() + vec2(-12.0, 12.0))
            .pivot(egui::Align2::RIGHT_TOP)
            .show(&ctx, |ui| {
                egui::Frame::popup(ui.style()).inner_margin(12).show(ui, |ui| {
                    ui.set_width(270.0);
                    self.inspector_contents(ui);
                });
            });
    }

    fn inspector_contents(&mut self, ui: &mut egui::Ui) {
        let single = (self.selection.len() == 1).then(|| self.selection[0]);

        // Title.
        if let Some(id) = single {
            if let Some(n) = self.doc.get_mut(id) {
                ui.add(egui::TextEdit::singleline(&mut n.name).font(egui::TextStyle::Heading).desired_width(f32::INFINITY));
            }
        } else {
            ui.heading(format!("{} objects", self.selection.len()));
        }
        ui.add_space(4.0);

        // Solid / Hole.
        let all_hole = self.selection.iter().all(|id| self.doc.get(*id).is_some_and(|n| n.hole));
        let any_hole = self.selection.iter().any(|id| self.doc.get(*id).is_some_and(|n| n.hole));
        ui.horizontal(|ui| {
            let w = (ui.available_width() - 6.0) / 2.0;
            if ui
                .add_sized([w, 30.0], egui::Button::selectable(!any_hole, RichText::new("Solid").size(14.0)))
                .on_hover_text("S")
                .clicked()
            {
                self.set_hole(false);
            }
            if ui
                .add_sized([w, 30.0], egui::Button::selectable(all_hole, RichText::new("Hole").size(14.0)))
                .on_hover_text("H")
                .clicked()
            {
                self.set_hole(true);
            }
        });

        // Colour (not for holes).
        if !all_hole {
            let current = self.selection.first().and_then(|id| self.doc.get(*id)).map(|n| n.color);
            ui.add_space(2.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                for c in PALETTE {
                    let (rect, resp) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::click());
                    let col = Color32::from_rgb(c.0[0], c.0[1], c.0[2]);
                    ui.painter().rect_filled(rect, 3.0, col);
                    if current == Some(c) {
                        ui.painter().rect_stroke(rect.expand(2.0), 4.0, (2.0, Color32::from_rgb(0x2b, 0x7c, 0xe9)), egui::StrokeKind::Inside);
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

        ui.separator();

        // Shape properties / group info.
        if let Some(id) = single {
            let mut ungroup = false;
            let mut baked = false;
            if let Some(n) = self.doc.get_mut(id) {
                match &mut n.content {
                    Content::Shape(s) => {
                        ui.label(RichText::new(s.kind().label()).strong().color(Color32::from_gray(70)));
                        if shape_props(ui, s) {
                            baked = true;
                        }
                    }
                    Content::Group(ch) => {
                        let mut solids = 0;
                        let mut holes = 0;
                        for c in ch.iter() {
                            if c.hole { holes += 1 } else { solids += 1 }
                        }
                        ui.label(format!("Group of {} ({} solid, {} hole)", solids + holes, solids, holes));
                        if ui.button("Ungroup").on_hover_text("Ctrl+Shift+G").clicked() {
                            ungroup = true;
                        }
                    }
                }
                if baked {
                    ops::bake_scale(n);
                }
            }
            if ungroup {
                self.ungroup();
                return;
            }
        } else if ui.button("Group").on_hover_text("Ctrl+G").clicked() {
            self.group();
            return;
        }

        ui.separator();
        self.transform_fields(ui);
    }

    fn transform_fields(&mut self, ui: &mut egui::Ui) {
        let Some((lo, hi)) = ops::bounds_of(&mut self.cache, &self.doc, &self.selection) else { return };
        let c = (lo + hi) * 0.5;
        let size = hi - lo;

        ui.label(RichText::new("Position").strong().color(Color32::from_gray(70)));
        let mut p = [c.x, c.y, lo.z];
        let mut moved = false;
        ui.horizontal(|ui| {
            for (i, l) in ["X", "Y", "Z"].iter().enumerate() {
                ui.label(*l);
                moved |= ui.add(DragValue::new(&mut p[i]).speed(0.1).max_decimals(2)).changed();
            }
        });
        if moved {
            self.translate_selection(DVec3::new(p[0] - c.x, p[1] - c.y, p[2] - lo.z));
        }

        ui.label(RichText::new("Size").strong().color(Color32::from_gray(70)));
        let mut s = [size.x, size.y, size.z];
        let mut resized = None;
        ui.horizontal(|ui| {
            for (i, l) in ["W", "L", "H"].iter().enumerate() {
                ui.label(*l);
                if ui.add(DragValue::new(&mut s[i]).speed(0.1).range(0.01..=10000.0).max_decimals(2)).changed() {
                    resized = Some(i);
                }
            }
        });
        if let Some(i) = resized {
            let mut f = DVec3::ONE;
            if size[i] > 1e-9 {
                f[i] = s[i] / size[i];
            }
            let anchor = DVec3::new(c.x, c.y, lo.z);
            let orig = ops::originals(&self.doc, &self.selection);
            ops::apply(&mut self.doc, &orig, ops::scale_about(anchor, f), true);
        }

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Rotate 90").color(Color32::from_gray(80)));
            for (i, a) in ["X", "Y", "Z"].iter().enumerate() {
                if ui.small_button(*a).on_hover_text(format!("Rotate 90 degrees around {a}")).clicked() {
                    self.rotate_selection(i, 90.0);
                }
            }
            ui.label(RichText::new("Mirror").color(Color32::from_gray(80)));
            for (i, a) in ["X", "Y", "Z"].iter().enumerate() {
                if ui.small_button(*a).on_hover_text(format!("Mirror along {a}")).clicked() {
                    self.mirror(i);
                }
            }
        });
    }
}
