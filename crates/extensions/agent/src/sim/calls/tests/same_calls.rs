//! A Forge edit and the agent method it stands for make the same guest
//! calls: each runs over the same fake game and the two traces are
//! compared call for call, with the vectors each call reads.
//! (docs/game/forge-design.md, "Slices"; docs/runtime/agent-api.md,
//! "Object edits").
use super::*;
use crate::forge::base::{BaseStep, RowBytes, inert_row};
use crate::forge::local::Guard;
use ::forge::BlockKind;

/// The scenario `placed_object` and `forge_vehicle` load.
const SCENARIO: u32 = 0x8100_0000;
const MADE: u32 = 0x0042_0003;

fn guard() -> Guard {
    Guard {
        scenario: SCENARIO,
        generation: 0,
        admitted: |_, _| true,
    }
}

/// One guest call and the vectors it reads: an object move's position,
/// forward and up (each given register), or a made object's placement
/// position, forward, up and placement index.
#[derive(Debug, PartialEq)]
struct Seen {
    call: GuestCall,
    vectors: Vec<[f32; 3]>,
    index: Option<u16>,
}

/// Runs `call` with the callees the game plays: `placement_data_new`
/// leaves the placement unlinked, `object_new` makes [`MADE`].
fn trace(w: World, call: Call) -> (Outcome, Vec<Seen>) {
    let mut seen = Vec::new();
    let (out, calls, _) = one(w, call, |c, m| {
        let mut vectors = Vec::new();
        let mut index = None;
        match c.target {
            addr::OBJECT_SET_POSITION => {
                for at in [c.eax, c.ecx, c.edx].into_iter().flatten() {
                    if at != 0 {
                        vectors.push(vec3(m, at));
                    }
                }
            }
            addr::PLACEMENT_NEW => {
                m.set_u16(c.esi.unwrap() + addr::PLACEMENT_INDEX, u16::MAX)
                    .unwrap();
            }
            addr::OBJECT_NEW => {
                let p = c.args[0];
                for at in [
                    addr::PLACEMENT_POSITION,
                    addr::PLACEMENT_FORWARD,
                    addr::PLACEMENT_UP,
                ] {
                    vectors.push(vec3(m, p + at));
                }
                index = Some(m.u16(p + addr::PLACEMENT_INDEX).unwrap());
            }
            _ => {}
        }
        seen.push(Seen { call: c.clone(), vectors, index });
        if c.target == addr::OBJECT_NEW { al(MADE) } else { al(0) }
    });
    assert_eq!(calls, seen.iter().map(|s| s.call.clone()).collect::<Vec<_>>());
    (out, seen)
}

#[test]
fn a_forge_move_makes_the_guest_calls_of_world_object_set() {
    let frame = ::forge::Frame {
        position: [2.0, -3.0, 4.5],
        rotation: [0.75, -0.25, 0.5],
    };
    let (forward, up) = forge_axes(&frame);
    let (w, id) = vehicle();
    let (agent, agent_calls) = trace(
        w,
        Call::ObjectSet {
            id,
            pos: Some(frame.position),
            forward: Some(forward),
            up: Some(up),
        },
    );
    let (w, id, first) = forge_vehicle();
    let (forge, forge_calls) = trace(
        w,
        Call::Forge {
            guard: first,
            steps: vec![ForgeStep::Move { target: ForgeTarget::Live(id), frame }],
        },
    );
    assert_eq!(agent.unwrap()["object"], id.to_string());
    assert_eq!(forge.unwrap()["handles"], json!([]));
    assert_eq!(agent_calls.len(), 1);
    assert_eq!(agent_calls[0].vectors, [frame.position, forward, up]);
    assert_eq!(forge_calls, agent_calls);
}

#[test]
fn a_forge_base_move_makes_the_guest_calls_of_world_object_set() {
    let frame = ::forge::Frame {
        position: [1.0, 2.0, 3.0],
        rotation: [0.5, 0.25, 0.125],
    };
    let (forward, up) = forge_axes(&frame);
    let (w, id) = placed_vehicle();
    let (_, agent_calls) = trace(
        w,
        Call::ObjectSet {
            id,
            pos: Some(frame.position),
            forward: Some(forward),
            up: Some(up),
        },
    );
    let (w, _) = placed_vehicle();
    let (forge, forge_calls) = trace(
        w,
        Call::Forge {
            guard: guard(),
            steps: vec![ForgeStep::Base(BaseStep::Move {
                block: BlockKind::Vehicles,
                index: 0,
                frame,
            })],
        },
    );
    assert_eq!(forge.unwrap()["handles"], json!([]));
    assert_eq!(forge_calls, agent_calls);
}

