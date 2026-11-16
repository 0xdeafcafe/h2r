//! The playlist (hopper) editor's state and commands with no UI: the open
//! `multiplayer_hopper.dat` and its working copy, undo, the map list and
//! the variant picker's sources, reading and writing both forms of the
//! file. h2-editor's egui tool and h2-studio's Dear ImGui tool draw it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use editor_core::{MapSource, UndoStack, variant_sources};
use live_proto::hopper::{HopperFile, INFLATED_SIZE, MAX_FILE};
use variants::GameVariant;
use variants::save::SaveStore;

use crate::catalog::Catalog;
use crate::choices::Choices;
use crate::model::{self, EditHopper};

/// What reading, editing or writing a hopper file can fail with.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The file or the edits do not validate; nothing was written.
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// How the file was stored, so a save writes it back the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// The download envelope (header, size, zlib) the server publishes.
    Envelope,
    /// The raw 0x162cc0-byte image the console caches
    /// (`n:\multiplayer_hopper.dat`).
    Image,
}

impl Format {
    pub fn describe(self) -> &'static str {
        match self {
            Format::Envelope => "download envelope",
            Format::Image => "raw image",
        }
    }
}

/// Reads either form of the file.
pub fn read_file(bytes: &[u8]) -> Result<(HopperFile, Format)> {
    let bad = |e: live_proto::hopper::HopperError| Error::Invalid(e.to_string());
    if bytes.len() == INFLATED_SIZE {
        Ok((HopperFile::from_image(bytes).map_err(bad)?, Format::Image))
    } else {
        Ok((
            HopperFile::parse_legacy(bytes).map_err(bad)?,
            Format::Envelope,
        ))
    }
}

/// The bytes a save writes: the envelope (refused when larger than the
/// game's download buffer) or the raw image.
pub fn write_file(file: &HopperFile, format: Format) -> Result<Vec<u8>> {
    match format {
        Format::Image => Ok(file.image().to_vec()),
        Format::Envelope => file.build_checked().map_err(|e| {
            Error::Invalid(format!(
                "{e}; the game's buffer is {MAX_FILE} bytes, so it would keep its built-in playlists"
            ))
        }),
    }
}

/// What undo steps through: the image the working copy was loaded from
/// (shared, never cloned) and the working copy.
#[derive(Debug, Clone)]
struct State {
    base: Arc<HopperFile>,
    baseline: Arc<Vec<EditHopper>>,
    hoppers: Vec<EditHopper>,
}

impl PartialEq for State {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.base, &other.base) && self.hoppers == other.hoppers
    }
}

impl State {
    fn new(base: Arc<HopperFile>) -> Self {
        let baseline = Arc::new(model::load(&base));
        State {
            hoppers: (*baseline).clone(),
            base,
            baseline,
        }
    }
}

struct Doc {
    state: State,
    /// What is on disk.
    saved: Arc<HopperFile>,
    path: Option<PathBuf>,
    format: Format,
    undo: UndoStack<State>,
}

enum CatalogState {
    None,
    Loaded(PathBuf, Catalog),
    Failed(PathBuf, String),
}

/// The variant picker's sources.
struct VariantSources {
    /// The map source they were read for; `None` when set by
    /// [`HopperSession::set_variant_sources`] (never re-read).
    auto_for: Option<Option<PathBuf>>,
    store: Option<SaveStore>,
    choices: Choices,
}

/// What an edit sees besides the working copy.
pub struct EditView<'a> {
    /// The image the working copy is relative to.
    pub base: &'a HopperFile,
    /// The working copy as loaded or last saved.
    pub baseline: &'a [EditHopper],
    /// The map list, when one was read.
    pub catalog: Option<&'a Catalog>,
    /// The variant picker's choices, once read.
    pub choices: Option<&'a Choices>,
}

