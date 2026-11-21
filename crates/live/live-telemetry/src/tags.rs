//! Tag names for frame fields: a player's species from their biped's tag
//! and the held weapon's tag file stem.
//!
//! The tag-name table is (likely) never loaded into guest memory: the cache
//! loader (`0x00122a52`) reads the index and metadata, and no instruction
//! refers to the header's tag-name fields. So names come
//! from the loaded map's file on the host, found by the boot through its
//! VFS. The file's tag index holds the same salted datums as the guest's,
//! and a table is only used after its scenario datum matched the guest's
//! (`TAG_INDEX_PTR +0x0c`), so a wrong file sends no names rather than wrong
//! ones.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read, Seek};
use std::path::PathBuf;

/// Where a map file sits on the host: a whole file, or a region inside a
/// disc image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapFile {
    pub path: PathBuf,
    pub offset: u64,
    pub len: u64,
}

/// Finds the file of the map the game loaded, by its cache short name (the
/// name is `[A-Za-z0-9_-]{1,64}`, checked before the call).
pub type MapLocator = Box<dyn Fn(&str) -> Option<MapFile> + Send + Sync>;

/// The package identity and optional base name, from the cache short
/// name. An alias can identify a mod while reports group it by base.
pub type MapIdLocator =
    Box<dyn Fn(&str) -> Option<(live_proto::maps::MapId, Option<String>)> + Send + Sync>;

/// The playlist of the game starting, asked at each multiplayer start; the
/// boot answers it with `live_hle::Hle::playlist`. It runs with guest
/// memory held, so it must not wait.
pub type PlaylistLocator = Box<dyn Fn() -> Option<crate::session::Playlist> + Send + Sync>;

/// The starting multiplayer game's own Live advertisement, read from the local HLE.
pub type HostedSessionLocator = Box<dyn Fn() -> Option<String> + Send + Sync>;

/// Names the Clankers a starting multiplayer game runs with, and the server
/// bot roster if one is in force (docs/game/bots.md §6, stage 6c): the boot
/// answers it from the Clankers driver and the local and phantom books. It runs with guest memory held, so it
/// must not wait.
pub type BotsLocator = Box<dyn Fn() -> Option<crate::session::Bots> + Send + Sync>;

const BIPED: [u8; 4] = *b"bipd";
const WEAPON: [u8; 4] = *b"weap";
const VEHICLE: [u8; 4] = *b"vehi";
const MACHINE: [u8; 4] = *b"mach";
const DAMAGE_EFFECT: [u8; 4] = *b"jpt!";
const PROJECTILE: [u8; 4] = *b"proj";
const EQUIPMENT: [u8; 4] = *b"eqip";
const KEPT: &[[u8; 4]] = &[
    BIPED,
    WEAPON,
    VEHICLE,
    MACHINE,
    DAMAGE_EFFECT,
    PROJECTILE,
    EQUIPMENT,
    *b"garb",
    *b"scen",
    *b"ctrl",
    *b"lifi",
    *b"ssce",
    *b"bloc",
    *b"crea",
];

/// Names of a map's object and damage-effect tags
/// by full salted datum, as the frame ids made from them: worked out once
/// when the table is made, not for every entity of every frame.
#[derive(Debug, Clone, PartialEq)]
pub struct TagNames {
    scenario: u32,
    names: HashMap<u32, Named>,
    /// Animation graphs (`jmad`) by datum, for the animation units play.
    graphs: HashMap<u32, Graph>,
}

/// An animation graph's animation names (by index; `None` for one the
/// frames can't name) and its inheritance list's graphs.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Graph {
    pub names: Vec<Option<String>>,
    pub parents: Vec<u32>,
}

/// A kept tag's group and the frame ids of its name.
#[derive(Debug, Clone, PartialEq)]
struct Named {
    group: [u8; 4],
    /// A biped's species ([`species_of`]); any other tag's file stem
    /// ([`stem_of`]).
    id: Option<String>,
    /// A biped's directory ([`dir_of`]: an AI biped's species).
    dir: Option<String>,
}

