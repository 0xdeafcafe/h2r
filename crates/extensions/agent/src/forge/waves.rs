//! `forge.waves` and the `waves` of `forge.status`' `variant`
//! (docs/runtime/agent-api.md, "Local Forge session"; the set itself is
//! docs/game/forge-firefight.md, "The wave set").
//!
//! The session keeps the set beside its document; the reply reads it from
//! the session. Replacing it is the game's ([`super::local::LocalApi::set_waves`]),
//! which holds the session.
use ::forge::Session;
use forge_doc::waves::{Def, MAX_WAVES_BYTES};
use serde_json::{Value, json};

/// What `forge.waves {set}` says when the game cannot edit the session.
pub const NO_EDIT: &str = "This game cannot edit the wave set.";

/// `forge.waves`: the set the session plays and its `Def::id`, whether it
/// is the default (Holdout, the session having none of its own), the
/// problems against the ground, and the drops in zone order.
pub fn reply(session: &Session) -> Value {
    let def = session.waves_or_default();
    let problems: Vec<Value> = session
        .wave_problems()
        .iter()
        .map(|p| {
            json!({
                "round": p.round, "wave": p.wave, "squad": p.squad,
                "what": p.what, "sentence": p.sentence(&def),
            })
        })
        .collect();
    let markers = session.markers().unwrap_or_default();
    let drops: Vec<Value> = session
        .drops()
        .into_iter()
        .enumerate()
        .map(|(zone, id)| {
            let marker = markers.iter().find(|m| m.id == id);
            json!({
                "zone": zone,
                "id": id,
                "position": marker.map(|m| m.frame.position),
                "facing": marker.map(|m| m.frame.rotation[0]),
            })
        })
        .collect();
    let id = def.id().ok();
    json!({
        "name": def.name,
        "id": id,
        "default": session.waves_are_default(),
        "set": def,
        "problems": problems,
        "drops": drops,
    })
}

/// `forge.status`' `variant.waves`: the session's own set's name, or null
/// when it plays the default and a save writes none.
pub fn status(session: &Session) -> Value {
    json!(session.waves().map(|d| d.name.as_str()))
}

/// The definition `forge.waves {set}` names: `null` clears the set. The
/// set's size is checked before it is decoded, then its shape; the
/// validator is the session's ([`Session::set_waves`]).
pub fn parse(set: Value) -> Result<Option<Def>, String> {
    if set.is_null() {
        return Ok(None);
    }
    let bytes = serde_json::to_vec(&set).map_err(|e| e.to_string())?;
    if bytes.len() > MAX_WAVES_BYTES {
        return Err(forge_doc::Refusal::Limit {
            what: "waves bytes".into(),
            max: MAX_WAVES_BYTES,
        }
        .to_string());
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|e| format!("waves: not a Firefight definition: {e}"))
}

/// Replace or clear `session`'s set from `forge.waves {set}`'s value, and
/// answer the reply. A refusal changes nothing.
pub fn apply(session: &mut Session, set: Value) -> Result<Value, String> {
    let def = parse(set)?;
    session.set_waves(def).map_err(|e| e.to_string())?;
    Ok(reply(session))
}
