//! The game's own functions, called for `sim.*`: teleports
//! (`sim.state.set`'s `pos` and facing), `sim.give` and `sim.spawn`
//! (docs/runtime/agent-api.md, "sim").
//!
//! A game method can't call into the guest: it runs in the tick observer,
//! after the tick. So it queues a [`Call`] on the [`Queue`] and defers its
//! answer ([`crate::GameCtx::defer`]). [`CallsFeature`], a `before` on
//! `game_tick`, finds the queue full at the start of the next tick, makes
//! the calls with an async body ([`run_queued`]), then runs the tick itself
//! (its chain runs again and, this time, continues), as our skulls do
//! (docs/game/skulls.md, "Calls into the game"). The method's deferred
//! answer reads the outcome after that tick.
//!
//! What each call does, all in the retail XBE:
//!
//! - **Teleport** is HaloScript `object_teleport`'s body (`0x29ffb0`,
//!   `SCRIPT_TELEPORT`) for a player's unit, with a position of our own in
//!   place of a cutscene flag's: the position goes to the co-op placement
//!   (`PLAYER_PLACE_NEAR`, `0x14e970`, EBX the player, ESI the point, the
//!   anchor -1, as `0x2a01ec..0x2a01f0` calls it), which takes the unit out
//!   of a vehicle (`0xe68c0` action `0x1e`) and places it through
//!   `0x14e200` a little above the point (`[0x4687b0]` times `[0x47f05c]`),
//!   so its physics hear of the move. A write of the body's position alone
//!   (`+0x64`) is undone by the biped's physics at the next tick. The
//!   facing: the unit's desired facing, aiming and looking vectors (`+0x150`,
//!   `+0x15c`, `+0x180`, as `0x2a019e..0x2a01d9` writes them), a local
//!   player's view (`0x1874b0`, ECX the local user, EAX the forward: yaw
//!   and pitch into the player controls at `[0x4ed284] + 0x14 + 0x94 *
//!   user`, `+0x10` and `+0x14`, as `0x2a022a` calls it), then
//!   `object_set_position` (`0xb75a0`: EAX position or 0, ECX forward, EDX
//!   up, stdcall `(object, 0, 0)`, `ret 0xc`; `0x2a0269`) with no position
//!   for the orientation.
//! - **Give** is how a starting profile (`0xcce78..0xcceae`) and an AI's
//!   equipment (`0x1e05c8..0x1e05f2`) give a weapon: `placement_data_new`
//!   (`0xb7930`, ESI the `0xc4`-byte placement, stdcall `(definition,
//!   owner unit, 0)`), `object_new` (`0xb7b40`, stdcall `(placement)`, the
//!   datum in EAX, -1 on failure), `unit_add_weapon` (`0xcd0c0`, stdcall
//!   `(unit, weapon, mode)`, `ret 0xc`, AL 0 when refused), and on a
//!   refusal `object_delete` (`0xb8540`). Its mode, from its jump table at
//!   `0xcd4bc`: 0 and 1 add the weapon and let the unit's own choice
//!   (`0xcdeb0`) say what it holds (the starting profile's 1); 2 first
//!   empties the unit's weapon slots (`0xccff0`, `0xcd14b`) and readies the
//!   new one (`+0x216` its slot, action 8 at `0xcd474`).
//! - **Spawn** is the netgame equipment spawn's (`0x15ab94..0x15abe6`):
//!   `placement_data_new` with owner -1, the position at placement `+0x1c`,
//!   forward and up at `+0x28` and `+0x34` (`0x11df60` fills them from the
//!   entry's angles at `0x15abcc`), then `object_new`.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use engine::GuestMemory;
use engine::entry::{Cx, EntryError, EntryFuture, GuestCall, Ret};
use engine::handle::DatumHandle;
use engine::view::{objects, players};
use serde_json::{Value, json};

