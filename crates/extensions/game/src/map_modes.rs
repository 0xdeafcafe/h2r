//! Which game types a map supports (docs/game/map-select.md, "One list,
//! with the modes each map supports"): Firefight and Siege, shown as tags
//! on Change Map's rows, listed first when the lobby plays that game type,
//! and the rule that picks a supported map when the game type changes.
//!
//! The model is pure. Each mode has one function over the data that says
//! it: [`firefight_support`] (a kit package exists or is built in, or the
//! map has a saved map variant with AI ground) and, in
//! `rules::gate::siege_objectives`, Siege's (the map has sites, a territory
//! or a flag stand). [`analyse`] runs both over a catalogue and gives the
//! [`View`] a picker reads.

use std::collections::{BTreeMap, BTreeSet};

use crate::lobby_start::{self, KIT_NAME_PREFIX};
use crate::lobby_tabs::Reason;
use crate::map_select::{Catalogue, Map};

/// The game type a lobby is about to play, as far as maps care.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Played {
    /// Any other game type: every map plays it.
    #[default]
    Other,
    Firefight,
    Siege,
}

/// The modes one map supports.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Support {
    pub firefight: bool,
    pub siege: bool,
}

impl Support {
    /// Whether the map plays `played`.
    pub fn allows(self, played: Played) -> bool {
        match played {
            Played::Other => true,
            Played::Firefight => self.firefight,
            Played::Siege => self.siege,
        }
    }
}

/// The kit package of the original map `base` (`Firefight – <name>`), if
/// one is installed. A kit package has no kit package of its own.
pub fn kit_package_of<'a>(cat: &'a Catalogue, base: &Map) -> Option<&'a Map> {
    if lobby_start::is_kit_map(base) {
        return None;
    }
    cat.all().find(|k| {
        k.name
            .strip_prefix(KIT_NAME_PREFIX)
            .is_some_and(|n| n.eq_ignore_ascii_case(&base.name))
    })
}

/// Whether `map` plays Firefight: it is a kit package, an installed kit
/// package names it, it is a map the game builds a kit for
/// ([`lobby_start::BUILTIN_KIT_MAPS`]), or it has a saved map variant with
/// AI ground (`ground`). With `wave` (a Firefight variant's prefix), the
/// set must take the map's base ([`lobby_start::supports_wave`],
/// [`lobby_start::supports_ground_wave`]).
pub fn firefight_support(cat: &Catalogue, map: &Map, ground: bool, wave: Option<[u8; 8]>) -> bool {
    if lobby_start::is_kit_map(map) {
        return wave.is_none_or(|w| lobby_start::supports_wave(map, w));
    }
    let own_kit = kit_package_of(cat, map).is_some_and(|k| wave.is_none_or(|w| lobby_start::supports_wave(k, w)));
    let built_in = lobby_start::BUILTIN_KIT_MAPS
        .iter()
        .any(|b| b.eq_ignore_ascii_case(&map.name))
        && lobby_start::supports_ground_wave(map, wave);
    own_kit || built_in || (ground && lobby_start::supports_ground_wave(map, wave))
}

/// What a picker knows about the catalogue's maps for one lobby.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct View {
    /// What the lobby plays.
    pub played: Played,
    /// By list id: the map's support (a kit package's, and an original map's
    /// own kit package's, the original's).
    support: BTreeMap<u32, Support>,
    /// An original map's id to its kit package's: what Change Map lists in
    /// a Firefight lobby, so the lobby's map carries the kit.
    kits: BTreeMap<u32, u32>,
    /// Kit packages listed under their original map instead of beside it.
    hidden: BTreeSet<u32>,
}

