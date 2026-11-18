//! The multiplayer map select by category (docs/game/map-select.md): Game
//! Setup's Change Map opens a list of categories (Halo 2, Map Pack, Mods,
//! Halo: CE; an empty one is left out), and a category opens its maps,
//! sorted. A on a map makes it the lobby's map and goes back to Game Setup;
//! B on the maps comes back to the categories, B there to Game Setup.
//!
//! - The model is pure: [`category_of`] reads a map's category from the
//!   path the game loads it from, [`Catalogue`] groups and sorts.
//! - In the game both steps are the retail Select Map dialog (screen
//!   `0xce`), with its pictures and descriptions ([`MapSelect`]): first in
//!   category mode (a row a category, standing on its first map, labelled
//!   with the category), then a second Select Map pushed on the same stack
//!   with the category's maps. Each lists an array of ours in place of the
//!   retail one.
//! - Offline, [`page`] models the two steps as one [`StackPage`] over
//!   [`CategoryRows`] and [`MapRows`], stepped by the table tests.
//!
//! The catalogue is read at boot ([`read`]): `mainmenu.map`'s multiplayer
//! list from the disc, then each content package's `patch.lvl`, as 1.0
//! fills the list itself (`0x19c120`, docs/game/map-pack.md 8.1).

use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use engine::entry::{Cx, EntryError, GuestCall};

use crate::list_class::{ListClass, Rows};
use crate::ui::Chosen;
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
const CATEGORIES_FOOTER: u32 = 5;
const MAPS_FOOTER: u32 = 6;
const MAP_LABELS: u32 = 7;
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
pub const MAP_PACK_OFFERS: std::ops::RangeInclusive<u32> = 0x0101..=0x03ff;
/// Converted Halo 1 maps' names (docs/game/h1-multiplayer.md "Names").
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
        Some(low)
            if content::map_list::is_new_map_offer(low) && map.starts_with(HALO1_PREFIX) =>
        {
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
    /// A disc map a package in `<data>/maps` replaces: the disc's own name
    /// (the slot's), `name` being the package's ([`Catalogue::with_mods`]).
    pub replaces: Option<String>,
}

impl Map {
    /// [`category_of`] its path, but Mods for a disc map a package replaces.
    pub fn category(&self) -> Category {
        match self.replaces {
            Some(_) => Category::Mods,
            None => category_of(&self.path),
        }
    }

    /// The map file's name: the path's basename, lower case.
    pub fn file(&self) -> String {
        self.path
            .rsplit('\\')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase()
    }
}

/// The maps by category: categories in [`Category::ALL`] order, the empty
/// ones left out; maps in the game's order ([`Category::game_order`]) or by
/// name, then id. A map id listed twice keeps its last entry, as a package
/// whose record names a slot's id refills that slot (`0x19be00`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Catalogue {
    groups: Vec<(Category, Vec<Map>)>,
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
                let mut maps: Vec<Map> =
                    by_id.iter().filter(|m| m.category() == c).cloned().collect();
                if c.game_order() {
                    maps.sort_by(|a, b| (a.sort, a.id).cmp(&(b.sort, b.id)));
                } else {
                    maps.sort_by(|a, b| {
                        (a.name.to_lowercase(), a.id).cmp(&(b.name.to_lowercase(), b.id))
                    });
                }
                (!maps.is_empty()).then_some((c, maps))
            })
            .collect();
        Catalogue { groups }
    }

    /// [`Catalogue::new`], with the disc maps a package replaces
    /// (`mods`: map file name, the package's name) listed under Mods by
    /// the package's name. The vfs serves one map per name
    /// (docs/game/maps.md §7, "Choice rule"), so while a package is chosen
    /// for a disc map its slot loads the package, and the disc's own map
    /// can't be offered beside it.
    pub fn with_mods(maps: impl IntoIterator<Item = Map>, mods: &[(String, String)]) -> Self {
        Self::new(maps.into_iter().map(|mut m| {
            if category_of(&m.path) == Category::Halo2
                && let Some((_, package)) = mods.iter().find(|(f, _)| f.eq_ignore_ascii_case(&m.file()))
            {
                m.replaces = Some(std::mem::replace(&mut m.name, package.clone()));
            }
            m
        }))
    }

    /// The categories with maps, in order.
    pub fn categories(&self) -> Vec<Category> {
        self.groups.iter().map(|(c, _)| *c).collect()
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
            s.push((IDS.id(CATEGORY_LABELS + c.index()), c.label().to_string()));
        }
        for (n, m) in self.all().enumerate() {
            s.push((IDS.id(MAP_LABELS + n as u32), m.name.clone()));
        }
        s
    }
}

