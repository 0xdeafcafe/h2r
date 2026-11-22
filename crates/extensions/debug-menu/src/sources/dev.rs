//! Dev: what a developer reaches for and a tester doesn't. Shader
//! overrides, live tag edits, the feature plan and the renderer's raw
//! stats, each a page of its own.
//!
//! Features parse the real shape of `features.list`: `plan` is the lines
//! of `hooks::Plan::text()` (`crates/hooking/hooks/src/plan.rs`), one
//! `feature <id>: on` or `feature <id>: off (<reason>)` a feature, then
//! one line a hooked address, `warning:` and `note:` lines. When the answer
//! has a structured `features` array, that is read instead.

use serde_json::{Value, json};

use super::method::{
    EDIT_CHANGES, EDIT_REVERT, FEATURES, RENDER_OVERRIDES, RENDER_RELOAD, RENDER_STATS,
    RENDER_TOGGLE,
};
use super::{flat, get, humanize, leaf, list, nothing, problem, str_of};
use crate::model::{Answer, Call, Row, Section, Setter, text};

pub fn section() -> Section {
    Section {
        id: "dev".into(),
        title: "Dev".into(),
        about: "shader overrides, tag edits, features, raw render stats".into(),
        calls: vec![Call::bare(EDIT_CHANGES)],
        rows: dev_rows,
    }
}

pub fn pages() -> Vec<Section> {
    vec![
        Section {
            id: "dev/shaders".into(),
            title: "Shader overrides".into(),
            about: "WGSL overrides: all on or off, reload, each one's state".into(),
            calls: vec![Call::bare(RENDER_OVERRIDES)],
            rows: shader_rows,
        },
        Section {
            id: "dev/tags".into(),
            title: "Tag edits".into(),
            about: "the live tag edits journal; revert one or all".into(),
            calls: vec![Call::bare(EDIT_CHANGES)],
            rows: tag_rows,
        },
        Section {
            id: "dev/features".into(),
            title: "Features".into(),
            about: "the feature plan and each feature's state".into(),
            calls: vec![Call::bare(FEATURES)],
            rows: feature_rows,
        },
        Section {
            id: "dev/stats".into(),
            title: "Render stats".into(),
            about: "the renderer's counters, read-only".into(),
            calls: vec![Call::bare(RENDER_STATS)],
            rows: stats_rows,
        },
    ]
}

fn dev_rows(a: &[Answer]) -> Vec<Row> {
    let edits = get(a, 0)
        .map(|v| list(&v, "changes").len())
        .unwrap_or_default();
    let tags = if edits > 0 {
        format!("Tag edits ({edits})")
    } else {
        "Tag edits".into()
    };
    vec![
        Row::open("Shader overrides", "dev/shaders"),
        Row::open(tags, "dev/tags"),
        Row::open("Features", "dev/features"),
        Row::open("Render stats", "dev/stats"),
    ]
}

// ---- shader overrides ----

/// Shader overrides: the switch for all of them, a reload, and each
/// loaded override with what the renderer did with it. A program is named
/// by its override file, which carries the shader's name; its id is the
/// footer.
fn shader_rows(a: &[Answer]) -> Vec<Row> {
    let v = match get(a, 0) {
        Ok(v) => v,
        Err(e) => return vec![problem("Shader overrides", e)],
    };
    let mut out = vec![
        Row::toggle(
            "All overrides",
            v["enabled"].as_bool().unwrap_or(true),
            Setter::new(RENDER_TOGGLE, json!({ "name": "overrides" }), "on"),
        )
        .about("off draws every program as translated, to compare"),
        Row::action("Reload", Call::bare(RENDER_RELOAD))
            .on_ok(|_| "reloaded the overrides".into())
            .about("read the override directory again"),
        Row::info(
            "Directory",
            v["dir"].as_str().unwrap_or("none (H2_RENDER_OVERRIDES)"),
        ),
    ];
    let overrides = list(&v, "overrides");
    if overrides.is_empty() {
        out.push(Row::info("Overrides", "none loaded"));
    }
    for o in overrides {
        let program = str_of(&o, "program");
        let file = str_of(&o, "file");
        let name = override_name(&file, &program);
        let row = Row::info(name, str_of(&o, "state")).about(format!("{file} (program {program})"));
        out.push(match o["error"].as_str() {
            Some(e) => row.disabled(format!("rejected: {e}")),
            None => row,
        });
    }
    for p in list(&v, "problems") {
        out.push(
            Row::info(override_name(&str_of(&p, "file"), ""), "not loaded")
                .disabled(str_of(&p, "error")),
        );
    }
    out
}

