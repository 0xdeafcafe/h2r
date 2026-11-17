//! Reading Halo 1 `ustr` and `hmt ` tags, and the scenario's text
//! references.
//!
//! Layouts and evidence: `docs/formats/h1-text.md`. Every field this module
//! reads was checked on the owner's retail disc (every map: ten campaign
//! levels, thirteen multiplayer maps and `ui.map`) by the ignored
//! `real_disc_*` tests; nothing here comes from community definition
//! files.

use crate::{Error, Result};
use h1map::{Block, Group, H1Map, Tag, TagRef};

/// A 20-byte Halo 1 data reference: (size, flags, offset in the tag,
/// address, unused). The offset is editor bookkeeping (the unpadded
/// cumulative size of the earlier strings); the engine and this reader
/// use the address.
pub const DATA_REF: usize = 0x14;

/// `ustr`: one UTF-16 string per element.
pub struct Ustr {
    /// The strings, in list order, without their NULs.
    pub strings: Vec<String>,
}

/// One element of an `hmt ` message: a run of blob text, or a glyph the
/// game substitutes at display time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MessagePart {
    /// Text taken from the string blob.
    Text(String),
    /// A substituted glyph (button or keyword) by its index.
    Glyph(u8),
}

/// One named message of an `hmt ` tag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HudMessage {
    /// The message's name (NUL-padded `char[32]`).
    pub name: String,
    /// The message's parts, in display order.
    pub parts: Vec<MessagePart>,
}

impl HudMessage {
    /// The message with glyphs dropped: what a converter that cannot render
    /// Halo 1's substitutions would show.
    pub fn plain_text(&self) -> String {
        self.parts
            .iter()
            .filter_map(|p| match p {
                MessagePart::Text(t) => Some(t.as_str()),
                MessagePart::Glyph(_) => None,
            })
            .collect()
    }
}

/// `hmt `: named messages assembled from a UTF-16 blob, with glyph
/// substitutions.
pub struct Hmt {
    /// The messages, in tag order.
    pub messages: Vec<HudMessage>,
}

/// The scenario's text references, read by [`scenario_text`].
#[derive(Clone, Debug, Default)]
pub struct ScenarioText {
    /// `scnr+0x574`: a `ustr` of names the scripts give placed objects
    /// (absent on some levels; null there).
    pub object_names: Option<TagRef>,
    /// `scnr+0x584`: a `ustr` of in-game help text.
    pub help_text: Option<TagRef>,
    /// `scnr+0x594`: the level's `hmt `.
    pub hud_messages: Option<TagRef>,
}

fn u16s(bytes: &[u8]) -> Vec<u16> {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect()
}

fn utf16(units: &[u16]) -> Result<String> {
    String::from_utf16(units).map_err(|_| Error::Invalid("string is not UTF-16".into()))
}

impl Ustr {
    /// Reads the `ustr` tag `tag` of `map`: a block of 20-byte data
    /// references, each to the string's NUL-terminated UTF-16 bytes.
    pub fn read(map: &H1Map, tag: &Tag) -> Result<Self> {
        if tag.group != Group::new(b"ustr") {
            return Err(Error::WrongGroup(tag.group.to_string(), "ustr"));
        }
        let bytes = map.tag_data(tag)?;
        let block = Block::read(bytes, 0)
            .ok_or_else(|| Error::Invalid("ustr tag shorter than its block".into()))?;
        let elements = map.block(block, DATA_REF)?;
        let mut strings = Vec::with_capacity(block.count as usize);
        for e in elements.as_chunks::<DATA_REF>().0 {
            let size = u32::from_le_bytes(e[0..4].try_into().unwrap()) as usize;
            let address = u32::from_le_bytes(e[12..16].try_into().unwrap());
            if size < 2 || !size.is_multiple_of(2) {
                return Err(Error::Invalid(format!(
                    "ustr string size {size:#x} is not an even byte count with its NUL"
                )));
            }
            let data = map
                .read(address, size)
                .map_err(|e| Error::Invalid(format!("ustr string: {e}")))?;
            let units = u16s(data);
            if *units.last().expect("size >= 2") != 0 {
                return Err(Error::Invalid(
                    "ustr string size does not end at its NUL".into(),
                ));
            }
            strings.push(utf16(&units[..units.len() - 1])?);
        }
        Ok(Self { strings })
    }
}

