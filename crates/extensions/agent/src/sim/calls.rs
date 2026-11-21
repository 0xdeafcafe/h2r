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
//! - **Forge** makes each of a local Forge edit's placements and restored
//!   deletes the same way, forward and up from the frame's rotation, then
//!   moves and deletes its live objects with the `world.object.set` and
//!   `.delete` bodies. A refusal deletes the objects the batch
//!   already made, newest first.
//! - **Queue a game** is the level boot's sequence (docs/runtime/level-boot.md)
//!   run from a tick: the host gives the steps ([`GameStep`]: the game's
//!   option builders, our field writes, its options check and its queue,
//!   `0x18e790`), over an options block in the scratch
//!   ([`SCRATCH_OPTIONS`]). The game loads the queued options at its next
//!   transition, as it does the main menu's own start. `maps.test.load`
//!   uses it (docs/game/test-maps.md).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, mpsc};

use super::edits::{PreparedMapReload, ReservedMapReload, active_options, refused, replay_options};
use agent_proto::{ObjectId, RpcError, code};
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

/// Scratch layout: the placement, then three vectors, then a game options
/// block.
const SCRATCH_POS: u32 = 0xd0;
const SCRATCH_FORWARD: u32 = 0xdc;
const SCRATCH_UP: u32 = 0xe8;
/// Where [`Call::QueueGame`] builds its options.
pub const SCRATCH_OPTIONS: u32 = 0x100;
/// The game options block's size (docs/runtime/level-boot.md).
pub const OPTIONS_SIZE: u32 = 0x1118;
/// OPTIONS_QUEUE marks its single pending options block (docs/runtime/level-boot.md).
pub(super) const OPTIONS_PENDING: u32 = 0x004e_d39d;
/// Destination of OPTIONS_QUEUE's copy (docs/runtime/level-boot.md).
pub(super) const PENDING_OPTIONS: u32 = 0x004e_d3a8;
/// The scratch [`CallsFeature`] allocates.
pub const SCRATCH_SIZE: u32 = 0x1300;
const _: () = assert!(SCRATCH_OPTIONS + OPTIONS_SIZE <= SCRATCH_SIZE);

/// A value a [`GameStep::Call`] passes: a number, or an address in the
/// options block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arg {
    Value(u32),
    /// The options block plus this offset.
    Options(u32),
}

impl Arg {
    pub fn resolve(self, options: u32) -> u32 {
        match self {
            Arg::Value(v) => v,
            Arg::Options(o) => options.wrapping_add(o),
        }
    }
}

/// One step of building and queueing game options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GameStep {
    /// Write `bytes` at this offset of the options block.
    Write { offset: u32, bytes: Vec<u8> },
    /// Call the game: stack arguments first argument first, and EAX and
    /// ESI when the callee takes them there. With `report`, the answer
    /// says whether it returned AL nonzero, under that name.
    Call {
        target: u32,
        args: Vec<Arg>,
        eax: Option<Arg>,
        esi: Option<Arg>,
        report: Option<&'static str>,
    },
}

/// A salted object or an entry of the loaded scenario.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RespawnTarget {
    Object(ObjectId),
    Placement(agent_proto::sim::Placement),
}

/// The object a Forge step edits: one already in the world, or the
/// `index`th object this batch makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForgeTarget {
    Live(ObjectId),
    Made { index: usize, kind: agent_proto::Kind },
}

/// One engine call of a local Forge edit. [`Call::Forge`] makes every
/// object first, then moves, then deletes, so only the makes can be refused
/// once the live objects have been checked, and those it unwinds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ForgeStep {
    /// Make `tag` at the frame: a placement, or a delete being restored.
    Make { tag: u32, frame: ::forge::Frame },
    /// `world.object.set` with the frame's position, forward and up.
    Move {
        target: ForgeTarget,
        frame: ::forge::Frame,
    },
    /// `world.object.delete`.
    Delete { target: ForgeTarget },
}

/// The forward and up axes of a Forge frame's yaw, pitch and roll.
fn forge_axes(frame: &::forge::Frame) -> ([f32; 3], [f32; 3]) {
    let (forward, up) = live_telemetry::rotation_axes(frame.rotation.map(f64::from));
    (forward.map(|v| v as f32), up.map(|v| v as f32))
}

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
        /// Forge's probe rechecks the distributed host and scenario at execution.
        mp_scenario: Option<u32>,
    },
    ObjectSet {
        id: ObjectId,
        pos: Option<[f32; 3]>,
        forward: Option<[f32; 3]>,
        up: Option<[f32; 3]>,
    },
    ObjectDelete { id: ObjectId },
    ObjectRespawn { target: RespawnTarget },
    /// One local Forge edit's engine work, rechecking its admission and
    /// every live object at the next tick before the first guest call.
    Forge {
        guard: crate::forge::local::Guard,
        steps: Vec<ForgeStep>,
    },
    /// Build game options with `steps` and queue them: a new game loads
    /// at the game's next transition.
    QueueGame { steps: Vec<GameStep> },
}

/// What came of a call: the answer's fields, or why not.
pub type Outcome = Result<Value, RpcError>;

fn unavailable(why: impl Into<String>) -> RpcError {
    RpcError::new(code::UNAVAILABLE, why)
}

