//! `DefTable`: object definitions decoded once per map load
//! (docs/engine/sim-rewrite-objects.md section 4).
//!
//! An entry is built the first time an object of that definition is
//! touched, from the tag bytes resident in guest memory: the bytes the
//! guest itself would read, so it cannot disagree with them. Map patching
//! writes tags at load, before any object exists, so building lazily sees
//! the patched bytes. The table is derived state: it is dropped when the
//! epoch moves (map load, revert, restore, snapshot load), and dropping it
//! never changes a result.

use guest_layout::{Datum, Fault, Mem, ObjectDefinition, TagEntry, TagIndex};
use sim_core::Epoch;

/// What the object kernels read from a definition. Slices add fields as
/// their kernels need them (functions, change colours, attachments,
/// nodes).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Def {
    /// The definition's tag.
    pub tag: Datum,
    /// Definition flags (`+0x02`).
    pub flags: u16,
    pub bounding_radius: f32,
    pub bounding_offset: [f32; 3],
    pub dynamic_scale: f32,
    pub dynamic_offset: [f32; 3],
}

impl Def {
    /// Decode the definition whose data is at `addr`.
    pub fn read<M: Mem + ?Sized>(m: &M, tag: Datum, addr: u32) -> Result<Def, Fault> {
        let d = ObjectDefinition::at(addr);
        Ok(Def {
            tag,
            flags: d.flags(m)?,
            bounding_radius: d.bounding_radius(m)?,
            bounding_offset: d.bounding_offset(m)?,
            dynamic_scale: d.dynamic_scale(m)?,
            dynamic_offset: d.dynamic_offset(m)?,
        })
    }
}

/// The data address of `tag`: its slot is below the index's count, and
/// the entry's datum is the whole tag (salt included). `None` otherwise,
/// and for an entry with no data. The group is not checked; callers pass
/// a definition the object names.
pub fn tag_data<M: Mem + ?Sized>(m: &M, tag: Datum) -> Result<Option<u32>, Fault> {
    let slot = tag.index() as i16;
    if tag.is_none() || slot < 0 {
        return Ok(None);
    }
    let (Some(index), Some(entries)) = (
        guest_layout::global::TAG_INDEX.pointer(m)?,
        guest_layout::global::TAG_ENTRIES.pointer(m)?,
    ) else {
        return Ok(None);
    };
    if i32::from(slot) >= TagIndex::at(index).tag_count(m)? {
        return Ok(None);
    }
    let entry = TagEntry::at(entries.wrapping_add((slot as u32) * TagEntry::SIZE));
    if entry.datum(m)? != tag {
        return Ok(None);
    }
    let data = entry.address(m)?;
    Ok((data != 0).then_some(data))
}

/// Decoded definitions by tag index, valid for one epoch.
#[derive(Default)]
pub struct DefTable {
    /// 0: not decoded; else the index in `defs` plus one.
    by_tag_index: Vec<u32>,
    defs: Vec<Def>,
    epoch: Option<u64>,
}

impl DefTable {
    pub const fn new() -> DefTable {
        DefTable {
            by_tag_index: Vec::new(),
            defs: Vec::new(),
            epoch: None,
        }
    }

    /// Decoded definitions held now.
    pub fn len(&self) -> usize {
        self.defs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.defs.is_empty()
    }

    /// Drop every entry.
    pub fn clear(&mut self) {
        self.by_tag_index.clear();
        self.defs.clear();
    }

