//! `Session::budget_for` (docs/game/forge-map-variants-ui.md, "The budget").

use forge_doc::{BlockKind, Edit, Placement};
use map::scenario_layout as layout;

use super::Session;
use crate::{Binds, Budget};

impl Session {
    /// The entries of `block` as the document leaves them: the map's, less
    /// the removed, plus the added.
    fn block_count(&self, block: BlockKind) -> u32 {
        let base = self.limits.base_counts.get(&block).copied().unwrap_or(0);
        let mut added = 0u32;
        let mut removed = 0u32;
        for op in &self.doc.ops {
            if let Edit::Placement(p) = &op.edit
                && p.block() == block
            {
                match p {
                    Placement::Add { .. } => added += 1,
                    Placement::Remove { .. } => removed += 1,
                    Placement::Set { .. } => {}
                }
            }
        }
        if let Ok(resolved) = forge_doc::resolve(&self.doc.ops, &self.limits.base_counts) {
            removed += resolved
                .lowered
                .values()
                .filter(|p| matches!(p, Placement::Remove { .. }) && p.block() == block)
                .count() as u32;
        }
        (base + added).saturating_sub(removed)
    }

    /// What is left to add to `block`, and the limit that binds: the
    /// session's object budget (for a block whose entries make objects)
    /// or the engine's count for the block, whichever has less left. A
    /// block with neither limit has none. A tie binds the block, whose
    /// name is the more exact. Moving, turning and setting a field of an
    /// own entry costs nothing; removing one gives its row back.
    pub fn budget_for(&self, block: BlockKind) -> Option<Budget> {
        let def = layout::block(block);
        let engine = def.max_count.map(|max| {
            let max = max as u32;
            Budget {
                left: max.saturating_sub(self.block_count(block)),
                limit: max,
                binds: Binds::Block(block),
            }
        });
        let session = def.palette.is_some().then(|| {
            let limit = self.budget as u32;
            Budget {
                left: limit.saturating_sub(self.objects.len() as u32),
                limit,
                binds: Binds::Session,
            }
        });
        match (engine, session) {
            (Some(e), Some(s)) => Some(if s.left < e.left { s } else { e }),
            (e, s) => e.or(s),
        }
    }
}
