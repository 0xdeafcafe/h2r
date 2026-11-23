//! Scripts in VS Code and Zed (docs/editor/editor-guide.md, "Scripts in
//! VS Code and Zed"): the scenario's HaloScript as a working copy on disk,
//! and the polling that keeps both kinds of script in step with their
//! files.
//!
//! The working copy is every global and script of the open map,
//! decompiled as the Scripts tab shows them, in one `.hsc` file under
//! `<data root>/scripts/<map>/`, with the context `h2 hs lsp` checks it
//! against beside it (`hs::ide::CONTEXT_FILE`: the executable's builtins
//! and types, and the scenario's names). Both are made from the user's own
//! map and executable and stay in the data root. A save there is read
//! back and checked here; which definitions differ from the map is
//! [`Changes`]. The map itself is not written: that needs the script
//! compiler (docs/editor/rich-editors.md, "Scripts", slices H2 and H3).
//! When the map's scripts or names change in Studio, the context is
//! written again, and the working copy too while it holds no edits
//! ([`WorkingCopy::written`]); with edits it is left alone and marked
//! [`WorkingCopy::stale`].

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use hs::ide::{self, Context, DefItem};
use hs::{Expression, GlobalIndex, Item, Literal, NameFrom};

use super::rules_edit::{self, DiskPoll, Launch, Launched, Watch, modified};
use super::scripts::Loaded;

/// The file beside the working copy holding the text Studio last wrote,
/// so a working copy left from an earlier session is written over only
/// when it holds no edits.
pub const BASE_FILE: &str = ".h2hs-base.hsc";

/// Which definitions of a working copy differ from the map's, by name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    /// In both, with other tokens (layout and comments aside).
    pub changed: Vec<String>,
    /// Only in the working copy.
    pub added: Vec<String>,
    /// Only in the map.
    pub removed: Vec<String>,
}

impl Changes {
    pub fn is_empty(&self) -> bool {
        self.changed.is_empty() && self.added.is_empty() && self.removed.is_empty()
    }

    pub fn count(&self) -> usize {
        self.changed.len() + self.added.len() + self.removed.len()
    }

    /// `main and helper edited, intro added`.
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        for (names, what) in [
            (&self.changed, "edited"),
            (&self.added, "added"),
            (&self.removed, "removed"),
        ] {
            if !names.is_empty() {
                parts.push(format!("{} {what}", names.join(", ")));
            }
        }
        parts.join("; ")
    }
}

/// The open map's working copy.
pub struct WorkingCopy {
    pub map: String,
    pub path: PathBuf,
    pub context: Context,
    /// What Studio last wrote to the file (also in [`BASE_FILE`]); empty
    /// when the file held edits from before.
    pub written: String,
    /// The file's text as last read.
    pub text: String,
    pub watch: Watch,
    pub problems: Vec<ide::Diagnostic>,
    pub changes: Changes,
    /// The map's scripts changed while the file held edits: Studio did
    /// not write over them, so the file is of an older map.
    pub stale: bool,
    /// The scripts' read it was made from (`ScriptsTool::for_rev`).
    pub(crate) for_rev: Option<Option<u64>>,
    /// The editor's start ([`super::rules_edit::launch`]).
    pub launched: Option<Launched>,
}

impl WorkingCopy {
    /// Problems and changes again, for the text as it is now.
    fn check(&mut self, l: &Loaded) {
        self.problems = ide::diagnostics(&self.text, &self.context);
        self.changes = changes(l, &self.text, &self.context);
    }

    /// The 1-based line `name` starts on in the file, else 1.
    pub fn line_of(&self, name: &str) -> u32 {
        ide::definitions(&self.text, &self.context)
            .into_iter()
            .find(|d| d.name == name)
            .map_or(1, |d| d.line + 1)
    }

    /// One line for the log and the Scripts tab.
    pub fn summary(&self) -> String {
        let file = self
            .path
            .file_name()
            .map_or_else(|| self.path.display().to_string(), |n| n.to_string_lossy().into_owned());
        let errors = self.problems.iter().filter(|p| p.severity == ide::severity::ERROR).count();
        let problems = match errors {
            0 => String::new(),
            1 => ", 1 error".to_owned(),
            n => format!(", {n} errors"),
        };
        if self.changes.is_empty() {
            format!("{file}: as the map has it{problems}")
        } else {
            format!(
                "{file}: {}{problems}. Writing scripts into the map needs the script compiler, which is not built yet",
                self.changes.describe()
            )
        }
    }
}

