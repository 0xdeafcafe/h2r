//! The mod repository in game (docs/game/maps.md §5, "Mod downloads"): the
//! Xbox Live menu's last row, where retail had Content Download, says
//! "Mod Downloader" (or "Mod Downloader (3 new)") and opens a list of the maps the Live server
//! publishes, newest first; each has a page of its own.
//!
//! **The list** is laid out as the level select is: the maps on the left,
//! the focused map's picture on the right and its summary under it (base
//! map, who published it, size, downloads, what it changes, its state).
//! A opens the map's page; X downloads the focused map there and then.
//! Every row is something to press: with no list, its one row is Try
//! Again (or Refresh), and why is in the description. A row's state is a
//! mark after its name ([`mark`]): a star when new, a tick when installed,
//! an arrow and the percentage while downloading.
//!
//! **Map packs.** Maps listed with the same pack (`ModEntry::pack`: the
//! server's `pack`, else the credit's `release`) are one row of the list,
//! at its newest map's place, saying how many maps it has. A opens the
//! pack's list ([`PACK`], the same layout): Download All first, its
//! description the pack's summary (how many maps and by whom, what it is,
//! its size and how much is here), then its maps, each opening its page as
//! the list's rows do. X on the pack's row, or on Download All, downloads
//! every map of it not here yet.
//!
//! **A map's page** (docs/game/maps.md §5, "A mod's page") is laid out as
//! the pregame lobby is ([`page_layout`]): its one row, Download (or
//! Remove, once it is here), where the lobby's buttons are, and nothing
//! else selectable. Under it, on the lobby's panel, the mod's description
//! and a line saying what A does now; under that the base map's picture.
//! In the lobby's table column: a table of facts (base map, who made it,
//! size, downloads, date, version, game types, which disc, joins), then the
//! checklist of what it changes (`live_proto::maps::FEATURES`). A or X
//! downloads; on an installed map they remove it, after a second press.
//!
//! **The picture** is the base map's slot picture (`mod_slots::pictures`),
//! which is the mod's own once it is installed and chosen for its map (the
//! slot takes the package's preview). A map not downloaded yet shows its
//! base map's.
//!
//! **New.** An entry is new when it was published after the last-seen
//! position (`<data>/live-mods.toml`, `seen = N`). Opening the screen marks
//! everything listed as seen; the entries stay tagged "New" for that visit.
//!
//! **Off the game thread.** A thread of ours asks the HLE for the list
//! once the guest signs in to Live. The Live worker reads it with that
//! account's token when the menu is drawn or the screen opens, and every
//! five minutes. Downloads run on a thread of their own. The screens only
//! read the state ([`Repo`]) and every text is ours, written into guest
//! memory slots when it changes (on the frame tick and before a row's texts
//! are set).
//!
//! The state is pure over a [`Source`] and tested without a guest or a
//! server.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use content::MapId;
use guest_layout::memory::{GuestMemory, MemFault};
use live_hle::maps::Refusal;
use live_hle::mods::{self, InstallDone, ListDone, ModEntry, ModPage, ModsError, Progress};
use live_proto::maps::FEATURES;
use runtime::guest::Permissions;

use crate::list_class::{Action, ListClass, LiveText, Rows};
use crate::ui::Chosen;
use crate::ui_kit::ids::Family;
use crate::ui_kit::input::{Glyph, Prompt, footer_legend};
use crate::ui_kit::text::join_sentences;

/// The screens' ids (`list_class::loader_of`).
pub const SCREEN: &str = "mods";
pub const DETAIL: &str = "mods.detail";
/// A map pack's own list: its maps, and Download All.
pub const PACK: &str = "mods.pack";
/// The lobby's neutral map picture at the level select's picture position.
const LIST_PICTURE: crate::list_class::PictureSpec = crate::list_class::PictureSpec {
    at: crate::campaign_menu::PICTURE.at,
    animation: crate::campaign_menu::PICTURE.animation,
    ..page_layout::PICTURE
};
/// The feature id of the texts (`H2_FEATURES=-mods-screens`).
pub const FEATURE: &str = "mods-screens";
/// What a list asks for: the newest this many.
pub const LIMIT: u32 = 40;
/// Where the last-seen position is kept, under the data directory.
pub const SEEN_FILE: &str = "live-mods.toml";
/// How often the list is read again while nothing asks.
pub const REFRESH_EVERY: Duration = Duration::from_secs(300);
/// How soon a failed read is tried again.
pub const RETRY_AFTER: Duration = Duration::from_secs(15);
/// The least time between two reads someone asked for ([`Repo::want_list`]
/// is asked each time the Live menu draws the row).
pub const MIN_GAP: Duration = Duration::from_secs(30);

const TEXTS: Family = crate::ui_kit::ids::family("mod texts");
/// A slot's bytes: room for a description of five lines.
pub const SLOT_LEN: u32 = 256;
/// Slot layout: the Live menu's label, the three titles and footers, then
/// a line and a description per row of the list and of the page, the
/// page's panel texts ([`page_layout`]), then a line and a description per
/// row of a pack's list.
const MENU_SLOT: u32 = 0;
const LIST_TITLE: u32 = 1;
const LIST_FOOTER: u32 = 2;
const DETAIL_TITLE: u32 = 3;
const DETAIL_FOOTER: u32 = 4;
const PACK_TITLE: u32 = 5;
const PACK_FOOTER: u32 = 6;
const LIST_ROWS: u32 = 8;
const LIST_DESCRIPTIONS: u32 = LIST_ROWS + MAX_ROWS as u32;
const DETAIL_ROWS: u32 = LIST_DESCRIPTIONS + MAX_ROWS as u32;
const DETAIL_DESCRIPTIONS: u32 = DETAIL_ROWS + MAX_PAGE_ROWS as u32;
const PANEL_TEXTS: u32 = DETAIL_DESCRIPTIONS + MAX_PAGE_ROWS as u32;
const PACK_ROWS: u32 = PANEL_TEXTS + page_layout::TEXTS as u32;
const PACK_DESCRIPTIONS: u32 = PACK_ROWS + MAX_PACK_ROWS as u32;
const _: () = assert!(PACK_DESCRIPTIONS + MAX_PACK_ROWS as u32 <= TEXTS.len);
/// Rows the list (and a pack's list) shows at most.
pub const MAX_ROWS: usize = LIMIT as usize;
const MAX_PACK_ROWS: usize = 1 + live_proto::maps::MAX_PACK_MEMBERS;
/// Rows a map's page has: its action.
pub const MAX_PAGE_ROWS: usize = 1;

/// Where the list and installs come from: the HLE and the map gate in a
/// boot, a fake in tests.
pub trait Source: Send + Sync {
    /// Checks whether a list read can start, without asking the server.
    fn check_list(&self) -> Result<(), ModsError>;
    fn list(&self, limit: u32, done: ListDone) -> Result<(), ModsError>;
    fn install(&self, id: MapId, progress: Progress, done: InstallDone) -> Result<(), ModsError>;
    /// The maps of `ids` a package here already has.
    fn installed(&self, ids: &[MapId]) -> BTreeSet<MapId>;
    /// Removes the downloaded package of `id` (quick: a directory and a
    /// line of `active.toml`).
    fn remove(&self, id: MapId) -> Result<(), String>;
}

/// The HLE for the list and the install, the gate for what is here.
pub struct LiveSource {
    pub hle: Arc<live_hle::Hle>,
    pub gate: Arc<live_hle::maps::MapGate>,
}

impl Source for LiveSource {
    fn check_list(&self) -> Result<(), ModsError> {
        self.hle.check_mod_list()
    }
    fn list(&self, limit: u32, done: ListDone) -> Result<(), ModsError> {
        self.hle.mod_list(
            limit,
            None,
            Box::new(move |result| {
                if let Ok(page) = &result {
                    crate::map_select::remember_packs(&page.items);
                }
                done(result);
            }),
        )
    }
    fn install(&self, id: MapId, progress: Progress, done: InstallDone) -> Result<(), ModsError> {
        self.hle.install_mod(id, progress, done)
    }
    fn installed(&self, ids: &[MapId]) -> BTreeSet<MapId> {
        // One read of `<data>/maps`, not one per entry.
        let here: BTreeSet<MapId> = content::Library::open(self.gate.maps_dir())
            .packages()
            .iter()
            .map(|p| p.id)
            .collect();
        ids.iter().filter(|id| here.contains(id)).copied().collect()
    }
    fn remove(&self, id: MapId) -> Result<(), String> {
        self.gate.remove(&id).map(|_| ())
    }
}

/// The list as last read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum List {
    Loading,
    Loaded(Vec<ModEntry>),
    Failed(ModsError),
}

/// A release from the pack repository or a standalone map from a legacy server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Top {
    Map(ModEntry),
    Pack(String, Vec<ModEntry>),
}

/// Index released packs by identity. Entries from the legacy map endpoint
/// without an explicit pack use their published pack name.
fn grouped_rows(items: &[ModEntry]) -> Vec<Vec<&ModEntry>> {
    let mut positions = BTreeMap::new();
    let mut groups: Vec<Vec<&ModEntry>> = Vec::new();
    for e in items {
        let group = match pack_key(e) {
            Some(key) => *positions.entry(key).or_insert(groups.len()),
            None => groups.len(),
        };
        if group == groups.len() {
            groups.push(vec![e]);
        } else {
            groups[group].push(e);
        }
    }
    groups
}

fn top(group: Vec<&ModEntry>) -> Top {
    let first = group[0];
    if let Some(name) = &first.pack
        && (group.len() > 1 || first.pack_info.as_ref().is_some_and(|p| p.published_pack))
    {
        Top::Pack(name.clone(), group.into_iter().cloned().collect())
    } else {
        Top::Map(first.clone())
    }
}

