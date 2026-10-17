//! Application state, top bar, file handling, edit commands and undo.

use crate::doc::{Document, Node, NodeId, Rgb, ShapeKind};
use crate::file;
use crate::geom::{self, GeomCache};
use crate::ops;
use crate::render::camera::Camera;
use crate::render::{Renderer, Target};
use crate::viewport::Drag;
use egui::{Color32, Key, KeyboardShortcut, Modifiers, RichText};
use glam::{DMat4, DVec3};
use std::collections::HashMap;
use std::path::PathBuf;

const UNDO_LIMIT: usize = 200;

#[derive(Clone, Debug, PartialEq)]
pub enum Pending {
    New,
    Open(Option<PathBuf>),
    Close,
}

pub struct OrionApp {
    pub doc: Document,
    pub path: Option<PathBuf>,
    saved: Document,
    stable: Document,
    undo: Vec<Document>,
    redo: Vec<Document>,
    clipboard: Vec<Node>,

    pub selection: Vec<NodeId>,
    pub hovered: Option<NodeId>,
    pub cache: GeomCache,
    pub camera: Camera,
    pub drag: Option<Drag>,

    pub renderer: Option<Renderer>,
    pub view_target: Option<Target>,
    pub thumbs: HashMap<ShapeKind, egui::TextureId>,
    pub thumb_targets: Vec<Target>,

    status: Option<(String, f64)>,
    pending: Option<Pending>,
    allow_close: bool,
    title: String,
}

impl OrionApp {
    pub fn new(cc: &eframe::CreationContext<'_>, open: Option<PathBuf>) -> Self {
        let renderer = cc.wgpu_render_state.as_ref().map(|rs| Renderer::new(&rs.device));
        setup_style(&cc.egui_ctx);
        let doc = Document::default();
        let mut app = OrionApp {
            saved: doc.clone(),
            stable: doc.clone(),
            doc,
            path: None,
            undo: Vec::new(),
            redo: Vec::new(),
            clipboard: Vec::new(),
            selection: Vec::new(),
            hovered: None,
            cache: GeomCache::default(),
            camera: Camera::home(),
            drag: None,
            renderer,
            view_target: None,
            thumbs: HashMap::new(),
            thumb_targets: Vec::new(),
            status: None,
            pending: None,
            allow_close: false,
            title: String::new(),
        };
        if let Some(p) = open {
            app.open_path(p);
        }
        app
    }

    // -----------------------------------------------------------------------
    // Status / dirty
    // -----------------------------------------------------------------------

    pub fn set_status(&mut self, ctx: &egui::Context, msg: impl Into<String>) {
        self.status = Some((msg.into(), ctx.input(|i| i.time)));
    }

    pub fn is_dirty(&self) -> bool {
        self.doc.nodes != self.saved.nodes || self.doc.settings != self.saved.settings
    }

    fn file_name(&self) -> String {
        self.path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled".into())
    }

    // -----------------------------------------------------------------------
    // Undo
    // -----------------------------------------------------------------------

    /// Record the last stable state if the document changed. Called every
    /// frame when the user is not mid-interaction, so drags and text edits
    /// become single undo steps.
    fn flush_history(&mut self) {
        if self.doc != self.stable {
            let prev = std::mem::replace(&mut self.stable, self.doc.clone());
            self.undo.push(prev);
            if self.undo.len() > UNDO_LIMIT {
                self.undo.remove(0);
            }
            self.redo.clear();
        }
    }

