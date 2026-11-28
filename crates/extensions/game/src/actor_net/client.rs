//! One distributed client's puppets, owned by its actor-channel session.
//! See docs/game/ai-networking.md, "On a client, each tick".

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use ::actor_net::{Actor, ActorChange, Check, Control, Packet, Record, Replica, SetError, Verdict};
use guest_layout::entry::{Cx, EntryError, GuestCall, Ret};
use guest_layout::{DatumHandle, points};
use hooks::{Builder, Flow};

mod guest;

/// An actor-channel record can lead creation of its retail body by this
/// many local simulation ticks.
const WAIT_TICKS: u32 = 30;
/// Cue ids `(tick, sequence)` kept so a cue's three copies show once: a
/// copy comes in one of the next two packets.
const SEEN_CUES: usize = 32;
/// The named error a client reports when its digests keep disagreeing
/// with the host's after keys (docs/game/ai-networking.md, "Determinism
/// and digests").
pub const DIVERGED: &str = "actor_channel_diverged";

/// A show-channel cue received over the actor channel, before it is
/// checked: the rules runtime decodes it against the running script's
/// blob (`crate::rules::world::show::present`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceivedCue {
    pub tick: u32,
    pub sequence: u8,
    pub kind: u8,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Puppet {
    pub actor: Actor,
    pub unit: DatumHandle,
}

#[derive(Default)]
struct Presentation {
    unit: Option<DatumHandle>,
    posture_at: Option<u32>,
    control: Option<(u32, Control)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Line {
    entity: u32,
    vocalization: u16,
    tick: u32,
}

/// Create one per match. Both ingress and presentation run on the game
/// thread. Transport owns the receive queue and selects the session.
#[derive(Default)]
pub struct Client {
    replica: Replica,
    presentation: BTreeMap<u32, Presentation>,
    heard_or_queued: HashSet<Line>,
    pending_lines: Vec<(Line, u32)>,
    /// Cue ids seen, so a cue's three copies show once.
    seen_cues: VecDeque<(u32, u8)>,
    /// Cues received and not yet presented at a tick boundary.
    pending_cues: Vec<ReceivedCue>,
    /// Show states received and not yet held at a tick boundary, in
    /// packet order: the newest of them is the one kept.
    pending_show: Vec<Vec<u8>>,
    /// The run of digests that disagreed with the replica.
    digest: Check,
    /// The replica needs a key: a digest disagreed or a packet could not
    /// be applied. Taken by the transport ([`Client::take_key_request`]).
    wants_key: bool,
    /// The host log head of the last digest.
    host_log_head: Option<[u8; 32]>,
    /// The channel closed for this match after
    /// [`::actor_net::digest::DIVERGED_AFTER`] disagreeing digests in a row
    /// with keys between them.
    diverged: bool,
}

/// The channel's state on a client, for the agent API.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status {
    /// Disagreeing digests in the current run.
    pub digest_misses: u8,
    /// A key has been asked for and not yet sent.
    pub key_requested: bool,
    /// The host log head of the last digest.
    pub host_log_head: Option<[u8; 32]>,
    /// The channel closed for this match ([`DIVERGED`]).
    pub diverged: bool,
}

fn waiting(received: u32, now: u32) -> bool {
    now.wrapping_sub(received) < WAIT_TICKS
}

impl Client {
    /// Take one packet the transport drained. A packet that cannot be
    /// applied asks for a key; once the channel has diverged, packets are
    /// dropped for the rest of the match. Returns whether it was applied.
    pub fn receive(&mut self, packet: &Packet, local_tick: u32) -> bool {
        if self.diverged {
            return false;
        }
        match self.apply(packet, local_tick) {
            Ok(applied) => applied,
            Err(error) => {
                eprintln!(
                    "actor-net: host tick {} not applied, asking for a key: {error}",
                    packet.tick
                );
                self.wants_key = true;
                false
            }
        }
    }

    /// Whether the transport should ask the host for a key now. Asks once
    /// per need; a closed channel asks for none.
    pub fn take_key_request(&mut self) -> bool {
        std::mem::take(&mut self.wants_key) && !self.diverged
    }

