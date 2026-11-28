//! Local Forge sessions submit edits at a tick boundary. The ordinary
//! tools and the agent share the same host; the call queue owns guest entry.
use std::collections::BTreeMap;
use std::sync::{Arc, mpsc};

use ::forge::look::{Refusal, monitors::Who};
use ::forge::{BlockKind, Frame, PaletteItem, Session, World};
use agent_proto::forge::{
    GROUND_WAIT_TICKS, GroundBuildParams, NAV_WAIT_TICKS, NavParams, THING_KINDS, ThingsParams,
    WavesParams, method, refusal,
};
use agent_proto::{Boundary, Kind, ObjectId, RpcError, code};
use guest_layout::GuestMemory;
use forge_doc::{Entry, ForgeDoc, Op, Sha256, TagKey};
use map::scenario_layout as layout;
use serde::Deserialize;
use serde_json::{Value, json};

use super::base::{self, BaseStep};
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
pub const OBJECT_GONE: &str = refusal::OBJECT_GONE;

/// A live object edit names a block whose objects `world.object.*` cannot
/// resolve.
pub const OBJECT_KIND: &str = refusal::OBJECT_KIND;

/// A base edit names an own entry whose live object is held or attached, so
/// it is not the placement's to move.
pub const OBJECT_HELD: &str = refusal::OBJECT_HELD;

/// A kit add names a spare row the map does not have: it has no kit.
pub const NO_KIT: &str = refusal::NO_KIT;

/// The name a refused edit's sentence carries (`agent_proto::forge::refusal`):
/// a sentence that starts with a game call's name has it, a print or `was`
/// check that failed has its own, and any other is
/// [`refusal::REFUSED`]. The print and `was` sentences are forge-doc's own
/// `Refusal`s, so they are matched against those.
pub fn refusal_name(message: &str) -> &'static str {
    // A shared session's refusals lead with their name and a colon.
    if let Some((head, _)) = message.split_once(": ")
        && let Some(name) = refusal::SHARED.into_iter().find(|name| *name == head)
    {
        return name;
    }
    if let Some(name) = [OBJECT_GONE, OBJECT_KIND, OBJECT_HELD, NO_KIT]
        .into_iter()
        .find(|name| message.starts_with(name))
    {
        return name;
    }
    // The session's own sentence for a map with no kit.
    if message.starts_with(::forge::kit::NO_KIT) {
        return NO_KIT;
    }
    let print = forge_doc::Refusal::Print {
        block: BlockKind::Weapons,
        index: 0,
    }
    .to_string();
    if let Some((head, tail)) = print.split_once(" weapons entry 0 ")
        && message.starts_with(head)
        && message.ends_with(tail)
    {
        return refusal::PRINT;
    }
    if message == forge_doc::Refusal::Mismatch.to_string() {
        return refusal::WAS;
    }
    if message.contains(::forge::LEFT_OUT_SENTENCE) {
        return refusal::LEFT_OUT;
    }
    refusal::REFUSED
}

/// `forge.status`'s `variant`: the session's document as a map variant
/// would save it (docs/game/forge-map-variants.md, "The map variant
/// file"). `name` is the file it was last saved or loaded as, `dirty` that
/// it has edits since. `id` is the play form's identity and `tier` its
/// tier; a document whose identity does not compute has `id` null and
/// `id_refused` with the reason.
pub fn variant_status(doc: &ForgeDoc, name: Option<&str>, dirty: bool) -> Value {
    let play = forge_doc::play_form(doc);
    let mut out = json!({
        "name": name,
        "title": play.title,
        "tier": play.tier().name(),
        "dirty": dirty,
    });
    match play.id() {
        Ok(id) => out["id"] = json!(id),
        Err(e) => {
            out["id"] = Value::Null;
            out["id_refused"] = json!(e.to_string());
        }
    }
    out
}

/// One thing as `forge.things` lists it: its endpoint (the target an entry
/// op names), block, kind, whether the base map has it and an entry op
/// changes it, its frame, and its fields by layout name.
pub fn thing_json(thing: &::forge::Thing) -> Value {
    let fields: serde_json::Map<String, Value> = thing
        .fields
        .iter()
        .map(|(name, value)| ((*name).to_owned(), json!(value)))
        .collect();
    json!({
        "id": thing.id,
        "block": thing.block.name(),
        "kind": thing_kind(thing.kind),
        "own": thing.own,
        "changed": thing.changed,
        "position": thing.frame.position,
        "rotation": thing.frame.rotation,
        "fields": fields,
    })
}

