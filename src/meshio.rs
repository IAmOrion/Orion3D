//! Mesh file formats: STL / OBJ / 3MF export and import.

use crate::geom::ExportMesh;
use glam::{DMat4, DVec3};
use std::io::Write;

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Stl,
    Obj,
    ThreeMf,
}

impl Format {
    pub fn ext(self) -> &'static str {
        match self {
            Format::Stl => "stl",
            Format::Obj => "obj",
            Format::ThreeMf => "3mf",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Format::Stl => ".STL",
            Format::Obj => ".OBJ",
            Format::ThreeMf => ".3MF",
        }
    }
    pub fn hint(self) -> &'static str {
        match self {
            Format::Stl => "Single colour, for 3D printing",
            Format::Obj => "With colours, for other 3D apps",
            Format::ThreeMf => "Separate coloured parts, for multi-colour printing",
        }
    }
}

pub fn write(format: Format, meshes: &[ExportMesh]) -> Result<Vec<u8>, String> {
    match format {
        Format::Stl => Ok(write_stl(meshes)),
        Format::Obj => Ok(write_obj(meshes).into_bytes()),
        Format::ThreeMf => write_3mf(meshes),
    }
}

pub fn write_stl(meshes: &[ExportMesh]) -> Vec<u8> {
    let count: usize = meshes.iter().map(|m| m.indices.len() / 3).sum();
    let mut out = Vec::with_capacity(84 + count * 50);
    let mut header = [0u8; 80];
    let tag = b"Orion3D binary STL";
    header[..tag.len()].copy_from_slice(tag);
    out.extend_from_slice(&header);
    out.extend_from_slice(&(count as u32).to_le_bytes());
    for m in meshes {
        for t in m.indices.chunks_exact(3) {
            let [a, b, c] = [0, 1, 2].map(|k| glam::Vec3::from_array(m.positions[t[k] as usize]));
            let n = (b - a).cross(c - a).normalize_or_zero();
            for v in [n, a, b, c] {
                for x in v.to_array() {
                    out.extend_from_slice(&x.to_le_bytes());
                }
            }
            out.extend_from_slice(&0u16.to_le_bytes());
        }
    }
    out
}

fn srgb(c: u8) -> f32 {
    c as f32 / 255.0
}

pub fn write_obj(meshes: &[ExportMesh]) -> String {
    use std::fmt::Write as _;
    let mut s = String::from("# Orion3D\n# Units: millimetres\n");
    let mut base = 1usize;
    for m in meshes {
        let [r, g, b] = m.color.0.map(srgb);
        let _ = writeln!(s, "o {}", m.name.replace(char::is_whitespace, "_"));
        for p in &m.positions {
            let _ = writeln!(s, "v {} {} {} {r:.4} {g:.4} {b:.4}", p[0], p[1], p[2]);
        }
        for t in m.indices.chunks_exact(3) {
            let _ = writeln!(s, "f {} {} {}", t[0] as usize + base, t[1] as usize + base, t[2] as usize + base);
        }
        base += m.positions.len();
    }
    s
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

pub fn write_3mf(meshes: &[ExportMesh]) -> Result<Vec<u8>, String> {
    use std::fmt::Write as _;
    let mut model = String::new();
    model.push_str(r#"<?xml version="1.0" encoding="UTF-8"?>
<model unit="millimeter" xml:lang="en-US" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02">
 <metadata name="Application">Orion3D</metadata>
 <resources>
  <basematerials id="1">
"#);
    for m in meshes {
        let _ = writeln!(model, "   <base name=\"{}\" displaycolor=\"{}FF\" />", xml_escape(&m.name), m.color.to_hex_string().to_uppercase());
    }
    model.push_str("  </basematerials>\n");
    for (i, m) in meshes.iter().enumerate() {
        let _ = writeln!(model, "  <object id=\"{}\" name=\"{}\" type=\"model\" pid=\"1\" pindex=\"{}\">", i + 2, xml_escape(&m.name), i);
        model.push_str("   <mesh>\n    <vertices>\n");
        for p in &m.positions {
            let _ = writeln!(model, "     <vertex x=\"{}\" y=\"{}\" z=\"{}\" />", p[0], p[1], p[2]);
        }
        model.push_str("    </vertices>\n    <triangles>\n");
        for t in m.indices.chunks_exact(3) {
            let _ = writeln!(model, "     <triangle v1=\"{}\" v2=\"{}\" v3=\"{}\" />", t[0], t[1], t[2]);
        }
        model.push_str("    </triangles>\n   </mesh>\n  </object>\n");
    }
    model.push_str(" </resources>\n <build>\n");
    for i in 0..meshes.len() {
        let _ = writeln!(model, "  <item objectid=\"{}\" />", i + 2);
    }
    model.push_str(" </build>\n</model>\n");

    let content_types = r#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
 <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml" />
 <Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml" />
</Types>
"#;
    let rels = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
 <Relationship Target="/3D/3dmodel.model" Id="rel0" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel" />
</Relationships>
"#;
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, data) in [
            ("[Content_Types].xml", content_types.as_bytes()),
            ("_rels/.rels", rels.as_bytes()),
            ("3D/3dmodel.model", model.as_bytes()),
        ] {
            zip.start_file(name, opts).map_err(|e| e.to_string())?;
            zip.write_all(data).map_err(|e| e.to_string())?;
        }
        zip.finish().map_err(|e| e.to_string())?;
    }
    Ok(buf.into_inner())
}

