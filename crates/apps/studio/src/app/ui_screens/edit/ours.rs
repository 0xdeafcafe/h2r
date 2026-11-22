//! The "our screens" sink (docs/game/ui-builder.md "Editing in the
//! editor"): an edit of one of our screens sets a field of its
//! `ScreenDoc` (`boot::ui_kit::data`, `assets/ui/screens/<name>.yaml`) by
//! widget id: a widget's `text`, `at` and `size`, or a row's `text`.
//! Fields Rust owns are refused with the owner named.
//!
//! The sink keeps the docs as edited, the docs as last read or saved, and
//! an undo stack of whole-doc snapshots. Save writes `data::to_yaml` into
//! the checkout the editor was started from; the folder is polled so a
//! file changed on disk shows (and is taken when its screen has no
//! unsaved edits).
//!
//! Everything this module needs of `ui_kit::data` is in the few calls at
//! the top ([`with_docs`], [`baked`], [`parse`], [`to_yaml`]).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use boot::ui_kit::data::{self, ScreenDoc};
use boot::ui_kit::view::{Kind, Widget};

use super::Change;

/// Screen docs by screen name ([`boot::ui_kit::screens::NAMES`]).
pub type Docs = BTreeMap<String, ScreenDoc>;

/// Runs `f` with `docs` standing in for the baked screen files, so the
/// views it makes show them.
pub fn with_docs<R>(docs: &Docs, f: impl FnOnce() -> R) -> R {
    data::with_screens(docs, f)
}

/// Screen `screen`'s doc as the build baked it.
fn baked(screen: &str) -> Result<ScreenDoc, String> {
    data::baked(screen)
        .map(|d| (*d).clone())
        .ok_or_else(|| format!("{screen}: the screen has no doc"))
}

/// Screen `screen`'s doc from its file's text.
fn parse(screen: &str, text: &str) -> Result<ScreenDoc, String> {
    ScreenDoc::parse(screen, text, data::design()).map_err(|e| e.to_string())
}

fn to_yaml(doc: &ScreenDoc) -> String {
    data::to_yaml(doc)
}

/// Where the checkout keeps our screens' files.
pub const SCREENS_DIR: &str = "assets/ui/screens";

/// Screen `screen`'s file in `checkout`.
pub fn file_of(checkout: &Path, screen: &str) -> PathBuf {
    checkout.join(SCREENS_DIR).join(format!("{screen}.yaml"))
}

/// The checkout the editor was started from: the first folder at or
/// above the working directory, then above the executable, that has
/// [`SCREENS_DIR`].
pub fn find_checkout() -> Option<PathBuf> {
    let starts = [
        std::env::current_dir().ok(),
        std::env::current_exe().ok(),
    ];
    starts.into_iter().flatten().find_map(|start| {
        start
            .ancestors()
            .find(|a| a.join(SCREENS_DIR).is_dir())
            .map(Path::to_path_buf)
    })
}

/// How often the checkout's files are looked at for changes.
const POLL: Duration = Duration::from_secs(1);

/// One undo step: a screen's doc before and after.
#[derive(Clone, Debug)]
struct Step {
    screen: String,
    before: ScreenDoc,
    after: ScreenDoc,
    label: String,
    /// A step asked to merge joins the last one with the same key.
    merge: Option<String>,
}

/// The sink's state.
#[derive(Default)]
pub struct Ours {
    /// Each screen's doc as shown: the edits over the base.
    docs: Docs,
    /// Each screen's doc as last read from disk (or baked) or saved.
    base: Docs,
    /// Each file's modification time when last read or written.
    read: BTreeMap<String, Option<SystemTime>>,
    undo: Vec<Step>,
    redo: Vec<Step>,
    /// The last step may not take a merge (a drag let go).
    sealed: bool,
    checkout: Option<PathBuf>,
    looked: bool,
    /// Screens with unsaved edits whose file changed on disk since.
    changed: BTreeSet<String>,
    polled: Option<Instant>,
}

/// The slots of a template: each `{name}`.
pub fn slots(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut rest = text;
    while let Some(i) = rest.find('{') {
        let after = &rest[i + 1..];
        match after.find('}') {
            Some(j) if !after[..j].is_empty() && !after[..j].contains('{') => {
                out.insert(after[..j].to_owned());
                rest = &after[j + 1..];
            }
            _ => rest = after,
        }
    }
    out
}

