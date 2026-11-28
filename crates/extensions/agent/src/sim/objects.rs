//! `sim.objects` (docs/runtime/agent-api.md, "sim"): every live object
//! through `sim-state`'s typed views, the read side of the object update
//! rewrite (docs/engine/sim-rewrite-objects.md section 8). No offsets
//! here: the views own them.

use agent_proto::sim::objects::{
    ObjectsParams, ObjectsReply, SimObject, SimSphere, SimUnit, SimVitality, handle,
};
use agent_proto::{RpcError, code};
use guest_layout::GuestMemory;
use sim_state::{ObjectFlags, ObjectKind, ObjectView};

fn fault(e: guest_layout::MemFault) -> RpcError {
    RpcError::new(
        code::UNAVAILABLE,
        format!("guest memory at {:#x} faulted", e.addr),
    )
}

/// The kinds `p.kinds` names, refusing a group that is not one.
fn kinds(p: &ObjectsParams) -> Result<Vec<ObjectKind>, RpcError> {
    p.kinds
        .iter()
        .map(|g| {
            ObjectKind::ALL
                .into_iter()
                .find(|k| k.group() == g.as_str())
                .ok_or_else(|| {
                    RpcError::new(
                        code::INVALID_PARAMS,
                        format!("kinds: {g:?} is not an object tag group"),
                    )
                })
        })
        .collect()
}

fn flags(f: ObjectFlags) -> Vec<String> {
    [
        (f.active, "active"),
        (f.awake, "awake"),
        (f.post, "post"),
        (f.new, "new"),
        (f.deleting, "deleting"),
        (f.defer, "defer"),
        (f.attach, "attach"),
        (f.child, "child"),
    ]
    .into_iter()
    .filter(|(on, _)| *on)
    .map(|(_, name)| name.to_owned())
    .collect()
}

fn datum(d: guest_layout::Datum) -> Option<String> {
    d.get().map(handle)
}

fn object(o: &ObjectView<'_, dyn GuestMemory + '_>) -> Result<SimObject, guest_layout::MemFault> {
    let t = o.transform()?;
    let sphere = o.bounding_sphere()?;
    let v = o.vitality()?;
    let unit = match o.as_unit()? {
        None => None,
        Some(u) => Some(SimUnit {
            player: u.player()?.and_then(datum),
            actor: u.actor()?.and_then(datum),
            team: u.team()?,
            seat: u.seat()?,
            weapons: u.weapons()?.iter().map(|w| w.map(|id| handle(id.0))).collect(),
            held: u.held_weapon()?.map(|id| handle(id.0)),
        }),
    };
    Ok(SimObject {
        id: handle(o.id().0),
        kind: o.kind()?.map(|k| k.group().to_owned()),
        definition: datum(o.definition()?),
        flags: flags(o.flags()?),
        parent: o.parent()?.map(|id| handle(id.0)),
        position: t.position,
        forward: t.forward,
        up: t.up,
        scale: t.scale,
        velocity: o.velocity()?,
        sphere: SimSphere { center: sphere.center, radius: sphere.radius },
        vitality: SimVitality { body: v.body, shield: v.shield, depleted: v.depleted() },
        nodes: o.node_count()?,
        unit,
    })
}

/// The answer over `m`.
pub fn list(m: &(dyn GuestMemory + '_), p: &ObjectsParams) -> Result<ObjectsReply, RpcError> {
    let kinds = kinds(p)?;
    let max = p.max.unwrap_or(ObjectsReply::MAX).min(ObjectsReply::MAX);
    let mut objects = Vec::new();
    let mut truncated = false;
    for o in sim_state::objects(m).map_err(fault)? {
        let o = o.map_err(fault)?;
        if !kinds.is_empty() && !o.kind().map_err(fault)?.is_some_and(|k| kinds.contains(&k)) {
            continue;
        }
        if objects.len() == max {
            truncated = true;
            break;
        }
        objects.push(object(&o).map_err(fault)?);
    }
    Ok(ObjectsReply { objects, truncated })
}
