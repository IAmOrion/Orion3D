//! Geometry: builds shapes, evaluates the node tree with CSG booleans and
//! produces render/export meshes. The kernel is Manifold (robust mesh
//! booleans, same engine OpenSCAD uses).

use crate::doc::{Content, FilletKind, Node, Shape};
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
        Content::Group(children) => {
            1u8.hash(h);
            children.len().hash(h);
            for c in children {
                c.hole.hash(h);
                for v in c.transform.0.to_cols_array() {
                    v.to_bits().hash(h);
                }
                hash_content(&c.content, h);
            }
        }
    }
}

/// Mesh ready for drawing and picking, in the node's local space.
pub struct EvalMesh {
    /// Triangle soup: 3 entries per triangle.
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    pub min: Vec3,
    pub max: Vec3,
}

#[derive(Default)]
pub struct GeomCache {
    solids: HashMap<u64, Manifold>,
    meshes: HashMap<u64, Arc<EvalMesh>>,
    used: HashMap<u64, u64>,
    frame: u64,
}

impl GeomCache {
    /// Local-space manifold for a node's content (cached).
    pub fn content_manifold(&mut self, content: &Content) -> Manifold {
        let key = content_hash(content);
        self.used.insert(key, self.frame);
        if let Some(m) = self.solids.get(&key) {
            return m.clone();
        }
        let m = match content {
            Content::Shape(s) => build_shape(s),
            Content::Group(children) => {
                let mut solids = Vec::new();
                let mut holes = Vec::new();
                for c in children {
                    let m = self.content_manifold(&c.content).transform(&c.transform.to_affine12());
                    if c.hole {
                        holes.push(m);
                    } else {
                        solids.push(m);
                    }
                }
                if solids.is_empty() {
                    // A group of only holes is itself a hole shape.
                    Manifold::batch_union(&holes)
                } else {
                    let s = Manifold::batch_union(&solids);
                    if holes.is_empty() {
                        s
                    } else {
                        s.difference(&Manifold::batch_union(&holes))
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
        let m = Arc::new(to_eval_mesh(&self.content_manifold(content)));
        self.meshes.insert(key, m.clone());
        (key, m)
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

/// Convert a manifold to a smooth-shaded triangle soup. Normals are averaged
/// only across faces within ~35 degrees, so flat faces and hard edges stay
/// crisp while curved surfaces look smooth.
pub fn to_eval_mesh(m: &Manifold) -> EvalMesh {
    let (props, np, idx) = m.to_mesh_f32();
    let vcount = if np == 0 { 0 } else { props.len() / np };
    let verts: Vec<Vec3> = (0..vcount)
        .map(|i| Vec3::new(props[i * np], props[i * np + 1], props[i * np + 2]))
        .collect();
    let tris = idx.len() / 3;

    // Face normals (area weighted via cross product length).
    let mut fnorm = Vec::with_capacity(tris);
    for t in 0..tris {
        let a = verts[idx[t * 3] as usize];
        let b = verts[idx[t * 3 + 1] as usize];
        let c = verts[idx[t * 3 + 2] as usize];
        fnorm.push((b - a).cross(c - a));
    }

    // Manifold may split vertices along property seams, so weld by position
    // for adjacency.
    let mut weld: HashMap<[u32; 3], u32> = HashMap::new();
    let mut vid = vec![0u32; vcount];
    for (i, v) in verts.iter().enumerate() {
        let key = [v.x.to_bits(), v.y.to_bits(), v.z.to_bits()];
        let n = weld.len() as u32;
        vid[i] = *weld.entry(key).or_insert(n);
    }
    let mut vfaces: Vec<Vec<u32>> = vec![Vec::new(); weld.len()];
    for t in 0..tris {
        for k in 0..3 {
            vfaces[vid[idx[t * 3 + k] as usize] as usize].push(t as u32);
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
    EvalMesh { positions, normals, min, max }
}

/// World-space axis-aligned bounds of a local mesh under a transform.
pub fn world_bounds(mesh: &EvalMesh, m: &DMat4) -> (DVec3, DVec3) {
    let mut lo = DVec3::splat(f64::MAX);
    let mut hi = DVec3::splat(f64::MIN);
    // Use actual vertices so rotated round shapes get tight bounds.
    let step = (mesh.positions.len() / 20000).max(1);
    for p in mesh.positions.iter().step_by(step) {
        let w = m.transform_point3(p.as_dvec3());
        lo = lo.min(w);
        hi = hi.max(w);
    }
    if mesh.positions.is_empty() {
        let o = m.transform_point3(DVec3::ZERO);
        return (o, o);
    }
    (lo, hi)
}

/// Ray / mesh intersection in local space. Returns distance along the ray.
pub fn ray_hit(mesh: &EvalMesh, origin: Vec3, dir: Vec3) -> Option<f32> {
    // Slab test against the bounds first.
    let inv = dir.recip();
    let t1 = (mesh.min - origin) * inv;
    let t2 = (mesh.max - origin) * inv;
    let tmin = t1.min(t2).max_element();
    let tmax = t1.max(t2).min_element();
    if tmax < tmin.max(0.0) {
        return None;
    }
    let mut best: Option<f32> = None;
    for tri in mesh.positions.chunks_exact(3) {
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
        if t > 1e-6 && best.is_none_or(|b| t < b) {
            best = Some(t);
        }
    }
    best
}

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------

/// World-space manifold for a top-level node.
pub fn world_manifold(cache: &mut GeomCache, node: &Node) -> Manifold {
    cache.content_manifold(&node.content).transform(&node.transform.to_affine12())
}

/// Binary STL of the given nodes (holes are skipped, like Tinkercad).
pub fn export_stl(cache: &mut GeomCache, nodes: &[&Node]) -> Vec<u8> {
    let mut tris: Vec<[[f32; 3]; 4]> = Vec::new();
    for n in nodes.iter().filter(|n| !n.hole) {
        let m = world_manifold(cache, n);
        let (props, np, idx) = m.to_mesh_f32();
        let v = |i: u32| {
            let i = i as usize * np;
            Vec3::new(props[i], props[i + 1], props[i + 2])
        };
        for t in idx.chunks_exact(3) {
            let (a, b, c) = (v(t[0]), v(t[1]), v(t[2]));
            let nrm = (b - a).cross(c - a).normalize_or_zero();
            tris.push([nrm.to_array(), a.to_array(), b.to_array(), c.to_array()]);
        }
    }
    let mut out = Vec::with_capacity(84 + tris.len() * 50);
    let mut header = [0u8; 80];
    let tag = b"Orion3D binary STL";
    header[..tag.len()].copy_from_slice(tag);
    out.extend_from_slice(&header);
    out.extend_from_slice(&(tris.len() as u32).to_le_bytes());
    for t in &tris {
        for v in t {
            for c in v {
                out.extend_from_slice(&c.to_le_bytes());
            }
        }
        out.extend_from_slice(&0u16.to_le_bytes());
    }
    out
}
