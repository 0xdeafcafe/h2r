//! The menu's sections, each over agent methods (docs/runtime/debug-menu.md
//! "What it shows"). A section lists its page with read calls and turns
//! the answers into rows; each row that changes something carries the
//! write call. Nothing here is a row written by hand for one setting: the
//! rows are whatever the registries list.
//!
//! The agent API's own names (`render.*`, `edit.*`, `features.list`,
//! `profile.counters`) are the agent's; the ones the menu added beside
//! them are in [`method`].

use serde_json::{Map, Value, json};

use crate::model::{Answer, Call, Row, Section, Setter, text};

/// Agent methods the game's host answers for the menu (and for
/// `h2 agent`, like every other method).
pub mod method {
    /// `{}`: the cheats, their values, and why they are refused if they
    /// are. `{"name", "value"}` is [`CHEAT_SET`].
    pub const CHEAT_LIST: &str = "cheat.list";
    /// `{"name": "god", "value": true}`.
    pub const CHEAT_SET: &str = "cheat.set";
    /// `{"prefix"?}`: settings (`settings.toml`), how each applies.
    pub const SETTINGS_LIST: &str = "settings.list";
    /// `{"key", "value"}`: saved, and applied as the setting applies.
    pub const SETTINGS_SET: &str = "settings.set";
    /// `{}` reads, `{"on": bool}` sets: the perf overlay (the watermark).
    pub const PERF_OVERLAY: &str = "perf.overlay";
    /// The agent API's methods the menu reads.
    pub const RENDER_TOGGLE: &str = "render.toggle";
    pub const RENDER_PARAM: &str = "render.param";
    pub const RENDER_STATS: &str = "render.stats";
    pub const RENDER_OVERRIDES: &str = "render.overrides";
    pub const RENDER_RELOAD: &str = "render.reload";
    pub const EDIT_CHANGES: &str = "edit.changes";
    pub const EDIT_REVERT: &str = "edit.revert";
    pub const FEATURES: &str = "features.list";
    pub const COUNTERS: &str = "profile.counters";
}

/// The settings tables the Render page lists.
pub const RENDER_SETTINGS: &str = "graphics.";

/// Every section, in first-page order.
pub fn all() -> Vec<Section> {
    vec![render(), shaders(), tags(), cheats(), perf(), features()]
}

/// Rendering: the host renderer's toggles and parameters (`render.toggle`,
/// `render.param`), the graphics settings, and `render.stats` read-only.
pub fn render() -> Section {
    Section {
        id: "render".into(),
        title: "Render".into(),
        about: "renderer toggles, parameters, graphics settings, render.stats".into(),
        calls: vec![
            Call::bare(method::RENDER_TOGGLE),
            Call::bare(method::RENDER_PARAM),
            Call::new(method::SETTINGS_LIST, json!({ "prefix": RENDER_SETTINGS })),
            Call::bare(method::RENDER_STATS),
        ],
        rows: render_rows,
    }
}

fn render_rows(a: &[Answer]) -> Vec<Row> {
    let mut out = Vec::new();
    match get(a, 0) {
        Ok(v) => out.extend(list(&v, "toggles").iter().map(toggle_row)),
        Err(e) => out.push(problem("toggles", e)),
    }
    match get(a, 1) {
        Ok(v) => out.extend(list(&v, "params").iter().map(param_row)),
        Err(e) => out.push(problem("parameters", e)),
    }
    match get(a, 2) {
        Ok(v) => out.extend(list(&v, "settings").iter().map(setting_row)),
        Err(e) => out.push(problem("settings", e)),
    }
    match get(a, 3) {
        Ok(v) => out.extend(flat("stats", &v)),
        Err(e) => out.push(problem("render.stats", e)),
    }
    out
}

/// A `render.toggle` entry. One that says `restart` is greyed with it.
fn toggle_row(t: &Value) -> Row {
    let name = str_of(t, "name");
    let row = Row::toggle(
        name.clone(),
        t["on"].as_bool().unwrap_or(false),
        Setter::new(method::RENDER_TOGGLE, json!({ "name": name }), "on"),
    )
    .about(str_of(t, "about"));
    restart(row, t)
}

