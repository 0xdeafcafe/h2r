//! The multiplayer map select by category (docs/game/map-select.md): Game
//! Setup's Change Map opens a list of categories (Halo 2, Map Pack, Mods,
//! Halo: CE; an empty one is left out), and a category opens its maps,
//! sorted. A on a map makes it the lobby's map and goes back to Game Setup;
//! B on the maps comes back to the categories, B there to Game Setup.
//!
//! Forge and Custom Games open the installed map catalogue. A Firefight
//! variant opens its compatible kit maps directly. Script variants apply
//! the lobby start gate's map requirements to the catalogue.
//!
//! - The model is pure: [`category_of`] reads a map's category from the
//!   path the game loads it from, [`Catalogue`] groups and sorts.
//! - In the game both steps are the retail Select Map dialog (screen
//!   `0xce`), with its pictures and descriptions ([`MapSelect`]): first in
//!   category mode (a row a category, standing on its first map, labelled
//!   with the category), then a second Select Map with the category's
//!   maps. The dialog's channel replaces its active screen, so from a
//!   map list B or Back rebuilds the categories with the opened row focused.
//!   A map row reads its name only; the pane lists the modes it supports
//!   under its description ([`pane_text`]), and A on a map with saved map
//!   variants opens a third Select Map, the map itself and then its
//!   variants ([`Mode::Variants`]).
//! - Offline, [`page`] models the two steps as one [`StackPage`] over
//!   [`CategoryRows`] and [`MapRows`], stepped by the table tests.
//!
//! The catalogue is read at boot ([`read`]): `mainmenu.map`'s multiplayer
//! list from the disc, then each content package's `patch.lvl`, as 1.0
//! fills the list itself (`0x19c120`, docs/game/map-pack.md 8.1). It is
//! read again when a package is installed while the game runs
//! ([`packages_changed`]), and the game refreshes its content slots.

pub(crate) mod packs;
pub use packs::remember_packs;
use packs::{InstalledPack, PackIndex};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use guest_layout::entry::{Cx, EntryError, GuestCall, Ret};
use guest_layout::{GuestMemory, points};

use crate::list_class::{ListClass, Rows};
use crate::ui::Chosen;
use crate::ui_back::{self, Navigation};
use crate::ui_kit::ids::{self, Family};
use crate::ui_kit::input::{Glyph, Input, Prompt, footer_legend};
use crate::ui_kit::lifecycle;
use crate::ui_kit::page::{ListPage, StackPage};
use crate::ui_kit::spec::{Describe, Press, RowSpec, ScreenSpec};

/// The categories' screen and the maps' screen, by the names `Open` rows
/// and `list_class::loader_of` use.
pub const CATEGORIES: &str = "map-categories";
pub const MAPS: &str = "map-list";
/// The feature ids (`features.<id>` switches each).
pub const CATEGORIES_FEATURE: &str = "map-categories-screen";
pub const MAPS_FEATURE: &str = "map-list-screen";

/// The multiplayer list's slots in `mainmenu.map`: 12 retail and 38 free on
/// the user's disc (map-pack.md 8.1). 1.0 lists no more.
pub const MAX_MAPS: usize = 50;

/// Our strings: the title, the categories' labels (the maps' screen's
/// title too), the two legends, then one a map.
pub const IDS: Family = ids::family("map select");
const TITLE: u32 = 0;
const CATEGORY_LABELS: u32 = 1;
const CATEGORIES_FOOTER: u32 = CATEGORY_LABELS + Category::ALL.len() as u32;
const MAPS_FOOTER: u32 = CATEGORIES_FOOTER + 1;
const MAP_LABELS: u32 = MAPS_FOOTER + 1;
const TITLE_TEXT: &str = "Select Map";

/// Where a map comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Category {
    /// On the Halo 2 disc.
    Halo2,
    /// The Multiplayer Map Pack (`h2 import-mappack`).
    MapPack,
    /// Maps of the community: new map packages (docs/game/maps.md §4) and
    /// any other content package.
    Mods,
    /// Converted Halo 1 maps (`h2 import halo1`, `h1_<map>`).
    Halo1,
}

impl Category {
    /// The categories' order on the screen.
    pub const ALL: [Category; 4] = [
        Category::Halo2,
        Category::MapPack,
        Category::Mods,
        Category::Halo1,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Category::Halo2 => "Halo 2",
            Category::MapPack => "Map Pack",
            Category::Mods => "Mods",
            Category::Halo1 => "Halo: CE",
        }
    }

    fn index(self) -> u32 {
        Category::ALL.iter().position(|c| *c == self).unwrap() as u32
    }

    /// Whether the maps keep the game's own order (the list's sort
    /// order, as `0x19c7e0` sorts them); otherwise by name.
    pub fn game_order(self) -> bool {
        matches!(self, Category::Halo2 | Category::MapPack)
    }
}

/// The Map Pack import's offers (docs/game/map-pack.md §5): `…0101` to
/// `…03ff`.
pub const MAP_PACK_OFFERS: std::ops::RangeInclusive<u32> = content::map_pack::OFFERS;
/// Converted Halo 1 maps' names (docs/halo1/multiplayer.md "Converting a map").
pub const HALO1_PREFIX: &str = "h1_";

/// The category of the map the game loads from `path`, a list slot's path
/// (`+0xb4c`):
///
/// - not under `t:\`: the disc's (`d:\maps\<name>.map`, `0x214db0`): Halo 2;
/// - `t:\$C\<offer ID>\<map>`, the low half of the offer in the Map Pack's
///   range: the Map Pack;
/// - a new map's offer ([`content::map_list::is_new_map_offer`]) whose map
///   is `h1_<map>`: Halo 1;
/// - any other content: Mods.
pub fn category_of(path: &str) -> Category {
    let lower = path.to_ascii_lowercase();
    let Some(rest) = lower.strip_prefix("t:\\$c\\") else {
        return Category::Halo2;
    };
    let mut parts = rest.split('\\');
    let offer = parts
        .next()
        .filter(|o| o.len() == 16)
        .and_then(|o| u64::from_str_radix(o, 16).ok());
    let map = lower.rsplit('\\').next().unwrap_or("");
    match offer.map(|o| o as u32) {
        Some(low) if MAP_PACK_OFFERS.contains(&low) => Category::MapPack,
        Some(low) if content::map_list::is_new_map_offer(low) && map.starts_with(HALO1_PREFIX) => {
            Category::Halo1
        }
        _ => Category::Mods,
    }
}

/// One map of the multiplayer list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Map {
    /// Its list id (slot `+0`), what the lobby's map is set to.
    pub id: u32,
    /// Its English name (slot `+0xc`).
    pub name: String,
    /// The list's sort order (slot `+0xc4c`).
    pub sort: i32,
    /// Where the game loads it from (slot `+0xb4c`).
    pub path: String,
}

impl Map {
    /// Its category, by its path ([`category_of`]). A kit package
    /// (`Firefight – <map>`) is in its path's category like any other map.
    pub fn category(&self) -> Category {
        category_of(&self.path)
    }

    fn slot_path(&self) -> Option<String> {
        let base = self.short_name();
        let slot = if self.path.to_ascii_lowercase().starts_with("t:\\") {
            let (folder, _) = self.path.rsplit_once('\\')?;
            format!("{folder}\\{base}.map")
        } else {
            // Disc records may carry a scenario tag path instead of a DVD filename.
            format!("d:\\maps\\{base}.map")
        };
        crate::map_slots::slot_file(&slot)
    }

    /// The map file's name: the path's basename, lower case.
    pub fn file(&self) -> String {
        self.path
            .rsplit('\\')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase()
    }

    /// Its short name, what its `.map` file and cache header are named
    /// (`lockout`): [`Map::file`] without a `.map` ending.
    pub fn short_name(&self) -> String {
        let f = self.file();
        f.strip_suffix(".map")
            .map_or_else(|| f.clone(), str::to_owned)
    }
}

/// The maps by category: categories in [`Category::ALL`] order, the empty
/// ones left out; maps in the game's order ([`Category::game_order`]) or by
/// name, then id. A map id listed twice keeps its last entry, as a package
/// whose record names a slot's id refills that slot (`0x19be00`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Catalogue {
    groups: Vec<(Category, Vec<Map>)>,
    overflow: usize,
    package_slots: BTreeMap<content::MapId, u32>,
    packs: Vec<InstalledPack>,
    active_pack: Option<String>,
    /// What each map supports, for a picker's lobby ([`crate::map_modes`]).
    modes: crate::map_modes::View,
}

impl Catalogue {
    pub fn new(maps: impl IntoIterator<Item = Map>) -> Self {
        let mut by_id: Vec<Map> = Vec::new();
        for m in maps {
            if m.id == u32::MAX {
                continue;
            }
            match by_id.iter_mut().find(|k| k.id == m.id) {
                Some(k) => *k = m,
                None => by_id.push(m),
            }
        }
        by_id.truncate(MAX_MAPS);
        let groups = Category::ALL
            .iter()
            .filter_map(|&c| {
                let mut maps: Vec<Map> = by_id
                    .iter()
                    .filter(|m| m.category() == c)
                    .cloned()
                    .collect();
                if c.game_order() {
                    maps.sort_by_key(|m| (m.sort, m.id));
                } else {
                    maps.sort_by_key(|m| (m.name.to_lowercase(), m.id));
                }
                (!maps.is_empty()).then_some((c, maps))
            })
            .collect();
        Catalogue {
            groups,
            ..Default::default()
        }
    }

    /// Take one snapshot for the whole picker, including its submenus.
    pub(crate) fn for_variant(self: &Arc<Self>, mode: crate::lobby_start::Mode, variant: Option<&variants::GameVariant>) -> Arc<Self> {
        if mode == crate::lobby_start::Mode::Forge {
            return self.clone();
        }
        #[cfg(feature = "rules")]
        // Siege keeps every map: the ones without its objectives are marked.
        if let Some(accepts) = variant
            .filter(|v| !crate::rules::gate::is_siege_record(v))
            .and_then(crate::rules::gate::map_filter)
        {
            let mut view = (**self).clone();
            for (_, maps) in &mut view.groups {
                maps.retain(|map| accepts(map.id));
            }
            let ids: std::collections::BTreeSet<_> = view.all().map(|m| m.id).collect();
            for pack in &mut view.packs {
                pack.members.retain(|(id, _)| ids.contains(id));
            }
            view.packs.retain(|p| !p.members.is_empty());
            return Arc::new(view);
        }
        self.clone()
    }

