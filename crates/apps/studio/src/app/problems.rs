//! Problems (docs/editor/studio-redesign.md, "The shell"): one list over
//! every check and build error, read by the Problems panel
//! (`ui/problems.rs`) and `problems.list` (`api/problems.rs`).
//!
//! A problem has a **source**: `check` (`check.run`'s models and sounds),
//! `ui` (`ui.check`), `hud` (`hud.check` and the HUD kit's), `package`
//! (the manifest checks `h2 package check` runs over the packages under
//! `<data>/packages/`) and `build` (what the game's package set refuses
//! to load, as the boot log says it). Each row names what it opens: a tag
//! (`asset`, with the `field` the finding names), a package or one of its
//! items, or the tool that holds the screen or the HUD.
//!
//! [`Action::OpenProblem`] opens a row, as the panel's double-click does:
//! the document in front and, for a tag, the field the finding names.
//!
//! [`Action::OpenProblem`]: super::Action::OpenProblem

use serde_json::{Value, json};

use super::assets::read_set;
use super::navigation::NavAction;
use super::{Action, App, ToolKind, tag_api};
use crate::api::checks::{CHECKS, ContentCheck, Finding, Severity};

/// Every source, in the order the panel lists them.
pub const SOURCES: [&str; 5] = ["check", "ui", "hud", "package", "build"];

/// The source a check's findings belong to: the content checks' areas
/// `ui` and `hud` are their own sources, `models` and `sounds` are `check`.
pub fn source_of(area: &str) -> &'static str {
    match area {
        "ui" => "ui",
        "hud" => "hud",
        "package" => "package",
        "build" => "build",
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
}

impl Problems {
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
        asset,
        tool: match source {
            "ui" => Some(ToolKind::UiScreens),
            "hud" => Some(ToolKind::Hud),
            _ => None,
        },
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
    }
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
            out.ran.push(json!({
                "source": "package", "check": "package.check", "findings": found.len()
            }));
            out.rows.extend(found.iter().map(from_package));
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

    /// [`Action::OpenProblem`]: the document of row `index`, in front: a tag
    /// in its editor with the finding's field shown, a package or item in
    /// the browser, a screen or a HUD in its tool.
    pub(super) fn open_problem(&mut self, index: usize) {
        let Some(p) = self.problems.rows.get(index).cloned() else {
            return self.error(format!("no problem {index} in the list"));
        };
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
