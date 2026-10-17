//! Geometry: builds shapes, evaluates the node tree with CSG booleans and
//! produces render/export meshes. The kernel is Manifold (robust mesh
//! booleans, same engine OpenSCAD uses).

use crate::doc::{Content, FilletKind, GroupOp, MeshData, Node, NodeId, Rgb, Shape};
use glam::{DMat4, DVec3, Vec3};
use manifold_csg::{CrossSection, Manifold};
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Shapes
// ---------------------------------------------------------------------------

fn segs(n: u32, min: u32) -> i32 {
    n.clamp(min, 256) as i32
}

fn pos(v: f64) -> f64 {
    v.max(0.01)
}

pub fn build_shape(shape: &Shape) -> Manifold {
    match *shape {
        Shape::Box { size, radius, steps } => {
            let [x, y, z] = size.map(pos);
            let r = radius.min(x.min(y).min(z) * 0.5 - 1e-4);
            if r <= 0.01 {
                Manifold::cube(x, y, z, true).translate(0.0, 0.0, z * 0.5)
            } else {
                let s = Manifold::sphere(r, segs(steps * 4, 8));
                let (hx, hy, hz) = (x * 0.5 - r, y * 0.5 - r, z * 0.5 - r);
                let mut corners = Vec::with_capacity(8);
                for sx in [-1.0, 1.0] {
                    for sy in [-1.0, 1.0] {
                        for sz in [-1.0, 1.0] {
                            corners.push(s.translate(sx * hx, sy * hy, sz * hz));
                        }
                    }
                }
                Manifold::batch_hull(&corners).translate(0.0, 0.0, z * 0.5)
            }
        }
        Shape::Cylinder { radius, height, sides } => {
            Manifold::cylinder(pos(height), pos(radius), pos(radius), segs(sides, 3), false)
        }
        Shape::Polygon { radius, height, sides } => {
            Manifold::cylinder(pos(height), pos(radius), pos(radius), segs(sides, 3), false)
        }
        Shape::Sphere { radius, steps } => {
            let r = pos(radius);
            Manifold::sphere(r, segs(steps, 4)).translate(0.0, 0.0, r)
        }
        Shape::HalfSphere { radius, steps } => {
            let r = pos(radius);
            Manifold::sphere(r, segs(steps, 4)).trim_by_plane([0.0, 0.0, 1.0], 0.0)
        }
        Shape::Cone { base_radius, top_radius, height, sides } => Manifold::cylinder(
            pos(height),
            pos(base_radius),
            top_radius.max(0.0),
            segs(sides, 3),
            false,
        ),
        Shape::Tube { radius, wall, height, sides } => {
            let r = pos(radius);
            let inner = (r - wall.max(0.01)).max(0.0);
            let n = segs(sides, 3);
            let mut cs = CrossSection::circle(r, n);
            if inner > 0.01 {
                cs = cs.difference(&CrossSection::circle(inner, n));
            }
            cs.extrude(pos(height))
        }
        Shape::Torus { radius, tube, sides, steps } => {
            let t = pos(tube).min(pos(radius));
            let ring = CrossSection::circle(t, segs(sides, 3)).translate(pos(radius), 0.0);
            Manifold::revolve(&ring, segs(steps, 3), 360.0).translate(0.0, 0.0, t)
        }
        Shape::Roof { size } => {
            let [x, y, z] = size.map(pos);
            let (hx, hy) = (x * 0.5, y * 0.5);
            Manifold::hull_pts(&[
                [-hx, -hy, 0.0],
                [hx, -hy, 0.0],
                [hx, hy, 0.0],
                [-hx, hy, 0.0],
                [-hx, 0.0, z],
                [hx, 0.0, z],
            ])
        }
        Shape::Wedge { size } => {
            let [x, y, z] = size.map(pos);
            let (hx, hy) = (x * 0.5, y * 0.5);
            Manifold::hull_pts(&[
                [-hx, -hy, 0.0],
                [hx, -hy, 0.0],
                [hx, hy, 0.0],
                [-hx, hy, 0.0],
                [-hx, -hy, z],
                [-hx, hy, z],
            ])
        }
        Shape::Pyramid { radius, height, sides } => {
            let n = sides.clamp(3, 64);
            let r = pos(radius);
            // Rotate so a square pyramid has its edges aligned with X/Y.
            let off = std::f64::consts::PI / n as f64;
            let mut pts: Vec<[f64; 3]> = (0..n)
                .map(|i| {
                    let a = off + i as f64 / n as f64 * std::f64::consts::TAU;
                    [r * a.cos(), r * a.sin(), 0.0]
                })
                .collect();
            pts.push([0.0, 0.0, pos(height)]);
            Manifold::hull_pts(&pts)
        }
        Shape::MetaFillet { radius, height, kind, segments } => {
            // Profile: an R x R square with the corner region removed, so the
            // remaining face is concave (fillet) or flat (chamfer). Extruded
            // by `height`. Recreates the retired Tinkercad MetaFillet generator.
            let r = pos(radius);
            let profile = match kind {
                FilletKind::Fillet => CrossSection::square(r, r, false).difference(
                    &CrossSection::circle(r, segs(segments * 4, 12)).translate(0.0, r),
                ),
                FilletKind::Chamfer => {
                    CrossSection::from_simple_polygon(&[[0.0, 0.0], [r, 0.0], [r, r]])
                }
            };
            profile.extrude(pos(height)).translate(-r * 0.5, -r * 0.5, 0.0)
        }
    }
}

