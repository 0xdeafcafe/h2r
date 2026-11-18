//! A map's HUDs: every `nhdt` with who names it (bipeds' and vehicles'
//! HUD interfaces, weapons, the globals), and one HUD's widgets as bytes
//! at their metadata addresses, so an edit is a patch of those bytes.

use std::io::{Read, Seek};

use anyhow::{Context as _, Result, bail};
use map::{FourCC, Map, TagId};

use crate::layout::{self, Field, Kind, Mode, bitmap, text};

pub const NHDT: FourCC = FourCC(*b"nhdt");
pub const BITM: FourCC = FourCC(*b"bitm");

/// `unit +0x1a0`: a unit's HUD interfaces, a block of 8-byte `nhdt`
/// references (docs/halo1/health.md; E `0x13954b` draws them).
pub const UNIT_HUD_INTERFACES: usize = 0x1a0;
/// `weap +0x2b0`: a weapon's HUD (E `0x13b071` reads the datum at
/// `+0x2b4`).
pub const WEAPON_HUD: usize = 0x2b0;

/// The name suffixes the Halo 1 converter gives the HUDs it writes
/// (`h1objects::h1hud`, `h1objects::health_hud`).
pub const HALO1_SUFFIXES: [&str; 3] = ["_h1_hud", "_h1_weapon_hud", "_h1_health"];

/// Who names a HUD.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Owner {
    pub group: FourCC,
    pub id: TagId,
    pub name: String,
    /// Where the reference sits: `unit +0x1a0[i]`, `weap +0x2b0`, or a
    /// root offset found by scanning.
    pub at: String,
}

/// One `nhdt` of the map.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HudEntry {
    pub id: TagId,
    pub name: String,
    pub owners: Vec<Owner>,
    /// Written by the Halo 1 converter.
    pub halo1: bool,
}

impl HudEntry {
    /// The list's grouping: who uses it.
    pub fn category(&self) -> &'static str {
        if self.halo1 {
            return "Halo 1 (converted)";
        }
        let has = |g: &[u8; 4]| self.owners.iter().any(|o| o.group.0 == *g);
        if has(b"bipd") {
            "Players and bipeds"
        } else if has(b"weap") {
            "Weapons"
        } else if has(b"vehi") {
            "Vehicles"
        } else if has(b"matg") {
            "Globals"
        } else {
            "Other"
        }
    }
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

fn tag_name(map: &Map<impl Read + Seek>, id: TagId) -> String {
    map.tag(id)
        .ok()
        .and_then(|t| t.name.clone())
        .unwrap_or_else(|| format!("{id}"))
}

/// Every `nhdt` of `map`, by name, with the bipeds, vehicles, weapons and
/// globals that name it.
pub fn list<R: Read + Seek>(map: &mut Map<R>) -> Result<Vec<HudEntry>> {
    let huds: Vec<TagId> = map
        .tags()
        .iter()
        .filter(|t| t.group == NHDT && t.location.is_some())
        .map(|t| t.id)
        .collect();
    let is_hud = |v: u32| huds.iter().any(|h| h.0 == v);
    let mut out: Vec<HudEntry> = huds
        .iter()
        .map(|&id| {
            let name = tag_name(map, id);
            HudEntry {
                id,
                halo1: HALO1_SUFFIXES.iter().any(|s| name.ends_with(s)),
                name,
                owners: Vec::new(),
            }
        })
        .collect();
    let owners: Vec<(TagId, FourCC)> = map
        .tags()
        .iter()
        .filter(|t| {
            t.location.is_some() && [b"bipd", b"vehi", b"weap", b"matg"].contains(&&t.group.0)
        })
        .map(|t| (t.id, t.group))
        .collect();
    for (id, group) in owners {
        let Ok(root) = map.read_tag_bytes(id) else {
            continue;
        };
        let mut found: Vec<(u32, String)> = Vec::new();
        // A unit's HUD interfaces.
        if (group.0 == *b"bipd" || group.0 == *b"vehi") && root.len() >= UNIT_HUD_INTERFACES + 8 {
            let count = u32_at(&root, UNIT_HUD_INTERFACES);
            let address = u32_at(&root, UNIT_HUD_INTERFACES + 4);
            if count > 0 && count < 64 {
                let block = map
                    .file_offset(address, count * 8)
                    .and_then(|o| map.read_file_range(o, u64::from(count) * 8));
                if let Ok(block) = block {
                    for i in 0..count as usize {
                        let v = u32_at(&block, i * 8 + 4);
                        if is_hud(v) {
                            found.push((v, format!("unit +0x1a0[{i}]")));
                        }
                    }
                }
            }
        }
        // Any other reference in the tag's data (its root and the blocks
        // laid out in it): a group word then the datum.
        for at in (0..root.len().saturating_sub(7)).step_by(4) {
            let v = u32_at(&root, at + 4);
            if u32_at(&root, at) == NHDT.as_u32() && is_hud(v) && !found.iter().any(|f| f.0 == v) {
                let label = match (&group.0, at) {
                    (b"weap", WEAPON_HUD) => "weap +0x2b0".to_owned(),
                    _ => format!("{group} +{at:#x}"),
                };
                found.push((v, label));
            }
        }
        if found.is_empty() {
            continue;
        }
        let name = tag_name(map, id);
        for (hud, at) in found {
            if let Some(e) = out.iter_mut().find(|e| e.id.0 == hud) {
                let o = Owner {
                    group,
                    id,
                    name: name.clone(),
                    at,
                };
                if !e.owners.contains(&o) {
                    e.owners.push(o);
                }
            }
        }
    }
    out.sort_by(|a, b| a.category().cmp(b.category()).then(a.name.cmp(&b.name)));
    Ok(out)
}

