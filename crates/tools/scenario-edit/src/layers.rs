//! Viewport visibility shared by the renderer, picking, the editor and its API.
use std::collections::BTreeSet;

use crate::volumes::VolumeKind;
use crate::{BlockKind, layout};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum LayerId {
    Structure,
    Sky,
    Objects,
    Fog,
    Markers,
    Placement(BlockKind),
    SpawnPoints,
    Flags,
    NetgameItems,
    /// Trigger volumes drawn as each kind ([`crate::volumes`]).
    Volume(VolumeKind),
    /// Every structure BSP's collision surfaces
    /// ([`crate::collision_view`]).
    LevelCollision,
    /// The collision surfaces no drawn surface covers: the invisible
    /// walls ([`crate::collision_view::classify`]).
    CollisionOnly,
    /// Instanced geometry's collision, placed
    /// ([`crate::collision_view::instance_surfaces`]).
    InstanceCollision,
    /// Each placed object's `coll` BSPs and `phmo` shapes as wire shapes
    /// ([`crate::object_collision`]).
    ObjectCollision,
    /// Each structure BSP's own pathfinding sectors and links, Halo 2's AI
    /// navigation ([`crate::nav_view::LevelPathfinding`]).
    Pathfinding,
    /// The bots' nav mesh: polygons and links by kind
    /// ([`crate::nav_view::BotNav`]).
    BotNav,
    /// The superbounce and crouch spots the bots use, by state
    /// ([`crate::nav_view::BotSpot`]).
    BotSpots,
    Halo1,
    Live,
}

impl LayerId {
    /// The collision layers, hidden until shown ([`Layers::default`]).
    pub const COLLISION: [Self; 4] = [
        Self::LevelCollision,
        Self::CollisionOnly,
        Self::InstanceCollision,
        Self::ObjectCollision,
    ];

    /// The navigation layers, hidden until shown ([`Layers::default`]).
    pub const NAVIGATION: [Self; 3] = [Self::Pathfinding, Self::BotNav, Self::BotSpots];

    /// Legend order. Every placement block has a row, even on an empty map.
    pub fn all() -> impl Iterator<Item = Self> {
        [
            Self::Structure,
            Self::Sky,
            Self::Objects,
            Self::Fog,
            Self::Markers,
        ]
        .into_iter()
        .chain(layout::BLOCKS.iter().map(|b| Self::Placement(b.kind)))
        .chain([Self::SpawnPoints, Self::Flags, Self::NetgameItems])
        .chain(VolumeKind::ALL.map(Self::Volume))
        .chain(Self::COLLISION)
        .chain(Self::NAVIGATION)
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
            Self::SpawnPoints => "spawn_points",
            Self::Flags => "flags",
            Self::NetgameItems => "netgame_items",
            Self::Volume(kind) => match kind {
                VolumeKind::Trigger => "trigger_volumes",
                VolumeKind::Kill => "kill_volumes",
                VolumeKind::BspSwitch => "bsp_switches",
            },
            Self::LevelCollision => "level_collision",
            Self::CollisionOnly => "collision_only",
            Self::InstanceCollision => "instance_collision",
            Self::ObjectCollision => "object_collision",
            Self::Pathfinding => "pathfinding",
            Self::BotNav => "bot_nav",
            Self::BotSpots => "bot_spots",
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
            Self::SpawnPoints => "Spawn points",
            Self::Flags => "Netgame flags",
            Self::NetgameItems => "Netgame equipment",
            Self::Volume(VolumeKind::Trigger) => "Trigger volumes",
            Self::Volume(VolumeKind::Kill) => "Kill volumes",
            Self::Volume(VolumeKind::BspSwitch) => "BSP switches",
            Self::LevelCollision => "Level collision",
            Self::CollisionOnly => "Collision only",
            Self::InstanceCollision => "Instance collision",
            Self::ObjectCollision => "Object collision",
            Self::Pathfinding => "AI pathfinding",
            Self::BotNav => "Bot nav mesh",
            Self::BotSpots => "Bot superbounce spots",
            Self::Halo1 => "Halo 1 overlay",
            Self::Live => "Live objects",
        }
    }

    pub fn gameplay(kind: BlockKind) -> Option<Self> {
        match kind {
            BlockKind::StartingLocations => Some(Self::SpawnPoints),
            BlockKind::NetgameFlags => Some(Self::Flags),
            BlockKind::NetgameEquipment => Some(Self::NetgameItems),
            _ => None,
        }
    }

    pub fn group(self) -> &'static str {
        match self {
            Self::Structure | Self::Sky | Self::Objects | Self::Fog => "World",
            Self::Markers | Self::Placement(_) => "Placements",
            Self::SpawnPoints | Self::Flags | Self::NetgameItems => "Gameplay",
            Self::Volume(_) => "Volumes",
            Self::LevelCollision
            | Self::CollisionOnly
            | Self::InstanceCollision
            | Self::ObjectCollision => "Collision",
            Self::Pathfinding | Self::BotNav | Self::BotSpots => "Navigation",
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
    Navigation,
    Everything,
}