// ---------------------------------------------------------------------------
// Tree evaluation
// ---------------------------------------------------------------------------

/// Hash of a node's content (everything that affects its local geometry,
/// excluding its own transform, colour and hole flag).
pub fn content_hash(content: &Content) -> u64 {
    let mut h = DefaultHasher::new();
    hash_content(content, &mut h);
    h.finish()
}

fn hash_content(content: &Content, h: &mut DefaultHasher) {
    match content {
        Content::Shape(s) => {
            0u8.hash(h);
            // Shapes are tiny; JSON is a simple stable byte form for f64 data.
            serde_json::to_vec(s).unwrap_or_default().hash(h);
        }
        Content::Group(g) => {
            1u8.hash(h);
            g.op.hash(h);
            g.children.len().hash(h);
            for c in &g.children {
                c.hole.hash(h);
                for v in c.transform.0.to_cols_array() {
                    v.to_bits().hash(h);
                }
                hash_content(&c.content, h);
            }
        }
        Content::Mesh(m) => {
            2u8.hash(h);
            m.id.hash(h);
        }
    }
}

/// Mesh ready for drawing and picking, in the node's local space.
pub struct EvalMesh {
    /// Triangle soup: 3 entries per triangle.
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    /// Hard edges (crease > ~25 degrees or open), as segments.
    pub edges: Vec<[Vec3; 2]>,
    pub min: Vec3,
    pub max: Vec3,
}

/// One drawable piece of a top-level node. Bundles produce one item per
/// part (so parts keep their colours); everything else is one item.
#[derive(Clone)]
pub struct Item {
    pub node: NodeId,
    pub key: u64,
    pub mesh: Arc<EvalMesh>,
    pub model: DMat4,
    pub color: Rgb,
    pub hole: bool,
}

#[derive(Default)]
pub struct GeomCache {
    solids: HashMap<u64, Manifold>,
    meshes: HashMap<u64, Arc<EvalMesh>>,
    used: HashMap<u64, u64>,
    frame: u64,
}

/// Collect leaf solids/holes of a group, looking through bundles.
fn gather(
    cache: &mut GeomCache,
    children: &[Node],
    m: DMat4,
    force_hole: bool,
    solids: &mut Vec<Manifold>,
    holes: &mut Vec<Manifold>,
) {
    for c in children {
        let cm = m * c.transform.0;
        let hole = force_hole || c.hole;
        match &c.content {
            Content::Group(g) if g.op == GroupOp::Bundle => {
                gather(cache, &g.children, cm, hole, solids, holes);
            }
            content => {
                let man = cache.content_manifold(content).transform(&crate::doc::Xform(cm).to_affine12());
                if hole {
                    holes.push(man);
                } else {
                    solids.push(man);
                }
            }
        }
    }
}

