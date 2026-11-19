//! Typed tag definitions: every byte of a group's tags as a named field or
//! an explicit `unknown` span.
//!
//! [`crate::schema`] knows where the pointers, references, string IDs and
//! raw pointers are, and nothing else. A definition here knows every field:
//! its offset, its type (integer, float, enum, flags, string ID, tag
//! reference, block, data, raw, ...), our own name for it and what it
//! means, with the evidence for the name. Bytes nobody understands yet are
//! `unknown` spans, never left implicit, so a tag built from a definition
//! has a defined value in every byte.
//!
//! From a definition ([`StructDef`]) a tag can be decoded from its loose
//! form ([`crate::repo::LooseTag`]) into a value tree ([`Record`]), encoded
//! back ([`encode`]), built into a [`map::build::Block`] tree for the map
//! builder ([`to_built`]), read back from one ([`from_built`]), created
//! fresh with every field at its default ([`default_record`]) and compared
//! field by field with another ([`diff`]).
//!
//! The definitions are checked-in text files (`src/defs/*.txt`, format in
//! `docs/formats/tag-definitions.md`), inferred from retail data and the
//! XBE's readers. No community definition was used. The files record
//! offsets, types and our names; never retail values at length.

use map::FourCC;
use std::collections::BTreeMap;
use std::sync::OnceLock;

pub mod skeleton;
pub mod value;
pub use value::*;

#[cfg(test)]
mod tests;

/// The checked-in definition files, parsed in this order: a struct is
/// defined before the files that use it.
pub const FILES: &[(&str, &str)] = &[
    ("obje.txt", include_str!("defs/obje.txt")),
    ("scen.txt", include_str!("defs/scen.txt")),
    ("bloc.txt", include_str!("defs/bloc.txt")),
    ("ssce.txt", include_str!("defs/ssce.txt")),
    ("crea.txt", include_str!("defs/crea.txt")),
    ("unit.txt", include_str!("defs/unit.txt")),
    ("item.txt", include_str!("defs/item.txt")),
    ("devi.txt", include_str!("defs/devi.txt")),
    ("proj.txt", include_str!("defs/proj.txt")),
    ("jpt_.txt", include_str!("defs/jpt_.txt")),
    ("hlmt.txt", include_str!("defs/hlmt.txt")),
    ("mode.txt", include_str!("defs/mode.txt")),
    ("coll.txt", include_str!("defs/coll.txt")),
    ("phmo.txt", include_str!("defs/phmo.txt")),
    ("shad.txt", include_str!("defs/shad.txt")),
    ("bitm.txt", include_str!("defs/bitm.txt")),
    ("jmad.txt", include_str!("defs/jmad.txt")),
    ("snd_.txt", include_str!("defs/snd_.txt")),
    ("lsnd.txt", include_str!("defs/lsnd.txt")),
    ("sky_.txt", include_str!("defs/sky_.txt")),
    ("effe.txt", include_str!("defs/effe.txt")),
    ("prt3.txt", include_str!("defs/prt3.txt")),
    ("cont.txt", include_str!("defs/cont.txt")),
    ("ligh.txt", include_str!("defs/ligh.txt")),
    ("deca.txt", include_str!("defs/deca.txt")),
    ("udlg.txt", include_str!("defs/udlg.txt")),
    ("foot.txt", include_str!("defs/foot.txt")),
    ("lens.txt", include_str!("defs/lens.txt")),
    ("garb.txt", include_str!("defs/garb.txt")),
    ("itmc.txt", include_str!("defs/itmc.txt")),
    ("vehc.txt", include_str!("defs/vehc.txt")),
    ("sily.txt", include_str!("defs/sily.txt")),
    ("stem.txt", include_str!("defs/stem.txt")),
    ("spas.txt", include_str!("defs/spas.txt")),
    ("vrtx.txt", include_str!("defs/vrtx.txt")),
    ("ugh_.txt", include_str!("defs/ugh_.txt")),
    ("pmov.txt", include_str!("defs/pmov.txt")),
    ("MGS2.txt", include_str!("defs/MGS2.txt")),
    ("skin.txt", include_str!("defs/skin.txt")),
    ("wigl.txt", include_str!("defs/wigl.txt")),
    ("wgtz.txt", include_str!("defs/wgtz.txt")),
    ("wgit.txt", include_str!("defs/wgit.txt")),
    ("unic.txt", include_str!("defs/unic.txt")),
    ("goof.txt", include_str!("defs/goof.txt")),
    ("matg.txt", include_str!("defs/matg.txt")),
    ("sbsp.txt", include_str!("defs/sbsp.txt")),
    ("ltmp.txt", include_str!("defs/ltmp.txt")),
    ("fpch.txt", include_str!("defs/fpch.txt")),
    ("snde.txt", include_str!("defs/snde.txt")),
    ("fog_.txt", include_str!("defs/fog_.txt")),
    ("nhdt.txt", include_str!("defs/nhdt.txt")),
    ("trak.txt", include_str!("defs/trak.txt")),
];

