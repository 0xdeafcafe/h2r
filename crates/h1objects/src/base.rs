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
}

impl Base {
    pub fn read<R: Read + Seek>(map: &mut Map<R>) -> Result<Self> {
        Ok(Self {
            meta: Metadata::read(map)?,
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

    /// A tag's root (its group's root size).
    pub fn root(&self, id: TagId) -> Result<Vec<u8>> {
        let t = self
            .tags
            .iter()
            .find(|t| t.0 == id)
            .ok_or_else(|| Error::NoTemplate(format!("{id:?}")))?;
        let def = GroupDef::find(t.1).ok_or_else(|| Error::NoTemplate(t.1.to_string()))?;
        Ok(self.meta.slice(t.3, def.root_size.into())?.to_vec())
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
