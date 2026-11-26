//! `forge.pieces` (docs/runtime/agent-api.md, "Local Forge session"; the
//! pieces themselves are docs/game/forge-blockout.md).
//!
//! The session knows each piece and its state, so the reply is read from
//! it: a piece is `live` (an object of its definition's `bloc` stands),
//! `slot` (an object of a spare definition, slice BO10) or `pending` (in
//! the document with no object, until **Rebuild map**).
use ::forge::Session;
use serde_json::{Value, json};

/// `forge.pieces`: `{pieces, slots, pending}`. Each piece is
/// `{target, def, name, state, position, rotation, bounds, slot?}`, in the
/// order of the crates block: `target` is the endpoint a `Forge::Piece` op
/// names (`{added: op}`), `def` the definition as the document writes it,
/// `name` its canonical name (`forge\blockout\<name>` is its `bloc`),
/// `bounds` the box it fills, `{min, max}` in world units. `slots` lists
/// the spare definitions in use, and `pending` counts the pieces with no
/// object.
pub fn reply(session: &Session) -> Value {
    let pieces: Vec<Value> = session
        .pieces()
        .into_iter()
        .map(|p| {
            json!({
                "target": p.target(),
                "def": p.def,
                "name": p.name,
                "state": p.state.name(),
                "position": p.frame.position,
                "rotation": p.frame.rotation,
                "bounds": {"min": p.bounds.min, "max": p.bounds.max},
            })
        })
        .collect();
    json!({
        "pieces": pieces,
        "slots": [],
        "pending": session.pending_pieces(),
    })
}
