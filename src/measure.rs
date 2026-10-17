//! Measure tool (in the spirit of 123D Design): pick edges, circles or
//! faces on the finished geometry and read distances, radii and diameters,
//! without ungrouping anything.

use crate::geom::EvalMesh;
use glam::{DMat4, DVec3};
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub enum Feature {
    /// Straight edge.
    Line { a: DVec3, b: DVec3 },
    /// Circular edge or arc (full circle if `full`).
    Circle { center: DVec3, normal: DVec3, radius: f64, pts: Vec<DVec3>, full: bool },
    /// Any other edge chain.
    Poly { pts: Vec<DVec3> },
    /// A flat face (plane through `point`).
    Face { point: DVec3, normal: DVec3 },
}

impl Feature {
    /// Points to draw/highlight the feature.
    pub fn polyline(&self) -> Vec<DVec3> {
        match self {
            Feature::Line { a, b } => vec![*a, *b],
            Feature::Circle { pts, .. } | Feature::Poly { pts } => pts.clone(),
            Feature::Face { .. } => Vec::new(),
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Feature::Line { a, b } => format!("Edge  length {}", mm(a.distance(*b))),
            Feature::Circle { radius, full, .. } => {
                let kind = if *full { "Circle" } else { "Arc" };
                format!("{kind}  R {}   Dia {}", mm(*radius), mm(radius * 2.0))
            }
            Feature::Poly { pts } => {
                let len: f64 = pts.windows(2).map(|w| w[0].distance(w[1])).sum();
                format!("Edge  length {}", mm(len))
            }
            Feature::Face { .. } => "Face".to_string(),
        }
    }
}

pub fn mm(v: f64) -> String {
    format!("{:.2} mm", v)
}

// ---------------------------------------------------------------------------
// Feature extraction from hard edges
// ---------------------------------------------------------------------------

/// Build edge features (lines, circles, other chains) in world space.
pub fn extract(mesh: &EvalMesh, m: &DMat4) -> Vec<Feature> {
    // Weld edge endpoints.
    let q = |p: DVec3| [(p.x * 1e4).round() as i64, (p.y * 1e4).round() as i64, (p.z * 1e4).round() as i64];
    let mut ids: HashMap<[i64; 3], usize> = HashMap::new();
    let mut pts: Vec<DVec3> = Vec::new();
    let mut adj: Vec<Vec<usize>> = Vec::new();
    let mut id_of = |p: DVec3, pts: &mut Vec<DVec3>, adj: &mut Vec<Vec<usize>>| {
        *ids.entry(q(p)).or_insert_with(|| {
            pts.push(p);
            adj.push(Vec::new());
            pts.len() - 1
        })
    };
    for e in &mesh.edges {
        let a = id_of(m.transform_point3(e[0].as_dvec3()), &mut pts, &mut adj);
        let b = id_of(m.transform_point3(e[1].as_dvec3()), &mut pts, &mut adj);
        if a != b && !adj[a].contains(&b) {
            adj[a].push(b);
            adj[b].push(a);
        }
    }

    // Walk chains, breaking at junctions and at sharp turns.
    let mut used: std::collections::HashSet<(usize, usize)> = std::collections::HashSet::new();
    let key = |a: usize, b: usize| (a.min(b), a.max(b));
    let turn_limit = 30f64.to_radians().cos();
    let mut chains: Vec<Vec<usize>> = Vec::new();
    let starts: Vec<usize> = (0..pts.len()).filter(|&v| adj[v].len() != 2).chain(0..pts.len()).collect();
    for s in starts {
        for &n0 in &adj[s].clone() {
            if used.contains(&key(s, n0)) {
                continue;
            }
            let mut chain = vec![s, n0];
            used.insert(key(s, n0));
            loop {
                let cur = *chain.last().unwrap();
                let prev = chain[chain.len() - 2];
                if adj[cur].len() != 2 || cur == s {
                    break;
                }
                let next = if adj[cur][0] == prev { adj[cur][1] } else { adj[cur][0] };
                if used.contains(&key(cur, next)) {
                    break;
                }
                let d0 = (pts[cur] - pts[prev]).normalize_or_zero();
                let d1 = (pts[next] - pts[cur]).normalize_or_zero();
                if d0.dot(d1) < turn_limit {
                    break;
                }
                used.insert(key(cur, next));
                chain.push(next);
            }
            chains.push(chain);
        }
    }

    // Merge chains that continue smoothly (split only because of where
    // the walk started on a loop).
    let mut out = Vec::new();
    for c in chains {
        let p: Vec<DVec3> = c.iter().map(|i| pts[*i]).collect();
        out.push(classify(p));
    }
    out
}