/// The addresses, with the evidence in the module doc.
pub mod addr {
    pub const PLACEMENT_NEW: u32 = 0x000b_7930;
    /// The placement `placement_data_new` fills.
    pub const PLACEMENT_SIZE: u32 = 0xc4;
    pub const PLACEMENT_POSITION: u32 = 0x1c;
    pub const PLACEMENT_FORWARD: u32 = 0x28;
    pub const PLACEMENT_UP: u32 = 0x34;
    pub const OBJECT_NEW: u32 = 0x000b_7b40;
    pub const OBJECT_DELETE: u32 = 0x000b_8540;
    pub const UNIT_ADD_WEAPON: u32 = 0x000c_d0c0;
    pub const PLAYER_PLACE_NEAR: u32 = engine::points::PLAYER_PLACE_NEAR.base;
    pub const PLAYER_SET_FACING: u32 = 0x0018_74b0;
    pub const OBJECT_SET_POSITION: u32 = 0x000b_75a0;
    /// Unit: desired facing, aiming and looking vectors, which the
    /// teleport writes (`0x2a01a2`, `0x2a01b8`, `0x2a01ce`).
    pub const UNIT_FACINGS: [u32; 3] = [0x150, 0x15c, 0x180];
    /// `unit_add_weapon` modes (module doc).
    pub const MODE_ADD: u32 = 1;
    pub const MODE_REPLACE: u32 = 2;
}

/// Scratch layout: the placement, then three vectors.
const SCRATCH_POS: u32 = 0xd0;
const SCRATCH_FORWARD: u32 = 0xdc;
const SCRATCH_UP: u32 = 0xe8;
/// The scratch [`CallsFeature`] allocates.
pub const SCRATCH_SIZE: u32 = 0x100;

/// One piece of work for the game.
#[derive(Debug, Clone, PartialEq)]
pub enum Call {
    /// Move a player's unit and/or turn it and the player's view.
    Teleport {
        /// The player's salted handle.
        player: u32,
        pos: Option<[f32; 3]>,
        /// A unit vector.
        facing: Option<[f32; 3]>,
    },
    /// Make a weapon from `weapon` (a `weap` tag datum) and add it to
    /// `unit` with `unit_add_weapon`'s `mode`.
    Give { unit: u32, weapon: u32, mode: u32 },
    /// Make an object from `tag` at `pos`.
    Spawn {
        tag: u32,
        pos: [f32; 3],
        facing: Option<[f32; 3]>,
    },
}

/// What came of a call: the answer's fields, or why not.
pub type Outcome = Result<Value, String>;

/// Outcomes kept for answers that never came for them (a timed-out
/// client): the oldest go past this many.
const DONE_KEEP: usize = 64;

/// The calls waiting for the next tick, and the outcomes waiting for their
/// answers.
#[derive(Debug, Default)]
pub struct Queue {
    /// [`CallsFeature`] is built: something will make the calls.
    armed: bool,
    next: u64,
    pending: Vec<(u64, Call)>,
    done: BTreeMap<u64, Outcome>,
}

/// A [`Queue`] shared by the methods and the feature.
pub type Shared = Arc<Mutex<Queue>>;

pub fn shared() -> Shared {
    Arc::new(Mutex::new(Queue::default()))
}

pub fn lock(q: &Shared) -> MutexGuard<'_, Queue> {
    q.lock().unwrap_or_else(|e| e.into_inner())
}

impl Queue {
    /// Something makes the calls (the feature is built).
    pub fn arm(&mut self) {
        self.armed = true;
    }

    pub fn armed(&self) -> bool {
        self.armed
    }

    /// Queues `call` for the next tick; its ticket.
    pub fn submit(&mut self, call: Call) -> Result<u64, String> {
        if !self.armed {
            return Err(
                "the game's own functions aren't reachable here: the agent-calls feature isn't \
                 installed (H2_FEATURES=-agent-calls, or a game without the agent's tick hook)"
                    .into(),
            );
        }
        self.next += 1;
        self.pending.push((self.next, call));
        Ok(self.next)
    }

