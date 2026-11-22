//! Read one XML field tree at a time. An unrecognised field still occupies
//! its declared offset and participates in conflict detection.

use std::collections::BTreeMap;
use std::fmt::Display;
use std::io::BufRead;

use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};

use super::GAME;
use crate::{Field, Kind, Plugin};

/// Widths of the pointer fields, which differ between engine generations.
#[derive(Clone, Copy)]
struct Widths {
    reference: u32,
    block: u32,
    data: u32,
}

impl Widths {
    /// Halo 2 (Xbox, Vista and its MCC edition) uses 8-byte references,
    /// blocks and data references. The later engines' plugins use 16, 12
    /// and 20 bytes.
    fn of(game: &str) -> Self {
        if matches!(game, "Halo2Xbox" | "Halo2" | "Halo2MCC") {
            Self { reference: 8, block: 8, data: 8 }
        } else {
            Self { reference: 16, block: 12, data: 20 }
        }
    }
}

fn message(e: impl Display) -> String {
    e.to_string()
}

#[derive(Default)]
struct Node {
    tag: String,
    attrs: BTreeMap<String, String>,
    text: String,
    children: Vec<Node>,
}

impl Node {
    fn start(e: &BytesStart<'_>) -> Result<Self, String> {
        let mut node = Node {
            tag: std::str::from_utf8(e.name().as_ref())
                .map_err(message)?
                .to_owned(),
            ..Default::default()
        };
        for attr in e.attributes() {
            let attr = attr.map_err(message)?;
            let key = std::str::from_utf8(attr.key.as_ref())
                .map_err(message)?
                .to_ascii_lowercase();
            match key.as_str() {
                "game" | "basesize" | "elementsize" | "name" | "offset" | "visible" | "comment"
                | "description" | "tooltip" | "text" | "value" | "index" | "size" | "length"
                | "withgroup" | "alpha" => {
                    let value = attr
                        .decoded_and_normalized_value(XmlVersion::Implicit1_0, e.decoder())
                        .map_err(message)?;
                    node.attrs.insert(key, value.into_owned());
                }
                _ => {}
            }
        }
        Ok(node)
    }

    fn attr(&self, key: &str) -> Option<&str> {
        self.attrs.get(key).map(String::as_str)
    }

    fn flag(&self, key: &str) -> Option<bool> {
        match self.attr(key)?.trim() {
            value if value == "0" || value.eq_ignore_ascii_case("false") => Some(false),
            value if value == "1" || value.eq_ignore_ascii_case("true") => Some(true),
            _ => None,
        }
    }

    fn required(&self, key: &str) -> Result<&str, String> {
        self.attr(key)
            .ok_or_else(|| format!("{} requires {key}", self.tag))
    }

    fn number(&self, key: &str) -> Result<Option<u32>, String> {
        self.attr(key)
            .map(|s| number(s).and_then(|v| u32::try_from(v).map_err(message)))
            .transpose()
            .map_err(|e| format!("{} {key}: {e}", self.tag))
    }

