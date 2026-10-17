//! Shape library panel with rendered thumbnails. Click a shape to add it
//! at the centre of the view, or drag it onto the workplane.

use crate::app::OrionApp;
use crate::doc::{Content, LIBRARY, ShapeKind};
use crate::icons::{self, Icon};
use crate::render::camera::Camera;
use crate::render::{DrawItem, SceneParams};
use egui::{Color32, RichText, Sense, vec2};
use glam::Mat4;

const THUMB_PX: u32 = 160;

fn full_uv() -> egui::Rect {
    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0))
}

fn tile_label(kind: ShapeKind, hole: bool) -> String {
    if hole { format!("{} Hole", kind.label()) } else { kind.label().to_string() }
}

impl OrionApp {
    pub fn make_thumbnails(&mut self, frame: &mut eframe::Frame) {
        if !self.thumbs.is_empty() {
            return;
        }
        let (Some(rs), Some(renderer)) = (frame.wgpu_render_state(), self.renderer.as_mut()) else {
            return;
        };
        let ppp = 1.0;
        for (kind, hole) in LIBRARY {
            let content = Content::Shape(kind.default_shape());
            let (key, mesh) = self.cache.content_mesh(&content);
            let mut cam = Camera::home();
            cam.yaw = (-120f32).to_radians();
            cam.pitch = 26f32.to_radians();
            cam.fit(mesh.min, mesh.max);
            cam.dist *= 0.66;
            let col = kind.default_color().0;
            let c = egui::Rgba::from(Color32::from_rgb(col[0], col[1], col[2]));
            let items = [DrawItem {
                key,
                mesh,
                model: Mat4::IDENTITY,
                color: [c.r(), c.g(), c.b(), 1.0],
                hole,
                selected: false,
                hovered: false,
                group: 0,
                ghost: false,
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
                    ppp,
                    footprint: None,
                },
            );
            self.thumbs.insert((kind, hole), id);
            self.thumb_targets.push(target);
        }
    }

    pub fn library_panel(&mut self, ui: &mut egui::Ui) {
        egui::Panel::right("library")
            .resizable(false)
            .exact_size(292.0)
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(0xf6, 0xf7, 0xf9))
                    .stroke(egui::Stroke::new(1.0, Color32::from_gray(225)))
                    .inner_margin(12),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("category")
                        .selected_text(RichText::new("Basic Shapes").size(15.0))
                        .width(200.0)
                        .show_ui(ui, |ui| {
                            let _ = ui.selectable_label(true, "Basic Shapes");
                        });
                    let (r, resp) = ui.allocate_exact_size(vec2(36.0, 30.0), Sense::click());
                    ui.painter().rect_filled(r, 4.0, Color32::WHITE);
                    ui.painter().rect_stroke(r, 4.0, (1.0, Color32::from_gray(215)), egui::StrokeKind::Inside);
                    icons::paint(
                        ui.painter(),
                        egui::Rect::from_center_size(r.center(), vec2(18.0, 18.0)),
                        Icon::Search,
                        Color32::from_gray(40),
                        Color32::WHITE,
                    );
                    if resp.on_hover_text("Search shapes").clicked() {
                        ui.memory_mut(|m| m.request_focus(egui::Id::unique("lib_search")));
                    }
                });
                ui.add_space(4.0);
                ui.add(
                    egui::TextEdit::singleline(&mut self.lib_search)
                        .id(egui::Id::unique("lib_search"))
                        .hint_text("Search shapes")
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(6.0);
                let q = self.lib_search.trim().to_lowercase();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    egui::Grid::new("shape_grid").spacing([6.0, 6.0]).show(ui, |ui| {
                        let mut n = 0;
                        for (kind, hole) in LIBRARY {
                            if !q.is_empty() && !tile_label(kind, hole).to_lowercase().contains(&q) {
                                continue;
                            }
                            self.library_tile(ui, kind, hole);
                            n += 1;
                            if n % 3 == 0 {
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

    fn library_tile(&mut self, ui: &mut egui::Ui, kind: ShapeKind, hole: bool) {
        let size = vec2(82.0, 86.0);
        let thumb = self.thumbs.get(&(kind, hole)).copied();
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
        resp.dnd_set_drag_payload((kind, hole));
        let hovered = resp.hovered() || resp.dragged();
        let p = ui.painter();
        p.rect_filled(rect, 6.0, if hovered { Color32::from_rgb(0xdd, 0xea, 0xf8) } else { Color32::WHITE });
        let img = egui::Rect::from_center_size(rect.center(), vec2(74.0, 74.0));
        if let Some(t) = thumb {
            p.image(t, img, full_uv(), Color32::WHITE);
        }
        let resp = resp.on_hover_text(tile_label(kind, hole));
        if resp.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            // Ghost of the shape under the cursor while dragging.
            if let (Some(pos), Some(t)) = (ui.ctx().pointer_hover_pos(), thumb) {
                let layer = egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("lib_drag_ghost"));
                let r = egui::Rect::from_center_size(pos, vec2(64.0, 64.0));
                ui.ctx().layer_painter(layer).image(t, r, full_uv(), Color32::from_white_alpha(200));
            }
        }
        if resp.clicked() {
            self.add_shape(kind, hole, None);
        }
    }
}
