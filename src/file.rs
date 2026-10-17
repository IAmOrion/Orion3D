//! `.orion3d` file format.
//!
//! A zip archive containing:
//!   design.json  - the full node tree (human readable)
//!
//! The tree is the only source of truth: meshes are never stored for
//! shapes or groups, so every group can always be ungrouped.

use crate::doc::{Document, FORMAT_NAME, FORMAT_VERSION};
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
    from_json(&buf)
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