impl GeomCache {
    /// Local-space manifold for a node's content (cached). An imported mesh
    /// that is not watertight yields an empty manifold (it still renders).
    pub fn content_manifold(&mut self, content: &Content) -> Manifold {
        let key = content_hash(content);
        self.used.insert(key, self.frame);
        if let Some(m) = self.solids.get(&key) {
            return m.clone();
        }
        let m = match content {
            Content::Shape(s) => build_shape(s),
            Content::Mesh(md) => mesh_manifold(md).unwrap_or_else(Manifold::empty),
            Content::Group(g) => {
                let mut solids = Vec::new();
                let mut holes = Vec::new();
                gather(self, &g.children, DMat4::IDENTITY, false, &mut solids, &mut holes);
                match g.op {
                    GroupOp::Bundle => Manifold::batch_union(&solids),
                    _ if solids.is_empty() => {
                        // A group of only holes is itself a hole shape.
                        Manifold::batch_union(&holes)
                    }
                    op => {
                        let s = if op == GroupOp::Intersect {
                            let mut acc = solids[0].clone();
                            for x in &solids[1..] {
                                acc = acc.intersection(x);
                            }
                            acc
                        } else {
                            Manifold::batch_union(&solids)
                        };
                        if holes.is_empty() { s } else { s.difference(&Manifold::batch_union(&holes)) }
                    }
                }
            }
        };
        self.solids.insert(key, m.clone());
        m
    }

    /// Local-space render mesh for a node's content (cached).
    pub fn content_mesh(&mut self, content: &Content) -> (u64, Arc<EvalMesh>) {
        let key = content_hash(content);
        self.used.insert(key, self.frame);
        if let Some(m) = self.meshes.get(&key) {
            return (key, m.clone());
        }
        let m = match content {
            Content::Mesh(md) if mesh_manifold(md).is_none() => {
                let verts: Vec<Vec3> = md.positions.iter().map(|p| Vec3::from_array(*p)).collect();
                Arc::new(soup(&verts, &md.indices))
            }
            _ => Arc::new(to_eval_mesh(&self.content_manifold(content))),
        };
        self.meshes.insert(key, m.clone());
        (key, m)
    }

    /// Drawable items for a top-level node (or a bundle's part).
    pub fn node_items(&mut self, top: NodeId, node: &Node, parent: DMat4, force_hole: bool, out: &mut Vec<Item>) {
        let m = parent * node.transform.0;
        let hole = force_hole || node.hole;
        if let Content::Group(g) = &node.content {
            if g.op == GroupOp::Bundle {
                for c in &g.children {
                    self.node_items(top, c, m, hole, out);
                }
                return;
            }
        }
        let (key, mesh) = self.content_mesh(&node.content);
        out.push(Item { node: top, key, mesh, model: m, color: node.color, hole });
    }

    pub fn items_of(&mut self, node: &Node) -> Vec<Item> {
        let mut v = Vec::new();
        self.node_items(node.id, node, DMat4::IDENTITY, false, &mut v);
        v
    }

    /// Drop entries not used for a while.
    pub fn end_frame(&mut self) {
        self.frame += 1;
        if self.frame % 120 == 0 {
            let cutoff = self.frame.saturating_sub(600);
            let used = &self.used;
            self.solids.retain(|k, _| used.get(k).is_some_and(|f| *f >= cutoff));
            self.meshes.retain(|k, _| used.get(k).is_some_and(|f| *f >= cutoff));
            self.used.retain(|_, f| *f >= cutoff);
        }
    }
}

/// Manifold from an imported mesh, welding duplicate vertices first
/// (STL stores every triangle separately). None if not watertight.
pub fn mesh_manifold(md: &MeshData) -> Option<Manifold> {
    if md.indices.is_empty() {
        return None;
    }
    let mut weld: HashMap<[u32; 3], u64> = HashMap::new();
    let mut verts: Vec<f64> = Vec::new();
    let mut remap = Vec::with_capacity(md.positions.len());
    for p in md.positions.iter() {
        let key = [p[0].to_bits(), p[1].to_bits(), p[2].to_bits()];
        let n = weld.len() as u64;
        let id = *weld.entry(key).or_insert_with(|| {
            verts.extend_from_slice(&[p[0] as f64, p[1] as f64, p[2] as f64]);
            n
        });
        remap.push(id);
    }
    let mut tris: Vec<u64> = Vec::with_capacity(md.indices.len());
    for t in md.indices.chunks_exact(3) {
        let (a, b, c) = (remap[t[0] as usize], remap[t[1] as usize], remap[t[2] as usize]);
        if a != b && b != c && a != c {
            tris.extend_from_slice(&[a, b, c]);
        }
    }
    let m = Manifold::from_mesh_f64(&verts, 3, &tris).ok()?;
    (!m.is_empty()).then_some(m)
}