    /// The calls to make now.
    pub fn drain(&mut self) -> Vec<(u64, Call)> {
        std::mem::take(&mut self.pending)
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Records what came of `ticket`.
    pub fn finish(&mut self, ticket: u64, out: Outcome) {
        self.done.insert(ticket, out);
        while self.done.len() > DONE_KEEP {
            self.done.pop_first();
        }
    }

    /// What came of `ticket`, once (`None` until the game made it).
    pub fn take(&mut self, ticket: u64) -> Option<Outcome> {
        self.done.remove(&ticket)
    }

    /// Why a machine snapshot can't be taken now: calls queued would be
    /// lost, or made twice after a load.
    pub fn live(&self) -> Option<String> {
        (!self.pending.is_empty()).then(|| {
            format!(
                "{} agent call(s) queued for the next tick",
                self.pending.len()
            )
        })
    }
}

/// A tag as a method names it, resolved in the loaded map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagRef {
    pub datum: u32,
    /// The group, as text (`weap`).
    pub group: String,
    pub name: Option<String>,
}

impl TagRef {
    pub fn json(&self) -> Value {
        json!({ "tag": format!("{:#010x}", self.datum), "group": self.group, "name": self.name })
    }
}

/// Finds tags in the loaded map by datum or name (`edit.*`'s rules).
pub trait Tags: Send {
    fn find(&mut self, tag: &str) -> Result<TagRef, String>;
}

/// [`Tags`] over `hot_tags`' editor (names from the loaded map's file).
pub struct MapTags {
    pub editor: hot_tags::Editor,
    pub guest: crate::hot::WithGuest,
}

impl Tags for MapTags {
    fn find(&mut self, tag: &str) -> Result<TagRef, String> {
        let mut out = None;
        let ed = &mut self.editor;
        (self.guest)(&mut |g| out = Some(ed.find_tag(g, tag)));
        let (e, name) = out.ok_or("no guest memory")?.map_err(|e| e.message)?;
        Ok(TagRef {
            datum: e.datum,
            group: e.group.to_string(),
            name,
        })
    }
}

/// An up vector square to `forward` (a unit vector): world up with the
/// forward part taken out, or +x for a forward straight up or down.
pub fn up_of(f: [f32; 3]) -> [f32; 3] {
    let d = f[2];
    let u = [-d * f[0], -d * f[1], 1.0 - d * f[2]];
    let len = (u[0] * u[0] + u[1] * u[1] + u[2] * u[2]).sqrt();
    if len < 1e-4 {
        return [1.0, 0.0, 0.0];
    }
    u.map(|v| v / len)
}

fn put_vec(m: &mut dyn GuestMemory, at: u32, v: [f32; 3]) -> Result<(), engine::MemFault> {
    for (i, c) in v.iter().enumerate() {
        m.set_f32(at + 4 * i as u32, *c)?;
    }
    Ok(())
}

/// A player's unit handle, its body and its local user, read now.
fn player_unit(m: &dyn GuestMemory, player: u32) -> Result<Option<(u32, u32, i16)>, String> {
    let fault = |e: engine::MemFault| format!("guest memory at {:#x} faulted", e.addr);
    let Some(array) = players::array(m).map_err(fault)? else {
        return Err("no players array (no game loaded)".into());
    };
    let Some(p) = array.get(m, DatumHandle(player)).map_err(fault)? else {
        return Err(format!("player {player:#010x} is gone"));
    };
    let Some(unit) = p.unit(m).map_err(fault)? else {
        return Ok(None);
    };
    let Some((_, body)) = objects::get(m, unit).map_err(fault)? else {
        return Ok(None);
    };
    Ok(Some((
        unit.raw(),
        body.addr,
        p.local_user(m).map_err(fault)?,
    )))
}

fn body_of(m: &dyn GuestMemory, object: u32) -> Option<objects::Body> {
    objects::get(m, DatumHandle::from_raw(object)?)
        .ok()
        .flatten()
        .map(|(_, b)| b)
}

fn hex(v: u32) -> String {
    format!("{v:08x}")
}

