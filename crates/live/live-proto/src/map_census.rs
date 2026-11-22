//! A map's census, for the hopper gate (docs/game/rules-start-gate.md,
//! "Hoppers, System Link, offline and Theater"): what the map holds that a
//! rules script can require, which Live cannot read for itself since it
//! holds no map bytes. An admin writes it from their own copy of the map
//! (`h2 live maps census`) as a server-signed `MapCensusSet` event on the
//! map's [`census_stream`]; `PUT /v1/maps/{map}/census`.
//!
//! The key is the map id a hopper entry names (`hopper::Record::id`: 30 is
//! Zanzibar), not a package's `MapId`: a hopper entry carries the level
//! id. The census is the `rules` crate's own type, so the gate's check
//! (`rules::labels::check`) reads it as it reads any census.
use rules::labels::Census;
use serde::{Deserialize, Serialize};

/// `MapCensusSet`: an admin set (or cleared) a map's census (server-signed,
/// on [`census_stream`]). Data: [`CensusSet`].
pub const MAP_CENSUS_SET: &str = "MapCensusSet";

/// The longest label text or tag path.
pub const MAX_CENSUS_TEXT: usize = 300;
/// At most this many distinct labels, and members over all labels.
pub const MAX_CENSUS_LABELS: usize = 512;
pub const MAX_CENSUS_MEMBERS: usize = 20_000;
/// At most this many tags.
pub const MAX_CENSUS_TAGS: usize = 20_000;

/// The stream a map's census events are on.
pub fn census_stream(map_id: u32) -> String {
    format!("mapcensus-{map_id}")
}

/// `MapCensusSet` data. `census` None: the map's census is cleared.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CensusSet {
    pub map_id: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub census: Option<Census>,
    /// The admin's account (None: the operator's key).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<crate::Xuid>,
}

/// One hopper entry a census blocks (or whose script Live cannot find):
/// the answer of `PUT /v1/maps/{map}/census` lists them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockedEntry {
    /// The hopper's number, from 1, as the builder shows it.
    pub hopper: usize,
    /// The entry's number in the hopper, from 1.
    pub entry: usize,
    pub map_id: u32,
    pub problems: Vec<String>,
}

/// What `PUT /v1/maps/{map}/census` answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CensusAnswer {
    pub position: u64,
    pub map_id: u32,
    /// Whether a census is now set for the map.
    pub set: bool,
    /// The published hopper entries on this map that the census blocks.
    pub blocked: Vec<BlockedEntry>,
}

/// Checks a census against the limits above.
pub fn validate(c: &Census) -> Result<(), String> {
    let text_ok = |s: &str| {
        !s.is_empty() && s.chars().count() <= MAX_CENSUS_TEXT && !s.chars().any(char::is_control)
    };
    if c.labels.len() > MAX_CENSUS_LABELS {
        return Err(format!("at most {MAX_CENSUS_LABELS} labels"));
    }
    if c.labels.values().map(Vec::len).sum::<usize>() > MAX_CENSUS_MEMBERS {
        return Err(format!("at most {MAX_CENSUS_MEMBERS} labelled members"));
    }
    if c.tags.len() > MAX_CENSUS_TAGS {
        return Err(format!("at most {MAX_CENSUS_TAGS} tags"));
    }
    if let Some(t) = c.labels.keys().chain(c.tags.keys()).find(|t| !text_ok(t)) {
        return Err(format!(
            "{t:?}: a label or tag is 1 to {MAX_CENSUS_TEXT} characters on one line"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rules::labels::{Class, Member, ObjectType};

    fn census() -> Census {
        let mut c = Census::default();
        c.labels.insert(
            "goal".into(),
            vec![Member {
                class: Class::Volume,
                number: None,
                team: None,
            }],
        );
        c.tags.insert("weapons\\rifle".into(), ObjectType::Weapon);
        c
    }

    #[test]
    fn the_event_data_round_trips_and_a_clear_has_no_census() {
        let d = CensusSet {
            map_id: 30,
            census: Some(census()),
            by: None,
        };
        let v = serde_json::to_value(&d).unwrap();
        assert_eq!(serde_json::from_value::<CensusSet>(v).unwrap(), d);
        let clear = serde_json::json!({"map_id": 30});
        assert_eq!(
            serde_json::from_value::<CensusSet>(clear).unwrap().census,
            None
        );
    }

    #[test]
    fn a_census_is_bounded() {
        assert_eq!(validate(&census()), Ok(()));
        let mut c = census();
        c.labels.insert(String::new(), vec![]);
        assert!(validate(&c).is_err());
        let mut c = census();
        c.tags.insert("a\nb".into(), ObjectType::Vehicle);
        assert!(validate(&c).is_err());
    }

    #[test]
    fn the_stream_is_the_maps() {
        assert_eq!(census_stream(30), "mapcensus-30");
    }
}