/// A `render.param` entry: a float, stepped by a tenth of its default
/// (0.05 when that is 0).
fn param_row(p: &Value) -> Row {
    let name = str_of(p, "name");
    let default = p["default"].as_f64().unwrap_or(0.0);
    let step = if default == 0.0 {
        0.05
    } else {
        nice_step(default.abs() / 10.0)
    };
    let row = Row::float(
        name.clone(),
        p["value"].as_f64().unwrap_or(default),
        step,
        (f64::MIN, f64::MAX),
        Setter::new(method::RENDER_PARAM, json!({ "name": name }), "value"),
    )
    .about(str_of(p, "about"));
    restart(row, p)
}

/// 1, 2 or 5 times a power of ten, at most `v`.
fn nice_step(v: f64) -> f64 {
    let p = 10f64.powf(v.log10().floor());
    [5.0, 2.0, 1.0]
        .into_iter()
        .map(|m| m * p)
        .find(|s| *s <= v * (1.0 + 1e-9))
        .unwrap_or(p)
}

/// A `settings.list` entry: `{key, label, kind, value, apply, locked,
/// choices?, steps?, min?, max?, help}`.
pub fn setting_row(s: &Value) -> Row {
    let key = str_of(s, "key");
    let label = if s["label"].is_string() {
        str_of(s, "label")
    } else {
        key.clone()
    };
    let set = Setter::new(method::SETTINGS_SET, json!({ "key": key }), "value");
    let v = s["value"].clone();
    let row = match str_of(s, "kind").as_str() {
        "bool" => Row::toggle(label, v.as_bool().unwrap_or(false), set),
        "choice" => {
            let choices = list(s, "choices")
                .iter()
                .map(|c| (c["value"].clone(), str_of(c, "label")))
                .collect();
            Row::choice(label, v, choices, set)
        }
        "int" => {
            // The Video screen's steps (`{value, label}` or bare numbers),
            // as a choice.
            let steps: Vec<(Value, String)> = list(s, "steps")
                .iter()
                .map(|x| match x.get("value") {
                    Some(v) => (v.clone(), str_of(x, "label")),
                    None => (x.clone(), text(x)),
                })
                .collect();
            if steps.len() > 1 {
                Row::choice(label, v, steps, set)
            } else {
                let min = s["min"].as_i64().unwrap_or(i64::MIN);
                let max = s["max"].as_i64().unwrap_or(i64::MAX);
                Row::int(label, v.as_i64().unwrap_or(min), 1, (min, max), set)
            }
        }
        _ => Row::info(label, v),
    }
    .about(format!("{key}: {}", str_of(s, "help")));
    match (str_of(s, "apply").as_str(), s["locked"].as_str()) {
        (_, Some(by)) => row.disabled(format!("set by {by} for this run")),
        ("restart", _) => row.disabled("applies after a restart; change it in settings.toml"),
        _ => restart(row, s),
    }
}

fn restart(row: Row, v: &Value) -> Row {
    match v["restart"].as_str() {
        Some(why) => row.disabled(format!("needs a restart: {why}")),
        None => row,
    }
}

/// Shader overrides: the switch for all of them, a reload, and each
/// loaded override with what the renderer did with it.
pub fn shaders() -> Section {
    Section {
        id: "shaders".into(),
        title: "Shader overrides".into(),
        about: "WGSL overrides: all on or off, reload, each one's state".into(),
        calls: vec![Call::bare(method::RENDER_OVERRIDES)],
        rows: shader_rows,
    }
}

