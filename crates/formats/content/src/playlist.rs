//! A hopper fragment: the playlists a `playlist` item of a package adds to
//! the default hopper (docs/editor/studio-redesign.md, "Contents";
//! docs/live/live-matchmaking.md, "6.8 The default set").
//!
//! A fragment is TOML: named lists of maps, then `[[playlist]]` tables with
//! their `[[playlist.entry]]` rows. A map is its level-list name or its
//! number; a variant is the key of a matchmaking script of the same set.
//! The fragment holds no bytes of a hopper file: the server turns it into
//! descriptors and entries (`live_proto::hopper`).
use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

/// A parsed fragment, as written.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fragment {
    /// Named map lists an entry's `maps` can use.
    #[serde(default)]
    pub maps: BTreeMap<String, Vec<MapRef>>,
    #[serde(default, rename = "playlist")]
    pub playlists: Vec<Playlist>,
}

/// A map by its level-list name or its map id.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum MapRef {
    Id(u32),
    Name(String),
}

/// One playlist: its descriptor's fields and its entries.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Playlist {
    /// The playlist id (`+0x08` of the descriptor), unique in a hopper.
    pub id: u32,
    pub title: String,
    pub description: String,
    pub rules: Rules,
    pub base_wait_ms: u32,
    pub search_timeout_ms: u32,
    /// A Clankermatch playlist's mark.
    #[serde(default)]
    pub clankers: Option<Clankers>,
    /// Plays the entries of the playlist with this id, in its fragment.
    #[serde(default)]
    pub entries_of: Option<u32>,
    #[serde(default, rename = "entry")]
    pub entries: Vec<Entry>,
}

/// A playlist's clanker mark (`live_proto::hopper::clankers::Clankers`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Clankers {
    #[serde(default)]
    pub ranked: bool,
    #[serde(default)]
    pub ranks_as: Option<u32>,
}

/// The descriptor's type and its rules (`live_proto::hopper::layout::Rules`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Rules {
    FreeForAll {
        ranked: bool,
        players: [u32; 2],
    },
    UnrankedTeams {
        teams: u32,
        team_size: [u32; 2],
        #[serde(default)]
        uneven: bool,
    },
    RankedTeams {
        teams: u32,
        team_size: [u32; 2],
        #[serde(default)]
        max_difference: u32,
        party: [u32; 2],
        #[serde(default)]
        party_as_team: Option<[u32; 2]>,
    },
    Clan {
        teams: u32,
        team_size: [u32; 2],
        #[serde(default)]
        max_difference: u32,
    },
}

/// One group of entries: a variant on some maps, at one weight.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    /// The key of a matchmaking script.
    pub variant: String,
    pub maps: EntryMaps,
    pub weight: u32,
}

/// An entry's maps: the name of a list of the fragment, or the maps.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum EntryMaps {
    List(Vec<MapRef>),
    Named(String),
}

/// One row of a playlist, with its list expanded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub variant: String,
    pub map: MapRef,
    pub weight: u32,
}

/// Why a fragment is refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Toml(String),
    #[error("playlist id {0} appears twice")]
    DuplicateId(u32),
    #[error("playlist {id}: no map list named {name:?}")]
    NoList { id: u32, name: String },
    #[error("playlist {id}: entries_of {of} names no playlist of this fragment")]
    NoSource { id: u32, of: u32 },
    #[error("playlist {id}: entries_of {of} is itself a copy, or has entries of its own beside it")]
    BadSource { id: u32, of: u32 },
    #[error("playlist {id} has no entries")]
    Empty { id: u32 },
    #[error("playlist {id}: an entry on no maps")]
    NoMaps { id: u32 },
}

impl Fragment {
    /// Reads a fragment and checks what needs no hopper: ids are unique,
    /// every named list and `entries_of` exists, every playlist has rows.
    pub fn parse(text: &str) -> Result<Fragment, Error> {
        let f: Fragment = toml::from_str(text).map_err(|e| Error::Toml(e.to_string()))?;
        let mut seen = BTreeSet::new();
        for p in &f.playlists {
            if !seen.insert(p.id) {
                return Err(Error::DuplicateId(p.id));
            }
        }
        for p in &f.playlists {
            f.rows(p)?;
        }
        Ok(f)
    }

    /// The playlist with this id.
    pub fn playlist(&self, id: u32) -> Option<&Playlist> {
        self.playlists.iter().find(|p| p.id == id)
    }

