//! The retail Halo 2 map the converted objects are built on: where the
//! templates come from.

use std::io::{Read, Seek};

use map::{FourCC, Map, TagId};
use tags::object::{GroupDef, Metadata};

use crate::{Error, Result};

/// A Halo 2 map's tag list and metadata, read once.
pub struct Base {
    pub meta: Metadata,
    /// Every tag with a payload: datum, group, name, payload address.
    pub tags: Vec<(TagId, FourCC, String, u32)>,
    /// The shader converted models' materials copy, with Halo 1's bitmaps
    /// ([`h1convert::textures::model_template`]); `None` leaves them null.
    pub model_shader: Option<h1convert::textures::ModelTemplate>,
    /// The globals' material names, in order: a material's index is its
    /// position ([`global_materials`]).
    pub materials: Vec<String>,
}

/// `matg +0x150`: the global materials block; elements of `0xb4`, the
/// name a string ID at `+0`. V: on the disc's maps every projectile
/// material response's material index (`proj +0x19c` element `+0x10`) is
/// the position of its name here (`tests/weapon_rebuild_disc.rs`).
pub const GLOBAL_MATERIALS: usize = 0x150;
pub const GLOBAL_MATERIAL_SIZE: usize = 0xb4;

/// The global material names of `map`'s globals (none without globals).
pub fn global_materials<R: Read + Seek>(map: &mut Map<R>, meta: &Metadata) -> Vec<String> {
    let Some(t) = map.tags().iter().find(|t| t.group == FourCC(*b"matg")) else {
        return Vec::new();
    };
    let Some(at) = t.location.map(|l| l.address) else {
        return Vec::new();
    };
    let word = |b: &[u8], o: usize| u32::from_le_bytes(b[o..o + 4].try_into().expect("4"));
    let Ok(head) = meta.slice(at, (GLOBAL_MATERIALS + 8) as u64) else {
        return Vec::new();
    };
    let (n, first) = (
        word(head, GLOBAL_MATERIALS),
        word(head, GLOBAL_MATERIALS + 4),
    );
    let strings = map.strings();
    (0..n)
        .map(|i| {
            meta.slice(first + i * GLOBAL_MATERIAL_SIZE as u32, 4)
                .ok()
                .and_then(|b| {
                    strings
                        .get((word(b, 0) & 0xffff) as usize)
                        .cloned()
                        .flatten()
                })
                .unwrap_or_default()
        })
        .collect()
}

impl Base {
    pub fn read<R: Read + Seek>(map: &mut Map<R>) -> Result<Self> {
        let meta = Metadata::read(map)?;
        let materials = global_materials(map, &meta);
        Ok(Self {
            meta,
            materials,
            tags: map
                .tags()
                .iter()
                .filter_map(|t| Some((t.id, t.group, t.name.clone()?, t.location?.address)))
                .collect(),
            model_shader: h1convert::textures::model_template(map).ok().flatten(),
        })
    }

    /// The tag of `group` named `name`.
    pub fn find(&self, group: &[u8; 4], name: &str) -> Option<TagId> {
        self.tags
            .iter()
            .find(|t| t.1 == FourCC(*group) && t.2 == name)
            .map(|t| t.0)
    }

    pub fn name(&self, id: TagId) -> Option<(FourCC, &str)> {
        self.tags
            .iter()
            .find(|t| t.0 == id)
            .map(|t| (t.1, t.2.as_str()))
    }

    /// A tag's root (its group's root size: the object layouts' for the
    /// object groups and `hlmt`, else the group's definition's, as for
    /// `mode` and `phmo`).
    pub fn root(&self, id: TagId) -> Result<Vec<u8>> {
        let t = self
            .tags
            .iter()
            .find(|t| t.0 == id)
            .ok_or_else(|| Error::NoTemplate(format!("{id:?}")))?;
        let size = match GroupDef::find(t.1) {
            Some(def) => def.root_size,
            None => {
                tags::defs::Defs::builtin()
                    .group(t.1)
                    .ok_or_else(|| Error::NoTemplate(t.1.to_string()))?
                    .size
            }
        };
        Ok(self.meta.slice(t.3, size.into())?.to_vec())
    }

