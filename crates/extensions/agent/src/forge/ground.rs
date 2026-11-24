//! `forge.ground`, `forge.ground.build` and the `ground` field of
//! `forge.status` (docs/runtime/agent-api.md, "Local Forge session"; the
//! ground itself is docs/game/forge-map-variants.md, "AI ground").
//!
//! The session holds the markers, the recipe and the last build's report;
//! what only the game knows (a builder is installed, a build is running, the
//! built map is the loaded one) comes in as [`Facts`].
use ::forge::{GroundLabel, Session};
use serde_json::{Value, json};

/// What `forge.ground.build` says when the game has no builder.
pub const NO_BUILDER: &str = "This game cannot build AI ground.";

/// What the game knows of the ground besides the session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    /// The game has a builder installed.
    pub available: bool,
    /// A build is running; edits wait for it.
    pub building: bool,
    /// The last build's map is the one running.
    pub loaded: bool,
}

fn problems(session: &Session) -> Vec<Value> {
    session.ground_report().map_or_else(Vec::new, |report| {
        report
            .problems
            .iter()
            .map(|p| json!({"label": p.label, "position": p.position, "reason": p.reason}))
            .collect()
    })
}

fn report(session: &Session) -> Value {
    let Some(report) = session.ground_report() else {
        return Value::Null;
    };
    json!({
        "zones": report.zones,
        "squads": report.squads,
        "problems": report.problems.len(),
        "receipt": report.receipt.as_ref().map(|r| json!({
            "base": r.base, "labels": r.labels, "kit_version": r.kit_version,
        })),
    })
}

/// `forge.status`'s `ground`: the state in a few words.
pub fn status(session: &Session, facts: Facts) -> Value {
    let mut out = json!({
        "state": session.ground_state().name(),
        "building": facts.building,
        "available": facts.available,
        "loaded": facts.loaded,
        "markers": session.ground_counts().iter().sum::<usize>(),
        "problems": session.ground_report().map_or(0, |r| r.problems.len()),
    });
    if let Some(report) = session.ground_report() {
        out["zones"] = json!(report.zones);
        out["squads"] = json!(report.squads);
    }
    out
}

/// `forge.ground`: the state, the recipe, how many of each label are placed
/// and where, and the last build's report with its problems and receipt.
pub fn reply(session: &Session, facts: Facts) -> Value {
    let counts: serde_json::Map<String, Value> = GroundLabel::ALL
        .iter()
        .zip(session.ground_counts())
        .map(|(label, n)| (label.text().to_owned(), json!(n)))
        .collect();
    let markers: Vec<Value> = session
        .markers()
        .unwrap_or_default()
        .iter()
        .map(|m| {
            json!({
                "id": m.id, "label": m.label.text(), "number": m.number,
                "position": m.frame.position, "facing": m.frame.rotation[0], "team": m.team,
            })
        })
        .collect();
    json!({
        "state": session.ground_state().name(),
        "building": facts.building,
        "available": facts.available,
        "loaded": facts.loaded,
        "recipe": session.ground_recipe().map(|r| json!({
            "kit_version": r.kit_version, "home": r.home, "probe": r.probe,
        })),
        "counts": counts,
        "markers": markers,
        "problems": problems(session),
        "report": report(session),
    })
}