// ---------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------

/// An imported part: indexed triangles in millimetres, plus an optional colour.
pub struct ImportedMesh {
    pub name: String,
    pub positions: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
    pub color: Option<crate::doc::Rgb>,
}

pub fn import(path: &std::path::Path) -> Result<Vec<ImportedMesh>, String> {
    let data = std::fs::read(path).map_err(|e| format!("Cannot read file: {e}"))?;
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Import".into());
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let parts = match ext.as_str() {
        "stl" => vec![read_stl(&data, &stem)?],
        "obj" => vec![read_obj(&String::from_utf8_lossy(&data), &stem)?],
        "3mf" => read_3mf(&data, &stem)?,
        _ => return Err("Unsupported file type (use STL, OBJ or 3MF)".into()),
    };
    let parts: Vec<ImportedMesh> = parts.into_iter().filter(|p| !p.indices.is_empty()).collect();
    if parts.is_empty() {
        return Err("The file contains no triangles".into());
    }
    Ok(parts)
}

pub fn read_stl(data: &[u8], name: &str) -> Result<ImportedMesh, String> {
    // Binary if the size matches the triangle count exactly.
    let is_binary = data.len() >= 84 && {
        let n = u32::from_le_bytes(data[80..84].try_into().unwrap()) as usize;
        data.len() == 84 + n * 50
    };
    let mut positions = Vec::new();
    if is_binary {
        let n = u32::from_le_bytes(data[80..84].try_into().unwrap()) as usize;
        for t in 0..n {
            let o = 84 + t * 50 + 12;
            for v in 0..3 {
                let mut p = [0f32; 3];
                for (k, c) in p.iter_mut().enumerate() {
                    let at = o + v * 12 + k * 4;
                    *c = f32::from_le_bytes(data[at..at + 4].try_into().unwrap());
                }
                positions.push(p);
            }
        }
    } else {
        let text = String::from_utf8_lossy(data);
        for line in text.lines() {
            let mut it = line.split_whitespace();
            if it.next() == Some("vertex") {
                let v: Vec<f32> = it.filter_map(|x| x.parse().ok()).collect();
                if v.len() == 3 {
                    positions.push([v[0], v[1], v[2]]);
                }
            }
        }
        if positions.is_empty() {
            return Err("Not a valid STL file".into());
        }
    }
    positions.truncate(positions.len() / 3 * 3);
    if positions.iter().flatten().any(|c| !c.is_finite()) {
        return Err("STL contains invalid numbers".into());
    }
    let indices = (0..positions.len() as u32).collect();
    Ok(ImportedMesh { name: name.into(), positions, indices, color: None })
}

