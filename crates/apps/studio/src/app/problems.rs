//! Problems (docs/editor/studio-redesign.md, "The shell"): one list over
//! every check and build error, read by the Problems panel
//! (`ui/problems.rs`) and `problems.list` (`api/problems.rs`).
//!
//! A problem has a **source**: `check` (`check.run`'s models and sounds),
//! `ui` (`ui.check`), `hud` (`hud.check` and the HUD kit's), `package`
//! (the manifest checks `h2 package check` runs over the packages under
//! `<data>/packages/`, a file that holds bytes of the user's own store, and
//! the conflict rows of the last `package.diff` of each level: a package
//! off for it, a field a later package takes) and `build` (what the game's package set refuses
//! to load, as the boot log says it). Each row names what it opens: a tag
//! (`asset`, with the `field` the finding names), a package or one of its
//! items, or the tool that holds the screen or the HUD.
//!
//! [`Action::OpenProblem`] opens a row, as the panel's double-click does:
//! the document in front and, for a tag, the field the finding names.
//!
//! [`Action::OpenProblem`]: super::Action::OpenProblem

use serde_json::{Value, json};

use super::assets::{AssetId, read_set};
use super::mods::Conflict;
use super::navigation::NavAction;
use super::{Action, App, Doc, ToolKind, rules_edit, tag_api};
use crate::api::checks::{CHECKS, ContentCheck, Finding, Severity};

/// Every source, in the order the panel lists them.
pub const SOURCES: [&str; 6] = ["check", "ui", "hud", "package", "build", "ground"];

/// The source a check's findings belong to: the content checks' areas
/// `ui` and `hud` are their own sources, `models` and `sounds` are `check`.
pub fn source_of(area: &str) -> &'static str {
    match area {
        "ui" => "ui",
        "hud" => "hud",
        "package" => "package",
        "build" => "build",
        "ground" => "ground",
        _ => "check",
    }
}

/// A source's name in the panel.
pub fn source_label(source: &str) -> &'static str {
    match source {
        "check" => "Checks",
        "ui" => "UI",
        "hud" => "HUD",
        "package" => "Packages",
        "ground" => "AI ground",
        _ => "Build",
    }
}

/// One row of the list.
#[derive(Clone, Debug, PartialEq)]
pub struct Problem {
    pub source: &'static str,
    /// The check's area (`models`, `sounds`, `ui`, `hud`), `package` or `build`.
    pub area: String,
    pub code: String,
    pub severity: Severity,
    pub message: String,
    /// What it is about: a tag, a clip, `screen/state`, a HUD, a package.
    pub target: String,
    /// The file that holds it, when there is one.
    pub path: Option<String>,
    /// What opens: `tag:<group>:<path>`, `package:<id>` or `item:<id>:<n>`.
    pub asset: Option<String>,
    /// The tool that holds it, when it is not an asset.
    pub tool: Option<ToolKind>,
    /// The field of the tag the finding names.
    pub field: Option<String>,
    /// The line and column (1-based) a script finding is at.
    pub at: Option<(u32, u32)>,
}

impl Problem {
    pub fn json(&self) -> Value {
        let mut v = json!({
            "source": self.source,
            "area": self.area,
            "code": self.code,
            "severity": self.severity.name(),
            "message": self.message,
            "target": self.target,
        });
        for (key, value) in [
            ("path", self.path.clone()),
            ("asset", self.asset.clone()),
            ("field", self.field.clone()),
            ("tool", self.tool.map(|t| crate::api::tool_name(t).to_owned())),
        ] {
            if let Some(s) = value {
                v[key] = json!(s);
            }
        }
        if let Some((line, col)) = self.at {
            v["line"] = json!(line);
            v["col"] = json!(col);
        }
        v
    }
}