/// One array element's layout (a group's root, a block's element, or a
/// struct embedded in either).
#[derive(Clone, Debug, PartialEq)]
pub struct StructDef {
    /// Our name for it (a group's root is named after the group).
    pub name: String,
    /// Element size in bytes.
    pub size: u32,
    /// Alignment of an array of these.
    pub align: u32,
    /// Every field, by ascending offset, tiling `0..size` exactly.
    pub fields: Vec<FieldDef>,
}

impl FieldDef {
    /// A field whose type is known but whose role is not: by convention
    /// its name ends in `_<offset in hex>` (`string_1c`, `foot_ref_54`).
    pub fn typed_only(&self) -> bool {
        self.name.ends_with(&format!("_{:x}", self.offset))
    }
}

/// One field of a [`StructDef`].
#[derive(Clone, Debug, PartialEq)]
pub struct FieldDef {
    /// Byte offset in the element.
    pub offset: u32,
    /// Our name. Unknown spans are `unknown_<offset>`, padding `pad_<offset>`.
    pub name: String,
    /// Type.
    pub kind: Kind,
    /// What it means and the evidence for the name (`E` engine code, `V`
    /// checked on the retail disc, `D` a choice made here).
    pub meaning: String,
    /// Default value text for a fresh tag (`default=` in the file), else
    /// zero, null or empty.
    pub default: Option<String>,
}

/// A field's type.
#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    /// Integer of 1, 2 or 4 bytes, `count` of them in a row.
    Int { bytes: u8, signed: bool, count: u32 },
    /// IEEE single, `count` in a row (a point, vector, colour, bounds).
    Float { count: u32 },
    /// Enumeration of 1, 2 or 4 bytes; `values[i]` names value `i`.
    Enum { bytes: u8, values: Vec<String> },
    /// Flag bits of 1, 2 or 4 bytes; `bits[i]` names bit `i` (`_` unnamed).
    Flags { bytes: u8, bits: Vec<String> },
    /// 4 bytes: a string ID.
    StringId,
    /// 8 bytes: group word, then the datum of a tag of one of `groups`
    /// (empty: any).
    TagRef { groups: Vec<FourCC> },
    /// 4 bytes: a bare datum.
    Datum { groups: Vec<FourCC> },
    /// 4 bytes: a raw-resource pointer.
    Raw,
    /// 4 bytes: byte length of the raw resource whose pointer is at `of`
    /// in the same element.
    RawSize { of: u32 },
    /// 4 bytes of plain data that can look like an address (packed indices
    /// with flag bits).
    Word,
    /// 12 bytes: a scenario BSP descriptor.
    Bsp,
    /// 8 bytes: count and address of an array of these elements.
    Block(Box<StructDef>),
    /// 8 bytes: byte length and address of bytes.
    Data { align: u32 },
    /// A struct embedded at this offset.
    Struct(Box<StructDef>),
    /// Bytes not understood yet. `seen` summarises the retail values
    /// (`zero`, `const`, `<n>` distinct, `many`), never the values.
    Unknown { len: u32, seen: String },
    /// Bytes zero in every retail instance and read by nothing found.
    Pad { len: u32 },
}

impl Kind {
    /// Width in the element.
    pub fn width(&self) -> u32 {
        match self {
            Kind::Int { bytes, count, .. } => u32::from(*bytes) * count,
            Kind::Float { count } => 4 * count,
            Kind::Enum { bytes, .. } | Kind::Flags { bytes, .. } => u32::from(*bytes),
            Kind::StringId | Kind::Datum { .. } | Kind::Raw | Kind::RawSize { .. } | Kind::Word => {
                4
            }
            Kind::TagRef { .. } | Kind::Block(_) | Kind::Data { .. } => 8,
            Kind::Bsp => 12,
            Kind::Struct(s) => s.size,
            Kind::Unknown { len, .. } | Kind::Pad { len } => *len,
        }
    }