// ---- reading the list ----

/// Slot offsets (map-pack.md 8.1): id, English name (32 UTF-16 units), the
/// scenario path, the sort order.
const SLOT_NAME: usize = 0xc;
const NAME_UNITS: usize = 32;
const SLOT_PATH: usize = 0xb4c;
const PATH_LEN: usize = 0x100;
const SLOT_SORT: usize = 0xc4c;

/// A used slot of `mainmenu.map`'s multiplayer list as a [`Map`]; `None`
/// for a free one (id `-1`) or a short one.
pub fn map_of_slot(slot: &[u8]) -> Option<Map> {
    let slot = slot.get(..content::map_list::SLOT_LEN)?;
    let u32_at = |at: usize| u32::from_le_bytes(slot[at..at + 4].try_into().unwrap());
    let id = u32_at(0);
    if id == u32::MAX {
        return None;
    }
    let units: Vec<u16> = slot[SLOT_NAME..SLOT_NAME + NAME_UNITS * 2]
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .take_while(|&u| u != 0)
        .collect();
    let path = &slot[SLOT_PATH..SLOT_PATH + PATH_LEN];
    let path = &path[..path.iter().position(|&b| b == 0).unwrap_or(PATH_LEN)];
    Some(Map {
        id,
        name: String::from_utf16_lossy(&units),
        sort: u32_at(SLOT_SORT) as i32,
        path: String::from_utf8_lossy(path).into_owned(),
        replaces: None,
    })
}

/// The map package `dir` (a `$c\<offer ID>` directory) adds to the list:
/// its `patch.lvl` record, at the path 1.0 gives it
/// (`t:\$C\<offer ID>\<basename of the record's scenario>`).
pub fn map_of_package(dir: &Path) -> Option<Map> {
    let offer = dir.file_name()?.to_str()?;
    let bytes = std::fs::read(dir.join("patch.lvl")).ok()?;
    let e = content::map_list::Entry::from_v2(bytes.get(..content::patch_lvl::V2_LEN)?).ok()?;
    let base = e.scenario.rsplit('\\').next().unwrap_or(&e.scenario).to_owned();
    Some(Map {
        id: e.map_id,
        name: e.names[0].clone(),
        sort: e.sort,
        path: format!("t:\\$C\\{offer}\\{base}"),
        replaces: None,
    })
}

/// The list as 1.0 will have it: the disc's used slots, then each content
/// package's record under `content_root` (`TDATA\4D530064\$c`), in
/// directory order.
pub fn read(slots: &[Vec<u8>], content_root: &Path) -> Catalogue {
    read_with_mods(slots, content_root, &[])
}