/// An override's name: its file without the directory and extension
/// (`ui_blur.wgsl` is `Ui blur`), else its program id.
fn override_name(file: &str, program: &str) -> String {
    let stem = leaf(file).rsplit_once('.').map_or(leaf(file), |(s, _)| s);
    if stem.is_empty() {
        format!("Program {program}")
    } else {
        humanize(stem)
    }
}

// ---- tag edits ----

/// Live tag edits: the journal (`edit.changes`) read-only, a revert for
/// each and one for all. Editing fields is `h2 agent edit set`'s.
fn tag_rows(a: &[Answer]) -> Vec<Row> {
    let v = match get(a, 0) {
        Ok(v) => v,
        Err(e) => return vec![problem("Tag edits", e)],
    };
    let changes = list(&v, "changes");
    if changes.is_empty() {
        return nothing("No edits", "h2 agent edit set changes a field");
    }
    let mut out = vec![
        Row::action("Revert all", Call::bare(EDIT_REVERT))
            .confirm()
            .on_ok(|_| "reverted every tag edit".into())
            .about("put back every change, newest first; press again to confirm"),
    ];
    for c in changes {
        let id = c["id"].as_u64().unwrap_or(0);
        let name = c["name"]
            .as_str()
            .map_or_else(|| str_of(&c, "tag"), str::to_owned);
        let label = format!("{} {}", super::tag_label(&name), str_of(&c, "path"));
        let state = if c["stale"].as_bool() == Some(true) {
            "other map"
        } else if c["live"].as_bool() == Some(true) {
            "live"
        } else {
            "changed since"
        };
        out.push(
            Row::action(label, Call::new(EDIT_REVERT, json!({ "id": id })))
                .on_ok(|_| "reverted the edit".into())
                .about(format!(
                    "{} {}: {} -> {} ({state}); select reverts it",
                    str_of(&c, "group"),
                    name,
                    text(&c["old"]),
                    text(&c["new"]),
                )),
        );
    }
    out
}

// ---- features ----

/// One feature: its id, whether it is on, and why not.
#[derive(Debug, PartialEq)]
struct Feature {
    id: String,
    on: bool,
    why: Option<String>,
}

/// `feature <id>: on` or `feature <id>: off (<reason>)`.
fn parse_feature(line: &str) -> Option<Feature> {
    let rest = line.trim().strip_prefix("feature ")?;
    let (id, state) = rest.split_once(": ")?;
    let state = state.trim();
    if state == "on" {
        return Some(Feature {
            id: id.trim().to_owned(),
            on: true,
            why: None,
        });
    }
    let off = state.strip_prefix("off")?;
    let why = off
        .trim()
        .strip_prefix('(')
        .and_then(|r| r.strip_suffix(')'))
        .map(str::to_owned);
    Some(Feature {
        id: id.trim().to_owned(),
        on: false,
        why,
    })
}

/// The features of a `features.list` answer: its structured `features`
/// array when it has one (`{id, on, why?}`; the reason may also be called
/// `off` or `reason`), else the feature lines of the plan text.
fn features_of(v: &Value) -> Vec<Feature> {
    if let Some(arr) = v["features"].as_array() {
        return arr
            .iter()
            .filter_map(|f| {
                let id = f["id"].as_str().or(f["name"].as_str())?.to_owned();
                let why = ["why", "reason", "off"]
                    .iter()
                    .find_map(|k| f[*k].as_str())
                    .map(str::to_owned);
                let on = f["on"]
                    .as_bool()
                    .or_else(|| f["state"].as_str().map(|s| s == "on"))
                    .unwrap_or(why.is_none());
                Some(Feature { id, on, why })
            })
            .collect();
    }
    list(v, "plan")
        .iter()
        .filter_map(Value::as_str)
        .filter_map(parse_feature)
        .collect()
}

fn feature_rows(a: &[Answer]) -> Vec<Row> {
    let v = match get(a, 0) {
        Ok(v) => v,
        Err(e) => return vec![problem("Features", e)],
    };
    let mut out: Vec<Row> = features_of(&v)
        .into_iter()
        .map(|f| {
            let row = Row::info(humanize(&f.id), if f.on { "on" } else { "off" })
                .about(format!("feature {}", f.id));
            match (f.on, f.why) {
                (false, Some(why)) => row.disabled(why),
                (false, None) => row.disabled("switched off"),
                _ => row,
            }
        })
        .collect();
    // The plan's warnings are worth seeing; its hook lines are not.
    for l in list(&v, "plan").iter().filter_map(Value::as_str) {
        if let Some(w) = l.trim().strip_prefix("warning: ") {
            out.push(Row::info("Warning", "").disabled(w.to_owned()));
        }
    }
    if out.is_empty() {
        out.push(Row::info("No features", "").about("the plan is empty"));
    }
    out
}