/// First-class releases always have a folder, including one-map releases.
pub fn tops(items: &[ModEntry]) -> Vec<Top> {
    grouped_rows(items).into_iter().map(top).collect()
}

fn pack_key(e: &ModEntry) -> Option<&str> {
    e.pack_info
        .as_ref()
        .map(|p| p.id.as_str())
        .or(e.pack.as_deref())
}

/// A download, as the screens show it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Install {
    Downloading {
        done: u64,
        total: u64,
    },
    Done,
    Failed(ModsError),
    /// Removed from this console.
    Removed,
    RemoveFailed(String),
}

/// What a feature is called on the checklist, and what it means.
pub fn feature_text(id: &str) -> (&'static str, &'static str) {
    match id {
        "vehicles" => ("Vehicles", "vehicles added, taken out or moved"),
        "weapons" => (
            "Weapons",
            "weapons added, taken out or moved, on the map or its respawns",
        ),
        "objects" => (
            "Objects",
            "scenery, crates, lights or other objects added, taken out or moved",
        ),
        "textures" => ("Textures", "new or repainted textures"),
        "shaders" => ("Shaders", "new or changed shaders: how surfaces look"),
        "scripts" => ("Scripts", "its scripts changed: things happen as you play"),
        "geometry" => ("Geometry", "the level itself was rebuilt or reshaped"),
        "ai" => ("AI", "new or changed AI characters"),
        _ => ("Other", "something else"),
    }
}

/// A game type as a player reads it.
pub fn gametype_title(id: &str) -> &'static str {
    match id {
        "slayer" => "Slayer",
        "ctf" => "CTF",
        "assault" => "Assault",
        "oddball" => "Oddball",
        "koth" => "King of the Hill",
        "juggernaut" => "Juggernaut",
        "territories" => "Territories",
        _ => "Other",
    }
}

/// The facts in the page's table, label and value, at most
/// [`page_layout::FACT_ROWS`]: the base map, who made it (and who shared
/// it, when that is someone else), size, downloads, date and version (when
/// the server has them), game types, which disc the base is on, and that
/// players who join get it. Each value fits its own table cell.
pub fn facts(e: &ModEntry) -> Vec<(&'static str, String)> {
    let mut v = vec![("Map", mods::base_title(&e.base)), ("By", made_by(e))];
    if uploader_apart(e) {
        v.push(("Shared By", e.author.clone()));
    }
    v.push(("Size", mods::size_text(e.size)));
    if let Some(n) = e.downloads {
        v.push(("Downloads", n.to_string()));
    }
    if let Some(day) = e.published_at.get(..10) {
        v.push(("Published", day.to_string()));
    }
    if let Some(version) = &e.version {
        v.push(("Version", version.clone()));
    }
    v.push(("Modes", gametypes_text(&e.gametypes)));
    let disc = if content::map_pack::is_map(&e.base) {
        "Map Pack"
    } else {
        "Your disc"
    };
    v.push(("From", disc.to_string()));
    v.push(("Joiners", "Auto-install".to_string()));
    v.truncate(page_layout::FACT_ROWS);
    v.into_iter()
        .map(|(label, value)| {
            let room = i32::from(page_layout::TABLE_RIGHT - page_layout::VALUE_LEFT);
            (label, text_fit::line(&value, room))
        })
        .collect()
}

/// The checklist of what `e` changes, in [`FEATURES`] order: a tick and
/// a name each (`[x] Vehicles`, `[ ] AI`); one line saying so when the
/// server lists none.
pub fn checklist(e: &ModEntry) -> Vec<String> {
    if e.features.is_empty() {
        return vec!["Not listed".into()];
    }
    FEATURES
        .iter()
        .map(|id| {
            let tick = if e.features.iter().any(|f| f == id) {
                "[x]"
            } else {
                "[ ]"
            };
            format!("{tick} {}", feature_text(id).0)
        })
        .collect()
}

/// Every panel text of `e`'s page, by [`page_layout`] slot: the body, the
/// facts (empty past the last), the checklist's heading and cells.
pub fn panel_texts(e: &ModEntry) -> Vec<String> {
    let mut t = vec![String::new(); page_layout::TEXTS];
    t[page_layout::BODY_TEXT] = about(e);
    for (k, (label, value)) in facts(e).into_iter().enumerate() {
        t[page_layout::fact_label(k)] = label.to_string();
        t[page_layout::fact_value(k)] = value;
    }
    t[page_layout::CHECK_HEADING] = "Changes".into();
    for (i, line) in checklist(e)
        .into_iter()
        .take(2 * page_layout::CHECK_ROWS)
        .enumerate()
    {
        t[page_layout::check_cell(i / 2, i % 2)] = line;
    }
    t
}

/// Who made `e`, for its page and the list's summary: the server's credit
/// (the mod's own authors, `A & B`), else the publishing account.
pub fn made_by(e: &ModEntry) -> String {
    e.credit.clone().unwrap_or_else(|| e.author.clone())
}

/// Whether the page names the uploader on a row of its own: when the
/// credit names the maker, so the uploader is not lost.
pub fn uploader_apart(e: &ModEntry) -> bool {
    e.credit
        .as_deref()
        .is_some_and(|c| !c.eq_ignore_ascii_case(&e.author))
}

/// String slots in guest memory: a string id per slot, its text written
/// when it changes.
#[derive(Default)]
pub struct Slots {
    base: AtomicU32,
    slots: Mutex<Vec<(Vec<u8>, bool)>>,
}

impl Slots {
    pub fn id(slot: u32) -> u32 {
        TEXTS.id(slot)
    }
    /// Sets slot `slot`'s text (cut to fit, NUL-terminated UTF-8); its id.
    pub fn set(&self, slot: u32, text: &str) -> u32 {
        let mut cut = text.len().min(SLOT_LEN as usize - 1);
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        let mut b = text.as_bytes()[..cut].to_vec();
        b.push(0);
        let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        if slots.len() < TEXTS.len as usize {
            slots.resize(TEXTS.len as usize, (vec![0], false));
        }
        let s = &mut slots[slot as usize];
        if s.0 != b {
            *s = (b, true);
        }
        Self::id(slot)
    }
    /// The text in slot `slot`, for tests and logs.
    pub fn get(&self, slot: u32) -> Option<String> {
        let slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        let b = &slots.get(slot as usize)?.0;
        Some(String::from_utf8_lossy(&b[..b.len().saturating_sub(1)]).into_owned())
    }
    /// The guest address of string `id`, when it is one of ours.
    pub fn address(&self, id: u32) -> Option<u32> {
        let base = self.base.load(Ordering::SeqCst);
        (base != 0 && TEXTS.owns(id)).then(|| base + (id - TEXTS.base) * SLOT_LEN)
    }
    /// Every slot written again at the next [`Self::write`]: after a
    /// snapshot load the page holds the saved run's texts, which the
    /// cache can't tell from its own.
    pub fn rewrite_all(&self) {
        let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        for (_, dirty) in slots.iter_mut() {
            *dirty = true;
        }
    }
    /// Writes every changed slot.
    pub fn write(&self, m: &mut dyn GuestMemory) -> Result<(), MemFault> {
        let base = self.base.load(Ordering::SeqCst);
        if base == 0 {
            return Ok(());
        }
        let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        for (i, (b, dirty)) in slots.iter_mut().enumerate() {
            if *dirty {
                m.write(base + i as u32 * SLOT_LEN, b)?;
                *dirty = false;
            }
        }
        Ok(())
    }
}

struct Inner {
    list: List,
    /// The last-seen position, as kept on disk.
    seen: u64,
    /// What "New" is measured against on the screen this visit.
    shown_from: u64,
    // A populated visit keeps its row identities until B leaves the browser.
    visiting: bool,
    next_list: Option<Vec<ModEntry>>,
    selected: Option<MapId>,
    installed: BTreeSet<MapId>,
    installs: BTreeMap<MapId, Install>,
    /// The pack whose list is open.
    pack: Option<String>,
    /// A Remove pressed once, waiting for the second press.
    removing: Option<MapId>,
    /// When the list was last asked for, and whether that read failed.
    asked: Option<Instant>,
    failed: bool,
    signin_wait_logged: bool,
    in_flight: bool,
    /// The maps' pictures by map file name; None until read (again).
    pictures: Option<BTreeMap<String, u32>>,
}

/// The mod repository's state, shared by the screens, the Live menu row
/// and the threads.
pub struct Repo {
    inner: Mutex<Inner>,
    pub texts: Slots,
    source: Arc<dyn Source>,
    seen_file: Option<PathBuf>,
    wanted: AtomicBool,
    /// Bumped when a row's text may have changed (a list read, a
    /// download's progress, an install's end): the screens set their
    /// descriptions again.
    changed: AtomicU64,
}

fn published_pack(maps: &[ModEntry]) -> Option<&mods::ModPack> {
    maps.first()?
        .pack_info
        .as_ref()
        .filter(|p| p.published_pack)
}

fn pack_action_label(maps: &[ModEntry]) -> Option<&'static str> {
    let info = published_pack(maps)?;
    if crate::map_select::packs::active(&info.id) {
        Some("Use original maps")
    } else {
        crate::map_select::packs::playable(&info.id).then_some("Use pack")
    }
}

fn newest(items: &[ModEntry]) -> u64 {
    items.iter().filter_map(|e| e.position).max().unwrap_or(0)
}