    fn description(&self) -> String {
        let mut comments: Vec<&str> = ["comment", "description", "tooltip"]
            .into_iter()
            .filter_map(|key| self.attr(key))
            .collect();
        for child in &self.children {
            if ["comment", "description", "tooltip"]
                .iter()
                .any(|tag| child.tag.eq_ignore_ascii_case(tag))
            {
                if let Some(text) = child.attr("text").or_else(|| child.attr("value")) {
                    comments.push(text);
                }
                comments.push(&child.text);
            }
        }
        comments
            .into_iter()
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn fields(self, widths: Widths) -> Result<Vec<Field>, String> {
        self.children
            .into_iter()
            .filter(|n| n.attr("offset").is_some())
            .map(|n| n.field(widths))
            .collect()
    }

    fn field(self, widths: Widths) -> Result<Field, String> {
        let offset = self.number("offset")?.ok_or("field requires offset")?;
        let (kind, size) = self.ty(widths)?;
        let mut field = Field {
            name: self.attr("name").unwrap_or_default().to_owned(),
            offset,
            size,
            visible: self.flag("visible").unwrap_or(true),
            comment: self.description(),
            options: Vec::new(),
            bits: Vec::new(),
            children: Vec::new(),
            element_size: None,
            kind,
        };
        match field.kind {
            Kind::Block => {
                field.element_size = self.number("elementsize")?;
                field.children = self.fields(widths)?;
            }
            Kind::Enum => {
                for option in self
                    .children
                    .iter()
                    .filter(|n| n.tag.eq_ignore_ascii_case("option"))
                {
                    field.options.push((
                        number(option.required("value")?)?,
                        option.required("name")?.to_owned(),
                    ));
                }
            }
            Kind::Bits => {
                for bit in self.children.iter().filter(|n| n.attr("index").is_some()) {
                    let index = bit.number("index")?.ok_or("bit requires index")?;
                    field.bits.push((index, bit.required("name")?.to_owned()));
                }
            }
            _ => {}
        }
        Ok(field)
    }

    fn ty(&self, widths: Widths) -> Result<(Kind, Option<u32>), String> {
        let (kind, size) = match self.tag.to_ascii_lowercase().as_str() {
            "int8" => (Kind::Int { signed: true }, 1),
            "int16" => (Kind::Int { signed: true }, 2),
            "int32" => (Kind::Int { signed: true }, 4),
            "uint8" => (Kind::Int { signed: false }, 1),
            "uint16" => (Kind::Int { signed: false }, 2),
            "uint32" => (Kind::Int { signed: false }, 4),
            "float32" | "float" => (Kind::Float, 4),
            "enum8" => (Kind::Enum, 1),
            "enum16" => (Kind::Enum, 2),
            "enum32" => (Kind::Enum, 4),
            "bitfield8" | "flags8" => (Kind::Bits, 1),
            "bitfield16" | "flags16" => (Kind::Bits, 2),
            "bitfield32" | "flags32" => (Kind::Bits, 4),
            "degree" => (Kind::Degree, 4),
            "degree2" | "ranged" => (Kind::Degree, 8),
            "rangef" => (Kind::Bounds, 8),
            "point16" | "range16" => (Kind::Int16Array, 4),
            "rect16" | "quat16" => (Kind::Int16Array, 8),
            "plane2" => (Kind::Vector, 12),
            "plane3" => (Kind::Vector, 16),
            "color32" => (Kind::PackedColour, 4),
            "colorf" => {
                let size = self.flag("alpha").map(|alpha| if alpha { 16 } else { 12 });
                return Ok((Kind::Colour, size));
            }
            "stringid" => (Kind::StringId, 4),
            "datum" => (Kind::Datum, 4),
            "tagref" if self.flag("withgroup") == Some(false) => (Kind::Datum, 4),
            "tagref" => (Kind::TagRef, widths.reference),
            "dataref" => (Kind::DataRef, widths.data),
            "tagblock" => (Kind::Block, widths.block),
            "point2" | "point2d" | "vector2" | "vector2d" => (Kind::Vector, 8),
            "point3" | "point3d" | "vector3" | "vector3d" => (Kind::Vector, 12),
            "point4" | "point4d" | "vector4" | "vector4d" => (Kind::Vector, 16),
            "rgb" | "colorrgb" | "colourrgb" | "colorrgbf" | "colourrgbf" => (Kind::Colour, 12),
            "argb" | "rgba" | "colorargb" | "colourargb" | "colorrgba" | "colourrgba"
            | "colorargbf" | "colourargbf" => (Kind::Colour, 16),
            "ascii" => {
                let key = if self.attr("length").is_some() {
                    "length"
                } else {
                    "size"
                };
                return Ok((Kind::Ascii, self.number(key)?));
            }
            "undefined" => (Kind::Undefined, self.number("size")?.unwrap_or(4)),
            _ => return Ok((Kind::Unknown(self.tag.clone()), None)),
        };
        Ok((kind, Some(size)))
    }
}

fn number(s: &str) -> Result<i64, String> {
    let s = s.trim();
    let unsigned = s.strip_prefix('-').unwrap_or(s);
    if let Some(hex) = unsigned.strip_prefix("0x").or_else(|| unsigned.strip_prefix("0X")) {
        let n = i64::from_str_radix(hex, 16).map_err(message)?;
        Ok(if s.starts_with('-') { -n } else { n })
    } else {
        s.parse::<i64>().map_err(message)
    }
}

/// Checks the game's exact spelling before reading a field.
pub(super) fn header<R: BufRead>(reader: &mut Reader<R>, expected: &str) -> Result<u32, String> {
    let mut buf = Vec::new();
    reader.config_mut().expand_empty_elements = true;
    loop {
        match reader.read_event_into(&mut buf).map_err(message)? {
            Event::Start(e) if e.name().as_ref() == b"plugin" => {
                let root = Node::start(&e)?;
                let game = root.required("game")?;
                if game != expected {
                    return Err(format!("game {game:?} is unsupported; expected {expected}"));
                }
                return root
                    .number("basesize")?
                    .ok_or_else(|| "plugin requires baseSize".into());
            }
            Event::Text(text) if text.iter().all(u8::is_ascii_whitespace) => {}
            Event::Decl(_) | Event::Comment(_) | Event::PI(_) => {}
            _ => return Err(format!("expected a {expected} plugin root")),
        }
        buf.clear();
    }
}

/// Parses one in-memory retail Xbox group plugin using the same checks as
/// the folder reader.
pub fn parse(group: &str, xml: &str) -> Result<Plugin, String> {
    read(group, GAME, xml.as_bytes())
}

/// Parses one in-memory plugin whose root must name `game`, with that
/// game's pointer widths.
pub fn parse_game(group: &str, game: &str, xml: &str) -> Result<Plugin, String> {
    read(group, game, xml.as_bytes())
}

pub(super) fn read<R: BufRead>(group: &str, game: &str, input: R) -> Result<Plugin, String> {
    let widths = Widths::of(game);
    let mut reader = Reader::from_reader(input);
    let base_size = header(&mut reader, game)?;
    let mut fields = Vec::new();
    let mut stack: Vec<Node> = Vec::new();
    let mut buf = Vec::new();
    let mut ended = false;
    loop {
        match reader.read_event_into(&mut buf).map_err(message)? {
            Event::Start(e) if !ended => {
                if stack.len() >= 128 {
                    return Err("plugin nesting exceeds 128 elements".into());
                }
                stack.push(Node::start(&e)?);
            }
            Event::End(_) if !ended => match stack.pop() {
                Some(node) => match stack.last_mut() {
                    Some(parent) => parent.children.push(node),
                    None if node.attr("offset").is_some() => fields.push(node.field(widths)?),
                    None => {}
                },
                None => ended = true,
            },
            Event::Text(text) => {
                if let Some(node) = stack.last_mut() {
                    node.text.push_str(&text.xml10_content().map_err(message)?);
                } else if !text.iter().all(u8::is_ascii_whitespace) {
                    return Err("unexpected text outside a field".into());
                }
            }
            Event::CData(text) => {
                let node = stack.last_mut().ok_or("unexpected CDATA outside a field")?;
                node.text.push_str(&text.xml10_content().map_err(message)?);
            }
            Event::GeneralRef(e) => {
                let decoded = e.decode().map_err(message)?;
                let node = stack.last_mut().ok_or("unexpected entity outside a field")?;
                if let Some(ch) = e.resolve_char_ref().map_err(message)? {
                    node.text.push(ch);
                } else if let Some(s) = quick_xml::escape::resolve_xml_entity(&decoded) {
                    node.text.push_str(s);
                } else {
                    return Err(format!("unsupported XML entity {decoded}"));
                }
            }
            Event::Comment(_) | Event::PI(_) => {}
            Event::Eof if ended => break,
            Event::Eof => return Err("incomplete plugin".into()),
            _ => return Err("unexpected markup in plugin".into()),
        }
        buf.clear();
    }
    Ok(Plugin {
        group: group.to_owned(),
        game: game.to_owned(),
        base_size,
        fields,
    })
}
