//! The multiplayer map select by category (docs/game/map-select.md): Game
//! Setup's Change Map opens a list of categories (Halo 2, Map Pack, Mods,
//! Halo: CE; an empty one is left out), and a category opens its maps,
//! sorted. A on a map makes it the lobby's map and goes back to Game Setup;
//! B on the maps comes back to the categories, B there to Game Setup.
//!
//! - The model is pure: [`category_of`] reads a map's category from the
//!   path the game loads it from, [`Catalogue`] groups and sorts.
//! - The two screens are list screens of ours ([`crate::list_class`]) built
//!   with the kit, their rows [`CategoryRows`] and [`MapRows`]; the
//!   categories open the maps on their own stack ([`Chosen::Push`]), so B
//!   comes back.
//! - Offline, [`page`] is the pair as one [`StackPage`], stepped by the
//!   table tests and drawn by `h2 ui preview`.
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
use crate::ui_kit::lifecycle::{self, AfterClose};
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
#[derive(Clone, Debug, PartialEq, Eq)]
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
    pub fn category(&self) -> Category {
        category_of(&self.path)
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
    maps.extend(dirs.iter().filter_map(|d| map_of_package(d)));
    Catalogue::new(maps)
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

// ---- the classes ----

/// `0x19a6f2`: sets the lobby's map (map id in ESI, the campaign id on the
/// stack, -1 for multiplayer; stdcall, `ret 4`; AL whether it took). The
/// Change Map list's press (`0x2c9b5d`) calls it so.
pub const SET_LOBBY_MAP: u32 = 0x0019_a6f2;
/// Game Setup's Change Map (`0x2bb8ac`): its request for the map list's
/// loader `0x2bb2db` (`0x149f49`, window 3, slot 4), returning here.
pub const CHANGE_MAP_REQUEST_RET: u32 = 0x002b_b8d7;
pub const CHANGE_MAP_LOADER: u32 = 0x002b_b2db;

/// Work for the maps' screen once it is freed: close the categories under
/// it (the same window and slot).
type CloseUnder = (u32, u32);

/// The two classes over `sel`.
pub fn classes(sel: &Arc<Selection>) -> [ListClass; 2] {
    let categories = ListClass::with_rows(
        CATEGORIES_FEATURE,
        CATEGORIES,
        Arc::new(CategoryRows(Arc::clone(sel))),
    )
    .footer(IDS.id(CATEGORIES_FOOTER));
    let waiting: Arc<AfterClose<CloseUnder>> = Arc::default();
    let pick = {
        let sel = Arc::clone(sel);
        let waiting = Arc::clone(&waiting);
        Arc::new(move |cx: Cx, screen: u32| {
            let sel = Arc::clone(&sel);
            let waiting = Arc::clone(&waiting);
            Box::pin(async move { pick_map(&cx, screen, &sel, &waiting).await })
                as std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), EntryError>> + Send>>
        })
    };
    let freed = Arc::new(move |cx: Cx, screen: u32| {
        let waiting = Arc::clone(&waiting);
        Box::pin(async move {
            if let Some((window, slot)) = waiting.freed(screen) {
                eprintln!("map select: the maps closed; closing the categories");
                lifecycle::close_slot(&cx, window, slot).await?;
            }
            Ok(())
        }) as std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), EntryError>> + Send>>
    });
    let maps = ListClass::with_rows(MAPS_FEATURE, MAPS, Arc::new(MapRows(Arc::clone(sel))))
        .footer(IDS.id(MAPS_FOOTER))
        .action(pick)
        .action_buttons(&[])
        .on_free(freed);
    [categories, maps]
}

/// A on a map: the lobby's map becomes it, as the retail list's press does,
/// then both screens close (the maps now, the categories once the maps are
/// freed).
async fn pick_map(
    cx: &Cx,
    screen: u32,
    sel: &Selection,
    waiting: &AfterClose<CloseUnder>,
) -> Result<(), EntryError> {
    let Some(id) = sel.take_pressed() else {
        return Ok(());
    };
    if waiting.closing(screen) {
        return Ok(());
    }
    let r = cx
        .call(GuestCall::new(SET_LOBBY_MAP).esi(id).args([u32::MAX]))
        .await?;
    if r.eax & 0xff == 0 {
        eprintln!("map select: map {id:#x} was not taken (0x19a6f2 said no); staying");
        return Ok(());
    }
    eprintln!("map select: the lobby's map is now {id:#x}");
    let (window, slot) = lifecycle::window_slot(cx, screen)?;
    waiting.then(screen, (window, slot));
    lifecycle::close_slot(cx, window, slot).await
}

// ---- prepared at boot ----