impl TagNames {
    /// A table for the map whose scenario tag is `scenario`, from
    /// (datum, group, name) rows. Only object and damage-effect rows are kept.
    pub fn new(scenario: u32, rows: impl IntoIterator<Item = (u32, [u8; 4], String)>) -> Self {
        Self {
            scenario,
            names: rows
                .into_iter()
                .filter(|(_, group, _)| KEPT.contains(group))
                .map(|(datum, group, name)| {
                    let named = if group == BIPED {
                        Named {
                            group,
                            id: species_of(&name),
                            dir: dir_of(&name),
                        }
                    } else {
                        Named {
                            group,
                            id: stem_of(&name),
                            dir: None,
                        }
                    };
                    (datum, named)
                })
                .collect(),
            graphs: HashMap::new(),
        }
    }

    /// The table with these animation graphs, by datum.
    pub fn with_graphs(mut self, graphs: impl IntoIterator<Item = (u32, Graph)>) -> Self {
        self.graphs.extend(graphs);
        self
    }

    /// Every `jmad` of the map: its animations' names (from the map's
    /// string table) and the graphs it inherits from. A graph that doesn't
    /// decode is left out (its units send no `anim`).
    fn graphs_of<R: Read + Seek>(map: &mut map::Map<R>) -> Vec<(u32, Graph)> {
        let ids: Vec<map::TagId> = map
            .tags()
            .iter()
            .filter(|t| t.group.0 == *b"jmad")
            .map(|t| t.id)
            .collect();
        let strings = map.strings().to_vec();
        ids.into_iter()
            .filter_map(|id| {
                let graph = tags::anim::AnimationGraph::read(map, id).ok()?;
                let names = graph
                    .animations
                    .iter()
                    .map(|a| {
                        a.name
                            .resolve(&strings)
                            .ok()
                            .flatten()
                            .filter(|n| live_proto::game::valid_anim_name(n))
                            .map(str::to_owned)
                    })
                    .collect();
                let parents = graph.inheritance.iter().map(|i| i.graph.id.0).collect();
                Some((id.0, Graph { names, parents }))
            })
            .collect()
    }

    pub fn from_map<R: Read + Seek>(map: &mut map::Map<R>) -> Option<Self> {
        let scenario = map.scenario()?.id.0;
        let names = Self::new(
            scenario,
            map.tags()
                .iter()
                .filter_map(|tag| Some((tag.id.0, tag.group.0, tag.name.clone()?))),
        );
        Some(names.with_graphs(Self::graphs_of(map)))
    }

    pub fn load(file: &MapFile) -> Result<Self, String> {
        let reader = File::open(&file.path).map_err(|e| format!("{}: {e}", file.path.display()))?;
        let mut map = map::Map::from_region(BufReader::new(reader), file.offset, file.len)
            .map_err(|e| format!("{}: {e}", file.path.display()))?;
        Self::from_map(&mut map).ok_or_else(|| format!("{}: no scenario tag", file.path.display()))
    }

    /// Whether this table is for the map whose scenario datum the guest's
    /// tag index holds.
    pub fn is_for(&self, guest_scenario: u32) -> bool {
        self.scenario == guest_scenario
    }

    /// Resolve definitions with this map's verified name table before publishing a film sample.
    pub fn name_film_scene(&self, scene: &mut live_proto::film::objects::Scene) {
        for object in &mut scene.objects {
            object.name = object
                .definition
                .and_then(|tag| self.names.get(&tag))
                .and_then(|tag| tag.id.clone());
        }
    }

    fn named(&self, datum: u32, group: [u8; 4]) -> Option<&Named> {
        self.names.get(&datum).filter(|n| n.group == group)
    }

    /// The species of the biped whose definition is `datum`.
    pub fn species(&self, datum: u32) -> Option<String> {
        self.named(datum, BIPED)?.id.clone()
    }

    /// The file stem of the weapon whose definition is `datum`.
    pub fn weapon(&self, datum: u32) -> Option<String> {
        self.named(datum, WEAPON)?.id.clone()
    }

    /// The file stem of the vehicle whose definition is `datum`.
    pub fn vehicle(&self, datum: u32) -> Option<String> {
        self.named(datum, VEHICLE)?.id.clone()
    }

    /// The file stem of the machine whose definition is `datum`.
    pub fn machine(&self, datum: u32) -> Option<String> {
        self.named(datum, MACHINE)?.id.clone()
    }

