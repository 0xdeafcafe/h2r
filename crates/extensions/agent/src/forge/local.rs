//! Local Forge sessions submit edits at a tick boundary. The ordinary
//! tools and the agent share the same host; the call queue owns guest entry.
use std::collections::BTreeMap;
use std::sync::{Arc, mpsc};

use ::forge::{BlockKind, Frame, PaletteItem, Session, World};
use agent_proto::{Boundary, Kind, ObjectId, RpcError, code, forge::method};
use engine::GuestMemory;
use forge_doc::{Entry, ForgeDoc, Op, Sha256, TagKey};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::sim::calls::{self, Call, ForgeStep, ForgeTarget};
use crate::{Access, Agent};

/// Rechecked at guest entry, after submission. The host checks the generation,
/// local multiplayer session, map and playback state in its admission predicate.
#[derive(Debug, Clone, Copy)]
pub struct Guard {
    pub scenario: u32,
    pub generation: u64,
    pub admitted: fn(&dyn GuestMemory, u64) -> bool,
}

impl PartialEq for Guard {
    fn eq(&self, other: &Self) -> bool {
        self.scenario == other.scenario
            && self.generation == other.generation
            && std::ptr::fn_addr_eq(self.admitted, other.admitted)
    }
}

impl Guard {
    pub(crate) fn allows(self, m: &dyn GuestMemory) -> bool {
        self.scenario != 0
            && guest_layout::global::SCENARIO.pointer(m) == Ok(Some(self.scenario))
            && (self.admitted)(m, self.generation)
    }
}

/// Why a Forge edit's live object can no longer be edited: the session's
/// handle no longer resolves with its kind and salt (`world.object.*`'s
/// check). The edit leaves the document as it was.
pub const OBJECT_GONE: &str = "forge_object_gone";

/// A live object edit names a block whose objects `world.object.*` cannot
/// resolve.
pub const OBJECT_KIND: &str = "forge_object_kind";

/// The `world.object` kind of a block's live objects.
fn live_kind(block: BlockKind) -> Option<Kind> {
    Some(match block {
        BlockKind::Scenery => Kind::Scenery,
        BlockKind::Vehicles => Kind::Vehicle,
        BlockKind::Equipment => Kind::Equipment,
        BlockKind::Weapons => Kind::Weapon,
        BlockKind::Machines => Kind::Machine,
        BlockKind::Crates => Kind::Crate,
        _ => return None,
    })
}

enum Staged {
    Make(TagKey, Frame),
    Move(ForgeTarget, Frame),
    Delete(ForgeTarget),
}

/// A validated command can build a candidate document and log on a private
/// session clone. Its engine work becomes one [`Call::Forge`] at the next
/// tick; engine denial discards the entire candidate.
#[derive(Default)]
pub struct ForgeWorld {
    steps: Vec<Staged>,
    made: u16,
    /// The block of each handle the session holds or this edit makes.
    blocks: BTreeMap<u32, BlockKind>,
}