static PREPARED: OnceLock<Arc<Selection>> = OnceLock::new();
static STRINGS: OnceLock<Vec<(u32, &'static str)>> = OnceLock::new();

/// Keeps the boot's catalogue; [`strings`] and [`installed_classes`] answer
/// after. With one category or none the retail list opens unchanged.
pub fn prepare(catalogue: Catalogue) -> Option<Arc<Selection>> {
    if catalogue.categories().len() < 2 {
        return None;
    }
    let strings = catalogue
        .strings()
        .into_iter()
        // Once a boot: the labels live as long as the game.
        .map(|(id, t)| (id, &*Box::leak(t.into_boxed_str())))
        .collect();
    let _ = STRINGS.set(strings);
    let sel = Arc::new(Selection::new(catalogue));
    let _ = PREPARED.set(Arc::clone(&sel));
    Some(sel)
}

/// The two screens' strings, once [`prepare`]d.
pub fn strings() -> Vec<(u32, &'static str)> {
    STRINGS.get().cloned().unwrap_or_default()
}

/// The two classes, once [`prepare`]d.
pub fn installed_classes() -> Option<[ListClass; 2]> {
    Some(classes(PREPARED.get()?))
}

/// Reads the list at boot (the disc's `mainmenu.map` slots, then the
/// content packages under `<data>/disk/.../$c`) and [`prepare`]s it. Never
/// fails: says why the retail list stays.
pub fn prepare_at_boot(data_root: &Path, disc: &Path) -> Option<Arc<Selection>> {
    use crate::new_maps::Disc as _;
    let content_root = vfs::Config::new(data_root, crate::new_maps::HALO2_TITLE)
        .title_data_dir()
        .join(vfs::CONTENT_DIR);
    // Nothing installed: one category, the retail list; the disc unread.
    let any = std::fs::read_dir(&content_root)
        .map(|d| d.flatten().any(|e| e.path().join("patch.lvl").is_file()))
        .unwrap_or(false);
    if !any {
        return None;
    }
    let slots = match crate::new_maps::IsoDisc::new(disc).slots() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("map select: mainmenu.map's map list unread ({e:#}); the retail list");
            return None;
        }
    };
    let cat = read(&slots, &content_root);
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

/// The hook: Game Setup's Change Map opens the categories in place of the
/// retail map list (its request's loader swapped, as the campaign picker
/// does the level select's; window 3 kept).
/// The feature id.
pub const FEATURE: &str = "map-select";

pub struct MapSelect {
    /// The categories' loader, once their class is built (0: the retail
    /// list opens).
    pub loader: Arc<dyn Fn() -> u32 + Send + Sync>,
}

impl feature::Feature for MapSelect {
    fn manifest(&self) -> feature::Manifest {
        feature::Manifest::new(FEATURE, "The multiplayer map select by category")
            .doc("docs/game/map-select.md")
    }
    fn build(&self, b: &mut feature::Builder) -> Result<(), feature::BuildError> {
        use engine::points::SCREEN_REQUEST;
        let loader = Arc::clone(&self.loader);
        b.raw().before(&SCREEN_REQUEST, "the map categories first", move |c| {
            let rt = Arc::clone(c.ctx.runtime());
            let mut mem = rt.memory();
            let word = |m: &dyn engine::memory::GuestMemory, n: u32| m.u32(c.cpu.esp + 4 * n).ok();
            let (Some(ret), Some(was)) = (word(&*mem, 0), word(&*mem, 4)) else {
                return Ok(feature::Flow::Continue);
            };
            if let Some(ours) = request_loader(ret, was, loader())
                && mem.set_u32(c.cpu.esp + 16, ours).is_ok()
            {
                eprintln!("map select: opening the map categories in place of the map list");
            }
            Ok(feature::Flow::Continue)
        })
        // The campaign picker swaps another request's loader at the same
        // point (`campaign_menu`); the two never act on the same request
        // (return address and loader differ), so either order is right,
        // but the plan needs one: ours sits inside theirs.
        .after_feature(crate::campaigns::FEATURE);
        Ok(())
    }
}

/// The loader a `0x149f49` request returning to `ret` for `loader` should
/// open instead: the categories' (`ours`, 0 until their class is built)
/// for Game Setup's Change Map, else none.
pub fn request_loader(ret: u32, loader: u32, ours: u32) -> Option<u32> {
    (ret == CHANGE_MAP_REQUEST_RET && loader == CHANGE_MAP_LOADER && ours != 0).then_some(ours)
}

/// Adds the hook when the boot [`prepare`]d a catalogue. Returns whether it
/// was added.
pub fn install(registry: &mut feature::Registry) -> anyhow::Result<bool> {
    if PREPARED.get().is_none() {
        return Ok(false);
    }
    let loader: Arc<dyn Fn() -> u32 + Send + Sync> =
        Arc::new(|| crate::list_class::loader_of(CATEGORIES).unwrap_or(0));
    let added = registry
        .add(&MapSelect { loader })
        .map_err(|e| anyhow::anyhow!("map select: {e}"))?;
    if !added {
        eprintln!("map select: features.map-select is off; the retail map list");
    }
    Ok(added)
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
