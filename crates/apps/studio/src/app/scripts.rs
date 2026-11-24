//! The Scripts tool (docs/editor/rich-editors.md, "Scripts"): the open
//! map's globals and scripts decompiled with the source's executable,
//! each name in the source a link, and the builtins' names. Compiling a
//! definition's source into the map is `app::script_edit`.
//!
//! The tables come from the map the scenario session has open
//! (`hs::Scenario::read`), the builtin and type tables from the source's
//! `default.xbe` (`editor_core::MapSource::executable`,
//! `hs::Catalog::from_xbe`), read when the map opens and never kept on
//! disk. Builtins no scenario calls take names only from a script
//! documentation listing the user points at (`hs::Naming`). "Open in VS
//! Code" and "Open in Zed" write the scripts as a working copy
//! (`app::external`).

use std::path::{Path, PathBuf};

use hs::{Catalog, Expression, GlobalIndex, Item, ItemSource, Literal, Naming, ScriptDoc};
use map::TagId;
use scenario_edit::BlockKind;
use scenario_edit::layout;

/// The tool's refusal without the executable.
pub const NO_EXECUTABLE: &str = "Scripts need the game's executable to name functions and types.";

/// What a link in the source names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LinkKind {
    Script,
    Global,
    /// A scenario object name: selects its placement.
    Object,
    /// A trigger volume, by name.
    Volume,
    /// A tag the scripts reference.
    Tag,
}

impl LinkKind {
    pub const ALL: [LinkKind; 5] = [
        LinkKind::Script,
        LinkKind::Global,
        LinkKind::Object,
        LinkKind::Volume,
        LinkKind::Tag,
    ];
    pub fn name(self) -> &'static str {
        match self {
            LinkKind::Script => "script",
            LinkKind::Global => "global",
            LinkKind::Object => "object",
            LinkKind::Volume => "volume",
            LinkKind::Tag => "tag",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.name() == s)
    }
}

/// A name in a definition's source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    /// Byte offsets into the source.
    pub start: usize,
    pub end: usize,
    pub kind: LinkKind,
    /// The name it is matched by: the script or global's name, the object
    /// or volume name, the tag's name as the script spells it.
    pub name: String,
    /// The script or global's index, or the tag's id.
    raw: u32,
}

/// Where a link goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Item(Item),
    Placement(BlockKind, usize),
    Volume(usize),
    Tag(TagId),
}

/// One global or script.
#[derive(Clone, Debug)]
pub struct Entry {
    pub source: ItemSource,
    pub links: Vec<Link>,
    /// The script kind (`startup`, `static`, ...) or `global`.
    pub kind: String,
    /// The return type, or the global's type.
    pub type_name: String,
    /// The node slots the definition's source shows.
    pub nodes: usize,
    /// The recordings its source names (values of type
    /// `cutscene_recording`), once per mention.
    pub recordings: Vec<super::recordings::Key>,
}

/// The read tables.
#[derive(Clone, Debug)]
pub struct Loaded {
    pub map: String,
    /// Where the executable was read.
    pub executable: String,
    pub tables: hs::Scenario,
    pub catalog: Catalog,
    pub naming: Naming,
    /// The documentation listing, when one is set: as read, or why not.
    pub doc: Option<(PathBuf, Result<ScriptDoc, String>)>,
    /// Globals, then scripts, in table order.
    pub entries: Vec<Entry>,
}