    /// The catalogue with `modes`: what a lobby's picker lists first, describes
    /// and refuses.
    pub(crate) fn with_modes(&self, modes: crate::map_modes::View) -> Catalogue {
        Catalogue { modes, ..self.clone() }
    }

    /// What the picker knows of the maps' modes (empty outside a picker).
    pub fn modes(&self) -> &crate::map_modes::View {
        &self.modes
    }

    /// The pause dialog uses one value list. Walk the same selectable rows
    /// as the pregame dialog, expanding each pack where its folder appears.
    pub(crate) fn playable_maps(&self, category: Category) -> Vec<Map> {
        let mode = Mode::Maps(category);
        let mut maps = Vec::new();
        let mut add = |mode, id, folder: Option<&str>| {
            if let Some(map) = self.all().find(|m| m.id == id) {
                let mut map = map.clone();
                if let Some((_, label)) = row_label(self, mode, id) {
                    map.name = match folder {
                        Some(pack) => format!("{pack} / {label}"),
                        None => label,
                    };
                }
                maps.push(map);
            }
        };
        for (id, _) in list_for(self, mode, &|_| 0) {
            if category == Category::Mods {
                if self.original_row() == Some(id) {
                    continue;
                }
                if let Some(pack) = self.pack_row(id) {
                    for (member, _) in list_for(self, Mode::Pack(id), &|_| 0) {
                        add(Mode::Pack(id), member, Some(&pack.name));
                    }
                    continue;
                }
            }
            add(mode, id, None);
        }
        maps
    }

    fn pack_row(&self, id: u32) -> Option<&InstalledPack> {
        self.packs.iter().find(|p| p.representative() == id)
    }

    fn active_pack(&self) -> Option<&InstalledPack> {
        let key = self.active_pack.as_deref()?;
        self.packs.iter().find(|p| p.key == key)
    }

    fn original_row(&self) -> Option<u32> {
        self.active_pack()?;
        self.maps(Category::Halo2).first().map(|m| m.id)
    }

    pub fn with_overflow(mut self, overflow: usize) -> Self {
        self.overflow = overflow;
        self
    }

    fn category_label(&self, c: Category) -> String {
        if c == Category::Mods && self.overflow != 0 {
            return format!("Mods ({} mods not shown, list full)", self.overflow);
        }
        c.label().to_owned()
    }

    /// Categories with maps that passed the selected variant's requirements.
    pub fn categories(&self) -> Vec<Category> {
        Category::ALL
            .into_iter()
            .filter(|&c| !self.maps(c).is_empty() || (c == Category::Mods && self.overflow != 0))
            .collect()
    }

    fn category_id(&self, c: Category) -> Option<u32> {
        if let Some(map) = self.maps(c).first() {
            return Some(map.id);
        }
        // The retail dialog draws each category from a real slot. A full,
        // empty Mods category uses a row nobody else represents.
        if c == Category::Mods && self.overflow != 0 {
            return self
                .groups
                .iter()
                .find_map(|(_, maps)| maps.get(1).map(|map| map.id));
        }
        None
    }

    /// Category `c`'s maps, in order (none when it is empty).
    pub fn maps(&self, c: Category) -> &[Map] {
        self.groups
            .iter()
            .find(|(k, _)| *k == c)
            .map_or(&[], |(_, m)| m.as_slice())
    }

    /// Every map, category by category: the order their string ids follow.
    pub fn all(&self) -> impl Iterator<Item = &Map> {
        self.groups.iter().flat_map(|(_, m)| m.iter())
    }

    /// The string id of map `id`'s label.
    pub fn label_id(&self, id: u32) -> Option<u32> {
        let n = self.all().position(|m| m.id == id)?;
        Some(IDS.id(MAP_LABELS + n as u32))
    }

    /// The strings the two screens show, by id.
    pub fn strings(&self) -> Vec<(u32, String)> {
        let mut s = vec![
            (IDS.id(TITLE), TITLE_TEXT.to_string()),
            (IDS.id(CATEGORIES_FOOTER), footer_legend(&prompts())),
            (IDS.id(MAPS_FOOTER), footer_legend(&prompts())),
        ];
        for c in Category::ALL {
            s.push((IDS.id(CATEGORY_LABELS + c.index()), self.category_label(c)));
        }
        for (n, m) in self.all().enumerate() {
            s.push((IDS.id(MAP_LABELS + n as u32), m.name.clone()));
        }
        s
    }
}

// ---- reading the list ----

/// Slot offsets (map-pack.md 8.1), for the tests' slots: the English
/// name, the scenario path, the sort order (`tags::ui_levels`).
#[cfg(test)]
const SLOT_NAME: usize = tags::ui_levels::MP_NAME;
#[cfg(test)]
const SLOT_PATH: usize = tags::ui_levels::MP_PATH;
#[cfg(test)]
const SLOT_SORT: usize = tags::ui_levels::MP_SORT;

/// A used slot of `mainmenu.map`'s multiplayer list as a [`Map`]; `None`
/// for a free one (id `-1`) or a short one. The slot is read by
/// `tags::ui_levels::mp_level`, as the Live catalog and the editor read it.
pub fn map_of_slot(slot: &[u8]) -> Option<Map> {
    let l = tags::ui_levels::mp_level(slot)?;
    Some(Map {
        id: l.id,
        name: l.name,
        sort: l.sort,
        path: l.path,
    })
}

/// The map package `dir` (a `$c\<offer ID>` directory) adds to the list:
/// its `patch.lvl` record, at the path 1.0 gives it
/// (`t:\$C\<offer ID>\<basename of the record's scenario>`).
pub fn map_of_package(dir: &Path) -> Option<Map> {
    let offer = dir.file_name()?.to_str()?;
    let bytes = std::fs::read(dir.join("patch.lvl")).ok()?;
    let e = content::map_list::Entry::from_v2(bytes.get(..content::patch_lvl::V2_LEN)?).ok()?;
    let base = e
        .scenario
        .rsplit('\\')
        .next()
        .unwrap_or(&e.scenario)
        .to_owned();
    Some(Map {
        id: e.map_id,
        name: e.names[0].clone(),
        sort: e.sort,
        path: format!("t:\\$C\\{offer}\\{base}"),
    })
}