/// The [`View`] of `cat` for a lobby playing `played`. `ground` says
/// whether a map has a saved map variant with AI ground, `siege` whether it
/// has Siege's objectives (`false` when that cannot be told).
pub fn analyse(
    cat: &Catalogue,
    played: Played,
    wave: Option<[u8; 8]>,
    ground: &dyn Fn(&Map) -> bool,
    siege: &dyn Fn(&Map) -> bool,
) -> View {
    let mut view = View { played, ..View::default() };
    for map in cat.all() {
        let is_kit = lobby_start::is_kit_map(map);
        let has_base = is_kit && cat.all().any(|b| !lobby_start::is_kit_map(b)
            && kit_package_of(cat, b).is_some_and(|k| k.id == map.id));
        let support = Support {
            firefight: firefight_support(cat, map, !is_kit && ground(map), wave),
            siege: siege(map),
        };
        view.support.insert(map.id, support);
        if has_base {
            view.hidden.insert(map.id);
        }
        if let Some(kit) = kit_package_of(cat, map) {
            view.kits.insert(map.id, kit.id);
            view.support.entry(kit.id).or_insert(support);
        }
    }
    view
}

impl View {
    /// Whether the picker has anything to say: it was analysed for a lobby.
    pub fn is_empty(&self) -> bool {
        self.support.is_empty()
    }

    /// The list id Change Map puts on `map`'s row: its kit package's in a
    /// Firefight lobby when it has one, else its own.
    pub fn row_id(&self, map: u32) -> u32 {
        match (self.played, self.kits.get(&map)) {
            (Played::Firefight, Some(kit)) => *kit,
            _ => map,
        }
    }

    /// Whether the map row `id` is hidden under its original map.
    pub fn hides(&self, id: u32) -> bool {
        self.hidden.contains(&id)
    }

    /// The support of the map row `id`.
    pub fn support(&self, id: u32) -> Support {
        self.support.get(&id).copied().unwrap_or_default()
    }

    /// Whether the lobby can play on the map row `id`. A map nothing is known
    /// of is allowed: the lobby's own checks decide.
    pub fn allows(&self, id: u32) -> bool {
        self.played == Played::Other || self.support.get(&id).is_none_or(|s| s.allows(self.played))
    }

    /// `rows` with the maps the lobby plays first, each group in its order.
    pub fn first_supported<T>(&self, rows: Vec<T>, id: impl Fn(&T) -> u32) -> Vec<T> {
        let (yes, no): (Vec<T>, Vec<T>) = rows.into_iter().partition(|r| self.allows(id(r)));
        yes.into_iter().chain(no).collect()
    }

    /// The text of the row `id` named `name`: the tags of the modes it
    /// supports, or in a Firefight or Siege lobby what it lacks. `None` for
    /// a row with nothing to add.
    pub fn label(&self, id: u32, name: &str) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        label(name, self.support(id), self.played)
    }

    /// Why the lobby cannot play on the row `id`.
    pub fn refusal(&self, id: u32) -> Option<Reason> {
        refusal(self.support.get(&id).copied()?, self.played)
    }
}

/// The game type a lobby plays: Firefight in a Firefight lobby (or with a
/// Firefight variant, `lobby`: [`crate::map_select::variant_mode`]'s
/// answer), Siege with the Siege variant `record`, else anything.
pub fn played(lobby: lobby_start::Mode, record: Option<&variants::GameVariant>) -> Played {
    if lobby == lobby_start::Mode::Firefight {
        return Played::Firefight;
    }
    #[cfg(feature = "rules")]
    if record.is_some_and(crate::rules::gate::is_siege_record) {
        return Played::Siege;
    }
    let _ = record;
    Played::Other
}

/// Whether the map has Siege's objectives, as far as this machine can tell.
fn siege_in_game(map: &Map) -> bool {
    #[cfg(feature = "rules")]
    {
        crate::rules::gate::siege_supports(map.id).unwrap_or(false)
    }
    #[cfg(not(feature = "rules"))]
    {
        let _ = map;
        false
    }
}

/// [`analyse`] over the game's data: AI ground from the saved map variants
/// under `data`, Siege's objectives from the map files.
pub fn in_game(cat: &Catalogue, played: Played, wave: Option<[u8; 8]>, data: &std::path::Path) -> View {
    analyse(
        cat,
        played,
        wave,
        &|m| crate::lobby_mode::has_ground_variant(data, m),
        &siege_in_game,
    )
}

/// The tag a mode shows on a row.
pub const FIREFIGHT_TAG: &str = "Firefight";
pub const SIEGE_TAG: &str = "Siege";