/// The tool's state: the read tables, or why there are none.
#[derive(Default)]
pub struct ScriptsTool {
    /// The session revision read (`None`: no map open).
    pub(crate) for_rev: Option<Option<u64>>,
    pub state: Option<Result<Loaded, String>>,
    /// The map's recordings, read with the tables (`app::recordings`).
    pub recordings: Option<Result<Vec<super::recordings::Row>, String>>,
    /// The recordings as edited, with their undo stack.
    pub recording_doc: Option<super::recordings::Doc>,
    pub selected: Option<Item>,
    /// The script documentation listing to name builtins from.
    pub doc_path: Option<PathBuf>,
    /// The working copy an outside editor has (`app::external`).
    pub external: Option<super::external::WorkingCopy>,
    /// The tables as the map holds them and the undo stack of compiles
    /// (`app::script_edit`); none when the scripts did not read.
    pub edit: Option<super::script_edit::Doc>,
    /// How the last compile ended, for the tab.
    pub compile: Option<super::script_edit::Outcome>,
    /// Compiles and undos run, so the tab can tell its own has finished.
    pub compiles: u64,
    /// The last expression sent to the paired game (`hs.eval`) and what
    /// it answered, as one line, or why it did not.
    pub evaluated: Option<(String, Result<String, String>)>,
}

/// A Scripts tool command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScriptAction {
    /// Show a global or a script.
    Select(Item),
    /// Follow link `link` of a definition's source.
    Follow { item: Item, link: usize },
    /// Name builtins from this listing, or (`None`) from scenarios only.
    SetDoc(Option<PathBuf>),
    /// Write the working copy and open it in `code`, `zed` or (`None`)
    /// any editor, at the shown definition.
    OpenInEditor(Option<&'static str>),
    /// Compile `source` into the map's tables (one undo step), or (not
    /// `apply`) only check it.
    Compile { source: String, apply: bool },
    /// Compile the definitions the working copy changed or added.
    CompileWorking,
    /// Undo (or redo) the last compile.
    CompileUndo { redo: bool },
    /// Play the recording `name` on `player`'s unit in the paired game
    /// and keep the path it takes (`recording.play`).
    PlayRecording { name: String, player: u16 },
    /// Evaluate an expression in the paired game (`hs.eval`); `ty` names
    /// the result's type where the expression's own is not fixed.
    Eval { expression: String, ty: Option<String> },
}

impl Loaded {
    /// Read the tables of `map` with the executable's bytes, or refuse.
    pub fn read<R: std::io::Read + std::io::Seek>(
        name: &str,
        map: &mut map::Map<R>,
        executable: Result<(String, Vec<u8>), String>,
        doc: Option<&Path>,
    ) -> Result<Self, String> {
        let (at, bytes) = executable.map_err(|e| format!("{NO_EXECUTABLE} {e}"))?;
        let xbe = xbe::Xbe::parse(&bytes)
            .map_err(|e| format!("{NO_EXECUTABLE} {at} is not an executable: {e}"))?;
        let mut catalog = Catalog::from_xbe(&xbe)
            .map_err(|e| format!("{NO_EXECUTABLE} {at} is not the supported one: {e}"))?;
        let tables =
            hs::Scenario::read(map).map_err(|e| format!("the scripts do not read: {e}"))?;
        catalog
            .observe(&tables)
            .map_err(|e| format!("the scripts do not check against {at}: {e}"))?;
        let each = tables
            .decompile_each(&catalog)
            .map_err(|e| format!("the scripts do not decompile: {e}"))?;
        let entries = each
            .into_iter()
            .map(|source| entry(&tables, &catalog, source))
            .collect::<Result<_, String>>()?;
        let mut loaded = Self {
            map: name.to_owned(),
            executable: at,
            naming: Naming::new(&catalog, None),
            catalog,
            tables,
            doc: None,
            entries,
        };
        loaded.set_doc(doc);
        Ok(loaded)
    }

    /// Name the builtins again, from this listing or from scenarios only.
    pub fn set_doc(&mut self, path: Option<&Path>) {
        self.doc = path.map(|p| (p.to_owned(), read_doc(p)));
        let doc = self.doc.as_ref().and_then(|(_, d)| d.as_ref().ok());
        self.naming = Naming::new(&self.catalog, doc);
    }

    pub fn entry(&self, item: Item) -> Option<&Entry> {
        self.entries.iter().find(|e| e.source.item == item)
    }

