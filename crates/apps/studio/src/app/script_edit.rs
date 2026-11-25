//! Scripts, write (docs/editor/rich-editors.md, "Scripts", slice H3): the
//! Scripts tool's source compiled into the open map's tables.
//!
//! [`App::script_compile`] hands source to `hs::compile` with the tool's
//! catalog and builtin names, and the tables it returns to
//! `hs::table_writes::writes`, which says how a save writes each one that
//! differs from the map's: in place, one patch per changed run, or (a
//! table that outgrew the map's) a copy appended on save. The session
//! holds those writes with the scenario's own
//! ([`scenario_edit::Session::set_script_changes`]), so they save with the
//! map, count as unsaved and need **Allow growing full blocks** when a
//! table grows. The tool's tables become the compiler's, so the source,
//! the free node count and the links show the edit.
//!
//! Nothing is applied unless every definition compiles, the new tables
//! pass `Scenario::validate`, `Scenario::validate_graph` and the codec's
//! symmetry (`Scenario::verify_written`), and the session takes the
//! writes. The undo stack is the [`Doc`]'s: the tables before each
//! compile.

use hs::table_writes::{self, Writes};
use hs::{Item, Message, Options, Scenario};

use super::scripts::Loaded;

/// The map's tables as opened and the header they were read from, with
/// the tables before each compile.
#[derive(Clone, Debug)]
pub struct Doc {
    /// The tables as the map holds them: what the writes are diffed from.
    pub source: Scenario,
    /// The scenario payload's guest address.
    pub base: u32,
    /// The first `0x240` bytes of the payload.
    pub header: Vec<u8>,
    undo: Vec<Scenario>,
    redo: Vec<Scenario>,
}

impl Doc {
    pub fn new(source: Scenario, base: u32, header: Vec<u8>) -> Self {
        Self {
            source,
            base,
            header,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    /// Compiles that can be undone, and undone ones that can be redone.
    pub fn depth(&self) -> (usize, usize) {
        (self.undo.len(), self.redo.len())
    }
}

/// What a compile did, or would do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    /// Each definition of the source: its name and which table.
    pub definitions: Vec<(String, Item)>,
    /// Nodes the compiled definitions use.
    pub nodes: usize,
    /// Slots freed from the replaced definitions.
    pub freed: usize,
    /// Free slots the new nodes took.
    pub reused: usize,
    /// Slots added to the node table.
    pub grown: usize,
    /// Bytes added to the string data.
    pub new_strings: usize,
    /// Whether a table outgrows the map's, which needs **Allow growing
    /// full blocks**: more nodes than free slots, a new string or a new
    /// script or global.
    pub grows: bool,
    /// Node slots no definition uses, after the compile.
    pub free_nodes: usize,
    /// Whether the session holds the writes.
    pub applied: bool,
}

/// Why nothing was compiled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The source has problems, each with its place.
    Problems(Vec<Message>),
    /// The request is wrong: the named definition is not in the source.
    Params(String),
    /// The request is fine and the editor will not: no map, no
    /// executable, growth off, tables that do not check.
    Refused(String),
}

impl Refusal {
    fn refused(why: impl std::fmt::Display) -> Self {
        Refusal::Refused(why.to_string())
    }

    /// One line for the log.
    pub fn line(&self) -> String {
        match self {
            Refusal::Problems(m) => format!(
                "{} problem{}: {}",
                m.len(),
                if m.len() == 1 { "" } else { "s" },
                m.first().map(ToString::to_string).unwrap_or_default()
            ),
            Refusal::Params(w) | Refusal::Refused(w) => w.clone(),
        }
    }
}

/// How the last compile of the tool ended, for the tab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Done(Report),
    Failed(Refusal),
}

impl Loaded {
    /// This read with `tables` in place of the map's: decompiled again,
    /// the builtin names learned from them, the links read again.
    pub fn rebuilt(&self, tables: Scenario) -> Result<Loaded, String> {
        let mut next = self.clone();
        next.catalog
            .observe(&tables)
            .map_err(|e| format!("the compiled scripts do not check against {}: {e}", self.executable))?;
        let each = tables
            .decompile_each(&next.catalog)
            .map_err(|e| format!("the compiled scripts do not decompile: {e}"))?;
        next.entries = each
            .into_iter()
            .map(|source| super::scripts::entry(&tables, &next.catalog, source))
            .collect::<Result<_, String>>()?;
        let doc = next.doc.as_ref().and_then(|(_, d)| d.as_ref().ok());
        next.naming = hs::Naming::new(&next.catalog, doc);
        next.tables = tables;
        Ok(next)
    }
}

fn grown(w: &Writes) -> Vec<scenario_edit::TagGrown> {
    w.grown
        .iter()
        .map(|g| scenario_edit::TagGrown {
            root: g.root,
            count: g.count,
            bytes: g.bytes.clone(),
        })
        .collect()
}