    pub fn status(&self) -> Status {
        Status {
            digest_misses: self.digest.misses(),
            key_requested: self.wants_key,
            host_log_head: self.host_log_head,
            diverged: self.diverged,
        }
    }

    /// The client's side of `actors.net` (`agent::actors::Client`).
    pub fn report(&self) -> agent::actors::Client {
        let actors = self.replica.set().actors().len();
        let puppets: Vec<_> = self
            .puppets()
            .map(|p| agent::actors::Puppet {
                entity: p.actor.entity,
                character: p.actor.character,
                squad: p.actor.squad,
                team: p.actor.team,
                posture: p.actor.posture,
                unit: p.unit.raw(),
            })
            .collect();
        agent::actors::Client {
            actors,
            waiting: actors - puppets.len(),
            puppets,
            lines_waiting: self.pending_lines.len(),
            mode_state_bytes: self.mode_state().len(),
            digest_misses: self.digest.misses(),
            key_requested: self.wants_key,
            diverged: self.diverged,
            host_log_head: self.host_log_head.as_ref().map(agent::actors::hex),
        }
    }

    /// Close the channel for this match: forget the puppets, the mode
    /// state and anything waiting. Retail replication still carries the
    /// bodies.
    fn diverge(&mut self) {
        eprintln!(
            "actor-net: {DIVERGED}: {} digests in a row disagree with the actor set, keys between them; the actor channel is closed for this match",
            self.digest.misses()
        );
        let (digest, host_log_head) = (self.digest, self.host_log_head);
        *self = Client {
            digest,
            host_log_head,
            diverged: true,
            ..Client::default()
        };
        // The offers rode the channel that just closed: the asking stops.
        #[cfg(feature = "rules")]
        crate::rules::world::pick::client_reset();
    }

    /// Resolve the host packet atomically before recording presentation work.
    /// `local_tick` is the local simulation tick at which transport drains it.
    pub fn apply(&mut self, packet: &Packet, local_tick: u32) -> Result<bool, SetError> {
        if !self.replica.apply(packet)? {
            return Ok(false);
        }
        for record in &packet.records {
            match record {
                Record::Key { actors, .. } => {
                    self.digest.key_applied();
                    self.presentation
                        .retain(|entity, _| actors.iter().any(|actor| actor.entity == *entity));
                    self.pending_lines
                        .retain(|(line, _)| self.presentation.contains_key(&line.entity));
                    for actor in actors {
                        self.presentation
                            .entry(actor.entity)
                            .or_default()
                            .posture_at = Some(local_tick);
                    }
                }
                Record::Actor(ActorChange::Upsert(actor)) => {
                    self.presentation
                        .entry(actor.entity)
                        .or_default()
                        .posture_at = Some(local_tick);
                }
                Record::Actor(ActorChange::Remove(entity)) => {
                    self.presentation.remove(entity);
                    self.pending_lines
                        .retain(|(line, _)| line.entity != *entity);
                }
                Record::Posture { entity, .. } => {
                    if let Some(state) = self.presentation.get_mut(entity) {
                        state.posture_at = Some(local_tick);
                    }
                }
                Record::Control { entity, control } => {
                    if let Some(state) = self.presentation.get_mut(entity) {
                        state.control = Some((local_tick, *control));
                    }
                }
                Record::Line {
                    entity,
                    vocalization,
                    tick,
                } => {
                    let line = Line {
                        entity: *entity,
                        vocalization: *vocalization,
                        tick: *tick,
                    };
                    if self.presentation.contains_key(entity) && self.heard_or_queued.insert(line) {
                        self.pending_lines.push((line, local_tick));
                    }
                }
                Record::State(_) => {}
                Record::Show(bytes) => self.pending_show.push(bytes.clone()),
                Record::Digest {
                    log_head,
                    state_hash,
                    actor_count,
                    show_hash,
                } => {
                    // The replica holds the whole packet by now, so the
                    // digest sees the tick's changes wherever it sits.
                    self.host_log_head = Some(*log_head);
                    match self
                        .digest
                        .observe(self.replica.set(), *state_hash, *actor_count)
                    {
                        Verdict::Agrees => {}
                        Verdict::RequestKey => self.wants_key = true,
                        Verdict::Diverged => {
                            self.diverge();
                            return Ok(true);
                        }
                    }
                    // The show state's hash beside the actor set's: a
                    // mismatch asks for a key, which carries the state
                    // whole (docs/game/rules-world-api.md, "The show
                    // channel"). The hash is over the state this very
                    // packet carries when it has one, else over what is
                    // held; a record the checks later refuse leaves the
                    // held one, which the next digest catches. A machine
                    // without the `rules` feature shows no script's HUD
                    // and asks for none.
                    #[cfg(feature = "rules")]
                    {
                        let offered = packet.records.iter().rev().find_map(|r| match r {
                            Record::Show(bytes) => Some(bytes.clone()),
                            _ => None,
                        });
                        let held = offered.or_else(crate::rules::world::show::held_bytes);
                        if ::actor_net::digest::state_hash(held.as_deref().unwrap_or(&[]))
                            != *show_hash
                        {
                            self.wants_key = true;
                        }
                    }
                }
                Record::Cue {
                    tick,
                    sequence,
                    kind,
                    bytes,
                } => {
                    let id = (*tick, *sequence);
                    if !self.seen_cues.contains(&id) {
                        if self.seen_cues.len() == SEEN_CUES {
                            self.seen_cues.pop_front();
                        }
                        self.seen_cues.push_back(id);
                        self.pending_cues.push(ReceivedCue {
                            tick: *tick,
                            sequence: *sequence,
                            kind: *kind,
                            bytes: bytes.clone(),
                        });
                    }
                }
            }
        }
        Ok(true)
    }

