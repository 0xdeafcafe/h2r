//! The subject in Studio's own game: the object that stands for the tag
//! being edited, and what is done after a write the game would otherwise
//! keep showing the old way (docs/editor/live-editors.md, "The subject").
//!
//! - The subject (`App::subject`) is made by `sim.spawn` where
//!   `app::subject::place` says (the viewport camera's focus in a level,
//!   the stage's subject pad with none), and replaced by
//!   `world.object.delete` and a new spawn when the subject changes.
//! - After a Field write to the subject that the game took
//!   ([`super::tag_sync`]'s fan-out): a field read at creation
//!   ([`Effect::NextSpawn`]) queues one `world.object.respawn` of the
//!   subject's object and of nothing else; a field read at use
//!   ([`Effect::NextUse`]) queues one retrigger (a sound is played again);
//!   a field no definition says when it is read ([`Effect::NotKnown`]) is
//!   marked unread. A queued call goes once the write's batch is out.
//! - Only Studio's own game is touched: an attached game keeps its Pending
//!   rows.
//!
//! The state is published as `pairing.subject` (`tag.subject`).
use std::collections::BTreeSet;
use std::sync::OnceLock;

use agent_proto::sim::method as sim;
use agent_proto::{Kind, ObjectId, Phase};
use map::TagId;
use serde_json::{Value, json};

use super::link::Purpose;
use super::playtest::Waiting;
use super::tag_sync::{Effect, change_effects};
use super::{LIFT, Play};
use crate::app::App;
use crate::app::live::Change;
use crate::app::subject::{self, Place, STAGE_MAP, Scene, Show};
use agent_proto::discover::MapMatch;

/// Why a sound's retrigger and an effect's, say, need a method the game
/// lacks.
const NO_RETRIGGER: &str = "the game has no method to start this kind of subject again";

/// A call whose answer the subject waits for.
#[derive(Clone, Copy, Debug, PartialEq)]
enum What {
    Spawn { tag: TagId, kind: Kind },
    Delete,
    Respawn,
    Play,
}

impl What {
    fn id(self) -> &'static str {
        match self {
            Self::Spawn { .. } => "spawn",
            Self::Delete => "delete",
            Self::Respawn => "respawn",
            Self::Play => "play",
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Flight {
    ticket: u64,
    what: What,
}

/// Follow-ups waiting for the writes' batch to be out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Owed {
    pub(super) respawn: bool,
    pub(super) retrigger: bool,
}

/// How many of each call the subject has made in this game.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Counts {
    spawns: u32,
    deletes: u32,
    respawns: u32,
    retriggers: u32,
}

#[derive(Debug, Default)]
pub(super) struct State {
    /// The pairing and map epoch it is for; a new one forgets the object.
    identity: Option<(u64, Option<u64>)>,
    /// The tag being edited.
    current: Option<TagId>,
    /// The object tag the standing object was made from.
    stands_for: Option<TagId>,
    object: Option<ObjectId>,
    /// The object tag last asked for, so a refused spawn is not retried
    /// every frame.
    tried: Option<TagId>,
    flight: Option<Flight>,
    next_ticket: u64,
    pub(super) owed: Owed,
    /// Fields written that no definition says when the game reads.
    unread: BTreeSet<String>,
    /// Why the subject has no object, from the registry or the tags.
    refusal: Option<String>,
    /// The last call the game refused.
    failure: Option<String>,
    counts: Counts,
}

static STAGE: OnceLock<Option<Scene>> = OnceLock::new();

impl Play {
    /// The scene the subject stands in: the stage's pad when the stage is
    /// the own game's map, the viewport's focus when the open level is,
    /// else nowhere.
    fn subject_scene(&self, app: &App) -> Scene {
        let Some(map) = self.map() else {
            return Scene::Nowhere;
        };
        if map.eq_ignore_ascii_case(STAGE_MAP) {
            return STAGE
                .get_or_init(|| Scene::stage().ok())
                .clone()
                .unwrap_or(Scene::Nowhere);
        }
        if app.session.scenario().is_some() && self.map_match(app) == MapMatch::Same {
            return Scene::Level {
                focus: app.session.camera().target,
            };
        }
        Scene::Nowhere
    }