/// The list as last read, and what ran.
#[derive(Clone, Debug, Default)]
pub struct Problems {
    pub rows: Vec<Problem>,
    /// `{source, check, findings}` per check that ran.
    pub ran: Vec<Value>,
    /// `{source, check?, why}` per check that could not run.
    pub skipped: Vec<Value>,
    /// Whether the list has been read at all.
    pub checked: bool,
    /// A package or item the browser is to show, set by opening a row.
    pub reveal: Option<String>,
    /// The conflict table's rows from the last build of each level
    /// (`package.diff` with a map): a package off for a level, and a field
    /// or an asset a later package takes. They list under `package`, and a
    /// refresh keeps them: only a later diff of the level replaces them.
    pub levels: Vec<Conflict>,
}

impl Problems {
    /// The rows a build of `map` gave, in place of the last ones for it.
    pub fn set_levels(&mut self, map: &str, rows: Vec<Conflict>) {
        self.levels.retain(|c| !c.map.eq_ignore_ascii_case(map));
        self.levels.extend(rows);
    }

    /// Take a fresh read of `sources` in place of what the list held for them.
    pub fn merge(&mut self, fresh: Problems, sources: &[&str]) {
        let keep = |v: &Value| !sources.iter().any(|s| v["source"] == *s);
        self.rows.retain(|p| !sources.contains(&p.source));
        self.ran.retain(keep);
        self.skipped.retain(keep);
        self.rows.extend(fresh.rows);
        self.ran.extend(fresh.ran);
        self.skipped.extend(fresh.skipped);
        self.checked = true;
        self.sort();
    }

    /// Worst first, then by source and target.
    fn sort(&mut self) {
        let order = |s: &str| SOURCES.iter().position(|x| *x == s);
        self.rows.sort_by(|a, b| {
            b.severity
                .cmp(&a.severity)
                .then_with(|| order(a.source).cmp(&order(b.source)))
                .then_with(|| a.target.cmp(&b.target))
        });
    }

    /// The rows of `source` (every one when `None`).
    pub fn of<'a>(&'a self, source: Option<&'a str>) -> impl Iterator<Item = &'a Problem> {
        self.rows
            .iter()
            .filter(move |p| source.is_none_or(|s| p.source == s))
    }

    /// The rows counted by severity and source.
    pub fn counts(&self, source: Option<&str>) -> Value {
        let n = |s: Severity| self.of(source).filter(|p| p.severity == s).count();
        let by_source: serde_json::Map<String, Value> = SOURCES
            .iter()
            .map(|s| ((*s).to_owned(), json!(self.of(Some(*s)).count())))
            .collect();
        json!({
            "error": n(Severity::Error),
            "warning": n(Severity::Warning),
            "note": n(Severity::Note),
            "total": self.of(source).count(),
            "by_source": by_source,
        })
    }

    /// What `problems.list` answers.
    pub fn json(&self, source: Option<&str>) -> Value {
        let keep = |v: &&Value| source.is_none_or(|s| v["source"] == s);
        json!({
            "problems": self.of(source).map(Problem::json).collect::<Vec<_>>(),
            "counts": self.counts(source),
            "ran": self.ran.iter().filter(keep).collect::<Vec<_>>(),
            "skipped": self.skipped.iter().filter(keep).collect::<Vec<_>>(),
        })
    }
}

/// The field a model finding names in its message, `(tag.set path <p>)`.
fn field_of(message: &str) -> Option<String> {
    let rest = message.split("(tag.set path ").nth(1)?;
    let path = rest.split(')').next()?.trim();
    (!path.is_empty()).then(|| path.to_owned())
}

fn from_finding(app: &App, source: &'static str, f: &Finding) -> Problem {
    // A finding about a tag of the open map names it `group:path`.
    let asset = (source == "check"
        && tag_api::find_tag(&app.tags, &f.target).is_some())
    .then(|| format!("tag:{}", f.target));
    Problem {
        source,
        area: f.area.to_owned(),
        code: f.code.clone(),
        severity: f.severity,
        message: f.message.clone(),
        target: f.target.clone(),
        path: f.path.clone(),
        field: asset.as_ref().and_then(|_| field_of(&f.message)),
        at: None,
        asset,
        tool: match source {
            "ui" => Some(ToolKind::UiScreens),
            "hud" => Some(ToolKind::Hud),
            _ => None,
        },
    }
}