/// The calls waiting for the next tick, and the outcomes waiting for their
/// answers.
#[derive(Debug, Default)]
pub struct Queue {
    /// [`CallsFeature`] is built: something will make the calls.
    armed: bool,
    spawn_enrollment: Option<SpawnEnrollment>,
    next: u64,
    pending: Vec<(u64, Call)>,
    waiting: BTreeMap<u64, QueuedCall>,
    previews: placement::Previews,
    active_scenario: Option<(String, u32)>,
    game_ticket: Option<u64>,
    reload: Option<Reload>,
    handoff: Option<ReloadHandoff>,
    transition: Option<activation::Transition>,
    // A failed restore keeps only its outstanding keys, never the DVD reservation.
    repair: Option<CacheRepair>,
}

struct CacheRepair(Box<dyn FnMut() -> Result<(), String> + Send>);

impl std::fmt::Debug for CacheRepair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("retired map cache keys")
    }
}

/// Captured at dispatch and checked again by the hook before queuing a reload.
#[derive(Debug)]
struct Reload {
    ticket: u64,
    options: Vec<u8>,
    binding: Option<PreparedMapReload>,
}

/// Lives in the queue even if the runtime discards the OPTIONS_QUEUE
/// continuation while it owns the guest-memory lock. Only the pending
/// byte can tell whether the guest queued the transition on that path.
struct ReloadHandoff {
    ticket: u64,
    options: u32,
    binding: Option<ReservedMapReload>,
    interrupted: bool,
    skulls: Option<Box<dyn FnOnce() + Send>>,
}

impl ReloadHandoff {
    fn publish(self) {
        if let Some(binding) = self.binding {
            (binding.publish)();
        }
        if let Some(apply) = self.skulls {
            apply();
        }
    }
}

impl std::fmt::Debug for ReloadHandoff {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReloadHandoff")
            .field("ticket", &self.ticket)
            .field("interrupted", &self.interrupted)
            .finish_non_exhaustive()
    }
}

/// Recheck the map and session at the execution boundary.
pub(super) type EditCheck = Box<dyn FnOnce(&dyn GuestMemory) -> Result<(), RpcError> + Send>;

struct QueuedCall {
    queues_game: bool,
    check: Option<EditCheck>,
    answer: Option<mpsc::Sender<Outcome>>,
}

impl std::fmt::Debug for QueuedCall {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueuedCall")
            .field("awaiting_check", &self.check.is_some())
            .finish_non_exhaustive()
    }
}

/// The deferred RPC owns its answer until it consumes it or stops waiting.
/// Dropping the ticket cancels any work the hook has yet to start.
pub struct PendingCall {
    queue: Shared,
    ticket: u64,
    answer: mpsc::Receiver<Outcome>,
    preparing: bool,
}

impl PendingCall {
    /// Tickets and unrelated full starts draw from the same process-local
    /// sequence. A superseded reload's identity is never issued again.
    pub(super) fn activation(&self) -> u64 {
        self.ticket
    }

    /// What came of this call, once. Pending calls have no outcome yet.
    pub fn outcome(&self) -> Option<Outcome> {
        self.answer.try_recv().ok()
    }

    /// Make this ticket runnable after its optional file preparation.
    pub(super) fn schedule_reload(&mut self, options: Vec<u8>, binding: Option<PreparedMapReload>) {
        debug_assert!(self.preparing);
        let mut q = lock(&self.queue);
        q.pending.push((self.ticket, replay_options(options.clone())));
        q.reload = Some(Reload {
            ticket: self.ticket,
            options,
            binding,
        });
        self.preparing = false;
    }
}

impl Drop for PendingCall {
    fn drop(&mut self) {
        let mut q = lock(&self.queue);
        if self.preparing {
            q.waiting.remove(&self.ticket);
            q.release_game(self.ticket);
        } else if let Some(index) = q.pending.iter().position(|(t, _)| *t == self.ticket) {
            q.pending.remove(index);
            q.waiting.remove(&self.ticket);
            q.release_game(self.ticket);
        } else if let Some(waiter) = q.waiting.get_mut(&self.ticket) {
            // Drained, possibly behind a suspended guest call. The hook
            // checks the receiver before starting and still accounts for
            // this ticket until the batch finishes or is dropped.
            waiter.answer = None;
        }
    }
}

/// Queue a guest call and keep its result for the deferred caller.
pub fn submit(queue: &Shared, call: Call) -> Result<PendingCall, String> {
    lock(queue).submit(queue, call, None)
}

pub(super) fn queue_edit(
    queue: &Shared,
    call: Call,
    check: EditCheck,
) -> Result<PendingCall, RpcError> {
    lock(queue).submit(queue, call, Some(check)).map_err(refused)
}

pub(super) fn hold_reload(queue: &Shared, check: EditCheck) -> Result<PendingCall, RpcError> {
    let mut calls = lock(queue);
    if calls.waiting.values().any(|call| call.queues_game) {
        return Err(refused("another game transition is already queued"));
    }
    let mut ticket = calls.admit(queue, true, Some(check)).map_err(refused)?;
    calls.game_ticket = Some(ticket.ticket);
    ticket.preparing = true;
    Ok(ticket)
}

#[cfg(test)]
fn queue_reload(
    queue: &Shared,
    options: Vec<u8>,
    binding: Option<PreparedMapReload>,
) -> Result<PendingCall, RpcError> {
    let mut ticket = hold_reload(queue, Box::new(|_| Ok(())))?;
    ticket.schedule_reload(options, binding);
    Ok(ticket)
}

/// Optional distributed enrollment for an object just made by `sim.spawn`.
/// The game supplies the eligibility check and guest call.
pub type SpawnEnrollment = fn(&dyn GuestMemory, u32) -> Option<GuestCall>;

/// A [`Queue`] shared by the methods and the feature.
pub type Shared = Arc<Mutex<Queue>>;

