//! Document model: a tree of nodes, exactly like Tinkercad's scene.
//!
//! A design is a list of top-level nodes. A node is either a parametric
//! shape or a group of other nodes. Groups are never baked: the merged
//! geometry is always recomputed from the children, so any group can be
//! ungrouped back to its parts, all the way down to the original shapes.

use glam::{DMat4, DVec3};
use serde::{Deserialize, Deserialize as De, Deserializer, Serialize, Serializer};

pub type NodeId = u64;

pub const FORMAT_NAME: &str = "orion3d";
pub const FORMAT_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// Transform
// ---------------------------------------------------------------------------

/// Full affine transform. Stored as a 4x4 matrix (column-major, 16 values)
/// so that grouping/ungrouping can compose transforms exactly, including
/// non-uniform scale on rotated objects.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Xform(pub DMat4);

impl Default for Xform {
    fn default() -> Self {
        Xform(DMat4::IDENTITY)
    }
}

impl Xform {
    pub fn translation(v: DVec3) -> Self {
        Xform(DMat4::from_translation(v))
    }

    /// 3x4 column-major layout used by the geometry kernel.
    pub fn to_affine12(&self) -> [f64; 12] {
        let c = self.0.to_cols_array();
        [
            c[0], c[1], c[2], c[4], c[5], c[6], c[8], c[9], c[10], c[12], c[13], c[14],
        ]
    }
}

impl Serialize for Xform {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        // Round away float noise so files stay readable and diff-friendly.
        let arr: Vec<f64> = self
            .0
            .to_cols_array()
            .iter()
            .map(|v| {
                let r = (v * 1e9).round() / 1e9;
                if r == 0.0 { 0.0 } else { r }
            })
            .collect();
        arr.serialize(s)
    }
}

impl<'de> De<'de> for Xform {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v: Vec<f64> = Vec::deserialize(d)?;
        if v.len() != 16 {
            return Err(serde::de::Error::custom("transform must have 16 values"));
        }
        let mut a = [0.0; 16];
        a.copy_from_slice(&v);
        Ok(Xform(DMat4::from_cols_array(&a)))
    }
}

// ---------------------------------------------------------------------------
// Colour
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rgb(pub [u8; 3]);

impl Rgb {
    pub const fn hex(v: u32) -> Self {
        Rgb([(v >> 16) as u8, (v >> 8) as u8, v as u8])
    }
    pub fn to_hex_string(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.0[0], self.0[1], self.0[2])
    }
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().trim_start_matches('#');
        if s.len() != 6 {
            return None;
        }
        u32::from_str_radix(s, 16).ok().map(Rgb::hex)
    }
}

impl Serialize for Rgb {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.to_hex_string().serialize(s)
    }
}

impl<'de> De<'de> for Rgb {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Rgb::parse(&s).ok_or_else(|| serde::de::Error::custom("bad colour"))
    }
}

/// Tinkercad-style colour palette.
pub const PALETTE: [Rgb; 12] = [
    Rgb::hex(0xe1352d), // red
    Rgb::hex(0xf28c28), // orange
    Rgb::hex(0xf7c600), // yellow
    Rgb::hex(0x47a83f), // green
    Rgb::hex(0x14a8a0), // teal
    Rgb::hex(0x2d8ad6), // blue
    Rgb::hex(0x3f51b5), // indigo
    Rgb::hex(0x8e44ad), // purple
    Rgb::hex(0xe05ca0), // pink
    Rgb::hex(0x8b5a2b), // brown
    Rgb::hex(0x9aa0a6), // grey
    Rgb::hex(0x3a3a3a), // charcoal
];

// ---------------------------------------------------------------------------
// Shapes
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilletKind {
    Fillet,
    Chamfer,
}

/// Parametric primitives. All dimensions in millimetres.
/// Every shape is built centred on the origin in X/Y, sitting on Z = 0.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Shape {
    Box { size: [f64; 3], radius: f64, steps: u32 },
    Cylinder { radius: f64, height: f64, sides: u32 },
    Sphere { radius: f64, steps: u32 },
    HalfSphere { radius: f64, steps: u32 },
    Cone { base_radius: f64, top_radius: f64, height: f64, sides: u32 },
    Tube { radius: f64, wall: f64, height: f64, sides: u32 },
    Torus { radius: f64, tube: f64, sides: u32, steps: u32 },
    Roof { size: [f64; 3] },
    Wedge { size: [f64; 3] },
    Pyramid { radius: f64, height: f64, sides: u32 },
    Polygon { radius: f64, height: f64, sides: u32 },
    MetaFillet { radius: f64, height: f64, kind: FilletKind, segments: u32 },
}