    /// A script, else a global, by name.
    pub fn find(&self, name: &str) -> Option<Item> {
        let by = |want: fn(Item) -> bool| {
            self.entries
                .iter()
                .find(|e| want(e.source.item) && e.source.name == name)
                .map(|e| e.source.item)
        };
        by(|i| matches!(i, Item::Script(_))).or_else(|| by(|i| matches!(i, Item::Global(_))))
    }

    /// Definitions whose name or source contains `filter` (any case).
    pub fn matching(&self, filter: &str) -> impl Iterator<Item = &Entry> {
        let want = filter.to_lowercase();
        self.entries.iter().filter(move |e| {
            want.is_empty()
                || e.source.name.to_lowercase().contains(&want)
                || e.source.source.to_lowercase().contains(&want)
        })
    }

    /// Node slots no definition uses.
    pub fn free_nodes(&self) -> usize {
        self.tables.nodes().iter().filter(|n| !n.live()).count()
    }

    /// Where `link` goes in the scenario as it is now, or why nowhere.
    pub fn resolve(
        &self,
        link: &Link,
        scenario: Option<&scenario_edit::Scenario>,
    ) -> Result<Target, String> {
        match link.kind {
            LinkKind::Script => Ok(Target::Item(Item::Script(link.raw as usize))),
            LinkKind::Global => Ok(Target::Item(Item::Global(link.raw as usize))),
            LinkKind::Tag => Ok(Target::Tag(TagId(link.raw))),
            LinkKind::Object => {
                let s = scenario.ok_or("no scenario is open")?;
                object(s, &link.name)
            }
            LinkKind::Volume => {
                let s = scenario.ok_or("no scenario is open")?;
                let mut found = s
                    .volumes
                    .triggers
                    .iter()
                    .enumerate()
                    .filter(|(_, v)| v.name.as_deref() == Some(link.name.as_str()));
                match (found.next(), found.next()) {
                    (Some((i, _)), None) => Ok(Target::Volume(i)),
                    (None, _) => Err(format!("no trigger volume is named {:?}", link.name)),
                    (Some(_), Some(_)) => Err(format!(
                        "more than one trigger volume is named {:?}",
                        link.name
                    )),
                }
            }
        }
    }

    /// The definitions that name recording `index` of `rows` (by its name
    /// or its number), with how many times.
    pub fn recording_uses(
        &self,
        rows: &[super::recordings::Row],
        index: usize,
    ) -> Vec<(Item, &str, usize)> {
        let Some(row) = rows.get(index) else {
            return Vec::new();
        };
        self.entries
            .iter()
            .filter_map(|e| {
                let n = e.recordings.iter().filter(|k| row.named_by(index, k)).count();
                (n > 0).then_some((e.source.item, e.source.name.as_str(), n))
            })
            .collect()
    }

    /// The definitions that name `name` as a `kind`, with how many times.
    pub fn uses(&self, kind: LinkKind, name: &str) -> Vec<(Item, &str, usize)> {
        self.entries
            .iter()
            .filter_map(|e| {
                let n = e
                    .links
                    .iter()
                    .filter(|l| l.kind == kind && (l.name == name || tag_id_matches(l, name)))
                    .count();
                (n > 0).then_some((e.source.item, e.source.name.as_str(), n))
            })
            .collect()
    }
}

fn tag_id_matches(link: &Link, name: &str) -> bool {
    link.kind == LinkKind::Tag && name == format!("{:#010x}", link.raw)
}

