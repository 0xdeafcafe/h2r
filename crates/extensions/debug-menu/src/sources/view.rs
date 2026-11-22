//! View: what is drawn. The world debug views (`debug.views`,
//! `debug.view`) are on the page itself; the renderer's own switches and
//! parameters (`render.toggle`, `render.param`) are one page down, and
//! the graphics settings are the Video page of Settings.
//!
//! The `overrides` toggle (all shader overrides on or off) is on the
//! Dev > Shader overrides page, not here.

use serde_json::{Value, json};

use super::method::{HOST_SCREENSHOT, RENDER_PARAM, RENDER_TOGGLE};
use super::{get, humanize, list, problem, str_of};
use crate::model::{Answer, Call, Row, Section, Setter};

/// The default draw range, world units (`agent_proto::debug`).
const RANGE_DEFAULT: i64 = 48;
/// Range steps by this much: a range is judged by eye, not unit by unit.
const RANGE_STEP: i64 = 8;

pub fn section() -> Section {
    Section {
        id: "view".into(),
        title: "View".into(),
        about: "debug views of the world, render passes, graphics".into(),
        calls: vec![Call::bare(agent_proto::debug::method::VIEWS)],
        rows: view_rows,
    }
}

pub fn pages() -> Vec<Section> {
    vec![Section {
        id: "view/render".into(),
        title: "Render passes".into(),
        about: "the renderer's switches and parameters".into(),
        calls: vec![Call::bare(RENDER_TOGGLE), Call::bare(RENDER_PARAM)],
        rows: render_rows,
    }]
}

fn view_rows(a: &[Answer]) -> Vec<Row> {
    use agent_proto::debug::{RANGE, TRACE_TICKS, ViewsResult, method};
    let state: ViewsResult =
        match get(a, 0).and_then(|v| serde_json::from_value(v).map_err(|e| e.to_string())) {
            Ok(state) => state,
            Err(e) => return vec![problem("Debug views", e)],
        };
    let mut rows: Vec<_> = state
        .views
        .into_iter()
        .map(|v| {
            let row = Row::toggle(
                v.label,
                v.on,
                Setter::new(method::VIEW, json!({"name": v.name}), "on"),
            );
            match v.why {
                Some(why) if !v.on => row.disabled(why),
                Some(why) => row.about(why),
                None => row,
            }
        })
        .collect();
    let setting = |key| Setter::new(method::VIEW, json!({}), key);
    rows.push(
        Row::int(
            "Range (world units)",
            state.range.into(),
            RANGE_STEP,
            ((*RANGE.start()).into(), (*RANGE.end()).into()),
            setting("range"),
        )
        // Explicit: the step scaled to the 1..=512 range would be ten.
        .step(RANGE_STEP as f64)
        .default_value(RANGE_DEFAULT)
        .about("how far from you the debug views draw"),
    );
    rows.push(
        Row::int(
            "Trace lifetime (ticks)",
            state.trace_ticks.into(),
            1,
            ((*TRACE_TICKS.start()).into(), (*TRACE_TICKS.end()).into()),
            setting("trace_ticks"),
        )
        .about("how long a collision trace stays on screen; 30 ticks is a second"),
    );
    rows.push(
        Row::action("Take screenshot", Call::bare(HOST_SCREENSHOT))
            .on_ok(shot_toast)
            .about("the newest frame as a PNG, in the game's screenshots directory"),
    );
    rows.push(Row::open("Render passes", "view/render"));
    rows.push(Row::open("Graphics settings", "settings/video"));
    rows
}

/// `screenshot <file>`: the file the shot was written to.
fn shot_toast(v: &Value) -> String {
    match v["path"]
        .as_str()
        .and_then(|p| p.rsplit(['/', '\\']).next())
        .filter(|n| !n.is_empty())
    {
        Some(name) => format!("saved {name}"),
        None => "screenshot saved".into(),
    }
}

fn render_rows(a: &[Answer]) -> Vec<Row> {
    let mut out = Vec::new();
    match get(a, 0) {
        Ok(v) => out.extend(
            list(&v, "toggles")
                .iter()
                .filter(|t| t["name"] != "overrides")
                .map(toggle_row),
        ),
        Err(e) => out.push(problem("Switches", e)),
    }
    match get(a, 1) {
        Ok(v) => out.extend(list(&v, "params").iter().map(param_row)),
        Err(e) => out.push(problem("Parameters", e)),
    }
    if out.is_empty() {
        out.push(Row::info("No render switches", "").about("the renderer declares none"));
    }
    out
}

