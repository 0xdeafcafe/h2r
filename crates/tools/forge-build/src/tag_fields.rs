//! Typed writes to ordinary metadata. Only fields named in the editor's
//! registry are writable here. Child blocks must stay in the tag payload.

use std::{
    collections::BTreeMap,
    io::{Read, Seek},
    sync::Arc,
};

use forge_doc::{F32, Refusal, TagKey, TagWrite, Value};
use map::{Map, TagId, TagLocation};
use tags::registry::{Kind, Layout, Registry};

use crate::placement::{invalid, lookup};

#[derive(Default)]
pub(super) struct TagFields {
    pub patches: BTreeMap<u32, Vec<u8>>,
    pub strings: Vec<String>,
}

impl TagFields {
    pub fn write<R: Read + Seek>(
        &mut self,
        map: &mut Map<R>,
        write: &TagWrite,
    ) -> forge_doc::Result<()> {
        let tag = lookup(map, &write.tag)?;
        if tag.group.0 == *b"scnr" {
            return Err(invalid(
                &write.path,
                "scenario fields belong to placement commands",
            ));
        }
        let location = tag
            .location
            .filter(|l| Some(l.space) == map.metadata_space())
            .ok_or_else(|| invalid(&write.path, "tag has no ordinary local metadata"))?;
        let layout = Registry::builtin()
            .group(tag.group)
            .map(Arc::as_ref)
            .ok_or_else(|| invalid(&write.path, "no field layout for the tag group"))?;
        let (at, kind) = resolve(map, location, layout, &write.path)?;
        if !matches!(
            kind,
            Kind::Int { count: 1, .. }
                | Kind::Enum { .. }
                | Kind::Flags { .. }
                | Kind::Float { count: 1..=4, .. }
                | Kind::StringId
                | Kind::TagRef { .. }
                | Kind::Datum { .. }
        ) {
            return Err(invalid(&write.path, "field has no writable portable value"));
        }
        let width = kind.width();
        let end = at
            .checked_add(width)
            .ok_or_else(|| invalid(&write.path, "field address overflows"))?;
        let current = match self.patches.range(..end).next_back() {
            Some((&start, bytes)) if start == at && bytes.len() == width as usize => bytes.clone(),
            Some((&start, bytes)) if u64::from(start) + bytes.len() as u64 > u64::from(at) => {
                return Err(invalid(&write.path, "overlaps another field write"));
            }
            _ => read(map, location, at, width, &write.path)?,
        };
        if let Some(was) = write.was
            && self.decode(map, kind, &current, &write.path)?.digest()? != was
        {
            return Err(Refusal::Mismatch);
        }
        let bytes = self.encode(map, kind, &write.value, &write.path)?;
        if bytes != current {
            self.patches.insert(at, bytes);
        }
        Ok(())
    }

    fn decode<R: Read + Seek>(
        &self,
        map: &Map<R>,
        kind: &Kind,
        bytes: &[u8],
        path: &str,
    ) -> forge_doc::Result<Value> {
        let wrong = || invalid(path, "this field has no portable scalar or vector value");
        Ok(match kind {
            Kind::Int {
                count: 1, signed, ..
            } => Value::Int(integer(bytes, *signed)),
            Kind::Enum { .. } | Kind::Flags { .. } => Value::Int(integer(bytes, false)),
            Kind::Float { .. } => {
                let values = bytes
                    .chunks_exact(4)
                    .map(|b| F32::new(f32::from_bits(word(b))))
                    .collect::<forge_doc::Result<Vec<_>>>()?;
                match values.as_slice() {
                    [x] => Value::Float(*x),
                    [x, y] => Value::Vec2([*x, *y]),
                    [x, y, z] => Value::Vec3([*x, *y, *z]),
                    [x, y, z, w] => Value::Vec4([*x, *y, *z, *w]),
                    _ => return Err(wrong()),
                }
            }
            Kind::TagRef { .. } | Kind::Datum { .. } => {
                let id = TagId(word(&bytes[bytes.len() - 4..]));
                let key = if id == TagId::NULL {
                    None
                } else {
                    let tag = map.tag(id).map_err(|e| invalid(path, e))?;
                    let name = tag
                        .name
                        .clone()
                        .ok_or_else(|| invalid(path, "reference has no tag name"))?;
                    Some(TagKey {
                        group: tags::defs::fourcc_text(tag.group),
                        path: name,
                    })
                };
                Value::Tag(key)
            }
            Kind::StringId => {
                let id = word(bytes);
                let index = (id & 0x00ff_ffff) as usize;
                let text = if id == tags::StringId::NULL.0 {
                    ""
                } else if let Some(index) = index.checked_sub(map.strings().len()) {
                    self.strings
                        .get(index)
                        .map(String::as_str)
                        .ok_or_else(|| invalid(path, "unknown string ID"))?
                } else {
                    tags::StringId(id)
                        .resolve(map.strings())
                        .map_err(|e| invalid(path, e))?
                        .ok_or_else(|| invalid(path, "unknown string ID"))?
                };
                Value::Text(text.into())
            }
            _ => return Err(wrong()),
        })
    }

