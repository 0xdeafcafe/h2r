//! The main menu's Theater library, film lobby and host playback controls.

pub mod adapter;
mod overlay;
mod pages;
pub mod preview;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use adapter::{Clip, FilmMeta, Library, OpenLibrary};
pub use overlay::{Control, Key, OverlayView};

pub const FILMS: &str = "theater.films";
pub const DETAIL: &str = "theater.film";
pub const OVERLAY: &str = "theater.playback";

/// The title menu's retail list constructor, item text and press (screen 6).
pub(crate) mod menu {
    pub const LIST_NAME: u32 = 0x0045_8b38;
    pub const VTABLE: u32 = 0x0045_8a70;
    pub const ROW: i16 = 5;
    pub const ROW_TEXT: u32 = 0x0023_07c8;
    pub const ROW_CHOSEN: u32 = 0x0023_0827;
    pub const LABEL: &str = "Theater";
    pub const LABEL_ID: u32 = crate::ui_kit::ids::family("theater menu").id(0);

    pub fn count(name: u32, count: u32) -> u32 {
        if name == LIST_NAME && count == ROW as u32 {
            count + 1
        } else {
            count
        }
    }

    // The retail tag's sole pane already shows six items. Only the
    // constructor's five datums and five embedded widgets need growing.
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
}

#[derive(Clone, Debug, Default)]
enum Load {
    #[default]
    Loading,
    Ready(Vec<FilmMeta>),
    Error(String),
}

#[derive(Default)]
struct State {
    load: Load,
    tab: Tab,
    selected: Option<Entry>,
    roster_page: usize,
    confirm_delete: bool,
    web: bool,
    status: String,
    clips: Vec<Clip>,
    clips_error: Option<String>,
    revision: u64,
    open_generation: u64,
    player: Option<String>,
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

    fn entries(&self) -> Vec<Entry> {
        if self.tab == Tab::Clips {
            return self
                .clips
                .iter()
                .rev()
                .filter_map(|clip| {
                    let film = self.films().iter().find(|film| film.id == clip.film)?;
                    if clip.to_tick > film.length_ticks {
                        return None;
                    }
                    Some(Entry {
                        film: film.clone(),
                        clip: Some(clip.clone()),
                    })
                })
                .collect();
        }
        self.films()
            .iter()
            .filter(|film| self.tab == Tab::Recent || film.saved)
            .take(if self.tab == Tab::Recent {
                25
            } else {
                pages::MAX_ROWS - 1
            })
            .cloned()
            .map(|film| Entry { film, clip: None })
            .collect()
    }

    fn select(&mut self, entry: Entry) {
        self.selected = Some(entry);
        self.roster_page = 0;
        self.confirm_delete = false;
        self.status.clear();
        self.web = false;
        self.changed();
    }

    fn change_tab(&mut self, right: bool) {
        self.tab = self.tab.next(right);
        self.changed();
    }
}

#[derive(Clone)]
struct Location {
    open: OpenLibrary,
    dir: PathBuf,
    stamp: String,
    web: String,
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
    playback_dir: Option<PathBuf>,
}

impl Theater {
    /// Construct the playback window's controller independently of the library UI.
    pub fn playback(
        film: FilmMeta,
        range: Option<[u64; 2]>,
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
            playback: Mutex::new(overlay::Session::playback(&Entry { film, clip }, rpc)),
            playback_dir: Some(dir),
            ..Shared::default()
        })))
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.0.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) fn install(
        &self,
        registry: &mut feature::Registry,
        dir: &Path,
        stamp: String,
        web: Option<&str>,
        open: OpenLibrary,
    ) -> anyhow::Result<()> {
        *self.0.location.lock().unwrap_or_else(|e| e.into_inner()) = Some(Location {
            open,
            dir: dir.to_owned(),
            stamp,
            web: web
                .unwrap_or(live_proto::DEFAULT_SERVER)
                .trim_end_matches('/')
                .to_owned(),
        });
        pages::install(registry, self.clone())
    }

    /// Opening from the main menu starts the disk read in the background.
    pub(crate) fn open(&self, player: Option<String>) {
        let generation = {
            let mut state = self.state();
            state.player = player;
            state.open_generation = state.open_generation.wrapping_add(1);
            state.tab = Tab::Recent;
            state.selected = None;
            state.web = false;
            state.load = Load::Loading;
            state.changed();
            state.open_generation
        };
        let this = self.clone();
        if let Err(e) = std::thread::Builder::new()
            .name("h2-theater-library".into())
            .spawn(move || this.read_library(generation))
        {
            let mut state = self.state();
            state.load = Load::Error(e.to_string());
            state.changed();
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
        let films = (location.open)(&location.dir).and_then(|library| {
            let mut films = library.list(&location.stamp)?;
            films.sort_by(|a, b| {
                b.started_ms
                    .cmp(&a.started_ms)
                    .then_with(|| a.id.cmp(&b.id))
            });
            Ok((library, films))
        });
        let clips = adapter::load_clips(&adapter::clips_path(&location.dir));
        let mut library = self.0.library.lock().unwrap_or_else(|e| e.into_inner());
        let mut state = self.state();
        if state.open_generation != generation {
            return;
        }
        state.load = match films {
            Ok((opened, films)) => {
                *library = Some(opened);
                Load::Ready(films)
            }
            Err(error) => {
                *library = None;
                Load::Error(error)
            }
        };
        match clips {
            Ok(clips) => {
                state.clips = clips;
                state.clips_error = None;
            }
            Err(error) => state.clips_error = Some(error),
        }
        state.changed();
    }

    fn change_library(&self, action: pages::Action, entry: &Entry) -> Result<(), String> {
        let id = &entry.film.id;
        {
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
        let Some(location) = location.as_ref() else { return };
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

    fn url(&self, entry: &Entry) -> Option<String> {
        if entry.film.source != adapter::Source::Host {
            return None;
        }
        let web = self
            .0
            .location
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map_or_else(|| live_proto::DEFAULT_SERVER.to_owned(), |l| l.web.clone());
        if entry.film.id.is_empty() || matches!(entry.film.id.as_str(), "." | "..") {
            return None;
        }
        let mut id = String::new();
        use std::fmt::Write;
        for b in entry.film.id.bytes() {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'~' | b'.') {
                id.push(char::from(b));
            } else {
                let _ = write!(id, "%{b:02X}");
            }
        }
        Some(format!("{web}/theater/{id}"))
    }

    pub fn web_url(&self) -> Option<String> {
        let entry = {
            let state = self.state();
            state.web.then(|| state.selected.clone()).flatten()?
        };
        self.url(&entry)
    }

    pub(crate) fn connect(&self, agent: &agent::Agent) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theater_grows_only_the_main_menu_list() {
        assert_eq!(menu::count(menu::LIST_NAME, 5), 6);
        assert_eq!(menu::count(menu::LIST_NAME, 6), 6);
        assert_eq!(menu::count(crate::custom_games::LIST_NAME, 5), 5);
    }
}