    /// Whether the field is an unknown span.
    pub fn is_unknown(&self) -> bool {
        matches!(self, Kind::Unknown { .. })
    }
}

/// Every definition: named structs and group roots.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Defs {
    /// Structs by name (`struct` lines), for embedding.
    pub structs: BTreeMap<String, StructDef>,
    /// Group roots (`group` lines).
    pub groups: BTreeMap<FourCC, StructDef>,
}

/// A definition file that does not parse.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{file}:{line}: {why}")]
pub struct ParseError {
    /// File name.
    pub file: String,
    /// 1-based line.
    pub line: usize,
    /// What is wrong.
    pub why: String,
}

impl Defs {
    /// The checked-in definitions ([`FILES`]).
    pub fn builtin() -> &'static Defs {
        static DEFS: OnceLock<Defs> = OnceLock::new();
        DEFS.get_or_init(|| {
            let mut d = Defs::default();
            for (name, text) in FILES {
                d.parse_into(name, text)
                    .unwrap_or_else(|e| panic!("built-in tag definitions: {e}"));
            }
            d
        })
    }

    /// Parses one file into these definitions (structs it uses must be
    /// defined already).
    pub fn parse_into(&mut self, file: &str, text: &str) -> Result<(), ParseError> {
        let lines = lex(file, text)?;
        let mut i = 0;
        while i < lines.len() {
            let l = &lines[i];
            let err = |why: String| ParseError {
                file: file.into(),
                line: l.number,
                why,
            };
            if l.indent != 0 {
                return Err(err("expected `group` or `struct` at column 0".into()));
            }
            let t: Vec<&str> = l.text.split_whitespace().collect();
            let (is_group, name) = match t.first() {
                Some(&"group") => (true, t.get(1).copied()),
                Some(&"struct") => (false, t.get(1).copied()),
                _ => return Err(err("expected `group` or `struct`".into())),
            };
            let name = name.ok_or_else(|| err("missing name".into()))?;
            let size = t
                .get(2)
                .and_then(|s| parse_num(s))
                .ok_or_else(|| err("missing size".into()))?;
            let align = match (t.get(3), t.get(4)) {
                (Some(&"align"), Some(a)) => parse_num(a).ok_or_else(|| err("bad align".into()))?,
                (None, _) => 4,
                _ => return Err(err("expected `align <n>`".into())),
            };
            i += 1;
            let fields = self.fields(file, &lines, &mut i, 2)?;
            let s = StructDef {
                name: name.to_owned(),
                size,
                align,
                fields,
            };
            check(&s).map_err(err)?;
            if is_group {
                let g = parse_fourcc(name).ok_or_else(|| err(format!("bad group {name}")))?;
                self.groups.insert(g, s);
            } else {
                self.structs.insert(name.to_owned(), s);
            }
        }
        Ok(())
    }

    /// The fields at `indent`, starting at `lines[*i]`.
    fn fields(
        &self,
        file: &str,
        lines: &[Line],
        i: &mut usize,
        indent: usize,
    ) -> Result<Vec<FieldDef>, ParseError> {
        let mut out = Vec::new();
        while *i < lines.len() && lines[*i].indent >= indent {
            let l = &lines[*i];
            let err = |why: String| ParseError {
                file: file.into(),
                line: l.number,
                why,
            };
            if l.indent != indent {
                return Err(err(format!("indent {} where {indent} expected", l.indent)));
            }
            *i += 1;
            let t: Vec<&str> = l.text.split_whitespace().collect();
            let offset = t
                .first()
                .and_then(|s| parse_num(s))
                .ok_or_else(|| err("expected an offset".into()))?;
            let ty = *t.get(1).ok_or_else(|| err("expected a type".into()))?;
            let mut params = BTreeMap::new();
            let mut words = Vec::new();
            for w in t.iter().skip(2) {
                match w.split_once('=') {
                    Some((k, v)) => {
                        params.insert(k.to_owned(), v.to_owned());
                    }
                    None => words.push(*w),
                }
            }
            let groups = |p: &BTreeMap<String, String>| -> Result<Vec<FourCC>, ParseError> {
                p.get("groups")
                    .map(|g| {
                        g.split('|')
                            .map(|x| parse_fourcc(x).ok_or_else(|| err(format!("bad group {x}"))))
                            .collect()
                    })
                    .unwrap_or(Ok(Vec::new()))
            };
            let align_of = |words: &[&str], at: usize| -> Result<u32, ParseError> {
                match (words.get(at), words.get(at + 1)) {
                    (Some(&"align"), Some(a)) => {
                        parse_num(a).ok_or_else(|| err("bad align".into()))
                    }
                    (None, _) => Ok(4),
                    _ => Err(err("expected `align <n>`".into())),
                }
            };
            let name_at = |k: usize| -> Result<String, ParseError> {
                words
                    .get(k)
                    .map(|s| (*s).to_owned())
                    .ok_or_else(|| err("missing name".into()))
            };
            let (name, kind) = match ty {
                "block" => {
                    let name = name_at(0)?;
                    let size = words
                        .get(1)
                        .and_then(|s| parse_num(s))
                        .ok_or_else(|| err("block needs an element size".into()))?;
                    let align = align_of(&words, 2)?;
                    let fields = self.fields(file, lines, i, indent + 2)?;
                    let s = StructDef {
                        name: name.clone(),
                        size,
                        align,
                        fields,
                    };
                    check(&s).map_err(err)?;
                    (name, Kind::Block(Box::new(s)))
                }
                "data" => (
                    name_at(0)?,
                    Kind::Data {
                        align: align_of(&words, 1)?,
                    },
                ),
                "struct" => {
                    let name = name_at(0)?;
                    let s = words
                        .get(1)
                        .ok_or_else(|| err("struct needs a struct name".into()))?;
                    let def = self
                        .structs
                        .get(*s)
                        .ok_or_else(|| err(format!("no struct {s} defined before")))?;
                    (name, Kind::Struct(Box::new(def.clone())))
                }
                "unknown" => {
                    let len = words
                        .first()
                        .and_then(|s| parse_num(s))
                        .ok_or_else(|| err("unknown needs a length".into()))?;
                    (
                        format!("unknown_{offset:x}"),
                        Kind::Unknown {
                            len,
                            seen: params.get("seen").cloned().unwrap_or_default(),
                        },
                    )
                }
                "pad" => {
                    let len = words
                        .first()
                        .and_then(|s| parse_num(s))
                        .ok_or_else(|| err("pad needs a length".into()))?;
                    (format!("pad_{offset:x}"), Kind::Pad { len })
                }
                _ => {
                    let (base, count) = match ty.split_once('[') {
                        Some((b, n)) => (
                            b,
                            n.strip_suffix(']')
                                .and_then(parse_num)
                                .ok_or_else(|| err(format!("bad count in {ty}")))?,
                        ),
                        None => (ty, 1),
                    };
                    let list = |k: &str| -> Vec<String> {
                        params
                            .get(k)
                            .map(|v| v.split('|').map(str::to_owned).collect())
                            .unwrap_or_default()
                    };
                    let kind = match base {
                        "u8" => Kind::Int {
                            bytes: 1,
                            signed: false,
                            count,
                        },
                        "i8" => Kind::Int {
                            bytes: 1,
                            signed: true,
                            count,
                        },
                        "u16" => Kind::Int {
                            bytes: 2,
                            signed: false,
                            count,
                        },
                        "i16" => Kind::Int {
                            bytes: 2,
                            signed: true,
                            count,
                        },
                        "u32" => Kind::Int {
                            bytes: 4,
                            signed: false,
                            count,
                        },
                        "i32" => Kind::Int {
                            bytes: 4,
                            signed: true,
                            count,
                        },
                        "f32" => Kind::Float { count },
                        "enum8" => Kind::Enum {
                            bytes: 1,
                            values: list("values"),
                        },
                        "enum16" => Kind::Enum {
                            bytes: 2,
                            values: list("values"),
                        },
                        "enum32" => Kind::Enum {
                            bytes: 4,
                            values: list("values"),
                        },
                        "flags8" => Kind::Flags {
                            bytes: 1,
                            bits: list("bits"),
                        },
                        "flags16" => Kind::Flags {
                            bytes: 2,
                            bits: list("bits"),
                        },
                        "flags32" => Kind::Flags {
                            bytes: 4,
                            bits: list("bits"),
                        },
                        "string" => Kind::StringId,
                        "tagref" => Kind::TagRef {
                            groups: groups(&params)?,
                        },
                        "datum" => Kind::Datum {
                            groups: groups(&params)?,
                        },
                        "raw" => Kind::Raw,
                        "rawsize" => Kind::RawSize {
                            of: params
                                .get("of")
                                .and_then(|s| parse_num(s))
                                .ok_or_else(|| err("rawsize needs of=<offset>".into()))?,
                        },
                        "word" => Kind::Word,
                        "bsp" => Kind::Bsp,
                        _ => return Err(err(format!("unknown type {ty}"))),
                    };
                    if count != 1 && !matches!(kind, Kind::Int { .. } | Kind::Float { .. }) {
                        return Err(err(format!("{base} takes no count")));
                    }
                    (name_at(0)?, kind)
                }
            };
            out.push(FieldDef {
                offset,
                name,
                kind,
                meaning: l.meaning.clone(),
                default: params.get("default").cloned(),
            });
        }
        Ok(out)
    }

    /// The root definition of `group`.
    pub fn group(&self, group: FourCC) -> Option<&StructDef> {
        self.groups.get(&group)
    }
}

