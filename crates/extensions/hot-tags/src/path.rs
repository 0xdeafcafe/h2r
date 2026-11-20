//! Field paths: from a tag's root, through its definition
//! (`tags::defs`, the field names and offsets the editor crates use), to
//! one field's guest address.
//!
//! A path is segments joined by `.`:
//! * `name`: a field of the current element. Embedded structs are looked
//!   through, so `bounding_radius` finds `obje.bounding_radius` in a `weap`;
//!   naming the struct (`obje.bounding_radius`) works too.
//! * `name[i]`: element `i` of a block (followed by its stored count and
//!   address), or component `i` of a field with several values
//!   (`specular_color[2]`).
//! * `+0x10:f32[3]` (last only): a raw field at that offset of the current
//!   element, for groups or bytes the definitions don't name. Types:
//!   `u8 u16 u32 i8 i16 i32 f32`, with `[n]` for several, or `bytes[n]`.
//! * `+0x50/0x54[3]`: element 3 of a raw block, the (count, address) pair
//!   at offset `0x50` of the current element, `0x54` bytes per element;
//!   the index is checked against the stored count. Only raw segments may
//!   follow (the element has no definition): `+0x50/0x54[3].+0x8:f32[3]`
//!   is a scenario placement's position (the layout is the caller's, e.g.
//!   `scenario_edit::layout`). Last, it is the element's bytes.
//!
//! The empty path is the tag's root element.

use runtime::Guest;
use tags::defs::{Kind, StructDef};

/// Where a path ends.
#[derive(Clone, Debug)]
pub struct Located<'d> {
    pub addr: u32,
    pub kind: Kind,
    /// The element definition, when the path ends at an element (the root,
    /// a block element, an embedded struct).
    pub element: Option<&'d StructDef>,
    /// What the definition says the field is.
    pub meaning: String,
}

/// A path segment.
#[derive(Clone, Debug, PartialEq)]
enum Segment {
    Field {
        name: String,
        index: Option<u32>,
    },
    Raw {
        offset: u32,
        kind: Kind,
    },
    /// Element `index` of the block at `offset`, `stride` bytes apart.
    RawBlock {
        offset: u32,
        stride: u32,
        index: u32,
    },
}

fn parse_num(s: &str) -> Option<u32> {
    match s.strip_prefix("0x") {
        Some(h) => u32::from_str_radix(h, 16).ok(),
        None => s.parse().ok(),
    }
}

/// A raw type: `f32`, `u16[4]`, `bytes[8]`.
pub fn raw_kind(t: &str) -> Result<Kind, String> {
    let (base, count) = match t.split_once('[') {
        Some((b, rest)) => {
            let n = rest
                .strip_suffix(']')
                .and_then(parse_num)
                .filter(|n| (1..=4096).contains(n))
                .ok_or_else(|| format!("{t}: bad count"))?;
            (b, n)
        }
        None => (t, 1),
    };
    Ok(match base {
        "u8" => Kind::Int {
            bytes: 1,
            signed: false,
            count,
        },
        "u16" => Kind::Int {
            bytes: 2,
            signed: false,
            count,
        },
        "u32" => Kind::Int {
            bytes: 4,
            signed: false,
            count,
        },
        "i8" => Kind::Int {
            bytes: 1,
            signed: true,
            count,
        },
        "i16" => Kind::Int {
            bytes: 2,
            signed: true,
            count,
        },
        "i32" => Kind::Int {
            bytes: 4,
            signed: true,
            count,
        },
        "f32" => Kind::Float { count },
        "bytes" => Kind::Unknown {
            len: count,
            seen: String::new(),
        },
        _ => {
            return Err(format!(
                "{t}: not a raw type (u8 u16 u32 i8 i16 i32 f32 bytes[n])"
            ));
        }
    })
}

