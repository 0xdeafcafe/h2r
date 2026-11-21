//! Forge document identity and the subset a local package resolver reads.
//! The complete editing schema lives in forge-doc.

use serde::Deserialize;
use serde_json::Value;

use crate::{MapId, Sha256, canonicalize};

/// The version of the Forge document that format 2 packages carry.
pub const DOCUMENT_FORMAT: u32 = 1;
/// Largest canonical document accepted inside a package.
pub const MAX_DOCUMENT_BYTES: usize = 8 * 1024 * 1024;

/// Hash the canonical document content with its expected result removed.
pub fn document_id(mut document: Value) -> Result<Sha256, String> {
    let fields = document.as_object_mut().ok_or("want a document object")?;
    fields.remove("expect");
    let json = canonicalize(&document).map_err(|e| e.to_string())?;
    Ok(Sha256::of(json.as_bytes()))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Base {
    pub map: String,
    pub sha256: Sha256,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Expect {
    pub map_id: MapId,
    pub geometry: Sha256,
}

#[derive(Deserialize)]
struct Wire {
    format: u32,
    base: Base,
    expect: Option<Expect>,
}

pub(crate) struct Header {
    pub id: Sha256,
    pub base: Base,
    pub expect: Option<Expect>,
}

pub(crate) fn read_header(bytes: &[u8]) -> Result<Header, String> {
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return Err(format!("exceeds {MAX_DOCUMENT_BYTES} bytes"));
    }
    let document: Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    // Package files are emitted by the canonical writer. This comparison
    // also rejects ambiguous JSON with duplicate object keys.
    if canonicalize(&document)
        .map_err(|e| e.to_string())?
        .as_bytes()
        != bytes
    {
        return Err("want the canonical document file".into());
    }
    let Wire {
        format,
        base,
        expect,
    } = Wire::deserialize(&document).map_err(|e| e.to_string())?;
    if format != DOCUMENT_FORMAT {
        return Err(format!("unsupported document format {format}"));
    }
    Ok(Header {
        id: document_id(document)?,
        base,
        expect,
    })
}