impl Repo {
    /// Over `source`, keeping the last-seen position in `seen_file`.
    pub fn new(source: Arc<dyn Source>, seen_file: Option<PathBuf>) -> Arc<Self> {
        let seen = seen_file.as_deref().map_or(0, read_seen);
        let repo = Arc::new(Repo {
            inner: Mutex::new(Inner {
                list: List::Loading,
                seen,
                shown_from: seen,
                visiting: false,
                next_list: None,
                selected: None,
                pack: None,
                installed: BTreeSet::new(),
                installs: BTreeMap::new(),
                removing: None,
                asked: None,
                failed: false,
                signin_wait_logged: false,
                in_flight: false,
                pictures: None,
            }),
            texts: Slots::default(),
            source,
            seen_file,
            wanted: AtomicBool::new(true),
            changed: AtomicU64::new(1),
        });
        repo.texts.set(LIST_TITLE, "Mod Downloader");
        repo.footer(SCREEN, Some(0));
        repo.footer(DETAIL, Some(0));
        repo.footer(PACK, Some(0));
        repo.refresh_menu();
        repo
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn touch(&self) {
        self.changed.fetch_add(1, Ordering::SeqCst);
    }

    /// The Live menu row's string id, its text kept current.
    pub fn menu_label(&self) -> u32 {
        Slots::id(MENU_SLOT)
    }

    /// The Live menu row's text: "Mod Downloader", or "Mod Downloader (N new)".
    pub fn menu_text(&self) -> String {
        let g = self.lock();
        if let Some(items) = &g.next_list {
            return menu_text(mods::new_count(items, g.seen));
        }
        let n = match &g.list {
            List::Loaded(items) => mods::new_count(items, g.seen),
            _ => 0,
        };
        menu_text(n)
    }

    fn refresh_menu(&self) {
        let t = self.menu_text();
        self.texts.set(MENU_SLOT, &t);
    }

    /// Asks for a fresh list at the reader's next turn (the Live menu was
    /// drawn, the screen opened). Never waits; safe with guest memory held.
    pub fn want_list(&self) {
        self.wanted.store(true, Ordering::SeqCst);
    }

    fn read_if_due(self: &Arc<Self>, now: Instant) {
        if let Err(e) = self.source.check_list() {
            let mut g = self.lock();
            // A new sign-in can fetch at once, even after a recent read.
            g.asked = None;
            if !g.in_flight && matches!(g.list, List::Loading) {
                g.list = List::Failed(e);
                self.touch();
            }
            return;
        }
        if self.due(now) {
            self.fetch(now);
        }
    }

    /// Whether the reader should ask now.
    pub fn due(&self, now: Instant) -> bool {
        let g = self.lock();
        if g.in_flight {
            return false;
        }
        if self.wanted.load(Ordering::SeqCst) {
            return g.asked.is_none_or(|at| now.duration_since(at) >= MIN_GAP);
        }
        match g.asked {
            None => true,
            Some(at) => {
                let wait = if g.failed { RETRY_AFTER } else { REFRESH_EVERY };
                now.duration_since(at) >= wait
            }
        }
    }

    /// Asks the source for the list; the answer lands in [`Repo::listed`].
    pub fn fetch(self: &Arc<Self>, now: Instant) {
        self.wanted.store(false, Ordering::SeqCst);
        {
            let mut g = self.lock();
            g.asked = Some(now);
            g.in_flight = true;
        }
        let me = Arc::downgrade(self);
        let r = self.source.list(
            LIMIT,
            Box::new(move |r| {
                if let Some(me) = me.upgrade() {
                    me.listed(r.map(|p: ModPage| p.items));
                }
            }),
        );
        if let Err(e) = r {
            self.listed(Err(e));
        }
    }

    /// The list's answer.
    pub fn listed(&self, r: Result<Vec<ModEntry>, ModsError>) {
        let installed = match &r {
            Ok(items) => {
                // A refresh may omit a map that is still on the current page.
                let mut ids: BTreeSet<_> = items.iter().map(|e| e.id).collect();
                {
                    let g = self.lock();
                    if g.visiting
                        && let List::Loaded(visible) = &g.list
                    {
                        ids.extend(visible.iter().map(|e| e.id));
                    }
                }
                Some(self.source.installed(&ids.into_iter().collect::<Vec<_>>()))
            }
            Err(_) => None,
        };
        {
            let mut g = self.lock();
            g.in_flight = false;
            g.failed = r.is_err();
            match r {
                Ok(items) => {
                    eprintln!(
                        "mods: {} on the server, {} new",
                        items.len(),
                        mods::new_count(&items, g.seen)
                    );
                    if g.visiting && matches!(&g.list, List::Loaded(rows) if !rows.is_empty()) {
                        g.next_list = Some(items);
                    } else {
                        if g.visiting {
                            self.mark_seen(&mut g, newest(&items));
                        }
                        g.list = List::Loaded(items);
                    }
                }
                Err(e) => {
                    if e == ModsError::NotSignedIn {
                        // The CLI's account selection precedes the guest's Live logon.
                        if !g.signin_wait_logged || crate::log::verbose() {
                            eprintln!("mods: the list requires an active in-game Xbox Live sign-in");
                            g.signin_wait_logged = true;
                        }
                    } else {
                        eprintln!("mods: no list: {e}");
                    }
                    // A list already read stays shown through a failed refresh.
                    if !matches!(&g.list, List::Loaded(rows) if !rows.is_empty()) {
                        g.list = List::Failed(e);
                    }
                }
            }
            if let Some(i) = installed {
                g.installed = i;
            }
        }
        self.touch();
        self.refresh_menu();
    }

    /// The screen is opening: everything listed is seen from now on (kept
    /// on disk), but tagged "New" for this visit; a fresh list is asked
    /// for, and the maps' pictures are read again.
    pub fn opened(&self) {
        {
            let mut g = self.lock();
            if let Some(items) = g.next_list.take() {
                g.list = List::Loaded(items);
            }
            g.visiting = true;
            g.shown_from = g.seen;
            g.pictures = None;
            if let List::Loaded(items) = &g.list {
                let position = newest(items);
                self.mark_seen(&mut g, position);
            }
        }
        self.footer(SCREEN, Some(0));
        self.want_list();
        self.touch();
        self.refresh_menu();
    }

    fn mark_seen(&self, g: &mut Inner, seen: u64) {
        if seen > g.seen {
            g.seen = seen;
            if let Some(path) = &self.seen_file
                && let Err(e) = write_seen(path, seen)
            {
                eprintln!("mods: the last-seen position could not be kept: {e}");
            }
        }
    }

    fn closed(&self) {
        let mut g = self.lock();
        g.visiting = false;
        if let Some(items) = g.next_list.take() {
            g.list = List::Loaded(items);
        }
        drop(g);
        self.refresh_menu();
        self.touch();
    }

    /// The entries as the list shows them (at most [`MAX_ROWS`]).
    pub fn entries(&self) -> Vec<ModEntry> {
        match &self.lock().list {
            List::Loaded(items) => items.iter().take(MAX_ROWS).cloned().collect(),
            _ => Vec::new(),
        }
    }

    pub fn list_state(&self) -> List {
        self.lock().list.clone()
    }

    /// The list's row `row` ([`Top`]), with a list read.
    fn top_at(g: &Inner, row: i16) -> Option<Top> {
        let List::Loaded(items) = &g.list else {
            return None;
        };
        let r = usize::try_from(row).ok().filter(|r| *r < MAX_ROWS)?;
        grouped_rows(items).into_iter().nth(r).map(top)
    }

    /// The list's rows as they are now (none without a list).
    pub fn tops(&self) -> Vec<Top> {
        match &self.lock().list {
            List::Loaded(items) => tops(items).into_iter().take(MAX_ROWS).collect(),
            _ => Vec::new(),
        }
    }

    /// Row `row` of the list: the map's name and its state ("New",
    /// "Installed", "Downloading N%", else its size), or the pack's name
    /// and its maps' ([`Self::pack_status`]); with no list, why.
    pub fn list_row(&self, row: i16) -> Option<(String, String)> {
        let g = self.lock();
        match &g.list {
            List::Loading => return (row == 0).then(|| ("Loading...".into(), String::new())),
            List::Failed(e) => return (row == 0).then(|| (failed_text(e), "Retry".into())),
            List::Loaded(items) if items.is_empty() => {
                return (row == 0).then(|| ("No mods yet".into(), String::new()));
            }
            List::Loaded(_) => {}
        }
        Some(match Self::top_at(&g, row)? {
            Top::Map(e) => (e.name.clone(), Self::status(&g, &e)),
            Top::Pack(name, maps) => (name, Self::pack_status(&g, &maps)),
        })
    }

    /// Row `row` of the list as its one line: the name, and its state as a
    /// mark ([`mark`]: a star, a tick, an arrow and its percentage) unless
    /// that is only its size; a pack's says how many maps it has. With no
    /// list, the one row is what A does: "Loading...", "Try Again" or
    /// "Refresh" (why is in its description).
    pub fn list_line(&self, row: i16) -> Option<String> {
        let (name, status) = self.list_row(row)?;
        let has_entry = Self::top_at(&self.lock(), row).is_some();
        if !has_entry {
            return Some(match self.lock().list {
                List::Loading => name,
                List::Failed(_) => "Try Again".into(),
                List::Loaded(_) => "Refresh".into(),
            });
        }
        Some(marked_line(&name, &status))
    }

    fn status(g: &Inner, e: &ModEntry) -> String {
        match g.installs.get(&e.id) {
            Some(Install::Downloading { done, total }) => return percent(*done, *total),
            Some(Install::Done) => return "Installed".into(),
            Some(Install::Removed) => return mods::size_text(e.size),
            Some(Install::Failed(_)) => return "Failed".into(),
            Some(Install::RemoveFailed(_)) | None => {}
        }
        if g.installed.contains(&e.id) {
            "Installed".into()
        } else if mods::is_new(e, g.shown_from) {
            "New".into()
        } else {
            mods::size_text(e.size)
        }
    }

    /// A pack's state on its row: its downloads' progress together while
    /// any runs, "Installed" once every map is here, "New" when one is new,
    /// else how many maps it has ("5 maps").
    fn pack_status(g: &Inner, maps: &[ModEntry]) -> String {
        // The percentage is the fraction of the whole pack available here.
        // A finished map still contributes after its progress callback stops.
        let mut available = 0u128;
        let mut total = 0u128;
        let mut running = false;
        for e in maps {
            let size = u128::from(e.size.max(1));
            total += size;
            if Self::is_here(g, &e.id) {
                available += size;
            } else if let Some(Install::Downloading { done, total }) = g.installs.get(&e.id) {
                available += size * u128::from((*done).min(*total)) / u128::from((*total).max(1));
                running = true;
            }
        }
        if running {
            format!("Downloading {}%", 100 * available / total)
        } else if maps.iter().all(|e| Self::is_here(g, &e.id)) {
            "Installed".into()
        } else if maps
            .iter()
            .any(|e| matches!(g.installs.get(&e.id), Some(Install::Failed(_))))
        {
            "Failed".into()
        } else if maps.iter().any(|e| Self::status(g, e) == "New") {
            "New".into()
        } else {
            format!(
                "{} {}",
                maps.len(),
                if maps.len() == 1 { "map" } else { "maps" }
            )
        }
    }

    fn is_here(g: &Inner, id: &MapId) -> bool {
        match g.installs.get(id) {
            Some(Install::Done) => true,
            Some(Install::Removed) => false,
            _ => g.installed.contains(id),
        }
    }

    /// The list's description of row `row`: a map's base map and
    /// publisher, size and downloads, what it changes, and its state; a
    /// pack's summary ([`Self::pack_summary`]).
    pub fn list_description(&self, row: i16) -> Option<String> {
        let g = self.lock();
        let Some(top) = Self::top_at(&g, row) else {
            return (row == 0).then(|| match &g.list {
                List::Loading => "Reading the mods on your Live server.".into(),
                List::Failed(e) => {
                    join_sentences([failed_text(e).as_str(), "A reads the list again."])
                }
                List::Loaded(_) => join_sentences([
                    "No mods on your Live server yet.",
                    "A reads the list again.",
                ]),
            });
        };
        Some(match top {
            Top::Map(e) => Self::map_description(&g, &e),
            Top::Pack(_, maps) => {
                let action = match Self::pack_status(&g, &maps).as_str() {
                    "Installed" => "Change Map > Mods".into(),
                    "Failed" => "Download failed".into(),
                    "New" => "New maps".into(),
                    s if s.starts_with("Downloading") => s.into(),
                    _ => "Open pack to browse".into(),
                };
                Self::pack_summary(&g, &maps, action)
            }
        })
    }

    /// A map's description on a list: base map and publisher, size and
    /// downloads, what it changes, and its state.
    fn map_description(g: &Inner, e: &ModEntry) -> String {
        let mut lines = vec![format!(
            "On {}, by {}",
            mods::base_title(&e.base),
            made_by(e)
        )];
        lines.push(match e.downloads {
            Some(1) => format!("{}, 1 download", mods::size_text(e.size)),
            Some(n) => format!("{}, {n} downloads", mods::size_text(e.size)),
            None => mods::size_text(e.size),
        });
        lines.push(if e.features.is_empty() {
            "Changes not listed".into()
        } else {
            let names: Vec<String> = e
                .features
                .iter()
                .map(|f| feature_text(f).0.to_lowercase())
                .collect();
            format!("Has {}", names.join(", "))
        });
        lines.push(match g.installs.get(&e.id) {
            Some(Install::Failed(err)) => failed_install_text(err),
            _ => match Self::status(g, e).as_str() {
                "Installed" => "Installed".into(),
                "New" => "New map".into(),
                s if s.starts_with("Downloading") => s.into(),
                _ => "Available to download".into(),
            },
        });
        lines
            .iter()
            .map(|l| text_fit::line(l, text_fit::SUMMARY))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// A pack's summary: how many maps and by whom, what it is (the
    /// server's words, else the maps it is built on), its size and how
    /// much of it is here, then `action`.
    fn pack_summary(g: &Inner, maps: &[ModEntry], action: String) -> String {
        let mut makers: Vec<String> = Vec::new();
        for m in maps.iter().map(made_by) {
            if !makers.contains(&m) {
                makers.push(m);
            }
        }
        let pack = published_pack(maps);
        let makers = pack
            .and_then(|p| p.authors.clone())
            .unwrap_or_else(|| makers.join(", "));
        let about = match maps.iter().find_map(|e| e.pack_description.as_deref()) {
            Some(d) => text_fit::paragraph(d, text_fit::SUMMARY, 2),
            None => {
                let mut bases: Vec<String> = Vec::new();
                for b in maps.iter().map(|e| mods::base_title(&e.base)) {
                    if !bases.contains(&b) {
                        bases.push(b);
                    }
                }
                text_fit::line(&format!("On {}", bases.join(", ")), text_fit::SUMMARY)
            }
        };
        let here = maps.iter().filter(|e| Self::is_here(g, &e.id)).count();
        let size = pack
            .and_then(|p| p.size)
            .unwrap_or_else(|| maps.iter().fold(0u64, |sum, e| sum.saturating_add(e.size)));
        let about = match pack.and_then(|p| p.advisory) {
            Some(label) => {
                let warning = match label {
                    live_proto::maps::ModPackAdvisory::CheatingPack => "Cheating pack",
                    live_proto::maps::ModPackAdvisory::NotForOnline => "Not for online",
                };
                format!("{warning}\n{}", text_fit::line(&about, text_fit::SUMMARY))
            }
            None => about,
        };
        [
            text_fit::line(
                &format!(
                    "{} {}, by {makers}",
                    maps.len(),
                    if maps.len() == 1 { "map" } else { "maps" }
                ),
                text_fit::SUMMARY,
            ),
            about,
            text_fit::line(
                &format!("{here}/{} installed, {}", maps.len(), mods::size_text(size)),
                text_fit::SUMMARY,
            ),
            text_fit::line(&action, text_fit::SUMMARY),
        ]
        .join("\n")
    }

    /// The rows the list opens with.
    pub fn list_count(&self) -> usize {
        match &self.lock().list {
            List::Loaded(items) => grouped_rows(items).len().clamp(1, MAX_ROWS),
            _ => 1,
        }
    }

    /// A press on the list's row `row`: the map's page, or the pack's list.
    pub fn press_list(&self, row: i16) -> Chosen {
        let top = Self::top_at(&self.lock(), row);
        match top {
            Some(Top::Map(e)) => self.open_page(&e),
            Some(Top::Pack(name, maps)) => {
                self.open_pack(&name, pack_key(&maps[0]).unwrap_or(&name))
            }
            None if row == 0 => {
                let mut g = self.lock();
                if g.in_flight || matches!(g.list, List::Loading) {
                    return Chosen::Nothing;
                }
                g.asked = None;
                g.list = List::Loading;
                drop(g);
                self.touch();
                self.want_list();
                Chosen::Stepped("mods: reading the list again".into())
            }
            None => Chosen::Nothing,
        }
    }

    /// Opens map `e`'s page.
    fn open_page(&self, e: &ModEntry) -> Chosen {
        {
            let mut g = self.lock();
            g.selected = Some(e.id);
            g.removing = None;
        }
        self.texts
            .set(DETAIL_TITLE, &text_fit::line(&e.name, text_fit::TITLE));
        self.footer(DETAIL, Some(0));
        self.set_panel();
        self.touch();
        Chosen::Open(DETAIL)
    }

    /// Opens pack `name`'s list.
    fn open_pack(&self, name: &str, key: &str) -> Chosen {
        self.lock().pack = Some(key.to_owned());
        self.texts
            .set(PACK_TITLE, &text_fit::line(name, text_fit::TITLE));
        self.footer(PACK, Some(0));
        self.touch();
        Chosen::Open(PACK)
    }

    /// X on the list's row `row`: that map downloads, its page unopened
    /// (an installed map is left alone: Remove is on its page); on a pack,
    /// every map of it not here yet.
    pub fn x_list(self: &Arc<Self>, row: Option<i16>) -> String {
        let top = row.and_then(|r| Self::top_at(&self.lock(), r));
        match top {
            Some(Top::Map(e)) => self.x_map(&e),
            Some(Top::Pack(name, maps)) => self.download_all(&name, &maps),
            None => "mods: X with no map focused".into(),
        }
    }

    /// X on map `e`'s row: it downloads unless it is here.
    fn x_map(self: &Arc<Self>, e: &ModEntry) -> String {
        if Self::is_here(&self.lock(), &e.id) {
            return format!("mods: {} is installed; its page removes it", e.name);
        }
        self.download(e)
    }

    fn download_prompt(g: &Inner, e: &ModEntry) -> Option<&'static str> {
        match g.installs.get(&e.id) {
            Some(Install::Downloading { .. }) => None,
            _ if Self::is_here(g, &e.id) => None,
            Some(Install::Failed(_)) => Some("Retry"),
            _ => Some("Download"),
        }
    }

    /// Downloads every map of pack `name` neither here nor under way.
    fn download_all(self: &Arc<Self>, name: &str, maps: &[ModEntry]) -> String {
        let mut queue = VecDeque::new();
        {
            let mut g = self.lock();
            for e in maps {
                if Self::download_prompt(&g, e).is_some() {
                    g.installs.insert(
                        e.id,
                        Install::Downloading {
                            done: 0,
                            total: e.size,
                        },
                    );
                    queue.push_back(e.id);
                }
            }
        }
        if queue.is_empty() {
            return format!("mods: {name}: every map is installed or downloading");
        }
        let count = queue.len();
        self.touch();
        self.start_next(Arc::new(Mutex::new(queue)));
        format!(
            "mods: {name}: downloading {count} {}",
            if count == 1 { "map" } else { "maps" }
        )
    }

    /// The open pack and its maps, newest first. A vanished pack has no actions.
    pub fn pack(&self) -> Option<(String, Vec<ModEntry>)> {
        let g = self.lock();
        let maps = Self::pack_maps(&g)?;
        Some((maps.first()?.pack.clone()?, maps))
    }

    fn pack_maps(g: &Inner) -> Option<Vec<ModEntry>> {
        let name = g.pack.as_ref()?;
        let List::Loaded(items) = &g.list else {
            return None;
        };
        // Older snapshots kept the display title instead of the ID. Restore
        // it only if a single release owns that title on this server.
        let key = if items.iter().any(|e| pack_key(e) == Some(name.as_str())) {
            name.as_str()
        } else {
            let mut matches = items
                .iter()
                .filter(|e| e.pack.as_ref() == Some(name))
                .filter_map(pack_key);
            let key = matches.next()?;
            if matches.any(|other| other != key) {
                return None;
            }
            key
        };
        let maps: Vec<_> = items
            .iter()
            .filter(|e| pack_key(e) == Some(key))
            .cloned()
            .collect();
        (!maps.is_empty()).then_some(maps)
    }

    /// The map at the open pack's row `row` (row 0 is Download All).
    fn pack_entry(&self, row: i16) -> Option<ModEntry> {
        let r = usize::try_from(row)
            .ok()
            .filter(|r| (1..MAX_PACK_ROWS).contains(r))?;
        self.pack()?.1.into_iter().nth(r - 1)
    }

    /// The rows the pack's list has: Download All, then its maps.
    pub fn pack_count(&self) -> usize {
        self.pack()
            .map_or(1, |(_, maps)| 1 + maps.len().min(MAX_PACK_ROWS - 1))
    }

    /// Row `row` of the pack's list: Download All (or how the downloads
    /// go, or All Installed), then a map's line as the list has it.
    pub fn pack_line(&self, row: i16) -> Option<String> {
        if row == 0 {
            let Some((_, maps)) = self.pack() else {
                return Some("Pack unavailable".into());
            };
            let g = self.lock();
            let left = maps
                .iter()
                .filter(|e| Self::download_prompt(&g, e).is_some())
                .count();
            if left > 0 {
                return Some(format!("Download All ({left})"));
            }
            let state = Self::pack_status(&g, &maps);
            return Some(if state == "Installed" {
                if published_pack(&maps).is_some() {
                    pack_action_label(&maps)
                        .unwrap_or("No free map slots")
                        .into()
                } else {
                    "All Installed".into()
                }
            } else {
                state
            });
        }
        let e = self.pack_entry(row)?;
        let g = self.lock();
        // The pack name is already in the header. Keep a distinctive map
        // name visible when the publisher prefixed every title with it.
        let name = e
            .pack
            .as_deref()
            .and_then(|pack| e.name.strip_prefix(pack))
            .and_then(|suffix| {
                suffix
                    .strip_prefix(':')
                    .or_else(|| suffix.strip_prefix(' '))
            })
            .map(str::trim_start)
            .filter(|suffix| !suffix.is_empty())
            .unwrap_or(&e.name);
        Some(marked_line(name, &Self::status(&g, &e)))
    }

    /// The pack's list's description of row `row`: the pack's summary on
    /// Download All, a map's as the list has it on the others.
    pub fn pack_description(&self, row: i16) -> Option<String> {
        if row == 0 {
            let Some((_, maps)) = self.pack() else {
                return Some(join_sentences([
                    "This pack is no longer listed.",
                    "Press B to return to Mod Downloader.",
                ]));
            };
            let g = self.lock();
            let left = maps.iter().filter(|e| !Self::is_here(&g, &e.id)).count();
            let action = match Self::pack_status(&g, &maps).as_str() {
                "Installed"
                    if published_pack(&maps).is_some() && pack_action_label(&maps).is_none() =>
                {
                    "Free map slots to use this pack".into()
                }
                "Installed" => "Change Map > Mods".into(),
                s if s.starts_with("Downloading") => s.to_string(),
                _ => format!("{left} to download"),
            };
            return Some(Self::pack_summary(&g, &maps, action));
        }
        let e = self.pack_entry(row)?;
        Some(Self::map_description(&self.lock(), &e))
    }

    /// A on the pack's row `row`: Download All, or a map's page.
    pub fn press_pack(self: &Arc<Self>, row: i16) -> Chosen {
        if row == 0 {
            return match self.pack() {
                Some((name, maps)) => Chosen::Stepped(self.act_on_pack(&name, &maps)),
                None => Chosen::Nothing,
            };
        }
        match self.pack_entry(row) {
            Some(e) => self.open_page(&e),
            None => Chosen::Nothing,
        }
    }

    fn act_on_pack(self: &Arc<Self>, name: &str, maps: &[ModEntry]) -> String {
        let installed = {
            let g = self.lock();
            maps.iter().all(|e| Self::is_here(&g, &e.id))
        };
        if installed && let Some(info) = published_pack(maps) {
            let state = match crate::map_select::packs::toggle(&info.id) {
                Some(true) => "selected for Change Map",
                Some(false) => "original maps selected",
                None => "waiting for a free slot for every member",
            };
            self.touch();
            return format!("mods: {name}: {state}");
        }
        self.download_all(name, maps)
    }

    /// X on the pack's row `row`: that map downloads; on Download All,
    /// every map not here.
    pub fn x_pack(self: &Arc<Self>, row: Option<i16>) -> String {
        match row {
            Some(0) => match self.pack() {
                Some((name, maps)) => self.act_on_pack(&name, &maps),
                None => "mods: no pack is open".into(),
            },
            Some(r) => match self.pack_entry(r) {
                Some(e) => self.x_map(&e),
                None => "mods: no map focused".into(),
            },
            None => "mods: no map focused".into(),
        }
    }

    /// The selected entry.
    pub fn selected(&self) -> Option<ModEntry> {
        let g = self.lock();
        let id = g.selected?;
        match &g.list {
            List::Loaded(items) => items.iter().find(|e| e.id == id).cloned(),
            _ => None,
        }
    }

    /// The page's one row's line: what A does (Download, Remove, the
    /// download's progress, or why it can't).
    pub fn detail_line(&self, row: i16) -> Option<String> {
        if row != 0 {
            return None;
        }
        let Some(e) = self.selected() else {
            return Some("Map unavailable".into());
        };
        let g = self.lock();
        Some(text_fit::line(&self.action_text(&g, &e), text_fit::ROW))
    }

    /// The page's status line, the row's description: what A or X does
    /// now, or how the download is going, in one line
    /// on the lobby panel.
    pub fn detail_description(&self, row: i16) -> Option<String> {
        if row != 0 {
            return None;
        }
        let Some(e) = self.selected() else {
            return Some("Press B to go back.".into());
        };
        let g = self.lock();
        let line = match g.installs.get(&e.id) {
            Some(Install::Downloading { done, total }) => format!(
                "{} of {}. You can leave.",
                percent(*done, *total).trim_start_matches("Downloading "),
                mods::size_text(e.size)
            ),
            Some(Install::Failed(err)) => failed_install_text(err),
            _ if g.removing == Some(e.id) => "Press A or X again to remove it.".into(),
            Some(Install::RemoveFailed(why)) => format!("Not removed: {why}"),
            _ if Self::is_here(&g, &e.id) => "Installed. Change Map > Mods.".into(),
            _ => format!("A or X downloads it: {}.", mods::size_text(e.size)),
        };
        Some(text_fit::line(
            &line,
            i32::from(page_layout::STATUS.rect[3] - page_layout::STATUS.rect[1]),
        ))
    }

    /// The page's panel texts into their slots, for the selected map: the
    /// page sets its widgets from them when it opens and again when the
    /// texts change ([`Rows::panel_text`]).
    fn set_panel(&self) {
        for (i, t) in self.panel().iter().enumerate() {
            self.texts.set(PANEL_TEXTS + i as u32, t);
        }
    }

    /// The selected map's page's panel texts ([`panel_texts`]); empty
    /// with none selected.
    pub fn panel(&self) -> Vec<String> {
        self.selected()
            .map(|e| panel_texts(&e))
            .unwrap_or_else(|| vec![String::new(); page_layout::TEXTS])
    }

    fn action_text(&self, g: &Inner, e: &ModEntry) -> String {
        match g.installs.get(&e.id) {
            Some(Install::Downloading { done, total }) => percent(*done, *total),
            _ if g.removing == Some(e.id) => "Remove? Press again".into(),
            Some(Install::Failed(_)) => "Retry download".into(),
            Some(Install::RemoveFailed(_)) => "Retry removal".into(),
            _ if Self::is_here(g, &e.id) => "Remove".into(),
            _ => "Download".into(),
        }
    }

    /// A on the page's row: it downloads, or removes.
    pub fn press_detail(self: &Arc<Self>, row: i16) -> Chosen {
        if row != 0 || self.selected().is_none() {
            return Chosen::Nothing;
        }
        Chosen::Stepped(self.act())
    }

    /// X on the page, or A on its first row: download the map, or remove
    /// it once it is here (on a second press).
    pub fn act(self: &Arc<Self>) -> String {
        let Some(e) = self.selected() else {
            return "mods: no map's page is open".into();
        };
        let remove = {
            let mut g = self.lock();
            if matches!(g.installs.get(&e.id), Some(Install::Downloading { .. })) {
                return format!("mods: {} is already {}", e.name, self.action_text(&g, &e));
            }
            if !Self::is_here(&g, &e.id) {
                false
            } else if g.removing == Some(e.id) {
                g.removing = None;
                true
            } else {
                g.removing = Some(e.id);
                drop(g);
                self.touch();
                return format!("mods: {}: press again to remove it", e.name);
            }
        };
        if remove {
            self.remove(&e)
        } else {
            self.download(&e)
        }
    }

    fn remove(&self, e: &ModEntry) -> String {
        let r = self.source.remove(e.id);
        let line = {
            let mut g = self.lock();
            match &r {
                Ok(()) => {
                    g.installed.remove(&e.id);
                    g.installs.insert(e.id, Install::Removed);
                    format!("mods: {} ({}) removed", e.name, e.id)
                }
                Err(why) => {
                    g.installs.insert(e.id, Install::RemoveFailed(why.clone()));
                    format!("mods: {} not removed: {why}", e.name)
                }
            }
        };
        self.after_change(e);
        line
    }

    /// Starts `e`'s download (unless it is here or under way).
    fn download(self: &Arc<Self>, e: &ModEntry) -> String {
        {
            let mut g = self.lock();
            if matches!(g.installs.get(&e.id), Some(Install::Downloading { .. })) {
                return format!("mods: {} is already downloading", e.name);
            }
            if Self::is_here(&g, &e.id) {
                return format!("mods: {} is installed", e.name);
            }
            g.installs.insert(
                e.id,
                Install::Downloading {
                    done: 0,
                    total: e.size,
                },
            );
        }
        self.touch();
        self.start_next(Arc::new(Mutex::new(VecDeque::from([e.id]))));
        format!("mods: downloading {} ({})", e.name, e.id)
    }

    /// Queue members up front but install them one at a time. A large pack
    /// uses one download thread at a time and retries only missing members.
    fn start_next(self: &Arc<Self>, queue: Arc<Mutex<VecDeque<MapId>>>) {
        let Some(id) = queue.lock().unwrap_or_else(|e| e.into_inner()).pop_front() else {
            return;
        };
        let next = Arc::clone(&queue);
        let (a, b) = (Arc::downgrade(self), Arc::downgrade(self));
        let r = self.source.install(
            id,
            Box::new(move |done, total| {
                if let Some(me) = a.upgrade() {
                    me.lock()
                        .installs
                        .insert(id, Install::Downloading { done, total });
                    me.touch();
                }
            }),
            Box::new(move |r| {
                if let Some(me) = b.upgrade() {
                    me.install_done(id, r.map(|_| ()));
                    me.start_next(next);
                }
            }),
        );
        if let Err(err) = r {
            self.install_done(id, Err(err));
            self.start_next(queue);
        }
    }

    /// An install's end.
    pub fn install_done(&self, id: MapId, r: Result<(), ModsError>) {
        let e = {
            let mut g = self.lock();
            match r {
                Ok(()) => {
                    eprintln!("mods: {id} installed");
                    g.installed.insert(id);
                    g.installs.insert(id, Install::Done);
                }
                Err(e) => {
                    eprintln!("mods: {id} not installed: {e}");
                    g.installs.insert(id, Install::Failed(e));
                }
            }
            match &g.list {
                List::Loaded(items) => items.iter().find(|e| e.id == id).cloned(),
                _ => None,
            }
        };
        match e {
            Some(e) => self.after_change(&e),
            None => self.touch(),
        }
    }

    /// A map was installed or removed: the page's footer says what X does
    /// now, the pictures are read again (its slot's may have changed), and
    /// the screens' texts are set again.
    fn after_change(&self, e: &ModEntry) {
        let on_page = {
            let mut g = self.lock();
            g.pictures = None;
            g.selected == Some(e.id)
        };
        if on_page {
            self.footer(DETAIL, Some(0));
            self.set_panel();
        }
        self.touch();
    }

    /// The picture for the map at list row `row` (or the open page's):
    /// its base map's slot bitmap, once the pictures are read.
    fn picture_of(&self, e: Option<ModEntry>) -> Option<u32> {
        let e = e?;
        let g = self.lock();
        g.pictures
            .as_ref()?
            .get(&e.base.to_ascii_lowercase())
            .copied()
    }

    /// Reads the maps' pictures from the loaded map's multiplayer slots
    /// when they are not read yet (the screen opened, a map installed).
    fn read_pictures(&self, m: &mut dyn GuestMemory) {
        if self.lock().pictures.is_some() {
            return;
        }
        let slots = crate::mod_slots::read_slots(&crate::mod_slots::WriteGm(m));
        let pictures = crate::mod_slots::pictures(&slots);
        self.lock().pictures = Some(pictures);
    }

    /// Sets the pictures as read (tests).
    pub fn set_pictures(&self, p: BTreeMap<String, u32>) {
        self.lock().pictures = Some(p);
    }

    /// The guest address of our string `id`.
    pub fn string(&self, id: u32) -> Option<u32> {
        self.texts.address(id)
    }

    /// Why a machine snapshot must wait: a download is under way, writing
    /// a map on the host that a load would not take back.
    pub fn snapshot_live(&self) -> Option<String> {
        let g = self.lock();
        let n = g
            .installs
            .values()
            .filter(|i| matches!(i, Install::Downloading { .. }))
            .count();
        (n != 0).then(|| format!("{n} mod downloads are under way"))
    }

    /// What a machine snapshot keeps (docs/runtime/snapshots.md): the mod
    /// whose page is open and the pack whose list is. The list and what is
    /// installed are the server's and the disk's, read again; the texts
    /// are written again.
    pub fn save_state(&self) -> Vec<u8> {
        let (selected, pack) = {
            let g = self.lock();
            (g.selected, g.pack.clone())
        };
        let mut w = snapshot::codec::Writer::new();
        w.bool(selected.is_some())
            .raw(&selected.map_or([0; 32], |id| id.0));
        w.bool(pack.is_some()).str(pack.as_deref().unwrap_or(""));
        w.into_bytes()
    }

    /// Puts [`Self::save_state`]'s bytes back, with the page's title when
    /// the mod is in this run's list, and every text written again.
    pub fn load_state(&self, bytes: &[u8]) -> Result<(), String> {
        let e = |e: snapshot::Error| e.to_string();
        let r = &mut snapshot::codec::Reader::new(bytes, "mods");
        let some = r.bool().map_err(e)?;
        let id = MapId(r.array::<32>().map_err(e)?);
        // Snapshots from before packs end here.
        let pack = if r.remaining() == 0 {
            None
        } else {
            let open = r.bool().map_err(e)?;
            let name = r.str().map_err(e)?.to_string();
            open.then_some(name)
        };
        r.finish().map_err(e)?;
        {
            let mut g = self.lock();
            g.removing = None;
            g.selected = some.then_some(id);
            g.pack = pack.clone();
            g.pictures = None;
        }
        if some {
            let title = self.selected().map_or_else(
                || "Mod details".into(),
                |e| text_fit::line(&e.name, text_fit::TITLE),
            );
            self.texts.set(DETAIL_TITLE, &title);
        }
        if let Some(key) = &pack {
            let name = self
                .pack()
                .map(|(name, _)| name)
                .unwrap_or_else(|| key.clone());
            self.texts
                .set(PACK_TITLE, &text_fit::line(&name, text_fit::TITLE));
        }
        self.set_panel();
        self.texts.rewrite_all();
        self.touch();
        Ok(())
    }
}

/// The game types as one line: "Slayer, CTF", or "any game type".
pub fn gametypes_text(ids: &[String]) -> String {
    if ids.is_empty() {
        return "any game type".into();
    }
    ids.iter()
        .map(|g| gametype_title(g))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The description fits above the action hint, including for an unbroken word.
pub fn about(e: &ModEntry) -> String {
    let fallback;
    let words = match e.description.as_deref().filter(|s| !s.trim().is_empty()) {
        Some(d) => d,
        None => {
            fallback = format!("No description. Built on {}.", mods::base_title(&e.base));
            &fallback
        }
    };
    text_fit::paragraph(
        words,
        i32::from(page_layout::BODY.rect[3] - page_layout::BODY.rect[1]),
        3,
    )
}

/// A list row's state as the mark after its name (`font::mod_marks`, drawn
/// into every font): `★` new, `✓` installed, `↓ 45%` downloading; `None`
/// for a state that is only the size (or none), and the word itself for
/// anything else.
pub fn mark(status: &str) -> Option<String> {
    let new = char::from_u32(u32::from(font::mod_marks::NEW))?;
    let installed = char::from_u32(u32::from(font::mod_marks::INSTALLED))?;
    let downloading = char::from_u32(u32::from(font::mod_marks::DOWNLOADING))?;
    match status {
        "" => None,
        s if s.ends_with(" MB") || s.ends_with(" KB") => None,
        "New" => Some(new.to_string()),
        "Installed" => Some(installed.to_string()),
        s => Some(match s.strip_prefix("Downloading ") {
            Some(p) => format!("{downloading} {p}"),
            None => s.to_string(),
        }),
    }
}

/// A row's line: `name`, then its state's [`mark`] one space after it,
/// reserving space for the mark before fitting the published name.
pub fn marked_line(name: &str, status: &str) -> String {
    match mark(status) {
        None => text_fit::line(name, text_fit::ROW),
        Some(m) => {
            let room = text_fit::ROW - text_fit::width(&m) - text_fit::width(" ");
            format!("{} {m}", text_fit::line(name, room))
        }
    }
}

/// The Live menu row's text for `new` new mods.
pub fn menu_text(new: usize) -> String {
    match new {
        0 => "Mod Downloader".into(),
        n if n > 99 => "Mod Downloader (99+ new)".into(),
        n => format!("Mod Downloader ({n} new)"),
    }
}

/// What the list says when there is none.
pub fn failed_text(e: &ModsError) -> String {
    match e {
        ModsError::Offline => "Live is off".into(),
        ModsError::NotSignedIn => "Sign in to Live first".into(),
        ModsError::Busy => "Live is busy".into(),
        ModsError::Server(_) | ModsError::Refused(_) => "Can't reach the server".into(),
    }
}

/// What the Download row says after a failed install.
pub fn failed_install_text(e: &ModsError) -> String {
    match e {
        ModsError::Refused(Refusal::NoMapPack { map, .. }) => {
            format!("Needs the Map Pack's {}", mods::base_title(map))
        }
        ModsError::Refused(Refusal::NoBase { map, .. }) if content::map_pack::is_map(map) => {
            format!("Needs the Map Pack's {}", mods::base_title(map))
        }
        ModsError::Refused(Refusal::NoBase { map, .. }) => {
            format!("Needs {} from your disc", mods::base_title(map))
        }
        ModsError::Refused(Refusal::TakenDown(_)) => "Taken down".into(),
        ModsError::Refused(Refusal::Unpublished(_)) => "Not on the server".into(),
        ModsError::NotSignedIn => "Sign in to Live first".into(),
        ModsError::Offline => "Live is off".into(),
        _ => "Download failed".into(),
    }
}

fn percent(done: u64, total: u64) -> String {
    let p = if total == 0 {
        0
    } else {
        (u128::from(done) * 100 / u128::from(total)).min(100)
    };
    format!("Downloading {p}%")
}

impl Repo {
    /// The commands the focused row can perform right now. X on a list
    /// only installs. Removal and its confirmation belong to the map page.
    fn prompts(&self, screen: &str, row: Option<i16>) -> Vec<Prompt> {
        let g = self.lock();
        let bulk = |maps: &[ModEntry]| {
            maps.iter()
                .any(|e| Self::download_prompt(&g, e).is_some())
                .then_some("Download All")
        };
        let (a, x) = match (screen, row) {
            (SCREEN, Some(r)) => match Self::top_at(&g, r) {
                Some(Top::Map(e)) => (Some("Details"), Self::download_prompt(&g, &e)),
                Some(Top::Pack(_, maps)) => (Some("Open"), bulk(&maps)),
                None if r == 0 => match &g.list {
                    List::Failed(_) => (Some("Retry"), None),
                    List::Loaded(_) => (Some("Refresh"), None),
                    List::Loading => (None, None),
                },
                None => (None, None),
            },
            (PACK, Some(r)) => match Self::pack_maps(&g) {
                Some(maps) if r == 0 => {
                    let action = bulk(&maps).or_else(|| {
                        maps.iter()
                            .all(|e| Self::is_here(&g, &e.id))
                            .then(|| pack_action_label(&maps))
                            .flatten()
                    });
                    (action, action)
                }
                Some(maps) if r > 0 => match maps.get(r as usize - 1) {
                    Some(e) => (Some("Details"), Self::download_prompt(&g, e)),
                    None => (None, None),
                },
                _ => (None, None),
            },
            (DETAIL, Some(0)) => {
                let action = g.selected.and_then(|id| {
                    let List::Loaded(items) = &g.list else {
                        return None;
                    };
                    let e = items.iter().find(|e| e.id == id)?;
                    if g.removing == Some(id) {
                        Some("Confirm")
                    } else if Self::is_here(&g, &id) {
                        Some("Remove")
                    } else {
                        Self::download_prompt(&g, e)
                    }
                });
                (action, action)
            }
            _ => (None, None),
        };
        let mut prompts = Vec::new();
        if let Some(text) = a {
            prompts.push(Prompt::new(&[Glyph::A], if a == x { "or" } else { text }));
        }
        if let Some(text) = x {
            prompts.push(Prompt::new(&[Glyph::X], text));
        }
        prompts.push(Prompt::new(&[Glyph::B], "Back"));
        prompts
    }

    fn footer(&self, screen: &str, row: Option<i16>) -> u32 {
        let slot = match screen {
            DETAIL => DETAIL_FOOTER,
            PACK => PACK_FOOTER,
            _ => LIST_FOOTER,
        };
        self.texts
            .set(slot, &footer_legend(&self.prompts(screen, row)))
    }
}

/// The last-seen position kept at `path` (0 when none).
pub fn read_seen(path: &std::path::Path) -> u64 {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| t.parse::<toml::Table>().ok())
        .and_then(|t| t.get("seen").and_then(|v| v.as_integer()))
        .and_then(|n| u64::try_from(n).ok())
        .unwrap_or(0)
}

/// Keeps `seen` at `path`.
pub fn write_seen(path: &std::path::Path, seen: u64) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("toml.part");
    std::fs::write(
        &tmp,
        format!("# The newest mod repository position seen on the Mods screen.\nseen = {seen}\n"),
    )?;
    std::fs::rename(tmp, path)
}