fn classify(p: Vec<DVec3>) -> Feature {
    let a = p[0];
    let b = *p.last().unwrap();
    let len: f64 = p.windows(2).map(|w| w[0].distance(w[1])).sum();
    // Straight?
    if a.distance(b) > 1e-9 {
        let dir = (b - a).normalize();
        let max_off = p.iter().map(|x| (*x - a).reject_from_normalized(dir).length()).fold(0.0, f64::max);
        if max_off < 1e-3 * len.max(1.0) {
            return Feature::Line { a, b };
        }
    }
    // Circle / arc?
    if p.len() >= 5 {
        if let Some((center, normal, radius)) = fit_circle(&p) {
            let ok = p.iter().all(|x| {
                let d = *x - center;
                (d.length() - radius).abs() < radius * 0.01 + 1e-4 && d.dot(normal).abs() < radius * 0.01 + 1e-4
            });
            if ok {
                let full = a.distance(b) < 1e-6 || len > std::f64::consts::TAU * radius * 0.97;
                return Feature::Circle { center, normal, radius, pts: p, full };
            }
        }
    }
    Feature::Poly { pts: p }
}

/// Circle through three well spaced points of the chain.
fn fit_circle(p: &[DVec3]) -> Option<(DVec3, DVec3, f64)> {
    let n = p.len();
    let (a, b, c) = (p[0], p[n / 3], p[(2 * n) / 3]);
    let ab = b - a;
    let ac = c - a;
    let normal = ab.cross(ac);
    let nl2 = normal.length_squared();
    if nl2 < 1e-18 {
        return None;
    }
    let center = a + (ac.length_squared() * normal.cross(ab) + ab.length_squared() * ac.cross(normal)) / (2.0 * nl2);
    Some((center, normal.normalize(), center.distance(a)))
}

// ---------------------------------------------------------------------------
// Measuring between two features
// ---------------------------------------------------------------------------

pub struct Result2 {
    /// Closest points used for the dimension line.
    pub from: DVec3,
    pub to: DVec3,
    pub lines: Vec<String>,
}

fn closest_on_segment(p: DVec3, a: DVec3, b: DVec3) -> DVec3 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-18)).clamp(0.0, 1.0);
    a + ab * t
}

fn point_line_dist(p: DVec3, a: DVec3, dir: DVec3) -> (f64, DVec3) {
    let foot = a + dir * (p - a).dot(dir);
    (p.distance(foot), foot)
}

fn parallel(a: DVec3, b: DVec3) -> bool {
    a.normalize().dot(b.normalize()).abs() > 0.9995
}

/// Brute-force closest points between two polylines (sampled).
fn closest_polylines(a: &[DVec3], b: &[DVec3]) -> (DVec3, DVec3) {
    let mut best = (f64::MAX, a[0], b[0]);
    let seg = |v: &[DVec3]| -> Vec<(DVec3, DVec3)> {
        if v.len() == 1 { vec![(v[0], v[0])] } else { v.windows(2).map(|w| (w[0], w[1])).collect() }
    };
    for (a0, a1) in seg(a) {
        for (b0, b1) in seg(b) {
            // Sample a few points along each segment; good enough for UI.
            for i in 0..=8 {
                let pa = a0.lerp(a1, i as f64 / 8.0);
                let pb = closest_on_segment(pa, b0, b1);
                let pa2 = closest_on_segment(pb, a0, a1);
                let d = pa2.distance(pb);
                if d < best.0 {
                    best = (d, pa2, pb);
                }
            }
        }
    }
    (best.1, best.2)
}