    /// The host's complete mode blob for the last accepted tick. The HUD
    /// and trait query read their own parts through `super::roles`.
    pub fn mode_state(&self) -> &[u8] {
        self.replica.set().state()
    }

    /// Live local bodies and the host metadata used to label them.
    pub fn puppets(&self) -> impl Iterator<Item = Puppet> + '_ {
        self.replica.set().actors().iter().filter_map(|actor| {
            self.presentation[&actor.entity].unit.map(|unit| Puppet {
                actor: *actor,
                unit,
            })
        })
    }

    /// Run on the game thread before `GAME_TICK`, after applying the queued
    /// packets. Revalidate each body even on a tick without a packet.
    pub async fn before_tick(&mut self, cx: &Cx, local_tick: u32) -> Result<(), EntryError> {
        let world = cx.read(guest::client_world)?;
        for actor in self.replica.set().actors() {
            let state = self
                .presentation
                .get_mut(&actor.entity)
                .expect("actor presentation");
            state.posture_at = state.posture_at.filter(|&at| waiting(at, local_tick));
            state.control = state.control.filter(|&(at, _)| waiting(at, local_tick));
            let Some(body) = cx.read(|m| guest::resolve(m, world, actor.entity))? else {
                state.unit = None;
                continue;
            };
            state.unit = Some(body.unit);
            let posture = state.posture_at.map(|_| actor.posture);
            let control = state.control.map(|(_, control)| control);
            if let Some(call) =
                cx.read(|m| guest::control_call(m, cx.entry().esp, body, posture, control))?
            {
                cx.call(call).await?;
            }
            state.posture_at = None;
            state.control = None;
        }

        self.pending_lines
            .retain(|(_, at)| waiting(*at, local_tick));
        let mut index = 0;
        while let Some(&(line, _)) = self.pending_lines.get(index) {
            let Some(body) = cx.read(|m| guest::resolve(m, world, line.entity))? else {
                index += 1;
                continue;
            };
            let sound = cx.read(|m| guest::line_sound(m, body, line.vocalization))?;
            // Consume before making a call so an error after starting a sound
            // (for example in lip sync) never starts the same line again.
            self.pending_lines.remove(index);
            if let Some(sound) = sound {
                guest::play_line(cx, body, sound).await?;
            }
        }

        // The show channel's cues: decoded against the running script's
        // blob and shown to this machine's local users in each audience
        // (`crate::rules::world::show::present`). The `Show` states beside
        // them: the newest held, and the HUDs it feeds drawn from it on a
        // machine that runs no VM (`crate::rules::world::show::receive`).
        // Without the `rules` feature there is no script to show them by,
        // and they are dropped. The pickers' offers beside them: this
        // machine's local players are asked at the boundary while the
        // asking waits (`crate::rules::world::pick::client_tick`), and a
        // press goes back up the channel.
        let cues = std::mem::take(&mut self.pending_cues);
        let shows = std::mem::take(&mut self.pending_show);
        #[cfg(feature = "rules")]
        {
            if !cues.is_empty() {
                crate::rules::world::show::present(cx, &cues).await;
            }
            if !shows.is_empty() {
                crate::rules::world::show::receive(cx, &shows).await;
            }
            crate::rules::world::pick::client_tick(cx, local_tick).await;
        }
        #[cfg(not(feature = "rules"))]
        drop((cues, shows));
        Ok(())
    }
}