fn shader_rows(a: &[Answer]) -> Vec<Row> {
    let v = match get(a, 0) {
        Ok(v) => v,
        Err(e) => return vec![problem("render.overrides", e)],
    };
    let mut out = vec![
        Row::toggle(
            "all overrides",
            v["enabled"].as_bool().unwrap_or(true),
            Setter::new(method::RENDER_TOGGLE, json!({ "name": "overrides" }), "on"),
        )
        .about("off draws every program as translated, to compare"),
        Row::action("reload", Call::bare(method::RENDER_RELOAD))
            .about("read the override directory again"),
        Row::info(
            "directory",
            v["dir"].as_str().unwrap_or("none (H2_RENDER_OVERRIDES)"),
        ),
    ];
    let overrides = list(&v, "overrides");
    if overrides.is_empty() {
        out.push(Row::info("overrides", "none loaded"));
    }
    for o in overrides {
        let state = str_of(&o, "state");
        let row = Row::info(str_of(&o, "program"), state.clone()).about(str_of(&o, "file"));
        out.push(match o["error"].as_str() {
            Some(e) => row.disabled(format!("rejected: {e}")),
            None => row,
        });
    }
    for p in list(&v, "problems") {
        out.push(Row::info(str_of(&p, "file"), "not loaded").disabled(str_of(&p, "error")));
    }
    out
}

/// Live tag edits: the journal (`edit.changes`) read-only, a revert for
/// each and one for all. Editing fields is `h2 agent edit set`'s.
pub fn tags() -> Section {
    Section {
        id: "tags".into(),
        title: "Tag edits".into(),
        about: "the live tag edits journal; revert one or all".into(),
        calls: vec![Call::bare(method::EDIT_CHANGES)],
        rows: tag_rows,
    }
}