pub fn measure(f1: &Feature, f2: &Feature) -> Result2 {
    use Feature::*;
    let mut lines = Vec::new();
    let (from, to) = match (f1, f2) {
        (Line { a: a1, b: b1 }, Line { a: a2, b: b2 }) if parallel(*b1 - *a1, *b2 - *a2) => {
            let dir = (*b1 - *a1).normalize();
            let mid2 = (*a2 + *b2) * 0.5;
            let (d, foot) = point_line_dist(mid2, *a1, dir);
            lines.push(format!("Gap between parallel edges: {}", mm(d)));
            (foot, mid2)
        }
        (Face { point: p1, normal: n1 }, Face { point: p2, normal: n2 }) if parallel(*n1, *n2) => {
            let n = n1.normalize();
            let d = (*p2 - *p1).dot(n);
            lines.push(format!("Distance between parallel faces: {}", mm(d.abs())));
            (*p1, *p1 + n * d)
        }
        (Circle { center: c1, radius: r1, normal: n1, .. }, Circle { center: c2, radius: r2, normal: n2, .. }) => {
            let d = c1.distance(*c2);
            lines.push(format!("Centre to centre: {}", mm(d)));
            if parallel(*n1, *n2) && (*c2 - *c1).dot(n1.normalize()).abs() < 1e-3 {
                let gap = d - r1 - r2;
                if gap > 0.0 {
                    lines.push(format!("Edge to edge: {}", mm(gap)));
                }
            }
            (*c1, *c2)
        }
        (Circle { center, .. }, Line { a, b }) | (Line { a, b }, Circle { center, .. }) => {
            let (d, foot) = point_line_dist(*center, *a, (*b - *a).normalize());
            lines.push(format!("Centre to edge: {}", mm(d)));
            if matches!(f1, Circle { .. }) { (*center, foot) } else { (foot, *center) }
        }
        (Circle { center, .. }, Face { point, normal }) | (Face { point, normal }, Circle { center, .. }) => {
            let n = normal.normalize();
            let d = (*center - *point).dot(n);
            lines.push(format!("Centre to face: {}", mm(d.abs())));
            let foot = *center - n * d;
            if matches!(f1, Circle { .. }) { (*center, foot) } else { (foot, *center) }
        }
        (Line { a, b }, Face { point, normal }) | (Face { point, normal }, Line { a, b }) => {
            let n = normal.normalize();
            let mid = (*a + *b) * 0.5;
            let d = (mid - *point).dot(n);
            if (*b - *a).normalize().dot(n).abs() < 1e-3 {
                lines.push(format!("Edge to face: {}", mm(d.abs())));
            } else {
                lines.push(format!("Edge midpoint to face: {}", mm(d.abs())));
            }
            let foot = mid - n * d;
            if matches!(f1, Line { .. }) { (mid, foot) } else { (foot, mid) }
        }
        (Face { point: p1, normal: n1 }, Face { point: p2, .. }) => {
            let n = n1.normalize();
            let d = (*p2 - *p1).dot(n);
            lines.push(format!("Point to face: {}", mm(d.abs())));
            (*p2 - n * d, *p2)
        }
        (a, b) => {
            let pa = if let Face { point, .. } = a { vec![*point] } else { a.polyline() };
            let pb = if let Face { point, .. } = b { vec![*point] } else { b.polyline() };
            let (x, y) = closest_polylines(&pa, &pb);
            lines.push(format!("Closest distance: {}", mm(x.distance(y))));
            (x, y)
        }
    };
    let d = to - from;
    lines.push(format!("dX {}   dY {}   dZ {}", fmt_signed(d.x), fmt_signed(d.y), fmt_signed(d.z)));
    if let (Line { a: a1, b: b1 }, Line { a: a2, b: b2 }) = (f1, f2) {
        if !parallel(*b1 - *a1, *b2 - *a2) {
            let ang = (*b1 - *a1).normalize().dot((*b2 - *a2).normalize()).abs().clamp(-1.0, 1.0).acos();
            lines.push(format!("Angle between edges: {:.1} deg", ang.to_degrees()));
        }
    }
    Result2 { from, to, lines }
}

fn fmt_signed(v: f64) -> String {
    let v = if v.abs() < 0.005 { 0.0 } else { v };
    format!("{v:+.2}")
}
