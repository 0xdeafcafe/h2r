//! The mod repository in game (docs/game/maps.md §5, "Mod downloads"): the
//! Xbox Live menu's last row, where retail had Content Download, says
//! "Mods" (or "Mods (3 new)") and opens a list of the maps the Live server
//! publishes, newest first; a map's page downloads and installs it into
//! `<data>/maps` through the join's own map gate.
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
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use content::MapId;
use engine::memory::{GuestMemory, MemFault};
use live_hle::maps::Refusal;
use live_hle::mods::{self, InstallDone, ListDone, ModEntry, ModPage, ModsError, Progress};
use runtime::guest::Permissions;

use crate::list_class::{ListClass, Rows};
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
pub const SLOT_LEN: u32 = 128;
/// Slot layout: the Live menu's label, the two titles and footers, then a
/// label and a value per row of the list and of the detail page.
const MENU_SLOT: u32 = 0;
const LIST_TITLE: u32 = 1;
const LIST_FOOTER: u32 = 2;
const DETAIL_TITLE: u32 = 3;
const DETAIL_FOOTER: u32 = 4;
const LIST_ROWS: u32 = 8;
const DETAIL_ROWS: u32 = 96;
/// Rows the list shows at most (two slots each).
pub const MAX_ROWS: usize = 40;
pub const DETAIL_COUNT: usize = 6;
/// The detail page's rows. Download first, so A on opening downloads.
/// Author is who made the mod (the server's credit; "Unknown" when it has
/// none), never the account that uploaded it, which is Uploaded By.
pub const ROW_DOWNLOAD: i16 = 0;
pub const ROW_AUTHOR: i16 = 1;
pub const ROW_MAP: i16 = 2;
pub const ROW_SIZE: i16 = 3;
pub const ROW_PUBLISHED: i16 = 4;
pub const ROW_UPLOADER: i16 = 5;
/// A credit longer than this is cut with "..." (the value column).
pub const CREDIT_CHARS: usize = 28;
/// A name longer than this is cut with "..." (the label column is 245
/// units of conduit-13).
pub const NAME_CHARS: usize = 24;

/// Where the list and installs come from: the HLE and the map gate in a
/// boot, a fake in tests.
pub trait Source: Send + Sync {
    fn list(&self, limit: u32, done: ListDone) -> Result<(), ModsError>;
    fn install(&self, id: MapId, progress: Progress, done: InstallDone) -> Result<(), ModsError>;
    /// The maps of `ids` a package here already has.
    fn installed(&self, ids: &[MapId]) -> BTreeSet<MapId>;
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
}

/// The list as last read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum List {
    Loading,
    Loaded(Vec<ModEntry>),
    Failed(ModsError),
}

/// A download, as the detail page shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Install {
    Downloading { done: u64, total: u64 },
    Done,
    Failed(ModsError),
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
    /// When the list was last asked for, and whether that read failed.
    asked: Option<Instant>,
    failed: bool,
    in_flight: bool,
}