/// Convert a manifold to a render mesh.
pub fn to_eval_mesh(m: &Manifold) -> EvalMesh {
    let (props, np, idx) = m.to_mesh_f32();
    let vcount = if np == 0 { 0 } else { props.len() / np };
    let verts: Vec<Vec3> = (0..vcount)
        .map(|i| Vec3::new(props[i * np], props[i * np + 1], props[i * np + 2]))
        .collect();
    soup(&verts, &idx)
}

/// Smooth-shaded triangle soup from an indexed mesh. Normals are averaged
/// only across faces within ~35 degrees, so flat faces and hard edges stay
/// crisp while curved surfaces look smooth. Also extracts hard edges.
pub fn soup(verts: &[Vec3], idx: &[u32]) -> EvalMesh {
    let tris = idx.len() / 3;

    // Face normals (area weighted via cross product length).
    let mut fnorm = Vec::with_capacity(tris);
    for t in 0..tris {
        let a = verts[idx[t * 3] as usize];
        let b = verts[idx[t * 3 + 1] as usize];
        let c = verts[idx[t * 3 + 2] as usize];
        fnorm.push((b - a).cross(c - a));
    }

    // Weld by position for adjacency (the kernel may split vertices along
    // property seams, and STL meshes are unwelded).
    let mut weld: HashMap<[u32; 3], u32> = HashMap::new();
    let mut vid = vec![0u32; verts.len()];
    let mut wpos: Vec<Vec3> = Vec::new();
    for (i, v) in verts.iter().enumerate() {
        let key = [v.x.to_bits(), v.y.to_bits(), v.z.to_bits()];
        let n = weld.len() as u32;
        vid[i] = *weld.entry(key).or_insert_with(|| {
            wpos.push(*v);
            n
        });
    }
    let mut vfaces: Vec<Vec<u32>> = vec![Vec::new(); weld.len()];
    let mut efaces: HashMap<(u32, u32), Vec<u32>> = HashMap::new();
    for t in 0..tris {
        let w = [vid[idx[t * 3] as usize], vid[idx[t * 3 + 1] as usize], vid[idx[t * 3 + 2] as usize]];
        for k in 0..3 {
            vfaces[w[k] as usize].push(t as u32);
            let (a, b) = (w[k], w[(k + 1) % 3]);
            efaces.entry((a.min(b), a.max(b))).or_default().push(t as u32);
        }
    }

    let crease = 25f32.to_radians().cos();
    let mut edges = Vec::new();
    for ((a, b), fs) in &efaces {
        let hard = if fs.len() == 2 {
            let n0 = fnorm[fs[0] as usize].normalize_or_zero();
            let n1 = fnorm[fs[1] as usize].normalize_or_zero();
            n0.dot(n1) < crease
        } else {
            true
        };
        if hard && a != b {
            edges.push([wpos[*a as usize], wpos[*b as usize]]);
        }
    }

    let cos_limit = 35f32.to_radians().cos();
    let mut positions = Vec::with_capacity(tris * 3);
    let mut normals = Vec::with_capacity(tris * 3);
    let mut min = Vec3::splat(f32::MAX);
    let mut max = Vec3::splat(f32::MIN);
    for t in 0..tris {
        let fn_t = fnorm[t].normalize_or_zero();
        for k in 0..3 {
            let vi = idx[t * 3 + k] as usize;
            let p = verts[vi];
            let mut n = Vec3::ZERO;
            for &f in &vfaces[vid[vi] as usize] {
                let nf = fnorm[f as usize];
                if nf.normalize_or_zero().dot(fn_t) >= cos_limit {
                    n += nf;
                }
            }
            let n = n.normalize_or(fn_t);
            positions.push(p);
            normals.push(n);
            min = min.min(p);
            max = max.max(p);
        }
    }
    if positions.is_empty() {
        min = Vec3::ZERO;
        max = Vec3::ZERO;
    }
    EvalMesh { positions, normals, edges, min, max }
}

