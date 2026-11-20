//! A converted tag: a `map::build` block tree plus the raw resources it
//! points at, ready to add to a [`TagSet`].

use map::build::{Block, FixupKind, RawId, RawRef, TagPayload, TagSet};
use map::{FourCC, TagId};

use crate::models::Result;

/// Placeholder datum for "this tag" inside a [`Converted`] tree (geometry
/// and resource owner fields). [`Converted::install`] replaces it with the
/// tag's real datum.
pub const SELF_ID: TagId = TagId(0xfffe_fffe);

/// A Halo 2 tag built from a Halo 1 one.
#[derive(Clone, Debug)]
pub struct Converted {
    /// Halo 2 group (`mode`, `jmad`).
    pub group: FourCC,
    /// The tag's block tree. [`RawRef::Local`] fixups index [`Self::raw`],
    /// and [`SELF_ID`] stands for the tag's own datum.
    pub root: Block,
    /// Raw resources: bytes and file alignment.
    pub raw: Vec<(Vec<u8>, u32)>,
}

impl Converted {
    /// Adds the tag to `set` under `name`: places its raw resources, points
    /// the tree's raw fixups at them, fills in its own datum, and declares
    /// the group if needed. Returns the tag's datum.
    pub fn install(mut self, set: &mut TagSet, name: &str) -> Result<TagId> {
        let slot = set.slots.len();
        let id = TagId(u32::from(set.salt_base.wrapping_add(slot as u16)) << 16 | slot as u32);
        let ids: Vec<RawId> = self
            .raw
            .drain(..)
            .map(|(bytes, align)| set.add_raw(bytes, align))
            .collect();
        rewrite(&mut self.root, &ids, id);
        set.add_group(self.group, None, None);
        let got = set.add_tag(self.group, name, TagPayload::Built(self.root))?;
        debug_assert_eq!(got, id);
        set.record_conversion(self.group, name, got);
        Ok(got)
    }
}

impl Converted {
    /// Like [`Self::install`], but into the existing tag `id`'s slot: the
    /// tag keeps its datum and takes this group, name and tree. For maps
    /// whose tag index can't grow (`TagSet::from_map`), in the slot of a
    /// tag the map no longer uses.
    pub fn install_at(mut self, set: &mut TagSet, id: TagId, name: &str) -> Result<TagId> {
        if set.tag(id).is_none() {
            return Err(crate::models::Error::Invalid(format!(
                "no tag {:#x} to replace",
                id.0
            )));
        }
        let ids: Vec<RawId> = self
            .raw
            .drain(..)
            .map(|(bytes, align)| set.add_raw(bytes, align))
            .collect();
        rewrite(&mut self.root, &ids, id);
        set.add_group(self.group, None, None);
        set.slots[id.index()].name = Some(name.to_owned());
        let tag = set.tag_mut(id).expect("checked");
        tag.group = self.group;
        tag.payload = TagPayload::Built(self.root);
        set.record_conversion(self.group, name, id);
        Ok(id)
    }
}

fn rewrite(b: &mut Block, raw: &[RawId], id: TagId) {
    for f in &mut b.fixups {
        match &mut f.kind {
            FixupKind::Block { child, .. } | FixupKind::Data(child) | FixupKind::Pointer(child) => {
                rewrite(child, raw, id)
            }
            FixupKind::Raw(RawRef::Local(r)) => *r = raw[r.0],
            FixupKind::TagId(t) | FixupKind::TagRef(t) if *t == SELF_ID => *t = id,
            _ => {}
        }
    }
}

/// Little helpers for building blocks.
pub(crate) trait BlockExt {
    fn put_i16(&mut self, at: usize, v: i16) -> &mut Self;
    fn put_u8(&mut self, at: usize, v: u8) -> &mut Self;
    fn put_f32s(&mut self, at: usize, v: &[f32]) -> &mut Self;
}
impl BlockExt for Block {
    fn put_i16(&mut self, at: usize, v: i16) -> &mut Self {
        self.put_u16(at, v as u16)
    }
    fn put_u8(&mut self, at: usize, v: u8) -> &mut Self {
        self.bytes[at] = v;
        self
    }
    fn put_f32s(&mut self, at: usize, v: &[f32]) -> &mut Self {
        for (i, x) in v.iter().enumerate() {
            self.put_f32(at + 4 * i, *x);
        }
        self
    }
}

/// Concatenates fixed-size element blocks into one block-array child.
pub(crate) fn array(elements: Vec<Block>, stride: usize) -> Block {
    let mut out = Block::zeroed(elements.len() * stride);
    for (i, e) in elements.into_iter().enumerate() {
        assert_eq!(e.bytes.len(), stride);
        out.bytes[i * stride..(i + 1) * stride].copy_from_slice(&e.bytes);
        for mut f in e.fixups {
            f.offset += (i * stride) as u32;
            out.fixups.push(f);
        }
    }
    out
}
