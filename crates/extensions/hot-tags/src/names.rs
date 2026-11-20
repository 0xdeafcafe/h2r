//! Tag names. The game never loads the name table (live-telemetry's
//! `tags` module explains why), so names come from the map file on the
//! host: the host gives a [`NameSource`] that finds the loaded map's file,
//! and the table is used only when its scenario datum is the guest's.

use std::collections::HashMap;
use std::io::{Read, Seek};

use map::FourCC;
use runtime::Guest;

/// The loaded map's tag names, by full datum.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Names {
    pub scenario: u32,
    by_datum: HashMap<u32, (FourCC, String)>,
}

/// Finds the names of the map loaded in `guest` (the host reads its file).
pub type NameSource = Box<dyn Fn(&Guest) -> Option<Names> + Send>;

/// Folds a tag name for comparison: lower case, `/` as `\`.
pub fn fold(name: &str) -> String {
    name.to_ascii_lowercase().replace('/', "\\")
}

impl Names {
    /// From (datum, group, name) rows of the map whose scenario is `scenario`.
    pub fn new(scenario: u32, rows: impl IntoIterator<Item = (u32, FourCC, String)>) -> Names {
        Names {
            scenario,
            by_datum: rows.into_iter().map(|(d, g, n)| (d, (g, n))).collect(),
        }
    }

    /// From a map file.
    pub fn from_map<R: Read + Seek>(map: &map::Map<R>) -> Option<Names> {
        let scenario = map.scenario()?.id.0;
        Some(Names::new(
            scenario,
            map.tags()
                .iter()
                .filter_map(|t| Some((t.id.0, t.group, t.name.clone()?))),
        ))
    }

    pub fn name(&self, datum: u32) -> Option<&str> {
        self.by_datum.get(&datum).map(|(_, n)| n.as_str())
    }

    /// Tags named `name` (folded), of `group` when given.
    pub fn find(&self, name: &str, group: Option<FourCC>) -> Vec<(u32, FourCC)> {
        let want = fold(name);
        let mut out: Vec<(u32, FourCC)> = self
            .by_datum
            .iter()
            .filter(|(_, (g, n))| group.is_none_or(|w| w == *g) && fold(n) == want)
            .map(|(d, (g, _))| (*d, *g))
            .collect();
        out.sort();
        out
    }
}