fn parse(path: &str) -> Result<Vec<Segment>, String> {
    let path = path.trim();
    if path.is_empty() {
        return Ok(Vec::new());
    }
    let parts: Vec<&str> = path.split('.').collect();
    let mut out = Vec::new();
    for (i, p) in parts.iter().enumerate() {
        if let Some(raw) = p.strip_prefix('+')
            && !raw.contains(':')
        {
            out.push(raw_block(p, raw)?);
            continue;
        }
        if let Some(raw) = p.strip_prefix('+') {
            if i + 1 != parts.len() {
                return Err(format!("{p}: a raw field must be last"));
            }
            let (off, ty) = raw.split_once(':').ok_or_else(|| {
                format!("{p}: a raw field is +<offset>:<type>, e.g. +0x10:f32[3]")
            })?;
            let offset = parse_num(off).ok_or_else(|| format!("{p}: bad offset"))?;
            out.push(Segment::Raw {
                offset,
                kind: raw_kind(ty)?,
            });
            continue;
        }
        let (name, index) = match p.split_once('[') {
            Some((n, rest)) => {
                let i = rest
                    .strip_suffix(']')
                    .and_then(parse_num)
                    .ok_or_else(|| format!("{p}: bad index"))?;
                (n, Some(i))
            }
            None => (*p, None),
        };
        if name.is_empty() {
            return Err(format!("empty segment in {path:?}"));
        }
        out.push(Segment::Field {
            name: name.into(),
            index,
        });
    }
    Ok(out)
}

/// `+0x50/0x54[3]`: a raw block's element.
fn raw_block(p: &str, raw: &str) -> Result<Segment, String> {
    let bad = || {
        format!(
            "{p}: a raw field is +<offset>:<type> (+0x10:f32[3]), a raw block +<offset>/<stride>[<index>] (+0x50/0x54[3])"
        )
    };
    let (off, rest) = raw.split_once('/').ok_or_else(bad)?;
    let (stride, index) = rest.split_once('[').ok_or_else(bad)?;
    let index = index.strip_suffix(']').ok_or_else(bad)?;
    let offset = parse_num(off).ok_or_else(|| format!("{p}: bad offset"))?;
    let stride = parse_num(stride)
        .filter(|s| (1..=0x10000).contains(s))
        .ok_or_else(|| format!("{p}: bad stride"))?;
    let index = parse_num(index).ok_or_else(|| format!("{p}: bad index"))?;
    Ok(Segment::RawBlock {
        offset,
        stride,
        index,
    })
}

/// Follows raw segments from an element at `base` (`what` names it).
fn follow_raw(
    g: &Guest,
    base: u32,
    what: &str,
    segments: &[Segment],
) -> Result<Located<'static>, String> {
    let mut base = base;
    let mut what = what.to_owned();
    for (i, seg) in segments.iter().enumerate() {
        let last = i + 1 == segments.len();
        match seg {
            Segment::Raw { offset, kind } => {
                return Ok(Located {
                    addr: base + offset,
                    kind: kind.clone(),
                    element: None,
                    meaning: format!("raw field at {offset:#x} of {what}"),
                });
            }
            Segment::RawBlock {
                offset,
                stride,
                index,
            } => {
                let name = format!("+{offset:#x}");
                let count = g
                    .u32(base + offset)
                    .map_err(|e| format!("{name}: guest fault at {:#x}", e.addr))?;
                let at = g
                    .u32(base + offset + 4)
                    .map_err(|e| format!("{name}: guest fault at {:#x}", e.addr))?;
                if count > BLOCK_MAX {
                    return Err(format!("{name}: count {count} is not a block"));
                }
                if *index >= count {
                    return Err(format!("{name}[{index}]: the block has {count} elements"));
                }
                base = at + index * stride;
                what = format!("{name}[{index}]");
                if last {
                    return Ok(Located {
                        addr: base,
                        kind: Kind::Unknown {
                            len: *stride,
                            seen: String::new(),
                        },
                        element: None,
                        meaning: format!("raw block element {what}"),
                    });
                }
            }
            Segment::Field { name, .. } => {
                return Err(format!(
                    "{name}: after a raw block only raw segments (+<offset>:<type>) name fields"
                ));
            }
        }
    }
    Err("an empty raw path".into())
}

/// A field named `name` in `s` or, looking through embedded structs, in
/// one of them: the field and its offset in `s`.
fn find<'d>(s: &'d StructDef, name: &str) -> Option<(&'d tags::defs::FieldDef, u32)> {
    if let Some(f) = s.field(name) {
        return Some((f, f.offset));
    }
    s.fields.iter().find_map(|f| match &f.kind {
        Kind::Struct(inner) => find(inner, name).map(|(g, off)| (g, f.offset + off)),
        _ => None,
    })
}

