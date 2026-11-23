//! A hash of each group's layout, so a tag tree extracted under one schema
//! knows which groups a later schema describes differently
//! (`docs/formats/tag-repo.md`, "Versions").
//!
//! A group's hash covers its own layout in [`Schema::to_text`] form. A
//! group with a raw field also covers every group's raw fields: a raw
//! resource with no stored length runs to the next raw start of its file,
//! and any group's raw pointers are those starts. A group the schema has no
//! layout for hashes as "none", so gaining a layout moves its hash.
use super::{Field, Schema, Struct, hex_fourcc};
use map::FourCC;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// What a hash covers. Change it when the hash's input changes shape.
const INPUT: &str = "h2 tag layout hash 1\n";

/// Per-group layout hashes of one schema.
pub struct LayoutHashes<'s> {
    schema: &'s Schema,
    /// Hash of every group's raw-field paths.
    raw: [u8; 32],
}

impl<'s> LayoutHashes<'s> {
    /// Hashes over `schema`.
    pub fn new(schema: &'s Schema) -> Self {
        let mut text = String::new();
        for (g, root) in &schema.groups {
            let mut paths = Vec::new();
            raw_paths(root, &mut Vec::new(), &mut paths);
            if !paths.is_empty() {
                let _ = writeln!(text, "{} {}", hex_fourcc(*g), paths.join(" "));
            }
        }
        LayoutHashes {
            schema,
            raw: Sha256::digest(text.as_bytes()).into(),
        }
    }

    /// The layout text of `group` this hash covers: its schema lines, or
    /// `group <name> none`.
    pub fn group_text(&self, group: FourCC) -> String {
        match self.schema.groups.get(&group) {
            Some(root) => Schema {
                groups: BTreeMap::from([(group, root.clone())]),
            }
            .to_text(),
            None => format!("group {} none\n", hex_fourcc(group)),
        }
    }

    /// The hash of `group`'s layout, with `extra` (what a user of the
    /// layout adds for the group, e.g. the extractor's raw-size pairs).
    pub fn of(&self, group: FourCC, extra: &str) -> [u8; 32] {
        let mut h = Sha256::new();
        h.update(INPUT.as_bytes());
        h.update(self.group_text(group).as_bytes());
        if self.schema.groups.get(&group).is_some_and(has_raw) {
            h.update(b"raw starts ");
            h.update(self.raw);
            h.update(b"\n");
        }
        h.update(extra.as_bytes());
        h.finalize().into()
    }
}

fn has_raw(s: &Struct) -> bool {
    s.fields.values().any(|f| match f {
        Field::Raw => true,
        Field::Block(c) => has_raw(c),
        _ => false,
    })
}

/// Every raw field of `s`, as `offset/offset/...` from the root.
fn raw_paths(s: &Struct, path: &mut Vec<u32>, out: &mut Vec<String>) {
    for (&o, f) in &s.fields {
        match f {
            Field::Raw => {
                let mut p: String = path.iter().map(|o| format!("{o:#x}/")).collect();
                let _ = write!(p, "{o:#x}");
                out.push(p);
            }
            Field::Block(c) => {
                path.push(o);
                raw_paths(c, path, out);
                path.pop();
            }
            _ => {}
        }
    }
}