/// Add the client half to the actor-net feature. The receive callback drains
/// AN5's inbox on the game thread. Reset the returned state when transport
/// changes sessions or its snapshot policy restores the guest.
pub fn install(
    b: &mut Builder,
    receive: impl Fn(&mut Client, u32) + Send + Sync + 'static,
) -> Arc<Mutex<Client>> {
    let state = Arc::new(Mutex::new(Client::default()));
    let new_match = Arc::clone(&state);
    b.game()
        .on_start("clear actor puppets for the new match", move |_| {
            *new_match.lock().unwrap() = Client::default();
            super::roles::receive(&[]);
            #[cfg(feature = "rules")]
            crate::rules::world::show::clear_held();
            #[cfg(feature = "rules")]
            crate::rules::world::pick::client_reset();
        });

    let client = Arc::clone(&state);
    let reentry = Arc::new(AtomicBool::new(false));
    b.raw()
        .before(
            &points::GAME_TICK,
            "apply replicated actor presentation",
            move |call| {
                if reentry.load(Ordering::Relaxed) {
                    return Ok(Flow::Continue);
                }
                let tick = {
                    let m = call.ctx.runtime().memory();
                    if guest::client_world(&*m).is_ok_and(|world| world.is_some()) {
                        bots::guest::game_tick(&*m)
                    } else {
                        None
                    }
                };
                let (mut current, tick) = {
                    let mut current = client.lock().unwrap();
                    let Some(tick) = tick else {
                        *current = Client::default();
                        super::roles::receive(&[]);
                        #[cfg(feature = "rules")]
                        crate::rules::world::show::clear_held();
                        #[cfg(feature = "rules")]
                        crate::rules::world::pick::client_reset();
                        super::report::clear(agent::actors::Role::Client);
                        return Ok(Flow::Continue);
                    };
                    receive(&mut current, tick);
                    super::report::publish_client(tick, &current);
                    super::roles::receive(current.mode_state());
                    // A picker's ask waiting on a client runs the boundary
                    // too, so the asking advances
                    // (`crate::rules::world::pick::client_tick`).
                    #[cfg(feature = "rules")]
                    let asks_waiting = crate::rules::world::pick::client_busy();
                    #[cfg(not(feature = "rules"))]
                    let asks_waiting = false;
                    if current.presentation.is_empty()
                        && current.pending_cues.is_empty()
                        && current.pending_show.is_empty()
                        && !asks_waiting
                    {
                        return Ok(Flow::Continue);
                    }
                    (std::mem::take(&mut *current), tick)
                };
                let (client, reentry) = (Arc::clone(&client), Arc::clone(&reentry));
                call.answer_async(move |cx| {
                    Box::pin(async move {
                        if let Err(error) = current.before_tick(&cx, tick).await {
                            eprintln!("actor-net: client presentation at tick {tick}: {error}");
                        }
                        *client.lock().unwrap() = current;
                        reentry.store(true, Ordering::Relaxed);
                        let result = cx.call(GuestCall::new(points::GAME_TICK.base)).await;
                        reentry.store(false, Ordering::Relaxed);
                        Ok(Ret::new(result?.eax, 0))
                    })
                })
            },
        )
        .first()
        // Inside the agent's calls, the skulls', the map select's, the dedicated
        // lobby's and the HUD kit's, which run the tick again.
        .after_feature(agent::sim::calls::FEATURE)
        .after_feature(crate::skulls_custom::FEATURE)
        .after_feature(crate::map_select::FEATURE)
        .after_feature("host-lobby")
        .after_feature(crate::hud_kit::FEATURE);
    state
}

#[cfg(test)]
mod tests;