/// The playlist editor.
pub struct HopperSession {
    doc: Option<Doc>,
    catalog: CatalogState,
    variants: Option<VariantSources>,
    /// The runtime data root (`--data`); `None` is the default one.
    data_root: Option<PathBuf>,
}

impl Default for HopperSession {
    fn default() -> Self {
        Self::new()
    }
}

impl HopperSession {
    pub fn new() -> Self {
        HopperSession {
            doc: None,
            catalog: CatalogState::None,
            variants: None,
            data_root: None,
        }
    }

    /// Read saved variants from the storage under `root` (`--data`).
    pub fn set_data_root(&mut self, root: PathBuf) {
        self.data_root = Some(root);
        self.variants = None;
    }

    /// Sets the variant picker's built-ins (from `schema`) and saves (from
    /// `store`), instead of reading them from the opened disc and the
    /// runtime's save storage. Loaded variant files stay.
    pub fn set_variant_sources(&mut self, schema: &variants::Schema, store: Option<SaveStore>) {
        let mut choices = Choices::new(schema, store.as_ref());
        if let Some(old) = self.variants.take() {
            choices.files = old.choices.files;
        }
        self.variants = Some(VariantSources {
            auto_for: None,
            store,
            choices,
        });
    }

    /// What the variant picker offers, once read.
    pub fn variant_choices(&self) -> Option<&Choices> {
        self.variants.as_ref().map(|v| &v.choices)
    }

    /// Re-reads the saved variants.
    pub fn refresh_saved_variants(&mut self) {
        if let Some(VariantSources {
            store: Some(store),
            choices,
            ..
        }) = &mut self.variants
        {
            choices.refresh_saved(store);
        }
    }

    /// Adds a `.h2variant` file to the picker; returns its label.
    pub fn load_variant_file(&mut self, path: &Path) -> Result<String, String> {
        let v = self.variants.get_or_insert_with(|| VariantSources {
            auto_for: None,
            store: None,
            choices: Choices::default(),
        });
        let c = v.choices.load_file(path)?;
        Ok(if c.usable() {
            c.label()
        } else {
            format!("{} (the game would reject it)", c.label())
        })
    }

    /// Reads the picker's sources for `source` when it changed since the
    /// last read: built-ins from its main menu map, saves from the
    /// runtime's storage. Sources set by [`Self::set_variant_sources`] stay.
    pub fn refresh_variants(&mut self, source: Option<&MapSource>) {
        let now = source.map(|s| s.path().to_owned());
        if let Some(v) = &self.variants
            && v.auto_for.as_ref().is_none_or(|p| *p == now)
        {
            return;
        }
        let (schema, _) = variant_sources::schema(source);
        let store = Some(variant_sources::store(source, self.data_root.as_deref()));
        self.set_variant_sources(&schema, store);
        if let Some(v) = &mut self.variants {
            v.auto_for = Some(now);
        }
    }

    /// The game variant entry `entry` of playlist `hopper` will carry,
    /// decoded from its slot.
    pub fn entry_variant(&self, hopper: usize, entry: usize) -> Option<GameVariant> {
        let doc = self.doc.as_ref()?;
        let r = doc.state.hoppers.get(hopper)?.records.get(entry)?;
        model::entry_variant(&doc.state.base, r)
    }

    /// Links `v` to an entry, as one undo step; refuses a variant the game
    /// would reject ([`model::set_variant`]).
    pub fn set_entry_variant(
        &mut self,
        hopper: usize,
        entry: usize,
        v: &GameVariant,
    ) -> Result<()> {
        let mut r = self
            .hoppers()
            .and_then(|h| h.get(hopper))
            .and_then(|h| h.records.get(entry))
            .cloned()
            .ok_or_else(|| Error::Invalid("no such entry".into()))?;
        model::set_variant(&mut r, v).map_err(|e| Error::Invalid(e.to_string()))?;
        self.edit(|hs| hs[hopper].records[entry] = r);
        Ok(())
    }

