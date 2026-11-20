//! The tag bundle: tags taken out of a map into a self-describing JSON
//! file (`.h2tag`), and put back into one. docs/editor/editor.md, "Tag
//! bundles".
//!
//! A tag is stored as its root element: the element's bytes as the map
//! had them, and a fixup for every field whose bytes only mean something
//! inside that map. A tag reference or datum carries the `group:name` it
//! named, a string ID its text, a block its elements (each an element the
//! same way), a data reference its bytes, a raw-resource pointer the file
//! it selects (`local`, `mainmenu`, `shared`, `single_player_shared`) with
//! its offset and size and, when the export copied them, the resource's
//! bytes as a blob. Importing writes the bytes back and resolves each
//! fixup in the target map (`crates/apps/studio/src/app/tag_bundle.rs`).
//!
//! Each tag also records its group's layout fingerprint ([`fingerprint`]):
//! the structural shape of the registry's layout (sizes, and the offsets
//! of references, string IDs, blocks, data and resources), not its names.
//! An import refuses a tag whose fingerprint differs from the importing
//! editor's: its fixups would land on the wrong bytes.
//!
//! The format is ours and holds whatever the user exported from their own
//! maps; bundles are user-local output and never belong in the repository.
use serde::{Deserialize, Serialize};
use tags::registry::{Kind, Layout};

use map::ResourceFile;

/// The `format` every bundle names.
pub const FORMAT: &str = "h2recomp.tag-bundle";
/// The format's version; a bundle of another is refused.
pub const VERSION: u32 = 1;
/// A bundle's file extension.
pub const EXTENSION: &str = "h2tag";

/// A tag bundle.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bundle {
    /// [`FORMAT`].
    pub format: String,
    /// [`VERSION`].
    pub version: u32,
    /// The map the tags were read from.
    pub origin: Origin,
    pub tags: Vec<BundleTag>,
    /// Raw resources copied with the tags; [`Fixup::Raw::blob`] indexes
    /// them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blobs: Vec<Blob>,
}

/// Where a bundle's tags were read.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Origin {
    /// The map's name as its source lists it (`ascension`, `shared`).
    pub map: String,
    /// The map header's build string.
    #[serde(default)]
    pub build: String,
}

/// One tag.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BundleTag {
    /// The group as the tag files write it (`weap`, `snd!`).
    pub group: String,
    /// The tag's path (`objects\weapons\...`).
    pub name: String,
    /// [`fingerprint`] of the group's root layout it was read with.
    pub layout: String,
    pub root: Element,
    /// The fields as the Tags tool reads them, for people and diffs.
    /// Import does not read it.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub fields: serde_json::Value,
}

impl BundleTag {
    /// `group:name`, as the Tags tool names tags.
    pub fn label(&self) -> String {
        format!("{}:{}", self.group, self.name)
    }
}

/// One element of a layout: its bytes and what in them is the map's own.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Element {
    #[serde(with = "b64")]
    pub bytes: Vec<u8>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fixups: Vec<Fixup>,
}

/// A field of an element whose bytes are resolved in the target map.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Fixup {
    /// A tag reference (8 bytes, group then datum) or a bare datum (4
    /// bytes) at `at`: the `group:name` it names, or `None` for none (or
    /// an ID the map did not have; the element's bytes keep the value).
    Tag {
        at: u32,
        #[serde(default)]
        target: Option<String>,
    },
    /// A string ID: its text, or `None` when the map had none for it (the
    /// element's bytes keep the value).
    StringId {
        at: u32,
        #[serde(default)]
        text: Option<String>,
    },
    /// A block (count, address): its elements, in order.
    Block { at: u32, elements: Vec<Element> },
    /// A data reference (length, address): its bytes.
    Data {
        at: u32,
        #[serde(with = "b64")]
        bytes: Vec<u8>,
    },
    /// A raw-resource pointer: the file it selects ([`file_name`]), its
    /// offset in that file, the size the element gives it, and the blob
    /// holding its bytes when the export copied them.
    Raw {
        at: u32,
        file: String,
        offset: u32,
        #[serde(default)]
        size: Option<u32>,
        #[serde(default)]
        blob: Option<usize>,
    },
}