    fn subject_ready(&self, app: &App) -> bool {
        self.waiting == Waiting::Idle
            && self.status.as_ref().is_some_and(|s| s.at.phase == Phase::Gameplay)
            && self.state_free()
            && !app.live.owed.send
    }

    fn state_free(&self) -> bool {
        self.subject.flight.is_none() && !self.edits.interrupted_write()
    }

    /// After the writes the game took this frame: queues what the subject
    /// needs to show them. Only writes to the subject's own tag count.
    pub(super) fn subject_follow(&mut self, app: &App, written: &[Change]) {
        let Some(tag) = app.subject() else { return };
        if tag != self.subject.current.unwrap_or(tag) {
            // The subject changed this frame: `subject_update` resets.
            return;
        }
        for change in written.iter().filter(|c| c.tag == Some(tag)) {
            for (effect, name) in change_effects(&app.tags, change) {
                match effect {
                    Effect::NextSpawn => self.subject.owed.respawn = true,
                    Effect::NextUse => self.subject.owed.retrigger = true,
                    Effect::NotKnown => {
                        self.subject.unread.insert(name);
                    }
                    // Now needs nothing; Reload is the build and reload of
                    // docs/editor/game-view.md (GV6).
                    Effect::Now | Effect::Reload => {}
                }
            }
        }
    }

    fn subject_send(&mut self, what: What, method: &'static str, params: Value) -> bool {
        if !self.has_method(method) {
            self.subject.failure = Some(format!("the game has no {method}"));
            return false;
        }
        self.subject.next_ticket += 1;
        let ticket = self.subject.next_ticket;
        self.subject.flight = Some(Flight { ticket, what });
        self.call(method, params, Purpose::Subject(ticket));
        true
    }

    /// Once a frame, after the fan-out: stands the subject in the own
    /// game (and replaces or removes it when the subject changes), makes
    /// the queued respawn or retrigger, and publishes the state.
    pub(super) fn subject_update(&mut self, app: &mut App) {
        let opened = app.pairing.opened.take();
        if !self.own_game() || self.link.is_none() {
            self.subject = State::default();
            app.pairing.scene = Scene::Nowhere;
            app.pairing.subject = Value::Null;
            return;
        }
        let identity = (self.pairing_id, self.edits.epoch);
        if self.subject.identity != Some(identity) {
            self.subject = State {
                identity: Some(identity),
                ..State::default()
            };
        }
        app.pairing.scene = self.subject_scene(app);
        let tag = opened.or_else(|| app.subject());
        if tag != self.subject.current {
            self.subject.current = tag;
            self.subject.tried = None;
            self.subject.owed = Owed::default();
            self.subject.unread.clear();
            self.subject.failure = None;
        }
        let place: Option<Place> = tag.map(|t| app.place_of(t));
        self.subject.refusal = place.as_ref().and_then(|p| p.note.clone());
        let want: Option<(TagId, Kind)> = match (&place, tag) {
            (Some(p), Some(t)) if p.show == Show::Object && p.at.is_some() => {
                match subject::spawn_tag(&app.tags, t) {
                    Ok(found) => Some(found),
                    Err(why) => {
                        self.subject.refusal = Some(why);
                        None
                    }
                }
            }
            _ => None,
        };
        if self.subject_ready(app) {
            self.subject_step(app, tag, place.as_ref(), want, opened.is_some());
        }
        app.pairing.subject = self.subject_json(app, tag, place.as_ref());
    }

