//! Shape library panel with rendered thumbnails. Click a shape to add it
//! at the centre of the view, or drag it onto the workplane.

use crate::app::OrionApp;
use crate::doc::{Content, LIBRARY, ShapeKind};
use crate::render::camera::Camera;
use crate::render::{DrawItem, SceneParams};
use egui::{Color32, RichText, Sense};
use glam::Mat4;

const THUMB_PX: u32 = 128;

impl OrionApp {
    pub fn make_thumbnails(&mut self, frame: &mut eframe::Frame) {
        if !self.thumbs.is_empty() {
            return;
        }
        let (Some(rs), Some(renderer)) = (frame.wgpu_render_state(), self.renderer.as_mut()) else {
            return;
        };
        for kind in LIBRARY {
            let content = Content::Shape(kind.default_shape());
            let (key, mesh) = self.cache.content_mesh(&content);
            let mut cam = Camera::home();
            cam.yaw = (-120f32).to_radians();
            cam.pitch = 28f32.to_radians();
            cam.fit(mesh.min, mesh.max);
            cam.dist *= 0.63;
            let c = egui::Rgba::from(Color32::from_rgb(
                kind.default_color().0[0],
                kind.default_color().0[1],
                kind.default_color().0[2],
            ));
            let items = [DrawItem {
                key,
                mesh,
                model: Mat4::IDENTITY,
                color: [c.r(), c.g(), c.b(), 1.0],
                hole: false,
                selected: false,
                hovered: false,
            }];
            let mut target = renderer.create_target(&rs.device, [THUMB_PX, THUMB_PX]);
            let id = renderer.render(
                rs,
                &mut target,
                [THUMB_PX, THUMB_PX],
                &SceneParams {
                    view_proj: cam.view_proj(1.0),
                    eye: cam.eye(),
                    items: &items,
                    workplane: None,
                    clear: [0.0, 0.0, 0.0, 0.0],
                    extra_lines: &[],
                    fine_grid: false,
                    grid_fade: 1.0,
                },
            );
            self.thumbs.insert(kind, id);
            self.thumb_targets.push(target);
        }
    }

    pub fn library_panel(&mut self, ui: &mut egui::Ui) {
        egui::Panel::right("library")
            .resizable(false)
            .exact_size(214.0)
            .frame(egui::Frame::new().fill(Color32::from_rgb(0xf6, 0xf8, 0xfb)).inner_margin(10))
            .show(ui, |ui| {
                ui.label(RichText::new("Basic Shapes").strong().size(15.0));
                ui.add_space(4.0);
                egui::ScrollArea::vertical().show(ui, |ui| {
                    egui::Grid::new("shape_grid").spacing([8.0, 8.0]).show(ui, |ui| {
                        for (i, kind) in LIBRARY.iter().enumerate() {
                            self.library_tile(ui, *kind);
                            if i % 2 == 1 {
                                ui.end_row();
                            }
                        }
                    });
                    ui.add_space(10.0);
                    ui.label(
                        RichText::new("Click to add at the centre of the view, or drag onto the workplane.")
                            .size(11.0)
                            .color(Color32::from_gray(120)),
                    );
                });
            });
    }

    fn library_tile(&mut self, ui: &mut egui::Ui, kind: ShapeKind) {
        let size = egui::vec2(90.0, 96.0);
        let thumb = self.thumbs.get(&kind).copied();
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
        resp.dnd_set_drag_payload(kind);
        let hovered = resp.hovered() || resp.dragged();
        let p = ui.painter();
        p.rect_filled(rect, 6.0, if hovered { Color32::from_rgb(0xdd, 0xea, 0xf8) } else { Color32::WHITE });
        p.rect_stroke(rect, 6.0, (1.0, Color32::from_gray(222)), egui::StrokeKind::Inside);
        let img = egui::Rect::from_center_size(rect.center_top() + egui::vec2(0.0, 38.0), egui::vec2(68.0, 68.0));
        if let Some(t) = thumb {
            p.image(t, img, full_uv(), Color32::WHITE);
        }
        p.text(
            rect.center_bottom() - egui::vec2(0.0, 6.0),
            egui::Align2::CENTER_BOTTOM,
            kind.label(),
            egui::FontId::proportional(12.0),
            Color32::from_gray(60),
        );
        if resp.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            // Ghost of the shape under the cursor while dragging.
            if let (Some(pos), Some(t)) = (ui.ctx().pointer_hover_pos(), thumb) {
                let layer = egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("lib_drag_ghost"));
                let r = egui::Rect::from_center_size(pos, egui::vec2(56.0, 56.0));
                ui.ctx().layer_painter(layer).image(t, r, full_uv(), Color32::from_white_alpha(200));
            }
        }
        if resp.clicked() {
            self.add_shape(kind, None);
        }
    }
}

fn full_uv() -> egui::Rect {
    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0))
}