/// Fields ascending, tiling `0..size` with no gap or overlap, names unique.
fn check(s: &StructDef) -> Result<(), String> {
    let mut at = 0;
    let mut names = std::collections::HashSet::new();
    for f in &s.fields {
        if f.offset != at {
            return Err(format!(
                "{}: field {} at {:#x}, but the previous field ends at {at:#x} (every byte needs a field or an unknown span)",
                s.name, f.name, f.offset
            ));
        }
        if !names.insert(f.name.as_str()) {
            return Err(format!("{}: field name {} twice", s.name, f.name));
        }
        at += f.kind.width();
    }
    if at != s.size {
        return Err(format!(
            "{}: fields end at {at:#x}, the element is {:#x}",
            s.name, s.size
        ));
    }
    Ok(())
}

struct Line {
    number: usize,
    indent: usize,
    text: String,
    meaning: String,
}

fn lex(file: &str, text: &str) -> Result<Vec<Line>, ParseError> {
    let mut out = Vec::new();
    for (n, raw) in text.lines().enumerate() {
        let trimmed = raw.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = raw.len() - trimmed.len();
        if raw[..indent].contains('\t') {
            return Err(ParseError {
                file: file.into(),
                line: n + 1,
                why: "indent with spaces".into(),
            });
        }
        let (body, meaning) = match trimmed.split_once(" -- ") {
            Some((b, m)) => (b.trim_end(), m.trim()),
            None => (trimmed.trim_end(), ""),
        };
        out.push(Line {
            number: n + 1,
            indent,
            text: body.to_owned(),
            meaning: meaning.to_owned(),
        });
    }
    Ok(out)
}

