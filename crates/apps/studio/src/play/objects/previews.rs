//! A respawn receipt links an otherwise loose, salted object to its document placement.
//! Keep the stable document key across swap-removals until a later receipt retires it.

use std::collections::BTreeMap;

use agent_proto::live::Object;
use scenario_edit::BlockKind;

use super::Key;
use crate::app::App;

#[derive(Debug, Default)]
pub(in crate::play) struct Previews {
    identity: Option<(u64, u64, u32)>,
    // The slot in the receipt is a hint. The document may have moved the row.
    by_object: BTreeMap<u32, (Key, usize)>,
    by_placement: BTreeMap<Key, u32>,
    revision: u64,
}

impl Previews {
    pub(in crate::play) fn clear(&mut self) {
        if !self.by_object.is_empty() {
            self.revision += 1;
        }
        self.identity = None;
        self.by_object.clear();
        self.by_placement.clear();
    }

    pub(in crate::play) fn revision(&self) -> u64 {
        self.revision
    }

    pub(in crate::play) fn set(
        &mut self,
        document: u64,
        epoch: u64,
        checksum: u32,
        key: Key,
        preview: Option<(u32, usize)>,
    ) {
        let identity = Some((document, epoch, checksum));
        if self.identity != identity {
            self.clear();
            self.identity = identity;
        }
        if self.by_placement.get(&key).copied() == preview.map(|(id, _)| id) {
            return;
        }
        if let Some(old) = self.by_placement.remove(&key) {
            self.by_object.remove(&old);
        }
        if let Some((id, slot)) = preview {
            if let Some((old, _)) = self.by_object.insert(id, (key, slot)) {
                self.by_placement.remove(&old);
            }
            self.by_placement.insert(key, id);
        }
        self.revision += 1;
    }

    fn current(&self, app: &App) -> bool {
        let Some((document, epoch, checksum)) = self.identity else {
            return false;
        };
        document == app.session.world_rev()
            && app.pairing.shown.as_ref().is_some_and(|s| !s.stale && s.epoch == epoch)
            && app.pairing.stream_map["checksum"].as_u64() == Some(u64::from(checksum))
            && crate::play::select::matching(app)
    }

    pub(in crate::play) fn placement(
        &self,
        app: &App,
        object: &Object,
    ) -> Option<(BlockKind, usize)> {
        let &(key, hint) = self.by_object.get(&object.id)?;
        if !self.current(app)
            || scenario_edit::layout::block(key.block).object_type != Some(i16::from(object.kind))
        {
            return None;
        }
        let block = app.session.scenario()?.block(key.block);
        let index = if block.placement_id(hint) == Some(key.id) {
            hint
        } else {
            (0..block.len()).find(|&i| block.placement_id(i) == Some(key.id))?
        };
        Some((key.block, index))
    }

    pub(in crate::play) fn object(
        &self,
        app: &App,
        block: BlockKind,
        index: usize,
    ) -> Option<u32> {
        let key = super::key(app.session.scenario()?, block, index)?;
        let id = self.by_placement.get(&key)?;
        self.current(app).then_some(*id)
    }
}
