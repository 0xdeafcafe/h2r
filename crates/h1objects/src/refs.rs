//! Finding a Halo 1 tag's references without knowing its layout.
//!
//! A reference field is 16 bytes: group, name address, name length, tag ID.
//! A word sequence counts as a reference when the ID names a tag of that
//! group (or a group derived from it) and the name address points at that
//! tag's own name, or when it is a null reference of a group the map uses.
//! The check is strict enough that no float or index run passes it by
//! accident; the disc tests check that every reference the typed readers
//! know is found here and nothing else in their roots is.

use h1map::{Group, H1Map, Tag, TagId, TagRef};

/// One reference field found in a run of bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    /// Offset of the field in the bytes scanned.
    pub offset: usize,
    /// The field's group.
    pub group: Group,
    /// The referenced tag, or `None` for a null reference.
    pub tag: Option<TagId>,
}

/// Whether `r` read at some offset is a reference field of `map`.
pub fn is_reference(map: &H1Map, r: &TagRef) -> Option<Option<TagId>> {
    if r.group.is_none() || r.group.0 == 0 {
        return None;
    }
    if r.id.is_none() {
        // A null reference: a group some tag in the map has (or derives
        // from), and no name.
        let known = map
            .tags()
            .iter()
            .any(|t| t.group == r.group || t.parent == r.group || t.grandparent == r.group);
        return (known && r.name_len == 0).then_some(None);
    }
    let t: &Tag = map.tag(r.id).ok()?;
    let fits = t.group == r.group || t.parent == r.group || t.grandparent == r.group;
    (fits && t.name_address == r.name_address).then_some(Some(r.id))
}

/// Every reference field in `bytes`, at 4-byte steps.
pub fn scan(map: &H1Map, bytes: &[u8]) -> Vec<Found> {
    let mut out = Vec::new();
    let mut at = 0;
    while at + 16 <= bytes.len() {
        if let Some(r) = TagRef::read(bytes, at)
            && let Some(tag) = is_reference(map, &r)
        {
            out.push(Found {
                offset: at,
                group: r.group,
                tag,
            });
            at += 16;
            continue;
        }
        at += 4;
    }
    out
}