    /// Starts from bytes read from `path` (either form).
    pub fn open_bytes(&mut self, bytes: &[u8], path: Option<PathBuf>) -> Result<()> {
        let (file, format) = read_file(bytes)?;
        self.adopt(file, format, path);
        Ok(())
    }

    /// Opens the file at `path`; returns the form it was in.
    pub fn open_path(&mut self, path: &Path) -> Result<Format> {
        let bytes = std::fs::read(path)?;
        self.open_bytes(&bytes, Some(path.to_owned()))?;
        Ok(self.format().expect("just opened"))
    }

    /// Starts from a blank image (every hopper empty), saved as an
    /// envelope.
    pub fn new_blank(&mut self) {
        self.adopt(HopperFile::blank(), Format::Envelope, None);
    }

    /// Starts from the Live server's built-in lineup (what a server serves
    /// before any file is published; `HopperFile::default_set`), saved as
    /// an envelope.
    pub fn new_default_set(&mut self) {
        self.adopt(HopperFile::default_set(), Format::Envelope, None);
    }

    /// Closes the file, dropping unsaved edits.
    pub fn close(&mut self) {
        self.doc = None;
    }

    fn adopt(&mut self, file: HopperFile, format: Format, path: Option<PathBuf>) {
        let base = Arc::new(file);
        self.doc = Some(Doc {
            state: State::new(base.clone()),
            saved: base,
            path,
            format,
            undo: UndoStack::default(),
        });
    }

    pub fn is_open(&self) -> bool {
        self.doc.is_some()
    }

    /// The working copy.
    pub fn hoppers(&self) -> Option<&[EditHopper]> {
        self.doc.as_ref().map(|d| &d.state.hoppers[..])
    }

    /// The working copy as loaded (or last saved): what validation compares
    /// against and presets copy variants from.
    pub fn baseline(&self) -> Option<&[EditHopper]> {
        self.doc.as_ref().map(|d| &d.state.baseline[..])
    }

    /// The image the working copy is relative to.
    pub fn base(&self) -> Option<&HopperFile> {
        self.doc.as_ref().map(|d| &*d.state.base)
    }

    pub fn format(&self) -> Option<Format> {
        self.doc.as_ref().map(|d| d.format)
    }

    /// The file being edited, once it has one.
    pub fn path(&self) -> Option<&Path> {
        self.doc.as_ref().and_then(|d| d.path.as_deref())
    }

    /// Edits the working copy as one undo step.
    pub fn edit(&mut self, f: impl FnOnce(&mut Vec<EditHopper>)) {
        self.edit_with(|hs, _| (f(hs), None));
    }