/// Library entries in display order.
pub const LIBRARY: [ShapeKind; 12] = [
    ShapeKind::Box,
    ShapeKind::Cylinder,
    ShapeKind::Sphere,
    ShapeKind::HalfSphere,
    ShapeKind::Cone,
    ShapeKind::Pyramid,
    ShapeKind::Roof,
    ShapeKind::Wedge,
    ShapeKind::Tube,
    ShapeKind::Torus,
    ShapeKind::Polygon,
    ShapeKind::MetaFillet,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShapeKind {
    Box,
    Cylinder,
    Sphere,
    HalfSphere,
    Cone,
    Tube,
    Torus,
    Roof,
    Wedge,
    Pyramid,
    Polygon,
    MetaFillet,
}

impl ShapeKind {
    pub fn label(self) -> &'static str {
        match self {
            ShapeKind::Box => "Box",
            ShapeKind::Cylinder => "Cylinder",
            ShapeKind::Sphere => "Sphere",
            ShapeKind::HalfSphere => "Half Sphere",
            ShapeKind::Cone => "Cone",
            ShapeKind::Tube => "Tube",
            ShapeKind::Torus => "Torus",
            ShapeKind::Roof => "Roof",
            ShapeKind::Wedge => "Wedge",
            ShapeKind::Pyramid => "Pyramid",
            ShapeKind::Polygon => "Polygon",
            ShapeKind::MetaFillet => "MetaFillet",
        }
    }

    pub fn default_shape(self) -> Shape {
        match self {
            ShapeKind::Box => Shape::Box { size: [20.0, 20.0, 20.0], radius: 0.0, steps: 10 },
            ShapeKind::Cylinder => Shape::Cylinder { radius: 10.0, height: 20.0, sides: 32 },
            ShapeKind::Sphere => Shape::Sphere { radius: 10.0, steps: 24 },
            ShapeKind::HalfSphere => Shape::HalfSphere { radius: 10.0, steps: 24 },
            ShapeKind::Cone => Shape::Cone { base_radius: 10.0, top_radius: 0.0, height: 20.0, sides: 32 },
            ShapeKind::Tube => Shape::Tube { radius: 10.0, wall: 2.0, height: 20.0, sides: 32 },
            ShapeKind::Torus => Shape::Torus { radius: 10.0, tube: 3.0, sides: 24, steps: 48 },
            ShapeKind::Roof => Shape::Roof { size: [20.0, 20.0, 20.0] },
            ShapeKind::Wedge => Shape::Wedge { size: [20.0, 20.0, 20.0] },
            ShapeKind::Pyramid => Shape::Pyramid { radius: std::f64::consts::SQRT_2 * 10.0, height: 20.0, sides: 4 },
            ShapeKind::Polygon => Shape::Polygon { radius: 10.0, height: 20.0, sides: 6 },
            ShapeKind::MetaFillet => Shape::MetaFillet {
                radius: 20.0,
                height: 8.0,
                kind: FilletKind::Fillet,
                segments: 32,
            },
        }
    }

    pub fn default_color(self) -> Rgb {
        match self {
            ShapeKind::Box => PALETTE[0],
            ShapeKind::Cylinder => PALETTE[1],
            ShapeKind::Sphere => PALETTE[5],
            ShapeKind::HalfSphere => PALETTE[8],
            ShapeKind::Cone => PALETTE[7],
            ShapeKind::Tube => PALETTE[4],
            ShapeKind::Torus => PALETTE[2],
            ShapeKind::Roof => PALETTE[3],
            ShapeKind::Wedge => PALETTE[6],
            ShapeKind::Pyramid => PALETTE[2],
            ShapeKind::Polygon => PALETTE[9],
            ShapeKind::MetaFillet => PALETTE[5],
        }
    }
}

impl Shape {
    pub fn kind(&self) -> ShapeKind {
        match self {
            Shape::Box { .. } => ShapeKind::Box,
            Shape::Cylinder { .. } => ShapeKind::Cylinder,
            Shape::Sphere { .. } => ShapeKind::Sphere,
            Shape::HalfSphere { .. } => ShapeKind::HalfSphere,
            Shape::Cone { .. } => ShapeKind::Cone,
            Shape::Tube { .. } => ShapeKind::Tube,
            Shape::Torus { .. } => ShapeKind::Torus,
            Shape::Roof { .. } => ShapeKind::Roof,
            Shape::Wedge { .. } => ShapeKind::Wedge,
            Shape::Pyramid { .. } => ShapeKind::Pyramid,
            Shape::Polygon { .. } => ShapeKind::Polygon,
            Shape::MetaFillet { .. } => ShapeKind::MetaFillet,
        }
    }
}