/// A map's name as a file name: letters, digits, `-`, `_` and `.`.
pub fn file_stem(map: &str) -> String {
    let s: String = map
        .chars()
        .map(|c| if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '_' })
        .collect();
    if s.is_empty() || s.starts_with('.') { format!("map{s}") } else { s }
}

/// Where a map's working copy goes: `<data root>/scripts/<map>/<map>.hsc`.
pub fn working_path(data_root: &Path, map: &str) -> PathBuf {
    let stem = file_stem(map);
    data_root
        .join("scripts")
        .join(&stem)
        .join(format!("{stem}.{}", ide::EXTENSION))
}

/// The working copy's text: a line saying what it is, then every global
/// and script as the Scripts tab shows it, a blank line between.
pub fn working_text(l: &Loaded) -> String {
    let mut out = format!(
        "; {}: the map's scripts, decompiled by Studio. A save comes back to Studio; the map keeps no comments or layout.\n",
        l.map
    );
    for e in &l.entries {
        out.push('\n');
        out.push_str(&e.source.source);
    }
    out
}

/// The context `h2 hs lsp` checks the working copy against: the
/// executable's types, script kinds and builtins (named as the Scripts tab
/// names them), the game globals the scripts name, and the scenario's
/// object names, trigger volumes and the other names the scripts spell.
pub fn context(l: &Loaded, scenario: Option<&scenario_edit::Scenario>) -> Context {
    use hs::casts::types::{OBJECT_NAME, SCENERY_NAME};
    let catalog = &l.catalog;
    let ty = |t: u16| catalog.type_name(t).unwrap_or("?").to_owned();
    let doc = l.doc.as_ref().and_then(|(_, d)| d.as_ref().ok());
    let builtins = (0..l.naming.len())
        .filter_map(|i| {
            let f = catalog.function(i as u16).ok()?;
            let name = l.naming.name(i).map(str::to_owned);
            let from = l.naming.from(i);
            let described = match from {
                Some(NameFrom::Doc(d)) => doc.and_then(|doc| doc.functions.get(d)),
                _ => name
                    .as_deref()
                    .and_then(|n| doc.and_then(|doc| doc.functions.iter().find(|e| e.name == n))),
            };
            Some(ide::BuiltinInfo {
                index: i as u32,
                name,
                returns: ty(f.return_type()),
                parameters: f.parameters().iter().map(|&p| ty(p)).collect(),
                own_arguments: f.flags() & 2 != 0,
                usage: f.usage().map(str::to_owned),
                doc: described.map(|e| e.description.clone()).filter(|d| !d.is_empty()),
                from: from.map(|f| match f {
                    NameFrom::Map => "map".to_owned(),
                    NameFrom::Doc(_) => "doc".to_owned(),
                }),
            })
        })
        .collect();
    let object_types: Vec<String> = (OBJECT_NAME..=SCENERY_NAME)
        .filter_map(|t| catalog.type_name(t).ok().map(str::to_owned))
        .collect();
    let mut names: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut externals: BTreeMap<String, String> = BTreeMap::new();
    for e in &l.entries {
        for t in e.source.tokens.iter().filter(|t| !t.head) {
            match l.tables.expression(t.datum) {
                Ok(Expression::Global {
                    index: GlobalIndex::External(_),
                    name,
                    result_type,
                }) => {
                    externals.insert(name.to_owned(), ty(result_type));
                }
                Ok(Expression::Atom {
                    source_type,
                    literal: Literal::Symbol { spelling, .. },
                    ..
                }) => {
                    let key = if (OBJECT_NAME..=SCENERY_NAME).contains(&source_type) {
                        "object_name".to_owned()
                    } else {
                        ty(source_type)
                    };
                    names.entry(key).or_default().insert(spelling.to_owned());
                }
                _ => {}
            }
        }
    }
    if let Some(s) = scenario {
        let objects = names.entry("object_name".to_owned()).or_default();
        objects.extend((0..s.names.len()).filter_map(|i| s.names.name(i).map(str::to_owned)));
        let volumes = names.entry("trigger_volume".to_owned()).or_default();
        volumes.extend(s.volumes.triggers.iter().filter_map(|v| v.name.clone()));
    }
    let kinds = catalog.script_kinds();
    Context {
        version: ide::CONTEXT_VERSION,
        map: l.map.clone(),
        types: catalog.types().to_vec(),
        script_kinds: kinds.to_vec(),
        typed_kinds: kinds
            .iter()
            .enumerate()
            .filter(|(i, _)| hs::script_kind_has_type(*i as u16))
            .map(|(_, k)| k.clone())
            .collect(),
        builtins,
        external_globals: externals
            .into_iter()
            .map(|(name, type_name)| ide::NamedType { name, type_name })
            .collect(),
        object_types,
        names: names.into_iter().map(|(k, v)| (k, v.into_iter().collect())).collect(),
    }
}