#[test]
fn a_forge_delete_makes_the_guest_calls_of_world_object_delete() {
    let (w, id) = vehicle();
    let (agent, agent_calls) = trace(w, Call::ObjectDelete { id });
    let (w, id, first) = forge_vehicle();
    let (forge, forge_calls) = trace(
        w,
        Call::Forge {
            guard: first,
            steps: vec![ForgeStep::Delete { target: ForgeTarget::Live(id) }],
        },
    );
    assert_eq!(agent.unwrap()["deleted"], id.to_string());
    assert_eq!(forge.unwrap()["handles"], json!([]));
    assert_eq!(agent_calls.len(), 1);
    assert_eq!(agent_calls[0].call.target, addr::OBJECT_DELETE);
    assert_eq!(forge_calls, agent_calls);

    // An own entry's removal deletes its live object the same way.
    let (w, id) = placed_vehicle();
    let (_, agent_calls) = trace(w, Call::ObjectDelete { id });
    let (w, _) = placed_vehicle();
    let (forge, forge_calls) = trace(
        w,
        Call::Forge {
            guard: guard(),
            steps: vec![ForgeStep::Base(BaseStep::Inert {
                block: BlockKind::Vehicles,
                index: 0,
            })],
        },
    );
    assert_eq!(forge.unwrap()["handles"], json!([]));
    assert_eq!(forge_calls, agent_calls);
}

/// The placed vehicle with no live object holding its placement.
fn without_object() -> World {
    let (mut w, _) = placed_vehicle();
    w.g.put_u16(
        UNIT_BODY + guest_layout::Object::PLACEMENT_INDEX.offset,
        u16::MAX,
    );
    w
}

#[test]
fn a_forge_restore_makes_the_guest_calls_of_world_object_respawn_and_links_the_object() {
    let (_, agent_calls) = trace(without_object(), respawn(vehicle_placement()));

    let mut w = without_object();
    let kept = w.g.bytes(ENTRY, 0x54).unwrap();
    w.g.put(ENTRY, &inert_row(BlockKind::Vehicles));
    let (forge, forge_calls) = trace(
        w,
        Call::Forge {
            guard: guard(),
            steps: vec![ForgeStep::Base(BaseStep::Restore {
                block: BlockKind::Vehicles,
                index: 0,
                row: RowBytes::new(BlockKind::Vehicles, &kept).unwrap(),
            })],
        },
    );
    assert_eq!(forge.unwrap()["handles"], json!([]));
    let targets = |seen: &[Seen]| seen.iter().map(|s| s.call.target).collect::<Vec<_>>();
    assert_eq!(targets(&agent_calls), [addr::PLACEMENT_NEW, addr::OBJECT_NEW]);
    // The same calls with the same arguments and the same frame ...
    for (forge, agent) in forge_calls.iter().zip(&agent_calls) {
        assert_eq!(forge.call, agent.call);
        assert_eq!(forge.vectors, agent.vectors);
    }
    assert_eq!(forge_calls.len(), agent_calls.len());
    // ... and the restore links the object to its entry, as
    // `world.placement.edit`'s create does; the respawn's stays unlinked.
    assert_eq!(agent_calls[1].index, Some(u16::MAX));
    assert_eq!(forge_calls[1].index, Some(0));
}

#[test]
fn a_forge_make_makes_the_guest_calls_of_sim_spawn() {
    let frame = ::forge::Frame {
        position: [5.0, 6.0, 7.0],
        rotation: [0.0; 3],
    };
    let tag = 0xe200_0002;
    let (agent, agent_calls) = trace(
        world(),
        Call::Spawn {
            tag,
            pos: frame.position,
            facing: Some([1.0, 0.0, 0.0]),
            mp_scenario: None,
        },
    );
    let mut w = world();
    w.g.put_u32(guest_layout::global::SCENARIO.base, SCENARIO);
    let (forge, forge_calls) = trace(
        w,
        Call::Forge {
            guard: guard(),
            steps: vec![ForgeStep::Make { tag, frame }],
        },
    );
    assert_eq!(agent.unwrap()["object"], "00420003");
    assert_eq!(forge.unwrap()["handles"], json!([MADE]));
    assert_eq!(forge_calls, agent_calls);
}