// ---- raw render stats ----

fn stats_rows(a: &[Answer]) -> Vec<Row> {
    match get(a, 0) {
        Ok(v) => {
            let rows = flat("Stats", &v);
            if rows.is_empty() {
                nothing("No stats", "the renderer reports none yet")
            } else {
                rows
            }
        }
        Err(e) => vec![problem("Render stats", e)],
    }
}

#[cfg(test)]
mod tests {
    use super::super::fake::{Game, page, row};
    use super::*;
    use crate::model::Edit;

    #[test]
    fn dev_opens_its_pages_and_counts_tag_edits() {
        let mut g = Game::default();
        let rows = page(&mut g, "dev");
        let labels: Vec<_> = rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(
            labels,
            ["Shader overrides", "Tag edits (1)", "Features", "Render stats"]
        );
        assert_eq!(rows[1].edit, Edit::Open("dev/tags".into()));
    }

    #[test]
    fn features_parse_what_plan_text_really_emits() {
        let mut g = Game::default();
        let rows = page(&mut g, "dev/features");
        let labels: Vec<_> = rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, ["Agent", "Lockstep", "Cheats", "Warning"]);
        assert_eq!(row(&rows, "Agent").shown(), "on");
        assert_eq!(row(&rows, "Agent").disabled, None);
        let lock = row(&rows, "Lockstep");
        assert_eq!(lock.shown(), "off");
        assert_eq!(lock.disabled.as_deref(), Some("not requested"));
        assert!(row(&rows, "Warning").disabled.as_deref().unwrap().starts_with("feature debug-views"));
    }

    #[test]
    fn a_feature_line_parses_both_states() {
        assert_eq!(
            parse_feature("feature agent: on"),
            Some(Feature { id: "agent".into(), on: true, why: None })
        );
        assert_eq!(
            parse_feature("feature x: off (H2_FEATURES=-x)"),
            Some(Feature { id: "x".into(), on: false, why: Some("H2_FEATURES=-x".into()) })
        );
        assert_eq!(
            parse_feature("feature y: off"),
            Some(Feature { id: "y".into(), on: false, why: None })
        );
        assert_eq!(parse_feature("0x00123456 game_tick: original"), None);
        assert_eq!(parse_feature("note: feature a: on"), None);
    }

    #[test]
    fn the_structured_features_array_wins_over_the_text() {
        let v = json!({"plan": ["feature old: on"], "features": [
            {"id": "agent", "on": true},
            {"id": "lockstep", "on": false, "why": "not requested"},
            {"name": "cheats", "state": "off", "reason": "H2_FEATURES"},
        ]});
        let f = features_of(&v);
        assert_eq!(f.len(), 3);
        assert_eq!(f[1], Feature { id: "lockstep".into(), on: false, why: Some("not requested".into()) });
        assert!(!f[2].on);
    }

    #[test]
    fn shader_overrides_are_named_by_their_file_and_the_switch_is_here() {
        let mut g = Game::default();
        let rows = page(&mut g, "dev/shaders");
        assert_eq!(rows[0].label, "All overrides");
        assert_eq!(
            rows[0].set.as_ref().unwrap().call(json!(false)).to_string(),
            r#"render.toggle {"name":"overrides","on":false}"#
        );
        let bad = row(&rows, "Bb");
        assert_eq!(bad.disabled.as_deref(), Some("rejected: binding 3"));
        assert_eq!(bad.about, "bb.wgsl (program 00000000000000bb)");
        assert_eq!(row(&rows, "Aa").shown(), "applied");
        assert!(rows.iter().all(|r| !r.label.contains("0000000")), "no hex ids as labels");
    }

    #[test]
    fn tag_edits_revert_one_or_all() {
        let mut g = Game::default();
        let rows = page(&mut g, "dev/tags");
        assert_eq!(rows[0].label, "Revert all");
        assert_eq!(rows[1].label, "Battle Rifle magazines[0].rounds_loaded_maximum");
        assert!(rows[1].about.contains("36 -> 99 (live)"), "{}", rows[1].about);
        let Edit::Action(c) = &rows[1].edit else { panic!("an action") };
        assert_eq!(*c, Call::new("edit.revert", json!({"id": 3})));
    }

    #[test]
    fn render_stats_are_read_only_rows_named_by_path() {
        let mut g = Game::default();
        let rows = page(&mut g, "dev/stats");
        let labels: Vec<_> = rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, ["Stats pipeline frames", "Stats pipeline on"]);
        assert!(rows.iter().all(|r| r.set.is_none()));
    }
}