    /// `p`'s rows in order: each entry's maps in order, a named list
    /// expanded, and a playlist with `entries_of` the rows of its source.
    pub fn rows(&self, p: &Playlist) -> Result<Vec<Row>, Error> {
        if let Some(of) = p.entries_of {
            let source = self.playlist(of).ok_or(Error::NoSource { id: p.id, of })?;
            if !p.entries.is_empty() || source.entries_of.is_some() {
                return Err(Error::BadSource { id: p.id, of });
            }
            return self.rows(source);
        }
        let mut rows = Vec::new();
        for e in &p.entries {
            let maps: &[MapRef] = match &e.maps {
                EntryMaps::List(l) => l,
                EntryMaps::Named(name) => self.maps.get(name).ok_or_else(|| Error::NoList {
                    id: p.id,
                    name: name.clone(),
                })?,
            };
            if maps.is_empty() {
                return Err(Error::NoMaps { id: p.id });
            }
            for m in maps {
                rows.push(Row {
                    variant: e.variant.clone(),
                    map: m.clone(),
                    weight: e.weight,
                });
            }
        }
        if rows.is_empty() {
            return Err(Error::Empty { id: p.id });
        }
        Ok(rows)
    }

    /// The matchmaking script keys the fragment names, once each.
    pub fn variants(&self) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        for p in &self.playlists {
            for e in &p.entries {
                out.insert(e.variant.clone());
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWO: &str = r#"
[maps]
small = ["Alpha", 20]

[[playlist]]
id = 1
title = "One"
description = "First."
rules = { kind = "free-for-all", ranked = true, players = [2, 8] }
base_wait_ms = 30000
search_timeout_ms = 150000

[[playlist.entry]]
variant = "a"
maps = "small"
weight = 10

[[playlist.entry]]
variant = "b"
maps = ["Gamma"]
weight = 3

[[playlist]]
id = 2
title = "Two"
description = "Second."
rules = { kind = "unranked-teams", teams = 2, team_size = [1, 4] }
base_wait_ms = 30000
search_timeout_ms = 150000
clankers = { ranked = true }
entries_of = 1
"#;

    #[test]
    fn rows_expand_lists_and_copies_in_order() {
        let f = Fragment::parse(TWO).unwrap();
        let rows = f.rows(f.playlist(1).unwrap()).unwrap();
        let shown: Vec<_> = rows
            .iter()
            .map(|r| (r.variant.as_str(), r.map.clone(), r.weight))
            .collect();
        assert_eq!(
            shown,
            [
                ("a", MapRef::Name("Alpha".into()), 10),
                ("a", MapRef::Id(20), 10),
                ("b", MapRef::Name("Gamma".into()), 3),
            ]
        );
        assert_eq!(f.rows(f.playlist(2).unwrap()).unwrap(), rows);
        assert_eq!(
            f.playlist(2).unwrap().clankers,
            Some(Clankers {
                ranked: true,
                ranks_as: None
            })
        );
        assert_eq!(
            f.playlist(2).unwrap().rules,
            Rules::UnrankedTeams {
                teams: 2,
                team_size: [1, 4],
                uneven: false
            }
        );
        assert_eq!(f.variants().into_iter().collect::<Vec<_>>(), ["a", "b"]);
    }

    #[test]
    fn a_fragment_is_refused_by_name() {
        let edit = |from: &str, to: &str| Fragment::parse(&TWO.replace(from, to)).unwrap_err();
        assert_eq!(edit("id = 2", "id = 1"), Error::DuplicateId(1));
        assert_eq!(
            edit("maps = \"small\"", "maps = \"large\""),
            Error::NoList {
                id: 1,
                name: "large".into()
            }
        );
        assert_eq!(
            edit("entries_of = 1", "entries_of = 9"),
            Error::NoSource { id: 2, of: 9 }
        );
        assert_eq!(
            edit("entries_of = 1", "entries_of = 2"),
            Error::BadSource { id: 2, of: 2 }
        );
        assert!(matches!(
            edit("weight = 3", "weight = 3\nextra = 1"),
            Error::Toml(_)
        ));
        assert!(matches!(
            edit("\"unranked-teams\"", "\"nine\""),
            Error::Toml(_)
        ));
        let empty = TWO.replace("entries_of = 1", "");
        assert_eq!(Fragment::parse(&empty).unwrap_err(), Error::Empty { id: 2 });
    }
}