// ---- the screens ----

/// The last change each screen's descriptions were set for: a newer one
/// sets them again ([`Rows::live_text`]).
#[derive(Default)]
struct Drawn(AtomicU64);

impl Drawn {
    fn stale(&self, repo: &Repo) -> bool {
        let now = repo.changed.load(Ordering::SeqCst);
        self.0.swap(now, Ordering::SeqCst) != now
    }
}

/// The list.
pub struct ListRows(pub Arc<Repo>, Drawn);

impl ListRows {
    pub fn new(repo: Arc<Repo>) -> Self {
        ListRows(repo, Drawn::default())
    }
    fn set_description(&self, row: i16) -> Option<u32> {
        let r = u32::try_from(row)
            .ok()
            .filter(|r| (*r as usize) < MAX_ROWS)?;
        let text = self.0.list_description(row).unwrap_or_default();
        Some(self.0.texts.set(LIST_DESCRIPTIONS + r, &text))
    }
}

impl Rows for ListRows {
    fn content_spec(&self) -> Option<crate::ui_kit::spec::ScreenSpec> {
        Some(spec::list_spec(&self.0))
    }

    fn restore_missing_picture(&self) -> bool {
        true
    }
    fn focused_footer(&self, row: Option<i16>) -> Option<u32> {
        Some(self.0.footer(SCREEN, row))
    }
    fn title(&self) -> u32 {
        Slots::id(LIST_TITLE)
    }
    fn count(&self) -> usize {
        self.0.list_count()
    }
    fn growing_capacity(&self) -> Option<usize> {
        Some(MAX_ROWS)
    }
    fn texts(&self, row: i16) -> Vec<(u32, u32)> {
        let Some(line) = self.0.list_line(row) else {
            return Vec::new();
        };
        let r = row as u32;
        self.set_description(row);
        vec![(
            crate::ui::LABEL_SLOT,
            self.0.texts.set(LIST_ROWS + r, &line),
        )]
    }
    fn description(&self, row: i16) -> Option<u32> {
        self.set_description(row)
    }
    fn picture(&self, row: i16) -> Option<u32> {
        let e = match Repo::top_at(&self.0.lock(), row) {
            Some(Top::Map(e)) => Some(e),
            Some(Top::Pack(_, maps)) => maps.into_iter().next(),
            None => None,
        };
        self.0.picture_of(e)
    }
    fn live_text(&self) -> LiveText {
        let redraw = self.1.stale(&self.0);
        if redraw {
            for r in 0..self.0.list_count() as i16 {
                self.set_description(r);
            }
        }
        LiveText {
            writes: Vec::new(),
            redraw,
        }
    }
    fn press(&self, row: i16) -> Chosen {
        self.0.press_list(row)
    }
    fn write_texts(&self, m: &mut dyn GuestMemory) -> Result<(), MemFault> {
        self.0.read_pictures(m);
        self.0.texts.write(m)
    }
    fn describe(&self, count: usize) -> Option<String> {
        Some(describe("Mod Downloader", count, |r| self.0.list_line(r)))
    }
}