/// A refusal or an override of one level, as a row of `package`: a package
/// off for the level is a warning, a field a later package takes a note.
fn from_conflict(c: &Conflict) -> Problem {
    let refused = c.kind == "refused";
    Problem {
        source: "package",
        area: "package".to_owned(),
        code: format!("package.{}", c.kind),
        severity: if refused {
            Severity::Warning
        } else {
            Severity::Note
        },
        message: if refused {
            format!("{c_package} is off for {}: {}", c.map, c.why, c_package = c.package)
        } else {
            format!("{}: {}", c.map, c.why)
        },
        target: format!("{}: {}", c.map, c.package),
        path: None,
        asset: Some(format!("package:{}", c.package)),
        tool: None,
        field: c.path.clone(),
        at: None,
    }
}

fn from_package(f: &::workspace::Finding) -> Problem {
    let (code, target, asset) = match f.item {
        Some(n) => (
            "package.item",
            format!("{} item {n}", f.package),
            format!("item:{}:{n}", f.package),
        ),
        None => (
            "package.set",
            f.package.clone(),
            format!("package:{}", f.package),
        ),
    };
    Problem {
        source: "package",
        area: "package".to_owned(),
        code: code.to_owned(),
        severity: Severity::Error,
        message: f.what.clone(),
        target,
        path: f.file.clone(),
        asset: Some(asset),
        tool: None,
        field: None,
        at: None,
    }
}

/// The open script's diagnostics as rows of `check`, area `scripts`: each
/// carries its line and column, and opens at them ([`App::open_problem`]).
fn script_rows(app: &mut App) -> Vec<Problem> {
    app.rules.refresh();
    let asset = app.doc_asset(Doc::Rules);
    let title = app.rules.title.clone();
    rules_edit::problems(&app.rules.text, &app.rules.analysis.diagnostics)
        .into_iter()
        .map(|p| Problem {
            source: "check",
            area: "scripts".to_owned(),
            code: if p.code.is_empty() {
                "script.diagnostic".to_owned()
            } else {
                format!("script.{}", p.code)
            },
            severity: match p.severity {
                "error" => Severity::Error,
                "warning" => Severity::Warning,
                _ => Severity::Note,
            },
            message: p.message,
            target: format!("{title}:{}", p.line),
            path: None,
            asset: asset.clone(),
            tool: Some(ToolKind::Variants),
            field: None,
            at: Some((p.line, p.col)),
        })
        .collect()
}