/// Makes one call. A guest call that faults ends the tick's work
/// (`EntryError`); a refusal is the outcome.
pub async fn run(cx: &Cx, call: &Call, scratch: u32) -> Result<Outcome, EntryError> {
    match *call {
        Call::Teleport {
            player,
            pos,
            facing,
        } => {
            let unit = match cx.read(|m| player_unit(m, player)) {
                Ok(Some(u)) => u,
                Ok(None) => return Ok(Err("the player has no unit now (between lives)".into())),
                Err(e) => return Ok(Err(e)),
            };
            let (unit, body, user) = unit;
            let mut out = serde_json::Map::new();
            if let Some(p) = pos {
                cx.write(|m| put_vec(m, scratch + SCRATCH_POS, p))?;
                let r = cx
                    .call(
                        GuestCall::new(addr::PLAYER_PLACE_NEAR)
                            .ebx(player)
                            .esi(scratch + SCRATCH_POS)
                            .args([u32::MAX]),
                    )
                    .await?;
                if r.eax & 0xff == 0 {
                    return Ok(Err(
                        "the game's placement (PLAYER_PLACE_NEAR 0x14e970) refused: no unit, or no \
                         room for it there"
                            .into(),
                    ));
                }
            }
            if let Some(f) = facing {
                let parent = cx.read(|m| objects::Body::at(body).parent(m))?;
                if parent.is_some() {
                    out.insert(
                        "facing_left".into(),
                        json!("the unit rides a vehicle: its facing is the seat's"),
                    );
                } else {
                    let up = up_of(f);
                    cx.write(|m| {
                        for o in addr::UNIT_FACINGS {
                            put_vec(m, body + o, f)?;
                        }
                        put_vec(m, scratch + SCRATCH_FORWARD, f)?;
                        put_vec(m, scratch + SCRATCH_UP, up)
                    })?;
                    if user >= 0 {
                        cx.call(
                            GuestCall::new(addr::PLAYER_SET_FACING)
                                .ecx(user as u32)
                                .eax(scratch + SCRATCH_FORWARD),
                        )
                        .await?;
                    }
                    cx.call(
                        GuestCall::new(addr::OBJECT_SET_POSITION)
                            .eax(0)
                            .ecx(scratch + SCRATCH_FORWARD)
                            .edx(scratch + SCRATCH_UP)
                            .args([unit, 0, 0]),
                    )
                    .await?;
                    out.insert("faced".into(), json!(true));
                }
            }
            // Where the game put it (the body it has now: the placement
            // may have taken it out of a vehicle).
            let now = cx.read(|m| {
                let b = body_of(m, unit)?;
                Some((b.position(m).ok()?, b.forward(m).ok()?))
            });
            if let Some((p, f)) = now {
                out.insert("pos".into(), json!(p));
                out.insert("body_forward".into(), json!(f));
            }
            out.insert("unit".into(), json!(hex(unit)));
            Ok(Ok(Value::Object(out)))
        }
        Call::Give { unit, weapon, mode } => {
            if cx.read(|m| body_of(m, unit)).is_none() {
                return Ok(Err("the unit is gone (it died?)".into()));
            }
            cx.call(
                GuestCall::new(addr::PLACEMENT_NEW)
                    .esi(scratch)
                    .args([weapon, unit, 0]),
            )
            .await?;
            let made = cx
                .call(GuestCall::new(addr::OBJECT_NEW).args([scratch]))
                .await?
                .eax;
            if made == u32::MAX {
                return Ok(Err(format!(
                    "object_new (0xb7b40) refused to make tag {weapon:#010x}"
                )));
            }
            let added = cx
                .call(GuestCall::new(addr::UNIT_ADD_WEAPON).args([unit, made, mode]))
                .await?
                .eax
                & 0xff;
            if added == 0 {
                cx.call(GuestCall::new(addr::OBJECT_DELETE).args([made]))
                    .await?;
                return Ok(Err(format!(
                    "unit_add_weapon (0xcd0c0, mode {mode}) refused it (a full inventory, or a \
                     unit that takes no weapons); the weapon was deleted"
                )));
            }
            Ok(Ok(
                json!({ "object": hex(made), "unit": hex(unit), "mode": mode }),
            ))
        }
        Call::Spawn { tag, pos, facing } => {
            cx.call(
                GuestCall::new(addr::PLACEMENT_NEW)
                    .esi(scratch)
                    .args([tag, u32::MAX, 0]),
            )
            .await?;
            cx.write(|m| {
                put_vec(m, scratch + addr::PLACEMENT_POSITION, pos)?;
                if let Some(f) = facing {
                    put_vec(m, scratch + addr::PLACEMENT_FORWARD, f)?;
                    put_vec(m, scratch + addr::PLACEMENT_UP, up_of(f))?;
                }
                Ok::<_, engine::MemFault>(())
            })?;
            let made = cx
                .call(GuestCall::new(addr::OBJECT_NEW).args([scratch]))
                .await?
                .eax;
            if made == u32::MAX {
                return Ok(Err(format!(
                    "object_new (0xb7b40) refused to make tag {tag:#010x} there"
                )));
            }
            let at = cx.read(|m| body_of(m, made).and_then(|b| b.position(m).ok()));
            Ok(Ok(json!({ "object": hex(made), "pos": at })))
        }
    }
}