    /// The definition `tag` names, decoded now if this epoch has not seen
    /// it. `None` when the tag does not resolve. A stale salt on a decoded
    /// index resolves again rather than returning the other tag's entry.
    pub fn get<M: Mem + ?Sized>(&mut self, m: &M, epoch: &Epoch, tag: Datum) -> Result<Option<&Def>, Fault> {
        let now = epoch.get();
        if self.epoch != Some(now) {
            self.clear();
            self.epoch = Some(now);
        }
        if tag.is_none() {
            return Ok(None);
        }
        let i = usize::from(tag.index());
        if let Some(&n) = self.by_tag_index.get(i)
            && n != 0
            && self.defs[n as usize - 1].tag == tag
        {
            return Ok(Some(&self.defs[n as usize - 1]));
        }
        let Some(addr) = tag_data(m, tag)? else {
            return Ok(None);
        };
        let def = Def::read(m, tag, addr)?;
        if self.by_tag_index.len() <= i {
            self.by_tag_index.resize(i + 1, 0);
        }
        self.defs.push(def);
        self.by_tag_index[i] = self.defs.len() as u32;
        Ok(self.defs.last())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::Pool;
    

    #[test]
    fn a_definition_decodes_once_per_epoch_from_guest_bytes() {
        let epoch = Epoch::new();
        let mut pool = Pool::new();
        let (tag, data) = pool.tag(3, 0xe001, 0.5, [1.0, 2.0, 3.0]);
        data.set(&mut pool.ram, ObjectDefinition::FLAGS, 0x1000).unwrap();
        data.set(&mut pool.ram, ObjectDefinition::DYNAMIC_SCALE, 2.0).unwrap();
        let mut t = DefTable::new();
        let d = *t.get(&pool.ram, &epoch, tag).unwrap().unwrap();
        assert_eq!(d.tag, tag);
        assert_eq!((d.flags, d.bounding_radius, d.bounding_offset), (0x1000, 0.5, [1.0, 2.0, 3.0]));
        assert_eq!(d.dynamic_scale, 2.0);
        assert_eq!(t.len(), 1);

        // Within the epoch the decoded entry stands, even if the bytes
        // move (tags are immutable after load; this shows it is cached).
        data.set(&mut pool.ram, ObjectDefinition::BOUNDING_RADIUS, 9.0).unwrap();
        assert_eq!(t.get(&pool.ram, &epoch, tag).unwrap().unwrap().bounding_radius, 0.5);
        assert_eq!(t.len(), 1);

        // A new epoch decodes again from what is in memory.
        epoch.bump();
        assert_eq!(t.get(&pool.ram, &epoch, tag).unwrap().unwrap().bounding_radius, 9.0);
        assert_eq!(t.len(), 1);
    }

    #[test]
    fn tags_that_do_not_resolve_are_none() {
        let epoch = Epoch::new();
        let mut pool = Pool::new();
        let (tag, _) = pool.tag(5, 0xe002, 1.0, [0.0; 3]);
        let mut t = DefTable::new();
        assert!(t.get(&pool.ram, &epoch, Datum::NONE).unwrap().is_none());
        // A stale salt.
        let stale = Datum(tag.raw() ^ 0x0001_0000);
        assert!(t.get(&pool.ram, &epoch, stale).unwrap().is_none());
        // Past the index's count.
        pool.set_tag_count(5);
        assert!(t.get(&pool.ram, &epoch, tag).unwrap().is_none());
        pool.set_tag_count(6);
        assert!(t.get(&pool.ram, &epoch, tag).unwrap().is_some());
        // An entry with no data.
        let (empty, _) = pool.tag(6, 0xe003, 1.0, [0.0; 3]);
        let entry = TagEntry::at(crate::synth::TAG_ENTRIES + 6 * TagEntry::SIZE);
        entry.set(&mut pool.ram, TagEntry::ADDRESS, 0).unwrap();
        pool.set_tag_count(64);
        assert!(t.get(&pool.ram, &epoch, empty).unwrap().is_none());
        // A negative index (the high bit of the low half).
        assert!(t.get(&pool.ram, &epoch, Datum(0xe004_8001)).unwrap().is_none());
    }

    #[test]
    fn a_reused_index_with_a_new_salt_is_decoded_again() {
        let epoch = Epoch::new();
        let mut pool = Pool::new();
        let (old, _) = pool.tag(2, 0xe010, 1.0, [0.0; 3]);
        let mut t = DefTable::new();
        assert!(t.get(&pool.ram, &epoch, old).unwrap().is_some());
        let (new, _) = pool.tag(2, 0xe011, 4.0, [0.0; 3]);
        assert_eq!(t.get(&pool.ram, &epoch, new).unwrap().unwrap().bounding_radius, 4.0);
        assert!(t.get(&pool.ram, &epoch, old).unwrap().is_none());
    }
}