fn parse_num(s: &str) -> Option<u32> {
    match s.strip_prefix("0x") {
        Some(h) => u32::from_str_radix(h, 16).ok(),
        None => s.parse().ok(),
    }
}

/// A group as written in the files: four characters, or 8 hex digits when
/// it has a space or an unprintable byte (as in `schema/layouts.txt`).
pub fn parse_fourcc(s: &str) -> Option<FourCC> {
    if s.len() == 8 && s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return u32::from_str_radix(s, 16).ok().map(FourCC::from_u32);
    }
    let b: [u8; 4] = s.as_bytes().try_into().ok()?;
    Some(FourCC(b))
}

/// A group as the files write it.
pub fn fourcc_text(g: FourCC) -> String {
    if g.0.iter().all(|b| b.is_ascii_graphic()) {
        String::from_utf8_lossy(&g.0).into_owned()
    } else {
        format!("{:08x}", g.as_u32())
    }
}

impl StructDef {
    /// The definition as file text, fields at `indent` spaces (the header
    /// line is the caller's).
    pub fn fields_text(&self, indent: usize, out: &mut String) {
        use std::fmt::Write as _;
        let pad = " ".repeat(indent);
        for f in &self.fields {
            let _ = write!(out, "{pad}{:#x} ", f.offset);
            let list = |v: &[String]| v.join("|");
            let groups = |g: &[FourCC]| {
                if g.is_empty() {
                    String::new()
                } else {
                    format!(
                        " groups={}",
                        g.iter()
                            .map(|g| fourcc_text(*g))
                            .collect::<Vec<_>>()
                            .join("|")
                    )
                }
            };
            let count = |c: u32| {
                if c == 1 {
                    String::new()
                } else {
                    format!("[{c}]")
                }
            };
            match &f.kind {
                Kind::Int {
                    bytes,
                    signed,
                    count: c,
                } => {
                    let _ = write!(
                        out,
                        "{}{}{} {}",
                        if *signed { "i" } else { "u" },
                        bytes * 8,
                        count(*c),
                        f.name
                    );
                }
                Kind::Float { count: c } => {
                    let _ = write!(out, "f32{} {}", count(*c), f.name);
                }
                Kind::Enum { bytes, values } => {
                    let _ = write!(out, "enum{} {}", bytes * 8, f.name);
                    if !values.is_empty() {
                        let _ = write!(out, " values={}", list(values));
                    }
                }
                Kind::Flags { bytes, bits } => {
                    let _ = write!(out, "flags{} {}", bytes * 8, f.name);
                    if !bits.is_empty() {
                        let _ = write!(out, " bits={}", list(bits));
                    }
                }
                Kind::StringId => {
                    let _ = write!(out, "string {}", f.name);
                }
                Kind::TagRef { groups: g } => {
                    let _ = write!(out, "tagref {}{}", f.name, groups(g));
                }
                Kind::Datum { groups: g } => {
                    let _ = write!(out, "datum {}{}", f.name, groups(g));
                }
                Kind::Raw => {
                    let _ = write!(out, "raw {}", f.name);
                }
                Kind::RawSize { of } => {
                    let _ = write!(out, "rawsize {} of={of:#x}", f.name);
                }
                Kind::Word => {
                    let _ = write!(out, "word {}", f.name);
                }
                Kind::Bsp => {
                    let _ = write!(out, "bsp {}", f.name);
                }
                Kind::Block(s) => {
                    let _ = write!(out, "block {} {:#x} align {}", f.name, s.size, s.align);
                }
                Kind::Data { align } => {
                    let _ = write!(out, "data {} align {align}", f.name);
                }
                Kind::Struct(s) => {
                    let _ = write!(out, "struct {} {}", f.name, s.name);
                }
                Kind::Unknown { len, seen } => {
                    let _ = write!(out, "unknown {len:#x}");
                    if !seen.is_empty() {
                        let _ = write!(out, " seen={seen}");
                    }
                }
                Kind::Pad { len } => {
                    let _ = write!(out, "pad {len:#x}");
                }
            }
            if let Some(d) = &f.default {
                let _ = write!(out, " default={d}");
            }
            if !f.meaning.is_empty() {
                let _ = write!(out, " -- {}", f.meaning);
            }
            out.push('\n');
            if let Kind::Block(s) = &f.kind {
                s.fields_text(indent + 2, out);
            }
        }
    }

