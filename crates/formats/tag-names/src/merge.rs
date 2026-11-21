//! A display layout for one tag. The registry retains ownership of pointer
//! kinds, element strides, game-facing field names and their evidence.
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::Arc;

use tags::defs::{FieldDef, Kind as DefKind, StructDef};
use tags::registry::{Field as Row, Hint, Kind as Type, Layout};

use crate::{Field, Kind, Plugin};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameSource {
    Ours,
    External { source: &'static str },
    None,
}

impl NameSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ours => "ours",
            Self::External { source } => source,
            Self::None => "none",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowNames {
    pub name_source: NameSource,
    pub description: String,
    /// The plugin's alias for a name that has evidence in our definition.
    pub theirs: Option<String>,
    /// Additional enum labels keyed by their wire value, including sparse
    /// and negative values. Our definition's labels take precedence.
    pub options: BTreeMap<u32, String>,
}

impl RowNames {
    pub fn ours_or_offset(field: &Row) -> Self {
        Self {
            name_source: if field.named {
                NameSource::Ours
            } else {
                NameSource::None
            },
            description: field.doc.clone().unwrap_or_default(),
            theirs: None,
            options: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    pub group: String,
    pub path: String,
    pub ours: String,
    pub theirs: String,
}

#[derive(Clone, Debug)]
pub struct Merged {
    pub layout: Arc<Layout>,
    /// Metadata for matched fields. The path consists of enclosing block
    /// offsets followed by the field offset, independent of array indices.
    pub rows: BTreeMap<Vec<u32>, RowNames>,
    pub conflicts: Vec<Conflict>,
    /// Visible fields whose names come from the plugin.
    pub named: usize,
}

/// Merge a single text definition, using the registry's own rules to
/// flatten embedded structs and identify which labels are only offsets.
pub fn merge(definition: Option<&StructDef>, plugin: &Plugin, tag_size: u32) -> Merged {
    let layout = definition.map(Layout::from_definition);
    compare(layout.as_ref(), definition, plugin, tag_size)
}

/// The registry supplies schema fixups. The definition supplies the span
/// boundaries that the registry splits into editable words.
pub fn compare(
    ours: Option<&Layout>,
    definition: Option<&StructDef>,
    plugin: &Plugin,
    tag_size: u32,
) -> Merged {
    let mut walk = Walk {
        group: &plugin.group,
        path: Vec::new(),
        rows: BTreeMap::new(),
        conflicts: Vec::new(),
        named: 0,
    };
    let bound = tag_size.min(plugin.base_size);
    let layout = match ours {
        Some(layout) => {
            if layout.size != plugin.base_size {
                walk.conflicts.push(Conflict {
                    group: plugin.group.clone(),
                    path: "root".into(),
                    ours: format!("root size {:#x}", layout.size),
                    theirs: format!("baseSize {:#x}", plugin.base_size),
                });
            }
            walk.known(layout, definition, &plugin.fields, bound.min(layout.size))
        }
        None => walk.external(&plugin.group, bound, &plugin.fields),
    };
    Merged {
        layout: Arc::new(layout),
        rows: walk.rows,
        conflicts: walk.conflicts,
        named: walk.named,
    }
}

struct Walk<'a> {
    group: &'a str,
    path: Vec<u32>,
    rows: BTreeMap<Vec<u32>, RowNames>,
    conflicts: Vec<Conflict>,
    named: usize,
}

/// Sorted spans with checked ends. In an overlap both claims are ambiguous,
/// including a field wholly enclosed by an earlier, longer one.
struct Span<'a> {
    field: &'a Field,
    end: u32,
    error: Option<&'static str>,
}

impl Span<'_> {
    fn description(&self) -> String {
        let f = self.field;
        let width = f.size.map_or_else(|| "?".into(), |n| format!("{n:#x}"));
        let stride = f
            .element_size
            .map_or_else(String::new, |n| format!(", element {n:#x}"));
        let reason = self.error.map_or_else(String::new, |e| format!(", {e}"));
        format!(
            "{} at +{:#x} ({:?}, {width} bytes{stride}{reason})",
            f.name, f.offset, f.kind
        )
    }
}