fn tag_rows(a: &[Answer]) -> Vec<Row> {
    let v = match get(a, 0) {
        Ok(v) => v,
        Err(e) => return vec![problem("edit.changes", e)],
    };
    let changes = list(&v, "changes");
    if changes.is_empty() {
        return vec![Row::info("no edits", "").about("h2 agent edit set changes a field")];
    }
    let mut out = vec![
        Row::action("revert all", Call::bare(method::EDIT_REVERT))
            .about("put back every change, newest first"),
    ];
    for c in changes {
        let id = c["id"].as_u64().unwrap_or(0);
        let name = c["name"]
            .as_str()
            .map_or_else(|| str_of(&c, "tag"), str::to_owned);
        let label = format!("#{id} {} {}", str_of(&c, "path"), leaf(&name));
        let state = if c["stale"].as_bool() == Some(true) {
            "other map"
        } else if c["live"].as_bool() == Some(true) {
            "live"
        } else {
            "changed since"
        };
        out.push(
            Row::action(label, Call::new(method::EDIT_REVERT, json!({ "id": id }))).about(format!(
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

/// The last path component of a tag name.
fn leaf(name: &str) -> &str {
    name.rsplit(['\\', '/']).next().unwrap_or(name)
}

/// The game's cheats as `cheat.list` gives them. In a co-op session the
/// answer says why they are refused, and every row is greyed with it.
pub fn cheats() -> Section {
    Section {
        id: "cheats".into(),
        title: "Cheats".into(),
        about: "god mode, ammo, game speed; off in co-op".into(),
        calls: vec![Call::bare(method::CHEAT_LIST)],
        rows: cheat_rows,
    }
}

fn cheat_rows(a: &[Answer]) -> Vec<Row> {
    let v = match get(a, 0) {
        Ok(v) => v,
        Err(e) => return vec![problem("cheat.list", e)],
    };
    let refused = v["refused"].as_str().map(str::to_owned);
    let mut out = Vec::new();
    if let Some(why) = &refused {
        out.push(Row::info("cheats are off", "").disabled(why.clone()));
    }
    for c in list(&v, "cheats") {
        let name = str_of(&c, "name");
        let label = if c["label"].is_string() {
            str_of(&c, "label")
        } else {
            name.clone()
        };
        let set = Setter::new(method::CHEAT_SET, json!({ "name": name }), "value");
        let row = match str_of(&c, "kind").as_str() {
            "choice" => {
                let choices = list(&c, "choices")
                    .iter()
                    .map(|x| (x["value"].clone(), str_of(x, "label")))
                    .collect();
                Row::choice(label, c["value"].clone(), choices, set)
            }
            _ => Row::toggle(label, c["value"].as_bool().unwrap_or(false), set),
        }
        .about(str_of(&c, "about"));
        out.push(match &refused {
            Some(why) => row.disabled(why.clone()),
            None => row,
        });
    }
    out
}

/// Perf: the overlay's switch (`perf.overlay`) and the frame-time window
/// (`profile.counters`) read-only.
pub fn perf() -> Section {
    Section {
        id: "perf".into(),
        title: "Perf".into(),
        about: "the perf overlay, and frame times".into(),
        calls: vec![
            Call::bare(method::PERF_OVERLAY),
            Call::bare(method::COUNTERS),
        ],
        rows: perf_rows,
    }
}

fn perf_rows(a: &[Answer]) -> Vec<Row> {
    let mut out = Vec::new();
    match get(a, 0) {
        Ok(v) => out.push(
            Row::toggle(
                "perf overlay",
                v["on"].as_bool().unwrap_or(false),
                Setter::new(method::PERF_OVERLAY, json!({}), "on"),
            )
            .about("the fps and build lines along the bottom (F3)"),
        ),
        Err(e) => out.push(problem("perf overlay", e)),
    }
    match get(a, 1) {
        Ok(v) => {
            for k in ["fps", "p50_ms", "p99_ms", "max_ms", "frames"] {
                if !v[k].is_null() {
                    out.push(Row::info(k, v[k].clone()));
                }
            }
            for (group, label) in [("phase_ms", "ms"), ("counts", "per frame")] {
                if let Some(o) = v[group].as_object() {
                    out.extend(
                        o.iter()
                            .map(|(k, x)| Row::info(format!("{k} {label}"), x.clone())),
                    );
                }
            }
        }
        Err(e) => out.push(problem("profile.counters", e)),
    }
    out
}

/// The feature plan (`features.list`), one line a row, read-only.
pub fn features() -> Section {
    Section {
        id: "features".into(),
        title: "Features".into(),
        about: "the feature plan and each feature's state".into(),
        calls: vec![Call::bare(method::FEATURES)],
        rows: feature_rows,
    }
}

fn feature_rows(a: &[Answer]) -> Vec<Row> {
    match get(a, 0) {
        Ok(v) => list(&v, "plan")
            .iter()
            .filter_map(|l| l.as_str())
            .filter(|l| !l.trim().is_empty())
            .map(|l| {
                let l = l.trim();
                // `name  state  rest`: the first word left, the rest right.
                match l.split_once(char::is_whitespace) {
                    Some((name, rest)) => Row::info(name, rest.trim()).about(l),
                    None => Row::info(l, ""),
                }
            })
            .collect(),
        Err(e) => vec![problem("features.list", e)],
    }
}

// ---- helpers ----

/// Answer `i`: waiting reads as an error saying so.
fn get(a: &[Answer], i: usize) -> Result<Value, String> {
    match a.get(i) {
        Some(Some(r)) => r.clone(),
        _ => Err("waiting for the game".into()),
    }
}

/// A row saying a listing failed.
fn problem(what: &str, e: String) -> Row {
    Row::info(what, "unavailable").disabled(e)
}

fn list(v: &Value, k: &str) -> Vec<Value> {
    v[k].as_array().cloned().unwrap_or_default()
}

fn str_of(v: &Value, k: &str) -> String {
    match &v[k] {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => text(other),
    }
}

/// Every scalar under `v` as a read-only row named by its path (`stats
/// pipeline.frames`). Objects and arrays nest; at most 40 rows.
pub fn flat(prefix: &str, v: &Value) -> Vec<Row> {
    fn walk(path: String, v: &Value, out: &mut Vec<Row>) {
        if out.len() >= 40 {
            return;
        }
        match v {
            Value::Object(o) => walk_map(&path, o, out),
            Value::Array(a) => {
                for (i, x) in a.iter().enumerate() {
                    walk(format!("{path}.{i}"), x, out);
                }
            }
            Value::Null => {}
            x => out.push(Row::info(path, x.clone())),
        }
    }
    fn walk_map(path: &str, o: &Map<String, Value>, out: &mut Vec<Row>) {
        for (k, x) in o {
            if k == "at" {
                continue;
            }
            walk(format!("{path}.{k}"), x, out);
        }
    }
    let mut out = Vec::new();
    walk(prefix.to_owned(), v, &mut out);
    out
}
