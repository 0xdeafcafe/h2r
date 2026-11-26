//! The main menu's Theater library, film lobby and host playback controls.

pub mod adapter;
mod link;
mod overlay;
pub use link::external_link;
mod pages;
mod playback_page;
pub mod preview;
mod selector;
pub mod stats;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use adapter::{Clip, FilmMeta, Library, OpenLibrary};
pub use overlay::{Control, Key};
use selector::Selector;

pub const FILMS: &str = "theater.films";
pub const DETAIL: &str = "theater.film";
pub const OVERLAY: &str = "theater.playback";
pub const LOADING: &str = "theater.loading";
pub const PAUSE_MENU: &str = "theater.pause-menu";

/// The main menu's retail list constructor, item text and press, and
/// THEATER, the one row of ours, after System Link and above Settings
/// (docs/game/forge-firefight-ui.md "The main menu"). Forge and Firefight
/// are modes of the lobby Split Screen, System Link and Xbox Live open,
/// not rows here (docs/game/firefight.md "The lobby").
pub(crate) mod menu {
    use std::sync::atomic::{AtomicBool, Ordering};

    pub const LIST_NAME: u32 = 0x0045_8b38;
    pub const VTABLE: u32 = 0x0045_8a70;
    pub const ROW_TEXT: u32 = 0x0023_07c8;
    pub const ROW_CHOSEN: u32 = 0x0023_0827;
    pub const LAST_ROW: u32 = 0x0051_0a14;
    pub const LABEL: &str = "THEATER";
    pub const LABEL_ID: u32 = crate::ui_kit::ids::family("theater menu").id(0);

    /// Retail's rows: 0 Campaign, 1 Xbox Live, 2 Split Screen, 3 System
    /// Link, 4 Settings.
    pub const VANILLA_ROWS: u32 = 5;
    const SYSTEM_LINK_ROW: i16 = 3;
    /// Theater's row: immediately after System Link.
    pub const ROW: i16 = SYSTEM_LINK_ROW + 1;
    /// The most rows the menu has: what the retail pane's item widgets
    /// and the one added widget hold, so it never scrolls.
    #[cfg(test)]
    pub const MAX_ROWS: u32 = 6;

    /// The menu's room (docs/game/forge-firefight-ui.md "The main menu",
    /// **V**): the first row's text is [`TEXT_HEIGHT`] units high from
    /// [`FIRST_TOP`], and the safe area ends at [`SAFE_BOTTOM`].
    pub const FIRST_TOP: i16 = -37;
    pub const TEXT_HEIGHT: i16 = 28;
    pub const SAFE_BOTTOM: i16 = -211;
    /// The least pitch kept (`custom_games::MIN_PITCH`).
    pub const MIN_PITCH: i16 = crate::custom_games::MIN_PITCH;

    /// The pitch that holds `rows` rows between the first row's top and
    /// the safe area's bottom; 29 for six. The list hook moves the rows
    /// to it only when the skin lays them out farther apart
    /// (`custom_games::repitch`).
    pub fn pitch(rows: u32) -> i16 {
        let room = FIRST_TOP - SAFE_BOTTOM - TEXT_HEIGHT;
        if rows <= 1 {
            return room;
        }
        (room / (rows as i16 - 1)).max(MIN_PITCH)
    }

