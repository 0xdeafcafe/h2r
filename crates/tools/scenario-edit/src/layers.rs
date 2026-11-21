//! Viewport visibility shared by the renderer, picking, the editor and its API.
use std::collections::BTreeSet;

use crate::{BlockKind, layout};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum LayerId {
    Structure,
    Sky,
    Objects,
    Fog,
    Markers,
    Placement(BlockKind),
    Halo1,
    Live,
}

impl LayerId {
    /// Legend order. Every placement block has a row, even on an empty map.
    pub fn all() -> impl Iterator<Item = Self> {
        [Self::Structure, Self::Sky, Self::Objects, Self::Fog, Self::Markers]
            .into_iter()
            .chain(layout::BLOCKS.iter().map(|b| Self::Placement(b.kind)))
            .chain([Self::Halo1, Self::Live])
    }

    pub fn id(self) -> &'static str {
        use BlockKind::*;
        match self {
            Self::Structure => "structure",
            Self::Sky => "sky",
            Self::Objects => "objects",
            Self::Fog => "fog",
            Self::Markers => "markers",
            Self::Placement(kind) => match kind {
                Scenery => "scenery",
                Vehicles => "vehicles",
                Equipment => "equipment",
                Weapons => "weapons",
                Machines => "machines",
                SoundScenery => "sound_scenery",
                Crates => "crates",
                StartingLocations => "starting_locations",
                NetgameFlags => "netgame_flags",
                NetgameEquipment => "netgame_equipment",
                StartingEquipment => "starting_equipment",
            },
            Self::Halo1 => "halo1",
            Self::Live => "live",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Structure => "Structure",
            Self::Sky => "Sky",
            Self::Objects => "Objects",
            Self::Fog => "Fog",
            Self::Markers => "Placement markers",
            Self::Placement(kind) => layout::block(kind).label,
            Self::Halo1 => "Halo 1 overlay",
            Self::Live => "Live objects",
        }
    }

    pub fn group(self) -> &'static str {
        match self {
            Self::Structure | Self::Sky | Self::Objects | Self::Fog => "World",
            Self::Markers | Self::Placement(_) => "Placements",
            Self::Halo1 => "Reference",
            Self::Live => "Live",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    Art,
    Gameplay,
    Collision,
    Everything,
}

impl Preset {
    pub const ALL: [Self; 4] = [Self::Art, Self::Gameplay, Self::Collision, Self::Everything];

    pub fn id(self) -> &'static str {
        match self {
            Self::Art => "art",
            Self::Gameplay => "gameplay",
            Self::Collision => "collision",
            Self::Everything => "everything",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Art => "Art",
            Self::Gameplay => "Gameplay",
            Self::Collision => "Collision",
            Self::Everything => "Everything",
        }
    }

    fn shows(self, id: LayerId) -> bool {
        use LayerId::*;
        match self {
            Self::Art => matches!(id, Structure | Sky | Objects | Fog | Placement(_)),
            Self::Gameplay => matches!(id, Structure | Objects | Markers | Placement(_) | Live),
            Self::Collision => id == Structure,
            Self::Everything => true,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Layers {
    hidden: BTreeSet<LayerId>,
    pub xray: bool,
}

impl Layers {
    pub fn visible(&self, id: LayerId) -> bool {
        !self.hidden.contains(&id)
    }

    pub fn placement(&self, kind: BlockKind) -> bool {
        self.visible(LayerId::Placement(kind))
    }

    pub fn set(&mut self, id: LayerId, on: bool) {
        if on {
            self.hidden.remove(&id);
        } else {
            self.hidden.insert(id);
        }
    }

    pub fn apply(&mut self, preset: Preset) {
        self.hidden = LayerId::all().filter(|&id| !preset.shows(id)).collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legend_keys_are_unique_and_presets_replace_previous_toggles() {
        let ids: BTreeSet<_> = LayerId::all().map(LayerId::id).collect();
        assert_eq!(ids.len(), LayerId::all().count());
        let mut layers = Layers::default();
        layers.set(LayerId::Placement(BlockKind::Weapons), false);
        layers.apply(Preset::Art);
        assert!(layers.placement(BlockKind::Weapons));
        assert!(!layers.visible(LayerId::Markers));
        layers.apply(Preset::Gameplay);
        assert!(layers.visible(LayerId::Markers));
        assert!(!layers.visible(LayerId::Fog));
        layers.apply(Preset::Collision);
        assert!(layers.visible(LayerId::Structure));
        assert!(!layers.visible(LayerId::Objects));
        layers.apply(Preset::Everything);
        assert_eq!(layers, Layers::default());
    }
}