/// Read the problems of `source` (every one when `None`) over `checks`: the
/// registered content checks, and the packages under `<data>/packages/`.
/// A check that cannot run is under `skipped`, not a row.
pub fn gather(app: &mut App, source: Option<&str>, checks: &[ContentCheck]) -> Problems {
    let wanted = |s: &str| source.is_none_or(|w| w == s);
    let mut out = Problems::default();
    app.sync_tags();
    for c in checks {
        let src = source_of(c.area);
        if !wanted(src) {
            continue;
        }
        match (c.run)(app, None) {
            Ok(found) => {
                out.ran.push(json!({"source": src, "check": c.name, "findings": found.len()}));
                out.rows
                    .extend(found.iter().map(|f| from_finding(app, src, f)));
            }
            Err(why) => out
                .skipped
                .push(json!({"source": src, "check": c.name, "why": why})),
        }
    }
    if wanted("check") && app.rules.opened {
        let rows = script_rows(app);
        out.ran.push(json!({"source": "check", "check": "script.analysis", "findings": rows.len()}));
        out.rows.extend(rows);
    }
    if wanted("ground") {
        let rows = app.ground_problem_rows();
        out.ran.push(json!({"source": "ground", "check": "ground.build", "findings": rows.len()}));
        out.rows.extend(rows);
    }
    if wanted("package") || wanted("build") {
        let set = read_set(&app.data_root());
        if wanted("package") {
            let local: Vec<::workspace::Loaded> = set
                .on
                .iter()
                .filter(|e| matches!(e.origin, ::workspace::Origin::Local(_)))
                .map(|e| e.loaded.clone())
                .collect();
            let mut found: Vec<::workspace::Finding> =
                local.iter().flat_map(::workspace::check_package).collect();
            found.extend(::workspace::check_set(&local));
            // A file that is a raw resource or a tag file of the user's own
            // store: the retail list on their machine.
            let store = app.tags_root();
            for l in &local {
                found.extend(::workspace::store_findings(&store, l));
            }
            out.ran.push(json!({
                "source": "package", "check": "package.check", "findings": found.len()
            }));
            out.rows.extend(found.iter().map(from_package));
            let conflicts = app.problems.levels.clone();
            out.ran.push(json!({
                "source": "package", "check": "package.diff", "findings": conflicts.len()
            }));
            out.rows.extend(conflicts.iter().map(from_conflict));
        }
        if wanted("build") {
            out.ran.push(json!({
                "source": "build", "check": "packages.load", "findings": set.problems.len()
            }));
            out.rows.extend(set.problems.iter().map(|line| Problem {
                source: "build",
                area: "build".to_owned(),
                code: "build.package".to_owned(),
                severity: Severity::Error,
                message: line.clone(),
                target: String::new(),
                path: None,
                asset: None,
                tool: None,
                field: None,
                at: None,
            }));
        }
    }
    out
}

impl App {
    /// [`Action::RefreshProblems`] and `problems.list`: read `source` again
    /// (every source when `None`) over the registered checks.
    pub fn refresh_problems(&mut self, source: Option<&str>) {
        self.refresh_problems_over(source, CHECKS);
    }

    /// [`Self::refresh_problems`] over the given checks.
    pub fn refresh_problems_over(&mut self, source: Option<&str>, checks: &[ContentCheck]) {
        let fresh = gather(self, source, checks);
        let sources: Vec<&str> = match source {
            Some(s) => SOURCES.iter().copied().filter(|x| *x == s).collect(),
            None => SOURCES.to_vec(),
        };
        self.problems.merge(fresh, &sources);
    }

    /// Refresh the findings that can belong to this asset and return its rows.
    pub fn refresh_asset_problems(
        &mut self,
        asset: &str,
        source: Option<&str>,
        checks: &[ContentCheck],
    ) -> Result<Problems, String> {
        let (ran, skipped) = if matches!(AssetId::parse(asset)?, AssetId::Item(..)) {
            // Package checks and the open script can name items. The other
            // checks name tags or tools and cannot contribute item findings.
            let mut ran = Vec::new();
            if source.is_none_or(|s| s == "package") {
                let packages = gather(self, Some("package"), &[]);
                ran.extend(packages.ran.iter().cloned());
                self.problems.merge(packages, &["package"]);
            }
            if source.is_none_or(|s| s == "check") {
                let script = gather(self, Some("check"), &[]);
                ran.extend(script.ran.iter().cloned());
                self.problems.rows.retain(|p| p.area != "scripts");
                self.problems.ran.retain(|v| v["check"] != "script.analysis");
                self.problems.rows.extend(script.rows);
                self.problems.ran.extend(script.ran);
                self.problems.checked = true;
                self.problems.sort();
            }
            (ran, Vec::new())
        } else {
            self.refresh_problems_over(source, checks);
            (self.problems.ran.clone(), self.problems.skipped.clone())
        };
        Ok(Problems {
            rows: self.asset_problems(asset),
            ran,
            skipped,
            ..Problems::default()
        })
    }

