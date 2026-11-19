//! The mod repository in game (docs/game/maps.md §5, "Mod downloads"): the
//! Xbox Live menu's last row, where retail had Content Download, says
//! "Mods" (or "Mods (3 new)") and opens a list of the maps the Live server
//! publishes, newest first; each has a page of its own.
//!
//! **The list** is laid out as the level select is: the maps on the left,
//! the focused map's picture on the right and its summary under it (base
//! map, who published it, size, downloads, what it changes, its state).
//! A opens the map's page; X downloads the focused map there and then.
//!
//! **A map's page** (docs/game/maps.md §5, "A mod's page"), the same
//! layout: Download (or Remove, once it is here) first, then the base map,
//! publisher, size, downloads, date, version and game types, then the
//! checklist of what it changes (`live_proto::maps::FEATURES`), whether it
//! needs the Map Pack and that guests get it when they join. The focused
//! row's text says more: the mod's description, what a feature means, or
//! what Download will do. A on Download, or X anywhere, downloads; on an
//! installed map they remove it, after a second press.
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
//! (the Live worker reads it with the signed-in account's token) when the
//! Live menu is drawn or the screen opens, and every five minutes; the
//! download runs on a thread of its own. The screens only read the state
//! ([`Repo`]) and every text is ours, written into guest memory slots when
//! it changes (on the frame tick and before a row's texts are set).
//!
//! The state is pure over a [`Source`] and tested without a guest or a
//! server.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use content::MapId;
use engine::memory::{GuestMemory, MemFault};
use live_hle::maps::Refusal;
use live_hle::mods::{self, InstallDone, ListDone, ModEntry, ModPage, ModsError, Progress};
use live_proto::maps::FEATURES;
use runtime::guest::Permissions;

use crate::list_class::{Action, ListClass, LiveText, Rows};
use crate::ui::Chosen;
use crate::ui_kit::ids::Family;
use crate::ui_kit::input::{Glyph, Prompt, footer_legend};

/// The screens' ids (`list_class::loader_of`).
pub const SCREEN: &str = "mods";
pub const DETAIL: &str = "mods.detail";
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
/// Slot layout: the Live menu's label, the two titles and footers, then a
/// line and a description per row of the list and of the page.
const MENU_SLOT: u32 = 0;
const LIST_TITLE: u32 = 1;
const LIST_FOOTER: u32 = 2;
const DETAIL_TITLE: u32 = 3;
const DETAIL_FOOTER: u32 = 4;
const LIST_ROWS: u32 = 8;
const LIST_DESCRIPTIONS: u32 = LIST_ROWS + MAX_ROWS as u32;
const DETAIL_ROWS: u32 = LIST_DESCRIPTIONS + MAX_ROWS as u32;
const DETAIL_DESCRIPTIONS: u32 = DETAIL_ROWS + MAX_PAGE_ROWS as u32;
const _: () = assert!(DETAIL_DESCRIPTIONS + MAX_PAGE_ROWS as u32 <= TEXTS.len);
/// Rows the list shows at most.
pub const MAX_ROWS: usize = 40;
/// Rows a map's page has at most.
pub const MAX_PAGE_ROWS: usize = 24;
/// A row's line is cut to this many characters with "..." (the level
/// select skin's one text, 190 units of conduit-12).
pub const LINE_CHARS: usize = 28;
/// A mod's description on its page is cut to this many characters (the
/// description rectangle holds about five lines).
pub const ABOUT_CHARS: usize = 200;

