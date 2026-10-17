//! Application state, header and toolbar, file handling, edit commands and
//! undo.

use crate::doc::{Document, GroupOp, MeshData, Node, NodeId, Rgb, ShapeKind};
use crate::file;
use crate::geom::{self, GeomCache};
use crate::icons::{self, Icon};
use crate::meshio;
use crate::ops;
use crate::render::camera::Camera;
use crate::render::{Renderer, Target};
use crate::viewport::{Drag, MeasureState, Tool};
use egui::{Color32, Key, KeyboardShortcut, Modifiers, RichText, vec2};
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ExportScope {
    Everything,
    Selection,
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
    pub tool: Tool,
    pub measure: MeasureState,

    pub renderer: Option<Renderer>,
    pub view_target: Option<Target>,
    pub thumbs: HashMap<(ShapeKind, bool), egui::TextureId>,
    pub thumb_targets: Vec<Target>,

    // UI state.
    pub lib_search: String,
    pub inspector_open: bool,
    pub palette_open: bool,
    export_open: bool,
    export_scope: ExportScope,

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
            tool: Tool::Select,
            measure: MeasureState::default(),
            renderer,
            view_target: None,
            thumbs: HashMap::new(),
            thumb_targets: Vec::new(),
            lib_search: String::new(),
            inspector_open: true,
            palette_open: false,
            export_open: false,
            export_scope: ExportScope::Everything,
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

    fn base_name(&self) -> String {
        self.file_name().trim_end_matches(".orion3d").to_string()
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
        self.selection.retain(|id| doc.get(*id).is_some_and(|n| !n.hidden));
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
                self.tool = Tool::Select;
                self.camera = Camera::home();
                self.reset_history();
            }
            Pending::Open(path) => {
                let path = path.or_else(|| {
                    rfd::FileDialog::new().add_filter("Orion3D design", &[file::EXTENSION]).pick_file()
                });
                if let Some(p) = path {
                    self.open_path(p);
                }
            }
            Pending::Close => self.allow_close = true,
        }
    }

    fn open_path(&mut self, p: PathBuf) {
        match file::load(&p) {
            Ok(doc) => {
                self.doc = doc;
                self.path = Some(p);
                self.selection.clear();
                self.tool = Tool::Select;
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
                .set_file_name(format!("{}.{}", self.base_name(), file::EXTENSION));
            if let Some(dir) = self.path.as_ref().and_then(|p| p.parent()) {
                dlg = dlg.set_directory(dir);
            }
            dlg.save_file().map(|p| if p.extension().is_none() { p.with_extension(file::EXTENSION) } else { p })
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

    fn export(&mut self, ctx: &egui::Context, format: meshio::Format) {
        let use_sel = self.export_scope == ExportScope::Selection && !self.selection.is_empty();
        let nodes: Vec<Node> = self
            .doc
            .nodes
            .iter()
            .filter(|n| !use_sel || self.selection.contains(&n.id))
            .cloned()
            .collect();
        let refs: Vec<&Node> = nodes.iter().collect();
        let meshes = geom::export_meshes(&mut self.cache, &refs);
        if meshes.is_empty() {
            self.set_status(ctx, "Nothing to export (holes and hidden objects are not exported)");
            return;
        }
        let Some(path) = rfd::FileDialog::new()
            .add_filter(format.label(), &[format.ext()])
            .set_file_name(format!("{}.{}", self.base_name(), format.ext()))
            .save_file()
        else {
            return;
        };
        let path = if path.extension().is_none() { path.with_extension(format.ext()) } else { path };
        match meshio::write(format, &meshes).and_then(|d| std::fs::write(&path, d).map_err(|e| e.to_string())) {
            Ok(()) => {
                self.export_open = false;
                self.set_status(ctx, format!("Exported to {}", path.display()));
            }
            Err(e) => self.set_status(ctx, format!("Export failed: {e}")),
        }
    }

    fn import(&mut self, ctx: &egui::Context) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("3D models (STL, OBJ, 3MF)", &["stl", "obj", "3mf"])
            .pick_file()
        else {
            return;
        };
        match meshio::import(&path) {
            Ok(parts) => {
                let mut ids = Vec::new();
                let mut broken = 0;
                for part in parts {
                    let md = MeshData::new(part.positions, part.indices);
                    if geom::mesh_manifold(&md).is_none() {
                        broken += 1;
                    }
                    let mut n = self.doc.new_mesh(&part.name, md, DVec3::ZERO);
                    if let Some(c) = part.color {
                        n.color = c;
                    }
                    ids.push(n.id);
                    self.doc.nodes.push(n);
                }
                // Several parts come in as one bundle, like a Tinkercad import.
                let id = if ids.len() > 1 { self.doc.group(&ids, GroupOp::Bundle).unwrap() } else { ids[0] };
                if let Some(n) = self.doc.get_mut(id) {
                    n.name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                }
                // Centre on the workplane, resting on it.
                self.selection = vec![id];
                if let Some((lo, hi)) = ops::bounds_of(&mut self.cache, &self.doc, &[id]) {
                    let c = (lo + hi) * 0.5;
                    self.translate_selection(DVec3::new(-c.x, -c.y, -lo.z));
                }
                self.fit_all();
                let mut msg = format!("Imported {}", path.display());
                if broken > 0 {
                    msg.push_str(" (not watertight: it can be moved and exported, but holes won't cut it)");
                }
                self.set_status(ctx, msg);
            }
            Err(e) => self.set_status(ctx, format!("Import failed: {e}")),
        }
    }

    // -----------------------------------------------------------------------
    // Edit commands
    // -----------------------------------------------------------------------

    pub fn add_shape(&mut self, kind: ShapeKind, hole: bool, at: Option<DVec3>) {
        let at = at.unwrap_or_else(|| {
            let t = self.camera.target.as_dvec3();
            DVec3::new(t.x, t.y, 0.0)
        });
        let s = self.doc.settings.snap;
        let at = DVec3::new(ops::snap(at.x, s), ops::snap(at.y, s), at.z);
        let mut node = self.doc.new_shape(kind, at);
        node.hole = hole;
        self.selection = vec![node.id];
        self.tool = Tool::Select;
        self.doc.nodes.push(node);
    }

    fn editable_selection(&self) -> Vec<NodeId> {
        self.selection.iter().copied().filter(|id| self.doc.get(*id).is_some_and(|n| !n.locked)).collect()
    }

    pub fn delete_selection(&mut self) {
        let del = self.editable_selection();
        self.doc.nodes.retain(|n| !del.contains(&n.id));
        self.selection.retain(|id| !del.contains(id));
    }

    fn copy(&mut self) {
        self.clipboard = self.doc.nodes.iter().filter(|n| self.selection.contains(&n.id)).cloned().collect();
    }

    fn paste(&mut self, offset: DVec3) {
        if self.clipboard.is_empty() {
            return;
        }
        let mut new_sel = Vec::new();
        for mut n in self.clipboard.clone() {
            self.doc.reassign_ids(&mut n);
            n.transform.0 = DMat4::from_translation(offset) * n.transform.0;
            n.locked = false;
            new_sel.push(n.id);
            self.doc.nodes.push(n);
        }
        self.selection = new_sel;
    }

    pub fn duplicate(&mut self) {
        let keep = std::mem::take(&mut self.clipboard);
        self.copy();
        self.paste(DVec3::ZERO);
        self.clipboard = keep;
    }

    pub fn group(&mut self, op: GroupOp) {
        if self.selection.len() == 1 {
            // Changing the type of an existing group.
            if let Some(n) = self.doc.get_mut(self.selection[0]) {
                if let crate::doc::Content::Group(g) = &mut n.content {
                    g.op = op;
                    n.name = op.label().into();
                }
            }
            return;
        }
        if self.selection_has_locked() {
            return;
        }
        if let Some(id) = self.doc.group(&self.selection, op) {
            self.selection = vec![id];
        }
    }

    pub fn ungroup(&mut self) {
        let mut new_sel = Vec::new();
        for id in self.selection.clone() {
            let locked = self.doc.get(id).is_some_and(|n| n.locked);
            let released = if locked { Vec::new() } else { self.doc.ungroup(id) };
            if released.is_empty() {
                new_sel.push(id);
            } else {
                new_sel.extend(released);
            }
        }
        self.selection = new_sel;
    }

    fn selection_has_locked(&self) -> bool {
        self.selection.iter().any(|id| self.doc.get(*id).is_some_and(|n| n.locked))
    }

    pub fn set_hole(&mut self, hole: bool) {
        for id in self.editable_selection() {
            if let Some(n) = self.doc.get_mut(id) {
                n.hole = hole;
            }
        }
    }

    pub fn set_color(&mut self, c: Rgb) {
        for id in self.editable_selection() {
            if let Some(n) = self.doc.get_mut(id) {
                // Recolouring a group recolours its parts, like Tinkercad.
                n.walk_mut(&mut |x| x.color = c);
                n.hole = false;
            }
        }
    }

    pub fn toggle_lock(&mut self) {
        let lock = !self.selection_has_locked();
        for id in self.selection.clone() {
            if let Some(n) = self.doc.get_mut(id) {
                n.locked = lock;
            }
        }
    }

    pub fn hide_selection(&mut self) {
        for id in std::mem::take(&mut self.selection) {
            if let Some(n) = self.doc.get_mut(id) {
                n.hidden = true;
            }
        }
    }

    pub fn show_all(&mut self) {
        for n in &mut self.doc.nodes {
            n.hidden = false;
        }
    }

    fn any_hidden(&self) -> bool {
        self.doc.nodes.iter().any(|n| n.hidden)
    }

    pub fn mirror(&mut self, axis: usize) {
        let ids = self.editable_selection();
        let Some((lo, hi)) = ops::bounds_of(&mut self.cache, &self.doc, &ids) else { return };
        let mut s = DVec3::ONE;
        s[axis] = -1.0;
        let orig = ops::originals(&self.doc, &ids);
        ops::apply(&mut self.doc, &orig, ops::scale_about((lo + hi) * 0.5, s), true);
    }

    pub fn drop_to_workplane(&mut self) {
        for id in self.editable_selection() {
            let Some(n) = self.doc.get(id).cloned() else { continue };
            let (lo, _) = ops::node_bounds(&mut self.cache, &n);
            if let Some(n) = self.doc.get_mut(id) {
                n.transform.0 = DMat4::from_translation(DVec3::new(0.0, 0.0, -lo.z)) * n.transform.0;
            }
        }
    }

    pub fn translate_selection(&mut self, d: DVec3) {
        for id in self.editable_selection() {
            if let Some(n) = self.doc.get_mut(id) {
                n.transform.0 = DMat4::from_translation(d) * n.transform.0;
            }
        }
    }

    pub fn fit_all(&mut self) {
        let ids: Vec<NodeId> = if self.selection.is_empty() {
            self.doc.nodes.iter().filter(|n| !n.hidden).map(|n| n.id).collect()
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
        let fwd = self.camera.target - self.camera.eye();
        // Snap the view direction to the nearest world axis.
        let f = if fwd.x.abs() > fwd.y.abs() {
            DVec3::new(fwd.x.signum() as f64, 0.0, 0.0)
        } else {
            DVec3::new(0.0, fwd.y.signum() as f64, 0.0)
        };
        let r = DVec3::new(f.y, -f.x, 0.0);
        self.translate_selection((f * forward + r * right + DVec3::Z * up) * step);
    }

    fn set_tool(&mut self, ctx: &egui::Context, tool: Tool) {
        if self.tool == tool || (matches!(self.tool, Tool::Align { .. }) && matches!(tool, Tool::Align { .. })) {
            self.tool = Tool::Select;
            return;
        }
        match tool {
            Tool::Align { .. } if self.selection.len() < 2 => {
                self.set_status(ctx, "Select two or more objects to align");
            }
            Tool::Mirror if self.selection.is_empty() => self.set_status(ctx, "Select something to mirror"),
            Tool::Measure => {
                self.measure.clear();
                self.tool = tool;
            }
            t => self.tool = t,
        }
    }

    // -----------------------------------------------------------------------
    // Keyboard
    // -----------------------------------------------------------------------

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let sc = |m: Modifiers, k: Key| KeyboardShortcut::new(m, k);
        let cmd = Modifiers::COMMAND;
        let cmd_shift = Modifiers::COMMAND | Modifiers::SHIFT;
        let plain = Modifiers::NONE;
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
            self.export_open = !self.export_open;
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
            self.group(GroupOp::Union);
        }
        if hit(sc(cmd, Key::B)) {
            self.group(GroupOp::Bundle);
        }
        if hit(sc(cmd, Key::I)) {
            self.group(GroupOp::Intersect);
        }
        if hit(sc(cmd, Key::L)) {
            self.toggle_lock();
        }
        if hit(sc(cmd_shift, Key::H)) {
            self.show_all();
        }
        if hit(sc(cmd, Key::H)) {
            self.hide_selection();
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
            self.selection = self.doc.nodes.iter().filter(|n| !n.hidden).map(|n| n.id).collect();
        }
        if hit(sc(plain, Key::Delete)) || hit(sc(plain, Key::Backspace)) {
            self.delete_selection();
        }
        if hit(sc(plain, Key::Escape)) {
            if self.tool != Tool::Select {
                self.tool = Tool::Select;
            } else {
                self.selection.clear();
            }
            self.drag = None;
            self.export_open = false;
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
        if hit(sc(plain, Key::L)) {
            self.set_tool(ctx, Tool::Align { key: None });
        }
        if hit(sc(plain, Key::M)) {
            self.set_tool(ctx, Tool::Mirror);
        }
        if hit(sc(plain, Key::R)) {
            self.set_tool(ctx, Tool::Measure);
        }
        if hit(sc(plain, Key::Plus)) || hit(sc(plain, Key::Equals)) || hit(sc(Modifiers::SHIFT, Key::Equals)) {
            self.camera.zoom(0.8, None);
        }
        if hit(sc(plain, Key::Minus)) {
            self.camera.zoom(1.25, None);
        }
        if !self.selection.is_empty() {
            let mult = if ctx.input(|i| i.modifiers.shift) { 10.0 } else { 1.0 };
            let lift = ctx.input(|i| i.modifiers.command);
            let held = |k: Key| {
                ctx.input_mut(|i| {
                    i.consume_key(Modifiers::NONE, k)
                        || i.consume_key(Modifiers::SHIFT, k)
                        || i.consume_key(Modifiers::COMMAND, k)
                        || i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, k)
                })
            };
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
    // Header and toolbar
    // -----------------------------------------------------------------------

    fn header(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        egui::Panel::top("header")
            .frame(egui::Frame::new().fill(Color32::WHITE).inner_margin(egui::Margin::symmetric(12, 8)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    logo(ui);
                    ui.add_space(10.0);
                    let name = format!("{}{}", self.base_name(), if self.is_dirty() { " *" } else { "" });
                    egui::Frame::new()
                        .stroke(egui::Stroke::new(1.0, Color32::from_gray(215)))
                        .corner_radius(14)
                        .inner_margin(egui::Margin::symmetric(12, 4))
                        .show(ui, |ui| ui.label(RichText::new(name).size(14.0).color(Color32::from_gray(50))));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        for (label, tip, action) in [
                            ("Save As", "Ctrl+Shift+S", 3),
                            ("Save", "Ctrl+S", 2),
                            ("Open", "Ctrl+O", 1),
                            ("New", "Ctrl+N", 0),
                        ] {
                            if ui.add(egui::Button::new(label).min_size(vec2(56.0, 26.0))).on_hover_text(tip).clicked() {
                                match action {
                                    0 => self.request(Pending::New),
                                    1 => self.request(Pending::Open(None)),
                                    2 => {
                                        self.save(&ctx, false);
                                    }
                                    _ => {
                                        self.save(&ctx, true);
                                    }
                                }
                            }
                        }
                    });
                });
            });

        egui::Panel::top("toolbar")
            .frame(
                egui::Frame::new()
                    .fill(Color32::WHITE)
                    .stroke(egui::Stroke::new(1.0, Color32::from_gray(225)))
                    .inner_margin(egui::Margin::symmetric(10, 4)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing = vec2(2.0, 0.0);
                    let has_sel = !self.selection.is_empty();
                    let editable = has_sel && !self.selection_has_locked();
                    if icons::button(ui, Icon::Copy, has_sel, "Copy (Ctrl+C)").clicked() {
                        self.copy();
                    }
                    if icons::button(ui, Icon::Paste, !self.clipboard.is_empty(), "Paste (Ctrl+V)").clicked() {
                        self.paste(DVec3::new(10.0, 10.0, 0.0));
                    }
                    if icons::button(ui, Icon::Duplicate, has_sel, "Duplicate (Ctrl+D)").clicked() {
                        self.duplicate();
                    }
                    if icons::button(ui, Icon::Delete, editable, "Delete (Del)").clicked() {
                        self.delete_selection();
                    }
                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(8.0);
                    if icons::button(ui, Icon::Undo, !self.undo.is_empty() || self.doc != self.stable, "Undo (Ctrl+Z)").clicked() {
                        self.undo();
                    }
                    if icons::button(ui, Icon::Redo, !self.redo.is_empty(), "Redo (Ctrl+Y)").clicked() {
                        self.redo();
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing = vec2(4.0, 0.0);
                        if ui.add(egui::Button::new("Export").min_size(vec2(72.0, 28.0))).on_hover_text("Ctrl+E").clicked() {
                            self.export_open = !self.export_open;
                        }
                        if ui.add(egui::Button::new("Import").min_size(vec2(72.0, 28.0))).on_hover_text("STL, OBJ or 3MF").clicked() {
                            self.import(&ui.ctx().clone());
                        }
                        ui.add_space(10.0);
                        let measuring = self.tool == Tool::Measure;
                        let r = icons::button(ui, Icon::Measure, true, "Measure edges, circles and faces (R)");
                        if measuring {
                            ui.painter().rect_stroke(r.rect, 4.0, (1.5, crate::viewport::ACCENT), egui::StrokeKind::Inside);
                        }
                        if r.clicked() {
                            self.set_tool(&ui.ctx().clone(), Tool::Measure);
                        }
                        let r = icons::button(ui, Icon::Mirror, editable, "Mirror (M)");
                        if self.tool == Tool::Mirror {
                            ui.painter().rect_stroke(r.rect, 4.0, (1.5, crate::viewport::ACCENT), egui::StrokeKind::Inside);
                        }
                        if r.clicked() {
                            self.set_tool(&ui.ctx().clone(), Tool::Mirror);
                        }
                        let r = icons::button(ui, Icon::Align, self.selection.len() >= 2, "Align (L)");
                        if matches!(self.tool, Tool::Align { .. }) {
                            ui.painter().rect_stroke(r.rect, 4.0, (1.5, crate::viewport::ACCENT), egui::StrokeKind::Inside);
                        }
                        if r.clicked() {
                            self.set_tool(&ui.ctx().clone(), Tool::Align { key: None });
                        }
                        ui.add_space(6.0);
                        let any_group = self.selection.iter().any(|id| self.doc.get(*id).is_some_and(|n| n.is_group()));
                        if icons::button(ui, Icon::Ungroup, any_group, "Ungroup (Ctrl+Shift+G)").clicked() {
                            self.ungroup();
                        }
                        let can_group = self.selection.len() >= 2 || any_group;
                        if icons::button(ui, Icon::Intersect, can_group, "Intersect group (Ctrl+I)").clicked() {
                            self.group(GroupOp::Intersect);
                        }
                        if icons::button(ui, Icon::Union, can_group, "Union group (Ctrl+G)").clicked() {
                            self.group(GroupOp::Union);
                        }
                        if icons::button(ui, Icon::Bundle, can_group, "Bundle group (Ctrl+B)").clicked() {
                            self.group(GroupOp::Bundle);
                        }
                        ui.add_space(6.0);
                        if icons::button(ui, Icon::Bulb, self.any_hidden(), "Show all (Ctrl+Shift+H)").clicked() {
                            self.show_all();
                        }
                    });
                });
            });

        // Unsaved-changes bar (inline, no modal dialog).
        if let Some(p) = self.pending.clone() {
            egui::Panel::top("pending_bar")
                .frame(egui::Frame::new().fill(Color32::from_rgb(0xff, 0xf1, 0xc4)).inner_margin(egui::Margin::symmetric(12, 6)))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("{} has unsaved changes.", self.file_name()))
                                .color(Color32::from_rgb(0x5a, 0x44, 0x00)),
                        );
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

    /// Export choices, as a panel under the Export button (not a dialog).
    fn export_panel(&mut self, ctx: &egui::Context) {
        if !self.export_open {
            return;
        }
        let screen = ctx.content_rect();
        egui::Area::new(egui::Id::unique("export_panel"))
            .fixed_pos(egui::pos2(screen.max.x - 16.0, 96.0))
            .pivot(egui::Align2::RIGHT_TOP)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).inner_margin(14).show(ui, |ui| {
                    ui.set_width(300.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Export").strong().size(16.0));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("Close").clicked() {
                                self.export_open = false;
                            }
                        });
                    });
                    ui.add_space(6.0);
                    ui.label(RichText::new("Include").color(Color32::from_gray(90)));
                    ui.radio_value(&mut self.export_scope, ExportScope::Everything, "Everything in the design");
                    ui.add_enabled_ui(!self.selection.is_empty(), |ui| {
                        ui.radio_value(&mut self.export_scope, ExportScope::Selection, "Selected shapes");
                    });
                    if self.selection.is_empty() {
                        self.export_scope = ExportScope::Everything;
                    }
                    ui.add_space(8.0);
                    ui.label(RichText::new("For 3D printing and other apps").color(Color32::from_gray(90)));
                    for f in [meshio::Format::Stl, meshio::Format::ThreeMf, meshio::Format::Obj] {
                        ui.horizontal(|ui| {
                            if ui.add(egui::Button::new(RichText::new(f.label()).strong()).min_size(vec2(70.0, 30.0))).clicked() {
                                self.export(&ui.ctx().clone(), f);
                            }
                            ui.label(RichText::new(f.hint()).size(12.0).color(Color32::from_gray(100)));
                        });
                    }
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new("Holes and hidden objects are not exported.").size(11.0).color(Color32::from_gray(120)),
                    );
                });
            });
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
                        if now - *t > 10.0 {
                            self.status = None;
                        }
                    } else {
                        ui.label(
                            RichText::new(
                                "Right-drag or Ctrl+Left-drag: orbit    Middle, Shift+Right or Ctrl+Shift+Left-drag: pan    Wheel: zoom    F: fit",
                            )
                            .color(Color32::from_gray(110)),
                        );
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let snaps = [0.0, 0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0];
                        let cur = self.doc.settings.snap;
                        let label = |v: f64| if v == 0.0 { "Off".to_string() } else { format!("{v:.1} mm") };
                        egui::ComboBox::from_id_salt("snap").selected_text(label(cur)).show_ui(ui, |ui| {
                            for s in snaps {
                                if ui.selectable_label(cur == s, label(s)).clicked() {
                                    self.doc.settings.snap = s;
                                }
                            }
                        });
                        ui.label("Snap Grid");
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

/// Four coloured tiles, a nod to Tinkercad's logo.
fn logo(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(26.0, 26.0), egui::Sense::hover());
    let cols = [
        Color32::from_rgb(0xe1, 0x35, 0x2d),
        Color32::from_rgb(0xf2, 0x8c, 0x28),
        Color32::from_rgb(0x2d, 0x8a, 0xd6),
        Color32::from_rgb(0x47, 0xa8, 0x3f),
    ];
    for (i, c) in cols.iter().enumerate() {
        let x = rect.min.x + (i % 2) as f32 * 13.0;
        let y = rect.min.y + (i / 2) as f32 * 13.0;
        ui.painter().rect_filled(egui::Rect::from_min_size(egui::pos2(x, y), vec2(12.0, 12.0)), 1.5, *c);
    }
    ui.label(RichText::new("ORION3D").strong().size(15.0).color(Color32::from_gray(40)));
}

fn setup_style(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Light);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(6.0, 6.0);
        s.spacing.button_padding = egui::vec2(8.0, 4.0);
        s.visuals.window_corner_radius = egui::CornerRadius::same(8);
        s.visuals.selection.bg_fill = Color32::from_rgb(0x5c, 0xa8, 0xe8);
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

        self.header(ui);
        self.status_bar(ui);
        self.library_panel(ui);
        egui::CentralPanel::no_frame().show(ui, |ui| {
            self.viewport(ui, frame);
        });
        self.export_panel(&ctx);

        // History: commit only when the user isn't mid-interaction.
        let busy = ctx.input(|i| i.pointer.any_down()) || ctx.egui_wants_keyboard_input();
        if !busy && self.drag.is_none() {
            self.flush_history();
        }
        self.cache.end_frame();
        self.update_title(&ctx);
    }
}