fn read_doc(path: &Path) -> Result<ScriptDoc, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    ScriptDoc::parse(&String::from_utf8_lossy(&bytes))
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// The placement object name `name` names, through the scenario's object
/// names block by its text.
fn object(s: &scenario_edit::Scenario, name: &str) -> Result<Target, String> {
    let mut found = (0..s.names.len()).filter(|&i| s.names.name(i) == Some(name));
    let i = match (found.next(), found.next()) {
        (Some(i), None) => i,
        (None, _) => return Err(format!("no object is named {name:?}")),
        (Some(_), Some(_)) => return Err(format!("more than one object is named {name:?}")),
    };
    let (ty, placement) = s
        .names
        .target(i)
        .ok_or_else(|| format!("object name {name:?} does not read"))?;
    let index = usize::try_from(placement)
        .map_err(|_| format!("object name {name:?} names no placement"))?;
    let kind = layout::BLOCKS
        .iter()
        .find(|b| b.object_type == Some(ty))
        .map(|b| b.kind)
        .ok_or_else(|| {
            format!("object name {name:?} names object type {ty}, which no placement block holds")
        })?;
    if index >= s.block(kind).len() {
        return Err(format!(
            "object name {name:?} names {} {index}, past the block's {}",
            s.block(kind).def.label,
            s.block(kind).len()
        ));
    }
    Ok(Target::Placement(kind, index))
}

pub(super) fn entry(tables: &hs::Scenario, catalog: &Catalog, source: ItemSource) -> Result<Entry, String> {
    let fail = |e: hs::Error| format!("{}: {e}", source.name);
    let (kind, type_name) = match source.item {
        Item::Global(i) => {
            let g = &tables.globals()[i];
            (
                "global".to_owned(),
                catalog.type_name(g.value_type()).map_err(fail)?.to_owned(),
            )
        }
        Item::Script(i) => {
            let s = &tables.scripts()[i];
            let kind = catalog
                .script_kinds()
                .get(s.kind() as usize)
                .cloned()
                .ok_or_else(|| format!("{}: unknown script kind {}", source.name, s.kind()))?;
            (
                kind,
                catalog.type_name(s.return_type()).map_err(fail)?.to_owned(),
            )
        }
    };
    let mut slots = std::collections::BTreeSet::new();
    let mut links = Vec::new();
    let mut recordings = Vec::new();
    for t in &source.tokens {
        slots.insert(t.datum);
        if t.head {
            slots.insert(hs::Datum(tables.node(t.datum).map_err(fail)?.data()));
        }
        if !t.head
            && let Some(k) = recording_key(tables, catalog, t.datum).map_err(fail)?
        {
            recordings.push(k);
        }
        if let Some((kind, name, raw)) = classify(tables, catalog, t.datum, t.head).map_err(fail)? {
            links.push(Link {
                start: t.start,
                end: t.end,
                kind,
                name,
                raw,
            });
        }
    }
    Ok(Entry {
        nodes: slots.len(),
        links,
        recordings,
        kind,
        type_name,
        source,
    })
}

/// The recording an atom of type `cutscene_recording` names.
fn recording_key(
    tables: &hs::Scenario,
    catalog: &Catalog,
    datum: hs::Datum,
) -> hs::Result<Option<super::recordings::Key>> {
    use super::recordings::Key;
    let Expression::Atom {
        source_type,
        literal,
        ..
    } = tables.expression(datum)?
    else {
        return Ok(None);
    };
    if catalog.type_name(source_type)? != "cutscene_recording" {
        return Ok(None);
    }
    Ok(match literal {
        Literal::Symbol { spelling, .. } => Some(Key::Name(spelling.to_owned())),
        Literal::Short(n) => Some(Key::Index(n.into())),
        Literal::Long(n) => Some(Key::Index(n.into())),
        _ => None,
    })
}

/// What a node names, if it names something the tool links to.
fn classify(
    tables: &hs::Scenario,
    catalog: &Catalog,
    datum: hs::Datum,
    head: bool,
) -> hs::Result<Option<(LinkKind, String, u32)>> {
    use hs::casts::types::{OBJECT_NAME, SCENERY_NAME};
    Ok(match tables.expression(datum)? {
        Expression::ScriptCall { index, script, .. } if head => Some((
            LinkKind::Script,
            script.name()?.to_owned(),
            u32::from(index),
        )),
        Expression::Global {
            index: GlobalIndex::Scenario(i),
            name,
            ..
        } => Some((LinkKind::Global, name.to_owned(), u32::from(i))),
        Expression::Atom {
            source_type,
            literal: Literal::Symbol { bits, spelling, .. },
            ..
        } => {
            if (OBJECT_NAME..=SCENERY_NAME).contains(&source_type) {
                Some((LinkKind::Object, spelling.to_owned(), 0))
            } else if catalog.type_name(source_type)? == "trigger_volume" {
                Some((LinkKind::Volume, spelling.to_owned(), 0))
            } else if tables.references().iter().any(|r| r.id().0 == bits) {
                Some((LinkKind::Tag, spelling.to_owned(), bits))
            } else {
                None
            }
        }
        _ => None,
    })
}