    /// Edits the working copy with everything a UI draws it with in view
    /// (a frame of a panel). `f` returns its result and the edit's key:
    /// edits with the same key in a row (typing in one field, dragging one
    /// number) make one undo step, and an edit that changes nothing makes
    /// none. Returns `None` with no file open.
    pub fn edit_with<R>(
        &mut self,
        f: impl FnOnce(&mut Vec<EditHopper>, &EditView<'_>) -> (R, Option<u64>),
    ) -> Option<R> {
        let doc = self.doc.as_mut()?;
        let before = doc.state.clone();
        let base = doc.state.base.clone();
        let baseline = doc.state.baseline.clone();
        let view = EditView {
            base: &base,
            baseline: &baseline,
            catalog: match &self.catalog {
                CatalogState::Loaded(_, c) => Some(c),
                _ => None,
            },
            choices: self.variants.as_ref().map(|v| &v.choices),
        };
        let (out, key) = f(&mut doc.state.hoppers, &view);
        doc.undo.track(before, &doc.state, key);
        Some(out)
    }

    /// Ends a run of keyed edits: the next edit is a new undo step.
    pub fn seal(&mut self) {
        if let Some(d) = &mut self.doc {
            d.undo.break_coalescing();
        }
    }

    /// Validation errors of the changed hoppers.
    pub fn errors(&self) -> Vec<String> {
        self.doc
            .as_ref()
            .map(|d| model::errors(&d.state.baseline, &d.state.hoppers))
            .unwrap_or_default()
    }

    /// The edited image, or why it cannot be written.
    pub fn build(&self) -> Result<HopperFile> {
        let doc = self
            .doc
            .as_ref()
            .ok_or(Error::Invalid("no file open".into()))?;
        model::apply(&doc.state.base, &doc.state.hoppers).map_err(|e| Error::Invalid(e.to_string()))
    }

    /// Writes the edits to `path` in the file's format; on success the
    /// written image becomes the new base (sources renumbered), as one undo
    /// step so undo can still walk back past the save. Returns the bytes
    /// written.
    pub fn save_to(&mut self, path: &Path) -> Result<usize> {
        let file = self.build()?;
        let doc = self.doc.as_mut().expect("built");
        let bytes = write_file(&file, doc.format)?;
        std::fs::write(path, &bytes)?;
        if doc.state.hoppers == *doc.state.baseline {
            // No edits: what was written is the base image itself.
            doc.saved = doc.state.base.clone();
        } else {
            let base = Arc::new(file);
            let next = State::new(base.clone());
            doc.undo.record(std::mem::replace(&mut doc.state, next));
            doc.saved = base;
        }
        doc.path = Some(path.to_owned());
        Ok(bytes.len())
    }

    /// Unsaved changes: edits, or a save undone past.
    pub fn is_dirty(&self) -> bool {
        self.doc.as_ref().is_some_and(|d| {
            !Arc::ptr_eq(&d.state.base, &d.saved) || d.state.hoppers != *d.state.baseline
        })
    }

    pub fn can_undo(&self) -> bool {
        self.doc.as_ref().is_some_and(|d| d.undo.can_undo())
    }

    pub fn can_redo(&self) -> bool {
        self.doc.as_ref().is_some_and(|d| d.undo.can_redo())
    }

    pub fn undo(&mut self) -> bool {
        self.doc.as_mut().is_some_and(|d| d.undo.undo(&mut d.state))
    }

    pub fn redo(&mut self) -> bool {
        self.doc.as_mut().is_some_and(|d| d.undo.redo(&mut d.state))
    }

    /// Sets the map list (normally read from the disc by
    /// [`Self::refresh_catalog`]).
    pub fn set_catalog(&mut self, from: PathBuf, catalog: Catalog) {
        self.catalog = CatalogState::Loaded(from, catalog);
    }

    /// The map list, when one was read.
    pub fn catalog(&self) -> Option<&Catalog> {
        match &self.catalog {
            CatalogState::Loaded(_, c) => Some(c),
            _ => None,
        }
    }

    /// Why there is no map list, when there is none.
    pub fn catalog_note(&self) -> Option<String> {
        match &self.catalog {
            CatalogState::None => {
                Some("No disc open: maps show as numbers and presets cannot resolve.".to_string())
            }
            CatalogState::Failed(_, e) => Some(format!("No map list from the disc ({e}).")),
            CatalogState::Loaded(..) => None,
        }
    }

    /// Reads the map list from `source` when it changed.
    pub fn refresh_catalog(&mut self, source: Option<&MapSource>) {
        let Some(source) = source else {
            return;
        };
        let path = source.path();
        let current = match &self.catalog {
            CatalogState::Loaded(p, _) | CatalogState::Failed(p, _) => Some(p.as_path()),
            CatalogState::None => None,
        };
        if current == Some(path) {
            return;
        }
        self.catalog = match Catalog::from_source(source) {
            Ok(c) => CatalogState::Loaded(path.to_owned(), c),
            Err(e) => CatalogState::Failed(path.to_owned(), format!("{e:#}")),
        };
    }
}

#[cfg(test)]
mod tests;