/// A `render.toggle` entry. One that says `restart` is greyed with it.
fn toggle_row(t: &Value) -> Row {
    let name = str_of(t, "name");
    let row = Row::toggle(
        humanize(&name),
        t["on"].as_bool().unwrap_or(false),
        Setter::new(RENDER_TOGGLE, json!({ "name": name }), "on"),
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
        humanize(&name),
        p["value"].as_f64().unwrap_or(default),
        step,
        (f64::MIN, f64::MAX),
        Setter::new(RENDER_PARAM, json!({ "name": name }), "value"),
    )
    .default_value(default)
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

fn restart(row: Row, v: &Value) -> Row {
    match v["restart"].as_str() {
        Some(why) => row.disabled(format!("takes effect on restart: {why}")),
        None => row,
    }
}

#[cfg(test)]
mod tests {
    use super::super::fake::{Game, page, row};
    use super::*;
    use crate::model::Edit;

    #[test]
    fn debug_views_have_a_range_that_steps_by_eight() {
        let mut g = Game::default();
        let rows = page(&mut g, "view");
        let labels: Vec<_> = rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Hitboxes",
                "Range (world units)",
                "Trace lifetime (ticks)",
                "Take screenshot",
                "Render passes",
                "Graphics settings"
            ]
        );
        let range = row(&rows, "Range (world units)");
        assert_eq!(range.stepped(1), Some(json!(56)));
        assert_eq!(range.stepped(-1), Some(json!(40)));
        assert_eq!(rows[0].set.as_ref().unwrap().call(json!(true)).to_string(), r#"debug.view {"name":"hitboxes","on":true}"#);
        assert_eq!(
            row(&rows, "Graphics settings").edit,
            Edit::Open("settings/video".into())
        );
    }

    #[test]
    fn a_view_the_game_cannot_draw_is_greyed_with_why() {
        let a: Vec<Answer> = vec![Some(Ok(json!({"views": [
            {"name": "hitboxes", "label": "Hitboxes", "on": false},
            {"name": "traces", "label": "Traces", "on": false, "why": "entry unavailable"},
            {"name": "kill_volumes", "label": "Kill volumes", "on": true, "why": "partly drawn"},
        ], "range": 48, "trace_ticks": 30})))];
        let rows = view_rows(&a);
        assert_eq!(row(&rows, "Hitboxes").disabled, None);
        assert_eq!(
            row(&rows, "Traces").disabled.as_deref(),
            Some("entry unavailable")
        );
        // One that is on stays switchable (to turn it off), with the note.
        let kill = row(&rows, "Kill volumes");
        assert_eq!(kill.disabled, None);
        assert_eq!(kill.about, "partly drawn");
    }

    #[test]
    fn take_screenshot_saves_the_newest_frame() {
        let mut g = Game::default();
        let rows = page(&mut g, "view");
        let r = row(&rows, "Take screenshot");
        assert!(r.disabled.is_none());
        let Edit::Action(call) = &r.edit else {
            panic!("an action")
        };
        assert_eq!(*call, Call::bare("host.screenshot"));
        let result = g.call(call).unwrap();
        assert_eq!(shot_toast(&result), "saved h2-123.png");
    }

    #[test]
    fn render_passes_leave_out_the_overrides_switch() {
        let mut g = Game::default();
        let rows = page(&mut g, "view/render");
        let labels: Vec<_> = rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, ["Fog", "Gain"]);
        assert_eq!(
            row(&rows, "Gain").set.as_ref().unwrap().call(json!(1.1)).to_string(),
            r#"render.param {"name":"gain","value":1.1}"#
        );
        // A tenth of the default.
        assert_eq!(row(&rows, "Gain").stepped(1), Some(json!(1.1)));
    }

    #[test]
    fn a_restart_only_switch_says_so() {
        let t = json!({"name": "fog", "on": false, "about": "", "restart": "read at map load"});
        assert_eq!(
            toggle_row(&t).disabled.as_deref(),
            Some("takes effect on restart: read at map load")
        );
        assert_eq!(nice_step(0.1), 0.1);
        assert_eq!(nice_step(0.25), 0.2);
    }
}
