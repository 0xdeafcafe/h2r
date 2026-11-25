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
    /// What headless bot matches measured on the map
    /// (docs/editor/playtest.md, "What is measured").
    Playtest(PlaytestLayer),
    Halo1,
    Live,
}

/// The playtest overlays, one a measure's picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PlaytestLayer {
    /// Where killers stood.
    Kills,
    /// Where players fell.
    Deaths,
    /// Each starting location by how fair it played, and the spawn kills.
    Spawns,
    /// Time per place.
    Traffic,
    /// Crossings between places, with their direction.
    Flow,
    /// The polygons no player entered.
    Unvisited,
    /// The longest and the commonest fighting lines.
    Sightlines,
    /// Each equipment placement sized by use; never-picked ones ringed.
    WeaponUse,
    /// Where bots stuck, and the islands no start reaches.
    Stuck,
}

impl PlaytestLayer {
    pub const ALL: [Self; 9] = [
        Self::Kills,
        Self::Deaths,
        Self::Spawns,
        Self::Traffic,
        Self::Flow,
        Self::Unvisited,
        Self::Sightlines,
        Self::WeaponUse,
        Self::Stuck,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Self::Kills => "playtest_kills",
            Self::Deaths => "playtest_deaths",
            Self::Spawns => "playtest_spawns",
            Self::Traffic => "playtest_traffic",
            Self::Flow => "playtest_flow",
            Self::Unvisited => "playtest_unvisited",
            Self::Sightlines => "playtest_sightlines",
            Self::WeaponUse => "playtest_weapon_use",
            Self::Stuck => "playtest_stuck",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Kills => "Kills",
            Self::Deaths => "Deaths",
            Self::Spawns => "Spawn fairness",
            Self::Traffic => "Traffic",
            Self::Flow => "Flow",
            Self::Unvisited => "Never entered",
            Self::Sightlines => "Sightlines",
            Self::WeaponUse => "Weapon use",
            Self::Stuck => "Stuck spots",
        }
    }
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

    /// The playtest layers, hidden until shown ([`Layers::default`]).
    pub const PLAYTEST: [Self; 9] = [
        Self::Playtest(PlaytestLayer::Kills),
        Self::Playtest(PlaytestLayer::Deaths),
        Self::Playtest(PlaytestLayer::Spawns),
        Self::Playtest(PlaytestLayer::Traffic),
        Self::Playtest(PlaytestLayer::Flow),
        Self::Playtest(PlaytestLayer::Unvisited),
        Self::Playtest(PlaytestLayer::Sightlines),
        Self::Playtest(PlaytestLayer::WeaponUse),
        Self::Playtest(PlaytestLayer::Stuck),
    ];

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
        .chain(Self::PLAYTEST)
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
            Self::Playtest(layer) => layer.id(),
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
            Self::Playtest(layer) => layer.label(),
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
            Self::Playtest(_) => "Playtest",
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
    Playtest,
    Everything,
}

impl Preset {
    pub const ALL: [Self; 6] = [
        Self::Art,
        Self::Gameplay,
        Self::Collision,
        Self::Navigation,
        Self::Playtest,
        Self::Everything,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Self::Art => "art",
            Self::Gameplay => "gameplay",
            Self::Collision => "collision",
            Self::Navigation => "navigation",
            Self::Playtest => "playtest",
            Self::Everything => "everything",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Art => "Art",
            Self::Gameplay => "Gameplay",
            Self::Collision => "Collision",
            Self::Navigation => "Navigation",
            Self::Playtest => "Playtest",
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
            // The level and the bots' mesh the results sit on.
            Self::Playtest => matches!(id, Structure | BotNav | Playtest(_)),
            Self::Everything => true,
        }
    }
}

/// The viewport's shown layers. Everything starts shown except the
/// collision, navigation and playtest layers, which cover the picture: the
/// Collision, Navigation, Playtest and Everything presets show them.
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
                .chain(LayerId::PLAYTEST)
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
            .chain(LayerId::PLAYTEST)
            .collect();
        assert_eq!(hidden, want);
        // The playtest preset shows the nine layers over the level and the mesh.
        layers.apply(Preset::Playtest);
        assert!(LayerId::PLAYTEST.iter().all(|&id| layers.visible(id)));
        assert!(layers.visible(LayerId::Structure) && layers.visible(LayerId::BotNav));
        assert!(!layers.visible(LayerId::Objects) && !layers.visible(LayerId::BotSpots));
        assert_eq!(LayerId::PLAYTEST.len(), PlaytestLayer::ALL.len());
    }
}
