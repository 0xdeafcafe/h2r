//! The main menu's Theater library, film lobby and host playback controls.

pub mod adapter;
mod overlay;
mod pages;
pub mod preview;
mod selector;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use adapter::{Clip, FilmMeta, Library, OpenLibrary};
pub use overlay::{Control, Key, OverlayView};
use selector::Selector;

pub const FILMS: &str = "theater.films";
pub const DETAIL: &str = "theater.film";
pub const OVERLAY: &str = "theater.playback";

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
                    film.archive_id().is_none() || self.recent_archives.contains(&film.id)
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
    stamps: Vec<String>,
    live: live_hle::LiveConfig,
    signed_in: Option<Arc<live_hle::Hle>>,
}

/// The guest screens and desktop compositor share one controller.
#[derive(Clone, Default)]
pub struct Theater(Arc<Shared>);

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    library: Mutex<Option<Box<dyn Library>>>,
    location: Mutex<Option<Location>>,
    playback: Mutex<overlay::Session>,
    archive_done: Mutex<Vec<(u64, pages::Action, Entry, Result<Option<Clip>, String>)>>,
    playback_dir: Option<PathBuf>,
}

impl Theater {
    /// Construct the playback window's controller independently of the library UI.
    pub fn playback(
        film: FilmMeta,
        range: Option<[u64; 2]>,
        clip_marks: [Option<u64>; 2],
        socket: PathBuf,
        dir: PathBuf,
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
            )),
            playback_dir: Some(dir),
            ..Shared::default()
        })))
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.0.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn install(
        &self,
        registry: &mut hooks::Registry,
        dir: &Path,
        stamps: Vec<String>,
        live: &live_hle::LiveConfig,
        signed_in: Option<Arc<live_hle::Hle>>,
        open: OpenLibrary,
    ) -> anyhow::Result<()> {
        *self.0.location.lock().unwrap_or_else(|e| e.into_inner()) = Some(Location {
            open,
            dir: dir.to_owned(),
            stamps,
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
            let films = library.list(&location.stamps)?;
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
        let Ok(Some(archive)) = live_hle::films::Archive::connect(&location.live, account)
            .inspect_err(|error| errors.push(error.clone()))
        else {
            if let Some(error) = errors.pop() {
                let mut state = self.state();
                if state.open_generation == generation {
                    state.archive_error = Some(error);
                    state.changed();
                }
            }
            return;
        };
        let recent = archive.recent().unwrap_or_else(|error| {
            errors.push(error);
            Vec::new()
        });
        let saved = archive.saved().unwrap_or_else(|error| {
            errors.push(error);
            Vec::new()
        });
        let clips = archive.clips().unwrap_or_else(|error| {
            errors.push(error);
            Vec::new()
        });
        let mut films = std::collections::BTreeMap::new();
        let mut recent_ids = HashSet::new();
        let mut archive_clips = Vec::new();
        for (rows, pinned) in [(recent, false), (saved, true)] {
            for row in rows {
                if let Ok(mut film) = FilmMeta::try_from(row) {
                    if !pinned {
                        recent_ids.insert(film.id.clone());
                    }
                    film.saved = pinned;
                    films.insert(film.id.clone(), film);
                }
            }
        }
        for row in clips {
            let Ok(film) = FilmMeta::try_from(row.film) else {
                continue;
            };
            if row.clip.from_tick >= row.clip.to_tick || !live_proto::game::valid_id(&row.clip.id) {
                continue;
            }
            let film = films.entry(film.id.clone()).or_insert(film);
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
        let mut state = self.state();
        if state.open_generation != generation {
            return;
        }
        if let Load::Ready(list) = &mut state.load {
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
                let result = (|| {
                    let archive = live_hle::films::Archive::connect(&location.live, account)?
                        .ok_or("Live archives are unavailable")?;
                    let game = entry.film.archive_id().ok_or("Missing Live film ID")?;
                    match action {
                        pages::Action::Watch => adapter::download_film(
                            &archive,
                            &location.dir,
                            game,
                            entry.archive_clip.as_ref().and(entry.clip.as_ref()),
                            entry.film.live_start_tick,
                        ),
                        pages::Action::Save => {
                            archive.set_saved(game, !entry.film.saved).map(|()| None)
                        }
                        pages::Action::Delete => {
                            let (id, _) = entry.archive_clip.as_ref().ok_or("Missing clip ID")?;
                            archive.delete_clip(id).map(|()| None)
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
            let state = self.state();
            let still_selected = state.open_generation == generation
                && state
                    .selected
                    .as_ref()
                    .is_some_and(|e| e.film.id == entry.film.id && e.clip == entry.clip);
            drop(state);
            if still_selected {
                let mut playback = entry.clone();
                if playback_clip.is_some() {
                    playback.clip = playback_clip;
                }
                self.session().play(&playback)
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
        let message = match result {
            Err(error) => error,
            Ok(()) => match action {
                pages::Action::Watch => "Opening film...".into(),
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
                    state
                        .archive_clips
                        .retain(|clip| clip.archive_clip != entry.archive_clip);
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
    }

    fn session(&self) -> MutexGuard<'_, overlay::Session> {
        self.0.playback.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn overlay(&self) -> Option<OverlayView> {
        self.session().view()
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

    pub fn playing(&self) -> bool {
        self.session().owns_pads()
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
        let (clip, open_error) = {
            let mut session = self.session();
            session.pump();
            (session.completed_clip.take(), session.open_error.take())
        };
        if let Some(error) = open_error {
            let mut state = self.state();
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