/// A pack's list.
pub struct PackRows(pub Arc<Repo>, Drawn);

impl PackRows {
    pub fn new(repo: Arc<Repo>) -> Self {
        PackRows(repo, Drawn::default())
    }
    fn set_description(&self, row: i16) -> Option<u32> {
        let r = u32::try_from(row)
            .ok()
            .filter(|r| (*r as usize) < MAX_PACK_ROWS)?;
        let text = self.0.pack_description(row).unwrap_or_default();
        Some(self.0.texts.set(PACK_DESCRIPTIONS + r, &text))
    }
}

impl Rows for PackRows {
    fn content_spec(&self) -> Option<crate::ui_kit::spec::ScreenSpec> {
        Some(spec::pack_spec(&self.0))
    }

    fn restore_missing_picture(&self) -> bool {
        true
    }
    fn focused_footer(&self, row: Option<i16>) -> Option<u32> {
        Some(self.0.footer(PACK, row))
    }
    fn title(&self) -> u32 {
        Slots::id(PACK_TITLE)
    }
    fn count(&self) -> usize {
        self.0.pack_count()
    }
    fn texts(&self, row: i16) -> Vec<(u32, u32)> {
        let Some(line) = self.0.pack_line(row) else {
            return Vec::new();
        };
        let r = row as u32;
        self.set_description(row);
        vec![(
            crate::ui::LABEL_SLOT,
            self.0.texts.set(PACK_ROWS + r, &line),
        )]
    }
    fn description(&self, row: i16) -> Option<u32> {
        self.set_description(row)
    }
    fn picture(&self, row: i16) -> Option<u32> {
        let e = match row {
            0 => self.0.pack().and_then(|(_, maps)| maps.into_iter().next()),
            r => self.0.pack_entry(r),
        };
        self.0.picture_of(e)
    }
    fn live_text(&self) -> LiveText {
        let redraw = self.1.stale(&self.0);
        if redraw {
            for r in 0..self.0.pack_count() as i16 {
                self.set_description(r);
            }
        }
        LiveText {
            writes: Vec::new(),
            redraw,
        }
    }
    fn press(&self, row: i16) -> Chosen {
        self.0.press_pack(row)
    }
    fn write_texts(&self, m: &mut dyn GuestMemory) -> Result<(), MemFault> {
        self.0.read_pictures(m);
        self.0.texts.write(m)
    }
    fn describe(&self, count: usize) -> Option<String> {
        let title = self.0.texts.get(PACK_TITLE).unwrap_or_default();
        Some(describe(&title, count, |r| self.0.pack_line(r)))
    }
}

