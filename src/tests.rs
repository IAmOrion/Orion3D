//! Core tests: grouping round-trips, file round-trips, CSG results.

use crate::doc::*;
#[allow(unused_imports)]
use crate::geom::Item;
use crate::geom::{self, GeomCache};
use crate::ops;
use glam::{DMat4, DVec3};

fn approx(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

fn volume(cache: &mut GeomCache, n: &Node) -> f64 {
    cache.content_manifold(&n.content).transform(&n.transform.to_affine12()).volume()
}

#[test]
fn box_minus_cylinder_volume() {
    let mut doc = Document::default();
    let b = doc.new_shape(ShapeKind::Box, DVec3::ZERO);
    let mut c = doc.new_shape(ShapeKind::Cylinder, DVec3::ZERO);
    c.hole = true;
    c.content = Content::Shape(Shape::Cylinder { radius: 5.0, height: 20.0, sides: 64 });
    let (bid, cid) = (b.id, c.id);
    doc.nodes.push(b);
    doc.nodes.push(c);
    let g = doc.group(&[bid, cid], GroupOp::Union).unwrap();
    let mut cache = GeomCache::default();
    let v = volume(&mut cache, doc.get(g).unwrap());
    let expected = 8000.0 - std::f64::consts::PI * 25.0 * 20.0;
    // 64-gon is slightly smaller than a true circle.
    assert!(approx(v, expected, 20.0), "volume {v} expected ~{expected}");
}

#[test]
fn ungroup_restores_parts_and_transforms() {
    let mut doc = Document::default();
    let a = doc.new_shape(ShapeKind::Box, DVec3::new(10.0, 0.0, 0.0));
    let b = doc.new_shape(ShapeKind::Sphere, DVec3::new(-10.0, 5.0, 0.0));
    let (aid, bid) = (a.id, b.id);
    let (a0, b0) = (a.clone(), b.clone());
    doc.nodes.push(a);
    doc.nodes.push(b);
    let g = doc.group(&[aid, bid], GroupOp::Union).unwrap();

    // Move and rotate the group, then nest it in another group.
    let m = DMat4::from_translation(DVec3::new(3.0, 4.0, 5.0)) * DMat4::from_rotation_z(0.7);
    doc.get_mut(g).unwrap().transform = Xform(m);
    let other = doc.new_shape(ShapeKind::Cone, DVec3::ZERO);
    let oid = other.id;
    doc.nodes.push(other);
    let outer = doc.group(&[g, oid], GroupOp::Union).unwrap();

    // Ungroup all the way down.
    let released = doc.ungroup(outer);
    assert_eq!(released.len(), 2);
    let parts = doc.ungroup(g);
    assert_eq!(parts, vec![aid, bid]);
    let a1 = doc.get(aid).unwrap();
    let b1 = doc.get(bid).unwrap();
    assert_eq!(a1.content, a0.content);
    assert_eq!(b1.content, b0.content);
    // Transforms carry the group's move.
    assert!(a1.transform.0.abs_diff_eq(m * a0.transform.0, 1e-12));
    assert!(b1.transform.0.abs_diff_eq(m * b0.transform.0, 1e-12));
}

#[test]
fn hole_state_survives_ungroup() {
    let mut doc = Document::default();
    let a = doc.new_shape(ShapeKind::Box, DVec3::ZERO);
    let mut b = doc.new_shape(ShapeKind::Cylinder, DVec3::ZERO);
    b.hole = true;
    let (aid, bid) = (a.id, b.id);
    doc.nodes.push(a);
    doc.nodes.push(b);
    let g = doc.group(&[aid, bid], GroupOp::Union).unwrap();
    doc.get_mut(g).unwrap().hole = true;
    doc.ungroup(g);
    assert!(!doc.get(aid).unwrap().hole);
    assert!(doc.get(bid).unwrap().hole);
}

#[test]
fn file_round_trip() {
    let mut doc = Document::default();
    let a = doc.new_shape(ShapeKind::MetaFillet, DVec3::new(1.5, 2.25, 0.0));
    let mut b = doc.new_shape(ShapeKind::Torus, DVec3::ZERO);
    b.hole = true;
    let (aid, bid) = (a.id, b.id);
    doc.nodes.push(a);
    doc.nodes.push(b);
    doc.group(&[aid, bid], GroupOp::Union);
    let dir = std::env::temp_dir().join(format!("orion3d_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("t.orion3d");
    crate::file::save(&doc, &path).unwrap();
    let loaded = crate::file::load(&path).unwrap();
    assert_eq!(loaded.nodes, doc.nodes);
    let mut loaded = loaded;
    // New ids continue after the highest stored id.
    let fresh = loaded.alloc_id();
    assert!(fresh > aid.max(bid));
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn metafillet_volumes() {
    let mut cache = GeomCache::default();
    let r = 20.0;
    let h = 8.0;
    let fillet = geom::build_shape(&Shape::MetaFillet { radius: r, height: h, kind: FilletKind::Fillet, segments: 64 });
    let expected = (r * r - std::f64::consts::PI * r * r / 4.0) * h;
    assert!(approx(fillet.volume(), expected, 3.0), "fillet {}", fillet.volume());
    let chamfer = geom::build_shape(&Shape::MetaFillet { radius: r, height: h, kind: FilletKind::Chamfer, segments: 64 });
    assert!(approx(chamfer.volume(), r * r * 0.5 * h, 1e-6));
    let _ = &mut cache;
}

#[test]
fn every_library_shape_builds() {
    for (k, _) in LIBRARY {
        let m = geom::build_shape(&k.default_shape());
        assert!(m.volume() > 1.0, "{} has no volume", k.label());
        let bb = m.bounding_box().unwrap();
        assert!(bb.min()[2].abs() < 1e-6, "{} should sit on Z=0", k.label());
    }
}

#[test]
fn bake_scale_keeps_geometry() {
    let mut doc = Document::default();
    let mut n = doc.new_shape(ShapeKind::Box, DVec3::new(5.0, 0.0, 0.0));
    n.content = Content::Shape(Shape::Box { size: [20.0, 20.0, 20.0], radius: 3.0, steps: 6 });
    let mut cache = GeomCache::default();
    let s = DMat4::from_rotation_z(0.4) * DMat4::from_scale(DVec3::new(2.0, 1.0, 0.5));
    n.transform = Xform(s * n.transform.0);
    let before = volume(&mut cache, &n);
    ops::bake_scale(&mut n);
    let Content::Shape(Shape::Box { size, radius, .. }) = &n.content else { panic!() };
    assert!(approx(size[0], 40.0, 1e-9) && approx(size[1], 20.0, 1e-9) && approx(size[2], 10.0, 1e-9));
    assert_eq!(*radius, 3.0);
    // The rounded corners now stay round, so volume differs slightly; the
    // overall box must still match.
    let after = volume(&mut cache, &n);
    assert!((before - after).abs() / before < 0.02);
    let (lo, hi) = ops::node_bounds(&mut cache, &n);
    assert!(approx(lo.z, 0.0, 1e-6) && approx(hi.z, 10.0, 1e-6));
}

#[test]
fn stl_export_has_triangles() {
    let mut doc = Document::default();
    let a = doc.new_shape(ShapeKind::Sphere, DVec3::ZERO);
    let mut cache = GeomCache::default();
    let data = crate::meshio::write_stl(&geom::export_meshes(&mut cache, &[&a]));
    let n = u32::from_le_bytes(data[80..84].try_into().unwrap()) as usize;
    assert!(n > 100);
    assert_eq!(data.len(), 84 + n * 50);
    doc.nodes.push(a);
}

#[test]
fn old_group_format_still_loads() {
    let json = br##"{"format":"orion3d","version":1,"units":"mm","nodes":[
      {"id":3,"name":"Group","hole":false,"color":"#e1352d","transform":[1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1],
       "content":{"group":[
         {"id":1,"name":"Box","hole":false,"color":"#e1352d","transform":[1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1],"content":{"shape":{"type":"box","size":[20,20,20],"radius":0,"steps":10}}},
         {"id":2,"name":"Cyl","hole":true,"color":"#e1352d","transform":[1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1],"content":{"shape":{"type":"cylinder","radius":5,"height":20,"sides":32}}}
       ]}}]}"##;
    let doc = crate::file::from_json(json).unwrap();
    let Content::Group(g) = &doc.nodes[0].content else { panic!() };
    assert_eq!(g.op, GroupOp::Union);
    assert_eq!(g.children.len(), 2);
}

#[test]
fn intersect_and_bundle() {
    let mut doc = Document::default();
    let a = doc.new_shape(ShapeKind::Box, DVec3::ZERO);
    let b = doc.new_shape(ShapeKind::Box, DVec3::new(10.0, 0.0, 0.0));
    let (aid, bid) = (a.id, b.id);
    doc.nodes.push(a);
    doc.nodes.push(b);
    let g = doc.group(&[aid, bid], GroupOp::Intersect).unwrap();
    let mut cache = GeomCache::default();
    let v = volume(&mut cache, doc.get(g).unwrap());
    assert!(approx(v, 10.0 * 20.0 * 20.0, 1e-6), "intersect volume {v}");

    // A bundle draws its parts separately.
    if let Some(n) = doc.get_mut(g) {
        if let Content::Group(gr) = &mut n.content {
            gr.op = GroupOp::Bundle;
        }
    }
    let items = cache.items_of(doc.get(g).unwrap());
    assert_eq!(items.len(), 2);
}

#[test]
fn mesh_import_round_trip() {
    let mut doc = Document::default();
    let cube = crate::geom::build_shape(&Shape::Box { size: [10.0, 10.0, 10.0], radius: 0.0, steps: 1 });
    let stl = crate::meshio::write_stl(&[crate::geom::ExportMesh {
        name: "c".into(),
        color: PALETTE[0],
        positions: {
            let (p, np, _) = cube.to_mesh_f32();
            p.chunks_exact(np).map(|c| [c[0], c[1], c[2]]).collect()
        },
        indices: cube.to_mesh_f32().2,
    }]);
    let imp = crate::meshio::read_stl(&stl, "c").unwrap();
    let md = MeshData::new(imp.positions, imp.indices);
    let m = crate::geom::mesh_manifold(&md).expect("cube STL should be watertight");
    assert!(approx(m.volume(), 1000.0, 1e-3));
    let n = doc.new_mesh("c", md, DVec3::ZERO);
    doc.nodes.push(n);

    let dir = std::env::temp_dir().join(format!("orion3d_mesh_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("m.orion3d");
    crate::file::save(&doc, &path).unwrap();
    let loaded = crate::file::load(&path).unwrap();
    let Content::Mesh(lm) = &loaded.nodes[0].content else { panic!() };
    assert_eq!(lm.indices.len(), 36);
    std::fs::remove_dir_all(dir).ok();

    // 3MF export then import gives the same volume.
    let mut cache = GeomCache::default();
    let meshes = geom::export_meshes(&mut cache, &[&loaded.nodes[0]]);
    let data = crate::meshio::write_3mf(&meshes).unwrap();
    let parts = crate::meshio::read_3mf(&data, "x").unwrap();
    let md2 = MeshData::new(parts[0].positions.clone(), parts[0].indices.clone());
    assert!(approx(crate::geom::mesh_manifold(&md2).unwrap().volume(), 1000.0, 1e-3));
}

#[test]
fn measure_notch_gap_and_circle() {
    use crate::measure::{Feature, extract, measure};
    // Cylinder r=10 with a 4 mm wide notch cut through it.
    let mut doc = Document::default();
    let mut c = doc.new_shape(ShapeKind::Cylinder, DVec3::ZERO);
    c.content = Content::Shape(Shape::Cylinder { radius: 10.0, height: 5.0, sides: 64 });
    let mut b = doc.new_shape(ShapeKind::Box, DVec3::new(8.0, 0.0, -1.0));
    b.hole = true;
    b.content = Content::Shape(Shape::Box { size: [8.0, 4.0, 7.0], radius: 0.0, steps: 1 });
    let (cid, bid) = (c.id, b.id);
    doc.nodes.push(c);
    doc.nodes.push(b);
    let g = doc.group(&[cid, bid], GroupOp::Union).unwrap();
    let mut cache = GeomCache::default();
    let item = cache.items_of(doc.get(g).unwrap()).remove(0);
    let feats = extract(&item.mesh, &item.model);
    // Find the two notch side edges at the top: lines parallel to X at y=+-2, z=5.
    let side = |y: f64| {
        feats
            .iter()
            .find(|f| matches!(f, Feature::Line { a, b } if (a.y - y).abs() < 1e-3 && (b.y - y).abs() < 1e-3 && (a.z - 5.0).abs() < 1e-3 && (b.z - 5.0).abs() < 1e-3))
            .cloned()
            .expect("notch edge")
    };
    let r = measure(&side(2.0), &side(-2.0));
    assert!(approx(r.from.distance(r.to), 4.0, 1e-6), "gap {}", r.from.distance(r.to));
    // The outer rim is an arc of radius 10.
    assert!(feats.iter().any(|f| matches!(f, Feature::Circle { radius, .. } if approx(*radius, 10.0, 0.05))));
}
