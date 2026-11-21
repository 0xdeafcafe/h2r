//! The F5 decision and the work that must wait for replies from the game.

use super::*;

pub(super) const MANUAL_RELOAD: &str =
    "Save the map and load it in the game; this game lacks map.reload.";

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Place {
    pub pos: Vec3,
    pub yaw: f64,
    pub pitch: f64,
}

impl Place {
    fn player(app: &App) -> Option<Self> {
        let sample = app.pairing.shown.as_ref()?;
        if !app.pairing.matches_sample(app, sample) {
            return None;
        }
        let (player, object) = app.pairing.player()?;
        player.local_user?;
        let pos = if object.parent.is_some() {
            crate::viewport::mirror::WorldPoses::new(sample).position(object.id)?
        } else {
            object.position
        };
        if !pos.iter().all(|x| x.is_finite()) {
            return None;
        }
        let (yaw, pitch) = agent_proto::sim::yaw_pitch(player.aim.map(f64::from))?;
        Some(Self {
            pos,
            yaw,
            pitch: pitch.clamp(-89.0, 89.0),
        })
    }

    pub(crate) fn json(self) -> Value {
        json!({"pos": self.pos, "yaw": self.yaw, "pitch": self.pitch})
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ask {
    Send,
    Load,
}

impl Ask {
    pub fn name(self) -> &'static str {
        match self {
            Self::Send => "send",
            Self::Load => "load",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reload {
    Method,
    Relaunch,
    Manual,
}

impl Reload {
    pub(super) fn route(method: bool, own: bool) -> Self {
        if method {
            Self::Method
        } else if own {
            Self::Relaunch
        } else {
            Self::Manual
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Launch,
    Pair,
    Resume,
    Reload(Reload),
    Ask { ask: Ask },
}

impl Step {
    pub fn report(self, place: Option<Place>) -> Value {
        let name = match self {
            Self::Launch => "launch",
            Self::Pair => "pair",
            Self::Resume => "resume",
            Self::Reload(Reload::Manual) => return json!({"step": "reload", "why": MANUAL_RELOAD}),
            Self::Reload(_) => "reload",
            Self::Ask { ask } => return json!({"step": "ask", "ask": ask.name()}),
        };
        let mut result = json!({"step": name});
        if matches!(self, Self::Reload(_) | Self::Launch) && let Some(place) = place {
            result["place"] = place.json();
        }
        result
    }
}

pub fn decide(
    link: &LinkState,
    pending: &[Value],
    own: bool,
    authorized: bool,
    reload: bool,
) -> Step {
    let reload = Step::Reload(Reload::route(reload, own));
    match link {
        LinkState::NoGame | LinkState::Lost { .. } => Step::Launch,
        LinkState::Found { .. } | LinkState::Connecting | LinkState::Reconnecting { .. } => {
            Step::Pair
        }
        LinkState::OtherMap { .. } if own || authorized => reload,
        LinkState::OtherMap { .. } => Step::Ask { ask: Ask::Load },
        _ if !pending.is_empty() && !own && !authorized => Step::Ask { ask: Ask::Send },
        _ if pending.iter().any(|row| row["class"] == tag_sync::RELOAD) => reload,
        _ => Step::Resume,
    }
}

#[derive(Clone, Debug)]
pub struct Offer {
    pub step: Step,
    pub confirmed: Step,
    pub place: Option<Place>,
    pub prompt: Option<Prompt>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Prompt {
    pub ask: Ask,
    pairing: u64,
    epoch: Option<u64>,
    world: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Waiting {
    #[default]
    Idle,
    Pick,
    Pair,
    Launch,
    Batch,
    Reload,
}

#[derive(Clone, Copy)]
pub(super) enum Destination {
    Position(Vec3),
    Place(Place),
}

impl Play {
    pub fn picking_game(&self) -> bool {
        self.waiting == Waiting::Pick
    }

    pub fn cancel_picker(&mut self) {
        if self.waiting == Waiting::Pick {
            self.waiting = Waiting::Idle;
        }
    }

    pub(super) fn own_game(&self) -> bool {
        self.child_endpoint.is_some() && self.child_endpoint.as_ref() == self.endpoint()
    }

    pub(super) fn remember_place(&mut self, app: &App) {
        if self.status.as_ref().is_some_and(|s| s.at.phase == Phase::Gameplay)
            && !matches!(self.waiting, Waiting::Reload | Waiting::Launch)
            && let (Some(map), Some(sum)) = (app.loaded_map(), app.session.source_checksum())
            && let Some(place) = Place::player(app)
        {
            if let Some(saved) = self.places.get_mut(map) {
                *saved = (sum, place);
            } else {
                self.places.insert(map.into(), (sum, place));
            }
        }
    }

    pub(super) fn saved_place(&self, app: &App) -> Option<Place> {
        let &(sum, place) = self.places.get(app.loaded_map()?)?;
        (Some(sum) == app.session.source_checksum()).then_some(place)
    }

    pub fn spawn_choice(&self, app: &App) -> Spawn {
        app.pairing.spawn.unwrap_or_else(|| {
            if self.saved_place(app).is_some() {
                Spawn::Resume
            } else {
                Spawn::Level
            }
        })
    }

    pub(super) fn destination(&self, app: &App) -> Option<Destination> {
        match self.spawn_choice(app) {
            Spawn::Resume => self.saved_place(app).map(Destination::Place),
            _ => self.spawn_point(app).map(Destination::Position),
        }
    }

    fn awaiting_identity(&self) -> bool {
        self.is_paired() && (!self.capabilities_ready || self.edits.epoch.is_none())
    }

    pub(super) fn offer(&self, app: &App, link: &LinkView) -> Offer {
        // A send decision for this map never authorizes switching a foreign game to another.
        let authorized = (self.live_edits || self.playtest_consent)
            && !matches!(link.state, LinkState::OtherMap { .. });
        let decision = |authorized| {
            decide(
                &link.state,
                &app.pairing.pending,
                self.own_game(),
                authorized,
                self.edits.reload,
            )
        };
        let found_after_loss = matches!(link.state, LinkState::Lost { .. })
            && !self.finder.running().is_empty();
        let step = if found_after_loss || self.awaiting_identity() {
            Step::Pair
        } else {
            decision(authorized)
        };
        let (confirmed, prompt) = match step {
            Step::Ask { ask } => (decision(true), Some(self.prompt(app, ask))),
            _ => (step, None),
        };
        let place = if self.spawn_choice(app) == Spawn::Resume {
            self.saved_place(app)
        } else {
            None
        };
        Offer {
            step,
            confirmed,
            place,
            prompt,
        }
    }

    fn prompt(&self, app: &App, ask: Ask) -> Prompt {
        Prompt {
            ask,
            pairing: self.pairing_id,
            epoch: self.edits.epoch,
            world: app.session.world_rev(),
        }
    }

    pub(super) fn playtest(
        &mut self,
        app: &mut App,
        answer: Option<(Prompt, String)>,
    ) -> Result<(), String> {
        if !matches!(self.waiting, Waiting::Idle | Waiting::Pick) {
            return Err("a playtest is already in progress".into());
        }
        self.refresh_pending(app, false);
        self.remember_place(app);
        self.play_world = app.session.world_rev();
        let offer = self.offer(app, &self.link_view(app));
        if answer.is_none() && self.awaiting_identity() {
            self.waiting = Waiting::Pair;
            return Ok(());
        }
        let mut step = offer.step;
        if let Some((prompt, choice)) = answer {
            if offer.prompt.as_ref() != Some(&prompt)
                || app.pairing.prompt.as_ref() != Some(&prompt)
                || step != (Step::Ask { ask: prompt.ask })
            {
                return Err("the game or pending edits changed; press Play again".into());
            }
            app.pairing.prompt = None;
            match (prompt.ask, choice.as_str()) {
                (_, "cancel") => return Ok(()),
                (Ask::Load, "open") => {
                    self.open_game_map(app);
                    return Ok(());
                }
                (Ask::Load, "load") | (Ask::Send, "send") => step = offer.confirmed,
                _ => return Err("choose one of the offered playtest actions".into()),
            }
        }
        match step {
            Step::Ask { .. } => app.pairing.prompt = offer.prompt,
            Step::Launch => self.launch_copy(app)?,
            Step::Pair => {
                let games = self.finder.running();
                if let Some(path) = auto::choose(
                    &games,
                    app.loaded_map(),
                    self.child_endpoint.as_ref(),
                    &HashSet::new(),
                ) {
                    self.pair(app, Endpoint::Unix(path));
                    self.waiting = Waiting::Pair;
                } else {
                    self.waiting = Waiting::Pick;
                }
            }
            Step::Resume => {
                if !app.pairing.pending.is_empty() {
                    if self.edits.sent.is_none() {
                        self.apply_edits(app, Apply::Send)?;
                        self.push_edits(app);
                    }
                    if self.edits.sent.is_none() {
                        return Err("resolve the pending edits before playing".into());
                    }
                }
                if self.edits.sent.is_some() {
                    self.waiting = Waiting::Batch;
                } else {
                    self.finish_playtest();
                }
            }
            Step::Reload(Reload::Manual) => return Err(MANUAL_RELOAD.into()),
            Step::Reload(route) => {
                if self.edits.sent.is_some() {
                    // Finish the outstanding conditional write before replacing the map.
                    self.waiting = Waiting::Batch;
                    return Ok(());
                }
                let refusal = if self.map_match(app) == MapMatch::Different {
                    self.session_refusal(app)
                } else {
                    self.edit_refusal(app)
                };
                if let Some(why) = refusal {
                    return Err(why);
                }
                self.reload_for_play(app, route)?;
            }
        }
        Ok(())
    }

    fn finish_playtest(&mut self) {
        if self.paused {
            self.ensure_claim();
            self.resume();
        }
        if self.focus_available {
            self.call(agent_proto::editor::method::FOCUS, Value::Null, Purpose::Focus);
        }
        self.waiting = Waiting::Idle;
    }

    pub(super) fn continue_playtest(&mut self, app: &mut App) {
        let decide_again = match self.waiting {
            Waiting::Pair => self.is_paired() && self.capabilities_ready && self.edits.journal_ready,
            Waiting::Batch => self.edits.sent.is_none(),
            Waiting::Launch => {
                if self.is_paired() && self.capabilities_ready && self.status.is_some()
                    && self.pending_spawn.is_none()
                {
                    self.finish_playtest();
                }
                false
            }
            _ => false,
        };
        if decide_again {
            self.waiting = Waiting::Idle;
            if let Err(why) = self.playtest(app, None) {
                app.error(why);
            }
        }
    }

    /// The native window supplies focus changes. Tests can supply the same signal.
    pub fn window_focus(&mut self, app: &App, focused: bool) {
        if focused && app.pairing.pause_while_editing && self.is_paired() && !self.paused {
            self.pause();
        }
    }
}
