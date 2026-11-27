//! Which game types a map supports (docs/game/map-select.md, "One list,
//! with the modes each map supports"): Firefight and Siege, listed in
//! Change Map's pane under the map's description, the maps that play the
//! lobby's game type first, and the rule that picks a supported map when
//! the game type changes.
//!
//! The model is pure. Each mode has one function over the data that says
//! it: [`firefight_support`] (a kit package exists or is built in) and, in
//! `rules::gate::siege_objectives`, Siege's (the map has sites, a territory
//! or a flag stand). A saved map variant has a [`Support`] of its own
//! ([`Variant`]: AI ground plays Firefight, its census Siege), and a map's
//! row supports what the map or any of its variants does. [`analyse`] runs
//! them over a catalogue and gives the [`View`] a picker reads: a row's
//! pane ([`View::pane`]) and the page of a map with variants
//! ([`View::page`]).

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

    /// What either supports.
    pub fn union(self, other: Support) -> Support {
        Support {
            firefight: self.firefight || other.firefight,
            siege: self.siege || other.siege,
        }
    }
}

/// A saved map variant of a map that passes the store's checks, and the
/// modes it plays.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Variant {
    /// The file stem the store takes.
    pub file: String,
    pub name: String,
    /// Firefight when its document has AI ground, Siege when the map with
    /// its edits has Siege's objectives.
    pub support: Support,
}

/// One row of a map's page ([`View::page`]): the map itself, then each of
/// its variants.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageRow {
    /// The variant's file stem; `None` for the map itself.
    pub file: Option<String>,
    pub name: String,
    /// What this row plays on its own.
    pub support: Support,
}

/// The most variants a map's page lists (the map itself is its first row).
pub const PAGE_VARIANTS: usize = 15;

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
    /// By list id: what the map's row supports, the map itself or any of
    /// its variants (a kit package's, and an original map's own kit
    /// package's, the original's).
    support: BTreeMap<u32, Support>,
    /// By list id: what the map supports without a variant.
    own: BTreeMap<u32, Support>,
    /// By list id: the map's variants, by name. Maps with none are absent.
    variants: BTreeMap<u32, Vec<Variant>>,
    /// An original map's id to its kit package's: what Change Map lists in
    /// a Firefight lobby, so the lobby's map carries the kit.
    kits: BTreeMap<u32, u32>,
    /// Kit packages listed under their original map instead of beside it.
    hidden: BTreeSet<u32>,
}

impl View {
    /// A view that knows only what the lobby plays: no map is analysed.
    pub fn playing(played: Played) -> Self {
        View { played, ..Default::default() }
    }
}