/// Why widget `id` of `screen` is not ours to edit: what fills it.
fn rust_owned(screen: &str, id: &str) -> String {
    let what = if id.starts_with("row.") && id.ends_with(".value") {
        "a row's value, read from its setting by the row's Rust code"
    } else if id == "description" {
        "the focused row's description, which the row's Rust code supplies"
    } else if id == "status" {
        "the page's status line, set by its step code"
    } else if id.starts_with("cell.") {
        "an entry cell, filled as the page is typed into"
    } else if id.starts_with("row.") {
        "part of a row, laid out by the list"
    } else if id.starts_with("frame.") || id.starts_with("panel.") || id.starts_with("backing.")
    {
        "the layout's frame art, placed by the screen's layout"
    } else {
        "drawn by Rust"
    };
    format!(
        "refused: {id} is {what}; it has no entry in {SCREENS_DIR}/{screen}.yaml"
    )
}

/// `row.N.label`'s row id, `row.N`.
fn label_row(id: &str) -> Option<&str> {
    let row = id.strip_suffix(".label")?;
    let n = row.strip_prefix("row.")?;
    n.parse::<usize>().ok().map(|_| row)
}

/// Sets `change` of widget `w` in `doc`, or why not.
pub fn apply(screen: &str, doc: &mut ScreenDoc, w: &Widget, change: &Change) -> Result<(), String> {
    let id = w.id.as_str();
    match change {
        Change::Text(text) => {
            if !matches!(w.kind, Kind::Text { .. }) {
                return Err(format!("refused: {id} is art, not a text"));
            }
            let slot = match label_row(id) {
                Some(row) => doc.rows.get_mut(row).map(|r| Slot::Row(&mut r.text)),
                None => match doc.widgets.get_mut(id) {
                    Some(entry) if entry.prompts.is_some() => {
                        return Err(format!(
                            "refused: {id} is made from its prompts (`prompts:` in {SCREENS_DIR}/{screen}.yaml); \
                             edit them there"
                        ));
                    }
                    Some(entry) => Some(Slot::Widget(&mut entry.text)),
                    None => None,
                },
            };
            let Some(mut slot) = slot else {
                return Err(rust_owned(screen, id));
            };
            let Some(old) = slot.get() else {
                return Err(format!(
                    "refused: {id}'s text is Rust's (its entry in {SCREENS_DIR}/{screen}.yaml has no text)"
                ));
            };
            let (had, has) = (slots(old), slots(text));
            if let Some(gone) = had.difference(&has).next() {
                return Err(format!(
                    "refused: {id}'s {{{gone}}} slot is filled by Rust; keep it in the text"
                ));
            }
            if let Some(new) = has.difference(&had).next() {
                return Err(format!(
                    "refused: Rust fills no {{{new}}} slot in {id} (its slots: {})",
                    had.iter().map(|s| format!("{{{s}}}")).collect::<Vec<_>>().join(", ")
                ));
            }
            slot.set(text.clone());
            Ok(())
        }
        Change::Rect([top, left, bottom, right]) => {
            if matches!(id, "title" | "legend") {
                return Err(format!(
                    "refused: {id} sits where the header layout puts it (`layout:` in {SCREENS_DIR}/{screen}.yaml)"
                ));
            }
            if id.starts_with("row.") {
                return Err(format!(
                    "refused: {id} sits where the list's layout puts it (`layout:` in {SCREENS_DIR}/{screen}.yaml)"
                ));
            }
            let Some(entry) = doc.widgets.get_mut(id) else {
                return Err(rust_owned(screen, id));
            };
            entry.at = Some([*left as _, *top as _]);
            entry.size = Some([(right - left) as _, (top - bottom) as _]);
            Ok(())
        }
    }
}

/// Where a text edit lands: a row's label (always there) or a widget's
/// text (none when Rust makes it).
enum Slot<'a> {
    Row(&'a mut String),
    Widget(&'a mut Option<String>),
}

impl Slot<'_> {
    fn get(&self) -> Option<&String> {
        match self {
            Slot::Row(s) => Some(s),
            Slot::Widget(s) => s.as_ref(),
        }
    }

    fn set(&mut self, text: String) {
        match self {
            Slot::Row(s) => **s = text,
            Slot::Widget(s) => **s = Some(text),
        }
    }
}

/// Widget `id`'s entry in `doc` put back as `base` has it.
fn revert_widget(doc: &mut ScreenDoc, base: &ScreenDoc, id: &str) -> Result<(), String> {
    if let Some(row) = label_row(id) {
        match (doc.rows.get_mut(row), base.rows.get(row)) {
            (Some(d), Some(b)) => *d = b.clone(),
            _ => return Err(format!("{id} has no row entry to revert")),
        }
        return Ok(());
    }
    match (doc.widgets.get_mut(id), base.widgets.get(id)) {
        (Some(d), Some(b)) => {
            *d = b.clone();
            Ok(())
        }
        _ => Err(format!("{id} has no widget entry to revert")),
    }
}

