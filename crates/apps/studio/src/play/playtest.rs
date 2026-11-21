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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    Refused(String),
    Launch,
    Pair,
    Resume,
    Reload(Reload),
    Ask { ask: Ask },
}

impl Step {
    pub fn report(&self, place: Option<Place>) -> Result<Value, &str> {
        let name = match self {
            Self::Refused(why) => return Err(why),
            Self::Launch => "launch",
            Self::Pair => "pair",
            Self::Resume => "resume",
            Self::Reload(Reload::Manual) => {
                return Ok(json!({"step": "reload", "why": MANUAL_RELOAD}));
            }
            Self::Reload(_) => "reload",
            Self::Ask { ask } => return Ok(json!({"step": "ask", "ask": ask.name()})),
        };
        let mut result = json!({"step": name});
        if matches!(self, Self::Reload(_) | Self::Launch) && let Some(place) = place {
            result["place"] = place.json();
        }
        Ok(result)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Changes {
    None,
    Send,
    Reload,
}

impl Changes {
    pub(super) fn of(rows: &[Value]) -> Self {
        let mut result = Self::None;
        for row in rows {
            if row["doc"] == "object" {
                // Successful preview and delete notices describe the loaded scenario.
                if row["refused"] == true || row["unknown"] == true || row["needs_reload"] == true {
                    return Self::Reload;
                }
            } else if tag_sync::Effect::is_reload(row) {
                return Self::Reload;
            } else {
                result = Self::Send;
            }
        }
        result
    }
}

pub fn decide(
    link: &LinkState,
    identity_refusal: Option<&str>,
    pending: &[Value],
    own: bool,
    authorized: bool,
    reload: bool,
) -> Step {
    decide_changes(link, identity_refusal, Changes::of(pending), own, authorized, reload)
}

fn decide_changes(
    link: &LinkState,
    identity_refusal: Option<&str>,
    changes: Changes,
    own: bool,
    authorized: bool,
    reload: bool,
) -> Step {
    if matches!(link, LinkState::Blocked { .. }) && let Some(why) = identity_refusal {
        return Step::Refused(why.into());
    }
    let reload = Step::Reload(Reload::route(reload, own));
    match link {
        LinkState::NoGame | LinkState::Lost { .. } => Step::Launch,
        LinkState::Found { .. } | LinkState::Connecting | LinkState::Reconnecting { .. } => {
            Step::Pair
        }
        LinkState::OtherMap { .. } if own || authorized => reload,
        LinkState::OtherMap { .. } => Step::Ask { ask: Ask::Load },
        _ if !own && !authorized && changes != Changes::None => {
            Step::Ask { ask: Ask::Send }
        }
        _ if changes == Changes::Reload => reload,
        _ => Step::Resume,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Document {
    world: u64,
    placements: u64,
    tags: u64,
}

impl Document {
    fn of(app: &App) -> Self {
        Self {
            world: app.session.world_rev(),
            placements: app.session.markers_rev(),
            tags: app.tags.rev(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Offer {
    document: Document,
    pub step: Step,
    pub place: Option<Place>,
    pub prompt: Option<Prompt>,
}

impl Offer {
    pub(crate) fn current_document(&self, app: &App) -> bool {
        self.document == Document::of(app)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Prompt {
    pub id: u64,
    pub ask: Ask,
    pub confirmed: Step,
    document: Document,
    pairing: u64,
    epoch: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Waiting {
    #[default]
    Idle,
    Pick,
    Pair,
    Launch,
    Batch,
    Sent,
    Reload,
    Spawn(Instant),
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
            && !matches!(self.waiting, Waiting::Reload | Waiting::Launch | Waiting::Spawn(_))
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
        self.is_paired()
            && (self.methods.is_none()
                || (self.has_method(agent_proto::hot::method::EDIT_MAP)
                    && self.edits.epoch.is_none()))
    }

    pub(super) fn offer(&self, app: &App, link: &LinkView) -> Offer {
        // A send decision for this map never authorizes switching a foreign game to another.
        let authorized = (self.live_edits || self.playtest_consent)
            && !matches!(link.state, LinkState::OtherMap { .. });
        let identity_refusal = if matches!(link.state, LinkState::Blocked { .. }) {
            self.map_refusal(app)
        } else {
            None
        };
        let decision = |authorized| {
            let step = decide_changes(
                &link.state,
                identity_refusal.as_deref(),
                app.pairing.pending.playtest_changes(),
                self.own_game(),
                authorized,
                self.has_method(agent_proto::sim::method::MAP_RELOAD),
            );
            if matches!(step, Step::Resume | Step::Reload(Reload::Method))
                && link.paused
                && let Some(why) = link.controls.resume
            {
                Step::Refused(why.into())
            } else {
                step
            }
        };
        let found_after_loss = matches!(link.state, LinkState::Lost { .. })
            && !self.finder.running().is_empty();
        let step = if found_after_loss || self.awaiting_identity() {
            Step::Pair
        } else {
            decision(authorized)
        };
        let document = Document::of(app);
        let prompt = match step {
            Step::Ask { ask } => {
                let active = app.pairing.prompt.as_ref();
                let mut prompt = Prompt {
                    id: active.map_or(app.pairing.next_prompt, |p| p.id),
                    ask,
                    confirmed: decision(true),
                    document,
                    pairing: self.pairing_id,
                    epoch: self.edits.epoch,
                };
                if active != Some(&prompt) {
                    prompt.id = app.pairing.next_prompt;
                }
                Some(prompt)
            }
            _ => None,
        };
        let place = if self.spawn_choice(app) == Spawn::Resume {
            self.saved_place(app)
        } else {
            None
        };
        Offer {
            document,
            step,
            place,
            prompt,
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
                (Ask::Load, "load") | (Ask::Send, "send") => step = prompt.confirmed,
                _ => return Err("choose one of the offered playtest actions".into()),
            }
        }
        match step {
            Step::Refused(why) => return Err(why),
            Step::Ask { .. } => app.pairing.show_prompt(offer.prompt),
            Step::Launch => self.launch_copy(app)?,
            Step::Pair => {
                let games = self.finder.running();
                if let Some(path) = auto::choose(
                    &games,
                    |game| pair::map_match(app, game.map()) == MapMatch::Same,
                    self.child_endpoint.as_ref(),
                    |_| false,
                ) {
                    self.pair(app, Endpoint::Unix(path));
                    self.waiting = Waiting::Pair;
                } else {
                    self.waiting = Waiting::Pick;
                }
            }
            Step::Resume => {
                if self.edits.sent.is_some() {
                    self.waiting = Waiting::Batch;
                } else if app.pairing.pending.playtest_changes() != Changes::None {
                    if let Some(why) = self.edit_refusal(app) {
                        return Err(why);
                    }
                    self.retry_refused_fields();
                    self.refresh_pending(app, true);
                    if self.edits.sent.is_none() {
                        return Err("resolve the pending edits before playing".into());
                    }
                    self.waiting = Waiting::Sent;
                } else {
                    self.finish_playtest(app);
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
                    self.map_refusal(app).or_else(|| self.session_refusal(app))
                };
                if let Some(why) = refusal {
                    return Err(why);
                }
                self.reload_for_play(app, route)?;
            }
        }
        Ok(())
    }

    pub(super) fn finish_playtest(&mut self, app: &mut App) {
        if self.paused {
            if let Some(why) = self.controls().resume {
                self.waiting = Waiting::Idle;
                app.error(why);
                return;
            }
            self.resume();
        } else if self.controller {
            // The pause can expire while this connection still owns the claim.
            self.release_control();
        }
        if self.has_method(agent_proto::editor::method::FOCUS) {
            self.pause_on_return = false;
            self.call(agent_proto::editor::method::FOCUS, Value::Null, Purpose::Focus);
        }
        self.waiting = Waiting::Idle;
    }

    pub(super) fn continue_playtest(&mut self, app: &mut App) {
        let decide_again = match self.waiting {
            Waiting::Pair => {
                self.is_paired()
                    && self.methods.is_some()
                    && (!self.has_method(agent_proto::hot::method::EDIT_MAP)
                        || (self.edits.epoch.is_some()
                            && (!self.has_method(agent_proto::hot::method::EDIT_CHANGES)
                                || self.edits.journal_ready)))
            }
            Waiting::Batch => self.edits.sent.is_none(),
            Waiting::Sent if self.edits.sent.is_none() => {
                self.waiting = Waiting::Idle;
                app.error("the game changed before the playtest batch was acknowledged");
                false
            }
            Waiting::Launch => {
                if self.is_paired() && self.methods.is_some() && self.status.is_some()
                    && self.pending_spawn.is_none()
                {
                    self.finish_playtest(app);
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
        let should_pause = focused && app.pairing.pause_while_editing && self.is_paired();
        self.pause_on_return = should_pause && self.waiting != Waiting::Idle;
        if should_pause && self.waiting == Waiting::Idle && !self.paused {
            self.pause();
        }
    }
}