    /// The field named `name`.
    pub fn field(&self, name: &str) -> Option<&FieldDef> {
        self.fields.iter().find(|f| f.name == name)
    }

    /// The element definition of the block or embedded struct at `path`
    /// (names joined by `.`: `regions.permutations`).
    pub fn child(&self, path: &str) -> Option<&StructDef> {
        let (first, rest) = match path.split_once('.') {
            Some((a, b)) => (a, Some(b)),
            None => (path, None),
        };
        let s = match &self.field(first)?.kind {
            Kind::Block(s) | Kind::Struct(s) => s,
            _ => return None,
        };
        match rest {
            Some(r) => s.child(r),
            None => Some(s),
        }
    }

    /// Bytes of one element by what its fields are (embedded structs
    /// counted through; a block or data field counts its own 8 bytes, not
    /// its elements).
    pub fn coverage(&self) -> Coverage {
        let mut c = Coverage::default();
        for f in &self.fields {
            match &f.kind {
                Kind::Struct(s) => c.add(&s.coverage()),
                Kind::Unknown { len, .. } => c.unknown += u64::from(*len),
                Kind::Pad { len } => c.pad += u64::from(*len),
                k if f.typed_only() => c.typed += u64::from(k.width()),
                k => c.named += u64::from(k.width()),
            }
        }
        c
    }