impl super::App {
    /// Compile `source` into the map's script tables. Without `apply`
    /// nothing changes: the report says what would. With it, the compile
    /// is one undo step and the tables, the tool and the unsaved count
    /// follow. `only` names a definition the source must hold.
    pub fn script_compile(
        &mut self,
        source: &str,
        only: Option<&str>,
        apply: bool,
    ) -> Result<Report, Refusal> {
        self.sync_scripts();
        let (compiled, writes, next) = {
            let l = self.scripts_loaded().map_err(Refusal::Refused)?;
            let doc = self
                .scripts
                .edit
                .as_ref()
                .ok_or_else(|| Refusal::refused("open a map to compile into it"))?;
            let options = Options {
                naming: Some(&l.naming),
                ..Options::default()
            };
            let compiled =
                hs::compile(&l.tables, &l.catalog, source, &options).map_err(Refusal::Problems)?;
            if let Some(name) = only
                && !compiled
                    .items
                    .iter()
                    .any(|&i| item_name(&compiled.scenario, i).as_deref() == Some(name))
            {
                return Err(Refusal::Params(format!(
                    "name: the source defines no script or global named {name:?}"
                )));
            }
            let writes =
                table_writes::writes(&doc.header, doc.base, &doc.source, &compiled.scenario)
                    .map_err(Refusal::refused)?;
            let next = if apply {
                Some(l.rebuilt(compiled.scenario.clone()).map_err(Refusal::Refused)?)
            } else {
                None
            };
            (compiled, writes, next)
        };
        let report = Report {
            definitions: compiled
                .items
                .iter()
                .filter_map(|&i| item_name(&compiled.scenario, i).map(|n| (n, i)))
                .collect(),
            nodes: compiled.nodes,
            freed: compiled.freed,
            reused: compiled.reused,
            grown: compiled.grown,
            new_strings: compiled.new_strings,
            grows: !writes.grown.is_empty(),
            free_nodes: compiled.scenario.nodes().iter().filter(|n| !n.live()).count(),
            applied: apply,
        };
        let Some(next) = next else {
            return Ok(report);
        };
        let copies = grown(&writes);
        self.session
            .set_script_changes(writes.patches, copies)
            .map_err(Refusal::refused)?;
        let doc = self.scripts.edit.as_mut().expect("read above");
        let Some(Ok(l)) = &mut self.scripts.state else {
            unreachable!("read above");
        };
        doc.undo.push(std::mem::replace(l, next).tables);
        doc.redo.clear();
        self.working_copy_follows();
        Ok(report)
    }

    /// Undo (or, with `redo`, redo) the last compile; `false` when there
    /// is none.
    pub fn script_step(&mut self, redo: bool) -> Result<bool, Refusal> {
        self.sync_scripts();
        let doc = self
            .scripts
            .edit
            .as_mut()
            .ok_or_else(|| Refusal::refused("open a map to compile into it"))?;
        let Some(target) = (if redo { doc.redo.pop() } else { doc.undo.pop() }) else {
            return Ok(false);
        };
        let put_back = |doc: &mut Doc, target: Scenario| {
            let stack = if redo { &mut doc.redo } else { &mut doc.undo };
            stack.push(target);
        };
        let l = match &self.scripts.state {
            Some(Ok(l)) => l,
            _ => {
                put_back(doc, target);
                return Err(Refusal::refused("the scripts are not read"));
            }
        };
        let tried = table_writes::writes(&doc.header, doc.base, &doc.source, &target)
            .map_err(Refusal::refused)
            .and_then(|w| l.rebuilt(target.clone()).map_err(Refusal::Refused).map(|n| (w, n)));
        let (writes, next) = match tried {
            Ok(x) => x,
            Err(e) => {
                put_back(doc, target);
                return Err(e);
            }
        };
        let copies = grown(&writes);
        if let Err(e) = self.session.set_script_changes(writes.patches, copies) {
            put_back(doc, target);
            return Err(Refusal::refused(e));
        }
        let Some(Ok(l)) = &mut self.scripts.state else {
            unreachable!("read above");
        };
        let now = std::mem::replace(l, next).tables;
        let stack = if redo { &mut doc.undo } else { &mut doc.redo };
        stack.push(now);
        self.working_copy_follows();
        Ok(true)
    }

    /// The working copy's differences from the map, again: the map's
    /// scripts are now the compiler's.
    fn working_copy_follows(&mut self) {
        if let (Some(Ok(l)), Some(wc)) = (&self.scripts.state, &mut self.scripts.external) {
            wc.check(l);
        }
    }

    /// The working copy's definitions that differ from the map's (changed
    /// and added), as source for [`Self::script_compile`]: the ones it
    /// holds unchanged are left out, so their nodes are not touched.
    pub fn working_edits(&self) -> Result<String, String> {
        let wc = self
            .scripts
            .external
            .as_ref()
            .ok_or("no working copy is open (script.external writes it)")?;
        let wanted: Vec<&String> = wc.changes.changed.iter().chain(&wc.changes.added).collect();
        let text: Vec<String> = hs::ide::definitions(&wc.text, &wc.context)
            .into_iter()
            .filter(|d| wanted.contains(&&d.name))
            .map(|d| d.text)
            .collect();
        if text.is_empty() {
            return Err("the working copy holds no edits".to_owned());
        }
        Ok(text.join("\n"))
    }

    /// The tool's command: compile `source` (and, `apply`, write it).
    pub(crate) fn run_script_compile(&mut self, source: &str, apply: bool) {
        self.scripts.compiles += 1;
        let result = self.script_compile(source, None, apply);
        match &result {
            Ok(r) => {
                let names: Vec<_> = r.definitions.iter().map(|(n, _)| n.as_str()).collect();
                self.info(format!(
                    "compiled {}: {} nodes, {} new string bytes{}",
                    names.join(", "),
                    r.nodes,
                    r.new_strings,
                    if r.grows { ", a table grows" } else { "" }
                ));
            }
            Err(e) => self.error(format!("script not compiled: {}", e.line())),
        }
        self.scripts.compile = Some(match result {
            Ok(r) => Outcome::Done(r),
            Err(e) => Outcome::Failed(e),
        });
    }
}

fn item_name(s: &Scenario, item: Item) -> Option<String> {
    match item {
        Item::Script(i) => s.scripts().get(i)?.name().ok().map(str::to_owned),
        Item::Global(i) => s.globals().get(i)?.name().ok().map(str::to_owned),
    }
}

#[cfg(test)]
#[path = "script_edit_tests.rs"]
mod tests;