// ---------------------------------------------------------------------------
// Nodes
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Content {
    Shape(Shape),
    Group(Vec<Node>),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub name: String,
    #[serde(default)]
    pub hole: bool,
    pub color: Rgb,
    pub transform: Xform,
    pub content: Content,
}

impl Node {
    pub fn is_group(&self) -> bool {
        matches!(self.content, Content::Group(_))
    }

    /// Visit this node and every descendant.
    pub fn walk(&self, f: &mut impl FnMut(&Node)) {
        f(self);
        if let Content::Group(ch) = &self.content {
            for c in ch {
                c.walk(f);
            }
        }
    }

    pub fn walk_mut(&mut self, f: &mut impl FnMut(&mut Node)) {
        f(self);
        if let Content::Group(ch) = &mut self.content {
            for c in ch {
                c.walk_mut(f);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Document
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// Snap grid in mm (0 = off).
    pub snap: f64,
    /// Workplane size in mm (X, Y).
    pub workplane: [f64; 2],
}

impl Default for Settings {
    fn default() -> Self {
        Settings { snap: 1.0, workplane: [200.0, 200.0] }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub format: String,
    pub version: u32,
    pub units: String,
    #[serde(default)]
    pub settings: Settings,
    pub nodes: Vec<Node>,
    #[serde(skip)]
    next_id: NodeId,
}

impl Default for Document {
    fn default() -> Self {
        Document {
            format: FORMAT_NAME.into(),
            version: FORMAT_VERSION,
            units: "mm".into(),
            settings: Settings::default(),
            nodes: Vec::new(),
            next_id: 1,
        }
    }
}

impl Document {
    pub fn alloc_id(&mut self) -> NodeId {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Recompute the id counter after loading.
    pub fn fix_ids(&mut self) {
        let mut max = 0;
        for n in &self.nodes {
            n.walk(&mut |c| max = max.max(c.id));
        }
        self.next_id = max + 1;
    }

    /// Give a node and all its descendants fresh ids (used for copy/paste).
    pub fn reassign_ids(&mut self, node: &mut Node) {
        let mut next = self.next_id;
        node.walk_mut(&mut |n| {
            n.id = next;
            next += 1;
        });
        self.next_id = next;
    }

    pub fn index_of(&self, id: NodeId) -> Option<usize> {
        self.nodes.iter().position(|n| n.id == id)
    }

    pub fn get(&self, id: NodeId) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }

    pub fn get_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }

    pub fn new_shape(&mut self, kind: ShapeKind, at: DVec3) -> Node {
        Node {
            id: self.alloc_id(),
            name: kind.label().to_string(),
            hole: false,
            color: kind.default_color(),
            transform: Xform::translation(at),
            content: Content::Shape(kind.default_shape()),
        }
    }

    /// Group the given top-level nodes. Returns the new group's id.
    /// Children keep their transforms; the group starts at identity, so
    /// nothing moves.
    pub fn group(&mut self, ids: &[NodeId]) -> Option<NodeId> {
        if ids.len() < 2 {
            return None;
        }
        let first_pos = ids.iter().filter_map(|id| self.index_of(*id)).min()?;
        let mut children = Vec::new();
        // Preserve scene order.
        let mut i = 0;
        while i < self.nodes.len() {
            if ids.contains(&self.nodes[i].id) {
                children.push(self.nodes.remove(i));
            } else {
                i += 1;
            }
        }
        let all_holes = children.iter().all(|c| c.hole);
        let color = children
            .iter()
            .find(|c| !c.hole)
            .map(|c| c.color)
            .unwrap_or(PALETTE[10]);
        let id = self.alloc_id();
        let group = Node {
            id,
            name: "Group".into(),
            hole: all_holes,
            color,
            transform: Xform::default(),
            content: Content::Group(children),
        };
        let pos = first_pos.min(self.nodes.len());
        self.nodes.insert(pos, group);
        Some(id)
    }

    /// Ungroup a top-level group, baking the group transform into each
    /// child. Returns the ids of the released children.
    pub fn ungroup(&mut self, id: NodeId) -> Vec<NodeId> {
        let Some(idx) = self.index_of(id) else { return vec![] };
        if !self.nodes[idx].is_group() {
            return vec![];
        }
        let group = self.nodes.remove(idx);
        let Content::Group(children) = group.content else { unreachable!() };
        let mut out = Vec::new();
        // Children keep their own solid/hole state, so ungrouping always
        // gets back exactly the parts that went in.
        for (k, mut c) in children.into_iter().enumerate() {
            c.transform = Xform(group.transform.0 * c.transform.0);
            out.push(c.id);
            self.nodes.insert(idx + k, c);
        }
        out
    }
}