/// A kind as `forge.things` spells it, one of `THING_KINDS`.
fn thing_kind(kind: ::forge::ThingKind) -> &'static str {
    use ::forge::ThingKind as K;
    match kind {
        K::Object => THING_KINDS[0],
        K::ItemSpawner => THING_KINDS[1],
        K::StartingPoint => THING_KINDS[2],
        K::Goal => THING_KINDS[3],
        K::Teleporter => THING_KINDS[4],
        K::Piece => THING_KINDS[5],
    }
}

/// `forge.things`' reply for `session`: its things that pass the filters,
/// and the blocks it left out, each as `block: reason`.
pub fn things_reply(session: &Session, params: &ThingsParams) -> Result<Value, RpcError> {
    if let Some(kind) = &params.kind
        && !THING_KINDS.contains(&kind.as_str())
    {
        return Err(super::invalid(format!(
            "kind: one of {}",
            THING_KINDS.join(", ")
        )));
    }
    let areas = session.areas().map_err(super::unavailable)?;
    let shows = session.shows().map_err(super::unavailable)?;
    let things: Vec<Value> = session
        .things()
        .into_iter()
        .filter(|t| {
            params
                .kind
                .as_deref()
                .is_none_or(|k| thing_kind(t.kind) == k)
        })
        .filter_map(|thing| {
            let area = areas.get(&thing.id.identity());
            let id = thing.id.identity();
            let changed = thing.changed || area.is_some() || shows.contains_key(&id);
            if params.changed.is_some_and(|wanted| changed != wanted) {
                return None;
            }
            let mut row = thing_json(&thing);
            if thing.block == BlockKind::NetgameFlags {
                row["area"] = json!(area);
                // A flag the document added is shown unless an op says
                // not; a flag of the map is shown only when one says so.
                row["show"] = json!(
                    shows
                        .get(&id)
                        .copied()
                        .unwrap_or(matches!(id, ::forge::Endpoint::Added(_)))
                );
                row["changed"] = json!(changed);
            }
            Some(row)
        })
        .collect();
    Ok(json!({"things": things, "left_out": session.left_out()}))
}

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
    Channel(u32, i16),
    /// A netgame item entry's spare clips.
    SpareClips(u32, Option<u8>),
    /// A starting location's weight.
    SpawnWeight(u32, Option<forge_doc::SpawnWeight>),
    /// A netgame item entry held back at a round's start, or placed.
    PlaceAtStart(u32, bool),
    /// An own entry's step that needs no tag resolved.
    Base(BaseStep),
    /// An own entry's field write, its value still a document value.
    Field {
        block: BlockKind,
        index: u32,
        field: &'static str,
        value: forge_doc::Value,
    },
    /// A kit row: the fields still document values.
    KitAdd {
        block: BlockKind,
        index: u32,
        fields: Vec<(&'static str, forge_doc::Value)>,
    },
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
    /// The block-out definitions the loaded map holds `bloc`s of: the
    /// names of the session's palette rows under `forge\\blockout\\`.
    pieces: std::collections::BTreeSet<String>,
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
            pieces: ::forge::piece::loaded_names(session.palette()),
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
                        Staged::Channel(flag, channel) => ForgeStep::Channel { flag, channel },
                        Staged::SpareClips(index, clips) => ForgeStep::SpareClips { index, clips },
                        Staged::SpawnWeight(index, weight) => {
                            ForgeStep::SpawnWeight { index, weight }
                        }
                        Staged::PlaceAtStart(index, held) => {
                            ForgeStep::PlaceAtStart { index, held }
                        }
                        Staged::Base(step) => ForgeStep::Base(step),
                        Staged::Field {
                            block,
                            index,
                            field,
                            value,
                        } => {
                            let (def, value) =
                                base::field_value(block, field, &value, |k| resolve(k))?;
                            ForgeStep::Base(BaseStep::Field {
                                block,
                                index,
                                field: def.name,
                                value,
                            })
                        }
                        Staged::KitAdd {
                            block,
                            index,
                            fields,
                        } => {
                            let row = base::new_row(block, &fields, |k| resolve(k))?;
                            ForgeStep::Base(BaseStep::Add { block, index, row })
                        }
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

    /// A definition is loaded when the map's palette has its `bloc`: the
    /// tag index resolves it ([`Staged::Make`]), as it does any palette tag.
    fn piece_loaded(&self, name: &str) -> bool {
        self.pieces.contains(name)
    }

    /// A teleporter link writes the flag's identifier in the loaded
    /// scenario, after the batch's objects.
    fn channel(&mut self, flag: u32, channel: i16) -> Result<(), String> {
        self.steps.push(Staged::Channel(flag, channel));
        Ok(())
    }

    /// A netgame item entry's spare clips: the game's table, and the weapon
    /// the entry has out, after the batch's objects.
    fn spare_clips(&mut self, index: u32, clips: Option<u8>) -> Result<(), String> {
        if clips.is_some_and(|c| c > forge_doc::SPARE_CLIPS_MAX) {
            return Err("Spare clips are 0 to 10.".into());
        }
        self.steps.push(Staged::SpareClips(index, clips));
        Ok(())
    }

    /// A starting location's weight: the game's table, after the batch's
    /// objects.
    fn spawn_weight(
        &mut self,
        index: u32,
        weight: Option<forge_doc::SpawnWeight>,
    ) -> Result<(), String> {
        self.steps.push(Staged::SpawnWeight(index, weight));
        Ok(())
    }

    /// A netgame item entry held back at a round's start: the game's
    /// table, after the batch's objects. It takes effect at the next round
    /// reset.
    fn place_at_start(&mut self, index: u32, held: bool) -> Result<(), String> {
        self.steps.push(Staged::PlaceAtStart(index, held));
        Ok(())
    }

    // The base calls: an own entry is named by its block and the index the
    // base map has it at, and each call stages one `BaseStep` (`base.rs`).

    /// Move and turn own entry `index` of `block`: its position and
    /// orientation in the loaded scenario, and its live object when the
    /// block makes objects.
    fn base_move(&mut self, block: BlockKind, index: u32, frame: Frame) -> Result<(), String> {
        if layout::block(block).role(layout::Role::Position).is_none() {
            return Err(format!("{} rows have no position", block.name()));
        }
        self.steps.push(Staged::Base(BaseStep::Move {
            block,
            index,
            frame,
        }));
        Ok(())
    }

    /// Set one typed, editable field of the entry in the loaded scenario.
    fn base_field(
        &mut self,
        block: BlockKind,
        index: u32,
        field: &str,
        value: &forge_doc::Value,
    ) -> Result<(), String> {
        let def = base::field_def(block, field)?;
        // The shape is checked now; a tag key resolves when the edit is applied.
        base::field_value(block, field, value, |_| Ok(0))?;
        self.steps.push(Staged::Field {
            block,
            index,
            field: def.name,
            value: value.clone(),
        });
        Ok(())
    }

    /// Make the entry's row inert and delete its live object.
    fn base_inert(&mut self, block: BlockKind, index: u32) -> Result<(), String> {
        self.steps
            .push(Staged::Base(BaseStep::Inert { block, index }));
        Ok(())
    }

    /// Write the kept `row` back and make the entry's object again.
    fn base_restore(&mut self, block: BlockKind, index: u32, row: &[u8]) -> Result<(), String> {
        let row = base::RowBytes::new(block, row)?;
        self.steps
            .push(Staged::Base(BaseStep::Restore { block, index, row }));
        Ok(())
    }

    /// Add an entry of a kit block as its row `index`, the next spare row.
    fn kit_add(
        &mut self,
        block: BlockKind,
        index: u32,
        fields: &[(&'static str, forge_doc::Value)],
    ) -> Result<(), String> {
        // The shape is checked now; a tag key resolves when the edit is applied.
        base::new_row(block, fields, |_| Ok(0))?;
        self.steps.push(Staged::KitAdd {
            block,
            index,
            fields: fields.to_vec(),
        });
        Ok(())
    }

    /// Take the kit block's last row, `index`, away.
    fn kit_drop(&mut self, block: BlockKind, index: u32) -> Result<(), String> {
        if !::forge::kit::is_block(block) {
            return Err(format!("the kit has no {} rows", block.name()));
        }
        self.steps
            .push(Staged::Base(BaseStep::Drop { block, index }));
        Ok(())
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

/// A ground build's answer: `forge.ground`'s reply with `built` set, or why
/// the build did not happen.
pub type GroundReply = mpsc::Receiver<Result<Value, String>>;

/// A generation's answer: `forge.nav`'s reply, or why it did not happen.
pub type NavReply = mpsc::Receiver<Result<Value, String>>;

pub trait LocalApi: Send + Sync + 'static {
    /// The profile and seat colour the monitor look methods speak for: the
    /// signed-in profile of local slot 0, from guest memory when there is
    /// one.
    fn who(&self, m: Option<&dyn GuestMemory>) -> Who;
    /// `forge.look`: the look, how it is drawn, the saved look and the
    /// catalogue. Works with no session.
    fn look(&self, who: &Who, seat: Option<u64>) -> Result<Value, Refusal>;
    /// `forge.look.set`. Refused outside a session.
    fn look_set(&self, who: &Who, params: Value) -> Result<Value, Refusal>;
    /// `forge.status.monitors`.
    fn monitors(&self, who: &Who) -> Value;
    fn status(&self) -> Result<Value, String>;
    /// The session as it stands, for `forge.things`; the reason there is
    /// none otherwise.
    fn session(&self) -> Result<Arc<Session>, String>;
    fn document(&self) -> Result<ForgeDoc, String>;
    fn log(&self, since: u32) -> Result<Vec<Entry>, String>;
    fn input(&self, m: &dyn GuestMemory, input: ::forge::tool::Input) -> Result<(), String>;
    fn propose(&self, tick: u32, op: Op, was: Option<Sha256>) -> Result<ProposalReply, String>;
    /// `forge.ground`: the ground's state, markers and last report. The
    /// default reads the session and says nothing the game alone knows.
    fn ground(&self) -> Result<Value, String> {
        Ok(super::ground::reply(
            &*self.session()?,
            super::ground::Facts::default(),
        ))
    }
    /// `forge.ground.build`: start a build (**Build AI ground**, or with
    /// `if_stale` **Test waves**) and answer when it has finished. With
    /// `classes`, ask for those enemy classes first
    /// (`Session::set_ground_classes`).
    fn ground_build(
        &self,
        _if_stale: bool,
        _classes: Option<Vec<String>>,
    ) -> Result<GroundReply, String> {
        Err(super::ground::NO_BUILDER.into())
    }
    /// `forge.nav`: the nav's state, the last generation's counts,
    /// problems and overlay. The default reads the session and says
    /// nothing the game alone knows.
    fn nav(&self) -> Result<Value, String> {
        Ok(super::nav::reply(
            &*self.session()?,
            super::nav::Facts::default(),
        ))
    }
    /// `forge.nav {generate: true}`: **Generate nav**, answering when it
    /// has finished.
    fn nav_generate(&self) -> Result<NavReply, String> {
        Err(super::nav::NO_GENERATOR.into())
    }
    /// `forge.waves`: the session's wave set, its id, its problems and the
    /// drops in zone order. The default reads the session.
    fn waves(&self) -> Result<Value, String> {
        Ok(super::waves::reply(&*self.session()?))
    }
    /// `forge.waves {set}`: replace the session's set with `set` (`None`
    /// clears it) and answer `forge.waves`' reply. A refusal is the
    /// sentence and changes nothing.
    fn set_waves(&self, _set: Option<forge_doc::waves::Def>) -> Result<Value, String> {
        Err(super::waves::NO_EDIT.into())
    }
    /// How many game ticks `forge.propose` waits for its answer. A shared
    /// session's goes to the host and back, and waits for the tick the
    /// stream applies the entry at.
    fn wait_ticks(&self) -> u64 {
        30
    }
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

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Look {
    seat: Option<u64>,
}

/// A monitor look refusal as an error: its code and sentence in the
/// message, and `{code, message, ...fields}` as the data.
fn look_refusal(r: Refusal) -> RpcError {
    let mut e = RpcError::new(code::INVALID_REQUEST, r.to_string());
    e.data = Some(r.to_value());
    e
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
        let mut status = host.status().map_err(super::unavailable)?;
        // The set is the session's, beside the document `variant` reads.
        if let Ok(session) = host.session()
            && let Some(variant) = status.get_mut("variant").filter(|v| v.is_object())
        {
            variant["waves"] = super::waves::status(&session);
        }
        Ok(status)
    });
    let host = backend.clone();
    agent.add_game_method(method::LOOK, Access::Read, move |v, ctx| {
        let p: Look = if v.is_null() {
            Look::default()
        } else {
            serde_json::from_value(v).map_err(|e| super::invalid(e.to_string()))?
        };
        let who = who_of(&*host, ctx);
        host.look(&who, p.seat).map_err(look_refusal)
    });
    let host = backend.clone();
    agent.add_game_method(method::LOOK_SET, Access::Act, move |v, ctx| {
        let who = who_of(&*host, ctx);
        host.look_set(&who, v).map_err(look_refusal)
    });
    let host = backend.clone();
    agent.add_game_method(method::MONITORS, Access::Read, move |v, ctx| {
        empty(v)?;
        let who = who_of(&*host, ctx);
        Ok(host.monitors(&who))
    });
    let host = backend.clone();
    agent.add_game_method(method::THINGS, Access::Read, move |v, _| {
        let p: ThingsParams = if v.is_null() {
            ThingsParams::default()
        } else {
            serde_json::from_value(v).map_err(|e| super::invalid(e.to_string()))?
        };
        let session = host.session().map_err(super::unavailable)?;
        things_reply(&session, &p)
    });
    let host = backend.clone();
    agent.add_game_method(method::PIECES, Access::Read, move |v, _| {
        empty(v)?;
        let session = host.session().map_err(super::unavailable)?;
        Ok(super::pieces::reply(&session))
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
    let host = backend.clone();
    agent.add_game_method(method::GROUND, Access::Read, move |v, _| {
        empty(v)?;
        host.ground().map_err(super::unavailable)
    });
    let host = backend.clone();
    agent.add_game_method(method::WAVES, Access::Act, move |v, _| {
        let p: WavesParams = if v.is_null() {
            WavesParams::default()
        } else {
            serde_json::from_value(v).map_err(|e| super::invalid(e.to_string()))?
        };
        match p.set {
            None => host.waves().map_err(super::unavailable),
            Some(set) => {
                let def = super::waves::parse(set).map_err(waves_refused)?;
                host.set_waves(def).map_err(waves_refused)
            }
        }
    });
    let host = backend.clone();
    agent.add_game_method(method::NAV, Access::Act, move |v, ctx| {
        let p: NavParams = if v.is_null() {
            NavParams::default()
        } else {
            serde_json::from_value(v).map_err(|e| super::invalid(e.to_string()))?
        };
        if !p.generate {
            return host.nav().map_err(super::unavailable);
        }
        if ctx.at.boundary != Boundary::Tick || ctx.at.mode.as_deref() != Some("multiplayer") {
            return Err(super::unavailable(
                "nav generation requires a local multiplayer tick",
            ));
        }
        let reply = host.nav_generate().map_err(refused)?;
        ctx.defer(
            NAV_WAIT_TICKS,
            Box::new(move |_| match reply.try_recv() {
                Ok(result) => Some(result.map_err(refused)),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    Some(Err(super::unavailable("the Forge session ended")))
                }
            }),
        );
        Ok(Value::Null)
    });
    let host = backend.clone();
    agent.add_game_method(method::GROUND_BUILD, Access::Act, move |v, ctx| {
        if ctx.at.boundary != Boundary::Tick || ctx.at.mode.as_deref() != Some("multiplayer") {
            return Err(super::unavailable(
                "a ground build requires a local multiplayer tick",
            ));
        }
        let p: GroundBuildParams = if v.is_null() {
            GroundBuildParams::default()
        } else {
            serde_json::from_value(v).map_err(|e| super::invalid(e.to_string()))?
        };
        let reply = host
            .ground_build(p.if_stale, p.classes)
            .map_err(refused)?;
        ctx.defer(
            GROUND_WAIT_TICKS,
            Box::new(move |_| match reply.try_recv() {
                Ok(result) => Some(result.map_err(refused)),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    Some(Err(super::unavailable("the Forge session ended")))
                }
            }),
        );
        Ok(Value::Null)
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
            backend.wait_ticks(),
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

/// Who the look methods speak for, from guest memory where there is any.
fn who_of(host: &impl LocalApi, ctx: &mut crate::GameCtx<'_>) -> Who {
    ctx.with_guest(|m| Ok(host.who(Some(&*m))))
        .unwrap_or_else(|_| host.who(None))
}

/// A refused `forge.waves {set}`: its sentence as the message, and
/// `{code: "forge_waves", message}` as the data.
fn waves_refused(message: String) -> RpcError {
    let mut e = RpcError::new(code::INVALID_REQUEST, message.clone());
    e.data = Some(json!({"code": refusal::WAVES, "message": message}));
    e
}

/// A refused input or edit: its sentence as the message, and
/// `{code, message}` as the data, `code` its name ([`refusal_name`]).
fn refused(message: String) -> RpcError {
    let name = refusal_name(&message);
    let mut e = RpcError::new(code::INVALID_REQUEST, message.clone());
    e.data = Some(json!({"code": name, "message": message}));
    e
}