/// Width of one value of a several-valued field.
fn component(kind: &Kind) -> Option<(Kind, u32, u32)> {
    match kind {
        Kind::Int {
            bytes,
            signed,
            count,
        } if *count > 1 => Some((
            Kind::Int {
                bytes: *bytes,
                signed: *signed,
                count: 1,
            },
            u32::from(*bytes),
            *count,
        )),
        Kind::Float { count } if *count > 1 => Some((Kind::Float { count: 1 }, 4, *count)),
        _ => None,
    }
}

/// A raw field (`+0x10:f32`) of a tag at `base` with no definition, or
/// one in a raw block's element (`+0x50/0x54[3].+0x8:f32[3]`).
pub fn raw(g: &Guest, base: u32, path: &str) -> Result<Located<'static>, String> {
    let segments = parse(path)?;
    if segments.is_empty() || segments.iter().any(|s| matches!(s, Segment::Field { .. })) {
        return Err(format!("{path:?} is not a raw path"));
    }
    follow_raw(g, base, "the root", &segments)
}

/// Most elements a block may claim before it is taken for garbage.
const BLOCK_MAX: u32 = 1 << 20;

/// Follows `path` from the root element `root` of a tag at `base`.
pub fn resolve<'d>(
    g: &Guest,
    root: &'d StructDef,
    base: u32,
    path: &str,
) -> Result<Located<'d>, String> {
    let segments = parse(path)?;
    let mut cur = root;
    let mut base = base;
    let mut here = Located {
        addr: base,
        kind: Kind::Struct(Box::new(root.clone())),
        element: Some(root),
        meaning: String::new(),
    };
    let n = segments.len();
    for (i, seg) in segments.iter().cloned().enumerate() {
        let last = i + 1 == n;
        match seg {
            Segment::Raw { .. } | Segment::RawBlock { .. } => {
                return follow_raw(g, base, &cur.name, &segments[i..]);
            }
            Segment::Field { name, index } => {
                let (f, off) = find(cur, &name).ok_or_else(|| {
                    format!(
                        "{}: no field {name:?} (fields: {})",
                        cur.name,
                        cur.fields
                            .iter()
                            .map(|f| f.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                })?;
                let addr = base + off;
                match (&f.kind, index) {
                    (Kind::Block(elem), Some(ix)) => {
                        let count = g
                            .u32(addr)
                            .map_err(|e| format!("{name}: guest fault at {:#x}", e.addr))?;
                        let at = g
                            .u32(addr + 4)
                            .map_err(|e| format!("{name}: guest fault at {:#x}", e.addr))?;
                        if count > BLOCK_MAX {
                            return Err(format!("{name}: count {count} is not a block"));
                        }
                        if ix >= count {
                            return Err(format!("{name}[{ix}]: the block has {count} elements"));
                        }
                        base = at + ix * elem.size;
                        cur = elem.as_ref();
                        here = Located {
                            addr: base,
                            kind: Kind::Struct(elem.clone()),
                            element: Some(elem.as_ref()),
                            meaning: f.meaning.clone(),
                        };
                    }
                    (Kind::Block(_), None) if !last => {
                        return Err(format!("{name} is a block: index it, {name}[0]"));
                    }
                    (Kind::Struct(inner), None) => {
                        base = addr;
                        cur = inner.as_ref();
                        here = Located {
                            addr,
                            kind: f.kind.clone(),
                            element: Some(inner.as_ref()),
                            meaning: f.meaning.clone(),
                        };
                    }
                    (kind, Some(ix)) => {
                        let (one, width, count) = component(kind)
                            .ok_or_else(|| format!("{name} has one value: no [{ix}]"))?;
                        if ix >= count {
                            return Err(format!("{name}[{ix}]: it has {count} values"));
                        }
                        if !last {
                            return Err(format!("{name}[{ix}] is a value: nothing after it"));
                        }
                        return Ok(Located {
                            addr: addr + ix * width,
                            kind: one,
                            element: None,
                            meaning: f.meaning.clone(),
                        });
                    }
                    (kind, None) => {
                        if !last {
                            return Err(format!("{name} is a value: nothing after it"));
                        }
                        return Ok(Located {
                            addr,
                            kind: kind.clone(),
                            element: None,
                            meaning: f.meaning.clone(),
                        });
                    }
                }
            }
        }
    }
    Ok(here)
}
