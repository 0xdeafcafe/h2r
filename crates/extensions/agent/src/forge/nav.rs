//! `forge.nav` and the `nav` field of `forge.status`
//! (docs/runtime/agent-api.md, "Local Forge session"; the nav itself is
//! docs/game/forge-nav.md).
//!
//! The session holds the last generation's report; what only the game
//! knows (a generator is installed, a generation is running) comes in as
//! [`Facts`].
use ::forge::Session;
use serde_json::{Value, json};

/// What `forge.nav {generate: true}` says when the game has no generator.
pub const NO_GENERATOR: &str = "This game cannot generate nav.";

/// What the game knows of the nav besides the session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    /// The game has a generator installed.
    pub available: bool,
    /// A generation is running; edits wait for it.
    pub generating: bool,
}

/// `forge.status`'s `nav`: the state in a few words.
pub fn status(session: &Session, facts: Facts) -> Value {
    let mut out = json!({
        "state": session.nav_state().name(),
        "available": facts.available,
        "generating": facts.generating,
        "problems": session.nav_report().map_or(0, |r| r.problems.len()),
    });
    if let Some(report) = session.nav_report() {
        out["sectors"] = json!(report.sectors);
        out["out_of_map"] = json!(report.out_of_map);
    }
    out
}

/// `forge.nav`: the state, the last generation's counts, its problems
/// and the overlay's lines.
pub fn reply(session: &Session, facts: Facts) -> Value {
    let report = session.nav_report();
    let problems: Vec<Value> = report.map_or_else(Vec::new, |r| {
        r.problems
            .iter()
            .map(|p| {
                json!({"kind": p.kind, "what": p.what, "position": p.position, "reason": p.reason})
            })
            .collect()
    });
    let lines: Vec<Value> = report.map_or_else(Vec::new, |r| {
        r.lines
            .iter()
            .map(|l| json!({"kind": l.kind, "a": l.a, "b": l.b}))
            .collect()
    });
    json!({
        "state": session.nav_state().name(),
        "available": facts.available,
        "generating": facts.generating,
        "counts": report.map(|r| json!({
            "sectors": r.sectors,
            "links": r.links,
            "out_of_map": r.out_of_map,
            "solids": r.solids,
        })),
        "problems": problems,
        "overlay": lines,
    })
}