/// The [`View`] of `cat` for a lobby playing `played`. `siege` says
/// whether a map has Siege's objectives (`false` when that cannot be
/// told), `variants` lists a map's usable saved variants with what each
/// plays (a kit package has none of its own). A variant's AI ground plays
/// Firefight only with a wave set the map takes
/// ([`lobby_start::supports_ground_wave`]).
pub fn analyse(
    cat: &Catalogue,
    played: Played,
    wave: Option<[u8; 8]>,
    siege: &dyn Fn(&Map) -> bool,
    variants: &dyn Fn(&Map) -> Vec<Variant>,
) -> View {
    let mut view = View { played, ..View::default() };
    for map in cat.all() {
        let is_kit = lobby_start::is_kit_map(map);
        let has_base = is_kit && cat.all().any(|b| !lobby_start::is_kit_map(b)
            && kit_package_of(cat, b).is_some_and(|k| k.id == map.id));
        let own = Support {
            firefight: firefight_support(cat, map, false, wave),
            siege: siege(map),
        };
        let mut listed = if is_kit { Vec::new() } else { variants(map) };
        let ground_wave = lobby_start::supports_ground_wave(map, wave);
        for v in &mut listed {
            v.support.firefight &= ground_wave;
        }
        listed.sort_by_key(|v| (v.name.to_lowercase(), v.file.clone()));
        listed.truncate(PAGE_VARIANTS);
        let support = listed.iter().fold(own, |all, v| all.union(v.support));
        let put = |view: &mut View, id: u32| {
            view.support.insert(id, support);
            view.own.insert(id, own);
            if !listed.is_empty() {
                view.variants.insert(id, listed.clone());
            }
        };
        // A kit package with its original map takes the original's support,
        // whichever of the two the catalogue lists first.
        if !has_base {
            put(&mut view, map.id);
        }
        if has_base {
            view.hidden.insert(map.id);
        }
        if let Some(kit) = kit_package_of(cat, map) {
            view.kits.insert(map.id, kit.id);
            put(&mut view, kit.id);
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

    /// What the map row `id` supports: the map itself or any variant.
    pub fn support(&self, id: u32) -> Support {
        self.support.get(&id).copied().unwrap_or_default()
    }

    /// What the map of row `id` supports without a variant.
    pub fn own(&self, id: u32) -> Support {
        self.own.get(&id).copied().unwrap_or_default()
    }

    /// The variants of the map row `id`, by name.
    pub fn variants(&self, id: u32) -> &[Variant] {
        self.variants.get(&id).map_or(&[], Vec::as_slice)
    }

    /// The page A on the map row `id` named `name` opens: the map itself,
    /// then each variant. Empty for a map without variants, which A picks.
    pub fn page(&self, id: u32, name: &str) -> Vec<PageRow> {
        let variants = self.variants(id);
        if variants.is_empty() {
            return Vec::new();
        }
        std::iter::once(PageRow { file: None, name: name.to_owned(), support: self.own(id) })
            .chain(variants.iter().map(|v| PageRow {
                file: Some(v.file.clone()),
                name: v.name.clone(),
                support: v.support,
            }))
            .collect()
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

    /// What the pane says under the description of the map row `id`: in a
    /// Firefight or Siege lobby what the row lacks, then the modes it
    /// supports, one a line, a mode only a variant gives with the variant's
    /// name. `None` for a row with nothing to say.
    pub fn pane(&self, id: u32) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        let (own, variants) = (self.own(id), self.variants(id));
        let givers = |plays: &dyn Fn(Support) -> bool| -> Vec<&str> {
            variants.iter().filter(|v| plays(v.support)).map(|v| v.name.as_str()).collect()
        };
        let modes = [
            mode_line(FIREFIGHT_TAG, own.firefight, &givers(&|s| s.firefight)),
            mode_line(SIEGE_TAG, own.siege, &givers(&|s| s.siege)),
        ];
        pane(lacks(self.support(id), self.played), modes.into_iter().flatten().collect())
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

/// The usable saved variants of `map` under `data`, each with its AI
/// ground and, as far as this machine can tell, Siege's objectives on the
/// map with its edits.
fn variants_in_game(data: &std::path::Path, map: &Map) -> Vec<Variant> {
    crate::lobby_mode::usable_variants(data, map)
        .into_iter()
        .map(|v| {
            #[cfg(feature = "rules")]
            let siege = crate::rules::gate::siege_supports_variant(map.id, &v.file, &format!("{:?}", v.id))
                .unwrap_or(false);
            #[cfg(not(feature = "rules"))]
            let siege = false;
            Variant { file: v.file, name: v.name, support: Support { firefight: v.ground, siege } }
        })
        .collect()
}

/// [`analyse`] over the game's data: Siege's objectives from the map
/// files, the variants from the saved map variants under `data`.
pub fn in_game(cat: &Catalogue, played: Played, wave: Option<[u8; 8]>, data: &std::path::Path) -> View {
    analyse(cat, played, wave, &siege_in_game, &|m| variants_in_game(data, m))
}

/// A mode's name in the pane's list.
pub const FIREFIGHT_TAG: &str = "Firefight";
pub const SIEGE_TAG: &str = "Siege";

/// The heading of the pane's list of modes.
pub const SUPPORTS: &str = "Supports";

/// What a lobby playing `played` is told a row with `support` lacks.
fn lacks(support: Support, played: Played) -> Option<&'static str> {
    match played {
        Played::Firefight if !support.firefight => Some("No Firefight kit or AI ground"),
        Played::Siege if !support.siege => Some("No Siege sites or flag stand"),
        _ => None,
    }
}

/// A mode's line in the pane: its name when the map plays it itself, else
/// with the variant that gives it (`Siege on Castle Keep`, and how many
/// more do). `None` when nothing plays it.
fn mode_line(mode: &str, own: bool, givers: &[&str]) -> Option<String> {
    match (own, givers) {
        (true, _) => Some(mode.to_owned()),
        (false, []) => None,
        (false, [one]) => Some(format!("{mode} on {one}")),
        (false, [first, rest @ ..]) => Some(format!("{mode} on {first} and {} more", rest.len())),
    }
}

/// The pane's lines as one text: what the row lacks, then [`SUPPORTS`] and
/// a mode a line. One space between words: the UI kit's spacing check
/// refuses doubled ones.
fn pane(lacks: Option<&str>, modes: Vec<String>) -> Option<String> {
    let mut lines: Vec<String> = lacks.map(str::to_owned).into_iter().collect();
    if !modes.is_empty() {
        lines.push(SUPPORTS.to_owned());
        lines.extend(modes);
    }
    (!lines.is_empty()).then(|| lines.join("\n"))
}

/// What the pane says for one row of a map's page ([`PageRow`]) in a lobby
/// playing `played`: what the row lacks, then the modes it plays.
pub fn page_pane(support: Support, played: Played) -> Option<String> {
    let modes = [
        mode_line(FIREFIGHT_TAG, support.firefight, &[]),
        mode_line(SIEGE_TAG, support.siege, &[]),
    ];
    pane(lacks(support, played), modes.into_iter().flatten().collect())
}

/// A map's description with its pane under it, a blank line between.
pub fn described(description: &str, pane: Option<&str>) -> String {
    match (description.trim(), pane) {
        (d, None) => d.to_owned(),
        ("", Some(p)) => p.to_owned(),
        (d, Some(p)) => format!("{d}\n\n{p}"),
    }
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

/// The sample catalogue of the `map.picker.*` screens and its view for
/// `played`: Lockout has a kit package and two variants (Castle Keep adds
/// Siege's sites, Lockout Towers AI ground), Midship has Siege's sites,
/// Beaver Creek a variant with AI ground.
fn sample_view(played: Played) -> Catalogue {
    let cat = Catalogue::new([
        map(1, "Ascension", r"d:\maps\ascension"),
        map(2, "Coagulation", r"d:\maps\coagulation"),
        map(3, "Lockout", r"d:\maps\lockout"),
        map(4, "Midship", r"d:\maps\midship"),
        map(5, "Beaver Creek", r"d:\maps\beavercreek"),
        map(6, "Firefight \u{2013} Lockout", r"t:\$C\4d53006400020010\lockout"),
    ]);
    let variant = |file: &str, name: &str, firefight, siege| Variant {
        file: file.into(),
        name: name.into(),
        support: Support { firefight, siege },
    };
    let view = analyse(&cat, played, None, &|m| m.name == "Midship", &|m| match m.name.as_str() {
        "Lockout" => vec![
            variant("castle-keep", "Castle Keep", false, true),
            variant("lockout-towers", "Lockout Towers", true, false),
        ],
        "Beaver Creek" => vec![variant("creek-nights", "Creek Nights", true, false)],
        _ => Vec::new(),
    });
    cat.with_modes(view)
}

/// What a sample map's description says (the retail slot's own, in the
/// game).
const SAMPLE_DESCRIPTION: &str = "Some believe this remote facility was once used to study the Flood.";

/// The maps the picker's sample lists (`ui_kit::screens`, the
/// `map.picker.*` screens): the picker's rows for `played`, each with
/// its name, whether it is refused, and what the pane says for it.
pub fn sample(played: Played) -> Vec<(String, bool, String)> {
    let cat = sample_view(played);
    let view = cat.modes();
    cat.categories()
        .into_iter()
        .flat_map(|c| crate::map_select::list_for(&cat, crate::map_select::Mode::Maps(c), &|_| 0))
        .filter_map(|(id, _)| {
            let named = cat.all().find(|m| view.row_id(m.id) == id)?;
            let pane = described(SAMPLE_DESCRIPTION, view.pane(id).as_deref());
            Some((named.name.clone(), view.refusal(id).is_some(), pane))
        })
        .collect()
}

/// The page of the sample's Lockout for `played` (the
/// `map.picker.variants` screen): the map's name, then each row's name,
/// whether it is refused, and what the pane says for it.
pub fn sample_page(played: Played) -> (String, Vec<(String, bool, String)>) {
    let cat = sample_view(played);
    let view = cat.modes();
    let lockout = cat.all().find(|m| m.name == "Lockout").expect("the sample has Lockout");
    let rows = view
        .page(view.row_id(lockout.id), &lockout.name)
        .into_iter()
        .map(|row| {
            let pane = described(SAMPLE_DESCRIPTION, page_pane(row.support, played).as_deref());
            (row.name, refusal(row.support, played).is_some(), pane)
        })
        .collect();
    (lockout.name.clone(), rows)
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