/// [`read`], with the disc maps packages replace listed under Mods
/// ([`Catalogue::with_mods`]).
pub fn read_with_mods(slots: &[Vec<u8>], content_root: &Path, mods: &[(String, String)]) -> Catalogue {
    let mut maps: Vec<Map> = slots.iter().filter_map(|s| map_of_slot(s)).collect();
    let mut dirs: Vec<_> = std::fs::read_dir(content_root)
        .map(|d| d.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    dirs.sort();
    maps.extend(dirs.iter().filter_map(|d| map_of_package(d)));
    Catalogue::with_mods(maps, mods)
}

// ---- the screens ----

/// What the two screens share: the catalogue, the category picked, and
/// the map pressed.
#[derive(Default)]
pub struct Selection {
    pub catalogue: Catalogue,
    picked: Mutex<Option<Category>>,
    pressed: Mutex<Option<u32>>,
}

impl Selection {
    pub fn new(catalogue: Catalogue) -> Self {
        Selection {
            catalogue,
            ..Default::default()
        }
    }
    /// The category whose maps the maps' screen lists: the one picked, else
    /// the first.
    pub fn category(&self) -> Option<Category> {
        let picked = *self.picked.lock().unwrap_or_else(|e| e.into_inner());
        picked.or_else(|| self.catalogue.categories().first().copied())
    }
    fn pick(&self, c: Category) {
        *self.picked.lock().unwrap_or_else(|e| e.into_inner()) = Some(c);
    }
    /// The maps the maps' screen lists.
    pub fn maps(&self) -> &[Map] {
        self.category().map_or(&[], |c| self.catalogue.maps(c))
    }
    /// The map last pressed on the maps' screen, taken.
    pub fn take_pressed(&self) -> Option<u32> {
        self.pressed.lock().unwrap_or_else(|e| e.into_inner()).take()
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
    fn title(&self) -> u32 {
        IDS.id(TITLE)
    }
    fn count(&self) -> usize {
        self.0.catalogue.categories().len()
    }
    fn texts(&self, row: i16) -> Vec<(u32, u32)> {
        let cats = self.0.catalogue.categories();
        match usize::try_from(row).ok().and_then(|r| cats.get(r)) {
            Some(c) => vec![(0, IDS.id(CATEGORY_LABELS + c.index()))],
            None => Vec::new(),
        }
    }
    fn press(&self, row: i16) -> Chosen {
        let cats = self.0.catalogue.categories();
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
        for c in self.0.catalogue.categories() {
            s.rows.push(
                RowSpec::action(c.label())
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
                .catalogue
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
            let id = self.0.catalogue.label_id(m.id).unwrap_or(0);
            s.rows.push(RowSpec::action(&m.name).press(Press::Act).ids(&[id]));
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

/// Game Setup's Change Map (`0x2bb8ac`): its request for Select Map's
/// loader `0x2bb2db` (`0x149f49`, window 3, slot 4), returning here.
pub const CHANGE_MAP_REQUEST_RET: u32 = 0x002b_b8d7;
pub const CHANGE_MAP_LOADER: u32 = 0x002b_b2db;
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
}

/// The element ids and lock bytes a Select Map in `mode` is built from
/// (`flags`: the retail array's byte `+2` by map id, which the list's
/// constructor tests with the profiles' progress at `0x2c99cc`).
pub fn list_for(cat: &Catalogue, mode: Mode, flags: &dyn Fn(u32) -> u8) -> Vec<(u32, u8)> {
    match mode {
        Mode::Categories => cat
            .categories()
            .iter()
            .filter_map(|&c| cat.maps(c).first().map(|m| (m.id, 0)))
            .collect(),
        Mode::Maps(c) => cat.maps(c).iter().map(|m| (m.id, flags(m.id))).collect(),
    }
}

/// The level-array shape a list is built from: 8-byte elements, the
/// lock byte at `+2`, the map id at `+4`.
const ELEMENT: i32 = 8;
const CAPACITY: i32 = 64;

/// Writes a data array at `at` (named by the string at `name`) holding
/// `items`, as `0x19c7e0` fills the retail one.
pub fn write_list(
    m: &mut dyn engine::memory::GuestMemory,
    at: u32,
    name: u32,
    items: &[(u32, u8)],
) -> Result<(), engine::memory::MemFault> {
    let a = engine::datum::DatumArray::at(at);
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
pub fn read_flags(m: &dyn engine::memory::GuestMemory, array: u32) -> Vec<(u32, u8)> {
    let a = engine::datum::DatumArray::at(array);
    let mut out = Vec::new();
    let Ok(hw) = a.header_field(m, engine::datum::layout::HIGH_WATER) else {
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

/// The flow over Select Map screens: what the next one built lists, and
/// what each built one is.
#[derive(Default)]
pub struct Flow {
    /// The next Select Map built lists this (`None`: retail).
    next: Mutex<Option<Mode>>,
    /// Built screens of ours, by list handler.
    screens: Mutex<Vec<(u32, Mode)>>,
    /// The list handler whose rows are being drawn (`0x2c9ac4`).
    drawing: std::sync::atomic::AtomicU32,
    /// Maps screens whose map was pressed (by list handler): the
    /// categories' screen under it, closed once that screen is freed.
    closing: Mutex<Vec<(u32, u32)>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Flow {
    /// Change Map was pressed: the next Select Map lists the categories.
    pub fn change_map(&self) {
        *lock(&self.next) = Some(Mode::Categories);
    }
    /// A Select Map list (`handler`) is being built: what it lists, now
    /// taken.
    pub fn building(&self, handler: u32) -> Option<Mode> {
        let mode = lock(&self.next).take()?;
        let mut s = lock(&self.screens);
        s.retain(|(h, _)| *h != handler);
        s.push((handler, mode));
        Some(mode)
    }
    pub fn mode_of(&self, handler: u32) -> Option<Mode> {
        lock(&self.screens).iter().find(|(h, _)| *h == handler).map(|(_, m)| *m)
    }
    /// A category was pressed: the Select Map pushed next lists its maps.
    pub fn pick(&self, c: Category) {
        *lock(&self.next) = Some(Mode::Maps(c));
    }
    /// A map was pressed on `handler`'s screen: the categories' screen
    /// under it (the latest categories built) closes once it is freed.
    /// Returns that screen; `None` without one.
    pub fn picked(&self, handler: u32) -> Option<u32> {
        let categories = lock(&self.screens)
            .iter()
            .rev()
            .find(|(_, m)| *m == Mode::Categories)
            .map(|(h, _)| h.wrapping_sub(SCREEN_LIST))?;
        let mut c = lock(&self.closing);
        c.retain(|(h, _)| *h != handler);
        c.push((handler, categories));
        Some(categories)
    }
    /// `screen` is being freed: forget it; the categories' screen to
    /// close, if a map was pressed on it.
    pub fn freed(&self, screen: u32) -> Option<u32> {
        let handler = screen.wrapping_add(SCREEN_LIST);
        lock(&self.screens).retain(|(h, _)| *h != handler);
        let mut c = lock(&self.closing);
        let i = c.iter().position(|(h, _)| *h == handler)?;
        Some(c.remove(i).1)
    }
}

/// UTF-16 labels in our memory: one 0x40-byte slot each (the slots' own
/// name width, 32 units), the four categories first, then the maps.
const LABEL_LEN: u32 = 0x40;

/// The label a row shows instead of its slot's name: the category's in
/// category mode, a package's name for a disc map it replaces. Its index
/// in the label table, and the text.
pub fn row_label(cat: &Catalogue, mode: Mode, id: u32) -> Option<(u32, String)> {
    match mode {
        Mode::Categories => {
            let c = cat
                .categories()
                .into_iter()
                .find(|&c| cat.maps(c).first().is_some_and(|m| m.id == id))?;
            Some((c.index(), c.label().to_string()))
        }
        Mode::Maps(_) => {
            let n = cat.all().position(|m| m.id == id)?;
            let m = cat.all().nth(n)?;
            m.replaces.as_ref()?;
            Some((4 + n as u32, m.name.clone()))
        }
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

/// Our memory: the list array, its name, then the labels.
const ARRAY_AT: u32 = 0;
const NAME_AT: u32 = 0x400;
const LABELS_AT: u32 = 0x1000;
const REGION: u32 = 0x3000;

/// The hooks (feature `map-select`).
pub struct MapSelect {
    pub sel: Arc<Selection>,
    pub flow: Arc<Flow>,
}

/// The feature id.
pub const FEATURE: &str = "map-select";

impl feature::Feature for MapSelect {
    fn manifest(&self) -> feature::Manifest {
        feature::Manifest::new(FEATURE, "The multiplayer map select by category")
            .doc("docs/game/map-select.md")
    }
    fn build(&self, b: &mut feature::Builder) -> Result<(), feature::BuildError> {
        use engine::points::{
            MAP_LIST_ROW_CHOSEN, MAP_LIST_ROW_TEXT, MAP_SCREEN_FREE, MP_LEVEL_ARRAY, MP_LEVEL_NAME,
            SCREEN_REQUEST,
        };
        use feature::Flow as F;
        let region = b.guest_alloc(REGION, runtime::guest::Permissions::RW)?;

        // Change Map: arm the next Select Map for the categories.
        let flow = Arc::clone(&self.flow);
        b.raw().observe_before(&SCREEN_REQUEST, "change map arms the categories", move |o| {
            let mem = o.guest();
            let word = |n: u32| mem.u32(o.cpu.esp + 4 * n).ok();
            if word(0) == Some(CHANGE_MAP_REQUEST_RET) && word(4) == Some(CHANGE_MAP_LOADER) {
                flow.change_map();
            }
        });

        // The list's array: ours for a Select Map of ours.
        let (flow, sel) = (Arc::clone(&self.flow), Arc::clone(&self.sel));
        b.raw().before(&MP_LEVEL_ARRAY, "a Select Map of ours lists ours", move |c| {
            let rt = Arc::clone(c.ctx.runtime());
            {
                let mem = rt.memory();
                if mem.u32(c.cpu.esp).ok() != Some(LIST_ARRAY_RET)
                    || mem.u32(LEVEL_ARRAY).unwrap_or(0) == 0
                {
                    return Ok(F::Continue);
                }
            }
            let Some(mode) = flow.building(c.cpu.esi) else {
                return Ok(F::Continue);
            };
            let mut mem = rt.memory();
            let retail = mem.u32(LEVEL_ARRAY).unwrap_or(0);
            let flags = read_flags(&*mem, retail);
            let flag = |id: u32| flags.iter().find(|(i, _)| *i == id).map_or(0, |(_, f)| *f);
            let items = list_for(&sel.catalogue, mode, &flag);
            let written = engine::memory::GuestMemory::write(
                &mut *mem,
                region + NAME_AT,
                b"map select list\0",
            )
            .and_then(|()| write_list(&mut *mem, region + ARRAY_AT, region + NAME_AT, &items));
            drop(mem);
            match written {
                Ok(()) => {
                    eprintln!("map select: Select Map lists {mode:?} ({} rows)", items.len());
                    c.cpu.eax = region + ARRAY_AT;
                    c.ret(0).map(F::Return)
                }
                Err(e) => {
                    eprintln!("map select: the list was not written ({e:?}); the retail list");
                    Ok(F::Continue)
                }
            }
        });

        // Which list is drawing its rows.
        let flow = Arc::clone(&self.flow);
        b.raw().observe_before(&MAP_LIST_ROW_TEXT, "note the list drawing", move |o| {
            flow.drawing.store(o.cpu.ecx, std::sync::atomic::Ordering::SeqCst);
        });

        // A row's name: the category's, or a replacing package's.
        let (flow, sel) = (Arc::clone(&self.flow), Arc::clone(&self.sel));
        b.raw().before(&MP_LEVEL_NAME, "category and package names", move |c| {
            let rt = Arc::clone(c.ctx.runtime());
            let mut mem = rt.memory();
            if mem.u32(c.cpu.esp).ok() != Some(ROW_NAME_RET) {
                return Ok(F::Continue);
            }
            let handler = flow.drawing.load(std::sync::atomic::Ordering::SeqCst);
            let Some(mode) = flow.mode_of(handler) else {
                return Ok(F::Continue);
            };
            let Some(id) = mem.u32(c.cpu.esp + 4).and_then(|slot| mem.u32(slot)).ok() else {
                return Ok(F::Continue);
            };
            let Some((n, text)) = row_label(&sel.catalogue, mode, id) else {
                return Ok(F::Continue);
            };
            let at = region + LABELS_AT + n * LABEL_LEN;
            if engine::memory::GuestMemory::write(&mut *mem, at, &utf16z(&text)).is_err() {
                return Ok(F::Continue);
            }
            drop(mem);
            c.cpu.eax = at;
            c.ret(4).map(F::Return)
        });

        // A press: a category pushes its maps; a map goes on as retail and
        // the categories close once its screen is freed.
        let (flow, sel) = (Arc::clone(&self.flow), Arc::clone(&self.sel));
        let request_at = region + NAME_AT + 0x100;
        b.raw().before(&MAP_LIST_ROW_CHOSEN, "categories open their maps", move |c| {
            let handler = c.cpu.ecx;
            let Some(mode) = flow.mode_of(handler) else {
                return Ok(F::Continue);
            };
            let screen = handler.wrapping_sub(SCREEN_LIST);
            let (id, chain, stack) = {
                let mem = c.ctx.runtime().memory();
                let chain = mem.u32(c.cpu.esp + 4).ok();
                let row = mem.u32(c.cpu.esp + 8).and_then(|p| mem.u32(p)).ok();
                let id = row.filter(|r| *r != u32::MAX).and_then(|r| {
                    let data = mem.u32(mem.u32(handler + 0x70).ok()? + 0x44).ok()?;
                    mem.u32(data + (r & 0xffff) * 8 + 4).ok()
                });
                let stack = (
                    mem.u32(screen + lifecycle::SCREEN_WINDOW).ok(),
                    mem.u32(screen + lifecycle::SCREEN_SLOT).ok(),
                );
                (id, chain, stack)
            };
            let (Some(id), (Some(window), Some(slot))) = (id, stack) else {
                return Ok(F::Continue);
            };
            match mode {
                Mode::Maps(_) => {
                    // The categories under this screen.
                    match flow.picked(handler) {
                        Some(_) => {
                            eprintln!("map select: map {id:#x} pressed; the categories close after")
                        }
                        None => eprintln!("map select: map {id:#x} pressed; no categories under it"),
                    }
                    Ok(F::Continue)
                }
                Mode::Categories => {
                    let Some(cat) = sel
                        .catalogue
                        .categories()
                        .into_iter()
                        .find(|&k| sel.catalogue.maps(k).first().is_some_and(|m| m.id == id))
                    else {
                        return c.ret(8).map(F::Return);
                    };
                    flow.pick(cat);
                    let mask = {
                        let mem = c.ctx.runtime().memory();
                        chain.and_then(|ch| crate::list_class::player_mask(&*mem, ch).ok().flatten())
                    };
                    let Some(mask) = mask else {
                        return c.ret(8).map(F::Return);
                    };
                    eprintln!(
                        "map select: {} picked; its maps on window {window}, slot {slot}",
                        cat.label()
                    );
                    let request =
                        crate::skulls_mp::open_request(mask as u16, window, slot, CHANGE_MAP_LOADER);
                    c.answer_async(move |cx| -> engine::entry::EntryFuture {
                        Box::pin(async move {
                            cx.write(|m| m.write(request_at, &request))?;
                            cx.call(GuestCall::new(CHANGE_MAP_LOADER).args([request_at]))
                                .await?;
                            Ok(engine::entry::Ret::new(0, 8))
                        })
                    })
                }
            }
        });

        // A maps screen freed after its press: close the categories.
        let flow = Arc::clone(&self.flow);
        b.raw().before(&MAP_SCREEN_FREE, "the categories close after a map", move |c| {
            let screen = c.cpu.ecx;
            let Some(categories) = flow.freed(screen) else {
                return Ok(F::Continue);
            };
            let flags = {
                let mem = c.ctx.runtime().memory();
                mem.u32(c.cpu.esp + 4).unwrap_or(0)
            };
            c.answer_async(move |cx| -> engine::entry::EntryFuture {
                Box::pin(async move {
                    // The destructor itself (this hook passes it now: the
                    // screen is forgotten), then the close. Not by window
                    // and slot: mid-free, the window's top word is 0 and
                    // that close read 0x42 (lifecycle::close_screen).
                    let r = cx
                        .call(GuestCall::new(MAP_SCREEN_FREE_AT).ecx(screen).args([flags]))
                        .await?;
                    eprintln!("map select: the maps closed; closing the categories");
                    lifecycle::close_screen(&cx, categories).await?;
                    Ok(engine::entry::Ret::new(r.eax, 4))
                })
            })
        });
        Ok(())
    }
}

const MAP_SCREEN_FREE_AT: u32 = 0x002b_b384;

/// Adds the hooks when the boot [`prepare`]d a catalogue. Returns whether
/// they were added.
pub fn install(registry: &mut feature::Registry) -> anyhow::Result<bool> {
    let Some(sel) = PREPARED.get() else {
        return Ok(false);
    };
    let added = registry
        .add(&MapSelect {
            sel: Arc::clone(sel),
            flow: Arc::default(),
        })
        .map_err(|e| anyhow::anyhow!("map select: {e}"))?;
    if !added {
        eprintln!("map select: features.map-select is off; the retail map list");
    }
    Ok(added)
}

// ---- prepared at boot ----

static PREPARED: OnceLock<Arc<Selection>> = OnceLock::new();

/// Keeps the boot's catalogue for [`install`]. With one category or none
/// the retail list opens unchanged.
pub fn prepare(catalogue: Catalogue) -> Option<Arc<Selection>> {
    if catalogue.categories().len() < 2 {
        return None;
    }
    let sel = Arc::new(Selection::new(catalogue));
    let _ = PREPARED.set(Arc::clone(&sel));
    Some(sel)
}

/// The disc maps a package in `<data>/maps` replaces: `(map file name,
/// package name)` for each name whose chosen package (the vfs's choice
/// rule, `Library::choose`) isn't a derived (new) map.
pub fn replacing_packages(data_root: &Path) -> Vec<(String, String)> {
    let maps = crate::map_override::packages_dir(data_root);
    if !maps.is_dir() {
        return Vec::new();
    }
    let (store, _) = crate::setting_defs::load(data_root, &[]);
    let sandbox = crate::setting_defs::halo1_sandbox(&store);
    let mut lib = content::Library::open_with_sandbox(&maps, Some(sandbox));
    let names: std::collections::BTreeSet<String> = lib
        .packages()
        .iter()
        .filter(|p| !p.manifest.base.derived)
        .map(|p| p.manifest.base.map.to_ascii_lowercase())
        .collect();
    let mut out = Vec::new();
    for name in names {
        if let Some(p) = lib.choose(&name).filter(|p| !p.manifest.base.derived) {
            out.push((name, p.manifest.package.name.clone()));
        }
    }
    out
}

/// Reads the list at boot (the disc's `mainmenu.map` slots, then the
/// content packages under `<data>/disk/.../$c`, and the packages that
/// replace disc maps) and [`prepare`]s it. Never fails: says why the retail
/// list stays.
pub fn prepare_at_boot(data_root: &Path, disc: &Path) -> Option<Arc<Selection>> {
    use crate::new_maps::Disc as _;
    let content_root = vfs::Config::new(data_root, crate::new_maps::HALO2_TITLE)
        .title_data_dir()
        .join(vfs::CONTENT_DIR);
    let mods = replacing_packages(data_root);
    // Nothing installed: one category, the retail list; the disc unread.
    let any = std::fs::read_dir(&content_root)
        .map(|d| d.flatten().any(|e| e.path().join("patch.lvl").is_file()))
        .unwrap_or(false);
    if !any && mods.is_empty() {
        return None;
    }
    let slots = match crate::new_maps::IsoDisc::new(disc).slots() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("map select: mainmenu.map's map list unread ({e:#}); the retail list");
            return None;
        }
    };
    let cat = read_with_mods(&slots, &content_root, &mods);
    let line: Vec<String> = cat
        .categories()
        .iter()
        .map(|&c| format!("{} {}", c.label(), cat.maps(c).len()))
        .collect();
    let sel = prepare(cat);
    match &sel {
        Some(_) => eprintln!("map select: by category ({})", line.join(", ")),
        None => eprintln!("map select: one category; the retail list"),
    }
    sel
}

// ---- offline ----

/// A catalogue of every category for the previews and tests: names only,
/// ids and paths shaped as the game's.
pub fn sample() -> Catalogue {
    let disc = |id: u32, name: &str, sort: i32| Map {
        id,
        name: name.into(),
        sort,
        path: format!("scenarios\\multi\\{}\\{}", name.to_lowercase(), name.to_lowercase()),
        replaces: None,
    };
    let package = |id: u32, offer: u32, name: &str, map: &str, sort: i32| Map {
        id,
        name: name.into(),
        sort,
        path: format!("t:\\$C\\4d530064{offer:08x}\\{map}"),
        replaces: None,
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
    for (n, c) in sel.catalogue.categories().into_iter().enumerate() {
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

#[cfg(test)]
mod tests;