pub fn read_obj(text: &str, name: &str) -> Result<ImportedMesh, String> {
    let mut verts: Vec<[f32; 3]> = Vec::new();
    let mut indices = Vec::new();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        match it.next() {
            Some("v") => {
                let v: Vec<f32> = it.take(3).filter_map(|x| x.parse().ok()).collect();
                if v.len() == 3 {
                    verts.push([v[0], v[1], v[2]]);
                }
            }
            Some("f") => {
                let ids: Vec<u32> = it
                    .filter_map(|tok| {
                        let i: i64 = tok.split('/').next()?.parse().ok()?;
                        let idx = if i < 0 { verts.len() as i64 + i } else { i - 1 };
                        (idx >= 0 && (idx as usize) < verts.len()).then_some(idx as u32)
                    })
                    .collect();
                // Fan-triangulate polygons.
                for k in 1..ids.len().saturating_sub(1) {
                    indices.extend_from_slice(&[ids[0], ids[k], ids[k + 1]]);
                }
            }
            _ => {}
        }
    }
    if indices.is_empty() {
        return Err("OBJ contains no faces".into());
    }
    Ok(ImportedMesh { name: name.into(), positions: verts, indices, color: None })
}

fn attr(e: &quick_xml::events::BytesStart, key: &str) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.local_name().as_ref() == key)
        .map(|a| a.value.to_string())
}

fn parse_3mf_transform(s: &str) -> DMat4 {
    let v: Vec<f64> = s.split_whitespace().filter_map(|x| x.parse().ok()).collect();
    if v.len() != 12 {
        return DMat4::IDENTITY;
    }
    // 3MF: row-vector convention, m00 m01 m02 m10 m11 m12 m20 m21 m22 m30 m31 m32.
    DMat4::from_cols_array(&[
        v[0], v[1], v[2], 0.0, v[3], v[4], v[5], 0.0, v[6], v[7], v[8], 0.0, v[9], v[10], v[11], 1.0,
    ])
}