    pub fn undo(&mut self) {
        self.drag = None;
        self.flush_history();
        if let Some(prev) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.doc, prev));
            self.stable = self.doc.clone();
            self.prune_selection();
        }
    }

    pub fn redo(&mut self) {
        self.drag = None;
        self.flush_history();
        if let Some(next) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.doc, next));
            self.stable = self.doc.clone();
            self.prune_selection();
        }
    }

    fn prune_selection(&mut self) {
        let doc = &self.doc;
        self.selection.retain(|id| doc.get(*id).is_some());
    }

    fn reset_history(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.stable = self.doc.clone();
        self.saved = self.doc.clone();
    }

    // -----------------------------------------------------------------------
    // File
    // -----------------------------------------------------------------------

    fn request(&mut self, p: Pending) {
        if self.is_dirty() {
            self.pending = Some(p);
        } else {
            self.run_pending(p);
        }
    }

    fn run_pending(&mut self, p: Pending) {
        match p {
            Pending::New => {
                self.doc = Document::default();
                self.path = None;
                self.selection.clear();
                self.camera = Camera::home();
                self.reset_history();
            }
            Pending::Open(path) => {
                let path = path.or_else(|| {
                    rfd::FileDialog::new()
                        .add_filter("Orion3D design", &[file::EXTENSION])
                        .pick_file()
                });
                if let Some(p) = path {
                    self.open_path(p);
                }
            }
            Pending::Close => {
                self.allow_close = true;
            }
        }
    }

    fn open_path(&mut self, p: PathBuf) {
        match file::load(&p) {
            Ok(doc) => {
                self.doc = doc;
                self.path = Some(p);
                self.selection.clear();
                self.reset_history();
                self.fit_all();
                self.status = Some((format!("Opened {}", self.file_name()), 0.0));
            }
            Err(e) => self.status = Some((format!("Could not open: {e}"), 0.0)),
        }
    }

    /// Returns true if saved.
    fn save(&mut self, ctx: &egui::Context, save_as: bool) -> bool {
        let path = if save_as || self.path.is_none() {
            let mut dlg = rfd::FileDialog::new()
                .add_filter("Orion3D design", &[file::EXTENSION])
                .set_file_name(format!("{}.{}", self.file_name().trim_end_matches(".orion3d"), file::EXTENSION));
            if let Some(dir) = self.path.as_ref().and_then(|p| p.parent()) {
                dlg = dlg.set_directory(dir);
            }
            dlg.save_file().map(|p| {
                if p.extension().is_none() { p.with_extension(file::EXTENSION) } else { p }
            })
        } else {
            self.path.clone()
        };
        let Some(path) = path else { return false };
        match file::save(&self.doc, &path) {
            Ok(()) => {
                self.path = Some(path);
                self.saved = self.doc.clone();
                let msg = format!("Saved {}", self.file_name());
                self.set_status(ctx, msg);
                true
            }
            Err(e) => {
                self.set_status(ctx, format!("Save failed: {e}"));
                false
            }
        }
    }

    fn export_stl(&mut self, ctx: &egui::Context) {
        let use_sel = !self.selection.is_empty();
        let nodes: Vec<Node> = self
            .doc
            .nodes
            .iter()
            .filter(|n| !use_sel || self.selection.contains(&n.id))
            .filter(|n| !n.hole)
            .cloned()
            .collect();
        if nodes.is_empty() {
            self.set_status(ctx, "Nothing to export (holes are not exported)");
            return;
        }
        let base = self.file_name().trim_end_matches(".orion3d").to_string();
        let Some(path) = rfd::FileDialog::new()
            .add_filter("STL", &["stl"])
            .set_file_name(format!("{base}.stl"))
            .save_file()
        else {
            return;
        };
        let refs: Vec<&Node> = nodes.iter().collect();
        let data = geom::export_stl(&mut self.cache, &refs);
        match std::fs::write(&path, data) {
            Ok(()) => {
                let what = if use_sel { "selection" } else { "design" };
                self.set_status(ctx, format!("Exported {what} to {}", path.display()));
            }
            Err(e) => self.set_status(ctx, format!("Export failed: {e}")),
        }
    }

    // -----------------------------------------------------------------------
    // Edit commands
    // -----------------------------------------------------------------------

    pub fn add_shape(&mut self, kind: ShapeKind, at: Option<DVec3>) {
        let at = at.unwrap_or_else(|| {
            let t = self.camera.target.as_dvec3();
            DVec3::new(t.x, t.y, 0.0)
        });
        let s = self.doc.settings.snap;
        let at = DVec3::new(ops::snap(at.x, s), ops::snap(at.y, s), at.z);
        let node = self.doc.new_shape(kind, at);
        self.selection = vec![node.id];
        self.doc.nodes.push(node);
    }

    pub fn delete_selection(&mut self) {
        let sel = std::mem::take(&mut self.selection);
        self.doc.nodes.retain(|n| !sel.contains(&n.id));
    }

    fn copy(&mut self) {
        self.clipboard = self
            .doc
            .nodes
            .iter()
            .filter(|n| self.selection.contains(&n.id))
            .cloned()
            .collect();
    }

    fn paste(&mut self, offset: DVec3) {
        if self.clipboard.is_empty() {
            return;
        }
        let mut new_sel = Vec::new();
        for mut n in self.clipboard.clone() {
            self.doc.reassign_ids(&mut n);
            n.transform.0 = DMat4::from_translation(offset) * n.transform.0;
            new_sel.push(n.id);
            self.doc.nodes.push(n);
        }
        self.selection = new_sel;
    }

    pub fn duplicate(&mut self) {
        self.copy();
        self.paste(DVec3::ZERO);
    }

    pub fn group(&mut self) {
        if let Some(id) = self.doc.group(&self.selection) {
            self.selection = vec![id];
        }
    }

    pub fn ungroup(&mut self) {
        let mut new_sel = Vec::new();
        for id in self.selection.clone() {
            let released = self.doc.ungroup(id);
            if released.is_empty() {
                new_sel.push(id);
            } else {
                new_sel.extend(released);
            }
        }
        self.selection = new_sel;
    }

    pub fn set_hole(&mut self, hole: bool) {
        for id in self.selection.clone() {
            if let Some(n) = self.doc.get_mut(id) {
                n.hole = hole;
            }
        }
    }

    pub fn set_color(&mut self, c: Rgb) {
        for id in self.selection.clone() {
            if let Some(n) = self.doc.get_mut(id) {
                // Recolouring a group recolours its parts, like Tinkercad.
                n.walk_mut(&mut |x| x.color = c);
            }
        }
    }

    pub fn mirror(&mut self, axis: usize) {
        let Some((lo, hi)) = ops::bounds_of(&mut self.cache, &self.doc, &self.selection) else { return };
        let mut s = DVec3::ONE;
        s[axis] = -1.0;
        let orig = ops::originals(&self.doc, &self.selection);
        ops::apply(&mut self.doc, &orig, ops::scale_about((lo + hi) * 0.5, s), true);
    }

    pub fn rotate_selection(&mut self, axis: usize, degrees: f64) {
        let Some((lo, hi)) = ops::bounds_of(&mut self.cache, &self.doc, &self.selection) else { return };
        let mut a = DVec3::ZERO;
        a[axis] = 1.0;
        let orig = ops::originals(&self.doc, &self.selection);
        let c = (lo + hi) * 0.5;
        ops::apply(&mut self.doc, &orig, ops::rotate_about(c, a, degrees.to_radians()), true);
        // Keep the object resting at the same height.
        if let Some((lo2, _)) = ops::bounds_of(&mut self.cache, &self.doc, &self.selection) {
            let orig = ops::originals(&self.doc, &self.selection);
            let dz = lo.z - lo2.z;
            ops::apply(&mut self.doc, &orig, DMat4::from_translation(DVec3::new(0.0, 0.0, dz)), false);
        }
    }

    pub fn drop_to_workplane(&mut self) {
        for id in self.selection.clone() {
            let Some(n) = self.doc.get(id) else { continue };
            let (lo, _) = ops::node_bounds(&mut self.cache, &n.clone());
            if let Some(n) = self.doc.get_mut(id) {
                n.transform.0 = DMat4::from_translation(DVec3::new(0.0, 0.0, -lo.z)) * n.transform.0;
            }
        }
    }

    pub fn translate_selection(&mut self, d: DVec3) {
        for id in self.selection.clone() {
            if let Some(n) = self.doc.get_mut(id) {
                n.transform.0 = DMat4::from_translation(d) * n.transform.0;
            }
        }
    }

    pub fn fit_all(&mut self) {
        let ids: Vec<NodeId> = if self.selection.is_empty() {
            self.doc.nodes.iter().map(|n| n.id).collect()
        } else {
            self.selection.clone()
        };
        if let Some((lo, hi)) = ops::bounds_of(&mut self.cache, &self.doc, &ids) {
            self.camera.fit(lo.as_vec3(), hi.as_vec3());
        } else {
            self.camera = Camera::home();
        }
    }

    /// Arrow-key nudge relative to the view direction.
    fn nudge(&mut self, forward: f64, right: f64, up: f64) {
        let step = if self.doc.settings.snap > 0.0 { self.doc.settings.snap } else { 1.0 };
        let eye = self.camera.eye();
        let fwd = self.camera.target - eye;
        // Snap the view direction to the nearest world axis.
        let f = if fwd.x.abs() > fwd.y.abs() {
            DVec3::new(fwd.x.signum() as f64, 0.0, 0.0)
        } else {
            DVec3::new(0.0, fwd.y.signum() as f64, 0.0)
        };
        let r = DVec3::new(f.y, -f.x, 0.0);
        self.translate_selection((f * forward + r * right + DVec3::Z * up) * step);
    }

    // -----------------------------------------------------------------------
    // Keyboard
    // -----------------------------------------------------------------------

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let sc = |m: Modifiers, k: Key| KeyboardShortcut::new(m, k);
        let cmd = Modifiers::COMMAND;
        let cmd_shift = Modifiers::COMMAND | Modifiers::SHIFT;
        let hit = |s: KeyboardShortcut| ctx.input_mut(|i| i.consume_shortcut(&s));

        if hit(sc(cmd_shift, Key::S)) {
            self.save(ctx, true);
        }
        if hit(sc(cmd, Key::S)) {
            self.save(ctx, false);
        }
        if hit(sc(cmd, Key::O)) {
            self.request(Pending::Open(None));
        }
        if hit(sc(cmd, Key::N)) {
            self.request(Pending::New);
        }
        if hit(sc(cmd, Key::E)) {
            self.export_stl(ctx);
        }

        // Everything else is ignored while typing in a field.
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        if hit(sc(cmd_shift, Key::Z)) || hit(sc(cmd, Key::Y)) {
            self.redo();
        }
        if hit(sc(cmd, Key::Z)) {
            self.undo();
        }
        if hit(sc(cmd_shift, Key::G)) {
            self.ungroup();
        }
        if hit(sc(cmd, Key::G)) {
            self.group();
        }
        if hit(sc(cmd, Key::C)) {
            self.copy();
        }
        if hit(sc(cmd, Key::X)) {
            self.copy();
            self.delete_selection();
        }
        if hit(sc(cmd, Key::V)) {
            self.paste(DVec3::new(10.0, 10.0, 0.0));
        }
        if hit(sc(cmd, Key::D)) {
            self.duplicate();
        }
        if hit(sc(cmd, Key::A)) {
            self.selection = self.doc.nodes.iter().map(|n| n.id).collect();
        }
        let plain = Modifiers::NONE;
        if hit(sc(plain, Key::Delete)) || hit(sc(plain, Key::Backspace)) {
            self.delete_selection();
        }
        if hit(sc(plain, Key::Escape)) {
            self.selection.clear();
            self.drag = None;
        }
        if hit(sc(plain, Key::H)) {
            self.set_hole(true);
        }
        if hit(sc(plain, Key::S)) {
            self.set_hole(false);
        }
        if hit(sc(plain, Key::D)) {
            self.drop_to_workplane();
        }
        if hit(sc(plain, Key::F)) {
            self.fit_all();
        }
        if !self.selection.is_empty() {
            let mult = if ctx.input(|i| i.modifiers.shift) { 10.0 } else { 1.0 };
            let lift = ctx.input(|i| i.modifiers.command);
            let held = |k: Key| ctx.input_mut(|i| {
                i.consume_key(Modifiers::NONE, k)
                    || i.consume_key(Modifiers::SHIFT, k)
                    || i.consume_key(Modifiers::COMMAND, k)
                    || i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, k)
            });
            if held(Key::ArrowUp) {
                if lift { self.nudge(0.0, 0.0, mult) } else { self.nudge(mult, 0.0, 0.0) }
            }
            if held(Key::ArrowDown) {
                if lift { self.nudge(0.0, 0.0, -mult) } else { self.nudge(-mult, 0.0, 0.0) }
            }
            if held(Key::ArrowRight) {
                self.nudge(0.0, mult, 0.0);
            }
            if held(Key::ArrowLeft) {
                self.nudge(0.0, -mult, 0.0);
            }
        }
    }

    // -----------------------------------------------------------------------
    // Top bar
    // -----------------------------------------------------------------------

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        egui::Panel::top("top_bar")
            .frame(egui::Frame::new().fill(Color32::from_rgb(0x1e, 0x2a, 0x3a)).inner_margin(egui::Margin::symmetric(10, 6)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("ORION3D").strong().size(16.0).color(Color32::from_rgb(0x7f, 0xc4, 0xff)));
                    ui.add_space(12.0);
                    let has_sel = !self.selection.is_empty();
                    let one_group = self.selection.iter().any(|id| self.doc.get(*id).is_some_and(|n| n.is_group()));

                    if bar_button(ui, "New", true, "Ctrl+N") { self.request(Pending::New); }
                    if bar_button(ui, "Open", true, "Ctrl+O") { self.request(Pending::Open(None)); }
                    if bar_button(ui, "Save", true, "Ctrl+S") { self.save(&ctx, false); }
                    if bar_button(ui, "Save As", true, "Ctrl+Shift+S") { self.save(&ctx, true); }
                    if bar_button(ui, "Export STL", !self.doc.nodes.is_empty(), "Ctrl+E  (exports the selection, or everything if nothing is selected)") { self.export_stl(&ctx); }
                    ui.separator();
                    if bar_button(ui, "Undo", !self.undo.is_empty() || self.doc != self.stable, "Ctrl+Z") { self.undo(); }
                    if bar_button(ui, "Redo", !self.redo.is_empty(), "Ctrl+Y") { self.redo(); }
                    ui.separator();
                    if bar_button(ui, "Copy", has_sel, "Ctrl+C") { self.copy(); }
                    if bar_button(ui, "Paste", !self.clipboard.is_empty(), "Ctrl+V") { self.paste(DVec3::new(10.0, 10.0, 0.0)); }
                    if bar_button(ui, "Duplicate", has_sel, "Ctrl+D") { self.duplicate(); }
                    if bar_button(ui, "Delete", has_sel, "Del") { self.delete_selection(); }
                    ui.separator();
                    if bar_button(ui, "Group", self.selection.len() >= 2, "Ctrl+G") { self.group(); }
                    if bar_button(ui, "Ungroup", one_group, "Ctrl+Shift+G") { self.ungroup(); }
                    ui.separator();
                    ui.label(RichText::new("Mirror").color(Color32::from_gray(170)));
                    for (i, a) in ["X", "Y", "Z"].iter().enumerate() {
                        if bar_button(ui, a, has_sel, "Mirror the selection") { self.mirror(i); }
                    }
                    if bar_button(ui, "Drop", has_sel, "D - drop to workplane") { self.drop_to_workplane(); }
                });
            });

        // Unsaved-changes bar (inline, no modal dialog).
        if let Some(p) = self.pending.clone() {
            egui::Panel::top("pending_bar")
                .frame(egui::Frame::new().fill(Color32::from_rgb(0xff, 0xf1, 0xc4)).inner_margin(egui::Margin::symmetric(12, 6)))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("{} has unsaved changes.", self.file_name())).color(Color32::from_rgb(0x5a, 0x44, 0x00)));
                        if ui.button("Save").clicked() && self.save(&ctx, false) {
                            self.pending = None;
                            self.run_pending(p.clone());
                        }
                        if ui.button("Discard changes").clicked() {
                            self.pending = None;
                            self.run_pending(p.clone());
                        }
                        if ui.button("Cancel").clicked() {
                            self.pending = None;
                        }
                    });
                });
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let now = ui.ctx().input(|i| i.time);
        egui::Panel::bottom("status")
            .frame(egui::Frame::new().fill(Color32::from_rgb(0xee, 0xf1, 0xf5)).inner_margin(egui::Margin::symmetric(10, 4)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if let Some((msg, t)) = &mut self.status {
                        if *t == 0.0 {
                            *t = now;
                        }
                        ui.label(msg.as_str());
                        if now - *t > 8.0 {
                            self.status = None;
                        }
                    } else {
                        ui.label(
                            RichText::new("Left-drag: move / select   Right-drag: orbit   Middle or Shift+Right-drag: pan   Wheel: zoom   F: fit")
                                .color(Color32::from_gray(110)),
                        );
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let snaps = [0.0, 0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0];
                        let cur = self.doc.settings.snap;
                        let label = |v: f64| if v == 0.0 { "Off".to_string() } else { format!("{v} mm") };
                        egui::ComboBox::from_id_salt("snap")
                            .selected_text(label(cur))
                            .show_ui(ui, |ui| {
                                for s in snaps {
                                    if ui.selectable_label(cur == s, label(s)).clicked() {
                                        self.doc.settings.snap = s;
                                    }
                                }
                            });
                        ui.label("Snap grid");
                    });
                });
            });
    }

    fn update_title(&mut self, ctx: &egui::Context) {
        let t = format!("{}{} - Orion3D", self.file_name(), if self.is_dirty() { " *" } else { "" });
        if t != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(t.clone()));
            self.title = t;
        }
    }
}

