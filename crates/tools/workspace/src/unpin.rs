//! [`MapUnpin`]: lays out the tags a decomposed map keeps pinned, so an
//! item can change them (the way `ui_kit::retail_edits` lays out a screen).
use std::collections::{BTreeSet, HashSet};
use std::io::{Read, Seek};

use map::build::{Block, TagPayload, TagSet};
use map::{Map, TagId};
use tags::schema::{Relocator, Schema};

use crate::build::Unpin;

/// The base map's relocator.
pub struct MapUnpin {
    relocator: Relocator<'static>,
    preserve: BTreeSet<TagId>,
}

impl MapUnpin {
    /// Read `map`'s metadata once. `map` is the file `TagSet::from_map`
    /// decomposed.
    pub fn new<R: Read + Seek>(map: &mut Map<R>) -> Result<Self, String> {
        let relocator = Relocator::new(Schema::builtin(), map)
            .map_err(|e| format!("reading the map's tags: {e}"))?;
        Ok(Self {
            relocator,
            preserve: BTreeSet::new(),
        })
    }

    /// Keep the old bytes of templates a later builder references by address.
    pub(crate) fn preserving(mut self, preserve: BTreeSet<TagId>) -> Self {
        self.preserve = preserve;
        self
    }
}

impl Unpin for MapUnpin {
    fn tree(&self, id: TagId) -> Result<Block, String> {
        if let Err(d) = self.relocator.check(id) {
            return Err(format!(
                "the tag schema cannot lay the tag out anew: {} (block path {:x?}, offset {:#x}, value {:#x})",
                d.refusal, d.path, d.offset, d.value
            ));
        }
        self.relocator
            .to_built_among(id, &HashSet::from([id]))
            .map_err(|r| format!("the tag schema cannot lay the tag out anew: {r}"))
    }

    fn relocate(&self, set: &mut TagSet, id: TagId) -> Result<(), String> {
        let tree = self.tree(id)?;
        let old = match set.tag(id).map(|t| &t.payload) {
            Some(TagPayload::Pinned { address, bytes }) => (*address, bytes.clone()),
            _ => return Err("the tag is not where the map had it".into()),
        };
        let others = self.relocator.pointed_into(id);
        // Also points the tags that share its bytes (index size 0) at it.
        self.relocator
            .relocate(set, &[id], false)
            .map_err(|e| format!("laying the tag out anew: {e}"))?;
        if !others.is_empty() || self.preserve.contains(&id) {
            // Another tag's array is in its old bytes: they stay where they
            // are, owned by no tag, so that pointer still reads them.
            set.pinned
                .as_mut()
                .ok_or("a decomposed map has its image")?
                .unowned
                .push(old);
        }
        set.tag_mut(id).ok_or("the tag left the set")?.payload = TagPayload::Built(tree);
        Ok(())
    }
}