    /// The file stem of the damage effect (`jpt!`) `datum`.
    /// The file stem of the projectile whose definition is `datum`.
    pub fn projectile(&self, datum: u32) -> Option<String> {
        self.named(datum, PROJECTILE)?.id.clone()
    }

    /// The file stem of the equipment whose definition is `datum`.
    pub fn equipment(&self, datum: u32) -> Option<String> {
        self.named(datum, EQUIPMENT)?.id.clone()
    }

    pub fn damage_effect(&self, datum: u32) -> Option<String> {
        self.named(datum, DAMAGE_EFFECT)?.id.clone()
    }

    /// The name of the animation a unit plays: its graph's, or an inherited
    /// graph's when the channel says so (`combat:rifle:any:idle`).
    pub fn animation(&self, a: &crate::event::Animation) -> Option<String> {
        let own = self.graphs.get(&a.graph)?;
        let graph = match usize::try_from(a.inherited) {
            Ok(i) => self.graphs.get(own.parents.get(i)?)?,
            Err(_) => own,
        };
        graph.names.get(usize::try_from(a.index).ok()?)?.clone()
    }

    /// An AI biped's species: the directory of its biped tag
    /// (`objects\characters\grunt\grunt` -> `grunt`).
    pub fn actor_species(&self, datum: u32) -> Option<String> {
        self.named(datum, BIPED)?.dir.clone()
    }
}

/// The directory holding a tag, lower case.
pub fn dir_of(name: &str) -> Option<String> {
    let mut parts = components(name).rev();
    parts.next()?;
    model_id(parts.next()?)
}

/// A frame id, `[a-z0-9_]{1,32}`, from a tag path component; `None` when
/// the component has anything else in it.
fn model_id(component: &str) -> Option<String> {
    let id = component.to_ascii_lowercase();
    live_proto::game::valid_model_id(&id).then_some(id)
}

fn components(name: &str) -> impl DoubleEndedIterator<Item = &str> {
    name.split(['\\', '/']).filter(|c| !c.is_empty())
}

/// `"spartan"` for the Master Chief's bipeds, `"elite"` for the Elite's and
/// the Arbiter's, else the biped's directory (`objects\characters\<x>\...`)
/// as a pass-through id.
pub fn species_of(name: &str) -> Option<String> {
    let lower = name.to_ascii_lowercase();
    let dirs: Vec<&str> = components(&lower).collect();
    let has = |word: &str| dirs.iter().any(|c| c.contains(word));
    if has("masterchief") || has("spartan") {
        return Some("spartan".into());
    }
    if has("elite") || has("dervish") {
        return Some("elite".into());
    }
    // The directory that holds the biped tag, so `..\brute\brute` -> brute.
    dir_of(name)
}