impl Preset {
    pub const ALL: [Self; 5] = [
        Self::Art,
        Self::Gameplay,
        Self::Collision,
        Self::Navigation,
        Self::Everything,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Self::Art => "art",
            Self::Gameplay => "gameplay",
            Self::Collision => "collision",
            Self::Navigation => "navigation",
            Self::Everything => "everything",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Art => "Art",
            Self::Gameplay => "Gameplay",
            Self::Collision => "Collision",
            Self::Navigation => "Navigation",
            Self::Everything => "Everything",
        }
    }

    fn shows(self, id: LayerId) -> bool {
        use LayerId::*;
        match self {
            Self::Art => matches!(
                id,
                Structure | Sky | Objects | Fog | Placement(_) | NetgameItems
            ),
            Self::Gameplay => matches!(
                id,
                Structure
                    | Objects
                    | Markers
                    | Placement(_)
                    | Live
                    | SpawnPoints
                    | Flags
                    | NetgameItems
                    | Volume(_)
            ),
            // Out of bounds is kill volumes and collision.
            Self::Collision => matches!(
                id,
                Structure
                    | Volume(VolumeKind::Kill)
                    | LevelCollision
                    | CollisionOnly
                    | InstanceCollision
                    | ObjectCollision
            ),
            // Where the AI and the bots walk, over the level and the
            // collision only walls they walk beside.
            Self::Navigation => matches!(
                id,
                Structure | CollisionOnly | Pathfinding | BotNav | BotSpots
            ),
            Self::Everything => true,
        }
    }
}

/// The viewport's shown layers. Everything starts shown except the
/// collision and navigation layers, which cover the picture: the
/// Collision, Navigation and Everything presets show them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layers {
    hidden: BTreeSet<LayerId>,
    pub xray: bool,
}

impl Default for Layers {
    fn default() -> Self {
        Self {
            hidden: LayerId::COLLISION
                .into_iter()
                .chain(LayerId::NAVIGATION)
                .collect(),
            xray: false,
        }
    }
}

impl Layers {
    pub fn visible(&self, id: LayerId) -> bool {
        !self.hidden.contains(&id)
    }

    pub fn placement(&self, kind: BlockKind) -> bool {
        self.visible(LayerId::Placement(kind))
            && LayerId::gameplay(kind).is_none_or(|id| self.visible(id))
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
        assert!(!layers.visible(LayerId::Volume(VolumeKind::Kill)));
        assert!(!layers.visible(LayerId::Markers));
        layers.apply(Preset::Gameplay);
        assert!(layers.visible(LayerId::Markers));
        assert!(!layers.visible(LayerId::Fog));
        assert!(layers.visible(LayerId::Volume(VolumeKind::Trigger)));
        assert!(!layers.visible(LayerId::CollisionOnly));
        layers.apply(Preset::Collision);
        assert!(layers.visible(LayerId::LevelCollision));
        assert!(LayerId::COLLISION.iter().all(|&id| layers.visible(id)));
        assert!(layers.visible(LayerId::Structure));
        assert!(layers.visible(LayerId::Volume(VolumeKind::Kill)));
        assert!(!layers.visible(LayerId::Volume(VolumeKind::Trigger)));
        assert!(!layers.visible(LayerId::Objects));
        layers.apply(Preset::Everything);
        assert!(LayerId::all().all(|id| layers.visible(id)));
        layers.apply(Preset::Navigation);
        assert!(LayerId::NAVIGATION.iter().all(|&id| layers.visible(id)));
        assert!(layers.visible(LayerId::Structure));
        assert!(layers.visible(LayerId::CollisionOnly));
        assert!(!layers.visible(LayerId::LevelCollision));
        assert!(!layers.visible(LayerId::Objects));
        // The default hides only the collision and navigation layers.
        let default = Layers::default();
        let hidden: Vec<_> = LayerId::all().filter(|&id| !default.visible(id)).collect();
        let want: Vec<_> = LayerId::COLLISION
            .into_iter()
            .chain(LayerId::NAVIGATION)
            .collect();
        assert_eq!(hidden, want);
    }
}