impl Hmt {
    /// Reads the `hmt ` tag `tag` of `map`.
    ///
    /// The tag holds one UTF-16 blob and, per message, a run of 2-byte
    /// elements. A text element carries a length in UTF-16 units, NUL
    /// included; the lengths of all text elements in tag order sum to
    /// exactly the blob, so the messages consume it in order.
    pub fn read(map: &H1Map, tag: &Tag) -> Result<Self> {
        let bytes = map.tag_data(tag)?;
        if tag.group != Group::new(b"hmt ") {
            return Err(Error::WrongGroup(tag.group.to_string(), "hmt "));
        }
        if bytes.len() < 0x2c {
            return Err(Error::Invalid("hmt tag shorter than its layout".into()));
        }
        let blob_size = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
        let blob_address = u32::from_le_bytes(bytes[0xc..0x10].try_into().unwrap());
        if !blob_size.is_multiple_of(2) {
            return Err(Error::Invalid(
                "hmt string blob has an odd byte size".into(),
            ));
        }
        let blob = u16s(
            map.read(blob_address, blob_size)
                .map_err(|e| Error::Invalid(format!("hmt string blob: {e}")))?,
        );
        let elements = Block::read(bytes, 0x14).expect("length checked");
        let element_bytes = map.block(elements, 2)?;
        let pairs: Vec<(u8, u8)> = element_bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| (c[0], c[1]))
            .collect();
        let messages = Block::read(bytes, 0x20).expect("length checked");
        let message_bytes = map.block(messages, 0x40)?;

        let mut out = Vec::with_capacity(messages.count as usize);
        let mut blob_at = 0usize;
        let mut element_at = 0usize;
        for e in message_bytes.as_chunks::<0x40>().0 {
            let name_end = e[..0x20].iter().position(|&b| b == 0).unwrap_or(0x20);
            let name = String::from_utf8(e[..name_end].to_vec())
                .map_err(|_| Error::Invalid("hmt message name is not UTF-8/ASCII".into()))?;
            // +0x20 packs an unresolved u16 below its element start u16.
            let start = u16::from_le_bytes(e[0x22..0x24].try_into().unwrap()) as usize;
            let count = u32::from_le_bytes(e[0x24..0x28].try_into().unwrap()) as usize;
            if start != element_at {
                return Err(Error::Invalid(format!(
                    "hmt message {name:?} starts at element {start}, want {element_at}: the messages do not tile the element block"
                )));
            }
            let end = start
                .checked_add(count)
                .ok_or_else(|| Error::Invalid("hmt message element range".into()))?;
            let mut parts = Vec::with_capacity(count);
            for &(kind, value) in pairs.get(start..end).ok_or_else(|| {
                Error::Invalid("hmt message element range outside the element block".into())
            })? {
                match kind {
                    0 => {
                        let units = value as usize;
                        let end = blob_at
                            .checked_add(units)
                            .filter(|&end| units > 0 && end <= blob.len())
                            .ok_or_else(|| {
                                Error::Invalid("hmt text element runs past the string blob".into())
                            })?;
                        let run = &blob[blob_at..end];
                        if *run.last().expect("units > 0") != 0 {
                            return Err(Error::Invalid(
                                "hmt text element does not end at its NUL".into(),
                            ));
                        }
                        parts.push(MessagePart::Text(utf16(&run[..run.len() - 1])?));
                        blob_at = end;
                    }
                    1 => parts.push(MessagePart::Glyph(value)),
                    other => return Err(Error::Unsupported(format!("hmt element type {other}"))),
                }
            }
            element_at = end;
            out.push(HudMessage { name, parts });
        }
        if element_at != pairs.len() {
            return Err(Error::Invalid(
                "hmt messages do not cover every element".into(),
            ));
        }
        if blob_at != blob.len() {
            return Err(Error::Invalid(
                "hmt text elements do not consume the whole string blob".into(),
            ));
        }
        Ok(Self { messages: out })
    }
}

/// The scenario's text references. Every reference that is not null must
/// name a tag of its field's group.
pub fn scenario_text(map: &H1Map) -> Result<ScenarioText> {
    let scenario = map.scenario();
    let bytes = map.tag_data(scenario)?;
    if bytes.len() < 0x5a4 {
        return Err(Error::Invalid(
            "scenario tag too short for its text fields".into(),
        ));
    }
    let mut refs = ScenarioText::default();
    for (at, slot, group) in [
        (0x574, &mut refs.object_names, b"ustr"),
        (0x584, &mut refs.help_text, b"ustr"),
        (0x594, &mut refs.hud_messages, b"hmt "),
    ] {
        let r = TagRef::read(bytes, at).expect("length checked");
        if r.id.is_none() {
            continue;
        }
        let target = map
            .tag(r.id)
            .map_err(|_| Error::Invalid(format!("scenario text reference at {at:#x}")))?;
        if target.group != Group::new(group) {
            return Err(Error::Invalid(format!(
                "scenario text reference at {at:#x} names a {} tag",
                target.group
            )));
        }
        *slot = Some(r);
    }
    Ok(refs)
}