fn spans(fields: &[Field], bound: u32) -> Vec<Span<'_>> {
    let mut ordered: Vec<_> = fields.iter().collect();
    ordered.sort_by_key(|f| f.offset);
    let mut high = 0;
    let mut spans = Vec::with_capacity(ordered.len());
    for (i, &field) in ordered.iter().enumerate() {
        let width = field.size.unwrap_or(0);
        let checked_end = field.offset.checked_add(width);
        let end = checked_end.unwrap_or(u32::MAX);
        let overlap = field.offset < high
            || (i > 0 && ordered[i - 1].offset == field.offset)
            || ordered.get(i + 1).is_some_and(|next| {
                next.offset == field.offset || next.offset < end
            });
        let error = if overlap {
            Some("overlapping plugin fields")
        } else if width == 0 {
            Some("unknown or zero field width")
        } else if checked_end.is_none() || end > bound {
            Some("outside the element")
        } else if field.kind == Kind::Block && field.element_size.is_none_or(|n| n == 0) {
            Some("unknown or zero block element size")
        } else {
            None
        };
        high = high.max(end);
        spans.push(Span { field, end, error });
    }
    spans
}

/// Flatten only embedded structs: a block starts a new address space and
/// its declarations are gathered when the walker enters that element.
fn declarations<'a>(
    def: &'a StructDef,
    base: u32,
    out: &mut BTreeMap<u32, &'a FieldDef>,
) {
    for f in &def.fields {
        let offset = base + f.offset;
        if let DefKind::Struct(child) = &f.kind {
            declarations(child, offset, out);
        } else {
            out.insert(offset, f);
        }
    }
}

/// The registry may put a single editor word across two declarations.
/// Split it only for comparison. Keep track of the words so a rejected
/// claim can leave exactly the original row in the result.
fn split_words<'a>(
    rows: &'a [Row],
    declared: &BTreeMap<u32, &FieldDef>,
) -> (Cow<'a, [Row]>, Vec<&'a Row>) {
    let mut split = Vec::new();
    let mut originals = Vec::new();
    let mut copied = 0;
    for (i, row) in rows.iter().enumerate() {
        let Type::Unknown { len } = row.kind else {
            continue;
        };
        let end = row.offset + len;
        if declared.range(row.offset + 1..end).next().is_none() {
            continue;
        }
        split.extend_from_slice(&rows[copied..i]);
        for offset in row.offset..end {
            let mut byte = row.clone();
            byte.offset = offset;
            byte.name = format!("+{offset:#x}");
            byte.kind = Type::Unknown { len: 1 };
            split.push(byte);
        }
        originals.push(row);
        copied = i + 1;
    }
    if copied == 0 {
        (Cow::Borrowed(rows), originals)
    } else {
        split.extend_from_slice(&rows[copied..]);
        (Cow::Owned(split), originals)
    }
}