/// A row's text: `name`, then the tags of what it supports (Firefight, then
/// Siege). In a lobby playing a mode the map lacks, `name` then what it
/// lacks instead.
pub fn label(name: &str, support: Support, played: Played) -> Option<String> {
    let lacks = match played {
        Played::Firefight if !support.firefight => Some("no Firefight kit"),
        Played::Siege if !support.siege => Some("no Siege sites"),
        _ => None,
    };
    if let Some(lacks) = lacks {
        return Some(format!("{name}  ({lacks})"));
    }
    let tags: Vec<&str> = [
        support.firefight.then_some(FIREFIGHT_TAG),
        support.siege.then_some(SIEGE_TAG),
    ]
    .into_iter()
    .flatten()
    .collect();
    (!tags.is_empty()).then(|| format!("{name}  {}", tags.join("  ")))
}

/// Why `support` does not play `played`; `None` when it does.
pub fn refusal(support: Support, played: Played) -> Option<Reason> {
    match played {
        Played::Firefight if !support.firefight => Some(Reason::MapNoFirefight),
        Played::Siege if !support.siege => Some(Reason::MapNoSiege),
        _ => None,
    }
}

/// The map a lobby moves to when its game type changes: `None` when
/// `current` plays it (or there is no map to go to), else `last` (the map
/// last used with this game type) when `rows` (id, plays it, in picker
/// order, supported first) has it as supported, else the first supported.
pub fn pick(rows: &[(u32, bool)], current: Option<u32>, last: Option<u32>) -> Option<u32> {
    let plays = |id: u32| rows.iter().find(|r| r.0 == id).is_some_and(|r| r.1);
    if current.is_some_and(plays) {
        return None;
    }
    last.filter(|&l| plays(l)).or_else(|| rows.iter().find(|r| r.1).map(|r| r.0))
}

/// The maps the picker's sample lists (`ui_kit::screens`, the
/// `map.picker.*` screens): the picker's rows for `played`, each with
/// its text, whether it is refused, and the line the picker says for it.
pub fn sample(played: Played) -> Vec<(String, bool, String)> {
    let cat = Catalogue::new([
        map(1, "Ascension", r"d:\maps\ascension"),
        map(2, "Coagulation", r"d:\maps\coagulation"),
        map(3, "Lockout", r"d:\maps\lockout"),
        map(4, "Midship", r"d:\maps\midship"),
        map(5, "Beaver Creek", r"d:\maps\beavercreek"),
        map(6, "Firefight \u{2013} Lockout", r"t:\$C\4d53006400020010\lockout"),
    ]);
    let view = analyse(&cat, played, None, &|m| m.name == "Beaver Creek", &|m| matches!(m.name.as_str(), "Lockout" | "Midship"));
    let cat = cat.with_modes(view);
    let view = cat.modes();
    cat.categories()
        .into_iter()
        .flat_map(|c| crate::map_select::list_for(&cat, crate::map_select::Mode::Maps(c), &|_| 0))
        .filter_map(|(id, _)| {
            let named = cat.all().find(|m| view.row_id(m.id) == id)?;
            let text = view.label(id, &named.name).unwrap_or_else(|| named.name.clone());
            let refused = view.refusal(id);
            let line = refused.map_or_else(
                || "Choose it to play on this map.".to_owned(),
                |r| r.text().to_owned(),
            );
            Some((text, refused.is_some(), line))
        })
        .collect()
}

fn map(id: u32, name: &str, path: &str) -> Map {
    Map { id, name: name.into(), sort: id as i32, path: path.into() }
}

/// The map last used with each game type (this session).
static LAST: std::sync::Mutex<[Option<u32>; 2]> = std::sync::Mutex::new([None; 2]);

fn slot(played: Played) -> Option<usize> {
    match played {
        Played::Other => None,
        Played::Firefight => Some(0),
        Played::Siege => Some(1),
    }
}

/// Notes that a lobby played `played` on the map `id`.
pub fn remember(played: Played, id: u32) {
    if let Some(i) = slot(played) {
        LAST.lock().unwrap_or_else(|e| e.into_inner())[i] = Some(id);
    }
}

/// The map last used with `played`.
pub fn last(played: Played) -> Option<u32> {
    LAST.lock().unwrap_or_else(|e| e.into_inner())[slot(played)?]
}

#[cfg(test)]
mod tests;