    fn subject_step(
        &mut self,
        app: &App,
        tag: Option<TagId>,
        place: Option<&Place>,
        want: Option<(TagId, Kind)>,
        opened: bool,
    ) {
        let object = self.subject.object;
        if let Some(id) = object
            && want.map(|w| w.0) != self.subject.stands_for
        {
            // What stands is not what the subject wants: it goes first.
            self.subject_send(What::Delete, sim::OBJECT_DELETE, json!({"id": id.to_string()}));
            return;
        }
        if object.is_none() {
            self.subject.owed.respawn = false;
            if let (Some((spawn, kind)), Some(place)) = (want, place)
                && (self.subject.tried != Some(spawn) || opened)
                && let Some(at) = place.at
            {
                self.subject.tried = Some(spawn);
                let pos = if place.scene == "level" {
                    super::ground(app, at)
                } else {
                    [at[0], at[1], at[2] + LIFT]
                };
                let mut params = json!({"tag": format!("{:#010x}", spawn.0), "pos": pos});
                if let Some(yaw) = place.yaw {
                    params["yaw"] = json!(yaw);
                }
                self.subject_send(What::Spawn { tag: spawn, kind }, sim::SPAWN, params);
                return;
            }
        } else if self.subject.owed.respawn
            && let Some(id) = object
        {
            self.subject.owed.respawn = false;
            self.subject_send(What::Respawn, sim::OBJECT_RESPAWN, json!({"id": id.to_string()}));
            return;
        }
        let plays = place.is_some_and(|p| p.show == Show::Play);
        if plays && (self.subject.owed.retrigger || opened)
            && let Some(sound) = tag
        {
            self.subject.owed.retrigger = false;
            self.subject_send(What::Play, sim::AUDIO_PLAY, json!({"sound": format!("{:#010x}", sound.0)}));
        } else if self.subject.owed.retrigger {
            self.subject.owed.retrigger = false;
            self.subject.failure = Some(NO_RETRIGGER.to_owned());
        }
    }

    /// The answer to one of the subject's calls.
    pub(super) fn subject_answer(
        &mut self,
        app: &mut App,
        ticket: u64,
        result: Result<Value, agent_proto::RpcError>,
    ) {
        let Some(flight) = self.subject.flight.filter(|f| f.ticket == ticket) else {
            return;
        };
        self.subject.flight = None;
        match (flight.what, result) {
            (What::Spawn { tag, kind }, Ok(reply)) => {
                // `sim.spawn` answers the datum handle as eight hex digits.
                self.subject.object = reply["object"]
                    .as_str()
                    .and_then(|hex| ObjectId::parse(&format!("{}.{hex}", kind.as_str())));
                self.subject.stands_for = self.subject.object.map(|_| tag);
                self.subject.counts.spawns += 1;
                self.subject.failure = None;
            }
            (What::Respawn, Ok(reply)) => {
                self.subject.object = reply["object"].as_str().and_then(ObjectId::parse);
                self.subject.counts.respawns += 1;
                self.subject.failure = None;
                if self.subject.object.is_none() {
                    self.subject.stands_for = None;
                }
            }
            (What::Delete, result) => {
                // Gone or already gone: nothing stands either way.
                self.subject.object = None;
                self.subject.stands_for = None;
                self.subject.counts.deletes += 1;
                if let Err(e) = result {
                    self.subject.failure = Some(format!("{}: {e}", flight.what.id()));
                }
            }
            (What::Play, Ok(_)) => self.subject.counts.retriggers += 1,
            (what, Err(e)) => {
                let why = format!("{}: {e}", what.id());
                self.feedback.problem(app, format!("the subject: {why}"));
                self.subject.failure = Some(why);
            }
        }
    }

    fn subject_json(&self, app: &App, tag: Option<TagId>, place: Option<&Place>) -> Value {
        let s = &self.subject;
        json!({
            "tag": tag.map(|t| format!("{:#010x}", t.0)),
            "name": tag.and_then(|t| app.tags.tag(t)).map(|t| t.name.clone()),
            "place": place.map(Place::json),
            "object": s.object.map(|o| o.to_string()),
            "stands_for": s.stands_for.map(|t| format!("{:#010x}", t.0)),
            "owed": {"respawn": s.owed.respawn, "retrigger": s.owed.retrigger},
            "unread": s.unread.iter().collect::<Vec<_>>(),
            "refusal": s.refusal,
            "failure": s.failure,
            "busy": s.flight.map(|f| f.what.id()),
            "calls": {
                "spawn": s.counts.spawns,
                "delete": s.counts.deletes,
                "respawn": s.counts.respawns,
                "retrigger": s.counts.retriggers,
            },
        })
    }
}

// The tests drive `play::testing::fake`, a Unix-socket game.
#[cfg(all(test, unix))]
mod tests;