impl Walk<'_> {
    fn issue(&mut self, at: u32, ours: String, theirs: String) {
        let path = self
            .path
            .iter()
            .chain(std::iter::once(&at))
            .map(|o| format!("+{o:#x}"))
            .collect::<Vec<_>>()
            .join(".");
        self.conflicts.push(Conflict {
            group: self.group.to_owned(),
            path,
            ours,
            theirs,
        });
    }

    fn known(
        &mut self,
        layout: &Layout,
        definition: Option<&StructDef>,
        fields: &[Field],
        bound: u32,
    ) -> Layout {
        let mut declared = BTreeMap::new();
        if let Some(def) = definition.filter(|d| d.size == layout.size) {
            declarations(def, 0, &mut declared);
        }
        let (rows, split) = split_words(&layout.fields, &declared);
        let spans = spans(fields, bound);
        let mut out = Vec::with_capacity(rows.len());
        let mut i = 0;
        while i < rows.len() {
            let first = &rows[i];
            let start = i;
            i += 1;
            if matches!(first.kind, Type::Unknown { .. }) && !first.named {
                // The registry splits contiguous unknown bytes into draggable
                // words. They can be tiled together inside one declaration.
                while i < rows.len()
                    && !declared.contains_key(&rows[i].offset)
                    && !rows[i].named
                    && matches!(rows[i].kind, Type::Unknown { .. })
                {
                    i += 1;
                }
            }
            let originals = &rows[start..i];
            let last = &originals[originals.len() - 1];
            let end = last.offset + last.kind.width();
            let lo = spans.partition_point(|s| s.field.offset < first.offset);
            let hi = spans.partition_point(|s| s.field.offset < end);
            let candidates = &spans[lo..hi];
            if !first.named && matches!(first.kind, Type::Unknown { .. } | Type::Pad { .. }) {
                self.tile(originals, end, candidates, &mut out);
            } else {
                let child = declared.get(&first.offset).and_then(|f| match &f.kind {
                    DefKind::Block(child) => Some(child.as_ref()),
                    _ => None,
                });
                out.push(self.match_row(first, child, candidates));
            }
        }
        // Restore the registry's word if all its bytes are still ours.
        // A successful attachment has a row entry even when it lacks a name.
        for original in split.into_iter().rev() {
            let start = original.offset;
            let end = start + original.kind.width();
            let lo = out.partition_point(|row| row.offset < start);
            let hi = out.partition_point(|row| row.offset < end);
            if hi - lo == (end - start) as usize && out[lo..hi].iter().all(|row| {
                self.path.push(row.offset);
                let untouched = matches!(row.kind, Type::Unknown { len: 1 })
                    && !self.rows.contains_key(&self.path);
                self.path.pop();
                untouched
            }) {
                out.splice(lo..hi, [original.clone()]);
            }
        }
        for s in spans.iter().filter(|s| s.field.offset >= layout.size) {
            self.issue(
                s.field.offset,
                format!("element ends at {:#x}", layout.size),
                s.description(),
            );
        }
        Layout {
            name: layout.name.clone(),
            size: layout.size,
            align: layout.align,
            fields: out,
        }
    }

    fn match_row(
        &mut self,
        row: &Row,
        definition: Option<&StructDef>,
        spans: &[Span<'_>],
    ) -> Row {
        let ours = || format!("{} ({})", row.name, row.kind.label());
        let mut out = row.clone();
        let [s] = spans else {
            if spans.is_empty() {
                if row.named {
                    self.issue(row.offset, ours(), "missing".into());
                }
            } else {
                for s in spans {
                    self.issue(s.field.offset, ours(), s.description());
                }
            }
            return out;
        };
        let f = s.field;
        if s.error.is_some() || f.offset != row.offset || !compatible(&row.kind, f) {
            self.issue(f.offset, ours(), s.description());
            return out;
        }
        self.path.push(row.offset);
        if let Type::Block(child) = &row.kind {
            let merged = self.known(child, definition, &f.children, child.size);
            out.kind = Type::Block(Arc::new(merged));
        }
        self.attach(&mut out, f);
        self.path.pop();
        out
    }

    fn tile(&mut self, originals: &[Row], end: u32, spans: &[Span<'_>], out: &mut Vec<Row>) {
        let first = &originals[0];
        // A registry hole carries ordinary bytes. An external claim that
        // requires relocation needs a matching canonical field at its offset.
        let mut missing_fixup = false;
        for span in spans {
            if matches!(
                span.field.kind,
                Kind::Block | Kind::DataRef | Kind::TagRef | Kind::Datum | Kind::StringId
            ) {
                self.issue(
                    span.field.offset,
                    format!("opaque range {:#x}..{end:#x} without a fixup", first.offset),
                    span.description(),
                );
                missing_fixup = true;
            }
        }
        if missing_fixup {
            out.extend_from_slice(originals);
            return;
        }
        let mut next = first.offset;
        let tiled = spans.iter().all(|s| {
            let valid = s.error.is_none() && s.field.offset == next;
            next = s.end;
            valid
        }) && next == end;
        if !tiled {
            if !spans.is_empty() {
                let summary = spans
                    .iter()
                    .map(Span::description)
                    .collect::<Vec<_>>()
                    .join(", ");
                self.issue(
                    first.offset,
                    format!("unnamed range {:#x}..{end:#x}", first.offset),
                    summary,
                );
            }
            out.extend_from_slice(originals);
            return;
        }
        for s in spans {
            let mut row = self.external_row(s.field);
            row.section.clone_from(&first.section);
            out.push(row);
        }
    }

    fn external(&mut self, name: &str, size: u32, fields: &[Field]) -> Layout {
        let mut out = Vec::new();
        let mut at = 0;
        for s in spans(fields, size) {
            if s.error.is_some() {
                self.issue(s.field.offset, format!("element size {size:#x}"), s.description());
                continue;
            }
            if at < s.field.offset {
                out.push(gap(at, s.field.offset));
            }
            out.push(self.external_row(s.field));
            at = s.end;
        }
        if at < size {
            out.push(gap(at, size));
        }
        Layout {
            name: name.to_owned(),
            size,
            align: 4,
            fields: out,
        }
    }

    /// Caller has checked offset, nonzero width, and block stride.
    fn external_row(&mut self, f: &Field) -> Row {
        let size = f.size.expect("checked span");
        self.path.push(f.offset);
        let mut row = gap(f.offset, f.offset + size);
        if f.visible {
            row.kind = match f.kind {
                Kind::Block => {
                    let stride = f.element_size.expect("checked stride");
                    let child = self.external(&f.name, stride, &f.children);
                    Type::Block(Arc::new(child))
                }
                Kind::DataRef => Type::Data,
                Kind::Datum => Type::Datum {
                    groups: Vec::new(),
                },
                Kind::PackedColour => Type::Word,
                Kind::Int16Array => Type::Int {
                    bytes: 2,
                    signed: true,
                    count: size / 2,
                },
                Kind::TagRef => Type::TagRef {
                    groups: Vec::new(),
                },
                Kind::StringId => Type::StringId,
                Kind::Int { signed } => Type::Int {
                    bytes: size as u8,
                    signed,
                    count: 1,
                },
                Kind::Bits => Type::Flags {
                    bytes: size as u8,
                    bits: Vec::new(),
                },
                Kind::Enum => Type::Enum {
                    bytes: size as u8,
                    values: Vec::new(),
                },
                Kind::Float | Kind::Vector | Kind::Colour | Kind::Degree | Kind::Bounds => {
                    Type::Float {
                        count: size / 4,
                        hint: match f.kind {
                            Kind::Colour => Hint::Colour,
                            Kind::Degree => Hint::Angle,
                            Kind::Bounds => Hint::Bounds,
                            _ => Hint::Plain,
                        },
                    }
                }
                Kind::Undefined if matches!(size, 1 | 2 | 4) => Type::Unknown { len: size },
                Kind::Undefined | Kind::Ascii | Kind::Unknown(_) => Type::Int {
                    bytes: 1,
                    signed: false,
                    count: size,
                },
            };
            self.attach(&mut row, f);
        }
        self.path.pop();
        row
    }

    fn attach(&mut self, row: &mut Row, f: &Field) {
        if !f.visible {
            return;
        }
        let mut info = RowNames::ours_or_offset(row);
        if !f.name.is_empty() {
            if row.named {
                info.theirs = Some(f.name.clone());
            } else {
                row.name.clone_from(&f.name);
                row.named = true;
                info.name_source = NameSource::External { source: "assembly" };
                self.named += 1;
            }
        }
        let text = match (info.theirs.as_deref(), f.comment.is_empty()) {
            (Some(alias), false) => format!("{alias} — {}", f.comment),
            (Some(alias), true) => alias.to_owned(),
            (None, _) => f.comment.clone(),
        };
        if !text.is_empty() {
            if !info.description.is_empty() {
                info.description.push('\n');
            }
            info.description.push_str("Assembly: ");
            info.description.push_str(&text);
        }
        if let Type::Enum { bytes, values } = &row.kind {
            let bits = u32::from(*bytes) * 8;
            let mask = (1_i64 << bits) - 1;
            for (value, label) in &f.options {
                if *value < -(1_i64 << (bits - 1)) || *value > mask {
                    continue;
                }
                let wire = (value & mask) as u32;
                if values
                    .get(wire as usize)
                    .is_none_or(|s| s.is_empty() || s == "_")
                {
                    info.options.insert(wire, label.clone());
                }
            }
        }
        if let Type::Flags { bytes, bits } = &mut row.kind {
            for (index, label) in &f.bits {
                if *index < 8 * u32::from(*bytes) {
                    let i = *index as usize;
                    if i >= bits.len() {
                        bits.resize(i + 1, "_".into());
                    }
                    if bits[i].is_empty() || bits[i] == "_" {
                        bits[i].clone_from(label);
                    }
                }
            }
        }
        self.rows.insert(self.path.clone(), info);
    }
}

fn compatible(ours: &Type, f: &Field) -> bool {
    if f.size != Some(ours.width()) {
        return false;
    }
    match (ours, &f.kind) {
        (Type::Int { signed, count: 1, .. }, Kind::Int { signed: other }) => signed == other,
        (Type::Block(child), Kind::Block) => Some(child.size) == f.element_size,
        (
            Type::Float { .. },
            Kind::Float | Kind::Vector | Kind::Colour | Kind::Degree | Kind::Bounds,
        )
        | (Type::Int { bytes: 2, signed: true, .. }, Kind::Int16Array)
        | (Type::Word, Kind::PackedColour)
        | (Type::Datum { .. }, Kind::Datum)
        | (Type::Flags { .. }, Kind::Bits)
        | (Type::Enum { .. }, Kind::Enum)
        | (Type::Data, Kind::DataRef)
        | (Type::TagRef { .. }, Kind::TagRef)
        | (Type::StringId, Kind::StringId) => true,
        _ => false,
    }
}

/// Holes in a definition supplied entirely by a plugin have no type or
/// name. Represent each as one hidden span, even for a very large offset.
fn gap(start: u32, end: u32) -> Row {
    Row {
        offset: start,
        name: format!("+{start:#x}"),
        named: false,
        kind: Type::Pad { len: end - start },
        section: String::new(),
        meaning: String::new(),
        applies: None,
        doc: None,
    }
}

#[cfg(test)]
mod tests;