    fn encode<R: Read + Seek>(
        &mut self,
        map: &Map<R>,
        kind: &Kind,
        value: &Value,
        path: &str,
    ) -> forge_doc::Result<Vec<u8>> {
        let wrong = || invalid(path, "wrong value type or field is not writable");
        match (kind, value) {
            (
                Kind::Int {
                    bytes,
                    signed,
                    count: 1,
                },
                Value::Int(n),
            ) => encode_int(*n, *bytes, *signed, path),
            (Kind::Enum { bytes, .. } | Kind::Flags { bytes, .. }, Value::Int(n)) => {
                encode_int(*n, *bytes, false, path)
            }
            (Kind::Float { count, .. }, value) => {
                let floats: &[F32] = match value {
                    Value::Float(x) => std::slice::from_ref(x),
                    Value::Vec2(a) => a,
                    Value::Vec3(a) => a,
                    Value::Vec4(a) => a,
                    _ => return Err(wrong()),
                };
                if floats.len() != *count as usize {
                    return Err(wrong());
                }
                Ok(floats.iter().flat_map(|v| v.get().to_le_bytes()).collect())
            }
            (Kind::TagRef { groups } | Kind::Datum { groups }, Value::Tag(key)) => {
                let (group, id) = if let Some(key) = key {
                    let tag = lookup(map, key)?;
                    let ancestors = map.groups().iter().find(|g| g.class == tag.group);
                    let accepts = groups.is_empty()
                        || groups.iter().any(|group| {
                            *group == tag.group
                                || ancestors.is_some_and(|g| {
                                    g.parent == Some(*group) || g.grandparent == Some(*group)
                                })
                        });
                    if !accepts {
                        return Err(invalid(path, "field refuses this tag group"));
                    }
                    (tag.group.as_u32(), tag.id.0)
                } else {
                    (u32::MAX, TagId::NULL.0)
                };
                let mut bytes = Vec::new();
                if matches!(kind, Kind::TagRef { .. }) {
                    bytes.extend_from_slice(&group.to_le_bytes());
                }
                bytes.extend_from_slice(&id.to_le_bytes());
                Ok(bytes)
            }
            (Kind::StringId, Value::Text(text)) => {
                let id = self.string_id(map, text, path)?;
                Ok(id.to_le_bytes().to_vec())
            }
            _ => Err(wrong()),
        }
    }

    fn string_id<R: Read + Seek>(
        &mut self,
        map: &Map<R>,
        text: &str,
        path: &str,
    ) -> forge_doc::Result<u32> {
        if text.is_empty() {
            return Ok(tags::StringId::NULL.0);
        }
        let old = map
            .strings()
            .iter()
            .position(|s| s.as_deref() == Some(text));
        let added = self
            .strings
            .iter()
            .position(|s| s == text)
            .map(|i| map.strings().len() + i);
        let index = old
            .or(added)
            .unwrap_or_else(|| map.strings().len() + self.strings.len());
        let id = map::build::string_id(index, text).map_err(|e| invalid(path, e))?;
        if old.is_none() && added.is_none() {
            if text.len() >= map::build::FIXED_STRING_SLOT {
                return Err(invalid(path, "string is too long for a new string ID"));
            }
            self.strings.push(text.into());
        }
        Ok(id)
    }
}