impl ForgeWorld {
    pub fn apply<T>(
        session: &Session,
        queue: &calls::Shared,
        guard: Guard,
        mut resolve: impl FnMut(&TagKey) -> Result<u32, String>,
        edit: impl FnOnce(&mut Session, &mut Self) -> Result<T, String>,
    ) -> Result<ForgeEdit<T>, String> {
        let mut candidate = session.clone();
        let mut world = ForgeWorld {
            blocks: session
                .objects()
                .iter()
                .map(|object| (object.handle, object.item.block))
                .collect(),
            ..Default::default()
        };
        let result = edit(&mut candidate, &mut world)?;
        let count = usize::from(world.made);
        let pending = if world.steps.is_empty() {
            None
        } else {
            let steps = world
                .steps
                .into_iter()
                .map(|step| {
                    Ok(match step {
                        Staged::Make(tag, frame) => ForgeStep::Make {
                            tag: resolve(&tag)?,
                            frame,
                        },
                        Staged::Move(target, frame) => ForgeStep::Move { target, frame },
                        Staged::Delete(target) => ForgeStep::Delete { target },
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            Some(calls::submit(queue, Call::Forge { guard, steps })?)
        };
        Ok(ForgeEdit {
            candidate: Some((candidate, result)),
            count,
            pending,
        })
    }

    fn make(&mut self, item: &PaletteItem, frame: Frame) -> Result<u32, String> {
        let handle = staged_handle(self.made);
        self.made = self.made.checked_add(1).ok_or("too many placements")?;
        self.blocks.insert(handle, item.block);
        self.steps.push(Staged::Make(item.tag.clone(), frame));
        Ok(handle)
    }

    fn target(&self, handle: u32) -> Result<ForgeTarget, String> {
        let block = *self
            .blocks
            .get(&handle)
            .ok_or_else(|| format!("{OBJECT_GONE}: the session holds no object {handle:#010x}"))?;
        let kind = live_kind(block).ok_or_else(|| {
            format!(
                "{OBJECT_KIND}: live {} objects cannot be moved or deleted",
                block.name().replace('_', " ")
            )
        })?;
        Ok(if handle as u16 == u16::MAX {
            ForgeTarget::Made {
                index: (handle >> 16) as usize,
                kind,
            }
        } else {
            ForgeTarget::Live(ObjectId { kind, key: handle })
        })
    }
}

/// Each call stages one engine step. Moves, deletes and restores apply to the
/// live object through the `world.object.set` and `.delete` bodies and the
/// ownerless spawn, at the tick boundary the queue runs them.
impl World for ForgeWorld {
    fn place(&mut self, item: &PaletteItem, frame: Frame) -> Result<u32, String> {
        self.make(item, frame)
    }

    fn move_object(&mut self, handle: u32, frame: Frame) -> Result<(), String> {
        let target = self.target(handle)?;
        self.steps.push(Staged::Move(target, frame));
        Ok(())
    }

    fn delete(&mut self, handle: u32) -> Result<(), String> {
        let target = self.target(handle)?;
        self.steps.push(Staged::Delete(target));
        Ok(())
    }

    fn respawn(&mut self, item: &PaletteItem, frame: Frame) -> Result<u32, String> {
        self.make(item, frame)
    }
}

/// Slot 0xffff is outside the engine's object pool. Different salt words
/// distinguish objects in a private session while one atomic load runs.
fn staged_handle(index: u16) -> u32 {
    u32::from(index) << 16 | 0xffff
}

/// The host serializes editing until this result is ready or the map ends.
/// Dropping it withdraws a placement the game has yet to start.
pub struct ForgeEdit<T> {
    candidate: Option<(Session, T)>,
    pending: Option<calls::PendingCall>,
    count: usize,
}

impl<T> ForgeEdit<T> {
    /// Returns the candidate once, and only on confirmed success.
    pub fn poll(&mut self) -> Option<Result<(Session, T), String>> {
        let handles = if let Some(reply) = &self.pending {
            let made = reply.outcome()?;
            self.pending = None;
            match made
                .map_err(|e| e.message)
                .and_then(|made| spawned_handles(made, self.count))
            {
                Ok(handles) => handles,
                Err(err) => {
                    self.candidate = None;
                    return Some(Err(err));
                }
            }
        } else {
            Vec::new()
        };
        let (mut session, result) = self.candidate.take()?;
        for (i, handle) in handles.into_iter().enumerate() {
            session.bind_spawn(staged_handle(i as u16), handle);
        }
        Some(Ok((session, result)))
    }
}

fn spawned_handles(made: Value, expected: usize) -> Result<Vec<u32>, String> {
    #[derive(Deserialize)]
    struct Made {
        handles: Vec<u32>,
    }
    let made: Made =
        serde_json::from_value(made).map_err(|e| format!("invalid spawn result: {e}"))?;
    if made.handles.len() != expected || made.handles.iter().any(|h| *h as u16 == u16::MAX) {
        return Err("object_new did not return each new object's handle".into());
    }
    Ok(made.handles)
}

/// Agent proposals complete through the same tick-boundary host as pad edits.
pub type ProposalReply = mpsc::Receiver<Result<Entry, String>>;

pub trait LocalApi: Send + Sync + 'static {
    fn status(&self) -> Result<Value, String>;
    fn document(&self) -> Result<ForgeDoc, String>;
    fn log(&self, since: u32) -> Result<Vec<Entry>, String>;
    fn input(&self, m: &dyn GuestMemory, input: ::forge::tool::Input) -> Result<(), String>;
    fn propose(&self, tick: u32, op: Op, was: Option<Sha256>) -> Result<ProposalReply, String>;
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Propose {
    op: Op,
    was: Option<Sha256>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Log {
    #[serde(default)]
    since: u32,
}

fn empty(v: Value) -> Result<(), RpcError> {
    if v.is_null() || v.as_object().is_some_and(|v| v.is_empty()) {
        Ok(())
    } else {
        Err(super::invalid("this Forge read takes no parameters"))
    }
}

fn tool_input(v: Value) -> Result<::forge::tool::Input, RpcError> {
    use ::forge::tool::{Action, Input, TURN_RATE};
    use agent_proto::forge::InputParams;
    let input = serde_json::from_value(v).map_err(|e| super::invalid(e.to_string()))?;
    match input {
        InputParams::Button { action } => Ok(Input {
            action: Some(
                Action::named(&action).ok_or_else(|| super::invalid("Unknown Forge action."))?,
            ),
            ..Default::default()
        }),
        InputParams::Analog {
            movement,
            look,
            roll,
            rotating,
        } => {
            if movement.is_none() && look.is_none() && roll.is_none() {
                return Err(super::invalid("Supply movement, look or roll."));
            }
            let movement = movement.unwrap_or_default();
            let look = look.unwrap_or_default();
            let roll = roll.unwrap_or_default();
            if !movement
                .into_iter()
                .chain(look)
                .chain([roll])
                .all(|v| (-1.0..=1.0).contains(&v))
            {
                return Err(super::invalid("Forge input axes must be between -1 and 1."));
            }
            if roll != 0.0 && !rotating {
                return Err(super::invalid("Roll requires rotating."));
            }
            let seconds = 1. / 30.;
            Ok(Input {
                movement,
                look: look.map(|v| v * TURN_RATE * seconds),
                roll: roll * TURN_RATE * seconds,
                seconds,
                rotating,
                ..Default::default()
            })
        }
    }
}

pub fn install(agent: &Agent, backend: impl LocalApi) {
    let backend = Arc::new(backend);
    let host = backend.clone();
    agent.add_game_method(method::STATUS, Access::Read, move |v, _| {
        empty(v)?;
        host.status().map_err(super::unavailable)
    });
    let host = backend.clone();
    agent.add_game_method(method::DOC, Access::Read, move |v, _| {
        empty(v)?;
        host.document()
            .map(|doc| json!({"document":doc}))
            .map_err(super::unavailable)
    });
    let host = backend.clone();
    agent.add_game_method(method::LOG, Access::Read, move |v, _| {
        let p = if v.is_null() {
            Log::default()
        } else {
            serde_json::from_value(v).map_err(|e| super::invalid(e.to_string()))?
        };
        host.log(p.since)
            .map(|entries| json!({"entries":entries}))
            .map_err(super::unavailable)
    });
    let host = backend.clone();
    agent.add_game_method(method::INPUT, Access::Act, move |v, ctx| {
        if ctx.at.boundary != Boundary::Tick
            || ctx.at.mode.as_deref() != Some("multiplayer")
            || ctx.at.tick.is_none()
        {
            return Err(super::unavailable(
                "Forge input requires a local multiplayer tick.",
            ));
        }
        let input = tool_input(v)?;
        ctx.with_guest(|m| host.input(m, input).map_err(refused))?;
        Ok(json!({"queued":true}))
    });
    agent.add_game_method(method::PROPOSE, Access::Act, move |v, ctx| {
        if ctx.at.boundary != Boundary::Tick || ctx.at.mode.as_deref() != Some("multiplayer") {
            return Err(super::unavailable(
                "a Forge proposal requires a local multiplayer tick",
            ));
        }
        let tick = ctx
            .at
            .tick
            .and_then(|v| u32::try_from(v).ok())
            .ok_or_else(|| super::unavailable("the multiplayer tick is unavailable"))?;
        let p: Propose = serde_json::from_value(v).map_err(|e| super::invalid(e.to_string()))?;
        let reply = backend.propose(tick, p.op, p.was).map_err(refused)?;
        ctx.defer(
            30,
            Box::new(move |_| match reply.try_recv() {
                Ok(result) => Some(result.map(|entry| json!({"entry":entry})).map_err(refused)),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    Some(Err(super::unavailable("the Forge session ended")))
                }
            }),
        );
        Ok(Value::Null)
    });
}

fn refused(message: String) -> RpcError {
    RpcError::new(code::INVALID_REQUEST, message)
}