impl Fixup {
    /// The field's offset in its element.
    pub fn at(&self) -> u32 {
        match self {
            Fixup::Tag { at, .. }
            | Fixup::StringId { at, .. }
            | Fixup::Block { at, .. }
            | Fixup::Data { at, .. }
            | Fixup::Raw { at, .. } => *at,
        }
    }
}

/// A raw resource's bytes, as read from the origin's file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Blob {
    pub file: String,
    pub offset: u32,
    #[serde(with = "b64")]
    pub bytes: Vec<u8>,
}

impl Bundle {
    /// An empty bundle of [`FORMAT`] and [`VERSION`].
    pub fn new(origin: Origin) -> Self {
        Self {
            format: FORMAT.to_owned(),
            version: VERSION,
            origin,
            tags: Vec::new(),
            blobs: Vec::new(),
        }
    }

    /// The bundle as pretty JSON.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("a bundle is plain data")
    }

    /// A bundle from JSON, refusing another format or version.
    pub fn from_json(text: &str) -> Result<Self, String> {
        let b: Bundle =
            serde_json::from_str(text).map_err(|e| format!("not a tag bundle: {e}"))?;
        if b.format != FORMAT {
            return Err(format!("not a tag bundle: format {:?}", b.format));
        }
        if b.version != VERSION {
            return Err(format!(
                "a tag bundle of version {}, and this editor reads version {VERSION}",
                b.version
            ));
        }
        Ok(b)
    }

    /// Every blob index a fixup names is in [`Self::blobs`].
    pub fn check_blobs(&self) -> Result<(), String> {
        fn walk(e: &Element, n: usize) -> Result<(), String> {
            for f in &e.fixups {
                match f {
                    Fixup::Raw { blob: Some(i), .. } if *i >= n => {
                        return Err(format!("a resource names blob {i}, and there are {n}"));
                    }
                    Fixup::Block { elements, .. } => {
                        for e in elements {
                            walk(e, n)?;
                        }
                    }
                    _ => {}
                }
            }
            Ok(())
        }
        for t in &self.tags {
            walk(&t.root, self.blobs.len()).map_err(|e| format!("{}: {e}", t.label()))?;
        }
        Ok(())
    }
}

/// A resource file's name in a bundle.
pub fn file_name(file: ResourceFile) -> &'static str {
    match file {
        ResourceFile::Local => "local",
        ResourceFile::MainMenu => "mainmenu",
        ResourceFile::Shared => "shared",
        ResourceFile::SinglePlayerShared => "single_player_shared",
    }
}

/// [`file_name`] back.
pub fn parse_file(name: &str) -> Option<ResourceFile> {
    Some(match name {
        "local" => ResourceFile::Local,
        "mainmenu" => ResourceFile::MainMenu,
        "shared" => ResourceFile::Shared,
        "single_player_shared" => ResourceFile::SinglePlayerShared,
        _ => return None,
    })
}

/// A file name for a bundle of tag `group:name`: the group, then the path
/// with its separators as dots and anything unusual as `_`.
pub fn default_file_name(group: &str, name: &str) -> String {
    let clean = |s: &str| -> String {
        s.chars()
            .map(|c| match c {
                '\\' | '/' => '.',
                c if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') => c,
                _ => '_',
            })
            .collect()
    };
    let name = clean(name);
    let name = name.trim_matches('.');
    format!(
        "{}-{}.{EXTENSION}",
        clean(group),
        if name.is_empty() { "tag" } else { name }
    )
}

/// The structural fingerprint of a layout: its size and, for every field
/// whose bytes an import resolves (references, datums, string IDs,
/// blocks with their elements' fingerprints, data, raw pointers and
/// sizes, BSP descriptors), its offset and kind. Names and plain value
/// types are left out, so naming a field does not change it. 16 hex
/// digits (FNV-1a, 64 bits).
pub fn fingerprint(layout: &Layout) -> String {
    let mut h = Fnv::new();
    shape(layout, &mut h, 0);
    format!("{:016x}", h.0)
}

/// Blocks nested deeper than this are not followed.
const SHAPE_DEPTH: u32 = 32;