/// A widget's kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WidgetKind {
    Bitmap,
    Text,
}

impl WidgetKind {
    pub fn fields(self) -> &'static [Field] {
        match self {
            WidgetKind::Bitmap => layout::BITMAP_FIELDS,
            WidgetKind::Text => layout::TEXT_FIELDS,
        }
    }
    pub fn size(self) -> usize {
        match self {
            WidgetKind::Bitmap => layout::BITMAP_SIZE,
            WidgetKind::Text => layout::TEXT_SIZE,
        }
    }
}

/// Which widget: its kind and index in its block.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WidgetKey {
    pub kind: WidgetKind,
    pub index: usize,
}

/// A field value, as read from a widget.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value {
    Int(i64),
    Float(f32),
    /// A tag reference's datum.
    Tag(TagId),
    /// A string ID.
    String(u32),
}

/// One widget: its bytes now and as read, and where they live.
#[derive(Clone, Debug, PartialEq)]
pub struct Widget {
    pub key: WidgetKey,
    /// Its first byte's metadata address.
    pub address: u32,
    pub bytes: Vec<u8>,
    pub original: Vec<u8>,
    /// The name string, resolved.
    pub name: String,
}

impl Widget {
    fn i16_at(&self, at: usize) -> i16 {
        i16::from_le_bytes([self.bytes[at], self.bytes[at + 1]])
    }
    fn u16_at(&self, at: usize) -> u16 {
        u16::from_le_bytes([self.bytes[at], self.bytes[at + 1]])
    }
    fn f32_at(&self, at: usize) -> f32 {
        f32::from_le_bytes(self.bytes[at..at + 4].try_into().unwrap())
    }
    pub fn anchor(&self) -> u16 {
        self.u16_at(bitmap::ANCHOR)
    }
    pub fn flags(&self) -> u16 {
        self.u16_at(bitmap::FLAGS)
    }
    pub fn inputs(&self) -> [u8; 4] {
        self.bytes[4..8].try_into().unwrap()
    }
    pub fn state(&self) -> &[u8] {
        &self.bytes[bitmap::STATE..bitmap::STATE_END]
    }
    fn offsets(&self) -> [usize; 3] {
        match self.key.kind {
            WidgetKind::Bitmap => bitmap::OFFSETS,
            WidgetKind::Text => text::OFFSETS,
        }
    }
    pub fn offset(&self, m: Mode) -> (i16, i16) {
        let at = self.offsets()[m as usize];
        (self.i16_at(at), self.i16_at(at + 2))
    }
    pub fn set_offset(&mut self, m: Mode, (x, y): (i16, i16)) {
        let at = self.offsets()[m as usize];
        self.bytes[at..at + 2].copy_from_slice(&x.to_le_bytes());
        self.bytes[at + 2..at + 4].copy_from_slice(&y.to_le_bytes());
    }
    /// A bitmap widget's registration for mode `m`.
    pub fn registration(&self, m: Mode) -> [f32; 2] {
        let at = bitmap::REGISTRATIONS[m as usize];
        [self.f32_at(at), self.f32_at(at + 4)]
    }
    pub fn sequence(&self, m: Mode) -> i8 {
        self.bytes[bitmap::SEQUENCES + m as usize] as i8
    }
    pub fn special(&self) -> u16 {
        self.u16_at(bitmap::SPECIAL)
    }
    /// The datum of the reference at `at` (`None` for a null one).
    pub fn reference(&self, at: usize) -> Option<TagId> {
        let v = u32::from_le_bytes(self.bytes[at + 4..at + 8].try_into().unwrap());
        (v != u32::MAX).then_some(TagId(v))
    }
    pub fn bitmap(&self) -> Option<TagId> {
        match self.key.kind {
            WidgetKind::Bitmap => self.reference(bitmap::BITMAP),
            WidgetKind::Text => None,
        }
    }
    pub fn shader(&self) -> Option<TagId> {
        self.reference(match self.key.kind {
            WidgetKind::Bitmap => bitmap::SHADER,
            WidgetKind::Text => text::SHADER,
        })
    }
    pub fn justification(&self) -> u16 {
        self.u16_at(text::JUSTIFICATION)
    }
    pub fn font(&self, m: Mode) -> i8 {
        self.bytes[text::FONTS + m as usize] as i8
    }
    pub fn string(&self) -> u32 {
        u32::from_le_bytes(self.bytes[text::STRING..text::STRING + 4].try_into().unwrap())
    }
    pub fn is_dirty(&self) -> bool {
        self.bytes != self.original
    }