    /// Which rows of ours the menu has: Theater, or none.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct Rows {
        pub theater: bool,
    }

    impl Rows {
        /// How many rows of ours.
        pub fn len(self) -> i16 {
            i16::from(self.theater)
        }

        pub fn any(self) -> bool {
            self.theater
        }

        /// Whether menu row `row` is Theater.
        pub fn is_theater(self, row: i16) -> bool {
            self.theater && row == ROW
        }

        /// The list's row count with ours added, for the retail list's
        /// constructor (`count` rows of the list named `name`).
        pub fn count(self, name: u32, count: u32) -> u32 {
            if name == LIST_NAME && count == VANILLA_ROWS {
                count + self.len() as u32
            } else {
                count
            }
        }

        /// Rows after ours still use their retail text and press handlers:
        /// the retail row a displayed row shows (Settings, under Theater).
        pub fn shifted_retail_row(self, row: i16) -> Option<i16> {
            let n = self.len();
            (n > 0 && (ROW + n..=VANILLA_ROWS as i16 + n - 1).contains(&row)).then(|| row - n)
        }
    }

    static THEATER: AtomicBool = AtomicBool::new(false);

    /// Records the rows the hooks installed.
    pub fn set_rows(rows: Rows) {
        THEATER.store(rows.theater, Ordering::Relaxed);
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::ui_kit::view::Opener;

        const THEATER_ON: Rows = Rows { theater: true };

        #[test]
        fn the_main_menu_has_six_rows_with_theater() {
            assert_eq!(THEATER_ON.count(LIST_NAME, VANILLA_ROWS), MAX_ROWS);
            assert_eq!(
                THEATER_ON.count(LIST_NAME, VANILLA_ROWS + 1),
                VANILLA_ROWS + 1,
                "a list already grown is left"
            );
            assert_eq!(
                THEATER_ON.count(crate::custom_games::LIST_NAME, VANILLA_ROWS),
                VANILLA_ROWS,
                "another list"
            );
            let rows = Opener::MAIN_MENU.rows;
            assert_eq!(rows.len() as u32, MAX_ROWS);
            assert_eq!(
                rows,
                [
                    "Campaign",
                    "Xbox Live",
                    "Split Screen",
                    "System Link",
                    LABEL,
                    "Settings"
                ]
            );
            assert_eq!(rows[ROW as usize], LABEL);
            assert!(THEATER_ON.is_theater(ROW));
            assert!(!THEATER_ON.is_theater(ROW - 1) && !THEATER_ON.is_theater(ROW + 1));
            // Campaign, Xbox Live, Split Screen and System Link keep their
            // rows; Settings, under Theater, is retail's row 4.
            let retail: Vec<_> = (0..6).map(|r| THEATER_ON.shifted_retail_row(r)).collect();
            assert_eq!(retail, [None, None, None, None, None, Some(4)]);
        }

        #[test]
        fn five_rows_without_theater() {
            let off = Rows::default();
            assert!(!off.any());
            assert_eq!(off.count(LIST_NAME, VANILLA_ROWS), VANILLA_ROWS);
            assert!(!off.is_theater(ROW));
            assert!((0..6).all(|r| off.shifted_retail_row(r).is_none()));
        }

        #[test]
        fn pitch_keeps_six_rows_above_the_footer() {
            for rows in [2, 5, MAX_ROWS] {
                let p = pitch(rows);
                let last_bottom = FIRST_TOP - (rows as i16 - 1) * p - TEXT_HEIGHT;
                assert!(last_bottom >= SAFE_BOTTOM, "{rows} rows end at {last_bottom}");
                assert!(p >= MIN_PITCH, "{rows} rows {p} apart");
            }
            assert_eq!(pitch(MAX_ROWS), 29, "one unit of gap between six rows");
            assert!(
                p_overlaps(7),
                "seven rows would overlap their text: the menu keeps six"
            );
        }

        fn p_overlaps(rows: u32) -> bool {
            pitch(rows) < TEXT_HEIGHT
        }

        #[test]
        fn the_label_is_a_menu_text() {
            assert!(crate::layout_glue::is_ours(LABEL_ID));
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Tab {
    #[default]
    Recent,
    Saved,
    Clips,
}

impl Tab {
    const ALL: [Self; 3] = [Self::Recent, Self::Saved, Self::Clips];

    fn label(self) -> &'static str {
        match self {
            Self::Recent => "Recent",
            Self::Saved => "Saved",
            Self::Clips => "Clips",
        }
    }

    fn next(self, right: bool) -> Self {
        Self::ALL[(self as usize + if right { 1 } else { 2 }) % Self::ALL.len()]
    }
}

#[derive(Clone, Debug)]
struct Entry {
    film: FilmMeta,
    clip: Option<Clip>,
    archive_clip: Option<(String, String)>,
}

#[derive(Clone, Debug, Default)]
enum Load {
    #[default]
    Loading,
    Ready(Vec<FilmMeta>),
}

#[derive(Default)]
struct State {
    load: Load,
    tab: Tab,
    selected: Option<Entry>,
    view: Option<agent_proto::theater::InitialView>,
    roster_page: usize,
    confirm_delete: bool,
    selector: Selector,
    recent_archives: HashSet<String>,
    archive_clips: Vec<Entry>,
    working: bool,
    status: String,
    clips: Vec<Clip>,
    clips_error: Option<String>,
    local_error: Option<String>,
    archive_error: Option<String>,
    revision: u64,
    open_generation: u64,
    player: Option<String>,
    controller: u32,
    account: Option<live_proto::Xuid>,
    /// Why a film's load failed, for the film page the viewer returns to.
    return_notice: Option<String>,
}

impl State {
    fn changed(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    fn films(&self) -> &[FilmMeta] {
        match &self.load {
            Load::Ready(list) => list,
            _ => &[],
        }
    }

    fn library_error(&self) -> Option<&str> {
        self.local_error
            .as_deref()
            .or(self.archive_error.as_deref())
    }

    fn tab_entries(&self) -> Vec<Entry> {
        if self.tab == Tab::Clips {
            let mut entries: Vec<_> = self
                .clips
                .iter()
                .rev()
                .filter_map(|clip| {
                    let film = self.films().iter().find(|film| film.id == clip.film)?;
                    if film.length_ticks != 0 && clip.to_tick > film.length_ticks {
                        return None;
                    }
                    Some(Entry {
                        film: film.clone(),
                        clip: Some(clip.clone()),
                        archive_clip: None,
                    })
                })
                .collect();
            entries.extend(self.archive_clips.iter().cloned());
            return entries;
        }
        self.films()
            .iter()
            .filter(|film| match self.tab {
                Tab::Recent => {
                    !film.saved && (film.archive_id().is_none() || self.recent_archives.contains(&film.id))
                }
                Tab::Saved => film.saved,
                Tab::Clips => false,
            })
            .cloned()
            .map(|film| Entry {
                film,
                clip: None,
                archive_clip: None,
            })
            .collect()
    }

    fn entries(&self) -> Vec<Entry> {
        let mut entries = self.tab_entries();
        self.selector.apply(&mut entries);
        entries.truncate(if self.tab == Tab::Recent {
            25
        } else {
            pages::FILM_ROWS
        });
        entries
    }

    fn select(&mut self, entry: Entry) {
        self.selected = Some(entry);
        self.view = None;
        self.roster_page = 0;
        self.confirm_delete = false;
        self.status.clear();
        self.changed();
    }

    fn change_tab(&mut self, right: bool) {
        self.tab = self.tab.next(right);
        self.selector.clear_filters();
        self.changed();
    }
}

#[derive(Clone)]
struct Location {
    open: OpenLibrary,
    dir: PathBuf,
    installation: crate::theater::Installation,
    live: live_hle::LiveConfig,
    signed_in: Option<Arc<live_hle::Hle>>,
}

/// The guest screens and desktop compositor share one controller.
#[derive(Clone, Default)]
pub struct Theater(Arc<Shared>);

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    link: Mutex<Option<link::Ready>>,
    sync_home: std::sync::OnceLock<(PathBuf, String, Option<live_proto::Xuid>)>,
    sync: Mutex<std::collections::BTreeMap<live_proto::Xuid, live_hle::films::sync::Sync>>,
    library: Mutex<Option<Box<dyn Library>>>,
    location: Mutex<Option<Location>>,
    playback: Mutex<overlay::Session>,
    archive_done: Mutex<Vec<(u64, pages::Action, Entry, Result<Option<Clip>, String>)>>,
    playback_dir: Option<PathBuf>,
    playback_recording: Option<PathBuf>,
}

impl Theater {
    /// Construct the playback window's controller independently of the library UI.
    pub fn playback(
        film: FilmMeta,
        range: Option<[u64; 2]>,
        clip_marks: [Option<u64>; 2],
        starting_seek: Option<u64>,
        socket: PathBuf,
        dir: PathBuf,
        recording: PathBuf,
    ) -> std::io::Result<Self> {
        let clip = range.map(|[from_tick, to_tick]| Clip {
            film: film.id.clone(),
            from_tick,
            to_tick,
        });
        let rpc = adapter::Playback::socket(socket)?;
        Ok(Self(Arc::new(Shared {
            playback: Mutex::new(overlay::Session::playback(
                &Entry {
                    film,
                    clip,
                    archive_clip: None,
                },
                rpc,
                clip_marks,
                starting_seek,
            )),
            playback_dir: Some(dir),
            playback_recording: Some(recording),
            ..Shared::default()
        })))
    }

    pub fn configure_sync(
        &self,
        data: &Path,
        server_id: &str,
        default_account: Option<live_proto::Xuid>,
    ) {
        let _ = self
            .0
            .sync_home
            .set((data.into(), server_id.into(), default_account));
    }

    pub fn sync(&self) -> Option<live_hle::films::sync::Sync> {
        let (data, server, default_account) = self.0.sync_home.get()?;
        let controller = self.state().controller;
        let location = self.0.location.lock().unwrap_or_else(|e| e.into_inner());
        let location = location.as_ref()?;
        let by = location
            .signed_in
            .as_ref()
            .and_then(|h| h.signed_in_account(controller))
            .map(|(id, _)| id)
            .or(*default_account)?;
        let mut accounts = self.0.sync.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(sync) = accounts.get(&by) {
            return Some(sync.clone());
        }
        let sync = live_hle::films::sync::Sync::start(data, server, by, location.live.clone())
            .inspect_err(|e| eprintln!("film library: {e}"))
            .ok()?;
        accounts.insert(by, sync.clone());
        Some(sync)
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.0.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn install(
        &self,
        registry: &mut hooks::Registry,
        dir: &Path,
        installation: crate::theater::Installation,
        live: &live_hle::LiveConfig,
        signed_in: Option<Arc<live_hle::Hle>>,
        open: OpenLibrary,
    ) -> anyhow::Result<()> {
        *self.0.location.lock().unwrap_or_else(|e| e.into_inner()) = Some(Location {
            open,
            dir: dir.to_owned(),
            installation,
            live: live.clone(),
            signed_in,
        });
        pages::install(registry, self.clone())
    }

    /// Opening Theater reads the local library and Live archives in the background.
    pub fn open(&self, controller: u32, player: Option<String>) {
        let account = self
            .0
            .location
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .and_then(|location| location.signed_in.as_ref()?.signed_in_account(controller));
        let generation = {
            let mut state = self.state();
            state.controller = controller;
            state.account = account.as_ref().map(|(xuid, _)| *xuid);
            state.player = account.map(|(_, name)| name).or(player);
            state.open_generation = state.open_generation.wrapping_add(1);
            state.tab = Tab::Recent;
            state.selected = None;
            state.working = false;
            state.status.clear();
            state.selector = Selector::default();
            state.recent_archives.clear();
            state.archive_clips.clear();
            state.clips.clear();
            state.clips_error = None;
            state.load = Load::Loading;
            state.local_error = None;
            state.archive_error = None;
            state.changed();
            state.open_generation
        };
        let this = self.clone();
        if let Err(e) = std::thread::Builder::new()
            .name("h2-theater-library".into())
            .spawn(move || this.read_library(generation))
        {
            let mut state = self.state();
            if state.open_generation == generation {
                state.load = Load::Ready(Vec::new());
                state.local_error = Some(e.to_string());
                state.changed();
            }
        }
    }

    fn read_library(&self, generation: u64) {
        let location = self
            .0
            .location
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let Some(location) = location else { return };
        let local = (location.open)(&location.dir).and_then(|library| {
            let films = library.list(&location.installation)?;
            Ok((library, films))
        });
        let (opened, films, local_error) = match local {
            Ok((library, films)) => (Some(library), films, None),
            Err(error) => (None, Vec::new(), Some(error)),
        };
        let clips = adapter::load_clips(&adapter::clips_path(&location.dir));
        let mut library = self.0.library.lock().unwrap_or_else(|e| e.into_inner());
        let mut state = self.state();
        if state.open_generation != generation {
            return;
        }
        *library = opened;
        state.load = Load::Ready(films);
        state.local_error = local_error;
        match clips {
            Ok(clips) => state.clips = clips,
            Err(error) => state.clips_error = Some(error),
        }
        let account = state.account;
        state.changed();
        drop(state);
        drop(library);
        self.read_archives(generation, &location, account);
    }

    fn read_archives(
        &self,
        generation: u64,
        location: &Location,
        account: Option<live_proto::Xuid>,
    ) {
        let mut errors = Vec::new();
        let sync = self.sync().filter(|s| Some(s.account()) == account);
        if let Some(e) = sync.as_ref().and_then(|s| s.error()) {
            errors.push(e);
        }
        let archive = match live_hle::films::Archive::connect(&location.live, account) {
            Ok(value) => value,
            Err(e) => {
                errors.push(e);
                None
            }
        };
        let current = archive
            .as_ref()
            .and_then(|a| a.library().inspect_err(|e| errors.push(e.clone())).ok());
        let recent = if let Some(rows) = current {
            if let Some(s) = &sync
                && let Err(e) = s.remember_library(&rows)
            {
                errors.push(e);
            }
            rows
        } else {
            sync.as_ref()
                .and_then(|s| {
                    s.read_library()
                        .inspect_err(|e| errors.push(e.clone()))
                        .ok()
                })
                .unwrap_or_default()
        };
        let saved = archive
            .as_ref()
            .and_then(|a| a.saved().inspect_err(|e| errors.push(e.clone())).ok())
            .unwrap_or_default();
        let clips = archive
            .as_ref()
            .and_then(|a| a.clips().inspect_err(|e| errors.push(e.clone())).ok())
            .unwrap_or_default();
        let mut films = std::collections::BTreeMap::new();
        let mut recent_ids = HashSet::new();
        let mut archive_clips = Vec::new();
        for (rows, pinned) in [(recent, false), (saved, true)] {
            for row in rows {
                if let Ok(mut film) = FilmMeta::try_from(row) {
                    film.check_cached(&location.dir, &location.installation);
                    if !pinned {
                        recent_ids.insert(film.id.clone());
                    }
                    film.saved |= pinned;
                    films.insert(film.id.clone(), film);
                }
            }
        }
        for row in clips {
            let Ok(mut film) = FilmMeta::try_from(row.film) else {
                continue;
            };
            if row.clip.from_tick >= row.clip.to_tick || !live_proto::game::valid_id(&row.clip.id) {
                continue;
            }
            let film = films.entry(film.id.clone()).or_insert_with(|| {
                film.check_cached(&location.dir, &location.installation);
                film
            });
            archive_clips.push(Entry {
                film: film.clone(),
                clip: Some(Clip {
                    film: film.id.clone(),
                    from_tick: row.clip.from_tick,
                    to_tick: row.clip.to_tick,
                }),
                archive_clip: Some((row.clip.id, row.clip.title)),
            });
        }
        let local_ids: Vec<_> = self.state().films().iter().map(|f| f.id.clone()).collect();
        let mut published = std::collections::HashMap::new();
        if let Some(sync) = sync {
            for id in local_ids {
                if let Some(remote) = sync.published(&id) {
                    published.insert(id, remote);
                }
            }
        }
        let mut state = self.state();
        if state.open_generation != generation {
            return;
        }
        if let Load::Ready(list) = &mut state.load {
            for local in list.iter_mut() {
                let remote = local
                    .live_game_id
                    .clone()
                    .or_else(|| published.remove(&local.id));
                let Some(remote) = remote else { continue };
                if let Some(row) = films.remove(&format!("live:{remote}")) {
                    local.live_game_id = Some(remote);
                    local.shared = row.shared;
                    local.saved |= row.saved;
                }
            }
            list.extend(films.into_values());
        }
        state.archive_clips = archive_clips;
        state.recent_archives = recent_ids;
        state.archive_error = errors.into_iter().next();
        state.changed();
    }

    fn change_library(&self, action: pages::Action, entry: &Entry) -> Result<(), String> {
        let id = &entry.film.id;
        if entry.clip.is_none() {
            if let Some(sync) = self
                .sync()
                .filter(|s| Some(s.account()) == self.state().account)
            {
                match action {
                    pages::Action::Save => sync.change_local(id, Some(!entry.film.saved))?,
                    pages::Action::Delete => sync.change_local(id, None)?,
                    _ => {}
                }
            }
            let library = self.0.library.lock().unwrap_or_else(|e| e.into_inner());
            let library = library.as_ref().ok_or("Film library is unavailable")?;
            match action {
                pages::Action::Save if entry.film.saved => library.unsave(id)?,
                pages::Action::Save => library.save(id)?,
                pages::Action::Delete if entry.clip.is_none() => library.delete(id)?,
                pages::Action::Delete => {}
                _ => return Ok(()),
            }
        }
        let mut state = self.state();
        if action == pages::Action::Save {
            let saved = !entry.film.saved;
            if let Load::Ready(films) = &mut state.load
                && let Some(film) = films.iter_mut().find(|film| film.id == *id)
            {
                film.saved = saved;
            }
            if let Some(selected) = state.selected.as_mut() {
                selected.film.saved = saved;
            }
        } else if entry.clip.is_none() {
            if let Load::Ready(films) = &mut state.load {
                films.retain(|film| film.id != *id);
            }
            state.selected = None;
        }
        state.changed();
        drop(state);
        if action == pages::Action::Delete
            && let Some(clip) = &entry.clip
        {
            self.edit_clips(|clips| clips.retain(|c| c != clip))?;
            self.state().selected = None;
        }
        Ok(())
    }

    fn edit_clips(&self, edit: impl FnOnce(&mut Vec<Clip>)) -> Result<(), String> {
        let location = self.0.location.lock().unwrap_or_else(|e| e.into_inner());
        let dir = self
            .0
            .playback_dir
            .as_ref()
            .or_else(|| location.as_ref().map(|l| &l.dir))
            .ok_or("Clip storage is unavailable")?;
        let path = adapter::clips_path(dir);
        let mut state = self.state();
        state.clips = adapter::update_clips(&path, edit)?;
        state.clips_error = None;
        state.changed();
        Ok(())
    }

    fn refresh_clips(&self) {
        let location = self.0.location.lock().unwrap_or_else(|e| e.into_inner());
        let Some(location) = location.as_ref() else {
            return;
        };
        let clips = adapter::load_clips(&adapter::clips_path(&location.dir));
        let mut state = self.state();
        match clips {
            Ok(clips) => {
                state.clips = clips;
                state.clips_error = None;
            }
            Err(error) => state.clips_error = Some(error),
        }
        state.changed();
    }

    fn archive_action(&self, action: pages::Action, entry: Entry) {
        let Some(location) = self
            .0
            .location
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        else {
            return;
        };
        let (generation, account) = {
            let mut state = self.state();
            if state.working {
                return;
            }
            state.working = true;
            state.status = match action {
                pages::Action::Watch => "Downloading host film...",
                _ => "Updating account films...",
            }
            .into();
            state.changed();
            (state.open_generation, state.account)
        };
        let this = self.clone();
        let work = std::thread::Builder::new()
            .name("h2-theater-archive".into())
            .spawn(move || {
                let mut entry = entry;
                let result = (|| {
                    let sync_account = this.sync().filter(|s| Some(s.account()) == account);
                    if matches!(action, pages::Action::Delete | pages::Action::Save)
                        && entry.archive_clip.is_none()
                        && entry.clip.is_none()
                        && let (Some(sync), Some(game)) =
                            (&sync_account, entry.film.live_game_id.as_deref())
                    {
                        let pin = (action == pages::Action::Save).then_some(!entry.film.saved);
                        sync.change_remote(game, pin)?;
                        return Ok(None);
                    }
                    if action == pages::Action::Watch
                        && let (Some(sync), Some(game)) = (&sync_account, entry.film.archive_id())
                        && let Some(result) = adapter::cached_film(
                            sync,
                            &location.dir,
                            game,
                            entry.archive_clip.as_ref().and(entry.clip.as_ref()),
                            entry.film.live_start_tick,
                        )
                    {
                        if result.is_ok() {
                            entry.film.check_cached(&location.dir, &location.installation);
                        }
                        return result;
                    }
                    let archive = live_hle::films::Archive::connect(&location.live, account)?
                        .ok_or("Live archives are unavailable")?;
                    let mut id = entry
                        .film
                        .archive_id()
                        .or(entry.film.live_game_id.as_deref())
                        .map(str::to_owned);
                    if action == pages::Action::Share {
                        if let Some(game) = &id {
                            if entry.film.shared
                                && archive.detail(game)?.visibility
                                    == Some(live_proto::film::sharing::Visibility::Private)
                            {
                                archive.set_visibility(
                                    game,
                                    live_proto::film::sharing::Visibility::Link,
                                )?;
                            }
                        } else {
                            let sync = this
                                .sync()
                                .filter(|s| Some(s.account()) == account)
                                .ok_or("Sign in to share this recording")?;
                            let local = film::Library::open(&location.dir)
                                .and_then(|library| library.get(&entry.film.id))
                                .map_err(|error| error.to_string())?;
                            sync.enqueue(&entry.film.id, &local.path)?;
                            let shared = sync.share(
                                &entry.film.id,
                                live_proto::film::sharing::Visibility::Link,
                            )?;
                            id = Some(shared);
                        }
                    } else if id.is_none()
                        && let Some(sync) = this.sync().filter(|s| Some(s.account()) == account)
                    {
                        id = sync.published(&entry.film.id);
                    }
                    let game = id.as_deref().ok_or("Share this film first")?;
                    match action {
                        pages::Action::Watch => adapter::download_film(
                            &archive,
                            sync_account.as_ref(),
                            &location.dir,
                            game,
                            entry.archive_clip.as_ref().and(entry.clip.as_ref()),
                            entry.film.live_start_tick,
                        )
                        .inspect(|_| {
                            entry.film.check_cached(&location.dir, &location.installation);
                        }),
                        pages::Action::Share | pages::Action::CopyLink => {
                            let clip = if let Some((id, _)) = &entry.archive_clip {
                                Some(id.clone())
                            } else if let Some(clip) = &entry.clip {
                                Some(archive.share_clip(game, clip.from_tick, clip.to_tick)?)
                            } else {
                                None
                            };
                            let link = link::share_url(&archive, &location, game, clip.as_deref())?;
                            if action == pages::Action::CopyLink {
                                link::copy(&link)?;
                            }
                            let mut state = this.state();
                            if state.open_generation == generation
                                && state
                                    .selected
                                    .as_ref()
                                    .is_some_and(|e| e.film.id == entry.film.id)
                            {
                                state.status = if action == pages::Action::CopyLink {
                                    "Link copied".into()
                                } else {
                                    link
                                };
                                state.changed();
                            }
                            Ok(None)
                        }
                        pages::Action::Save => {
                            archive.set_saved(game, !entry.film.saved).map(|()| None)
                        }
                        pages::Action::Delete => {
                            if let Some((id, _)) = &entry.archive_clip {
                                archive.delete_clip(id)?;
                            } else {
                                archive.remove(game)?;
                            }
                            Ok(None)
                        }
                    }
                })();
                // Keep guest navigation and mutations on the menu's own pump.
                let mut done = this
                    .0
                    .archive_done
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                done.push((generation, action, entry, result));
            });
        if let Err(error) = work {
            let mut state = self.state();
            state.working = false;
            state.status = error.to_string();
            state.changed();
        }
    }

    fn finish_archive_action(
        &self,
        generation: u64,
        action: pages::Action,
        entry: Entry,
        result: Result<Option<Clip>, String>,
    ) {
        let result = result.and_then(|playback_clip| {
            if action != pages::Action::Watch {
                return Ok(());
            }
            if let Some(reason) = entry.film.watch_unavailable() {
                return Err(reason.to_owned());
            }
            let state = self.state();
            let still_selected = state.open_generation == generation
                && state
                    .selected
                    .as_ref()
                    .is_some_and(|e| e.film.id == entry.film.id && e.clip == entry.clip);
            let view = state.view;
            drop(state);
            if still_selected {
                let mut playback = entry.clone();
                if playback_clip.is_some() {
                    playback.clip = playback_clip;
                }
                self.session().play(&playback, view)
            } else {
                Ok(())
            }
        });
        let mut state = self.state();
        if state.open_generation != generation {
            return;
        }
        state.working = false;
        let selected = state
            .selected
            .as_ref()
            .is_some_and(|e| e.film.id == entry.film.id && e.clip == entry.clip);
        if action == pages::Action::Watch && entry.film.version.is_some() {
            let update = |film: &mut FilmMeta| {
                film.version = entry.film.version;
                film.compatibility = entry.film.compatibility;
                film.compatibility_reason
                    .clone_from(&entry.film.compatibility_reason);
            };
            if let Load::Ready(films) = &mut state.load
                && let Some(film) = films.iter_mut().find(|film| film.id == entry.film.id)
            {
                update(film);
            }
            for clip in state
                .archive_clips
                .iter_mut()
                .filter(|clip| clip.film.id == entry.film.id)
            {
                update(&mut clip.film);
            }
            if let Some(current) = state.selected.as_mut().filter(|_| selected) {
                update(&mut current.film);
            }
        }
        let message = match result {
            Err(error) => error,
            Ok(()) => match action {
                pages::Action::Watch => "Opening film...".into(),
                pages::Action::CopyLink | pages::Action::Share => state.status.clone(),
                pages::Action::Save => {
                    let saved = !entry.film.saved;
                    if let Load::Ready(films) = &mut state.load
                        && let Some(film) = films.iter_mut().find(|f| f.id == entry.film.id)
                    {
                        film.saved = saved;
                    }
                    for clip in state
                        .archive_clips
                        .iter_mut()
                        .filter(|c| c.film.id == entry.film.id)
                    {
                        clip.film.saved = saved;
                    }
                    if let Some(film) = state.selected.as_mut().filter(|_| selected) {
                        film.film.saved = saved;
                    }
                    if saved {
                        "Added to Saved".into()
                    } else {
                        "Removed from Saved".into()
                    }
                }
                pages::Action::Delete => {
                    if entry.archive_clip.is_none() {
                        if let Load::Ready(rows) = &mut state.load {
                            rows.retain(|f| f.id != entry.film.id);
                        }
                        state.recent_archives.remove(&entry.film.id);
                        state.archive_clips.retain(|c| c.film.id != entry.film.id);
                        if selected {
                            state.selected = None;
                        }
                    } else {
                        state
                            .archive_clips
                            .retain(|clip| clip.archive_clip != entry.archive_clip);
                    }
                    if selected {
                        state.confirm_delete = false;
                    }
                    "Deleted".into()
                }
            },
        };
        if selected {
            state.status = message;
        }
        state.changed();
    }

    pub fn connect(&self, agent: &agent::Agent) {
        self.session().connect(agent);
        self.link_method(agent);
    }

    fn session(&self) -> MutexGuard<'_, overlay::Session> {
        self.0.playback.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn playback_page(&self) -> Option<crate::ui_kit::spec::ScreenSpec> {
        self.session().view()
    }

    pub fn playback_timeline(&self) -> Option<[u32; 4]> {
        self.session().timeline()
    }

    pub fn key(&self, key: Key, down: bool, repeat: bool) -> bool {
        self.session().key(key, down, repeat)
    }

    pub fn mouse_look(&self, dx: f64, dy: f64) -> bool {
        self.session().mouse_look(dx, dy)
    }

    pub fn mouse_zoom(&self, amount: f32) -> bool {
        self.session().mouse_zoom(amount)
    }

    pub fn scrub(&self, fraction: f32) -> bool {
        self.session().scrub(fraction)
    }

    fn save_playback(&self) {
        let Some(recording) = &self.0.playback_recording else { return };
        let Some(dir) = &self.0.playback_dir else { return };
        let notice = match adapter::save_playback(recording, dir) {
            Ok(()) => "Film added to Saved".into(),
            Err(error) => format!("Couldn't save film: {error}"),
        };
        self.session().notice = Some(notice);
    }

    pub fn add_playback_methods(&self, agent: &agent::Agent) {
        use agent_proto::{RpcError, code, method};
        use serde::Deserialize;
        use serde_json::json;
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Press {
            input: String,
        }

        fn screen(page: Option<crate::ui_kit::spec::ScreenSpec>) -> serde_json::Value {
            let top = page.as_ref().map(|s| s.underlay.as_deref().unwrap_or(s));
            let dialog = page.as_ref().filter(|spec| spec.dialog.is_some())
                .map(|spec| json!({"name": spec.name, "focus": spec.focus}));
            json!({
                "screen": top.map(|s| &s.name),
                "focus": top.map(|s| s.focus),
                "dialog": dialog,
                "native": false,
                "age_ms": 0,
                "page": page,
            })
        }

        let this = self.clone();
        agent.add_method(method::UI_SCREEN, agent::Access::Read, move |p| {
            if !(p.is_null() || p.as_object().is_some_and(|p| p.is_empty())) {
                return Err(RpcError::new(code::INVALID_PARAMS, "expected no parameters"));
            }
            Ok(screen(this.playback_page()))
        });
        let this = self.clone();
        agent.add_method(method::UI_INPUT, agent::Access::Act, move |p| {
            let press: Press = serde_json::from_value(p)
                .map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))?;
            let input = crate::ui_kit::input::Input::named(&press.input)
                .ok_or_else(|| RpcError::new(code::INVALID_PARAMS, "unknown playback input"))?;
            let mut session = this.session();
            session.press(input);
            Ok(screen(session.view()))
        });
    }

    pub fn playing(&self) -> bool {
        self.session().owns_pads()
    }

    /// A film is loaded and playing: the host draws the playback bar for it
    /// and for nothing else (not over the menus, not while a film is queued).
    pub fn film_loaded(&self) -> bool {
        self.session().loaded()
    }

    /// A film queued in this game has not played its first tick: the host
    /// draws the loading panel (`playback_page`) and nothing else.
    pub fn film_loading(&self) -> bool {
        self.session().loading()
    }

    /// The film's load failed (the game could not start it): the loading
    /// state ends and the viewer returns to the film page with `why`.
    pub fn load_failed(&self, why: &str) {
        let mut session = self.session();
        if session.loading() {
            session.fail_load(why.to_owned());
        }
    }

    pub fn release_keys(&self) {
        self.session().release_keys();
    }

    pub fn pad_gate(&self, next: Option<Box<dyn input::PadGate>>) -> Box<dyn input::PadGate> {
        Box::new(overlay::Gate::new(self.clone(), next))
    }

    pub fn leave_requested(&self) -> bool {
        std::mem::take(&mut self.session().leave_requested)
    }

    pub fn screenshot_requested(&self) -> bool {
        std::mem::take(&mut self.session().screenshot)
    }

    pub fn screenshot_answer(&self, answer: Result<PathBuf, String>) {
        let mut session = self.session();
        session.notice = Some(match answer {
            Ok(path) => format!(
                "Saved {}",
                path.file_name().unwrap_or_default().to_string_lossy()
            ),
            Err(error) => error,
        });
    }

    /// Called outside the compositor and guest memory lock. A completed
    /// in/out pair becomes a range entry in the local Clips tab.
    pub fn pump(&self) {
        self.pump_external_link();
        let done = std::mem::take(
            &mut *self
                .0
                .archive_done
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
        );
        for (generation, action, entry, result) in done {
            self.finish_archive_action(generation, action, entry, result);
        }
        let (clip, open_error, failed, save) = {
            let mut session = self.session();
            session.pump();
            (
                session.completed_clip.take(),
                session.open_error.take(),
                session.failed_film.take(),
                std::mem::take(&mut session.save_requested),
            )
        };
        if save {
            self.save_playback();
        }
        if let Some(error) = open_error {
            let mut state = self.state();
            if failed.is_some() {
                state.return_notice = Some(error.clone());
            }
            state.status = error;
            state.changed();
        }
        if let Some(clip) = clip {
            if let Err(error) = self.edit_clips(|clips| {
                if !clips.contains(&clip) {
                    clips.push(clip);
                }
            }) {
                self.session().notice = Some(error);
            }
        }
    }
}