/// The `before` on `game_tick`'s body: makes the queued calls, records
/// each outcome, then runs the tick with `reentry` set so this hook (and
/// the queue) let it through.
pub async fn run_queued(
    cx: Cx,
    queue: Shared,
    work: Vec<(u64, Call)>,
    scratch: u32,
    reentry: Arc<AtomicBool>,
) -> Result<Ret, EntryError> {
    for (ticket, call) in work {
        let out = match run(&cx, &call, scratch).await {
            Ok(out) => out,
            Err(e) => {
                lock(&queue).finish(ticket, Err(format!("the game faulted: {e}")));
                return Err(e);
            }
        };
        lock(&queue).finish(ticket, out);
    }
    reentry.store(true, Ordering::SeqCst);
    let r = cx
        .call(GuestCall::new(engine::points::GAME_TICK.base))
        .await;
    reentry.store(false, Ordering::SeqCst);
    r?;
    Ok(Ret::new(0, 0))
}

/// The feature id (`H2_FEATURES=-agent-calls` leaves the calls out).
pub const FEATURE: &str = "agent-calls";

/// The `before` on `game_tick` that makes the queued calls (module doc).
pub struct CallsFeature(pub Shared);

impl feature::Feature for CallsFeature {
    fn manifest(&self) -> feature::Manifest {
        feature::Manifest::new(
            FEATURE,
            "Agent API: the game's own functions for sim.give, sim.spawn and teleports",
        )
        .doc("docs/runtime/agent-api.md")
    }

    fn build(&self, b: &mut feature::Builder) -> Result<(), feature::BuildError> {
        use feature::points::GAME_TICK;
        let q = Arc::clone(&self.0);
        b.snapshot(feature::SnapshotPolicy::forbid_while(move || {
            lock(&q).live()
        }));
        let scratch = b.guest_alloc(SCRATCH_SIZE, runtime::guest::Permissions::RW)?;
        let q = Arc::clone(&self.0);
        let reentry = Arc::new(AtomicBool::new(false));
        b.raw()
            .before(&GAME_TICK, "the agent's guest calls", move |c| {
                if reentry.load(Ordering::SeqCst) {
                    return Ok(feature::Flow::Continue);
                }
                let work = lock(&q).drain();
                if work.is_empty() {
                    return Ok(feature::Flow::Continue);
                }
                let (q, reentry) = (Arc::clone(&q), Arc::clone(&reentry));
                c.answer_async(move |cx| -> EntryFuture {
                    Box::pin(run_queued(cx, q, work, scratch, reentry))
                })
            })
            // Outside the other hooks here: they run once, in the tick we
            // re-enter.
            .first();
        lock(&self.0).arm();
        Ok(())
    }
}

#[cfg(test)]
mod tests;