/// The mod repository's state, shared by the screens, the Live menu row
/// and the threads.
pub struct Repo {
    inner: Mutex<Inner>,
    pub texts: Slots,
    source: Arc<dyn Source>,
    seen_file: Option<PathBuf>,
    wanted: AtomicBool,
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
                asked: None,
                failed: false,
                in_flight: false,
            }),
            texts: Slots::default(),
            source,
            seen_file,
            wanted: AtomicBool::new(true),
        });
        repo.texts.set(LIST_TITLE, "Mods");
        repo.texts
            .set(LIST_FOOTER, &footer_legend(&prompts(SCREEN)));
        repo.texts
            .set(DETAIL_FOOTER, &footer_legend(&prompts(DETAIL)));
        repo.refresh_menu();
        repo
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
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
        self.refresh_menu();
    }

    /// The screen is opening: everything listed is seen from now on (kept
    /// on disk), but tagged "New" for this visit; a fresh list is asked for.
    pub fn opened(&self) {
        let seen = {
            let mut g = self.lock();
            g.shown_from = g.seen;
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

    /// Row `row` of the list: its label and value.
    pub fn list_row(&self, row: i16) -> Option<(String, String)> {
        let g = self.lock();
        let items = match &g.list {
            List::Loading => return (row == 0).then(|| ("Loading...".into(), String::new())),
            List::Failed(e) => return (row == 0).then(|| (failed_text(e), "Retry".into())),
            List::Loaded(items) if items.is_empty() => {
                return (row == 0).then(|| ("No mods yet".into(), String::new()));
            }
            List::Loaded(items) => items,
        };
        let e = items
            .get(usize::try_from(row).ok()?)
            .filter(|_| (row as usize) < MAX_ROWS)?;
        Some((cut(&e.name, NAME_CHARS), self.status(&g, e)))
    }

    fn status(&self, g: &Inner, e: &ModEntry) -> String {
        match g.installs.get(&e.id) {
            Some(Install::Downloading { done, total }) => return percent(*done, *total),
            Some(Install::Done) => return "Installed".into(),
            Some(Install::Failed(_)) | None => {}
        }
        if g.installed.contains(&e.id) {
            "Installed".into()
        } else if mods::is_new(e, g.shown_from) {
            "New".into()
        } else {
            mods::size_text(e.size)
        }
    }

    /// The rows the list opens with.
    pub fn list_count(&self) -> usize {
        match &self.lock().list {
            List::Loaded(items) => items.len().clamp(1, MAX_ROWS),
            _ => 1,
        }
    }

    /// A press on the list's row `row`.
    pub fn press_list(&self, row: i16) -> Chosen {
        let picked = {
            let mut g = self.lock();
            let picked = match &g.list {
                List::Loaded(items) => usize::try_from(row)
                    .ok()
                    .and_then(|r| items.get(r))
                    .map(|e| (e.id, e.name.clone())),
                _ => None,
            };
            if let Some((id, _)) = &picked {
                g.selected = Some(*id);
            }
            picked
        };
        match picked {
            Some((_, name)) => {
                self.texts.set(DETAIL_TITLE, &cut(&name, 40));
                Chosen::Open(DETAIL)
            }
            None => {
                self.want_list();
                Chosen::Stepped("mods: reading the list again".into())
            }
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

    /// Row `row` of the detail page.
    pub fn detail_row(&self, row: i16) -> Option<(String, String)> {
        let e = self.selected()?;
        let g = self.lock();
        Some(match row {
            ROW_DOWNLOAD => ("Download".into(), self.download_text(&g, &e)),
            ROW_AUTHOR => (
                "Author".into(),
                e.credit
                    .as_deref()
                    .map_or_else(|| "Unknown".into(), |c| cut(c, CREDIT_CHARS)),
            ),
            ROW_MAP => ("Map".into(), mods::base_title(&e.base)),
            ROW_SIZE => ("Size".into(), mods::size_text(e.size)),
            ROW_PUBLISHED => match e.downloads {
                Some(n) => ("Downloads".into(), n.to_string()),
                None => (
                    "Published".into(),
                    e.published_at.get(..10).unwrap_or("").to_string(),
                ),
            },
            ROW_UPLOADER => ("Uploaded By".into(), e.author.clone()),
            _ => return None,
        })
    }

    fn download_text(&self, g: &Inner, e: &ModEntry) -> String {
        match g.installs.get(&e.id) {
            Some(Install::Downloading { done, total }) => percent(*done, *total),
            Some(Install::Done) => "Installed".into(),
            Some(Install::Failed(err)) => failed_install_text(err),
            None if g.installed.contains(&e.id) => "Installed".into(),
            None => "Press A".into(),
        }
    }

    /// A press on the detail page's row `row`: Download starts the install
    /// (unless it is here or under way).
    pub fn press_detail(self: &Arc<Self>, row: i16) -> Chosen {
        if row != ROW_DOWNLOAD {
            return Chosen::Nothing;
        }
        let Some(e) = self.selected() else {
            return Chosen::Nothing;
        };
        {
            let mut g = self.lock();
            match g.installs.get(&e.id) {
                Some(Install::Downloading { .. } | Install::Done) => {
                    return Chosen::Stepped(format!(
                        "mods: {} is already {}",
                        e.name,
                        self.download_text(&g, &e)
                    ));
                }
                _ if g.installed.contains(&e.id) => {
                    return Chosen::Stepped(format!("mods: {} is installed", e.name));
                }
                _ => {}
            }
            g.installs.insert(
                e.id,
                Install::Downloading {
                    done: 0,
                    total: e.size,
                },
            );
        }
        let id = e.id;
        let (a, b) = (Arc::downgrade(self), Arc::downgrade(self));
        let r = self.source.install(
            id,
            Box::new(move |done, total| {
                if let Some(me) = a.upgrade() {
                    me.lock()
                        .installs
                        .insert(id, Install::Downloading { done, total });
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
        Chosen::Stepped(format!("mods: downloading {} ({})", e.name, e.id))
    }

    /// An install's end.
    pub fn install_done(&self, id: MapId, r: Result<(), ModsError>) {
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
        self.lock().selected = some.then_some(id);
        if let Some(entry) = self.selected() {
            self.texts.set(DETAIL_TITLE, &cut(&entry.name, 40));
        }
        self.texts.rewrite_all();
        Ok(())
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

/// The footer legends.
pub fn prompts(screen: &str) -> Vec<Prompt> {
    let a = if screen == DETAIL {
        "Download"
    } else {
        "Select"
    };
    vec![
        Prompt::new(&[Glyph::A], a),
        Prompt::new(&[Glyph::B], "Back"),
    ]
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

/// The list.
pub struct ListRows(pub Arc<Repo>);

impl Rows for ListRows {
    fn title(&self) -> u32 {
        Slots::id(LIST_TITLE)
    }
    fn count(&self) -> usize {
        self.0.list_count()
    }
    fn texts(&self, row: i16) -> Vec<(u32, u32)> {
        let Some((label, value)) = self.0.list_row(row) else {
            return Vec::new();
        };
        let r = row as u32;
        let t = &self.0.texts;
        vec![
            (crate::ui::LABEL_SLOT, t.set(LIST_ROWS + 2 * r, &label)),
            (crate::ui::VALUE_SLOT, t.set(LIST_ROWS + 2 * r + 1, &value)),
        ]
    }
    fn press(&self, row: i16) -> Chosen {
        self.0.press_list(row)
    }
    fn write_texts(&self, m: &mut dyn GuestMemory) -> Result<(), MemFault> {
        self.0.texts.write(m)
    }
    fn describe(&self, count: usize) -> Option<String> {
        Some(describe(&self.0, "Mods", count, |r| self.0.list_row(r)))
    }
}

/// A mod's page.
pub struct DetailRows(pub Arc<Repo>);

impl Rows for DetailRows {
    fn title(&self) -> u32 {
        Slots::id(DETAIL_TITLE)
    }
    fn count(&self) -> usize {
        DETAIL_COUNT
    }
    fn texts(&self, row: i16) -> Vec<(u32, u32)> {
        let Some((label, value)) = self.0.detail_row(row) else {
            return Vec::new();
        };
        let r = row as u32;
        let t = &self.0.texts;
        vec![
            (crate::ui::LABEL_SLOT, t.set(DETAIL_ROWS + 2 * r, &label)),
            (
                crate::ui::VALUE_SLOT,
                t.set(DETAIL_ROWS + 2 * r + 1, &value),
            ),
        ]
    }
    fn press(&self, row: i16) -> Chosen {
        self.0.press_detail(row)
    }
    fn write_texts(&self, m: &mut dyn GuestMemory) -> Result<(), MemFault> {
        self.0.texts.write(m)
    }
    fn describe(&self, count: usize) -> Option<String> {
        let title = self.0.texts.get(DETAIL_TITLE).unwrap_or_default();
        Some(describe(&self.0, &title, count, |r| self.0.detail_row(r)))
    }
}

fn describe(
    _: &Repo,
    title: &str,
    count: usize,
    row: impl Fn(i16) -> Option<(String, String)>,
) -> String {
    let mut out = format!("title {title:?}; rows:");
    for r in 0..count.min(MAX_ROWS) as i16 {
        if let Some((l, v)) = row(r) {
            out.push_str(&format!(" [{r}] {l:?} = {v:?}"));
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

/// The two screens' classes over `repo`.
pub fn classes(repo: &Arc<Repo>) -> [ListClass; 2] {
    [
        ListClass::with_rows("mods-screen", SCREEN, Arc::new(ListRows(Arc::clone(repo))))
            .footer(Slots::id(LIST_FOOTER)),
        ListClass::with_rows(
            "mods-detail",
            DETAIL,
            Arc::new(DetailRows(Arc::clone(repo))),
        )
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