impl super::App {
    /// Read the open map's scripts when the session opened another map
    /// since (or none is open now).
    pub fn sync_scripts(&mut self) {
        let rev = self.session.is_open().then(|| self.session.world_rev());
        if self.scripts.for_rev == Some(rev) {
            return;
        }
        self.scripts.for_rev = Some(rev);
        self.scripts.selected = None;
        self.scripts.state = rev.map(|_| self.read_scripts());
        self.scripts.compile = None;
        self.scripts.edit = match &self.scripts.state {
            Some(Ok(l)) => self.read_script_header().ok().map(|(base, header)| {
                super::script_edit::Doc::new(l.tables.clone(), base, header)
            }),
            _ => None,
        };
        (self.scripts.recordings, self.scripts.recording_doc) = match rev.map(|_| self.read_recordings()) {
            None => (None, None),
            Some(Ok((rows, doc))) => (Some(Ok(rows)), Some(doc)),
            Some(Err(e)) => (Some(Err(e)), None),
        };
        match &self.scripts.state {
            Some(Err(e)) => {
                let e = format!("scripts: {e}");
                self.error(e);
            }
            Some(Ok(l)) => self.scripts.selected = l.entries.first().map(|e| e.source.item),
            None => {}
        }
    }

    fn read_scripts(&self) -> Result<Loaded, String> {
        let name = self.session.map_name().unwrap_or("map").to_owned();
        let source = self.session.source().ok_or("no map is open")?;
        let mut map = source.open().map_err(|e| e.to_string())?;
        let executable = match self.source() {
            Some(s) => s.executable().map_err(|e| format!("{e:#}")),
            None => Err("no disc image or folder of maps is open".to_owned()),
        };
        Loaded::read(
            &name,
            &mut map,
            executable,
            self.scripts.doc_path.as_deref(),
        )
    }

    /// The scenario payload's address and table header, from the source.
    fn read_script_header(&self) -> Result<(u32, Vec<u8>), String> {
        let source = self.session.source().ok_or("no map is open")?;
        let mut map = source.open().map_err(|e| e.to_string())?;
        hs::Scenario::read_header(&mut map).map_err(|e| e.to_string())
    }

    /// The read tables, or why there are none.
    pub fn scripts_loaded(&self) -> Result<&Loaded, String> {
        match &self.scripts.state {
            None => Err("open a map to see its scripts".to_owned()),
            Some(Err(e)) => Err(e.clone()),
            Some(Ok(l)) => Ok(l),
        }
    }