/// Which definitions of `text` differ from the map's.
pub fn changes(l: &Loaded, text: &str, ctx: &Context) -> Changes {
    let theirs = ide::definitions(text, ctx);
    let item = |i: Item| match i {
        Item::Global(_) => DefItem::Global,
        Item::Script(_) => DefItem::Script,
    };
    let mut out = Changes::default();
    for e in &l.entries {
        match theirs
            .iter()
            .find(|d| d.item == item(e.source.item) && d.name == e.source.name)
        {
            None => out.removed.push(e.source.name.clone()),
            Some(d) if ide::normalise(&d.text) != ide::normalise(&e.source.source) => {
                out.changed.push(e.source.name.clone())
            }
            Some(_) => {}
        }
    }
    for d in &theirs {
        if !l
            .entries
            .iter()
            .any(|e| item(e.source.item) == d.item && e.source.name == d.name)
        {
            out.added.push(d.name.clone());
        }
    }
    out
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

impl super::App {
    /// Writes the open map's working copy and its context, or (for the map
    /// it already has) refreshes the context and keeps the file; watches
    /// the file. Returns the file and the 1-based line of `name` (the
    /// shown definition when `None`).
    pub fn script_working_copy(&mut self, name: Option<&str>) -> Result<(PathBuf, u32), String> {
        self.sync_scripts();
        let root = self.data_root();
        let for_rev = self.scripts.for_rev;
        let l = self.scripts_loaded()?;
        let ctx = context(l, self.session.scenario());
        let fresh = working_text(l);
        let path = working_path(&root, &l.map);
        let dir = path.parent().expect("a folder").to_path_buf();
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        write(&dir.join(ide::CONTEXT_FILE), &ctx.to_json())?;
        let base_path = dir.join(BASE_FILE);
        let disk = std::fs::read_to_string(&path).ok();
        let base = std::fs::read_to_string(&base_path).ok();
        let (written, text, stale) = match disk {
            // No file, or one with no edits: the map's scripts as they are.
            None => (fresh.clone(), fresh.clone(), false),
            Some(d) if Some(&d) == base.as_ref() || d == fresh => (fresh.clone(), fresh.clone(), false),
            // Edits from before: kept.
            Some(d) => {
                let stale = base.as_ref().is_some_and(|b| *b != fresh);
                (String::new(), d, stale)
            }
        };
        if !written.is_empty() {
            write(&path, &written)?;
            write(&base_path, &written)?;
        }
        let map = l.map.clone();
        let mut wc = WorkingCopy {
            map,
            watch: Watch::new(path.clone(), modified(&path)),
            path: path.clone(),
            context: ctx,
            written,
            text,
            problems: Vec::new(),
            changes: Changes::default(),
            stale,
            for_rev,
            launched: None,
        };
        wc.check(l);
        let shown = name.map(str::to_owned).or_else(|| {
            self.scripts
                .selected
                .and_then(|i| l.entry(i))
                .map(|e| e.source.name.clone())
        });
        let line = shown.map_or(1, |n| wc.line_of(&n));
        if !wc.changes.is_empty() {
            self.info(format!("kept your edits: {}", wc.summary()));
        }
        self.scripts.external = Some(wc);
        Ok((path, line))
    }

    /// The working copy ([`Self::script_working_copy`]) and the ways to
    /// open it in `only` (`code`, `zed`) or any editor at the definition
    /// `name` (the shown one when `None`); started when `launch`.
    pub fn open_scripts_in_editor(
        &mut self,
        name: Option<&str>,
        only: Option<&str>,
        launch: bool,
    ) -> Result<(PathBuf, u32, Vec<Launch>), String> {
        let (path, line) = self.script_working_copy(name)?;
        let launches = rules_edit::editor_launches_here_at(&path, Some((line, 1)), only);
        if launch {
            if launches.is_empty() {
                return Err(rules_edit::no_editor(only));
            }
            if let Some(w) = &mut self.scripts.external {
                w.launched = Some(rules_edit::launch(launches.clone()));
            }
            self.info(format!("Opening {} at line {line}", path.display()));
        }
        Ok((path, line, launches))
    }

    /// Once a frame, window or not (`ui::frame`, `Api::serve_once`): the
    /// outside editors' starts, the game type script's file both ways
    /// (`RulesEditor::poll_disk`, `RulesEditor::sync_out`), and the
    /// working copy ([`Self::check_working_copy`]).
    pub fn poll_external(&mut self, now: Instant) {
        if let Some(done) = self.rules.take_launched() {
            match done {
                Ok(how) => self.info(format!("Opened the script with {how}")),
                Err(why) => self.error(why),
            }
        }
        match self.rules.poll_disk(now) {
            DiskPoll::Nothing => {}
            DiskPoll::Took => self.rules_took_from_disk(),
            DiskPoll::Asked => self.info(
                "The script's file changed on disk and here: pick which to keep in the Game type script window.",
            ),
        }
        self.rules.sync_out(now);
        let launched = self
            .scripts
            .external
            .as_mut()
            .and_then(|w| w.launched.as_ref()?.lock().ok()?.take());
        if let Some(done) = launched {
            if let Some(w) = &mut self.scripts.external {
                w.launched = None;
            }
            match done {
                Ok(how) => self.info(format!("Opened the scripts with {how}")),
                Err(why) => self.error(why),
            }
        }
        self.check_working_copy(now, false);
    }

    /// The game type script took the file's text: say so, and reload it
    /// into the paired game while reload on save is on.
    pub fn rules_took_from_disk(&mut self) {
        let name = self.rules.title.clone();
        self.info(format!("{name}: took the changes saved on disk"));
        let paired = self.pairing.link.as_ref().is_some_and(|l| l.paired);
        if paired
            && self.rules.reload_on_save
            && let Err(why) = self.rules.queue_reload()
        {
            self.error(format!("Script not reloaded: {why}"));
        }
    }

    /// At most once a `WATCH_EVERY` (every time with `force`): the map's
    /// scripts read again when the scenario changed, the context and an
    /// unedited working copy written again, and a save of the working copy
    /// read and checked. Returns whether anything changed.
    pub fn check_working_copy(&mut self, now: Instant, force: bool) -> bool {
        let Some(mut wc) = self.scripts.external.take() else {
            return false;
        };
        if !force && !wc.watch.due(now) {
            self.scripts.external = Some(wc);
            return false;
        }
        self.sync_scripts();
        if !self.scripts_loaded().is_ok_and(|l| l.map == wc.map) {
            self.info(format!(
                "{}: no longer followed: its map is not open",
                wc.path.display()
            ));
            return true;
        }
        let l = self.scripts_loaded().expect("checked above");
        let mut changed = false;
        if self.scripts.for_rev != wc.for_rev {
            wc.for_rev = self.scripts.for_rev;
            wc.context = context(l, self.session.scenario());
            let dir = wc.path.parent().map(Path::to_path_buf).unwrap_or_default();
            let mut notes = Vec::new();
            if let Err(e) = write(&dir.join(ide::CONTEXT_FILE), &wc.context.to_json()) {
                notes.push(e);
            }
            let fresh = working_text(l);
            if fresh != wc.written {
                if !wc.written.is_empty() && wc.text == wc.written {
                    match write(&wc.path, &fresh).and_then(|()| write(&dir.join(BASE_FILE), &fresh)) {
                        Ok(()) => {
                            wc.written = fresh.clone();
                            wc.text = fresh;
                            wc.watch.seen = modified(&wc.path);
                        }
                        Err(e) => notes.push(e),
                    }
                } else if !wc.stale {
                    wc.stale = true;
                    notes.push(format!(
                        "the map's scripts changed while {} has edits: Studio did not write over them",
                        wc.path.display()
                    ));
                }
            }
            wc.check(l);
            changed = true;
            for n in notes {
                self.info(n);
            }
        }
        if wc.watch.changed(modified(&wc.path))
            && let Ok(disk) = std::fs::read_to_string(&wc.path)
            && disk != wc.text
        {
            wc.text = disk;
            wc.check(self.scripts_loaded().expect("checked above"));
            self.info(wc.summary());
            changed = true;
        }
        self.scripts.external = Some(wc);
        changed
    }
}

#[cfg(test)]
#[path = "external_tests.rs"]
mod tests;
