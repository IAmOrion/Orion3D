//! Editing operations on the document that need geometry (bounds etc.).

use crate::doc::{Content, Document, Node, NodeId, Shape, Xform};
use crate::geom::{GeomCache, world_bounds};
use glam::{DMat3, DMat4, DVec3};

fn merge(a: Option<(DVec3, DVec3)>, b: Option<(DVec3, DVec3)>) -> Option<(DVec3, DVec3)> {
    match (a, b) {
        (Some((a0, a1)), Some((b0, b1))) => Some((a0.min(b0), a1.max(b1))),
        (x, None) | (None, x) => x,
    }
}

/// World bounds of a top-level node (all its parts, holes included).
pub fn node_bounds_opt(cache: &mut GeomCache, node: &Node) -> Option<(DVec3, DVec3)> {
    let mut out = None;
    for it in cache.items_of(node) {
        out = merge(out, world_bounds(&it.mesh, &it.model));
    }
    out
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn node_bounds(cache: &mut GeomCache, node: &Node) -> (DVec3, DVec3) {
    node_bounds_opt(cache, node).unwrap_or_else(|| {
        let o = node.transform.0.transform_point3(DVec3::ZERO);
        (o, o)
    })
}

/// Bounds in another frame: `pre` maps world to that frame.
pub fn node_bounds_in(cache: &mut GeomCache, node: &Node, pre: &DMat4) -> Option<(DVec3, DVec3)> {
    let mut out = None;
    for it in cache.items_of(node) {
        out = merge(out, world_bounds(&it.mesh, &(*pre * it.model)));
    }
    out
}

pub fn bounds_in(cache: &mut GeomCache, doc: &Document, ids: &[NodeId], pre: &DMat4) -> Option<(DVec3, DVec3)> {
    let mut out = None;
    for id in ids {
        if let Some(n) = doc.get(*id) {
            out = merge(out, node_bounds_in(cache, n, pre));
        }
    }
    out
}

pub fn bounds_of(cache: &mut GeomCache, doc: &Document, ids: &[NodeId]) -> Option<(DVec3, DVec3)> {
    let mut out = None;
    for id in ids {
        if let Some(n) = doc.get(*id) {
            out = merge(out, node_bounds_opt(cache, n));
        }
    }
    out
}

pub fn snap(v: f64, step: f64) -> f64 {
    if step <= 0.0 { v } else { (v / step).round() * step }
}

/// Scale about an anchor point, world axes.
pub fn scale_about(anchor: DVec3, s: DVec3) -> DMat4 {
    DMat4::from_translation(anchor) * DMat4::from_scale(s) * DMat4::from_translation(-anchor)
}

pub fn rotate_about(center: DVec3, axis: DVec3, angle: f64) -> DMat4 {
    DMat4::from_translation(center)
        * DMat4::from_axis_angle(axis.normalize(), angle)
        * DMat4::from_translation(-center)
}

/// Snapshot of nodes at the start of an edit. Edits are always applied to
/// the snapshot (not on top of the previous frame), so dragging never
/// accumulates error, and baking scale into shape parameters stays exact.
pub type Originals = Vec<(NodeId, Node)>;

/// Apply `m` (world space) on top of the original nodes.
pub fn apply(doc: &mut Document, originals: &Originals, m: DMat4, bake: bool) {
    for (id, orig) in originals {
        if let Some(n) = doc.get_mut(*id) {
            n.transform = Xform(m * orig.transform.0);
            n.content = orig.content.clone();
            if bake {
                bake_scale(n);
            }
        }
    }
}

pub fn originals(doc: &Document, ids: &[NodeId]) -> Originals {
    ids.iter().filter_map(|id| doc.get(*id).map(|n| (*id, n.clone()))).collect()
}

/// Move scale out of a shape's transform and into its parameters when the
/// shape can represent it exactly. This keeps rounded box corners round,
/// cylinder radii true, MetaFillet radii true, etc. after resizing.
pub fn bake_scale(node: &mut Node) {
    let Content::Shape(shape) = &mut node.content else { return };
    let m = node.transform.0;
    let c = [m.x_axis.truncate(), m.y_axis.truncate(), m.z_axis.truncate()];
    let l = [c[0].length(), c[1].length(), c[2].length()];
    if l.iter().any(|v| *v < 1e-9) {
        return;
    }
    // Columns must be orthogonal (rotation + scale, no shear).
    for (i, j) in [(0, 1), (0, 2), (1, 2)] {
        if c[i].dot(c[j]).abs() > 1e-7 * l[i] * l[j] {
            return;
        }
    }
    if l.iter().all(|v| (v - 1.0).abs() < 1e-9) {
        return;
    }
    let eq = |a: f64, b: f64| (a - b).abs() <= 1e-9 * a.max(b);
    let [sx, sy, sz] = l;
    let ok = match shape {
        Shape::Box { size, .. } | Shape::Roof { size } | Shape::Wedge { size } => {
            *size = [size[0] * sx, size[1] * sy, size[2] * sz];
            true
        }
        Shape::Cylinder { radius, height, .. } | Shape::Polygon { radius, height, .. } if eq(sx, sy) => {
            *radius *= sx;
            *height *= sz;
            true
        }
        Shape::Pyramid { radius, height, .. } if eq(sx, sy) => {
            *radius *= sx;
            *height *= sz;
            true
        }
        Shape::Tube { radius, height, .. } if eq(sx, sy) => {
            *radius *= sx;
            *height *= sz;
            true
        }
        Shape::Cone { base_radius, top_radius, height, .. } if eq(sx, sy) => {
            *base_radius *= sx;
            *top_radius *= sx;
            *height *= sz;
            true
        }
        Shape::MetaFillet { radius, height, .. } if eq(sx, sy) => {
            *radius *= sx;
            *height *= sz;
            true
        }
        Shape::Sphere { radius, .. } | Shape::HalfSphere { radius, .. } if eq(sx, sy) && eq(sy, sz) => {
            *radius *= sx;
            true
        }
        Shape::Torus { radius, tube, .. } if eq(sx, sy) && eq(sy, sz) => {
            *radius *= sx;
            *tube *= sx;
            true
        }
        _ => false,
    };
    if ok {
        let r = DMat3::from_cols(c[0] / sx, c[1] / sy, c[2] / sz);
        let t = m.w_axis.truncate();
        node.transform = Xform(DMat4::from_mat3_translation(r, t));
    }
}