    /// Field `key`'s value.
    pub fn get(&self, key: &str) -> Option<Value> {
        let f = self.key.kind.fields().iter().find(|f| f.key == key)?;
        Some(self.get_field(f))
    }
    pub fn get_field(&self, f: &Field) -> Value {
        let at = f.offset;
        match f.kind {
            Kind::I8 => Value::Int(i64::from(self.bytes[at] as i8)),
            Kind::U8 => Value::Int(i64::from(self.bytes[at])),
            Kind::I16 => Value::Int(i64::from(self.i16_at(at))),
            Kind::U16 | Kind::Hex16 => Value::Int(i64::from(self.u16_at(at))),
            Kind::F32 => Value::Float(self.f32_at(at)),
            Kind::TagRef => Value::Tag(
                self.reference(at)
                    .unwrap_or(TagId::NULL),
            ),
            Kind::StringId => {
                Value::String(u32::from_le_bytes(self.bytes[at..at + 4].try_into().unwrap()))
            }
        }
    }
    /// The bytes field `f` would hold with `v` (`None`: out of range or
    /// the wrong type).
    pub fn encode(f: &Field, v: Value) -> Option<Vec<u8>> {
        Some(match (f.kind, v) {
            (Kind::I8, Value::Int(i)) => vec![i8::try_from(i).ok()? as u8],
            (Kind::U8, Value::Int(i)) => vec![u8::try_from(i).ok()?],
            (Kind::I16, Value::Int(i)) => i16::try_from(i).ok()?.to_le_bytes().to_vec(),
            (Kind::U16 | Kind::Hex16, Value::Int(i)) => {
                u16::try_from(i).ok()?.to_le_bytes().to_vec()
            }
            (Kind::F32, Value::Float(x)) if x.is_finite() => x.to_le_bytes().to_vec(),
            (Kind::TagRef, Value::Tag(id)) => id.0.to_le_bytes().to_vec(),
            _ => return None,
        })
    }
    /// The range a field's value occupies (a tag reference's datum only:
    /// its group word stays).
    pub fn value_range(f: &Field) -> std::ops::Range<usize> {
        match f.kind {
            Kind::TagRef => f.offset + 4..f.offset + 8,
            k => f.offset..f.offset + k.size(),
        }
    }
}

/// One HUD: its widgets in draw order (bitmaps, then texts: E `0x13a532`).
#[derive(Clone, Debug, PartialEq)]
pub struct Hud {
    pub id: TagId,
    pub name: String,
    pub widgets: Vec<Widget>,
}

impl Hud {
    pub fn widget(&self, key: WidgetKey) -> Option<&Widget> {
        self.widgets.iter().find(|w| w.key == key)
    }
    pub fn widget_mut(&mut self, key: WidgetKey) -> Option<&mut Widget> {
        self.widgets.iter_mut().find(|w| w.key == key)
    }
    pub fn is_dirty(&self) -> bool {
        self.widgets.iter().any(Widget::is_dirty)
    }
}

/// String ID `v`'s name in `map`.
pub fn string<R: Read + Seek>(map: &Map<R>, v: u32) -> Option<String> {
    tags::StringId(v)
        .resolve(map.strings())
        .ok()
        .flatten()
        .map(str::to_owned)
}

/// The `nhdt` `id`'s widgets.
pub fn read<R: Read + Seek>(map: &mut Map<R>, id: TagId) -> Result<Hud> {
    let tag = map.tag(id)?.clone();
    if tag.group != NHDT {
        bail!("{id} is a {} tag, not an nhdt", tag.group);
    }
    let name = tag.name.clone().unwrap_or_else(|| format!("{id}"));
    let root = map.read_tag_bytes(id)?;
    if root.len() < layout::ROOT_SIZE {
        bail!("{name}: the nhdt root is {} bytes, under {:#x}", root.len(), layout::ROOT_SIZE);
    }
    let mut widgets = Vec::new();
    for (kind, at) in [
        (WidgetKind::Bitmap, layout::ROOT_BITMAPS),
        (WidgetKind::Text, layout::ROOT_TEXTS),
    ] {
        let count = u32_at(&root, at);
        let address = u32_at(&root, at + 4);
        if count == 0 {
            continue;
        }
        if count > 1024 {
            bail!("{name}: {count} {kind:?} widgets");
        }
        let size = kind.size() as u32;
        let offset = map
            .file_offset(address, count * size)
            .with_context(|| format!("{name}: {kind:?} widgets at {address:#x}"))?;
        let bytes = map.read_file_range(offset, u64::from(count * size))?;
        for (i, w) in bytes.chunks_exact(size as usize).enumerate() {
            let label = string(map, u32_at(w, 0)).unwrap_or_else(|| format!("{kind:?} {i}"));
            widgets.push(Widget {
                key: WidgetKey { kind, index: i },
                address: address + i as u32 * size,
                bytes: w.to_vec(),
                original: w.to_vec(),
                name: label,
            });
        }
    }
    Ok(Hud {
        id,
        name,
        widgets,
    })
}