impl Ours {
    /// Sets the checkout (tests, or a window told where it is); `None`
    /// means there is none.
    pub fn set_checkout(&mut self, checkout: Option<PathBuf>) {
        self.checkout = checkout;
        self.looked = true;
    }

    /// The checkout the files are read from and saved into.
    pub fn checkout_path(&self) -> Option<&Path> {
        self.checkout.as_deref()
    }

    fn checkout(&mut self) -> Option<PathBuf> {
        if !self.looked {
            self.checkout = find_checkout();
            self.looked = true;
        }
        self.checkout.clone()
    }

    fn mtime(path: &Path) -> Option<SystemTime> {
        std::fs::metadata(path).and_then(|m| m.modified()).ok()
    }

    /// Screen `screen`'s doc as the checkout has it, else as baked.
    fn read(&mut self, screen: &str) -> Result<ScreenDoc, String> {
        if let Some(c) = self.checkout() {
            let path = file_of(&c, screen);
            if path.is_file() {
                let text = std::fs::read_to_string(&path)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
                self.read.insert(screen.to_owned(), Self::mtime(&path));
                return parse(screen, &text).map_err(|e| format!("{}: {e}", path.display()));
            }
        }
        self.read.insert(screen.to_owned(), None);
        baked(screen)
    }

    /// Screen `screen`'s doc as shown, read on first use.
    pub fn doc(&mut self, screen: &str) -> Result<&ScreenDoc, String> {
        if !self.docs.contains_key(screen) {
            let doc = self.read(screen)?;
            self.base.insert(screen.to_owned(), doc.clone());
            self.docs.insert(screen.to_owned(), doc);
        }
        Ok(&self.docs[screen])
    }

    /// The docs the views are drawn with: every screen read so far, as
    /// shown. Files on disk show unedited; unread screens are baked.
    pub fn shown(&self) -> Docs {
        self.docs.clone()
    }

    /// Sets `change` of widget `w` on `screen` as one undo step (or merged
    /// into the last). The step's label.
    pub fn change(
        &mut self,
        screen: &str,
        w: &Widget,
        change: Change,
        merge: bool,
    ) -> Result<String, String> {
        let before = self.doc(screen)?.clone();
        let mut after = before.clone();
        apply(screen, &mut after, w, &change)?;
        let what = match &change {
            Change::Rect(r) => format!("{} to {r:?}", w.id),
            Change::Text(t) => format!("{} text to {t:?}", w.id),
        };
        let label = format!("{screen}: {what}");
        let key = format!(
            "{screen}:{}:{}",
            w.id,
            matches!(change, Change::Rect(_)) as u8
        );
        if after == before {
            return Ok(label);
        }
        self.docs.insert(screen.to_owned(), after.clone());
        self.redo.clear();
        if merge
            && !self.sealed
            && let Some(last) = self.undo.last_mut()
            && last.merge.as_deref() == Some(key.as_str())
        {
            last.after = after;
            last.label = label.clone();
            return Ok(label);
        }
        self.sealed = false;
        self.undo.push(Step {
            screen: screen.to_owned(),
            before,
            after,
            label: label.clone(),
            merge: Some(key),
        });
        Ok(label)
    }

    /// Ends the current merge run.
    pub fn seal(&mut self) {
        self.sealed = true;
    }

