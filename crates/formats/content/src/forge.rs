//! Forge document identity and the subset a local package resolver reads.
//! The complete editing schema lives in forge-doc.

use serde::Deserialize;
use serde_json::Value;

use crate::{MapId, Sha256, canonicalize};

/// The version of the Forge document that format 2 packages carry.
pub const DOCUMENT_FORMAT: u32 = 1;
/// The document version a document with Forge labels takes.
pub const DOCUMENT_FORMAT_LABELS: u32 = 2;
/// The document version a document with an entry edit or an entry print
/// takes (docs/game/forge-map-variants.md, "The ops").
pub const DOCUMENT_FORMAT_ENTRIES: u32 = 3;
/// The document version a document with an object group takes
/// (docs/game/forge-editing.md, "Selection, groups and prefabs").
pub const DOCUMENT_FORMAT_GROUPS: u32 = 4;
/// The document version a document with a wave set takes
/// (docs/game/forge-firefight.md, "The wave set"). A package may carry any
/// of the five versions.
pub const DOCUMENT_FORMAT_WAVES: u32 = 5;

/// Whether a package's resolver reads documents of this version.
pub fn reads_document_format(format: u32) -> bool {
    matches!(
        format,
        DOCUMENT_FORMAT
            | DOCUMENT_FORMAT_LABELS
            | DOCUMENT_FORMAT_ENTRIES
            | DOCUMENT_FORMAT_GROUPS
            | DOCUMENT_FORMAT_WAVES
    )
}
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
    if !reads_document_format(format) {
        return Err(format!("unsupported document format {format}"));
    }
    Ok(Header {
        id: document_id(document)?,
        base,
        expect,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// forge-doc's format 3 golden document (`src/tests/entry.rs` there):
    /// a set, a reset and a removal of netgame flags by print.
    const ENTRIES: &str = r#"{"author":"Editor","base":{"map":"fixture","sha256":"1111111111111111111111111111111111111111111111111111111111111111"},"builder":{"bots":3,"forge_build":1,"pathfind":2},"description":"row 1\nrow 2","format":3,"imports":[],"ops":[{"edit":{"entry":{"set":{"target":{"base":{"block":"netgame_flags","index":0,"print":"1373ae4eb8ca684d"}},"write":{"field":"team","value":{"int":1}}}}},"id":1},{"edit":{"entry":{"reset":{"target":{"base":{"block":"netgame_flags","index":0,"print":"1373ae4eb8ca684d"}}}}},"id":2},{"edit":{"entry":{"remove":{"target":{"base":{"block":"netgame_flags","index":1,"print":"0123456789abcdef"}}}}},"id":3}],"recipes":[],"sources":[],"title":"Forge \"é\""}"#;

    #[test]
    fn a_format_3_header_reads_with_its_identity() {
        let header = read_header(ENTRIES.as_bytes()).unwrap();
        assert_eq!(
            header.id.to_string(),
            "be1eea848c1c67bdab49fdb7bfdaeb3c0a0eb98f0db07d9b99daaa151181795a"
        );
        assert_eq!(header.base.map, "fixture");
        assert!(header.expect.is_none());
    }

    #[test]
    fn formats_past_5_are_refused() {
        for format in [1, 2, 3, 4, 5] {
            assert!(reads_document_format(format));
        }
        let six = ENTRIES.replace("\"format\":3", "\"format\":6");
        assert_eq!(
            read_header(six.as_bytes()).err(),
            Some("unsupported document format 6".to_string())
        );
        let zero = ENTRIES.replace("\"format\":3", "\"format\":0");
        assert!(read_header(zero.as_bytes()).is_err());
    }
}