    pub(crate) fn run_script(&mut self, action: ScriptAction) {
        self.sync_scripts();
        match action {
            ScriptAction::SetDoc(path) => {
                self.scripts.doc_path = path.clone();
                let Some(Ok(l)) = &mut self.scripts.state else {
                    return match path {
                        Some(p) => self.info(format!(
                            "script documentation: {} (read when the scripts read)",
                            p.display()
                        )),
                        None => self.info("script documentation: none"),
                    };
                };
                l.set_doc(path.as_deref());
                let line = match (&l.doc, &l.naming.report) {
                    (None, _) => Ok("builtins named from scenarios only".to_owned()),
                    (Some((_, Err(e))), _) => Err(e.clone()),
                    (Some((p, Ok(_))), Some(r)) => Ok(format!(
                        "{}: {} entries, {} builtins named by signature",
                        p.display(),
                        r.functions,
                        r.named
                    )),
                    (Some((p, Ok(_))), None) => Ok(p.display().to_string()),
                };
                match line {
                    Ok(m) => self.info(format!("script documentation: {m}")),
                    Err(e) => self.error(format!("script documentation: {e}")),
                }
            }
            ScriptAction::Compile { source, apply } => self.run_script_compile(&source, apply),
            ScriptAction::CompileWorking => {
                self.check_working_copy(std::time::Instant::now(), true);
                match self.working_edits() {
                    Ok(source) => self.run_script_compile(&source, true),
                    Err(why) => self.error(format!("script not compiled: {why}")),
                }
            }
            ScriptAction::CompileUndo { redo } => match self.script_step(redo) {
                Ok(true) => self.info(if redo { "compile redone" } else { "compile undone" }),
                Ok(false) => self.info("no compile to undo or redo"),
                Err(e) => self.error(format!("compile not undone: {}", e.line())),
            },
            ScriptAction::PlayRecording { name, player } => {
                match crate::api::recordings::play_on(self, &name, player, None) {
                    Ok(_) => self.info(format!("recording {name}: playing on player {player}")),
                    Err(e) => self.error(format!("recording {name} not played: {}", e.message)),
                }
            }
            ScriptAction::Eval { expression, ty } => {
                let answer = crate::api::scripts::eval_on(self, &expression, ty.as_deref());
                let line = match answer {
                    Ok(v) if v["ok"] == true => Ok(format!(
                        "{} {}",
                        v["type"].as_str().unwrap_or("?"),
                        if v["value"].is_null() { "void".to_owned() } else { v["value"].to_string() }
                    )),
                    Ok(v) => Err(v["error"].as_str().unwrap_or("not evaluated").to_owned()),
                    Err(e) => Err(e.message),
                };
                self.scripts.evaluated = Some((expression, line));
            }
            ScriptAction::OpenInEditor(only) => {
                if let Err(why) = self.open_scripts_in_editor(None, only, true) {
                    self.error(format!("Scripts not opened: {why}"));
                }
            }
            ScriptAction::Select(item) => match self.scripts_loaded() {
                Ok(l) if l.entry(item).is_some() => self.scripts.selected = Some(item),
                Ok(_) => self.error(format!("no {item:?} in the scripts")),
                Err(e) => self.error(e),
            },
            ScriptAction::Follow { item, link } => {
                let target = self.scripts_loaded().and_then(|l| {
                    let e = l
                        .entry(item)
                        .ok_or_else(|| format!("no {item:?} in the scripts"))?;
                    let k = e.links.get(link).ok_or_else(|| {
                        format!(
                            "{} has {} links, not {}",
                            e.source.name,
                            e.links.len(),
                            link + 1
                        )
                    })?;
                    l.resolve(k, self.session.scenario())
                        .map(|t| (t, format!("{} {}", k.kind.name(), k.name)))
                        .map_err(|why| format!("{} {}: {why}", k.kind.name(), k.name))
                });
                match target {
                    Err(e) => self.error(e),
                    Ok((target, what)) => {
                        self.info(format!("followed {what}"));
                        self.follow(target);
                    }
                }
            }
        }
    }

    fn follow(&mut self, target: Target) {
        use super::{Action, Selection, TagAction, ToolKind};
        match target {
            Target::Item(item) => self.scripts.selected = Some(item),
            Target::Placement(kind, index) => {
                self.run(Action::GoTo(Selection::Placement(kind, index)));
            }
            Target::Volume(index) => {
                self.run(Action::SelectVolume(Some(index)));
                self.set_tool(ToolKind::Scenario);
            }
            Target::Tag(id) => {
                self.set_tool(ToolKind::Tags);
                self.run(Action::Tag(TagAction::Select(id)));
            }
        }
    }
}

#[cfg(test)]
#[path = "scripts_tests.rs"]
pub(crate) mod tests;
