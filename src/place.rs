//! "Cruise" placement: finding the flat face under the cursor so new shapes
//! can be dropped onto any face of another shape (like Tinkercad).

use crate::geom::EvalMesh;
use glam::{DMat4, DQuat, DVec3, Vec3};
use std::collections::HashMap;
use std::sync::Arc;

/// Triangle adjacency for a render mesh (triangle soup), built once per mesh.
pub struct Adjacency {
    tri_v: Vec<[u32; 3]>,
    edge_tris: HashMap<(u32, u32), Vec<u32>>,
}

impl Adjacency {
    pub fn build(mesh: &EvalMesh) -> Adjacency {
        let mut weld: HashMap<[u32; 3], u32> = HashMap::new();
        let mut id = |p: Vec3| {
            let k = [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()];
            let n = weld.len() as u32;
            *weld.entry(k).or_insert(n)
        };
        let tris = mesh.positions.len() / 3;
        let mut tri_v = Vec::with_capacity(tris);
        let mut edge_tris: HashMap<(u32, u32), Vec<u32>> = HashMap::with_capacity(tris * 2);
        for t in 0..tris {
            let v = [id(mesh.positions[t * 3]), id(mesh.positions[t * 3 + 1]), id(mesh.positions[t * 3 + 2])];
            for k in 0..3 {
                let (a, b) = (v[k], v[(k + 1) % 3]);
                edge_tris.entry((a.min(b), a.max(b))).or_default().push(t as u32);
            }
            tri_v.push(v);
        }
        Adjacency { tri_v, edge_tris }
    }
}

/// A flat face, in world space.
pub struct Face {
    pub normal: DVec3,
    /// Area-weighted centre of the face.
    pub centre: DVec3,
    /// Boundary edges (for the green outline).
    pub outline: Vec<[DVec3; 2]>,
    /// Triangles (for the green fill); empty if the face is very large.
    pub tris: Vec<[DVec3; 3]>,
}

/// The coplanar, connected region around triangle `tri`.
pub fn face_at(mesh: &EvalMesh, adj: &Adjacency, tri: usize, model: &DMat4) -> Option<Face> {
    let p = |t: usize, k: usize| mesh.positions[t * 3 + k].as_dvec3();
    let n0 = (p(tri, 1) - p(tri, 0)).cross(p(tri, 2) - p(tri, 0)).normalize_or_zero();
    if n0 == DVec3::ZERO {
        return None;
    }
    let p0 = p(tri, 0);
    let ext = (mesh.max - mesh.min).length().max(1e-3) as f64;
    let same_plane = |t: usize| {
        let n = (p(t, 1) - p(t, 0)).cross(p(t, 2) - p(t, 0)).normalize_or_zero();
        n.dot(n0) > 0.99995 && (p(t, 0) - p0).dot(n0).abs() < ext * 1e-5
    };

    let mut inside = vec![false; adj.tri_v.len()];
    let mut stack = vec![tri];
    let mut region = Vec::new();
    inside[tri] = true;
    while let Some(t) = stack.pop() {
        region.push(t);
        if region.len() > 200_000 {
            break;
        }
        let v = adj.tri_v[t];
        for k in 0..3 {
            let (a, b) = (v[k], v[(k + 1) % 3]);
            if let Some(ns) = adj.edge_tris.get(&(a.min(b), a.max(b))) {
                for &u in ns {
                    let u = u as usize;
                    if !inside[u] && same_plane(u) {
                        inside[u] = true;
                        stack.push(u);
                    }
                }
            }
        }
    }

    let w = |q: DVec3| model.transform_point3(q);
    let mut area = 0.0;
    let mut centre = DVec3::ZERO;
    let mut tris = Vec::new();
    let mut outline = Vec::new();
    for &t in &region {
        let (a, b, c) = (w(p(t, 0)), w(p(t, 1)), w(p(t, 2)));
        let ar = (b - a).cross(c - a).length() * 0.5;
        area += ar;
        centre += (a + b + c) / 3.0 * ar;
        if region.len() <= 4000 {
            tris.push([a, b, c]);
        }
        let v = adj.tri_v[t];
        for k in 0..3 {
            let (va, vb) = (v[k], v[(k + 1) % 3]);
            let shared = adj
                .edge_tris
                .get(&(va.min(vb), va.max(vb)))
                .is_some_and(|ns| ns.iter().any(|&u| u as usize != t && inside[u as usize]));
            if !shared {
                outline.push([w(p(t, k)), w(p(t, (k + 1) % 3))]);
            }
        }
    }
    if area <= 0.0 {
        return None;
    }
    let normal = model.inverse().transpose().transform_vector3(n0).normalize_or_zero();
    Some(Face { normal, centre: centre / area, outline, tris })
}

/// Transform for a new shape whose base sits at `at`, standing on a face
/// with the given outward normal.
pub fn standing_on(at: DVec3, normal: DVec3) -> DMat4 {
    let n = normal.normalize_or(DVec3::Z);
    let q = if n.dot(DVec3::Z) > 0.999999 {
        DQuat::IDENTITY
    } else {
        DQuat::from_rotation_arc(DVec3::Z, n)
    };
    DMat4::from_rotation_translation(q, at)
}

/// Small cache of adjacency per render mesh.
#[derive(Default)]
pub struct AdjCache {
    map: HashMap<u64, Arc<Adjacency>>,
}

impl AdjCache {
    pub fn get(&mut self, key: u64, mesh: &EvalMesh) -> Arc<Adjacency> {
        if self.map.len() > 64 {
            self.map.clear();
        }
        self.map.entry(key).or_insert_with(|| Arc::new(Adjacency::build(mesh))).clone()
    }
}