    /// A tag's first `len` bytes, for groups with no root size here.
    pub fn head(&self, id: TagId, len: u64) -> Result<Vec<u8>> {
        let t = self
            .tags
            .iter()
            .find(|t| t.0 == id)
            .ok_or_else(|| Error::NoTemplate(format!("{id:?}")))?;
        Ok(self.meta.slice(t.3, len)?.to_vec())
    }

    /// Element `i` (`stride` bytes) of the block field at `at` in `bytes`.
    pub fn element(
        &self,
        bytes: &[u8],
        at: usize,
        stride: usize,
        i: usize,
    ) -> Result<Option<Vec<u8>>> {
        let count = u32_at(bytes, at) as usize;
        if i >= count {
            return Ok(None);
        }
        let address = u32_at(bytes, at + 4) + (i * stride) as u32;
        Ok(Some(self.meta.slice(address, stride as u64)?.to_vec()))
    }

    /// The datum of the 8-byte reference at `at` in `bytes`, if live.
    pub fn reference(&self, bytes: &[u8], at: usize) -> Option<TagId> {
        let id = TagId(u32_at(bytes, at + 4));
        (id != TagId::NULL && self.name(id).is_some()).then_some(id)
    }

    /// Tag `id` read through its typed definition `def`: its root, every
    /// block and data reference followed in the metadata. References keep
    /// the base's datums ([`tags::defs::Target::Id`]) and string IDs their
    /// indices ([`tags::defs::Str::Index`]): a record for a tag installed
    /// on this base, as the template path's copied bytes are.
    pub fn record(&self, def: &tags::defs::StructDef, id: TagId) -> Result<tags::defs::Record> {
        let t = self
            .tags
            .iter()
            .find(|t| t.0 == id)
            .ok_or_else(|| Error::NoTemplate(format!("{id:?}")))?;
        let bytes = self.meta.slice(t.3, def.size.into())?.to_vec();
        let b = self.built(def, bytes, 1)?;
        tags::defs::from_built(def, &b).map_err(|e| Error::Invalid(format!("{}: {e}", t.2)))
    }

    /// `count` elements of `def` in `bytes` as a builder block whose
    /// blocks and data are children, as [`Self::record`] reads them.
    fn built(
        &self,
        def: &tags::defs::StructDef,
        bytes: Vec<u8>,
        count: usize,
    ) -> Result<map::build::Block> {
        let mut b = map::build::Block::new(bytes).aligned(def.align.max(1));
        for i in 0..count {
            self.children(def, &mut b, i * def.size as usize)?;
        }
        Ok(b)
    }

    fn children(
        &self,
        def: &tags::defs::StructDef,
        b: &mut map::build::Block,
        at: usize,
    ) -> Result<()> {
        use tags::defs::Kind;
        for f in &def.fields {
            let o = at + f.offset as usize;
            match &f.kind {
                Kind::Struct(s) => self.children(s, b, o)?,
                Kind::Block(e) => {
                    let n = u32_at(&b.bytes, o) as usize;
                    if n == 0 {
                        b.bytes[o..o + 8].fill(0);
                        continue;
                    }
                    let addr = u32_at(&b.bytes, o + 4);
                    let raw = self
                        .meta
                        .slice(addr, (n * e.size as usize) as u64)?
                        .to_vec();
                    let child = self.built(e, raw, n)?;
                    b.block(o as u32, n as u32, child);
                }
                Kind::Data { align } => {
                    let n = u32_at(&b.bytes, o) as u64;
                    if n == 0 {
                        b.bytes[o..o + 8].fill(0);
                        continue;
                    }
                    let addr = u32_at(&b.bytes, o + 4);
                    let raw = self.meta.slice(addr, n)?.to_vec();
                    b.data(
                        o as u32,
                        map::build::Block::new(raw).aligned((*align).max(1)),
                    );
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Every live 8-byte reference (group word, datum of a tag of that
    /// group) in `bytes`, at 4-byte steps: (offset, group, name).
    pub fn references(&self, bytes: &[u8]) -> Vec<(usize, FourCC, String)> {
        let mut out = Vec::new();
        let mut at = 0;
        while at + 8 <= bytes.len() {
            let id = TagId(u32_at(bytes, at + 4));
            if let Some((g, n)) = self.name(id)
                && u32_at(bytes, at) == u32::from_be_bytes(g.0)
            {
                out.push((at, g, n.to_owned()));
                at += 8;
                continue;
            }
            at += 4;
        }
        out
    }
}

pub(crate) fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}
