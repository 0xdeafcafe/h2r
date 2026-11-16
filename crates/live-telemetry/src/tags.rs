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

const BIPED: [u8; 4] = *b"bipd";
const WEAPON: [u8; 4] = *b"weap";

/// Names of a map's biped and weapon tags by full salted datum.
#[derive(Debug, Clone, PartialEq)]
pub struct TagNames {
    scenario: u32,
    names: HashMap<u32, ([u8; 4], String)>,
}

impl TagNames {
    /// A table for the map whose scenario tag is `scenario`, from
    /// (datum, group, name) rows. Only biped and weapon rows are kept.
    pub fn new(scenario: u32, rows: impl IntoIterator<Item = (u32, [u8; 4], String)>) -> Self {
        Self {
            scenario,
            names: rows
                .into_iter()
                .filter(|(_, group, _)| [BIPED, WEAPON].contains(group))
                .map(|(datum, group, name)| (datum, (group, name)))
                .collect(),
        }
    }

    pub fn from_map<R: Read + Seek>(map: &map::Map<R>) -> Option<Self> {
        let scenario = map.scenario()?.id.0;
        Some(Self::new(
            scenario,
            map.tags()
                .iter()
                .filter_map(|tag| Some((tag.id.0, tag.group.0, tag.name.clone()?))),
        ))
    }

    pub fn load(file: &MapFile) -> Result<Self, String> {
        let reader = File::open(&file.path).map_err(|e| format!("{}: {e}", file.path.display()))?;
        let map = map::Map::from_region(BufReader::new(reader), file.offset, file.len)
            .map_err(|e| format!("{}: {e}", file.path.display()))?;
        Self::from_map(&map).ok_or_else(|| format!("{}: no scenario tag", file.path.display()))
    }

    /// Whether this table is for the map whose scenario datum the guest's
    /// tag index holds.
    pub fn is_for(&self, guest_scenario: u32) -> bool {
        self.scenario == guest_scenario
    }

    fn name(&self, datum: u32, group: [u8; 4]) -> Option<&str> {
        self.names
            .get(&datum)
            .filter(|(g, _)| *g == group)
            .map(|(_, name)| name.as_str())
    }

    /// The species of the biped whose definition is `datum`.
    pub fn species(&self, datum: u32) -> Option<String> {
        species_of(self.name(datum, BIPED)?)
    }

    /// The file stem of the weapon whose definition is `datum`.
    pub fn weapon(&self, datum: u32) -> Option<String> {
        stem_of(self.name(datum, WEAPON)?)
    }
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
    let parent = dirs.len().checked_sub(2).map(|i| dirs[i])?;
    model_id(parent)
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
            stem_of("objects\\weapons\\melee\\Energy_Sword\\Energy_Sword").as_deref(),
            Some("energy_sword")
        );
        assert_eq!(stem_of("objects\\weapons\\x\\has space"), None);
        assert_eq!(stem_of(&format!("w\\{}", "a".repeat(33))), None);
        assert_eq!(stem_of(""), None);
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