fn resolve<'a, R: Read + Seek>(
    map: &mut Map<R>,
    location: TagLocation,
    mut layout: &'a Layout,
    path: &str,
) -> forge_doc::Result<(u32, &'a Kind)> {
    let mut at = location.address;
    checked_offset(location, at, layout.size, path)?;
    let root_end = at
        .checked_add(layout.size)
        .ok_or_else(|| invalid(path, "tag root overflows"))?;
    let mut steps = path.split('.').peekable();
    while let Some(step) = steps.next() {
        let (name, index) = match step.split_once('[') {
            Some((name, subscript)) => (
                name,
                subscript
                    .strip_suffix(']')
                    .and_then(|i| i.parse::<u32>().ok()),
            ),
            None => (step, None),
        };
        let field = layout
            .fields
            .iter()
            .find(|f| f.named && f.name == name)
            .ok_or_else(|| invalid(path, "unknown named field"))?;
        let address = at
            .checked_add(field.offset)
            .ok_or_else(|| invalid(path, "field address overflows"))?;
        match (&field.kind, index, steps.peek()) {
            (_, None, None) => return Ok((address, &field.kind)),
            (Kind::Block(child), Some(index), Some(_)) => {
                let pair = read(map, location, address, 8, path)?;
                let count = word(&pair[..4]);
                let pointer = word(&pair[4..]);
                if index >= count {
                    return Err(invalid(path, "block index is out of bounds"));
                }
                let len = count
                    .checked_mul(child.size)
                    .ok_or_else(|| invalid(path, "block size overflows"))?;
                if pointer < root_end {
                    return Err(invalid(path, "child block overlaps the tag root"));
                }
                checked_offset(location, pointer, len, path)?;
                at = pointer
                    .checked_add(index * child.size)
                    .ok_or_else(|| invalid(path, "element address overflows"))?;
                layout = child;
            }
            _ => {
                return Err(invalid(
                    path,
                    "only a block with an index can contain a field",
                ));
            }
        }
    }
    Err(invalid(path, "empty field path"))
}

fn checked_offset(
    location: TagLocation,
    address: u32,
    len: u32,
    path: &str,
) -> forge_doc::Result<u32> {
    address
        .checked_sub(location.address)
        .filter(|&n| len != 0 && u64::from(n) + u64::from(len) <= u64::from(location.size))
        .ok_or_else(|| invalid(path, "field or block lies outside its tag"))
}

fn read<R: Read + Seek>(
    map: &mut Map<R>,
    location: TagLocation,
    address: u32,
    len: u32,
    path: &str,
) -> forge_doc::Result<Vec<u8>> {
    checked_offset(location, address, len, path)?;
    let file = location
        .space
        .translate(address, len)
        .map_err(|e| invalid(path, e))?;
    map.read_file_range(file, len.into())
        .map_err(|e| invalid(path, e))
}

fn encode_int(value: i64, bytes: u8, signed: bool, path: &str) -> forge_doc::Result<Vec<u8>> {
    let width = u32::from(bytes) * 8;
    let (min, max) = if signed {
        (-(1_i64 << (width - 1)), (1_i64 << (width - 1)) - 1)
    } else {
        (0, (1_i64 << width) - 1)
    };
    if !(min..=max).contains(&value) {
        return Err(invalid(path, "integer does not fit its field"));
    }
    Ok(value.to_le_bytes()[..usize::from(bytes)].to_vec())
}

fn integer(bytes: &[u8], signed: bool) -> i64 {
    let fill = if signed && bytes.last().is_some_and(|byte| byte & 0x80 != 0) {
        0xff
    } else {
        0
    };
    let mut data = [fill; 8];
    data[..bytes.len()].copy_from_slice(bytes);
    i64::from_le_bytes(data)
}

fn word(bytes: &[u8]) -> u32 {
    u32::from_le_bytes(bytes.try_into().expect("one field word"))
}