pub fn read_3mf(data: &[u8], stem: &str) -> Result<Vec<ImportedMesh>, String> {
    use quick_xml::events::Event;
    use std::collections::HashMap;
    use std::io::Read;

    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(data)).map_err(|_| "Not a valid 3MF file".to_string())?;
    let mut model_name = None;
    for i in 0..zip.len() {
        let n = zip.by_index(i).map_err(|e| e.to_string())?.name().map_err(|e| e.to_string())?.to_string();
        if n.to_lowercase().ends_with(".model") && n.to_lowercase().starts_with("3d/") {
            model_name = Some(n);
            break;
        }
    }
    let model_name = model_name.ok_or("3MF has no model")?;
    let mut xml = String::new();
    zip.by_name(&model_name)
        .map_err(|e| e.to_string())?
        .read_to_string(&mut xml)
        .map_err(|e| e.to_string())?;

    struct Obj {
        name: Option<String>,
        positions: Vec<[f32; 3]>,
        indices: Vec<u32>,
        components: Vec<(String, DMat4)>,
        color: Option<crate::doc::Rgb>,
    }
    let mut objects: HashMap<String, Obj> = HashMap::new();
    let mut materials: HashMap<String, Vec<crate::doc::Rgb>> = HashMap::new();
    let mut items: Vec<(String, DMat4)> = Vec::new();
    let mut scale = 1.0f64;
    let mut cur: Option<String> = None;
    let mut cur_mat: Option<String> = None;

    let mut reader = quick_xml::Reader::from_str(&xml);
    loop {
        let ev = reader.read_event().map_err(|e| format!("3MF XML error: {e}"))?;
        let (e, _is_empty) = match &ev {
            Event::Start(e) => (e.clone(), false),
            Event::Empty(e) => (e.clone(), true),
            Event::End(e) => {
                match e.local_name().as_ref() {
                    "object" => cur = None,
                    "basematerials" => cur_mat = None,
                    _ => {}
                }
                continue;
            }
            Event::Eof => break,
            _ => continue,
        };
        match e.local_name().as_ref() {
            "model" => {
                scale = match attr(&e, "unit").as_deref() {
                    Some("micron") => 0.001,
                    Some("centimeter") => 10.0,
                    Some("inch") => 25.4,
                    Some("foot") => 304.8,
                    Some("meter") => 1000.0,
                    _ => 1.0,
                };
            }
            "basematerials" => {
                let id = attr(&e, "id").unwrap_or_default();
                materials.insert(id.clone(), Vec::new());
                cur_mat = Some(id);
            }
            "base" => {
                if let (Some(m), Some(c)) = (&cur_mat, attr(&e, "displaycolor")) {
                    let hex: String = c.trim_start_matches('#').chars().take(6).collect();
                    if let Some(rgb) = crate::doc::Rgb::parse(&hex) {
                        materials.get_mut(m).unwrap().push(rgb);
                    }
                }
            }
            "object" => {
                let id = attr(&e, "id").unwrap_or_default();
                let color = match (attr(&e, "pid"), attr(&e, "pindex")) {
                    (Some(pid), Some(pi)) => materials
                        .get(&pid)
                        .and_then(|v| v.get(pi.parse::<usize>().unwrap_or(0)).copied()),
                    _ => None,
                };
                objects.insert(
                    id.clone(),
                    Obj { name: attr(&e, "name"), positions: Vec::new(), indices: Vec::new(), components: Vec::new(), color },
                );
                cur = Some(id);
            }
            "vertex" => {
                if let Some(o) = cur.as_ref().and_then(|c| objects.get_mut(c)) {
                    let g = |k: &str| attr(&e, k).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
                    o.positions.push([(g("x") * scale) as f32, (g("y") * scale) as f32, (g("z") * scale) as f32]);
                }
            }
            "triangle" => {
                if let Some(o) = cur.as_ref().and_then(|c| objects.get_mut(c)) {
                    let g = |k: &str| attr(&e, k).and_then(|v| v.parse::<u32>().ok());
                    if let (Some(a), Some(b), Some(c)) = (g("v1"), g("v2"), g("v3")) {
                        let n = o.positions.len() as u32;
                        if a < n && b < n && c < n {
                            o.indices.extend_from_slice(&[a, b, c]);
                        }
                    }
                }
            }
            "component" => {
                if let Some(o) = cur.as_ref().and_then(|c| objects.get_mut(c)) {
                    let t = attr(&e, "transform").map(|s| parse_3mf_transform(&s)).unwrap_or(DMat4::IDENTITY);
                    o.components.push((attr(&e, "objectid").unwrap_or_default(), t));
                }
            }
            "item" => {
                let t = attr(&e, "transform").map(|s| parse_3mf_transform(&s)).unwrap_or(DMat4::IDENTITY);
                items.push((attr(&e, "objectid").unwrap_or_default(), t));
            }
            _ => {}
        }
    }

    // Flatten build items (with components) into world-space parts.
    fn emit(
        objects: &HashMap<String, Obj>,
        id: &str,
        m: DMat4,
        scale: f64,
        depth: u32,
        name: &str,
        out: &mut Vec<ImportedMesh>,
    ) {
        let Some(o) = objects.get(id) else { return };
        if depth > 16 {
            return;
        }
        let name = o.name.clone().unwrap_or_else(|| name.to_string());
        if !o.indices.is_empty() {
            // Translations in transforms are in model units.
            let positions = o
                .positions
                .iter()
                .map(|p| {
                    let local = DVec3::new(p[0] as f64, p[1] as f64, p[2] as f64) / scale;
                    (m.transform_point3(local) * scale).as_vec3().to_array()
                })
                .collect();
            let mut indices = o.indices.clone();
            if m.determinant() < 0.0 {
                for t in indices.chunks_exact_mut(3) {
                    t.swap(1, 2);
                }
            }
            out.push(ImportedMesh { name: name.clone(), positions, indices, color: o.color });
        }
        for (cid, t) in &o.components {
            emit(objects, cid, m * *t, scale, depth + 1, &name, out);
        }
    }
    let mut out = Vec::new();
    if items.is_empty() {
        let mut ids: Vec<&String> = objects.keys().collect();
        ids.sort();
        for id in ids {
            emit(&objects, id, DMat4::IDENTITY, scale, 0, stem, &mut out);
        }
    } else {
        for (id, t) in &items {
            emit(&objects, id, *t, scale, 0, stem, &mut out);
        }
    }
    Ok(out)
}
