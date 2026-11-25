//! The open map's changes as edit lines (docs/editor/studio-redesign.md,
//! "Editing a disc asset"; the format is docs/editor/tag-workspace.md,
//! "Edits are data"). The tag editor holds the bytes; this is the same
//! change said by field path, as `tag.set` takes it. The window's field
//! rows and `tag.set` both write bytes through `TagDoc`, so the lines come
//! from the document's state, not from which of them wrote: the same field
//! set either way is the same line.
//!
//! A change a `set` line cannot say is named in [`Lines::skipped`], never
//! dropped quietly: a list that grew, shrank or moved, a data reference's
//! bytes, and a field with no name in the definition.
use std::collections::BTreeSet;

use ::workspace::{SetLine, TagEdit};
use map::TagId;
use tags::registry::{Field, Kind, Layout};

use super::tag_api;
use super::tag_doc::{Access, TagDoc};

/// The lines of the changed tags, and what they leave out.
#[derive(Debug, Default)]
pub struct Lines {
    /// One entry per changed tag that has at least one line, by tag.
    pub edits: Vec<TagEdit>,
    /// Every tag that is changed, whether or not it has a line.
    pub changed: BTreeSet<String>,
    /// What a line cannot say, with the tag and the field.
    pub skipped: Vec<String>,
}

/// `<group>:<path>`, the form an edit line names a tag by; none for a tag
/// the map has no path for.
pub fn tag_ref(doc: &TagDoc, id: TagId) -> Option<String> {
    let t = doc.tag(id)?;
    (!t.name.starts_with('#')).then(|| format!("{}:{}", tags::defs::fourcc_text(t.group), t.name))
}

/// The edit lines of every changed tag of `doc`.
pub fn of_doc(doc: &TagDoc) -> Lines {
    let mut out = Lines::default();
    for t in doc.tags() {
        if !doc.is_changed(t.id) {
            continue;
        }
        let Some(name) = tag_ref(doc, t.id) else {
            out.skipped
                .push(format!("tag {:#010x}: the map gives it no path", t.id.0));
            continue;
        };
        out.changed.insert(name.clone());
        if !matches!(t.access, Access::Edit) {
            continue;
        }
        let Some(layout) = doc.layout(t.id) else {
            continue;
        };
        let mut walk = Walk {
            doc,
            tag: t.id,
            name: &name,
            sets: Vec::new(),
            skipped: Vec::new(),
        };
        walk.element(t.address, layout, "");
        out.skipped.extend(walk.skipped);
        if !walk.sets.is_empty() {
            out.edits.push(TagEdit {
                tag: name.clone(),
                was: None,
                block: Vec::new(),
                set: walk.sets,
            });
        }
    }
    out
}

struct Walk<'a> {
    doc: &'a TagDoc,
    tag: TagId,
    name: &'a str,
    sets: Vec<SetLine>,
    skipped: Vec<String>,
}

/// A field's name in a definition path: inside the embedded structs it
/// sits in, as `section.field`.
fn path_name(f: &Field) -> String {
    if f.section.is_empty() {
        f.name.clone()
    } else {
        format!("{}.{}", f.section, f.name)
    }
}

impl Walk<'_> {
    /// Whether the bytes at `a` are not the file's.
    fn differs(&self, a: u32, w: u32) -> bool {
        match (self.doc.read(self.tag, a, w), self.doc.read_source(self.tag, a, w)) {
            (Some(now), Some(then)) => now != then,
            (Some(_), None) => true,
            _ => false,
        }
    }

    fn skip(&mut self, label: &str, why: &str) {
        self.skipped.push(format!("{}: {label}: {why}", self.name));
    }

    fn element(&mut self, at: u32, layout: &Layout, prefix: &str) {
        for f in &layout.fields {
            let a = at + f.offset;
            let w = f.kind.width();
            let own = path_name(f);
            let label = if prefix.is_empty() {
                own
            } else {
                format!("{prefix}.{own}")
            };
            match &f.kind {
                Kind::Pad { .. } => {}
                Kind::Block(child) => {
                    if self.differs(a, w) {
                        self.skip(&label, "the list changed length or moved, which a set line cannot say");
                        continue;
                    }
                    let Some((n, first)) = self.doc.block(self.tag, a, child) else {
                        continue;
                    };
                    for i in 0..n {
                        self.element(first + i * child.size, child, &format!("{label}[{i}]"));
                    }
                }
                Kind::Data => {
                    if self.differs(a, w) {
                        self.skip(&label, "a data reference's bytes are not an edit line");
                    }
                }
                Kind::Int { .. }
                | Kind::Float { .. }
                | Kind::Enum { .. }
                | Kind::Flags { .. }
                | Kind::StringId
                | Kind::TagRef { .. }
                | Kind::Datum { .. } => {
                    if !self.differs(a, w) {
                        continue;
                    }
                    if !f.named {
                        self.skip(&label, "the field has no name in the definition");
                        continue;
                    }
                    let Some(bytes) = self.doc.read(self.tag, a, w) else {
                        continue;
                    };
                    let value = tag_api::value_of(self.doc, self.tag, at, f, &bytes);
                    match SetLine::from_json(&label, &value) {
                        Ok(line) => self.sets.push(line),
                        Err(why) => self.skip(&label, &why),
                    }
                }
                _ => {
                    if self.differs(a, w) {
                        self.skip(&label, "the field's kind is not an edit line yet");
                    }
                }
            }
        }
    }
}