/// A mod's page.
pub struct DetailRows(pub Arc<Repo>, Drawn);

impl DetailRows {
    pub fn new(repo: Arc<Repo>) -> Self {
        DetailRows(repo, Drawn::default())
    }
    fn set_description(&self, row: i16) -> Option<u32> {
        let r = u32::try_from(row)
            .ok()
            .filter(|r| (*r as usize) < MAX_PAGE_ROWS)?;
        let text = self.0.detail_description(row).unwrap_or_default();
        Some(self.0.texts.set(DETAIL_DESCRIPTIONS + r, &text))
    }
}

impl Rows for DetailRows {
    fn content_spec(&self) -> Option<crate::ui_kit::spec::ScreenSpec> {
        Some(spec::detail_spec(&self.0))
    }

    fn restore_missing_picture(&self) -> bool {
        true
    }
    fn focused_footer(&self, row: Option<i16>) -> Option<u32> {
        Some(self.0.footer(DETAIL, row))
    }
    fn title(&self) -> u32 {
        Slots::id(DETAIL_TITLE)
    }
    fn count(&self) -> usize {
        MAX_PAGE_ROWS
    }
    fn texts(&self, row: i16) -> Vec<(u32, u32)> {
        let Some(line) = self.0.detail_line(row) else {
            return Vec::new();
        };
        let r = row as u32;
        self.set_description(row);
        vec![(
            crate::ui::LABEL_SLOT,
            self.0.texts.set(DETAIL_ROWS + r, &line),
        )]
    }
    fn description(&self, row: i16) -> Option<u32> {
        self.set_description(row)
    }
    fn picture(&self, _row: i16) -> Option<u32> {
        self.0.picture_of(self.0.selected())
    }
    fn live_text(&self) -> LiveText {
        let redraw = self.1.stale(&self.0);
        if redraw {
            for r in 0..self.count() as i16 {
                self.set_description(r);
            }
        }
        LiveText {
            writes: Vec::new(),
            redraw,
        }
    }
    fn press(&self, row: i16) -> Chosen {
        self.0.press_detail(row)
    }
    fn panel_text(&self, i: usize) -> Option<u32> {
        (i < page_layout::TEXTS).then(|| Slots::id(PANEL_TEXTS + i as u32))
    }
    fn write_texts(&self, m: &mut dyn GuestMemory) -> Result<(), MemFault> {
        self.0.read_pictures(m);
        self.0.texts.write(m)
    }
    fn describe(&self, count: usize) -> Option<String> {
        let title = self.0.texts.get(DETAIL_TITLE).unwrap_or_default();
        Some(describe(&title, count, |r| self.0.detail_line(r)))
    }
}