    /// The rows about the document `doc`: its asset's rows, and for the
    /// documents with no asset of their own the rows its tool holds: a
    /// HUD's checks (not the kit's), the screen shown at its state, the
    /// open script's diagnostics. The header's count and `problems.list`
    /// with an `asset` read this.
    pub fn doc_problems(&self, doc: Doc) -> Vec<&Problem> {
        let asset = self.doc_asset(doc);
        let screen = match (doc, self.ui_screens.pick.as_ref()) {
            (Doc::Ui, Some(super::ui_screens::Pick::Native { screen, state })) => {
                Some(format!("{screen}/{state}"))
            }
            _ => None,
        };
        self.problems
            .rows
            .iter()
            .filter(|p| match doc {
                Doc::Hud => p.source == "hud" && !p.code.starts_with("hud.kit"),
                Doc::Rules => p.area == "scripts",
                Doc::Ui if screen.is_some() => p.source == "ui" && Some(&p.target) == screen.as_ref(),
                _ => asset.is_some() && p.asset == asset,
            })
            .collect()
    }

    /// How many rows [`Self::doc_problems`] has: the number on the
    /// header's **Check**.
    pub fn doc_problem_count(&self, doc: Doc) -> usize {
        self.doc_problems(doc).len()
    }

    /// The rows about asset `id`: those that name it, and those of a
    /// document whose asset it is ([`Self::doc_problems`]).
    pub fn asset_problems(&self, id: &str) -> Vec<Problem> {
        let norm = |s: &str| s.replace('/', "\\");
        let id = norm(id);
        let mut rows: Vec<Problem> = self
            .problems
            .rows
            .iter()
            .filter(|p| p.asset.as_deref().is_some_and(|a| norm(a) == id))
            .cloned()
            .collect();
        for doc in [Doc::Hud, Doc::Ui, Doc::Rules] {
            if self.doc_asset(doc).is_some_and(|a| norm(&a) == id) {
                for p in self.doc_problems(doc) {
                    if !rows.contains(p) {
                        rows.push(p.clone());
                    }
                }
            }
        }
        rows
    }

    /// [`Action::OpenProblem`]: the document of row `index`, in front: a tag
    /// in its editor with the finding's field shown, a package or item in
    /// the browser, a screen or a HUD in its tool.
    pub(super) fn open_problem(&mut self, index: usize) {
        let Some(p) = self.problems.rows.get(index).cloned() else {
            return self.error(format!("no problem {index} in the list"));
        };
        if let Some((line, col)) = p.at {
            // A script finding: the editor, with the cursor on the line.
            self.dispatch(Action::SetTool(ToolKind::Variants));
            let rows = rules_edit::problems(&self.rules.text, &self.rules.analysis.diagnostics);
            if let Some(row) = rows.iter().find(|r| r.line == line && r.col == col) {
                self.rules.want_cursor = Some(row.at);
                self.rules.cursor = row.at;
            }
            return;
        }
        if let Some(asset) = &p.asset {
            if let Some(text) = asset.strip_prefix("tag:") {
                self.open_asset(asset, None);
                if let Some(field) = &p.field
                    && let Some(tag) = tag_api::find_tag(&self.tags, text)
                {
                    let go = NavAction::Go {
                        tag,
                        path: Some(field.clone()),
                        element: None,
                    };
                    if let Err(e) = self.navigate(go) {
                        self.error(e);
                    }
                }
            } else if asset.starts_with("package:") {
                self.dispatch(Action::OpenAsset { asset: asset.clone(), map: None });
            } else {
                // The browser is a panel of the map tools.
                if !matches!(self.tool, ToolKind::Scenario | ToolKind::Tags | ToolKind::Scripts) {
                    self.dispatch(Action::SetTool(ToolKind::Scenario));
                }
                self.problems.reveal = Some(asset.clone());
            }
        } else if let Some(tool) = p.tool {
            self.dispatch(Action::SetTool(tool));
        } else {
            self.info(format!("{}: nothing to open", p.message));
        }
    }
}