/// Where the list and installs come from: the HLE and the map gate in a
/// boot, a fake in tests.
pub trait Source: Send + Sync {
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
    fn list(&self, limit: u32, done: ListDone) -> Result<(), ModsError> {
        self.hle.mod_list(limit, None, done)
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

/// One row of a map's page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageRow {
    /// Download, its progress, or Remove: what A and X do.
    Action,
    Map,
    By,
    /// The account that published it, when a credit names someone else
    /// as its maker.
    Uploader,
    Size,
    Downloads,
    Published,
    Version,
    GameTypes,
    /// `FEATURES[i]`, ticked or not.
    Feature(usize),
    /// The server lists no features for it.
    NoFeatures,
    MapPack,
    Online,
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
        "scripts" => (
            "Scripted events",
            "its scripts changed: things happen as you play",
        ),
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

/// The rows of `e`'s page.
pub fn page_rows(e: &ModEntry) -> Vec<PageRow> {
    let mut v = vec![PageRow::Action, PageRow::Map, PageRow::By];
    if uploader_apart(e) {
        v.push(PageRow::Uploader);
    }
    v.push(PageRow::Size);
    if e.downloads.is_some() {
        v.push(PageRow::Downloads);
    }
    if !e.published_at.is_empty() {
        v.push(PageRow::Published);
    }
    if e.version.is_some() {
        v.push(PageRow::Version);
    }
    v.push(PageRow::GameTypes);
    if e.features.is_empty() {
        v.push(PageRow::NoFeatures);
    } else {
        v.extend((0..FEATURES.len()).map(PageRow::Feature));
    }
    v.extend([PageRow::MapPack, PageRow::Online]);
    v.truncate(MAX_PAGE_ROWS);
    v
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
    selected: Option<MapId>,
    installed: BTreeSet<MapId>,
    installs: BTreeMap<MapId, Install>,
    /// A Remove pressed once, waiting for the second press.
    removing: Option<MapId>,
    /// When the list was last asked for, and whether that read failed.
    asked: Option<Instant>,
    failed: bool,
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

impl Repo {
    /// Over `source`, keeping the last-seen position in `seen_file`.
    pub fn new(source: Arc<dyn Source>, seen_file: Option<PathBuf>) -> Arc<Self> {
        let seen = seen_file.as_deref().map_or(0, read_seen);
        let repo = Arc::new(Repo {
            inner: Mutex::new(Inner {
                list: List::Loading,
                seen,
                shown_from: seen,
                selected: None,
                installed: BTreeSet::new(),
                installs: BTreeMap::new(),
                removing: None,
                asked: None,
                failed: false,
                in_flight: false,
                pictures: None,
            }),
            texts: Slots::default(),
            source,
            seen_file,
            wanted: AtomicBool::new(true),
            changed: AtomicU64::new(1),
        });
        repo.texts.set(LIST_TITLE, "Mods");
        repo.texts
            .set(LIST_FOOTER, &footer_legend(&prompts(SCREEN, false)));
        repo.texts
            .set(DETAIL_FOOTER, &footer_legend(&prompts(DETAIL, false)));
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

    /// The Live menu row's text: "Mods", or "Mods (N new)".
    pub fn menu_text(&self) -> String {
        let g = self.lock();
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
                let ids: Vec<MapId> = items.iter().map(|e| e.id).collect();
                Some(self.source.installed(&ids))
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
                    g.list = List::Loaded(items);
                }
                Err(e) => {
                    eprintln!("mods: no list: {e}");
                    // A list already read stays shown through a failed refresh.
                    if !matches!(g.list, List::Loaded(_)) {
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
        let seen = {
            let mut g = self.lock();
            g.shown_from = g.seen;
            g.pictures = None;
            if let List::Loaded(items) = &g.list {
                let page = ModPage {
                    items: items.clone(),
                    ..ModPage::default()
                };
                g.seen = mods::seen_after(&page, g.seen);
            }
            g.seen
        };
        if let Some(path) = &self.seen_file
            && let Err(e) = write_seen(path, seen)
        {
            eprintln!("mods: the last-seen position could not be kept: {e}");
        }
        self.want_list();
        self.touch();
        self.refresh_menu();
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

    fn entry_at(g: &Inner, row: i16) -> Option<ModEntry> {
        match &g.list {
            List::Loaded(items) => usize::try_from(row)
                .ok()
                .filter(|r| *r < MAX_ROWS)
                .and_then(|r| items.get(r))
                .cloned(),
            _ => None,
        }
    }

    /// Row `row` of the list: the map's name and its state ("New",
    /// "Installed", "Downloading N%", else its size); with no list, why.
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
        let e = Self::entry_at(&g, row)?;
        Some((e.name.clone(), Self::status(&g, &e)))
    }

    /// Row `row` of the list as its one line: the name, and its state
    /// ("New", "Installed", "45%") unless that is only its size.
    pub fn list_line(&self, row: i16) -> Option<String> {
        let (name, status) = self.list_row(row)?;
        let has_entry = Self::entry_at(&self.lock(), row).is_some();
        Some(match status.as_str() {
            _ if !has_entry => cut(&name, LINE_CHARS),
            "" => cut(&name, LINE_CHARS),
            s if s.ends_with(" MB") || s.ends_with(" KB") => cut(&name, LINE_CHARS),
            s => {
                let s = s.strip_prefix("Downloading ").unwrap_or(s);
                let room = LINE_CHARS.saturating_sub(s.chars().count() + 2);
                format!("{}  {s}", cut(&name, room))
            }
        })
    }

    fn status(g: &Inner, e: &ModEntry) -> String {
        match g.installs.get(&e.id) {
            Some(Install::Downloading { done, total }) => return percent(*done, *total),
            Some(Install::Done) => return "Installed".into(),
            Some(Install::Removed) => return mods::size_text(e.size),
            Some(Install::Failed(_) | Install::RemoveFailed(_)) | None => {}
        }
        if g.installed.contains(&e.id) {
            "Installed".into()
        } else if mods::is_new(e, g.shown_from) {
            "New".into()
        } else {
            mods::size_text(e.size)
        }
    }

    fn is_here(g: &Inner, id: &MapId) -> bool {
        match g.installs.get(id) {
            Some(Install::Done) => true,
            Some(Install::Removed) => false,
            _ => g.installed.contains(id),
        }
    }

    /// The list's description of row `row`: base map and publisher, size
    /// and downloads, what it changes, and its state.
    pub fn list_description(&self, row: i16) -> Option<String> {
        let g = self.lock();
        let e = Self::entry_at(&g, row)?;
        let mut lines = vec![format!(
            "On {}, by {}",
            mods::base_title(&e.base),
            made_by(&e)
        )];
        lines.push(match e.downloads {
            Some(1) => format!("{}, 1 download", mods::size_text(e.size)),
            Some(n) => format!("{}, {n} downloads", mods::size_text(e.size)),
            None => mods::size_text(e.size),
        });
        lines.push(if e.features.is_empty() {
            "What it changes isn't listed".into()
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
            _ => match Self::status(&g, &e).as_str() {
                "Installed" => "Installed: A for its page".into(),
                s if s.starts_with("Downloading") => s.into(),
                _ => "X downloads it, A opens its page".into(),
            },
        });
        Some(lines.join("\n"))
    }

    /// The rows the list opens with.
    pub fn list_count(&self) -> usize {
        match &self.lock().list {
            List::Loaded(items) => items.len().clamp(1, MAX_ROWS),
            _ => 1,
        }
    }

    /// A press on the list's row `row`: the map's page.
    pub fn press_list(&self, row: i16) -> Chosen {
        let picked = {
            let mut g = self.lock();
            let picked = Self::entry_at(&g, row);
            if let Some(e) = &picked {
                g.selected = Some(e.id);
                g.removing = None;
            }
            picked
        };
        match picked {
            Some(e) => {
                self.texts.set(DETAIL_TITLE, &cut(&e.name, 40));
                let here = Self::is_here(&self.lock(), &e.id);
                self.texts
                    .set(DETAIL_FOOTER, &footer_legend(&prompts(DETAIL, here)));
                self.touch();
                Chosen::Open(DETAIL)
            }
            None => {
                self.want_list();
                Chosen::Stepped("mods: reading the list again".into())
            }
        }
    }

    /// X on the list's row `row`: that map downloads, its page unopened.
    /// An installed map is left alone (Remove is on its page).
    pub fn x_list(self: &Arc<Self>, row: Option<i16>) -> String {
        let Some(e) = row.and_then(|r| Self::entry_at(&self.lock(), r)) else {
            return "mods: X with no map focused".into();
        };
        if Self::is_here(&self.lock(), &e.id) {
            return format!("mods: {} is installed; its page removes it", e.name);
        }
        self.download(&e)
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

    /// The page's rows, for the selected map.
    pub fn page(&self) -> Vec<PageRow> {
        self.selected().map(|e| page_rows(&e)).unwrap_or_default()
    }

    /// Row `row` of the page as its one line.
    pub fn detail_line(&self, row: i16) -> Option<String> {
        let e = self.selected()?;
        let r = *page_rows(&e).get(usize::try_from(row).ok()?)?;
        let g = self.lock();
        let tick = |on: bool| if on { "[x]" } else { "[ ]" };
        let line = match r {
            PageRow::Action => self.action_text(&g, &e),
            PageRow::Map => format!("Map: {}", mods::base_title(&e.base)),
            PageRow::By => format!("By: {}", made_by(&e)),
            PageRow::Uploader => format!("Uploaded By: {}", e.author),
            PageRow::Size => format!("Size: {}", mods::size_text(e.size)),
            PageRow::Downloads => format!("Downloads: {}", e.downloads.unwrap_or(0)),
            PageRow::Published => format!("Published: {}", e.published_at.get(..10).unwrap_or("")),
            PageRow::Version => format!("Version: {}", e.version.as_deref().unwrap_or("")),
            PageRow::GameTypes => format!("For: {}", gametypes_text(&e.gametypes)),
            PageRow::Feature(i) => {
                let id = FEATURES[i];
                format!(
                    "{} {}",
                    tick(e.features.iter().any(|f| f == id)),
                    feature_text(id).0
                )
            }
            PageRow::NoFeatures => "Changes: not listed".into(),
            PageRow::MapPack => {
                format!("{} Map Pack map", tick(content::map_pack::is_map(&e.base)))
            }
            PageRow::Online => format!("{} Joins get it", tick(true)),
        };
        Some(cut(&line, LINE_CHARS))
    }

    /// The page's description of row `row`.
    pub fn detail_description(&self, row: i16) -> Option<String> {
        let e = self.selected()?;
        let r = *page_rows(&e).get(usize::try_from(row).ok()?)?;
        let g = self.lock();
        let base = mods::base_title(&e.base);
        Some(match r {
            PageRow::Action => match g.installs.get(&e.id) {
                Some(Install::Downloading { .. }) => format!(
                    "{} of {}. You can leave this page; it carries on.",
                    percent_of(&g, &e),
                    mods::size_text(e.size)
                ),
                Some(Install::Failed(err)) => format!(
                    "{}. A or X tries again.",
                    failed_install_text(err).trim_end_matches("; press A to retry")
                ),
                _ if g.removing == Some(e.id) => {
                    "Press A or X again to remove it from this console.".into()
                }
                Some(Install::RemoveFailed(why)) => format!("Not removed: {}", cut(why, 120)),
                _ if Self::is_here(&g, &e.id) => {
                    "Installed. Host a game and pick it under Mods in Change Map. A or X removes it."
                        .into()
                }
                _ => format!(
                    "A or X downloads it ({}). Then host a game on {base} and pick it under Mods in Change Map.",
                    mods::size_text(e.size)
                ),
            },
            PageRow::Feature(i) => {
                let id = FEATURES[i];
                let (name, means) = feature_text(id);
                if e.features.iter().any(|f| f == id) {
                    format!("{name}: {means}.")
                } else {
                    format!("{name}: not in this mod ({means}).")
                }
            }
            PageRow::NoFeatures => {
                "The server doesn't list what this mod changes: it was published before the checklist."
                    .into()
            }
            PageRow::MapPack if content::map_pack::is_map(&e.base) => format!(
                "Built on {base} from the Multiplayer Map Pack: it needs the map pack installed (h2 import-mappack)."
            ),
            PageRow::MapPack => format!("Built on {base} from your disc."),
            PageRow::Online => {
                "Players who join your game on it download it on the way in, from your Live server."
                    .into()
            }
            PageRow::GameTypes if e.gametypes.is_empty() => {
                "Its author names no game type: play anything on it.".into()
            }
            PageRow::GameTypes => format!("Made for {}.", gametypes_text(&e.gametypes)),
            PageRow::Uploader => format!(
                "{} shared it on your Live server; {} made it.",
                e.author,
                made_by(&e)
            ),
            _ => about(&e),
        })
    }

    fn action_text(&self, g: &Inner, e: &ModEntry) -> String {
        match g.installs.get(&e.id) {
            Some(Install::Downloading { done, total }) => percent(*done, *total),
            _ if g.removing == Some(e.id) => "Remove? Press again".into(),
            Some(Install::Failed(err)) => failed_install_text(err),
            Some(Install::RemoveFailed(_)) => "Remove failed".into(),
            _ if Self::is_here(g, &e.id) => "Remove".into(),
            _ => "Download".into(),
        }
    }

    /// A on the page's row `row`: the Download row downloads, or removes.
    pub fn press_detail(self: &Arc<Self>, row: i16) -> Chosen {
        let is_action = usize::try_from(row).is_ok_and(|r| {
            self.selected()
                .is_some_and(|e| page_rows(&e).get(r) == Some(&PageRow::Action))
        });
        if !is_action {
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
        let id = e.id;
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
                }
            }),
        );
        if let Err(err) = r {
            self.install_done(id, Err(err));
        }
        format!("mods: downloading {} ({})", e.name, e.id)
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
        let (here, on_page) = {
            let mut g = self.lock();
            g.pictures = None;
            (Self::is_here(&g, &e.id), g.selected == Some(e.id))
        };
        if on_page {
            self.texts
                .set(DETAIL_FOOTER, &footer_legend(&prompts(DETAIL, here)));
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
    /// whose page is open. The list and what is installed are the
    /// server's and the disk's, read again; the texts are written again.
    pub fn save_state(&self) -> Vec<u8> {
        let selected = self.lock().selected;
        let mut w = snapshot::codec::Writer::new();
        w.bool(selected.is_some())
            .raw(&selected.map_or([0; 32], |id| id.0));
        w.into_bytes()
    }

    /// Puts [`Self::save_state`]'s bytes back, with the page's title when
    /// the mod is in this run's list, and every text written again.
    pub fn load_state(&self, bytes: &[u8]) -> Result<(), String> {
        let e = |e: snapshot::Error| e.to_string();
        let r = &mut snapshot::codec::Reader::new(bytes, "mods");
        let some = r.bool().map_err(e)?;
        let id = MapId(r.array::<32>().map_err(e)?);
        r.finish().map_err(e)?;
        {
            let mut g = self.lock();
            g.selected = some.then_some(id);
            g.pictures = None;
        }
        if let Some(entry) = self.selected() {
            self.texts.set(DETAIL_TITLE, &cut(&entry.name, 40));
        }
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

/// The mod's own words on its page: its description (cut), and who made it.
fn about(e: &ModEntry) -> String {
    let text = e
        .description
        .as_deref()
        .map(|d| cut(d.trim(), ABOUT_CHARS))
        .unwrap_or_else(|| "No description.".into());
    format!("{text}\nBy {}.", made_by(e))
}

fn percent_of(g: &Inner, e: &ModEntry) -> String {
    match g.installs.get(&e.id) {
        Some(Install::Downloading { done, total }) => percent(*done, *total),
        _ => String::new(),
    }
}

/// The Live menu row's text for `new` new mods.
pub fn menu_text(new: usize) -> String {
    match new {
        0 => "Mods".into(),
        n if n > 99 => "Mods (99+ new)".into(),
        n => format!("Mods ({n} new)"),
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
        _ => "Failed; press A to retry".into(),
    }
}

fn percent(done: u64, total: u64) -> String {
    let p = if total == 0 {
        0
    } else {
        (done.saturating_mul(100) / total).min(100)
    };
    format!("Downloading {p}%")
}

/// `s` cut to `max` characters, with "..." when cut.
pub fn cut(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(3)).collect();
    out.push_str("...");
    out
}

/// The footer legends: the list's A opens a map's page and X downloads;
/// the page's A and X download, or remove a map that is `here`.
pub fn prompts(screen: &str, here: bool) -> Vec<Prompt> {
    if screen == DETAIL {
        vec![
            Prompt::new(&[Glyph::A], "Select"),
            Prompt::new(&[Glyph::X], if here { "Remove" } else { "Download" }),
            Prompt::new(&[Glyph::B], "Back"),
        ]
    } else {
        vec![
            Prompt::new(&[Glyph::A], "Details"),
            Prompt::new(&[Glyph::X], "Download"),
            Prompt::new(&[Glyph::B], "Back"),
        ]
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
    fn title(&self) -> u32 {
        Slots::id(LIST_TITLE)
    }
    fn count(&self) -> usize {
        self.0.list_count()
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
        let e = Repo::entry_at(&self.0.lock(), row);
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
        Some(describe("Mods", count, |r| self.0.list_line(r)))
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
    fn title(&self) -> u32 {
        Slots::id(DETAIL_TITLE)
    }
    fn count(&self) -> usize {
        self.0.page().len().max(1)
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
    for r in 0..count.min(MAX_ROWS) as i16 {
        if let Some(l) = row(r) {
            out.push_str(&format!(" [{r}] {l:?}"));
        }
    }
    out
}

/// The texts' memory, the locale source and the frame tick.
pub struct ModTexts(pub Arc<Repo>);

impl feature::Feature for ModTexts {
    fn manifest(&self) -> feature::Manifest {
        feature::Manifest::new(
            FEATURE,
            "The mod repository: the Live menu's Mods row and screens",
        )
        .doc("docs/game/maps.md")
    }
    fn build(&self, b: &mut feature::Builder) -> Result<(), feature::BuildError> {
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
        b.snapshot(feature::SnapshotPolicy::section_forbid_while(
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

/// The two screens' classes over `repo`: the level select's list, bracket,
/// picture and description, as the campaign picker has them.
pub fn classes(repo: &Arc<Repo>) -> [ListClass; 2] {
    let level_select = |c: ListClass| {
        c.layout(crate::skulls_menu::LIST_LAYOUT)
            .description(crate::skulls_menu::DESCRIPTION_TEXT)
            .frame(crate::skulls_menu::FRAME)
            .picture(crate::campaign_menu::PICTURE)
            .action_buttons(&X_ONLY)
    };
    [
        level_select(ListClass::with_rows(
            "mods-screen",
            SCREEN,
            Arc::new(ListRows::new(Arc::clone(repo))),
        ))
        .action(list_action(repo))
        .footer(Slots::id(LIST_FOOTER)),
        level_select(ListClass::with_rows(
            "mods-detail",
            DETAIL,
            Arc::new(DetailRows::new(Arc::clone(repo))),
        ))
        .action(page_action(repo))
        .footer(Slots::id(DETAIL_FOOTER)),
    ]
}

/// The reader: asks for the list whenever [`Repo::due`].
fn reader(repo: std::sync::Weak<Repo>) {
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let Some(repo) = repo.upgrade() else { return };
        let now = Instant::now();
        if repo.due(now) {
            repo.fetch(now);
        }
    }
}

/// Builds the repository, its screens and texts, and starts the reader.
/// Live off (no HLE or no map gate), or `H2_NO_MODS_ROW=1`: nothing, and
/// the Live menu keeps Content Download's Map Pack dialog.
pub fn install(
    registry: &mut feature::Registry,
    hle: Option<Arc<live_hle::Hle>>,
    gate: Option<Arc<live_hle::maps::MapGate>>,
    data_root: &std::path::Path,
) -> anyhow::Result<bool> {
    let (Some(hle), Some(gate)) = (hle, gate) else {
        return Ok(false);
    };
    if std::env::var_os("H2_NO_MODS_ROW").is_some() {
        return Ok(false);
    }
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
    std::thread::Builder::new()
        .name("h2-mods".into())
        .spawn(move || reader(weak))?;
    Ok(true)
}

#[cfg(test)]
mod tests;