fn describe(title: &str, count: usize, row: impl Fn(i16) -> Option<String>) -> String {
    let mut out = format!("title {title:?}; rows:");
    for r in 0..count as i16 {
        if let Some(l) = row(r) {
            out.push_str(&format!(" [{r}] {l:?}"));
        }
    }
    out
}

/// The texts' memory, the locale source and the frame tick.
pub struct ModTexts(pub Arc<Repo>);

impl hooks::Feature for ModTexts {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new(
            FEATURE,
            "The mod repository: the Live menu's Mods row and screens",
        )
        .doc("docs/game/maps.md")
    }
    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        let base = b.guest_alloc(TEXTS.len * SLOT_LEN, Permissions::RW)?;
        self.0.texts.base.store(base, Ordering::SeqCst);
        let repo = Arc::clone(&self.0);
        b.locale().source("mod texts", move |id| repo.string(id));
        let repo = Arc::clone(&self.0);
        b.frame().on_tick("mod texts", move |o| {
            let mut g = o.guest();
            let _ = repo.texts.write(&mut *g);
        });
        let (live, save, load) = (
            Arc::clone(&self.0),
            Arc::clone(&self.0),
            Arc::clone(&self.0),
        );
        b.snapshot(hooks::SnapshotPolicy::section_forbid_while(
            move || live.snapshot_live(),
            move || save.save_state(),
            move |bytes, _| load.load_state(bytes),
        ));
        Ok(())
    }
}

