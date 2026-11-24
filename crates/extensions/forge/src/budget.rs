//! What is left to add, by kind of thing (docs/game/forge-map-variants-ui.md,
//! "The budget"). One number has one source: `Session::budget_for` (`session/budget.rs`) gives
//! what is left of a block, the limit and which limit binds, and the HUD's
//! pill, the readout's guide line, the Map limits rows and a refused add's
//! sentence all read it.
//!
//! Two limits apply to an added entry: the session's object budget, for the
//! blocks whose entries make objects, and the engine's count for the block
//! (`map::scenario_layout`, `max_count`). The object pool (`forge.capacity`)
//! is the game's and is not in a session, so it is not a limit here.

use forge_doc::BlockKind;

/// The limit that binds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Binds {
    /// The session's object budget.
    Session,
    /// The engine's count for the block.
    Block(BlockKind),
}

/// What is left of the limit that binds first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    /// Entries that can still be added.
    pub left: u32,
    /// The binding limit's size.
    pub limit: u32,
    pub binds: Binds,
}

/// The block's entries as a player names them: singular and plural.
pub fn noun(block: BlockKind) -> (&'static str, &'static str) {
    match block {
        BlockKind::Scenery => ("scenery piece", "scenery pieces"),
        BlockKind::Vehicles => ("vehicle", "vehicles"),
        BlockKind::Equipment => ("equipment", "equipment"),
        BlockKind::Weapons => ("weapon", "weapons"),
        BlockKind::Machines => ("machine", "machines"),
        BlockKind::SoundScenery => ("sound scenery", "sound scenery"),
        BlockKind::Crates => ("crate", "crates"),
        BlockKind::StartingLocations => ("starting point", "starting points"),
        BlockKind::NetgameFlags => ("goal", "goals"),
        BlockKind::NetgameEquipment => ("item spawner", "item spawners"),
        BlockKind::StartingEquipment => ("starting equipment", "starting equipment"),
    }
}

impl Budget {
    /// What the limit counts: `object` for the session's, else the block's.
    pub fn singular(&self) -> &'static str {
        match self.binds {
            Binds::Session => "object",
            Binds::Block(block) => noun(block).0,
        }
    }

    pub fn plural(&self) -> &'static str {
        match self.binds {
            Binds::Session => "objects",
            Binds::Block(block) => noun(block).1,
        }
    }

    /// The share of the limit that is used, `0.0` to `1.0`.
    pub fn used(&self) -> f32 {
        if self.limit == 0 {
            1.0
        } else {
            1.0 - self.left.min(self.limit) as f32 / self.limit as f32
        }
    }

    /// The sentence of a refused add: `Item spawners are full (100).`
    pub fn full(&self) -> String {
        let mut what = self.plural().to_owned();
        if let Some(first) = what.get_mut(..1) {
            first.make_ascii_uppercase();
        }
        format!("{what} are full ({}).", self.limit)
    }
}
