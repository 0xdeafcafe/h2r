//! `world.object.respawn {definition}`: every placed object of one
//! definition remade, each linked to its placement as a load would make it
//! (docs/editor/realtime-edits.md, "Realtime coverage").
//!
//! The survey reads the placement blocks of every object type and keeps the
//! entries whose palette names the definition. An entry whose placement
//! [`super::read`] refuses (held, named, flagged, extra fields), or that no
//! live object occupies, is listed in `skipped` with the reason and left to
//! its next creation. A live object of the definition that no placement
//! made (an engine-made one, a Forge or spawn object) is listed in
//! `engine_made` and not touched. Each remade entry is made first, then the
//! old occupant is deleted, as `world.object.respawn` does.

use agent_proto::sim::Placement;
use agent_proto::{ObjectId, RpcError};
use guest_layout::GuestMemory;
use guest_layout::entry::{Cx, EntryError, GuestCall};
use serde_json::{Value, json};

use super::super::{Outcome, RespawnTarget, Shared, addr, lock, object_delete, prepare_spawn};
use super::{BLOCKS, Entry, Previews, fault, read, record, resolve};

/// One placement that names the definition, and what the engine path makes
/// of its record.
struct Candidate {
    block: &'static str,
    index: u16,
    entry: Result<Entry, RpcError>,
}

struct Survey {
    candidates: Vec<Candidate>,
    engine_made: Vec<ObjectId>,
}

/// The placements of every block that name `definition` (a tag datum
/// handle), and the live objects of it that no placement made.
fn survey(m: &dyn GuestMemory, definition: u32, previews: &Previews) -> Result<Survey, RpcError> {
    let scnr = m.u32(guest_layout::points::SCENARIO.base).map_err(fault)?;
    let mut candidates = Vec::new();
    if scnr != 0 {
        for &(block, _, ty) in BLOCKS {
            let Some((placements, palette, stride)) = tags::object::OBJECT_TYPES[ty as usize].placement
            else {
                continue;
            };
            let (Some(root), Some(palette_root)) = (
                scnr.checked_add(u32::from(placements)),
                scnr.checked_add(u32::from(palette)),
            ) else {
                continue;
            };
            let count = m.i32(root).map_err(fault)?.clamp(0, i32::from(i16::MAX)) as u16;
            let base = m.u32(root + 4).map_err(fault)?;
            if base == 0 {
                continue;
            }
            for index in 0..count {
                let at = base.saturating_add(u32::from(index) * u32::from(stride));
                let Ok(slot) = m.u16(at) else { continue };
                let Ok(row) = record(m, palette_root, 0x28, slot) else {
                    continue;
                };
                if m.u32(row + 4).map_err(fault)? != definition {
                    continue;
                }
                let target = RespawnTarget::Placement(Placement { block: block.into(), index });
                candidates.push(Candidate { block, index, entry: read(m, &target, previews) });
            }
        }
    }
    let mut engine_made = Vec::new();
    for obj in sim_state::objects(m).map_err(fault)? {
        let obj = obj.map_err(fault)?;
        let Some(ty) = obj.kind().map_err(fault)? else {
            continue;
        };
        if obj.flags().map_err(fault)?.deleting
            || obj.definition().map_err(fault)?.raw() != definition
            || obj.placement().map_err(fault)?.is_some()
        {
            continue;
        }
        // The kinds a placement block makes: `of_object_type` leaves out the
        // vehicles and machines, which have scans of their own.
        let Some(kind) = BLOCKS
            .iter()
            .find_map(|&(_, kind, t)| (t as u8 == ty as u8).then_some(kind))
        else {
            continue;
        };
        let id = ObjectId { kind, key: obj.id().0 };
        if previews.index(id).is_none() {
            engine_made.push(id);
        }
    }
    Ok(Survey { candidates, engine_made })
}

fn skip(block: &str, index: u16, why: impl Into<String>) -> Value {
    json!({ "placement": { "block": block, "index": index }, "why": why.into() })
}

/// [`Call::ObjectRespawnDefinition`](super::super::Call).
pub(in crate::sim::calls) async fn run(
    cx: &Cx,
    scratch: u32,
    queue: &Shared,
    definition: u32,
) -> Result<Outcome, EntryError> {
    let survey = match cx.read(|m| survey(m, definition, &lock(queue).previews)) {
        Ok(s) => s,
        Err(e) => return Ok(Err(e)),
    };
    let mut respawned = Vec::new();
    let mut skipped = Vec::new();
    for c in survey.candidates {
        let entry = match c.entry {
            Ok(entry) => entry,
            Err(e) => {
                skipped.push(skip(c.block, c.index, e.message));
                continue;
            }
        };
        let Some(old) = entry.old else {
            skipped.push(skip(c.block, c.index, "no live object holds this placement"));
            continue;
        };
        prepare_spawn(cx, scratch, entry.tag, entry.pos, Some(entry.axes)).await?;
        cx.write(|m| m.write(scratch + addr::PLACEMENT_INDEX, &c.index.to_le_bytes()))?;
        let made = cx.call(GuestCall::new(addr::OBJECT_NEW).args([scratch])).await?.eax;
        if made == u32::MAX {
            skipped.push(skip(c.block, c.index, "object_new refused the replacement"));
            continue;
        }
        let object = ObjectId { kind: entry.kind, key: made };
        // The replacement exists; the old occupant goes after it, so a
        // refused `object_new` never leaves a placement empty.
        let _ = object_delete(cx, queue, old).await?;
        let linked = cx
            .read(|m| resolve(m, object).and_then(|b| b.placement_index(m).map_err(fault)))
            .is_ok_and(|p| p == c.index as i16);
        respawned.push(json!({
            "placement": { "block": c.block, "index": c.index },
            "old": old.to_string(),
            "object": object.to_string(),
            "linked": linked,
        }));
    }
    Ok(Ok(json!({
        "definition": format!("{definition:08x}"),
        "respawned": respawned,
        "skipped": skipped,
        "engine_made": survey.engine_made.iter().map(ToString::to_string).collect::<Vec<_>>(),
    })))
}