    /// Where the [`crate::schema`] puts its layout-dependent fields, as
    /// this definition describes them: offset in the element, then the
    /// schema field. Embedded structs are flattened.
    pub fn schema_fields(&self) -> Result<crate::schema::Struct, String> {
        use crate::schema::{Field, Struct};
        let mut out = Struct {
            size: self.size,
            align: self.align,
            fields: BTreeMap::new(),
        };
        fn walk(s: &StructDef, base: u32, out: &mut BTreeMap<u32, Field>) -> Result<(), String> {
            for f in &s.fields {
                let at = base + f.offset;
                let v = match &f.kind {
                    Kind::Struct(sub) => {
                        walk(sub, at, out)?;
                        continue;
                    }
                    Kind::Block(sub) => Field::Block(Box::new(sub.schema_fields()?)),
                    Kind::Data { align } => Field::Block(Box::new(Struct {
                        size: 1,
                        align: *align,
                        fields: BTreeMap::new(),
                    })),
                    Kind::TagRef { .. } => Field::TagRef,
                    Kind::Datum { .. } => Field::TagId,
                    Kind::StringId => Field::StringId,
                    Kind::Raw => Field::Raw,
                    Kind::RawSize { of } => Field::RawSize(*of),
                    Kind::Word => Field::Word,
                    Kind::Bsp => Field::Bsp,
                    _ => continue,
                };
                out.insert(at, v);
            }
            Ok(())
        }
        walk(self, 0, &mut out.fields)?;
        Ok(out)
    }
}

/// Bytes by what the definition says they are.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Coverage {
    /// Named fields (blocks and data references count their 8 bytes).
    pub named: u64,
    /// Fields whose type is known (a reference, string ID, block, ...) but
    /// not their role: names ending in `_<offset>`.
    pub typed: u64,
    /// Padding: zero everywhere, read by nothing found.
    pub pad: u64,
    /// `unknown` spans.
    pub unknown: u64,
    /// Contents of data references (vertex buffers, compressed blobs):
    /// named as a whole, not field by field.
    pub data: u64,
}

impl Coverage {
    /// Adds `o`.
    pub fn add(&mut self, o: &Coverage) {
        self.named += o.named;
        self.typed += o.typed;
        self.pad += o.pad;
        self.unknown += o.unknown;
        self.data += o.data;
    }

    /// Every byte counted.
    pub fn total(&self) -> u64 {
        self.named + self.typed + self.pad + self.unknown + self.data
    }

    /// Named, padding and data bytes as a share of all, in percent.
    pub fn known_percent(&self) -> f64 {
        if self.total() == 0 {
            return 100.0;
        }
        100.0 * (self.total() - self.unknown) as f64 / self.total() as f64
    }
}

/// How a definition and the [`crate::schema`] layout of its group compare.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Agreement {
    /// Schema fields the definition lacks or types otherwise (errors).
    pub missing: Vec<String>,
    /// Layout-dependent fields only the definition has: null in every
    /// retail instance of this group, so inference could not see them.
    pub extra: Vec<String>,
    /// Blocks whose alignment differs (the schema keeps the weakest
    /// alignment seen per group, which is sometimes stronger than the
    /// element needs).
    pub align: Vec<String>,
}

/// Compares `def` with the schema layout `s`.
pub fn agree(def: &StructDef, s: &crate::schema::Struct) -> Result<Agreement, String> {
    let mut a = Agreement::default();
    let d = def.schema_fields()?;
    agree_into(&d, s, &def.name, &mut a);
    Ok(a)
}

fn agree_into(d: &crate::schema::Struct, s: &crate::schema::Struct, path: &str, a: &mut Agreement) {
    use crate::schema::Field;
    if d.size != s.size {
        a.missing.push(format!(
            "{path}: element {:#x}, schema {:#x}",
            d.size, s.size
        ));
        return;
    }
    if d.align != s.align {
        a.align
            .push(format!("{path}: align {}, schema {}", d.align, s.align));
    }
    for (o, sf) in &s.fields {
        let p = format!("{path}+{o:#x}");
        match (d.fields.get(o), sf) {
            (Some(Field::Block(dc)), Field::Block(sc)) => agree_into(dc, sc, &p, a),
            (Some(df), sf) if df == sf => {}
            (df, sf) => a
                .missing
                .push(format!("{p}: schema {sf:?}, definition {df:?}")),
        }
    }
    for (o, df) in &d.fields {
        if !s.fields.contains_key(o) {
            let kind = match df {
                Field::Block(_) => "block",
                Field::TagRef => "tagref",
                Field::TagId => "datum",
                Field::StringId => "string",
                Field::Raw => "raw",
                Field::RawSize(_) => "rawsize",
                Field::Bsp => "bsp",
                Field::Word => "word",
            };
            a.extra.push(format!("{path}+{o:#x} {kind}"));
        }
    }
}
