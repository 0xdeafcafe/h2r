//! The effects family: `effe`, `prt3`, `pmov`, `jpt!`, `cont`, `deca`, `ligh`,
//! `lens`, plus the shared "function" (curve) data they embed.
//!
//! Hand-written fixed codecs (not in `schema.rs`), self-contained like
//! [`crate::sound`]. Layout evidence and provenance: `docs/formats/effects.md`.
//!
//! Every top-level tag type implements [`EffectTag`]: it is decoded from the
//! virtual address of the tag's root record (not from the tag's recorded
//! payload, which is empty for tags whose data is shared with another tag) and
//! reports each outgoing tag reference so callers can validate them.
pub mod contrail;
pub mod damage;
pub mod decal;
pub mod effect;
pub mod function;
pub mod h1_attachment;
pub mod h1_glow;
pub mod lens;
pub mod light;
pub mod mem;
pub mod movement;
pub mod particle;
pub use mem::{Budget, Ctx, Memory, Property, Rec, SliceMemory, TagRef};

use crate::{Error, Result};
use map::{FourCC, Map, TagId};
use std::io::{Read, Seek};

/// A decoded tag of the effects family.
pub trait EffectTag: Sized + std::fmt::Debug {
    /// Tag class, in readable order (`b"effe"`).
    const GROUP: [u8; 4];
    /// Decode the tag whose root record starts at virtual `address`.
    /// Child blocks are followed through `ctx` and charged to its budget.
    fn decode_at(ctx: &mut Ctx<'_>, address: u32) -> Result<Self>;
    /// Every tag reference the tag holds (including nested ones), with a
    /// readable field path, in file order. Null references are included.
    fn references(&self) -> Vec<(String, TagRef)>;
    /// Every embedded function blob (properties and bare function data) with a
    /// readable path, for validating the shared function format. Default: none.
    fn functions(&self) -> Vec<(String, &[u8])> {
        Vec::new()
    }
}

/// Decode one tag of a map. Rejects a tag of the wrong class.
pub fn read<T: EffectTag, R: Read + Seek>(map: &mut Map<R>, id: TagId) -> Result<T> {
    let tag = map.tag(id)?;
    if tag.group != FourCC(T::GROUP) {
        return Err(Error::UnsupportedGroup(tag.group));
    }
    let address = tag.location.ok_or(map::Error::NoTagData)?.address;
    T::decode_at(&mut Ctx::new(map), address)
}