/// The list as 1.0 will have it: the disc's used slots, then each content
/// package's record under `content_root` (`TDATA\4D530064\$c`), in
/// directory order.
pub fn read(slots: &[Vec<u8>], content_root: &Path) -> Catalogue {
    let mut maps: Vec<Map> = slots.iter().filter_map(|s| map_of_slot(s)).collect();
    let mut dirs: Vec<_> = std::fs::read_dir(content_root)
        .map(|d| d.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    dirs.sort();
    let mut slots = BTreeMap::new();
    for dir in dirs {
        let Some(map) = map_of_package(&dir) else { continue };
        if let Some(id) = crate::new_maps::package_id(&dir, &map.file()) {
            slots.insert(id, map.id);
        }
        maps.push(map);
    }
    let mut catalogue = Catalogue::new(maps);
    catalogue.package_slots = slots;
    catalogue
}

// ---- the screens ----

/// What the two screens share: the catalogue, the category picked, and
/// the map pressed. The catalogue is read at boot and again when
/// a package is installed while the game runs ([`packages_changed`]); each
/// change bumps [`Selection::generation`].
#[derive(Default)]
pub struct Selection {
    catalogue: Mutex<Arc<Catalogue>>,
    pack_index: Mutex<PackIndex>,
    generation: std::sync::atomic::AtomicU64,
    /// A content package (a new map) was added or removed this session:
    /// the game's own content scan runs again before the list is next
    /// built.
    content_changed: std::sync::atomic::AtomicBool,
    picker: Mutex<Option<Arc<Catalogue>>>,
    picked: Mutex<Option<Category>>,
    pressed: Mutex<Option<u32>>,
}

impl Selection {
    pub fn new(catalogue: Catalogue) -> Self {
        Self {
            catalogue: Mutex::new(Arc::new(catalogue)),
            ..Default::default()
        }
    }
    /// The catalogue as last read.
    pub fn catalogue(&self) -> Arc<Catalogue> {
        Arc::clone(&lock(&self.catalogue))
    }
    /// Start a visit from any multiplayer Select Map entry point. Child
    /// screens and Back use this same snapshot until the next visit.
    fn begin_picker(&self, lobby: crate::lobby_start::Mode, record: Option<&variants::GameVariant>) -> OpenList {
        let mode = variant_mode(lobby, record);
        let base = self.catalogue();
        let mut catalogue = base.for_variant(mode, record);
        let played = crate::map_modes::played(mode, record);
        if let Some(sources) = SOURCES.get() {
            let wave = record
                .map(|v| v.ext().script)
                .filter(|s| s.mode == variants::ext::Mode::Firefight)
                .map(|s| s.prefix);
            let view = crate::map_modes::in_game(&catalogue, played, wave, &sources.data_root);
            catalogue = Arc::new(catalogue.with_modes(view));
        } else {
            // Nothing to analyse the maps with: the lobby's game type still
            // says what it plays.
            let view = crate::map_modes::View::playing(played);
            catalogue = Arc::new(catalogue.with_modes(view));
        }
        *lock(&self.picker) = Some(catalogue);
        OpenList { mode: Mode::Categories, focus: None, focus_map: None, from_setup: false }
    }

    fn picker_catalogue(&self) -> Arc<Catalogue> {
        lock(&self.picker).clone().unwrap_or_else(|| self.catalogue())
    }
    /// Publish the refreshed catalogue and notify the game of changed offers.
    pub fn set(&self, mut catalogue: Catalogue, content: bool) {
        let index = lock(&self.pack_index);
        let mut current = lock(&self.catalogue);
        #[cfg(feature = "rules")]
        crate::rules::gate::forget_siege_maps();
        catalogue.packs = index.installed(&catalogue);
        catalogue.active_pack.clone_from(&current.active_pack);
        *current = Arc::new(catalogue);
        if content {
            self.content_changed
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
        self.generation
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
    /// Bumped by each [`Selection::set`].
    pub fn generation(&self) -> u64 {
        self.generation.load(std::sync::atomic::Ordering::SeqCst)
    }
    /// Whether the game's content scan must run again (and forget it).
    pub fn take_content_changed(&self) -> bool {
        self.content_changed
            .swap(false, std::sync::atomic::Ordering::SeqCst)
    }
    /// Whether Change Map opens the categories: two or more of them.
    pub fn active(&self) -> bool {
        self.catalogue().categories().len() >= 2
    }
    /// The category whose maps the maps' screen lists: the one picked, else
    /// the first.
    pub fn category(&self) -> Option<Category> {
        let picked = *self.picked.lock().unwrap_or_else(|e| e.into_inner());
        picked.or_else(|| self.catalogue().categories().first().copied())
    }
    fn pick(&self, c: Category) {
        *self.picked.lock().unwrap_or_else(|e| e.into_inner()) = Some(c);
    }
    /// The maps the maps' screen lists.
    pub fn maps(&self) -> Vec<Map> {
        let cat = self.catalogue();
        self.category()
            .map_or_else(Vec::new, |c| cat.maps(c).to_vec())
    }
    /// The map last pressed on the maps' screen, taken.
    pub fn take_pressed(&self) -> Option<u32> {
        self.pressed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }
}

/// The legend both screens show: A selects, B goes back.
pub fn prompts() -> Vec<Prompt> {
    vec![
        Prompt::new(&[Glyph::A], "Select"),
        Prompt::new(&[Glyph::B], "Back"),
    ]
}

/// The categories' screen.
pub struct CategoryRows(pub Arc<Selection>);

impl Rows for CategoryRows {
    fn content_spec(&self) -> Option<crate::ui_kit::spec::ScreenSpec> {
        Some(crate::ui_kit::spec::Describe::spec(self))
    }

    fn title(&self) -> u32 {
        IDS.id(TITLE)
    }
    fn count(&self) -> usize {
        self.0.catalogue().categories().len()
    }
    fn texts(&self, row: i16) -> Vec<(u32, u32)> {
        let cats = self.0.catalogue().categories();
        match usize::try_from(row).ok().and_then(|r| cats.get(r)) {
            Some(c) => vec![(0, IDS.id(CATEGORY_LABELS + c.index()))],
            None => Vec::new(),
        }
    }
    fn press(&self, row: i16) -> Chosen {
        let cats = self.0.catalogue().categories();
        let Some(&c) = usize::try_from(row).ok().and_then(|r| cats.get(r)) else {
            return Chosen::Nothing;
        };
        self.0.pick(c);
        Chosen::Push(MAPS)
    }
    fn describe(&self, _count: usize) -> Option<String> {
        Some(self.spec().log_line())
    }
}

impl Describe for CategoryRows {
    fn spec(&self) -> ScreenSpec {
        let mut s = screen_spec(CATEGORIES, TITLE_TEXT, IDS.id(CATEGORIES_FOOTER));
        for c in self.0.catalogue().categories() {
            s.rows.push(
                RowSpec::action(&self.0.catalogue().category_label(c))
                    .press(Press::Open(MAPS.into()))
                    .ids(&[IDS.id(CATEGORY_LABELS + c.index())]),
            );
        }
        s
    }
}

/// The maps' screen: the picked category's maps.
pub struct MapRows(pub Arc<Selection>);

impl Rows for MapRows {
    fn content_spec(&self) -> Option<crate::ui_kit::spec::ScreenSpec> {
        Some(crate::ui_kit::spec::Describe::spec(self))
    }

    fn title(&self) -> u32 {
        let c = self.0.category().unwrap_or(Category::Halo2);
        IDS.id(CATEGORY_LABELS + c.index())
    }
    fn count(&self) -> usize {
        self.0.maps().len()
    }
    fn texts(&self, row: i16) -> Vec<(u32, u32)> {
        let maps = self.0.maps();
        match usize::try_from(row).ok().and_then(|r| maps.get(r)) {
            Some(m) => self
                .0
                .catalogue()
                .label_id(m.id)
                .map(|id| vec![(0, id)])
                .unwrap_or_default(),
            None => Vec::new(),
        }
    }
    fn press(&self, row: i16) -> Chosen {
        let maps = self.0.maps();
        let Some(m) = usize::try_from(row).ok().and_then(|r| maps.get(r)) else {
            return Chosen::Nothing;
        };
        *self.0.pressed.lock().unwrap_or_else(|e| e.into_inner()) = Some(m.id);
        Chosen::Act
    }
    fn describe(&self, _count: usize) -> Option<String> {
        Some(self.spec().log_line())
    }
}

impl Describe for MapRows {
    fn spec(&self) -> ScreenSpec {
        let c = self.0.category().unwrap_or(Category::Halo2);
        let mut s = screen_spec(MAPS, c.label(), IDS.id(MAPS_FOOTER));
        s.ids.push(IDS.id(CATEGORY_LABELS + c.index()));
        for m in self.0.maps() {
            let id = self.0.catalogue().label_id(m.id).unwrap_or(0);
            s.rows
                .push(RowSpec::action(&m.name).press(Press::Act).ids(&[id]));
        }
        s
    }
}

fn screen_spec(name: &str, title: &str, footer: u32) -> ScreenSpec {
    let mut s = ScreenSpec::new(name, title);
    s.ids = vec![IDS.id(TITLE), footer];
    s.inputs = vec![Input::Up, Input::Down, Input::A, Input::B];
    s.prompts = prompts();
    s.footer = true;
    // The Halo 2 list has 12 maps, the Controls list shows 5.
    s.scrolls = true;
    s
}

// ---- the retail Select Map, in two modes ----

pub const CHANGE_MAP_LOADER: u32 = points::SELECT_MAP_LOAD.base;
/// Where the Select Map list's constructor asks for the level array
/// (`0x2c9980`, list handler in ESI).
pub const LIST_ARRAY_RET: u32 = 0x002c_9985;
/// Where the Select Map row text asks for a slot's name (`0x2c9afc`).
pub const ROW_NAME_RET: u32 = 0x002c_9b01;
/// The list handler's place in its screen (`0x2bb29f`: `screen + 0x614`).
pub const SCREEN_LIST: u32 = 0x614;
/// The retail level array (`0x19c6a0` returns it).
pub const LEVEL_ARRAY: u32 = 0x004e_e4e4;

/// What a Select Map screen of ours lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// One row a category, each standing on its first map (its picture and
    /// description), labelled with the category.
    Categories,
    /// The category's maps, in its order.
    Maps(Category),
    /// An installed pack's maps. Its first member represents its folder.
    Pack(u32),
    /// The page of the map row `map` (a list id), opened from the maps of
    /// `from`: the map itself, then each of its saved map variants
    /// ([`crate::map_modes::View::page`]). Every row carries the map's id,
    /// so each shows its picture; a row is told from its place in the list.
    Variants { map: u32, from: Category },
}

/// The element ids and lock bytes a Select Map in `mode` is built from
/// (`flags`: the retail array's byte `+2` by map id, which the list's
/// constructor tests with the profiles' progress at `0x2c99cc`).
pub fn list_for(cat: &Catalogue, mode: Mode, flags: &dyn Fn(u32) -> u8) -> Vec<(u32, u8)> {
    match mode {
        Mode::Categories => cat
            .categories()
            .iter()
            .filter_map(|&c| cat.category_id(c).map(|id| (id, 0)))
            .collect(),
        Mode::Maps(Category::Mods) => {
            let first: Vec<_> = cat.original_row().into_iter().map(|id| (id, 0)).collect();
            let rows: Vec<_> = cat.maps(Category::Mods).iter().filter(|m| !cat.modes.hides(m.id)).filter_map(|m| {
                match cat.packs.iter().find(|p| p.members.iter().any(|(id, _)| *id == m.id)) {
                    Some(pack) => (m.id == pack.representative()).then_some((m.id, 0)),
                    None => Some((m.id, flags(m.id))),
                }
            }).collect();
            first.into_iter().chain(cat.modes.first_supported(rows, |r| r.0)).collect()
        }
        Mode::Maps(c) => cat.modes.first_supported(cat.maps(c).iter().filter(|m| !cat.modes.hides(m.id)).map(|m| {
            let member = c.game_order().then(|| cat.active_pack()?.slot_for(m)).flatten();
            (member.unwrap_or_else(|| cat.modes.row_id(m.id)), flags(m.id))
        }).collect(), |r| r.0),
        Mode::Pack(id) => cat.pack_row(id).into_iter().flat_map(|p| p.members.iter())
            .map(|(id, _)| (*id, flags(*id))).collect(),
        Mode::Variants { map, .. } => page_of(cat, map).iter().map(|_| (map, 0)).collect(),
    }
}

/// The page of the map row `id` ([`crate::map_modes::View::page`]): empty
/// for a map without variants.
pub fn page_of(cat: &Catalogue, id: u32) -> Vec<crate::map_modes::PageRow> {
    cat.all()
        .find(|m| cat.modes.row_id(m.id) == id)
        .map_or_else(Vec::new, |named| cat.modes.page(id, &named.name))
}

/// Whether A on the map row `id` opens its page instead of picking it: the
/// lobby can play on the row and the map has variants.
pub fn opens_page(cat: &Catalogue, id: u32) -> bool {
    cat.modes.refusal(id).is_none() && !cat.modes.variants(id).is_empty()
}

/// What the pane says under the description of the focused row (`id`, and
/// on a map's page its place `row`): the modes it supports
/// ([`crate::map_modes::View::pane`], [`crate::map_modes::page_pane`]).
/// `None` for a category, a pack folder and a row with nothing to say.
pub fn pane_text(cat: &Catalogue, mode: Mode, id: u32, row: usize) -> Option<String> {
    match mode {
        Mode::Categories => None,
        Mode::Maps(Category::Mods) if cat.original_row() == Some(id) || cat.pack_row(id).is_some() => None,
        Mode::Maps(_) | Mode::Pack(_) => cat.modes.pane(id),
        Mode::Variants { map, .. } => {
            let page = page_of(cat, map);
            crate::map_modes::page_pane(page.get(row)?.support, cat.modes.played)
        }
    }
}

/// The name of row `row` on the page of the map row `map`, with its label
/// slot: the map's own name first, then each variant's.
pub fn page_label(cat: &Catalogue, map: u32, row: usize) -> Option<(u32, String)> {
    let name = page_of(cat, map).get(row)?.name.clone();
    Some((PAGE_LABELS + row as u32, name))
}

/// The rows of `items` (as [`list_for`] makes them) the retail dialog
/// won't show by name, each with why: a map id none of the loaded list's
/// `slots` holds (the row text asks `0x19c5f0` for its slot and names it
/// from there, `0x2c9afc`), or a lock byte, which the list's constructor
/// skips unless the profiles' progress allows it (`0x2c99cc`). Our
/// catalogue is read from the disc and the packages on the host; the
/// game's list is what its own content scan (`0x19c120`) applied, so a
/// map in one and not the other shows here.
pub fn unshown(cat: &Catalogue, items: &[(u32, u8)], slots: &[u32]) -> Vec<(String, &'static str)> {
    let name = |id: u32| {
        cat.all()
            .find(|m| m.id == id)
            .map_or_else(|| format!("{id:#x}"), |m| format!("{} ({id:#x})", m.name))
    };
    items
        .iter()
        .filter_map(|&(id, lock)| {
            let why = if !slots.contains(&id) {
                "not in the game's list"
            } else if lock != 0 {
                "locked"
            } else {
                return None;
            };
            Some((name(id), why))
        })
        .collect()
}

/// The level-array shape a list is built from: 8-byte elements, the
/// lock byte at `+2`, the map id at `+4`.
const ELEMENT: i32 = 8;
const CAPACITY: i32 = 64;

/// Writes a data array at `at` (named by the string at `name`) holding
/// `items`, as `0x19c7e0` fills the retail one.
pub fn write_list(
    m: &mut dyn guest_layout::memory::GuestMemory,
    at: u32,
    name: u32,
    items: &[(u32, u8)],
) -> Result<(), guest_layout::memory::MemFault> {
    let a = guest_layout::datum::DatumArray::at(at);
    a.initialize_contiguous(m, name, CAPACITY, ELEMENT, 0, 0)?;
    a.make_valid(m)?;
    a.delete_all(m)?;
    for &(id, flag) in items.iter().take(CAPACITY as usize) {
        let Some(h) = a.new_datum(m)? else { break };
        let e = a.element_addr(m, h.index() as i32)?;
        m.set_u8(e + 2, flag)?;
        m.set_u32(e + 4, id)?;
    }
    Ok(())
}

/// The retail array's lock byte by map id.
pub fn read_flags(m: &dyn guest_layout::memory::GuestMemory, array: u32) -> Vec<(u32, u8)> {
    let a = guest_layout::datum::DatumArray::at(array);
    let mut out = Vec::new();
    let Ok(hw) = a.header_field(m, guest_layout::datum::layout::HIGH_WATER) else {
        return out;
    };
    for i in 0..hw.clamp(0, 256) {
        if let Ok(Some(e)) = a.get_absolute(m, i)
            && let (Ok(flag), Ok(id)) = (m.u8(e + 2), m.u32(e + 4))
        {
            out.push((id, flag));
        }
    }
    out
}

/// A categories list can open fresh or restore a category on Back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OpenList {
    mode: Mode,
    from_setup: bool,
    focus: Option<Category>,
    /// The map row a maps list rebuilt by Back from its page focuses.
    focus_map: Option<u32>,
}

#[derive(Clone, Copy)]
struct OpenedList {
    handler: u32,
    from_setup: bool,
    mode: Mode,
    focus_id: Option<u32>,
}

/// The flow over Select Map screens. Metadata follows each list's lifetime.
#[derive(Default)]
pub(crate) struct Flow {
    next: Mutex<Option<OpenList>>,
    screens: Mutex<Vec<OpenedList>>,
    /// The list handler whose rows are being drawn.
    drawing: std::sync::atomic::AtomicU32,
    /// The place in its list of the row being drawn.
    drawing_row: std::sync::atomic::AtomicU32,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Flow {
    fn categories(&self, focus: Option<Category>) {
        *lock(&self.next) = Some(OpenList {
            mode: Mode::Categories,
            from_setup: false,
            focus,
            focus_map: None,
        });
    }

    fn open(&self, mode: Mode) {
        *lock(&self.next) = Some(OpenList { mode, focus: None, focus_map: None, from_setup: false });
    }

    /// Back from the page of the map row `map`: the maps of `from` again,
    /// on that row.
    fn maps_on(&self, from: Category, map: u32) {
        *lock(&self.next) = Some(OpenList {
            mode: Mode::Maps(from),
            focus: None,
            focus_map: Some(map),
            from_setup: false,
        });
    }

    fn opening(&self) -> Option<OpenList> {
        lock(&self.next).take()
    }

    pub(crate) fn built(&self, handler: u32, mode: Mode, focus_id: Option<u32>, from_setup: bool) {
        let mut screens = lock(&self.screens);
        screens.retain(|list| list.handler != handler);
        screens.push(OpenedList {
            handler,
            from_setup,
            mode,
            focus_id,
        });
    }

    fn list(&self, handler: u32) -> Option<OpenedList> {
        lock(&self.screens)
            .iter()
            .find(|list| list.handler == handler)
            .copied()
    }

    pub(crate) fn mode_of(&self, handler: u32) -> Option<Mode> {
        self.list(handler).map(|list| list.mode)
    }

    pub(crate) fn maps_from_setup(&self, handler: u32) -> bool {
        self.list(handler).is_some_and(|list| list.from_setup)
    }

    fn freed(&self, screen: u32) {
        let handler = screen.wrapping_add(SCREEN_LIST);
        lock(&self.screens).retain(|list| list.handler != handler);
    }

    fn snapshot_live(&self) -> Option<String> {
        if let Some(open) = *lock(&self.next) {
            return Some(format!("a Select Map of {:?} is to be built", open.mode));
        }
        let screens = lock(&self.screens).len();
        (screens != 0).then(|| format!("{screens} Select Map screens of ours are up"))
    }
}

const LIST_ARRAY: u32 = 0x70;
/// An item widget's row: a handle in its list's array, -1 for none
/// (`guest_layout::view::ui::Item`).
const ITEM_ROW: u32 = 0x70;

/// Find the new datum by map id, since the live list can omit locked rows
/// and the rebuilt array uses fresh handles.
fn focus_handle(mem: &dyn GuestMemory, array: u32, id: u32) -> Option<u32> {
    let array = guest_layout::datum::DatumArray::at(array);
    let count = array.header_field(mem, guest_layout::datum::layout::HIGH_WATER).ok()?;
    (0..count.clamp(0, MAX_MAPS as i32)).find_map(|row| {
        let element = array.get_absolute(mem, row).ok()??;
        if mem.u32(element + 4).ok()? != id {
            return None;
        }
        Some(array.handle_of(mem, row).ok()??.raw())
    })
}

/// Space for each category label, including the overflow notice.
const LABEL_LEN: u32 = 0x80;

pub fn row_label(cat: &Catalogue, mode: Mode, id: u32) -> Option<(u32, String)> {
    let slot_label = || {
        let i = cat.all().position(|map| map.id == id)?;
        Some(Category::ALL.len() as u32 + i as u32)
    };
    match mode {
        Mode::Categories => {
            let c = cat.categories().into_iter().find(|&c| cat.category_id(c) == Some(id))?;
            Some((c.index(), cat.category_label(c)))
        }
        Mode::Maps(Category::Mods) if cat.original_row() == Some(id) =>
            Some((slot_label()?, "Use original maps".into())),
        Mode::Maps(Category::Mods) => Some((slot_label()?, cat.pack_row(id)?.name.clone())),
        Mode::Pack(group) => {
            let (_, base) = cat.pack_row(group)?.members.iter().find(|(slot, _)| *slot == id)?;
            Some((slot_label()?, live_hle::mods::base_title(base)))
        }
        Mode::Maps(c) if c.game_order() => {
            let pack = cat.active_pack()?;
            let base = cat.maps(c).iter().find(|map| pack.slot_for(map) == Some(id))?;
            Some((slot_label()?, format!("{} ({})", base.name, pack.name)))
        }
        // Any other map row reads its slot's own name: what it supports is
        // in the pane ([`pane_text`]).
        _ => None,
    }
}

fn utf16z(text: &str) -> Vec<u8> {
    let mut b: Vec<u8> = text
        .encode_utf16()
        .take((LABEL_LEN / 2 - 1) as usize)
        .flat_map(u16::to_le_bytes)
        .collect();
    b.extend([0, 0]);
    b
}

/// Our memory: the list array, its name, the labels, then the panes.
const ARRAY_AT: u32 = 0;
const NAME_AT: u32 = 0x400;
const LABELS_AT: u32 = 0x1000;
/// The first label of a map's page, after the categories' and the maps'.
const PAGE_LABELS: u32 = Category::ALL.len() as u32 + MAX_MAPS as u32;
const PANES_AT: u32 = LABELS_AT + (PAGE_LABELS + 1 + crate::map_modes::PAGE_VARIANTS as u32) * LABEL_LEN;
/// A pane's text: its UTF-16 units with the terminator, and its bytes with
/// the slack [`pane_at`] places it in.
const PANE_UNITS: usize = 384;
const PANE_LEN: u32 = 0x100 + PANE_UNITS as u32 * 2;
/// One pane a kind of list: a list and the one it opened are up together
/// while one replaces the other.
const PANES: u32 = 4;
const REGION: u32 = PANES_AT + PANES * PANE_LEN;

/// The retail Select Map's focus handler (`0x2c9b7c`, screen in ECX) asks
/// for the language at `0x2c9be6` with the screen in EDI and the focused
/// row's slot in EBP, then shows `slot + MP_DESC + 0x100 * language` in
/// its description block.
pub const PANE_LANGUAGE_RET: u32 = 0x002c_9beb;
/// The language's cache (`0x11cae0` fills it; -1 before).
const LANGUAGE: u32 = 0x0047_ff38;
/// A slot has a description a language (`0x24c..0xb4c`).
const LANGUAGES: u32 = 9;
const SLOT_DESC: u32 = tags::ui_levels::MP_DESC as u32;
const DESC_LEN: u32 = 0x100;

fn pane_index(mode: Mode) -> u32 {
    match mode {
        Mode::Categories => 0,
        Mode::Maps(_) => 1,
        Mode::Pack(_) => 2,
        Mode::Variants { .. } => 3,
    }
}

/// Where the pane of `mode` is written so the retail handler reads it as
/// a description of `slot`: in the pane's space under `region`, a whole
/// number of descriptions past the slot's first.
pub fn pane_at(region: u32, mode: Mode, slot: u32) -> u32 {
    let base = region + PANES_AT + pane_index(mode) * PANE_LEN;
    base + (slot.wrapping_add(SLOT_DESC).wrapping_sub(base) & (DESC_LEN - 1))
}

/// The language to answer so the description of `slot` is read at `at`
/// ([`pane_at`]): the handler computes `slot + MP_DESC + (language << 8)`
/// in 32 bits.
pub fn pane_language(at: u32, slot: u32) -> u32 {
    at.wrapping_sub(slot.wrapping_add(SLOT_DESC)) >> 8
}

/// The pane's text as UTF-16 with its terminator, cut to [`PANE_UNITS`].
fn pane_bytes(text: &str) -> Vec<u8> {
    let mut b: Vec<u8> = text
        .encode_utf16()
        .take(PANE_UNITS - 1)
        .flat_map(u16::to_le_bytes)
        .collect();
    b.extend([0, 0]);
    b
}

/// The description the game holds for `slot` in `language`.
fn slot_description(mem: &dyn GuestMemory, slot: u32, language: u32) -> String {
    let at = slot.wrapping_add(SLOT_DESC + language * DESC_LEN);
    let units: Vec<u16> = (0..tags::ui_levels::DESC_UNITS as u32)
        .map_while(|i| mem.u16(at.wrapping_add(i * 2)).ok().filter(|&u| u != 0))
        .collect();
    String::from_utf16_lossy(&units)
}

/// The hooks (feature `map-select`).
pub struct MapSelect {
    pub sel: Arc<Selection>,
    pub navigation: Arc<Navigation>,
    pub slots: Arc<Slots>,
    pub live: Option<Arc<live_hle::Hle>>,
}

/// Guards reentry when a tick resumes after refreshing the content slots.
#[derive(Debug, Default)]
pub struct Slots {
    passing: std::sync::atomic::AtomicBool,
}

/// Writes the array a Select Map in `mode` lists into our memory at
/// `region`; its address, `None` when it could not be written.
fn write_ours(
    mem: &mut dyn guest_layout::memory::GuestMemory,
    cat: &Catalogue,
    mode: Mode,
    region: u32,
) -> Option<u32> {
    // The catalogue already contains the maps supported by this variant.
    // Retail profile unlocks must not remove rows from only this picker.
    let items = list_for(cat, mode, &|_| 0);
    let slots: Vec<u32> = crate::mod_slots::read_slots(&crate::layout_glue::ReadGm(&*mem))
        .iter()
        .map(crate::mod_slots::Slot::id)
        .collect();
    // No slots read (not the main menu's globals): nothing to compare.
    let lost = if slots.is_empty() {
        Vec::new()
    } else {
        unshown(cat, &items, &slots)
    };
    if !lost.is_empty() {
        let lost: Vec<String> = lost.iter().map(|(m, why)| format!("{m}: {why}")).collect();
        eprintln!(
            "map select: {mode:?}: {} of {} rows won't show by name ({})",
            lost.len(),
            items.len(),
            lost.join(", ")
        );
    }
    let written =
        guest_layout::memory::GuestMemory::write(&mut *mem, region + NAME_AT, b"map select list\0")
            .and_then(|()| write_list(&mut *mem, region + ARRAY_AT, region + NAME_AT, &items));
    match written {
        Ok(()) => {
            eprintln!(
                "map select: Select Map lists {mode:?} ({} rows)",
                items.len()
            );
            eprintln!("map select: rows: {}", row_names(cat, mode, &items));
            Some(region + ARRAY_AT)
        }
        Err(e) => {
            eprintln!("map select: the list was not written ({e:?}); the retail list");
            None
        }
    }
}

/// The rows of a Select Map list in order, as `h2 bots test`'s driver
/// reads them off `map select: rows: a, b, c` to count its presses: a
/// category by its label, a map by its short name (`zanzibar`), anything
/// else by its id.
fn row_names(cat: &Catalogue, mode: Mode, items: &[(u32, u8)]) -> String {
    let categories = cat.categories();
    items
        .iter()
        .enumerate()
        .map(|(i, &(id, _))| match mode {
            Mode::Categories => categories
                .iter()
                .filter(|&&c| cat.category_id(c).is_some())
                .nth(i)
                .map_or_else(|| format!("{id:#x}"), |c| c.label().to_owned()),
            _ => cat
                .all()
                .find(|m| m.id == id)
                .map_or_else(|| format!("{id:#x}"), Map::short_name),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Refresh in the main menu so a joining console can use its newly
/// acquired map without opening Change Map.
async fn refresh_slots(cx: &Cx, sel: &Selection) -> Result<(), EntryError> {
    use crate::layout_glue::Gm;
    let catalogue = sel.catalogue();
    cx.write(|m| {
        let mut m = crate::mod_slots::WriteGm(m);
        let slots = crate::mod_slots::read_slots(&m);
        for at in crate::mod_slots::obsolete(&slots, &catalogue) {
            m.set_u32(at, u32::MAX);
        }
    });
    cx.call(GuestCall::new(crate::mod_slots::CONTENT_SCAN))
        .await?;
    if let Some(array) = cx.read(|m| m.u32(LEVEL_ARRAY)).ok().filter(|&a| a != 0) {
        cx.call(GuestCall::new(crate::mod_slots::MP_ARRAY_BUILD).args([array]))
            .await?;
    }
    Ok(())
}

/// The feature id.
pub const FEATURE: &str = "map-select";

impl hooks::Feature for MapSelect {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new(FEATURE, "The multiplayer map select by category")
            .doc("docs/game/map-select.md")
    }
    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        use guest_layout::points::{
            GAME_TICK, LOBBY_SET_MAP, LOBBY_START_STATE, MAP_LIST_ROW_CHOSEN, MAP_LIST_ROW_TEXT, MAP_SCREEN_FREE,
            MP_LEVEL_ARRAY, MP_LEVEL_NAME, UI_LANGUAGE,
        };
        use hooks::Flow as F;
        let region = b.guest_alloc(REGION, runtime::guest::Permissions::RW)?;

        if let Some(hle) = self.live.clone() {
            b.raw()
                .observe_after(&LOBBY_SET_MAP, "advertise the selected map", move |o| {
                    let guest = o.guest();
                    if crate::coop_probe::read_session(&*guest)
                        .is_some_and(|s| s.live() && s.is_host() && !s.is_campaign())
                    {
                        hle.lobby_map_changed(&guest);
                    }
                });
        }

        let (sel, slots) = (Arc::clone(&self.sel), Arc::clone(&self.slots));
        b.raw()
            .before(&GAME_TICK, "refresh installed map offers", move |c| {
                use std::sync::atomic::Ordering;
                if slots.passing.swap(false, Ordering::SeqCst) {
                    return Ok(F::Continue);
                }
                let main_menu = {
                    let mem = c.ctx.runtime().memory();
                    crate::mod_slots::in_main_menu(&crate::layout_glue::ReadGm(&*mem))
                        && mem.u32(LEVEL_ARRAY).unwrap_or(0) != 0
                };
                if !main_menu || !sel.take_content_changed() {
                    return Ok(F::Continue);
                }
                let (sel, slots) = (Arc::clone(&sel), Arc::clone(&slots));
                c.answer_async(move |cx| -> guest_layout::entry::EntryFuture {
                    Box::pin(async move {
                        refresh_slots(&cx, &sel).await?;
                        slots.passing.store(true, Ordering::SeqCst);
                        let r = cx.call(GuestCall::new(GAME_TICK.base)).await?;
                        Ok(guest_layout::entry::Ret::new(r.eax, 0))
                    })
                })
            })
            // Keep these wrappers outside the refresh. Co-op and sim
            // run inside it, on the tick we re-enter.
            .first()
            .after_feature(agent::sim::calls::FEATURE)
            .after_feature(crate::skulls_custom::FEATURE);

        // The built-in Firefight kits finished building while the lobby
        // waited on a map without the kit: put it on Coagulation's
        // (`lobby_start::landing_map`), as switching to Firefight does.
        b.raw().before(&GAME_TICK, "land a Firefight lobby on its kit map", move |c| {
            if !crate::lobby_start::take_kit_landing() {
                return Ok(F::Continue);
            }
            let landing = {
                let mem = c.ctx.runtime().memory();
                let state = crate::lobby_start::state();
                (state.lobby && state.mode == crate::lobby_start::Mode::Firefight)
                    .then(|| {
                        let catalogue = current()?;
                        let id = mem.u32(crate::h1_options_row::LOBBY_MAP).ok();
                        (!crate::lobby_start::KitMaps::now(&*mem).lobby)
                            .then(|| crate::lobby_start::landing_map(&catalogue, id))
                            .flatten()
                    })
                    .flatten()
            };
            let Some(map) = landing else {
                return Ok(F::Continue);
            };
            c.answer_async(move |cx| -> guest_layout::entry::EntryFuture {
                Box::pin(async move {
                    // Not while the game is starting or counting down.
                    let state = cx
                        .call(GuestCall::new(LOBBY_START_STATE.base).args([0]))
                        .await?
                        .eax;
                    if !matches!(state, 8 | 10) {
                        cx.call(GuestCall::new(LOBBY_SET_MAP.base).esi(map).args([u32::MAX]))
                            .await?;
                        eprintln!("lobby: landed on the Firefight kit map {map}");
                    }
                    let r = cx.call(GuestCall::new(GAME_TICK.base)).await?;
                    Ok(guest_layout::entry::Ret::new(r.eax, 0))
                })
            })
        })
        // Outside co-op, sim, Firefight and the rules, like the refresh
        // above: they run inside the tick this re-enters.
        .first()
        .after_feature(agent::sim::calls::FEATURE)
        .after_feature(crate::skulls_custom::FEATURE);

        // The lobby's game type needs a map it supports: Firefight or Siege
        // chosen on a map without their support moves the lobby to the map
        // last used with them, else the first the picker lists.
        b.raw().before(&GAME_TICK, "pick a map the game type supports", move |c| {
            let state = crate::lobby_start::state();
            if !state.lobby {
                return Ok(F::Continue);
            }
            let to = {
                let mem = c.ctx.runtime().memory();
                let map = mem.u32(crate::h1_options_row::LOBBY_MAP).ok();
                let record = crate::pause_change_map::lobby_variant(&*mem);
                drop(mem);
                auto_pick(&state, map, record.as_ref())
            };
            let Some(map) = to else {
                return Ok(F::Continue);
            };
            c.answer_async(move |cx| -> guest_layout::entry::EntryFuture {
                Box::pin(async move {
                    // Not while the game is starting or counting down.
                    let started = cx
                        .call(GuestCall::new(LOBBY_START_STATE.base).args([0]))
                        .await?
                        .eax;
                    if !matches!(started, 8 | 10) {
                        cx.call(GuestCall::new(LOBBY_SET_MAP.base).esi(map).args([u32::MAX]))
                            .await?;
                    }
                    let r = cx.call(GuestCall::new(GAME_TICK.base)).await?;
                    Ok(guest_layout::entry::Ret::new(r.eax, 0))
                })
            })
        })
        .first()
        .after_feature(agent::sim::calls::FEATURE)
        .after_feature(crate::skulls_custom::FEATURE);

        // The list's array for a Select Map of ours.
        let (nav, sel) = (Arc::clone(&self.navigation), Arc::clone(&self.sel));
        b.raw().before(
            &MP_LEVEL_ARRAY,
            "a Select Map of ours lists ours",
            move |c| {
                let rt = Arc::clone(c.ctx.runtime());
                let mut mem = rt.memory();
                if mem.u32(c.cpu.esp).ok() != Some(LIST_ARRAY_RET) {
                    return Ok(F::Continue);
                }
                if mem.u32(LEVEL_ARRAY).unwrap_or(0) == 0 {
                    return Ok(F::Continue);
                }
                let open = match nav.maps.opening() {
                    Some(parent_or_child) => parent_or_child,
                    None => {
                        let variant = crate::pause_change_map::lobby_variant(&*mem);
                        drop(mem);
                        let first = sel.begin_picker(crate::lobby_start::mode(), variant.as_ref());
                        mem = rt.memory();
                        first
                    }
                };
                let catalogue = sel.picker_catalogue();
                match write_ours(&mut *mem, &catalogue, open.mode, region) {
                    Some(at) => {
                        let focus_id = open
                            .focus
                            .and_then(|folder| catalogue.category_id(folder))
                            .or(open.focus_map);
                        nav.maps.built(c.cpu.esi, open.mode, focus_id, open.from_setup);
                        drop(mem);
                        c.cpu.eax = at;
                        c.ret(0).map(F::Return)
                    }
                    None => Ok(F::Continue),
                }
            },
        );

        // Which list is drawing its rows.
        let nav = Arc::clone(&self.navigation);
        b.raw()
            .observe_before(&MAP_LIST_ROW_TEXT, "note the list drawing", move |o| {
                use std::sync::atomic::Ordering;
                nav.maps.drawing.store(o.cpu.ecx, Ordering::SeqCst);
                // The item widget's row: a handle in the list's array, its
                // low word the row's place.
                let guest = o.guest();
                let row = guest
                    .u32(o.cpu.esp + 4)
                    .and_then(|item| guest.u32(item + ITEM_ROW))
                    .ok()
                    .filter(|&handle| handle != u32::MAX)
                    .map_or(u32::MAX, |handle| handle & 0xffff);
                nav.maps.drawing_row.store(row, Ordering::SeqCst);
            });

        // Category rows supply their own labels. Map rows use their slot.
        let (nav, sel) = (Arc::clone(&self.navigation), Arc::clone(&self.sel));
        b.raw().before(&MP_LEVEL_NAME, "category names", move |c| {
            let rt = Arc::clone(c.ctx.runtime());
            let mut mem = rt.memory();
            if mem.u32(c.cpu.esp).ok() != Some(ROW_NAME_RET) {
                return Ok(F::Continue);
            }
            let handler = nav.maps.drawing.load(std::sync::atomic::Ordering::SeqCst);
            let Some(mode) = nav.maps.mode_of(handler) else {
                return Ok(F::Continue);
            };
            let Some(id) = mem.u32(c.cpu.esp + 4).and_then(|slot| mem.u32(slot)).ok() else {
                return Ok(F::Continue);
            };
            let label = match mode {
                Mode::Variants { map, .. } => {
                    let row = nav.maps.drawing_row.load(std::sync::atomic::Ordering::SeqCst);
                    page_label(&sel.picker_catalogue(), map, row as usize)
                }
                _ => row_label(&sel.picker_catalogue(), mode, id),
            };
            let Some((n, text)) = label else {
                return Ok(F::Continue);
            };
            let at = region + LABELS_AT + n * LABEL_LEN;
            if guest_layout::memory::GuestMemory::write(&mut *mem, at, &utf16z(&text)).is_err() {
                return Ok(F::Continue);
            }
            drop(mem);
            c.cpu.eax = at;
            c.ret(4).map(F::Return)
        });

        // A press opens a category's maps or selects a map in retail.
        let (nav, sel) = (Arc::clone(&self.navigation), Arc::clone(&self.sel));
        let request_at = region + NAME_AT + 0x100;
        b.raw().before(
            &MAP_LIST_ROW_CHOSEN,
            "categories open their maps",
            move |c| {
                let handler = c.cpu.ecx;
                let Some(mode) = nav.maps.mode_of(handler) else {
                    return Ok(F::Continue);
                };
                let screen = handler.wrapping_sub(SCREEN_LIST);
                let (id, place, chain, stack, ready) = {
                    let mem = c.ctx.runtime().memory();
                    let chain = mem.u32(c.cpu.esp + 4).ok();
                    let row = mem.u32(c.cpu.esp + 8).and_then(|p| mem.u32(p)).ok();
                    let place = row.filter(|r| *r != u32::MAX).map(|r| (r & 0xffff) as usize);
                    let id = row.filter(|r| *r != u32::MAX).and_then(|r| {
                        let data = mem.u32(mem.u32(handler + 0x70).ok()? + 0x44).ok()?;
                        mem.u32(data + (r & 0xffff) * 8 + 4).ok()
                    });
                    let stack = mem.u32(screen + lifecycle::SCREEN_WINDOW).ok()
                        .zip(mem.u32(screen + lifecycle::SCREEN_SLOT).ok());
                    let ready = stack
                        .and_then(|(window, slot)| crate::ui_watch::channel(window, slot))
                        .is_some_and(|ch| {
                            mem.u32(ch + crate::ui_watch::ACTIVE).ok() == Some(screen)
                                && mem.u32(ch + crate::ui_watch::PENDING).ok() == Some(0)
                        });
                    (id, place, chain, stack, ready)
                };
                if !ready {
                    return c.ret(8).map(F::Return);
                }
                let (Some(id), Some((window, slot))) = (id, stack) else {
                    return Ok(F::Continue);
                };
                let catalogue = sel.picker_catalogue();
                let next = match mode {
                    Mode::Categories => catalogue.categories().into_iter()
                        .find(|&k| catalogue.category_id(k) == Some(id)).map(Mode::Maps),
                    Mode::Maps(Category::Mods) if catalogue.original_row() == Some(id) => {
                        Arc::make_mut(&mut *lock(&sel.catalogue)).active_pack = None;
                        if let Some(view) = lock(&sel.picker).as_mut() {
                            Arc::make_mut(view).active_pack = None;
                        }
                        Some(Mode::Maps(Category::Halo2))
                    }
                    Mode::Maps(Category::Mods) if catalogue.pack_row(id).is_some() => Some(Mode::Pack(id)),
                    // A map with variants opens its page.
                    Mode::Maps(from) if opens_page(&catalogue, id) => Some(Mode::Variants { map: id, from }),
                    _ => None,
                };
                match next {
                    None if mode == Mode::Categories => c.ret(8).map(F::Return),
                    None => {
                        if let Mode::Pack(group) = mode
                            && let Some(pack) = catalogue.pack_row(group)
                        {
                            Arc::make_mut(&mut *lock(&sel.catalogue)).active_pack = Some(pack.key.clone());
                        }
                        // A lobby that plays Firefight or Siege takes no map
                        // without it, and says why.
                        if let Mode::Variants { map, .. } = mode {
                            // A row of a map's page: the map itself plays
                            // without a variant, a variant becomes the
                            // lobby's, and the retail press sets the map.
                            let page = page_of(&catalogue, map);
                            let Some(row) = place.and_then(|p| page.get(p)) else {
                                return c.ret(8).map(F::Return);
                            };
                            if let Some(reason) = crate::map_modes::refusal(row.support, catalogue.modes().played) {
                                crate::lobby_tabs::notice(reason);
                                return c.ret(8).map(F::Return);
                            }
                            if let Some(sources) = SOURCES.get() {
                                let choose = crate::lobby_mode::choose_in_lobby(sources.data_root.clone(), Some(map));
                                eprintln!("map select: {}", choose(row.file.as_deref()));
                            }
                        } else if let Some(reason) = catalogue.modes().refusal(id) {
                            crate::lobby_tabs::notice(reason);
                            return c.ret(8).map(F::Return);
                        }
                        // With Clankers on, the map's nav mesh is asked for now
                        // (docs/game/bots.md §10 stage 2).
                        let cat = sel.picker_catalogue();
                        let map = cat.all().find(|m| m.id == id).map(Map::short_name);
                        if let Some(map) = map {
                            eprintln!("map select: picked {map}");
                            crate::clankers::map_picked(&map);
                        }
                        Ok(F::Continue)
                    }
                    Some(next) => {
                        let mask = {
                            let mem = c.ctx.runtime().memory();
                            chain.and_then(|ch| {
                                crate::list_class::player_mask(&*mem, ch).ok().flatten()
                            })
                        };
                        let Some(mask) = mask else {
                            return c.ret(8).map(F::Return);
                        };
                        eprintln!(
                            "map select: {next:?} on window {window}, slot {slot}"
                        );
                        let request = crate::skulls_mp::open_request(
                            mask as u16,
                            window,
                            slot,
                            CHANGE_MAP_LOADER,
                        );
                        let nav = Arc::clone(&nav);
                        c.answer_async(move |cx| {
                            Box::pin(async move {
                                cx.write(|m| m.write(request_at, &request))?;
                                nav.maps.open(next);
                                let opened = cx
                                    .call(GuestCall::new(CHANGE_MAP_LOADER).args([request_at]))
                                    .await;
                                let _ = nav.maps.opening();
                                opened?;
                                Ok(Ret::new(0, 8))
                            })
                        })
                    }
                }
            },
        );

        // The window holds one active dialog. Back from maps schedules
        // a replacement with the categories, keeping the folder's identity.
        let nav = Arc::clone(&self.navigation);
        b.raw().before(&points::SCREEN_BACK_EVENT, "return from maps to their folders", move |c| {
            let screen = c.cpu.ecx;
            let Some(mode) = nav.maps.mode_of(screen.wrapping_add(SCREEN_LIST)) else {
                return Ok(F::Continue);
            };
            let (action, source) = {
                let m = c.ctx.runtime().memory();
                let mem = crate::layout_glue::ReadGm(&*m);
                let input = m
                    .u32(c.cpu.esp + 4)
                    .ok()
                    .and_then(|event| ui_back::request_input(&mem, event));
                ui_back::back(&mem, screen, input)
            };
            match (action, source, mode) {
                (ui_back::Back::Wait, _, _) => {
                    c.cpu.eax = 1;
                    c.ret(4).map(F::Return)
                }
                // A Forge or Firefight lobby's maps were opened straight
                // from Game Setup: B goes back there.
                (ui_back::Back::Reopen, _, Mode::Maps(_))
                    if nav.maps.maps_from_setup(screen.wrapping_add(SCREEN_LIST)) =>
                {
                    Ok(F::Continue)
                }
                (
                    ui_back::Back::Reopen,
                    Some(source),
                    mode @ (Mode::Maps(_) | Mode::Pack(_) | Mode::Variants { .. }),
                ) => {
                    let nav = Arc::clone(&nav);
                    c.answer_async(move |cx| Box::pin(async move {
                        match mode {
                            Mode::Maps(category) => nav.maps.categories(Some(category)),
                            Mode::Pack(_) => nav.maps.open(Mode::Maps(Category::Mods)),
                            Mode::Variants { map, from } => nav.maps.maps_on(from, map),
                            Mode::Categories => unreachable!(),
                        }
                        let opened = ui_back::reopen(&cx, request_at, source, CHANGE_MAP_LOADER).await;
                        let _ = nav.maps.opening();
                        opened?;
                        Ok(Ret::new(1, 4))
                    }))
                }
                _ => Ok(F::Continue),
            }
        });

        // At push the list has its actual rows. The retail activation
        // path applies this handle once the incoming widgets exist.
        let nav = Arc::clone(&self.navigation);
        b.raw().before(&points::SCREEN_PUSH, "restore the opened folder's focus", move |c| {
            let handler = c.cpu.edi.wrapping_add(SCREEN_LIST);
            if let Some(OpenedList {
                focus_id: Some(id),
                ..
            }) = nav.maps.list(handler)
            {
                let mut mem = c.ctx.runtime().memory();
                let request = c.cpu.ebx;
                if mem.u8(request).is_ok_and(|flags| flags & 1 != 0)
                    && let Ok(array) = mem.u32(handler.wrapping_add(LIST_ARRAY))
                    && let Some(handle) = focus_handle(&*mem, array, id)
                {
                    mem.set_u32(request.wrapping_add(ui_back::REQUEST_FOCUS), handle)?;
                }
            }
            Ok(F::Continue)
        })
        // ui-back records the replaced Game Setup before the folder
        // focus is restored here.
        .after_feature(crate::ui_back::FEATURE);

        // The pane: the focused row's description, then what it supports.
        // The retail handler shows the slot's description for the
        // language it is told; told one that lands on our text, it shows
        // ours ([`pane_at`], [`pane_language`]).
        let (nav, sel) = (Arc::clone(&self.navigation), Arc::clone(&self.sel));
        b.raw().before(&UI_LANGUAGE, "a map's pane lists what it supports", move |c| {
            let rt = Arc::clone(c.ctx.runtime());
            let mut mem = rt.memory();
            if mem.u32(c.cpu.esp).ok() != Some(PANE_LANGUAGE_RET) {
                return Ok(F::Continue);
            }
            let (screen, slot) = (c.cpu.edi, c.cpu.ebp);
            let handler = screen.wrapping_add(SCREEN_LIST);
            let Some(mode) = nav.maps.mode_of(handler) else {
                return Ok(F::Continue);
            };
            let language = mem.u32(LANGUAGE).ok().filter(|&l| l < LANGUAGES);
            let (Some(language), Ok(id)) = (language, mem.u32(slot)) else {
                return Ok(F::Continue);
            };
            let place = match mode {
                Mode::Variants { .. } => mem
                    .u32(screen + lifecycle::SCREEN_WINDOW)
                    .ok()
                    .zip(mem.u32(screen + lifecycle::SCREEN_SLOT).ok())
                    .and_then(|(window, slot)| crate::ui_watch::channel(window, slot))
                    .and_then(|ch| lifecycle::focused_row_handle(&*mem, ch, handler))
                    .map_or(0, |handle| (handle & 0xffff) as usize),
                _ => 0,
            };
            let Some(pane) = pane_text(&sel.picker_catalogue(), mode, id, place) else {
                return Ok(F::Continue);
            };
            let text = crate::map_modes::described(&slot_description(&*mem, slot, language), Some(&pane));
            let at = pane_at(region, mode, slot);
            if guest_layout::memory::GuestMemory::write(&mut *mem, at, &pane_bytes(&text)).is_err() {
                return Ok(F::Continue);
            }
            drop(mem);
            c.cpu.eax = pane_language(at, slot);
            c.ret(0).map(F::Return)
        });

        let nav = Arc::clone(&self.navigation);
        b.raw().observe_before(&MAP_SCREEN_FREE, "forget the freed map list", move |o| {
            nav.maps.freed(o.cpu.ecx);
        });
        let nav = Arc::clone(&self.navigation);
        let (save, load) = (Arc::clone(&self.sel), Arc::clone(&self.sel));
        b.snapshot(hooks::SnapshotPolicy::section_forbid_while(
            move || nav.maps.snapshot_live(),
            move || save.catalogue().active_pack.as_deref().unwrap_or_default().as_bytes().to_vec(),
            move |bytes, _| {
                let key = std::str::from_utf8(bytes).map_err(|e| e.to_string())?;
                Arc::make_mut(&mut *lock(&load.catalogue)).active_pack =
                    (!key.is_empty()).then(|| key.to_owned());
                Ok(())
            },
        ));
        Ok(())
    }
}

/// The lobby's game type and map as the picking rule last saw them.
static WATCHED: Mutex<Option<(crate::map_modes::Played, u32)>> = Mutex::new(None);

/// The map the lobby on `map` (its list id) moves to when its game type
/// `record` needs one the map does not support
/// ([`crate::map_modes::pick`]): the map last used with that game type,
/// else the first the picker lists. Decides once for each game type and
/// map the lobby is seen on; it also notes a supported map as the one last
/// used. `None` when nothing moves.
pub(crate) fn auto_pick(
    state: &crate::lobby_start::State,
    map: Option<u32>,
    record: Option<&variants::GameVariant>,
) -> Option<u32> {
    use crate::map_modes::{self, Played};
    let played = map_modes::played(variant_mode(state.mode, record), record);
    let key = map.map(|m| (played, m));
    {
        let mut seen = lock(&WATCHED);
        if *seen == key {
            return None;
        }
        *seen = key;
    }
    if played == Played::Other {
        return None;
    }
    let cat = current()?;
    let sources = SOURCES.get()?;
    let wave = record
        .map(|v| v.ext().script)
        .filter(|s| s.mode == variants::ext::Mode::Firefight)
        .map(|s| s.prefix);
    let view = map_modes::in_game(&cat, played, wave, &sources.data_root);
    if map.is_some_and(|m| view.allows(m)) {
        map_modes::remember(played, map?);
        return None;
    }
    let view_cat = cat.with_modes(view);
    let rows: Vec<(u32, bool)> = view_cat
        .categories()
        .into_iter()
        .flat_map(|c| list_for(&view_cat, Mode::Maps(c), &|_| 0))
        .map(|(id, _)| (id, view_cat.modes().allows(id)))
        .collect();
    let to = map_modes::pick(&rows, map, map_modes::last(played))?;
    eprintln!("lobby: {played:?} needs a map that supports it, moving to {to}");
    *lock(&WATCHED) = Some((played, to));
    Some(to)
}

/// Adds the hooks when the boot [`prepare`]d a catalogue (it always does:
/// with one category or none Change Map opens the retail list until a
/// package installed while the game runs makes a second). Returns whether
/// they were added.
pub fn install(
    registry: &mut hooks::Registry,
    live: Option<Arc<live_hle::Hle>>,
    navigation: Arc<Navigation>,
) -> anyhow::Result<bool> {
    let Some(sel) = PREPARED.get() else {
        return Ok(false);
    };
    let added = registry
        .add(&MapSelect {
            sel: Arc::clone(sel),
            navigation,
            slots: Arc::default(),
            live,
        })
        .map_err(|e| anyhow::anyhow!("map select: {e}"))?;
    if !added {
        eprintln!("map select: features.map-select is off; the retail map list");
    }
    Ok(added)
}

// ---- prepared at boot, read again when packages change ----

static PREPARED: OnceLock<Arc<Selection>> = OnceLock::new();
static SOURCES: OnceLock<Sources> = OnceLock::new();

/// The selected map's content path, for a Live lobby advertisement.
pub fn selected_path(id: u32) -> Option<String> {
    let cat = PREPARED.get()?.catalogue();
    let map = cat.all().find(|map| map.id == id)?;
    map.slot_path()
}

/// The short name (`lockout`) of the catalogue's map `id`: the base map
/// a map variant of the lobby's map is stored under.
pub fn short_name_of(id: u32) -> Option<String> {
    let cat = PREPARED.get()?.catalogue();
    Some(cat.all().find(|map| map.id == id)?.short_name())
}

pub(super) fn opened_pack_name(id: u32) -> Option<String> {
    Some(PREPARED.get()?.catalogue().pack_row(id)?.name.clone())
}

/// The name of the map whose page ([`Mode::Variants`]) is open.
pub(super) fn opened_map_name(id: u32) -> Option<String> {
    let cat = PREPARED.get()?.picker_catalogue();
    let named = cat.all().find(|m| cat.modes.row_id(m.id) == id)?;
    Some(named.name.clone())
}

/// The category of the selected map, using the catalogue kept at boot
/// and refreshed as content packages change.
pub(crate) fn selected_category(id: u32) -> Option<Category> {
    let cat = PREPARED.get()?.catalogue();
    let map = cat.all().find(|map| map.id == id)?;
    Some(map.category())
}

/// The catalogue as boot read it and package installs since refreshed it
/// (a lobby's map picker lists from it), or `None` before boot prepared it.
pub fn current() -> Option<Arc<Catalogue>> {
    Some(PREPARED.get()?.catalogue())
}

pub fn prepare(catalogue: Catalogue) -> Arc<Selection> {
    let sel = Arc::new(Selection::new(catalogue));
    let _ = PREPARED.set(Arc::clone(&sel));
    sel
}

/// Where the catalogue is read from: the data directory, and the disc's
/// slots, read once and only when a catalogue needs them.
pub struct Sources {
    pub data_root: std::path::PathBuf,
    pub disc_slots: Box<dyn Fn() -> anyhow::Result<Vec<Vec<u8>>> + Send + Sync>,
    slots: Mutex<Option<Vec<Vec<u8>>>>,
}

impl Sources {
    pub fn new(
        data_root: &Path,
        disc_slots: impl Fn() -> anyhow::Result<Vec<Vec<u8>>> + Send + Sync + 'static,
    ) -> Self {
        Sources {
            data_root: data_root.to_path_buf(),
            disc_slots: Box::new(disc_slots),
            slots: Mutex::new(None),
        }
    }

    fn content_root(&self) -> std::path::PathBuf {
        vfs::Config::new(&self.data_root, crate::new_maps::HALO2_TITLE)
            .title_data_dir()
            .join(vfs::CONTENT_DIR)
    }

    /// Read the catalogue after registering the installed offers.
    pub fn read(&self, overflow: usize) -> anyhow::Result<Catalogue> {
        let content_root = self.content_root();
        let slots = {
            let mut cached = lock(&self.slots);
            if cached.is_none() {
                *cached = Some((self.disc_slots)()?);
            }
            cached.clone().unwrap_or_default()
        };
        Ok(read(&slots, &content_root).with_overflow(overflow))
    }
}

fn summary(cat: &Catalogue) -> String {
    cat.categories()
        .iter()
        .map(|&c| format!("{} {}", c.label(), cat.maps(c).len()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Read the disc's multiplayer slots and the installed content offers at boot.
pub fn prepare_at_boot(data_root: &Path, disc: &Path, overflow: usize) -> Option<Arc<Selection>> {
    use crate::new_maps::Disc as _;
    let disc = disc.to_path_buf();
    let sources = Sources::new(data_root, move || {
        crate::new_maps::IsoDisc::new(&disc).slots()
    });
    let cat = match sources.read(overflow) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("map select: mainmenu.map's map list unread ({e:#}); the retail list");
            Catalogue::default()
        }
    };
    match cat.categories().len() {
        0 => eprintln!("map select: no installed maps"),
        _ => eprintln!("map select: by category ({})", summary(&cat)),
    }
    let index = PackIndex::read(data_root);
    let mut cat = cat;
    cat.packs = index.installed(&cat);
    let sel = prepare(cat);
    *lock(&sel.pack_index) = index;
    let _ = SOURCES.set(sources);
    Some(sel)
}

/// Whether the boot prepared the catalogue ([`packages_changed`] has
/// something to refresh).
pub fn prepared() -> bool {
    PREPARED.get().is_some() && SOURCES.get().is_some()
}

/// Rebuild the offers and catalogue on the download or removal thread.
pub fn packages_changed(disc: &Path) {
    let (Some(sel), Some(sources)) = (PREPARED.get(), SOURCES.get()) else {
        return;
    };
    refresh(sel, sources, || {
        crate::new_maps::install_at_boot(&sources.data_root, disc)
    });
}

static VFS_RESCAN: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

/// The boot's way to make the VFS read the maps folder again
/// ([`publish_packages`]).
pub fn on_vfs_rescan(rescan: Box<dyn Fn() + Send + Sync>) {
    let _ = VFS_RESCAN.set(rescan);
}

/// Whether [`publish_packages`] has everything it needs: the boot prepared
/// the catalogue and gave the VFS's rescan.
pub fn publish_ready() -> bool {
    prepared() && VFS_RESCAN.get().is_some()
}

/// Packages were written under `<data>/maps` while the game runs (the
/// built-in Firefight kits): [`packages_changed`], then the VFS reads the
/// folder again. A no-op before [`publish_ready`].
pub fn publish_packages(disc: &Path) {
    if !publish_ready() {
        return;
    }
    packages_changed(disc);
    if let Some(rescan) = VFS_RESCAN.get() {
        rescan();
    }
}

/// Install and publish the updated set on the caller's thread.
pub fn refresh(sel: &Selection, sources: &Sources, install: impl FnOnce() -> usize) {
    let overflow = install();
    match sources.read(overflow) {
        Ok(cat) => {
            eprintln!("map select: packages changed ({})", summary(&cat));
            sel.set(cat, true);
        }
        Err(e) => eprintln!("map select: packages changed but the list is unread ({e:#})"),
    }
}

// ---- offline ----

/// A catalogue of every category for the previews and tests: names only,
/// ids and paths shaped as the game's.
pub fn sample() -> Catalogue {
    let disc = |id: u32, name: &str, sort: i32| Map {
        id,
        name: name.into(),
        sort,
        path: format!(
            "scenarios\\multi\\{}\\{}",
            name.to_lowercase(),
            name.to_lowercase()
        ),
    };
    let package = |id: u32, offer: u32, name: &str, map: &str, sort: i32| Map {
        id,
        name: name.into(),
        sort,
        path: format!("t:\\$C\\4d530064{offer:08x}\\{map}"),
    };
    let new_map = |id: u32, name: &str, map: &str| {
        package(id, content::map_list::offer_content(id), name, map, 1000)
    };
    Catalogue::new([
        disc(0x50, "Midship", 50),
        disc(0x0a, "Lockout", 10),
        disc(0x14, "Ascension", 20),
        disc(0x1e, "Zanzibar", 30),
        disc(0x28, "Coagulation", 40),
        disc(0x32, "Ivory Tower", 60),
        disc(0x3c, "Beaver Creek", 70),
        package(0x3e8, 0x101, "Containment", "triplicate", 130),
        package(0x3f2, 0x102, "Warlock", "warlock", 140),
        new_map(0x2_0001, "Coldsnap", "coldsnap"),
        new_map(0x2_0002, "Arena", "arena"),
        new_map(0x2_0003, "Blood Gulch", "h1_bloodgulch"),
        new_map(0x2_0004, "Hang 'Em High", "h1_hangemhigh"),
        new_map(0x2_0005, "Battle Creek", "h1_beavercreek"),
    ])
}

/// The two steps as list classes, for the offline model only: the game
/// shows both in the retail Select Map ([`MapSelect`]).
pub fn classes(sel: &Arc<Selection>) -> [ListClass; 2] {
    let categories = ListClass::with_rows(
        "map-categories-model",
        CATEGORIES,
        Arc::new(CategoryRows(Arc::clone(sel))),
    );
    let pick: crate::list_class::Action = Arc::new(|_cx: Cx, _screen: u32| {
        Box::pin(async { Ok(()) })
            as std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), EntryError>> + Send>>
    });
    let maps = ListClass::with_rows("map-list-model", MAPS, Arc::new(MapRows(Arc::clone(sel))))
        .action(pick)
        .action_buttons(&[]);
    [categories, maps]
}

/// The pair as one page: the categories, A opening a category's maps, B
/// back. Named states: the categories, and each category's maps.
pub fn page(sel: &Arc<Selection>) -> StackPage {
    let [categories, maps] = classes(sel);
    let c_spec = {
        let sel = Arc::clone(sel);
        move || CategoryRows(Arc::clone(&sel)).spec()
    };
    let m_spec = {
        let sel = Arc::clone(sel);
        move || MapRows(Arc::clone(&sel)).spec()
    };
    let mut p = StackPage::new(CATEGORIES, ListPage::new(categories, c_spec))
        .screen(MAPS, ListPage::new(maps, m_spec));
    for (n, c) in sel.catalogue().categories().into_iter().enumerate() {
        let mut presses = vec![Input::Down; n];
        presses.push(Input::A);
        p = p.state(&format!("maps-{}", slug(c)), &presses);
    }
    p
}

fn slug(c: Category) -> &'static str {
    match c {
        Category::Halo2 => "halo2",
        Category::MapPack => "mappack",
        Category::Mods => "mods",
        Category::Halo1 => "halo1",
    }
}

/// A director variant selects kit maps inside Custom Games.
pub(crate) fn variant_mode(mode: crate::lobby_start::Mode, variant: Option<&variants::GameVariant>) -> crate::lobby_start::Mode {
    use crate::lobby_start::Mode;
    if mode == Mode::Multiplayer && variant.is_some_and(|v| v.ext().script.mode == variants::ext::Mode::Firefight) {
        Mode::Firefight
    } else {
        mode
    }
}

#[cfg(test)]
mod tests;