/// World-space axis-aligned bounds of a local mesh under a transform.
pub fn world_bounds(mesh: &EvalMesh, m: &DMat4) -> Option<(DVec3, DVec3)> {
    if mesh.positions.is_empty() {
        return None;
    }
    let mut lo = DVec3::splat(f64::MAX);
    let mut hi = DVec3::splat(f64::MIN);
    // Use actual vertices so rotated round shapes get tight bounds.
    let step = (mesh.positions.len() / 20000).max(1);
    for p in mesh.positions.iter().step_by(step) {
        let w = m.transform_point3(p.as_dvec3());
        lo = lo.min(w);
        hi = hi.max(w);
    }
    Some((lo, hi))
}

/// Ray / mesh intersection in local space. Returns (distance along the ray,
/// triangle index).
pub fn ray_hit(mesh: &EvalMesh, origin: Vec3, dir: Vec3) -> Option<(f32, usize)> {
    // Slab test against the bounds first.
    let inv = dir.recip();
    let t1 = (mesh.min - origin) * inv;
    let t2 = (mesh.max - origin) * inv;
    let tmin = t1.min(t2).max_element();
    let tmax = t1.max(t2).min_element();
    if tmax < tmin.max(0.0) {
        return None;
    }
    let mut best: Option<(f32, usize)> = None;
    for (i, tri) in mesh.positions.chunks_exact(3).enumerate() {
        // Moller-Trumbore, double sided.
        let e1 = tri[1] - tri[0];
        let e2 = tri[2] - tri[0];
        let p = dir.cross(e2);
        let det = e1.dot(p);
        if det.abs() < 1e-12 {
            continue;
        }
        let inv_det = 1.0 / det;
        let s = origin - tri[0];
        let u = s.dot(p) * inv_det;
        if !(0.0..=1.0).contains(&u) {
            continue;
        }
        let q = s.cross(e1);
        let v = dir.dot(q) * inv_det;
        if v < 0.0 || u + v > 1.0 {
            continue;
        }
        let t = e2.dot(q) * inv_det;
        if t > 1e-6 && best.is_none_or(|b| t < b.0) {
            best = Some((t, i));
        }
    }
    best
}

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------

/// A world-space indexed mesh ready for writing to a file.
pub struct ExportMesh {
    pub name: String,
    pub color: Rgb,
    pub positions: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
}

fn export_leaf(cache: &mut GeomCache, name: &str, color: Rgb, content: &Content, m: DMat4, out: &mut Vec<ExportMesh>) {
    let man = cache.content_manifold(content);
    let (positions, indices) = if !man.is_empty() {
        let w = man.transform(&crate::doc::Xform(m).to_affine12());
        let (props, np, idx) = w.to_mesh_f32();
        let positions = props.chunks_exact(np).map(|c| [c[0], c[1], c[2]]).collect();
        (positions, idx)
    } else if let Content::Mesh(md) = content {
        // Not watertight: export the raw triangles.
        let positions: Vec<[f32; 3]> = md
            .positions
            .iter()
            .map(|p| m.transform_point3(DVec3::new(p[0] as f64, p[1] as f64, p[2] as f64)).as_vec3().to_array())
            .collect();
        let mut indices = md.indices.to_vec();
        if m.determinant() < 0.0 {
            for t in indices.chunks_exact_mut(3) {
                t.swap(1, 2);
            }
        }
        (positions, indices)
    } else {
        return;
    };
    if !indices.is_empty() {
        out.push(ExportMesh { name: name.to_string(), color, positions, indices });
    }
}

fn export_node(cache: &mut GeomCache, node: &Node, parent: DMat4, force_hole: bool, out: &mut Vec<ExportMesh>) {
    let m = parent * node.transform.0;
    let hole = force_hole || node.hole;
    if let Content::Group(g) = &node.content {
        if g.op == GroupOp::Bundle {
            for c in &g.children {
                export_node(cache, c, m, hole, out);
            }
            return;
        }
    }
    if !hole {
        export_leaf(cache, &node.name, node.color, &node.content, m, out);
    }
}

/// World-space meshes for the given top-level nodes (holes and hidden
/// objects are skipped, like Tinkercad).
pub fn export_meshes(cache: &mut GeomCache, nodes: &[&Node]) -> Vec<ExportMesh> {
    let mut out = Vec::new();
    for n in nodes.iter().filter(|n| !n.hidden) {
        export_node(cache, n, DMat4::IDENTITY, false, &mut out);
    }
    out
}
