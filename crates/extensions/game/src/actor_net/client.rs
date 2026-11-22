//! One distributed client's puppets, owned by its actor-channel session.
//! See docs/game/ai-networking.md, "On a client, each tick".

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use ::actor_net::{Actor, ActorChange, Control, Packet, Record, Replica, SetError};
use engine::entry::{Cx, EntryError, GuestCall, Ret};
use engine::{DatumHandle, points};
use hooks::{Builder, Flow};

use crate::rules::world::show::ReceivedCue;

mod guest;

/// An actor-channel record can lead creation of its retail body by this
/// many local simulation ticks.
const WAIT_TICKS: u32 = 30;
/// Cue ids `(tick, sequence)` kept so a cue's three copies show once: a
/// copy comes in one of the next two packets.
const SEEN_CUES: usize = 32;

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
}

fn waiting(received: u32, now: u32) -> bool {
    now.wrapping_sub(received) < WAIT_TICKS
}

impl Client {
    /// Resolve the host packet atomically before recording presentation work.
    /// `local_tick` is the local simulation tick at which transport drains it.
    pub fn apply(&mut self, packet: &Packet, local_tick: u32) -> Result<bool, SetError> {
        if !self.replica.apply(packet)? {
            return Ok(false);
        }
        for record in &packet.records {
            match record {
                Record::Key { actors, .. } => {
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
                Record::State(_) | Record::Digest { .. } => {}
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

    /// The host's mode state as last received: the Firefight HUD's blob
    /// (`crate::firefight::hud`), empty when none was sent.
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
        // (`crate::rules::world::show::present`).
        if !self.pending_cues.is_empty() {
            let cues = std::mem::take(&mut self.pending_cues);
            crate::rules::world::show::present(cx, &cues);
        }
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
            crate::firefight::hud::received().set(None);
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
                        crate::firefight::hud::received().set(None);
                        return Ok(Flow::Continue);
                    };
                    receive(&mut current, tick);
                    // The HUD reads the mode state from here, not from the
                    // client, which is taken out of its lock while its
                    // presentation runs.
                    let mode = current.mode_state();
                    crate::firefight::hud::received()
                        .set((!mode.is_empty()).then(|| mode.to_vec()));
                    if current.presentation.is_empty() && current.pending_cues.is_empty() {
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
        .first();
    state
}

#[cfg(test)]
mod tests;