/// The last component of a tag path, lower case.
pub fn stem_of(name: &str) -> Option<String> {
    model_id(components(name).next_back()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn species_from_biped_names() {
        for (name, species) in [
            (
                "objects\\characters\\masterchief\\masterchief_mp",
                "spartan",
            ),
            ("objects\\characters\\masterchief\\masterchief", "spartan"),
            ("objects\\characters\\elite\\elite_mp", "elite"),
            ("objects\\characters\\dervish\\dervish", "elite"),
            ("objects\\characters\\brute\\brute", "brute"),
        ] {
            assert_eq!(species_of(name).as_deref(), Some(species), "{name}");
        }
        assert_eq!(species_of("bare"), None, "no directory to name it by");
        assert_eq!(species_of("objects\\odd name\\x"), None);
    }

    #[test]
    fn weapon_stems_are_short_lower_case_ids() {
        assert_eq!(
            stem_of("objects\\weapons\\rifle\\battle_rifle\\battle_rifle").as_deref(),
            Some("battle_rifle")
        );
        assert_eq!(
            stem_of("objects\\weapons\\melee\\Energy_Blade\\Energy_Blade").as_deref(),
            Some("energy_blade")
        );
        // Every multiplayer stem the models lane extracts passes through as is.
        for stem in [
            "magnum",
            "needler",
            "plasma_pistol",
            "battle_rifle",
            "covenant_carbine",
            "plasma_rifle",
            "shotgun",
            "smg",
            "sniper_rifle",
            "flak_cannon",
            "rocket_launcher",
            "brute_shot",
            "beam_rifle",
            "brute_plasma_rifle",
            "energy_blade",
            "ball",
            "flag",
            "assault_bomb",
        ] {
            let name = format!("objects\\weapons\\any\\{stem}\\{stem}");
            assert_eq!(stem_of(&name).as_deref(), Some(stem));
        }
        assert_eq!(stem_of("objects\\weapons\\x\\has space"), None);
        assert_eq!(stem_of(&format!("w\\{}", "a".repeat(33))), None);
        assert_eq!(stem_of(""), None);
    }

    #[test]
    fn animations_are_named_through_the_graph_or_its_parents() {
        use crate::event::Animation;
        let names = TagNames::new(1, []).with_graphs([
            (
                0xe000_0010,
                Graph {
                    names: vec![Some("combat:rifle:any:idle".into()), None],
                    parents: vec![0xe000_0020],
                },
            ),
            (
                0xe000_0020,
                Graph {
                    names: vec![Some("a".into()), Some("combat:unarmed:any:idle".into())],
                    parents: vec![],
                },
            ),
        ]);
        let at = |graph, inherited, index| Animation {
            graph,
            inherited,
            index,
            frame: 0.0,
        };
        assert_eq!(
            names.animation(&at(0xe000_0010, -1, 0)).as_deref(),
            Some("combat:rifle:any:idle")
        );
        assert_eq!(
            names.animation(&at(0xe000_0010, 0, 1)).as_deref(),
            Some("combat:unarmed:any:idle")
        );
        assert_eq!(names.animation(&at(0xe000_0010, -1, 1)), None, "unnamed");
        assert_eq!(
            names.animation(&at(0xe000_0010, 1, 0)),
            None,
            "no such parent"
        );
        assert_eq!(names.animation(&at(0xe000_0010, -1, 9)), None);
        assert_eq!(
            names.animation(&at(0xe000_0099, -1, 0)),
            None,
            "unknown graph"
        );
    }

    #[test]
    fn vehicle_kinds_and_actor_species() {
        let names = TagNames::new(
            1,
            [
                (
                    10,
                    VEHICLE,
                    "objects\\vehicles\\warthog\\warthog".to_string(),
                ),
                (11, BIPED, "objects\\characters\\bugger\\bugger".to_string()),
                (
                    12,
                    BIPED,
                    "objects\\characters\\elite\\elite_specops".to_string(),
                ),
                (
                    13,
                    BIPED,
                    "objects\\characters\\FloodCombat_Human\\floodcombat_human".to_string(),
                ),
            ],
        );
        assert_eq!(names.vehicle(10).as_deref(), Some("warthog"));
        assert_eq!(names.vehicle(11), None, "a biped is not a vehicle");
        assert_eq!(names.actor_species(11).as_deref(), Some("bugger"));
        assert_eq!(names.actor_species(12).as_deref(), Some("elite"));
        assert_eq!(
            names.actor_species(13).as_deref(),
            Some("floodcombat_human")
        );
        assert_eq!(names.actor_species(10), None);
        assert_eq!(dir_of("bare"), None);
    }

    #[test]
    fn names_are_looked_up_by_datum_and_group() {
        let names = TagNames::new(
            0xe000_0001,
            [
                (
                    0xe100_0010,
                    BIPED,
                    "objects\\characters\\elite\\elite_mp".to_string(),
                ),
                (
                    0xe100_0011,
                    WEAPON,
                    "objects\\weapons\\rifle\\smg\\smg".to_string(),
                ),
                (
                    0xe100_0012,
                    *b"vehi",
                    "objects\\vehicles\\warthog\\warthog".to_string(),
                ),
            ],
        );
        assert!(names.is_for(0xe000_0001));
        assert!(!names.is_for(0xe001_0001));
        assert_eq!(names.species(0xe100_0010).as_deref(), Some("elite"));
        assert_eq!(names.weapon(0xe100_0011).as_deref(), Some("smg"));
        // Wrong group, stale salt, other groups: nothing.
        assert_eq!(names.species(0xe100_0011), None);
        assert_eq!(names.weapon(0xe100_0010), None);
        assert_eq!(names.weapon(0xe200_0011), None);
        assert_eq!(names.weapon(0xe100_0012), None);
    }
}