pub fn shared() -> Shared {
    Arc::new(Mutex::new(Queue::default()))
}

pub fn lock(q: &Shared) -> MutexGuard<'_, Queue> {
    q.lock().unwrap_or_else(|e| e.into_inner())
}

/// The cache name and game mode of the timeline which owns a preview.
pub(crate) fn preview_scenario(m: &dyn engine::GuestMemory) -> Option<(String, u32)> {
    let mut name = [0u8; 32];
    m.read(guest_layout::global::CACHE_MAP_NAME.base, &mut name).ok()?;
    let len = name.iter().position(|&b| b == 0).unwrap_or(name.len());
    if len == 0 {
        return None;
    }
    let game = guest_layout::game::GameGlobals::current(m).ok()??;
    Some((String::from_utf8_lossy(&name[..len]).into_owned(), game.options().mode(m).ok()?))
}

impl Queue {
    /// The skull hook normally applies its selection before OPTIONS_QUEUE.
    /// An editor reload applies it only once retail has accepted these options.
    pub fn stage_reload_skulls<F: FnOnce() + Send + 'static>(
        &mut self, options: u32, apply: impl FnOnce() -> F,
    ) -> bool {
        if let Some(handoff) = self.handoff.as_mut().filter(|h| !h.interrupted && h.options == options) {
            handoff.skulls = Some(Box::new(apply()));
            true
        } else {
            false
        }
    }

    /// A campaign reactivates the same mission when restoring a checkpoint.
    /// Retain tag history while handles on that timeline are ambiguous.
    pub fn map_activated(&mut self, scenario: Option<(String, u32)>) {
        let fresh = scenario.as_ref().is_some_and(|(name, mode)| {
            *mode != guest_layout::game::MODE_CAMPAIGN || self.active_scenario.as_ref()
                .is_some_and(|(old, _)| !old.eq_ignore_ascii_case(name))
        });
        if fresh {
            self.previews = placement::Previews::default();
        } else {
            self.checkpoint_reverted();
        }
        self.active_scenario = scenario;
    }

    pub fn checkpoint_reverted(&mut self) {
        self.previews.checkpoint_reverted();
    }

    pub fn preview_refusal(&self) -> Option<String> {
        (!self.previews.is_empty()).then(|| {
            "snapshots cannot reproduce an unlinked editor preview; change maps first".into()
        })
    }

    fn preview_deleted(&mut self, id: ObjectId) {
        let campaign = self.active_scenario.as_ref()
            .is_none_or(|(_, mode)| *mode == guest_layout::game::MODE_CAMPAIGN);
        self.previews.deleted(id, campaign);
    }

    /// Something makes the calls (the feature is built).
    pub fn arm(&mut self) {
        self.armed = true;
    }

    pub fn armed(&self) -> bool {
        self.armed
    }

    pub fn set_spawn_enrollment(&mut self, enroll: SpawnEnrollment) {
        self.spawn_enrollment = Some(enroll);
    }

    fn submit(
        &mut self,
        queue: &Shared,
        call: Call,
        check: Option<EditCheck>,
    ) -> Result<PendingCall, String> {
        let queues_game = matches!(&call, Call::QueueGame { steps } if steps.iter().any(|step| {
            matches!(step, GameStep::Call { target, .. } if *target == engine::points::OPTIONS_QUEUE.base)
        }));
        let ticket = self.admit(queue, queues_game, check)?;
        self.pending.push((ticket.ticket, call));
        Ok(ticket)
    }

    fn admit(
        &mut self,
        queue: &Shared,
        game: bool,
        check: Option<EditCheck>,
    ) -> Result<PendingCall, String> {
        if !self.armed {
            return Err(
                "the game's own functions aren't reachable here: the agent-calls feature isn't \
                 installed (H2_FEATURES=-agent-calls, or a game without the agent's tick hook)"
                    .into(),
            );
        }
        if game && self.game_ticket.is_some() {
            return Err("an editor reload already owns the pending transition".into());
        }
        self.next += 1;
        let ticket = self.next;
        let (answer, rx) = mpsc::channel();
        self.waiting.insert(ticket, QueuedCall {
            queues_game: game,
            check,
            answer: Some(answer),
        });
        Ok(PendingCall {
            queue: queue.clone(),
            ticket,
            answer: rx,
            preparing: false,
        })
    }

    fn release_game(&mut self, ticket: u64) {
        if self.game_ticket == Some(ticket) {
            self.game_ticket = None;
            self.reload = None;
        }
    }

    /// The calls to make now.
    pub fn drain(&mut self) -> Vec<(u64, Call)> {
        std::mem::take(&mut self.pending)
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    pub(crate) fn blocks_input_log(&self) -> bool {
        self.game_ticket.is_some() || self.reload_transition_pending()
    }

    /// Records what came of `ticket`.
    pub fn finish(&mut self, ticket: u64, out: Outcome) {
        self.release_game(ticket);
        if let Some(waiter) = self.waiting.remove(&ticket) {
            if let Some(answer) = waiter.answer {
                let _ = answer.send(out);
            }
        }
    }

    /// The editor's options were accepted, but their game has yet to start.
    pub(crate) fn reload_transition_pending(&self) -> bool {
        self.transition.is_some()
    }

    /// Why a machine snapshot would lose or repeat pending work.
    pub fn live(&self) -> Option<String> {
        if self.reload_transition_pending() {
            return Some("an editor reload is waiting for its map to start".into());
        }
        if self.repair.is_some() {
            return Some("an editor reload still has map cache keys to restore".into());
        }
        (!self.waiting.is_empty()).then(|| {
            format!("{} agent call(s) awaiting completion", self.waiting.len())
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
    /// `tag` by datum, `group:name`, `name.group` or a bare name, which
    /// is looked for among the tags of `among` (any group when empty).
    fn find(&mut self, tag: &str, among: &[&str]) -> Result<TagRef, String>;
}

/// [`Tags`] over `hot_tags`' editor (names from the loaded map's file).
pub struct MapTags {
    pub editor: hot_tags::Editor,
    pub guest: crate::hot::WithGuest,
}

impl Tags for MapTags {
    fn find(&mut self, tag: &str, among: &[&str]) -> Result<TagRef, String> {
        let mut out = None;
        let ed = &mut self.editor;
        (self.guest)(&mut |g| out = Some(ed.find_tag_in(g, tag, among)));
        let (e, name) = out.ok_or("no guest memory")?.map_err(|e| e.message)?;
        Ok(TagRef {
            datum: e.datum,
            group: e.group.to_string(),
            name,
        })
    }
}

/// `world.*`'s tag names, from the same editor.
impl crate::TagNames for MapTags {
    fn names(&mut self, datums: &[u32]) -> Vec<Option<String>> {
        let mut out = vec![None; datums.len()];
        let ed = &mut self.editor;
        (self.guest)(&mut |g| out = ed.names_of(g, datums));
        out
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

/// The ownerless placement used by both spawn and scenario previews.
async fn prepare_spawn(
    cx: &Cx,
    scratch: u32,
    tag: u32,
    pos: [f32; 3],
    axes: Option<([f32; 3], [f32; 3])>,
) -> Result<(), EntryError> {
    cx.call(
        GuestCall::new(addr::PLACEMENT_NEW)
            .esi(scratch)
            .args([tag, u32::MAX, 0]),
    )
    .await?;
    cx.write(|m| {
        put_vec(m, scratch + addr::PLACEMENT_POSITION, pos)?;
        if let Some((forward, up)) = axes {
            put_vec(m, scratch + addr::PLACEMENT_FORWARD, forward)?;
            put_vec(m, scratch + addr::PLACEMENT_UP, up)?;
        }
        Ok::<_, engine::MemFault>(())
    })?;
    Ok(())
}

/// `sim.spawn` and Forge make an ownerless object the same way: the
/// placement [`prepare_spawn`] fills, then `object_new`.
async fn spawn(
    cx: &Cx,
    scratch: u32,
    tag: u32,
    pos: [f32; 3],
    axes: Option<([f32; 3], [f32; 3])>,
) -> Result<Result<u32, RpcError>, EntryError> {
    prepare_spawn(cx, scratch, tag, pos, axes).await?;
    let made = cx.call(GuestCall::new(addr::OBJECT_NEW).args([scratch])).await?.eax;
    if made == u32::MAX {
        return Ok(Err(unavailable(format!(
            "object_new (0xb7b40) refused to make tag {tag:#010x} there"
        ))));
    }
    Ok(Ok(made))
}

/// Release an object made by the current command after an engine refusal.
/// sim.give uses this when the unit cannot take the new weapon, and a Forge
/// batch to unwind its earlier placements when a later one is refused.
async fn discard_new(cx: &Cx, made: u32) -> Result<(), EntryError> {
    cx.call(GuestCall::new(addr::OBJECT_DELETE).args([made]))
        .await?;
    Ok(())
}

/// `world.object.set` and a Forge move: `object_set_position` on a
/// resolved object, an omitted axis kept from the object's own.
async fn object_set(
    cx: &Cx,
    scratch: u32,
    id: ObjectId,
    pos: Option<[f32; 3]>,
    forward: Option<[f32; 3]>,
    up: Option<[f32; 3]>,
) -> Result<Outcome, EntryError> {
    let body = match cx.read(|m| placement::resolve(m, id)) {
        Ok(body) => body,
        Err(e) => return Ok(Err(e)),
    };
    let turning = forward.is_some() || up.is_some();
    if turning {
        let axes = cx.read(|m| -> Result<_, engine::MemFault> {
            Ok((
                match forward {
                    Some(f) => f,
                    None => body.forward(m)?,
                },
                match up {
                    Some(u) => u,
                    None => body.up(m)?,
                },
            ))
        });
        let (f, u) = match axes {
            Ok(axes) => axes,
            Err(e) => {
                return Ok(Err(unavailable(format!(
                    "cannot read the object's unchanged orientation at {:#x}",
                    e.addr
                ))));
            }
        };
        if !super::edits::valid_axes(f, u) {
            return Ok(Err(super::invalid(
                "forward and up must be perpendicular unit vectors",
            )));
        }
        cx.write(|m| {
            put_vec(m, scratch + SCRATCH_FORWARD, f)?;
            put_vec(m, scratch + SCRATCH_UP, u)
        })?;
    }
    if let Some(p) = pos {
        cx.write(|m| put_vec(m, scratch + SCRATCH_POS, p))?;
    }
    cx.call(
        GuestCall::new(addr::OBJECT_SET_POSITION)
            .eax(if pos.is_some() { scratch + SCRATCH_POS } else { 0 })
            .ecx(if turning { scratch + SCRATCH_FORWARD } else { 0 })
            .edx(if turning { scratch + SCRATCH_UP } else { 0 })
            .args([id.key, 0, 0]),
    )
    .await?;
    Ok(Ok(json!({ "object": id.to_string() })))
}

/// `world.object.delete` and a Forge delete: `object_delete` on a resolved object.
async fn object_delete(cx: &Cx, queue: &Shared, id: ObjectId) -> Result<Outcome, EntryError> {
    if let Err(e) = cx.read(|m| placement::resolve(m, id)) {
        return Ok(Err(e));
    }
    cx.call(GuestCall::new(addr::OBJECT_DELETE).args([id.key]))
        .await?;
    lock(queue).preview_deleted(id);
    Ok(Ok(json!({ "deleted": id.to_string() })))
}

/// A stale Forge handle is a named refusal, so the session reports it and
/// keeps its document. Memory faults stay as they are.
fn forge_gone(e: RpcError, id: ObjectId) -> RpcError {
    if e.code == code::GONE {
        refused(format!(
            "{}: {id} is no longer in the world",
            crate::forge::local::OBJECT_GONE
        ))
    } else {
        e
    }
}

fn forge_id(target: ForgeTarget, made: &[u32]) -> ObjectId {
    match target {
        ForgeTarget::Live(id) => id,
        ForgeTarget::Made { index, kind } => ObjectId {
            kind,
            key: made[index],
        },
    }
}

/// Release the objects this batch made and has not deleted, newest first,
/// before the game tick can give them to another player or system.
async fn unwind_made(cx: &Cx, made: &[u32], deleted: &[usize]) -> Result<(), EntryError> {
    for (index, &handle) in made.iter().enumerate().rev() {
        if !deleted.contains(&index) {
            discard_new(cx, handle).await?;
        }
    }
    Ok(())
}

/// A local Forge edit, in the [`ForgeStep`] order. The guard and the
/// `world.object.*` object check run before the first guest call: every
/// live target must still resolve with its kind and salt, and every frame
/// must give perpendicular unit axes. Moves and deletes are the
/// `world.object.set` and `.delete` bodies ([`object_set`],
/// [`object_delete`]); a make, a placement or a restored delete, is the
/// ownerless [`spawn`] that `sim.spawn` uses.
async fn run_forge(
    cx: &Cx,
    scratch: u32,
    queue: &Shared,
    guard: crate::forge::local::Guard,
    steps: &[ForgeStep],
) -> Result<Outcome, EntryError> {
    if !cx.read(|m| guard.allows(m)) {
        return Ok(Err(unavailable(
            "the local Forge session changed before the edit",
        )));
    }
    let makes = steps
        .iter()
        .filter(|step| matches!(step, ForgeStep::Make { .. }))
        .count();
    for step in steps {
        let (target, frame) = match step {
            ForgeStep::Make { frame, .. } => (None, Some(frame)),
            ForgeStep::Move { target, frame } => (Some(*target), Some(frame)),
            ForgeStep::Delete { target } => (Some(*target), None),
        };
        match target {
            Some(ForgeTarget::Live(id)) => {
                if let Err(e) = cx.read(|m| placement::resolve(m, id)) {
                    return Ok(Err(forge_gone(e, id)));
                }
            }
            Some(ForgeTarget::Made { index, .. }) if index >= makes => {
                return Ok(Err(super::invalid(
                    "a Forge step names an object its batch does not make",
                )));
            }
            _ => {}
        }
        if let Some(frame) = frame {
            let (forward, up) = forge_axes(frame);
            if !frame.position.iter().all(|c| c.is_finite())
                || !super::edits::valid_axes(forward, up)
            {
                return Ok(Err(super::invalid(
                    "a Forge frame needs a finite position and rotation",
                )));
            }
        }
    }
    if makes > 0 {
        match cx.read(crate::forge::object_pool) {
            Ok(pool) if makes <= (pool.limit - pool.used) as usize => {}
            Ok(_) => {
                return Ok(Err(unavailable(
                    "the game object pool has insufficient free slots",
                )));
            }
            Err(e) => return Ok(Err(e)),
        }
    }
    let mut made = Vec::with_capacity(makes);
    for step in steps {
        if let ForgeStep::Make { tag, frame } = *step {
            match spawn(cx, scratch, tag, frame.position, Some(forge_axes(&frame))).await? {
                Ok(handle) => made.push(handle),
                Err(e) => {
                    unwind_made(cx, &made, &[]).await?;
                    return Ok(Err(e));
                }
            }
        }
    }
    // The checks above leave only a guest change during this batch to
    // refuse a move or delete. The batch then releases what it made; the
    // moves it already made stay.
    for step in steps {
        if let ForgeStep::Move { target, frame } = *step {
            let (forward, up) = forge_axes(&frame);
            let id = forge_id(target, &made);
            let out =
                object_set(cx, scratch, id, Some(frame.position), Some(forward), Some(up)).await?;
            if let Err(e) = out {
                unwind_made(cx, &made, &[]).await?;
                return Ok(Err(forge_gone(e, id)));
            }
        }
    }
    let mut deleted = Vec::new();
    for step in steps {
        if let ForgeStep::Delete { target } = *step {
            let id = forge_id(target, &made);
            if let Err(e) = object_delete(cx, queue, id).await? {
                unwind_made(cx, &made, &deleted).await?;
                return Ok(Err(forge_gone(e, id)));
            }
            if let ForgeTarget::Made { index, .. } = target {
                deleted.push(index);
            }
        }
    }
    Ok(Ok(json!({ "handles": made })))
}

/// Makes one call. A guest call that faults ends the tick's work
/// (`EntryError`); a refusal is the outcome.
#[cfg(test)]
async fn run(
    cx: &Cx,
    call: &Call,
    scratch: u32,
    queue: &Shared,
) -> Result<Outcome, EntryError> {
    run_call(cx, call, scratch, queue, None).await
}

async fn run_call(
    cx: &Cx,
    call: &Call,
    scratch: u32,
    queue: &Shared,
    ticket: Option<u64>,
) -> Result<Outcome, EntryError> {
    match *call {
        Call::Teleport {
            player,
            pos,
            facing,
        } => {
            let unit = match cx.read(|m| player_unit(m, player)) {
                Ok(Some(u)) => u,
                Ok(None) => return Ok(Err(unavailable("the player has no unit now (between lives)"))),
                Err(e) => return Ok(Err(unavailable(e))),
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
                    return Ok(Err(unavailable(
                        "the game's placement (PLAYER_PLACE_NEAR 0x14e970) refused: no unit, or no \
                         room for it there",
                    )));
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
                return Ok(Err(unavailable("the unit is gone (it died?)")));
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
                return Ok(Err(unavailable(format!(
                    "object_new (0xb7b40) refused to make tag {weapon:#010x}"
                ))));
            }
            let added = cx
                .call(GuestCall::new(addr::UNIT_ADD_WEAPON).args([unit, made, mode]))
                .await?
                .eax
                & 0xff;
            if added == 0 {
                discard_new(cx, made).await?;
                return Ok(Err(unavailable(format!(
                    "unit_add_weapon (0xcd0c0, mode {mode}) refused it (a full inventory, or a \
                     unit that takes no weapons); the weapon was deleted"
                ))));
            }
            Ok(Ok(
                json!({ "object": hex(made), "unit": hex(unit), "mode": mode }),
            ))
        }
        Call::Spawn {
            tag,
            pos,
            facing,
            mp_scenario,
        } => {
            if let Some(scenario) = mp_scenario {
                let still_host = cx.read(|m| {
                    use guest_layout::global;
                    m.u32(global::SCENARIO.base) == Ok(scenario)
                        && m.u32(global::SIM_GLOBALS.base)
                            .ok()
                            .filter(|&world| world != 0)
                            .and_then(|world| world.checked_add(8))
                            .and_then(|mode| m.u32(mode).ok())
                            .map(agent_proto::sim::SessionKind::from_sim_mode)
                            == Some(agent_proto::sim::SessionKind::MpHost)
                });
                if !still_host {
                    return Ok(Err(unavailable(
                        "the replication probe's multiplayer host or scenario changed",
                    )));
                }
            }
            let made = match spawn(cx, scratch, tag, pos, facing.map(|f| (f, up_of(f)))).await? {
                Ok(made) => made,
                Err(e) => return Ok(Err(e)),
            };
            let spawn_enrollment = lock(queue).spawn_enrollment;
            if let Some(enroll) = spawn_enrollment
                && let Some(call) = cx.read(|m| enroll(m, made))
            {
                cx.call(call).await?;
            }
            let at = cx.read(|m| body_of(m, made).and_then(|b| b.position(m).ok()));
            Ok(Ok(json!({ "object": hex(made), "pos": at })))
        }
        Call::ObjectSet {
            id,
            pos,
            forward,
            up,
        } => object_set(cx, scratch, id, pos, forward, up).await,
        Call::ObjectDelete { id } => object_delete(cx, queue, id).await,
        Call::ObjectRespawn { ref target } => {
            let entry = match cx.read(|m| placement::read(m, target, &lock(queue).previews)) {
                Ok(entry) => entry,
                Err(e) => return Ok(Err(e)),
            };
            prepare_spawn(cx, scratch, entry.tag, entry.pos, Some(entry.axes)).await?;
            let previous = cx.read(|m| {
                let mut q = lock(queue);
                if q.previews.is_empty() {
                    // A machine restore can enter a different map without
                    // running GAME_ACTIVATE. Read the origin from this guest.
                    q.active_scenario = preview_scenario(m);
                }
                q.previews.creating(&entry)
            });
            let made = cx.call(GuestCall::new(addr::OBJECT_NEW).args([scratch])).await?.eax;
            let old = entry.old.map(|id| id.to_string());
            if made == u32::MAX {
                lock(queue).previews.refused(&entry, previous);
                return Ok(Err(refused("object_new refused the replacement")));
            }
            let object = ObjectId {
                kind: entry.kind,
                key: made,
            };
            // Keep the uncertain preview marked until the old object's deletion
            // returns. If retail interrupts the transfer, another placement
            // request must not make a third object beside the unlinked one.
            if let Some(old) = entry.old {
                cx.call(GuestCall::new(addr::OBJECT_DELETE).args([old.key]))
                    .await?;
            }
            lock(queue).previews.remember(&entry, object);
            Ok(Ok(json!({ "old": old, "object": object.to_string(), "linked": false })))
        }
        Call::Forge { guard, ref steps } => run_forge(cx, scratch, queue, guard, steps).await,
        Call::QueueGame { ref steps } => {
            let options = scratch + SCRATCH_OPTIONS;
            let mut out = serde_json::Map::new();
            let mut calls = 0;
            for step in steps {
                match step {
                    GameStep::Write { offset, bytes } => {
                        if offset.saturating_add(bytes.len() as u32) > OPTIONS_SIZE {
                            return Ok(Err(unavailable(format!(
                                "a write at {offset:#x} runs past the options block"
                            ))));
                        }
                        cx.write(|m| m.write(options + offset, bytes))?;
                    }
                    GameStep::Call {
                        target,
                        args,
                        eax,
                        esi,
                        report,
                    } => {
                        let mut c = GuestCall::new(*target)
                            .args(args.iter().map(|a| a.resolve(options)));
                        if let Some(a) = eax {
                            c = c.eax(a.resolve(options));
                        }
                        if let Some(a) = esi {
                            c = c.esi(a.resolve(options));
                        }
                        if *target == engine::points::OPTIONS_QUEUE.base
                            && let Some(ticket) = ticket
                            && let Err(why) = reserve_reload(queue, ticket, options)
                        {
                            return Ok(Err(refused(why)));
                        }
                        let r = cx.call(c).await?;
                        calls += 1;
                        if let Some(name) = report {
                            out.insert((*name).into(), json!(r.eax & 0xff != 0));
                        }
                    }
                }
            }
            out.insert("queued".into(), json!(true));
            out.insert("calls".into(), json!(calls));
            Ok(Ok(Value::Object(out)))
        }
    }
}

/// The options block is complete and no guest function has run for this
/// reload. Reserve its source and retire the cache keys before OPTIONS_QUEUE
/// and its skull hooks can change game state.
fn reserve_reload(queue: &Shared, ticket: u64, options: u32) -> Result<(), String> {
    let reload = {
        let mut q = lock(queue);
        if !q.reload.as_ref().is_some_and(|r| r.ticket == ticket) {
            return Ok(());
        }
        q.reload.take().expect("matched reload")
    };
    if reload.binding.is_some() && lock(queue).repair.is_some() {
        return Err("an earlier editor reload still has map cache keys to restore".into());
    }
    let mut binding = reload.binding.map(PreparedMapReload::reserve).transpose()?;
    if let Some(why) = binding.as_mut().and_then(|b| b.retirement_error.take()) {
        return Err(cancel_binding(queue, binding, why));
    }
    lock(queue).handoff = Some(ReloadHandoff {
        ticket,
        options,
        binding,
        interrupted: false,
        skulls: None,
    });
    Ok(())
}

/// A nested guest transfer may drop the entry future without returning an
/// EntryError. Every dispatched ticket still needs an answer on that path.
struct DrainedCalls {
    queue: Shared,
    remaining: std::vec::IntoIter<(u64, Call)>,
    current: Option<u64>,
}

impl DrainedCalls {
    fn next(&mut self) -> Option<(u64, Call)> {
        let next = self.remaining.next()?;
        self.current = Some(next.0);
        Some(next)
    }

    fn finish(&mut self, ticket: u64, out: Outcome) {
        lock(&self.queue).finish(ticket, out);
        self.current = None;
    }
}

impl Drop for DrainedCalls {
    fn drop(&mut self) {
        let mut q = lock(&self.queue);
        if let Some(ticket) = self.current {
            if let Some(load) = q.handoff.as_mut().filter(|load| load.ticket == ticket) {
                // Do not take guest memory from Drop. The transfer driver may
                // be holding it. The transition/tick boundary reconciles this.
                load.interrupted = true;
            } else {
                q.finish(ticket, Err(unavailable("the guest call was interrupted")));
            }
        }
        for (ticket, _) in &mut self.remaining {
            q.finish(ticket, Err(unavailable("an earlier guest call was interrupted")));
        }
    }
}

struct TickReentry(Arc<AtomicBool>);

impl Drop for TickReentry {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// The `before` on `game_tick`'s body: makes the queued calls, records
/// each outcome, then runs the tick with `reentry` set so this hook (and
/// the queue) let it through.
pub fn run_queued(
    cx: Cx,
    queue: Shared,
    work: Vec<(u64, Call)>,
    scratch: u32,
    reentry: Arc<AtomicBool>,
) -> impl std::future::Future<Output = Result<Ret, EntryError>> {
    let work = DrainedCalls {
        queue: queue.clone(),
        remaining: work.into_iter(),
        current: None,
    };
    run_batch(cx, queue, work, scratch, reentry)
}

async fn run_batch(
    cx: Cx,
    queue: Shared,
    mut work: DrainedCalls,
    scratch: u32,
    reentry: Arc<AtomicBool>,
) -> Result<Ret, EntryError> {
    while let Some((ticket, call)) = work.next() {
        let check = lock(&queue)
            .waiting
            .get_mut(&ticket)
            .filter(|w| w.answer.is_some())
            .map(|w| w.check.take());
        let editor_reload = matches!(&check, Some(Some(_))) && matches!(&call, Call::QueueGame { .. });
        let authorized = match check {
            Some(Some(check)) => cx.read(check),
            Some(None) => Ok(()),
            None => Err(unavailable("the caller stopped waiting for the call")),
        };
        if let Err(e) = authorized {
            work.finish(ticket, Err(e));
            continue;
        }
        if editor_reload {
            let ready = cx.read(|m| {
                let q = lock(&queue);
                let Some(reload) = q.reload.as_ref().filter(|r| r.ticket == ticket) else {
                    return Err(unavailable("the editor reload ticket was withdrawn"));
                };
                if active_options(m)? != reload.options {
                    return Err(refused("the active game options changed before reload"));
                }
                let fault = |e: engine::MemFault| {
                    refused(format!("cannot read the pending game transition at {:#x}", e.addr))
                };
                if m.u8(OPTIONS_PENDING).map_err(fault)? != 0 {
                    return Err(refused("the game already has a pending map transition"));
                }
                // Reconciliation must be able to read the entire retail
                // destination even when the flag at dispatch is clear.
                let mut bytes = [0; OPTIONS_SIZE as usize];
                m.read(PENDING_OPTIONS, &mut bytes).map_err(fault)
            });
            if let Err(e) = ready {
                work.finish(ticket, Err(e));
                continue;
            }
        }
        let out = match run_call(&cx, &call, scratch, &queue, editor_reload.then_some(ticket)).await {
            Ok(out) => out,
            Err(e) => {
                if !editor_reload || !lock(&queue).handoff.as_ref().is_some_and(|h| h.ticket == ticket) {
                    work.finish(ticket, Err(unavailable(format!("the game faulted: {e}"))));
                }
                return Err(e);
            }
        };
        let handoff = if editor_reload {
            lock(&queue).handoff.take_if(|h| h.ticket == ticket)
        } else {
            None
        };
        if let Some(handoff) = handoff {
            let pending = cx.read(|m| pending_options(m, handoff.options));
            if let Err(error) = settle_reload(&queue, handoff, pending) {
                work.finish(ticket, Err(error));
                continue;
            }
        }
        work.finish(ticket, out);
    }
    reentry.store(true, Ordering::SeqCst);
    let _reentry = TickReentry(reentry);
    cx.call(GuestCall::new(engine::points::GAME_TICK.base)).await?;
    Ok(Ret::new(0, 0))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PendingOptions {
    Empty,
    Reload,
    Other,
}

fn pending_options(m: &dyn GuestMemory, options: u32) -> Result<PendingOptions, String> {
    let fault = |e: engine::MemFault| format!("cannot read queued options at {:#x}", e.addr);
    if m.u8(OPTIONS_PENDING).map_err(fault)? == 0 {
        return Ok(PendingOptions::Empty);
    }
    // OPTIONS_QUEUE copies the block from EAX. The scratch still holds
    // exactly what retail's before hooks left there.
    // It belongs to this ticket until reconciliation releases game_ticket.
    let queued = m.bytes(PENDING_OPTIONS, OPTIONS_SIZE as usize).map_err(fault)?;
    let requested = m.bytes(options, OPTIONS_SIZE as usize).map_err(fault)?;
    Ok(if queued == requested { PendingOptions::Reload } else { PendingOptions::Other })
}

/// Drop the DVD reservation even if a cache key is temporarily unwritable.
/// Retry only the failed keys, without holding up the ticket or another tick.
fn cancel_binding(queue: &Shared, binding: Option<ReservedMapReload>, why: String) -> String {
    if let Some(mut binding) = binding
        && let Err(restore) = (binding.restore)()
    {
        lock(queue).repair = Some(CacheRepair(binding.restore));
        return format!("{why}; {restore}");
    }
    why
}

/// An interrupted guest call may still have queued this reload. Resolve
/// that outcome at the first safe boundary, before retail can use the cache.
fn reconcile_reload(queue: &Shared, pending_options: impl FnOnce(u32) -> Result<PendingOptions, String>) {
    let (repair, handoff) = {
        let mut q = lock(queue);
        (q.repair.take(), q.handoff.take_if(|h| h.interrupted))
    };
    if let Some(mut repair) = repair
        && (repair.0)().is_err()
    {
        lock(queue).repair = Some(repair);
    }
    let Some(load) = handoff else { return };
    let ticket = load.ticket;
    let pending = pending_options(load.options);
    let result = settle_reload(queue, load, pending).map(|()| json!({ "queued": true }));
    lock(queue).finish(ticket, result);
}

fn settle_reload(
    queue: &Shared,
    load: ReloadHandoff,
    pending: Result<PendingOptions, String>,
) -> Result<(), RpcError> {
    let why = match pending {
        Ok(PendingOptions::Reload) => {
            let transition = activation::Transition::queued(load.ticket, load.options);
            load.publish();
            lock(queue).transition = Some(transition);
            return Ok(());
        }
        Ok(PendingOptions::Other) => "another game transition superseded the reload".into(),
        Ok(PendingOptions::Empty) => "the game did not queue the reload".into(),
        Err(why) => why,
    };
    Err(unavailable(cancel_binding(queue, load.binding, why)))
}

fn recover_before(c: &mut hooks::Call<'_>, queue: &Shared) -> Result<hooks::Flow, runtime::GuestFault> {
    reconcile_reload(queue, |options| pending_options(&*c.ctx.runtime().memory(), options));
    Ok(hooks::Flow::Continue)
}

mod activation;
pub use activation::reload_hook_points;
pub(crate) use activation::install_reload_recovery;

/// The feature id (`H2_FEATURES=-agent-calls` leaves the calls out).
pub const FEATURE: &str = "agent-calls";

/// The `before` on `game_tick` that makes the queued calls (module doc).
pub struct CallsFeature(pub Shared);

impl hooks::Feature for CallsFeature {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new(
            FEATURE,
            "Agent API: the game's own functions for sim.give, sim.spawn, teleports and maps.test.load",
        )
        .doc("docs/runtime/agent-api.md")
    }

    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        use hooks::points::GAME_TICK;
        let q = Arc::clone(&self.0);
        b.snapshot(hooks::SnapshotPolicy::forbid_while(move || {
            lock(&q).live()
        }));
        let scratch = b.guest_alloc(SCRATCH_SIZE, runtime::guest::Permissions::RW)?;
        let q = Arc::clone(&self.0);
        let reentry = Arc::new(AtomicBool::new(false));
        b.raw()
            .before(&GAME_TICK, "the agent's guest calls", move |c| {
                if reentry.load(Ordering::SeqCst) {
                    return Ok(hooks::Flow::Continue);
                }
                let work = lock(&q).drain();
                if work.is_empty() {
                    return Ok(hooks::Flow::Continue);
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

mod placement;

#[cfg(test)]
mod tests;