    /// Puts widget `id` (or the whole screen) back as last read or saved.
    pub fn revert(&mut self, screen: &str, id: Option<&str>) -> Result<String, String> {
        let before = self.doc(screen)?.clone();
        let base = self.base[screen].clone();
        let after = match id {
            None => base,
            Some(id) => {
                let mut d = before.clone();
                revert_widget(&mut d, &base, id)?;
                d
            }
        };
        let label = match id {
            Some(id) => format!("{screen}: reverted {id}"),
            None => format!("{screen}: reverted the screen"),
        };
        if after == before {
            return Ok(format!("{label} (nothing was edited)"));
        }
        self.docs.insert(screen.to_owned(), after.clone());
        self.redo.clear();
        self.sealed = true;
        self.undo.push(Step {
            screen: screen.to_owned(),
            before,
            after,
            label: label.clone(),
            merge: None,
        });
        Ok(label)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Undoes the last step; its label.
    pub fn undo(&mut self) -> Option<String> {
        let s = self.undo.pop()?;
        self.docs.insert(s.screen.clone(), s.before.clone());
        let label = s.label.clone();
        self.redo.push(s);
        self.sealed = true;
        Some(label)
    }

    pub fn redo(&mut self) -> Option<String> {
        let s = self.redo.pop()?;
        self.docs.insert(s.screen.clone(), s.after.clone());
        let label = s.label.clone();
        self.undo.push(s);
        self.sealed = true;
        Some(label)
    }

    /// Drops every unsaved edit and the history.
    pub fn discard(&mut self) {
        self.docs = self.base.clone();
        self.undo.clear();
        self.redo.clear();
        self.changed.clear();
        self.sealed = true;
    }

    /// Screens whose doc differs from the one last read or saved.
    pub fn dirty_screens(&self) -> Vec<String> {
        self.docs
            .iter()
            .filter(|(s, d)| self.base.get(*s) != Some(*d))
            .map(|(s, _)| s.clone())
            .collect()
    }

    pub fn dirty(&self) -> bool {
        !self.dirty_screens().is_empty()
    }

    /// Screens with unsaved edits whose file changed on disk.
    pub fn changed_on_disk(&self) -> Vec<String> {
        self.changed.iter().cloned().collect()
    }

    /// Writes every edited screen's file into the checkout; the paths.
    pub fn save(&mut self) -> Result<Vec<PathBuf>, String> {
        let dirty = self.dirty_screens();
        if dirty.is_empty() {
            return Ok(Vec::new());
        }
        let Some(checkout) = self.checkout() else {
            return Err(format!(
                "refused: the editor was not started from an h2recomp checkout (no {SCREENS_DIR} \
                 at or above the working directory or the executable), and our screens' files \
                 live there"
            ));
        };
        let mut written = Vec::new();
        for screen in dirty {
            let doc = self.docs[&screen].clone();
            let path = file_of(&checkout, &screen);
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            }
            std::fs::write(&path, to_yaml(&doc)).map_err(|e| format!("{}: {e}", path.display()))?;
            self.read.insert(screen.clone(), Self::mtime(&path));
            self.base.insert(screen.clone(), doc);
            self.changed.remove(&screen);
            written.push(path);
        }
        Ok(written)
    }

    /// Reads every screen read so far again from the checkout: a screen
    /// with unsaved edits keeps them. How many were read.
    pub fn reload(&mut self) -> Result<usize, String> {
        let screens: Vec<String> = self.docs.keys().cloned().collect();
        let mut n = 0;
        for screen in screens {
            let doc = self.read(&screen)?;
            let edited = self.base.get(&screen) != self.docs.get(&screen);
            if edited {
                if self.base.get(&screen) != Some(&doc) {
                    self.changed.insert(screen.clone());
                }
                continue;
            }
            self.base.insert(screen.clone(), doc.clone());
            self.docs.insert(screen.clone(), doc);
            self.changed.remove(&screen);
            n += 1;
        }
        Ok(n)
    }

    /// At most once a [`POLL`]: reads again the files that changed on disk
    /// since they were read. Whether anything shown changed.
    pub fn poll(&mut self) -> bool {
        if self.polled.is_some_and(|t| t.elapsed() < POLL) {
            return false;
        }
        self.polled = Some(Instant::now());
        let Some(checkout) = self.checkout() else {
            return false;
        };
        let stale: Vec<String> = self
            .docs
            .keys()
            .filter(|s| {
                let now = Self::mtime(&file_of(&checkout, s));
                self.read.get(*s).is_some_and(|was| *was != now)
            })
            .cloned()
            .collect();
        let mut shown = false;
        for screen in stale {
            let Ok(doc) = self.read(&screen) else {
                continue;
            };
            if self.base.get(&screen) != self.docs.get(&screen) {
                self.changed.insert(screen);
                continue;
            }
            self.base.insert(screen.clone(), doc.clone());
            self.docs.insert(screen, doc);
            shown = true;
        }
        shown
    }

    /// The edits as `ui.edits` reports them: each edited screen's file and
    /// its doc as YAML.
    pub fn edits_json(&self) -> serde_json::Value {
        let checkout = self.checkout.as_deref();
        serde_json::Value::Array(
            self.dirty_screens()
                .into_iter()
                .map(|s| {
                    serde_json::json!({
                        "screen": s,
                        "file": checkout.map(|c| file_of(c, &s).display().to_string()),
                        "yaml": to_yaml(&self.docs[&s]),
                    })
                })
                .collect(),
        )
    }
}