static REPO: OnceLock<Arc<Repo>> = OnceLock::new();

/// The repository, once installed (Live on).
pub fn repo() -> Option<&'static Arc<Repo>> {
    REPO.get()
}

/// X on the list: the focused map downloads.
fn list_action(repo: &Arc<Repo>) -> Action {
    let repo = Arc::clone(repo);
    Arc::new(move |cx, screen| {
        let repo = Arc::clone(&repo);
        Box::pin(async move {
            let row = crate::list_class::focused_row(&cx, screen).await?;
            eprintln!("{}", repo.x_list(row));
            Ok(())
        })
    })
}

/// X on a pack's list: the focused map downloads, or (on Download All)
/// every map not here.
fn pack_action(repo: &Arc<Repo>) -> Action {
    let repo = Arc::clone(repo);
    Arc::new(move |cx, screen| {
        let repo = Arc::clone(&repo);
        Box::pin(async move {
            let row = crate::list_class::focused_row(&cx, screen).await?;
            eprintln!("{}", repo.x_pack(row));
            Ok(())
        })
    })
}

/// X on a map's page: download, or remove.
fn page_action(repo: &Arc<Repo>) -> Action {
    let repo = Arc::clone(repo);
    Arc::new(move |_cx, _screen| {
        let repo = Arc::clone(&repo);
        Box::pin(async move {
            eprintln!("{}", repo.act());
            Ok(())
        })
    })
}

/// X only: Start keeps doing nothing on these screens.
const X_ONLY: [u32; 1] = [crate::list_class::X_BUTTON];

/// The three screens' classes over `repo`: the list and a pack's list on
/// the level select's list, bracket, picture and description, as the
/// campaign picker has them; a map's page on the pregame lobby's layout
/// ([`page_layout`]).
pub fn classes(repo: &Arc<Repo>) -> [ListClass; 3] {
    [
        crate::skulls_menu::level_select_list(
            ListClass::with_rows(
                "mods-screen",
                SCREEN,
                Arc::new(ListRows::new(Arc::clone(repo))),
            ),
            crate::skulls_menu::LIST_LAYOUT,
        )
        .description(crate::skulls_menu::DESCRIPTION_TEXT)
        .picture(LIST_PICTURE)
        .action_buttons(&X_ONLY)
        .action(list_action(repo))
        .on_free({
            let repo = Arc::clone(repo);
            Arc::new(move |_, _| {
                repo.closed();
                Box::pin(async { Ok(()) })
            })
        })
        .footer(Slots::id(LIST_FOOTER)),
        crate::skulls_menu::level_select_list(
            ListClass::with_rows("mods-pack", PACK, Arc::new(PackRows::new(Arc::clone(repo)))),
            crate::skulls_menu::LIST_LAYOUT,
        )
        .description(crate::skulls_menu::DESCRIPTION_TEXT)
        .picture(LIST_PICTURE)
        .action_buttons(&X_ONLY)
        .action(pack_action(repo))
        .footer(Slots::id(PACK_FOOTER)),
        ListClass::with_rows(
            "mods-detail",
            DETAIL,
            Arc::new(DetailRows::new(Arc::clone(repo))),
        )
        .layout(page_layout::LIST)
        .description(page_layout::STATUS)
        .picture(page_layout::PICTURE)
        .panel(page_layout::panel())
        .action_buttons(&X_ONLY)
        .action(page_action(repo))
        .footer(Slots::id(DETAIL_FOOTER)),
    ]
}

/// The reader: checks for a due read once a second.
fn reader(repo: std::sync::Weak<Repo>) {
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let Some(repo) = repo.upgrade() else { return };
        repo.read_if_due(Instant::now());
    }
}

/// Builds the repository, its screens and texts, and starts the reader.
/// Live off (no HLE or no map gate): nothing, and the Live menu's Mods
/// row opens the Map Pack dialog.
pub fn install(
    registry: &mut hooks::Registry,
    hle: Option<Arc<live_hle::Hle>>,
    gate: Option<Arc<live_hle::maps::MapGate>>,
    data_root: &std::path::Path,
) -> anyhow::Result<bool> {
    let (Some(hle), Some(gate)) = (hle, gate) else {
        return Ok(false);
    };
    let repo = Repo::new(
        Arc::new(LiveSource { hle, gate }),
        Some(data_root.join(SEEN_FILE)),
    );
    let _ = REPO.set(Arc::clone(&repo));
    for c in classes(&repo) {
        registry.add(&c).map_err(|e| anyhow::anyhow!("mods: {e}"))?;
    }
    registry
        .add(&ModTexts(Arc::clone(&repo)))
        .map_err(|e| anyhow::anyhow!("mods: {e}"))?;
    let weak = Arc::downgrade(&repo);
    background::spawn("h2-mods", move || reader(weak))?;
    Ok(true)
}

pub mod page_layout;
pub mod spec;
use crate::ui_kit::text_fit;

#[cfg(test)]
mod tests;