fn bar_button(ui: &mut egui::Ui, label: &str, enabled: bool, tip: &str) -> bool {
    ui.add_enabled(
        enabled,
        egui::Button::new(RichText::new(label).color(if enabled { Color32::WHITE } else { Color32::from_gray(110) }))
            .fill(Color32::from_rgb(0x2c, 0x3d, 0x52)),
    )
    .on_hover_text(tip)
    .clicked()
}

fn setup_style(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Light);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(6.0, 6.0);
        s.spacing.button_padding = egui::vec2(8.0, 4.0);
        s.visuals.window_corner_radius = egui::CornerRadius::same(8);
    });
}

impl eframe::App for OrionApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        // Intercept window close when there are unsaved changes.
        if ctx.input(|i| i.viewport().close_requested()) && !self.allow_close && self.is_dirty() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.pending = Some(Pending::Close);
        }
        if self.allow_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        self.make_thumbnails(frame);
        self.shortcuts(&ctx);

        self.top_bar(ui);
        self.status_bar(ui);
        self.library_panel(ui);
        egui::CentralPanel::no_frame().show(ui, |ui| {
            self.viewport(ui, frame);
        });

        // History: commit only when the user isn't mid-interaction.
        let busy = ctx.input(|i| i.pointer.any_down()) || ctx.egui_wants_keyboard_input();
        if !busy && self.drag.is_none() {
            self.flush_history();
        }
        self.cache.end_frame();
        self.update_title(&ctx);
    }
}
