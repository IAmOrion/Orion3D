//! `.orion3d` file format.
//!
//! A zip archive containing:
//!   design.json       - the full node tree (human readable)
//!   meshes/<id>.bin   - triangles of imported meshes (if any)
//!
//! The tree is the only source of truth: meshes are never stored for
//! shapes or groups, so every group can always be ungrouped.

use crate::doc::{Content, Document, FORMAT_NAME, FORMAT_VERSION};
use std::io::{Read, Write};
use std::path::Path;

pub const EXTENSION: &str = "orion3d";

pub fn save(doc: &Document, path: &Path) -> Result<(), String> {
    let json = serde_json::to_vec_pretty(doc).map_err(|e| e.to_string())?;
    // Write to a temp file first so a failed save never destroys the old file.
    let tmp = path.with_extension("orion3d.tmp");
    {
        let f = std::fs::File::create(&tmp).map_err(|e| format!("Cannot write file: {e}"))?;
        let mut zip = zip::ZipWriter::new(f);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("design.json", opts).map_err(|e| e.to_string())?;
        zip.write_all(&json).map_err(|e| e.to_string())?;
        // Imported meshes, each stored once.
        let mut written = std::collections::HashSet::new();
        for n in &doc.nodes {
            let mut err = None;
            n.walk(&mut |c| {
                if let Content::Mesh(m) = &c.content {
                    if err.is_none() && written.insert(m.id.clone()) {
                        let r = zip
                            .start_file(m.file_name(), opts)
                            .map_err(|e| e.to_string())
                            .and_then(|_| zip.write_all(&m.to_bytes()).map_err(|e| e.to_string()));
                        if let Err(e) = r {
                            err = Some(e);
                        }
                    }
                }
            });
            if let Some(e) = err {
                return Err(e);
            }
        }
        zip.finish().map_err(|e| e.to_string())?;
    }
    std::fs::rename(&tmp, path).map_err(|e| format!("Cannot replace file: {e}"))?;
    Ok(())
}

pub fn load(path: &Path) -> Result<Document, String> {
    let f = std::fs::File::open(path).map_err(|e| format!("Cannot open file: {e}"))?;
    let mut zip = zip::ZipArchive::new(f).map_err(|_| "Not an Orion3D file".to_string())?;
    let mut entry = zip
        .by_name("design.json")
        .map_err(|_| "File has no design.json".to_string())?;
    let mut buf = Vec::new();
    entry.read_to_end(&mut buf).map_err(|e| e.to_string())?;
    drop(entry);
    let mut doc = from_json(&buf)?;
    // Fill in imported meshes from the archive.
    let mut missing = None;
    for n in &mut doc.nodes {
        n.walk_mut(&mut |c| {
            if let Content::Mesh(m) = &mut c.content {
                let mut data = Vec::new();
                let ok = zip
                    .by_name(&m.file_name())
                    .ok()
                    .and_then(|mut e| e.read_to_end(&mut data).ok())
                    .is_some();
                if !ok || m.fill_from_bytes(&data).is_err() {
                    missing = Some(c.name.clone());
                }
            }
        });
    }
    if let Some(name) = missing {
        return Err(format!("The mesh data for \"{name}\" is missing or damaged"));
    }
    Ok(doc)
}

pub fn from_json(buf: &[u8]) -> Result<Document, String> {
    let mut doc: Document =
        serde_json::from_slice(buf).map_err(|e| format!("Design data is invalid: {e}"))?;
    if doc.format != FORMAT_NAME {
        return Err("Not an Orion3D design".into());
    }
    if doc.version > FORMAT_VERSION {
        return Err(format!(
            "This design was made with a newer Orion3D (format v{}). Please update.",
            doc.version
        ));
    }
    doc.fix_ids();
    Ok(doc)
}