fn shape(l: &Layout, h: &mut Fnv, depth: u32) {
    h.u32(l.size);
    if depth > SHAPE_DEPTH {
        return;
    }
    for f in &l.fields {
        let class = match &f.kind {
            Kind::TagRef { .. } => 1,
            Kind::Datum { .. } => 2,
            Kind::StringId => 3,
            Kind::Block(_) => 4,
            Kind::Data => 5,
            Kind::Raw => 6,
            Kind::RawSize { .. } => 7,
            Kind::Bsp => 8,
            _ => continue,
        };
        h.u32(f.offset);
        h.u32(class);
        match &f.kind {
            Kind::Block(child) => shape(child, h, depth + 1),
            Kind::RawSize { of } => h.u32(*of),
            _ => {}
        }
    }
}

struct Fnv(u64);
impl Fnv {
    fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
    fn u32(&mut self, v: u32) {
        for b in v.to_le_bytes() {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}

/// Standard base64 (RFC 4648, with padding), for the bundle's bytes.
pub mod b64 {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub fn encode(b: &[u8]) -> String {
        let mut out = String::with_capacity(b.len().div_ceil(3) * 4);
        for c in b.chunks(3) {
            let n = u32::from(c[0]) << 16
                | u32::from(c.get(1).copied().unwrap_or(0)) << 8
                | u32::from(c.get(2).copied().unwrap_or(0));
            for k in 0..4usize {
                if k <= c.len() {
                    out.push(char::from(ALPHABET[(n >> (18 - 6 * k)) as usize & 63]));
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    fn value(c: u8) -> Option<u32> {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        Some(u32::from(v))
    }

    /// Decodes [`encode`]'s output; whitespace is skipped.
    pub fn decode(s: &str) -> Result<Vec<u8>, String> {
        let s: Vec<u8> = s.bytes().filter(|c| !c.is_ascii_whitespace()).collect();
        if !s.len().is_multiple_of(4) {
            return Err("base64 of a length that is not a multiple of 4".into());
        }
        let mut out = Vec::with_capacity(s.len() / 4 * 3);
        let quads = s.len() / 4;
        for (i, q) in s.chunks(4).enumerate() {
            let pad = q.iter().rev().take_while(|c| **c == b'=').count();
            if pad > 2 || (pad > 0 && i + 1 != quads) {
                return Err("base64 padding in the wrong place".into());
            }
            let mut n = 0u32;
            for (k, &c) in q.iter().enumerate() {
                let v = if k >= 4 - pad {
                    0
                } else {
                    value(c).ok_or_else(|| format!("{:?} is not base64", char::from(c)))?
                };
                n = n << 6 | v;
            }
            out.extend_from_slice(&n.to_be_bytes()[1..4 - pad]);
        }
        Ok(out)
    }

    pub fn serialize<S: serde::Serializer, T: AsRef<[u8]>>(b: &T, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&encode(b.as_ref()))
    }

    pub fn deserialize<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = <String as serde::Deserialize>::deserialize(d)?;
        decode(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tags::registry::{Field, Hint};

    #[test]
    fn base64_round_trips_every_length_and_matches_the_rfc() {
        // RFC 4648 section 10.
        for (plain, coded) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(b64::encode(plain.as_bytes()), coded);
            assert_eq!(b64::decode(coded).unwrap(), plain.as_bytes());
        }
        let all: Vec<u8> = (0..=255u8).chain((0..=255u8).rev()).collect();
        for n in 0..all.len() {
            assert_eq!(b64::decode(&b64::encode(&all[..n])).unwrap(), &all[..n]);
        }
        assert!(b64::decode("Zg=").is_err());
        assert!(b64::decode("Zg==Zg==").is_err());
        assert!(b64::decode("Z!==").is_err());
        assert_eq!(b64::decode("Zm9v\nYmFy").unwrap(), b"foobar");
    }

    fn field(offset: u32, kind: Kind) -> Field {
        Field {
            offset,
            name: format!("f{offset:x}"),
            section: String::new(),
            named: false,
            kind,
            meaning: String::new(),
        }
    }

    fn layout(fields: Vec<Field>, size: u32) -> Layout {
        Layout {
            name: "test".into(),
            size,
            align: 4,
            fields,
        }
    }

    #[test]
    fn the_fingerprint_follows_structure_not_names() {
        let child = Arc::new(layout(vec![field(0, Kind::TagRef { groups: vec![] })], 8));
        let a = layout(
            vec![
                field(0, Kind::Float { count: 1, hint: Hint::Plain }),
                field(4, Kind::Block(child.clone())),
            ],
            12,
        );
        let mut renamed = a.clone();
        renamed.fields[0].name = "speed".into();
        renamed.fields[0].kind = Kind::Int {
            bytes: 4,
            signed: false,
            count: 1,
        };
        assert_eq!(fingerprint(&a), fingerprint(&renamed));
        assert_eq!(fingerprint(&a).len(), 16);
        // A reference moved in the child changes it.
        let moved = Arc::new(layout(
            vec![
                field(0, Kind::Unknown { len: 4 }),
                field(4, Kind::Datum { groups: vec![] }),
            ],
            8,
        ));
        let mut b = a.clone();
        b.fields[1].kind = Kind::Block(moved);
        assert_ne!(fingerprint(&a), fingerprint(&b));
        // So does the size.
        let mut c = a.clone();
        c.size = 16;
        assert_ne!(fingerprint(&a), fingerprint(&c));
    }

    fn sample() -> Bundle {
        let mut b = Bundle::new(Origin {
            map: "example".into(),
            build: "01.02.03".into(),
        });
        b.tags.push(BundleTag {
            group: "weap".into(),
            name: "objects\\test\\blaster".into(),
            layout: "0123456789abcdef".into(),
            root: Element {
                bytes: vec![0, 1, 2, 3, 0xff, 0xfe],
                fixups: vec![
                    Fixup::Tag {
                        at: 0,
                        target: Some("itmc:objects\\test\\kit".into()),
                    },
                    Fixup::Tag { at: 8, target: None },
                    Fixup::StringId {
                        at: 0x10,
                        text: Some("blaster".into()),
                    },
                    Fixup::Block {
                        at: 0x14,
                        elements: vec![Element {
                            bytes: vec![9; 12],
                            fixups: vec![Fixup::Data {
                                at: 4,
                                bytes: vec![1, 2, 3],
                            }],
                        }],
                    },
                    Fixup::Raw {
                        at: 0x1c,
                        file: "shared".into(),
                        offset: 0x1234,
                        size: Some(3),
                        blob: Some(0),
                    },
                ],
            },
            fields: serde_json::json!({"speed": 2.5}),
        });
        b.blobs.push(Blob {
            file: "shared".into(),
            offset: 0x1234,
            bytes: vec![7, 8, 9],
        });
        b
    }

    #[test]
    fn a_bundle_round_trips_through_its_json() {
        let b = sample();
        let text = b.to_json();
        assert!(text.contains("\"format\": \"h2recomp.tag-bundle\""), "{text}");
        assert!(text.contains("\"kind\": \"string_id\""), "{text}");
        assert!(text.contains("\"bytes\": \"AAECA//+\""), "{text}");
        let back = Bundle::from_json(&text).unwrap();
        assert_eq!(back, b);
        back.check_blobs().unwrap();
        assert_eq!(b.tags[0].label(), "weap:objects\\test\\blaster");
    }

    #[test]
    fn another_format_version_or_a_missing_blob_is_refused() {
        let mut b = sample();
        b.version = VERSION + 1;
        assert!(Bundle::from_json(&b.to_json()).unwrap_err().contains("version"));
        let mut b = sample();
        b.format = "something.else".into();
        assert!(Bundle::from_json(&b.to_json()).is_err());
        assert!(Bundle::from_json("{}").is_err());
        let mut b = sample();
        b.blobs.clear();
        assert!(b.check_blobs().unwrap_err().contains("blob 0"));
    }

    #[test]
    fn files_and_names() {
        for f in [
            ResourceFile::Local,
            ResourceFile::MainMenu,
            ResourceFile::Shared,
            ResourceFile::SinglePlayerShared,
        ] {
            assert_eq!(parse_file(file_name(f)), Some(f));
        }
        assert_eq!(parse_file("other"), None);
        assert_eq!(
            default_file_name("weap", "objects\\weapons\\rifle\\battle rifle"),
            "weap-objects.weapons.rifle.battle_rifle.h2tag"
        );
        assert_eq!(default_file_name("snd!", ""), "snd_-tag.h2tag");
    }
}
