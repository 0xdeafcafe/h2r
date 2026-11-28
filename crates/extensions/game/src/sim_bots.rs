//! `sim.player.*`'s bots (`agent::sim::bots::Bots`) over docs/game/bots.md's
//! `bots::phantom`.
//!
//! Every simulated player is a phantom peer (docs/game/bots.md §5.1): on
//! the books at once, it joins once a multiplayer game this console hosts
//! runs. A local controller seat is a real person's; `sim.player.add` with
//! `kind = "local"` is refused with an error saying so. A phantom's source:
//!
//! - a script is the `Remote` sim fills (`sim.player.add`'s steps, then
//!   `sim.player.pad`), read each tick with `state_at_tick` then
//!   `state_read`, as the agent's input tap reads `input.pad`'s;
//! - a recording is an input log's reads of its first pad, one per frame
//!   ([`bots::pad_source::Recorded`]);
//! - the brain is [`bots::Brain`].

use agent::sim::bots::{Bot, Bots, PadSource};
use agent_proto::sim::{BotBrain, BotMood, BotPersonality, ClankerView, Identity, PlayerKind};
use bots::pad_source::{self, Frame};
use input::backend::remote::Remote;
use input::gamepad::{Gamepad, State};
use input::host::HostPadState;

/// The bots sim calls: the process's phantoms.
pub struct SimBots {
    allow_add: bool,
}

impl SimBots {
    pub fn new(allow_add: bool) -> Self {
        SimBots { allow_add }
    }
}

/// A script source: the pad `Remote` holds now, timed in bot frames.
struct ScriptPad(Remote);

impl pad_source::PadSource for ScriptPad {
    fn pad(&mut self, f: &Frame<'_>) -> HostPadState {
        let g = self
            .0
            .state_at_tick(std::time::Instant::now(), Some(f.frame));
        self.0.state_read();
        HostPadState::from_gamepad(g)
    }
}

/// An input log's pads: the successful reads of the first handle it read.
pub fn recording_pads(text: &str) -> Result<Vec<HostPadState>, String> {
    let log = xapi::tap::InputLog::parse(text)?;
    let Some(first) = log.reads.first().map(|r| r.handle) else {
        return Err("an empty input log".into());
    };
    Ok(log
        .reads
        .iter()
        .filter(|r| r.handle == first && r.status == 0)
        .map(|r| {
            let pad: &[u8; Gamepad::SIZE] = r.packet[State::SIZE - Gamepad::SIZE..]
                .try_into()
                .expect("a packet holds a gamepad");
            HostPadState::from_gamepad(Gamepad::from_bytes(pad))
        })
        .collect())
}

fn source(s: PadSource, id: &bots::phantom::Identity) -> Result<Box<dyn pad_source::PadSource>, String> {
    Ok(match s {
        PadSource::Script(r) => Box::new(ScriptPad(r)),
        PadSource::Recording { name, path } => {
            let text = std::fs::read_to_string(&path)
                .map_err(|e| format!("recording {name} ({}): {e}", path.display()))?;
            let pads = recording_pads(&text).map_err(|e| format!("recording {name}: {e}"))?;
            Box::new(pad_source::Recorded::new(pads))
        }
        PadSource::Brain => Box::new(
            bots::Brain::new(bots::mind::Skill::default(), id.xuid).with_name_profile(&id.gamertag),
        ),
    })
}

/// An xuid as the server gives it: hex, `0x` optional.
fn xuid(s: &str) -> Result<u64, String> {
    let t = s.trim();
    u64::from_str_radix(t.strip_prefix("0x").unwrap_or(t), 16)
        .map_err(|_| format!("xuid {s:?}: hex, as the local embedded server issues it"))
}

fn profile_view(view: bots::BrainView) -> Option<agent_proto::sim::BotProfile> {
    use agent_proto::sim::{BotProfile, BotWeights};
    let profile = view.profile?;
    let w = view.weights?;
    Some(BotProfile {
        archetype: profile.label().to_owned(),
        favorite_weapon: bots::stats::role_name(view.personality.favorite_weapon).to_owned(),
        summary: profile.summary(),
        try_hard: profile.try_hard,
        weights: BotWeights {
            radar_use: w.radar_use,
            noob_combo: w.noob_combo,
            dual_wield: w.dual_wield,
            aggression: w.aggression,
            grenades: w.grenades,
            melee: w.melee,
            weapon_preference: w.weapon_preference,
            range_preference: w.range_preference,
            strafe: w.strafe,
            jump: w.jump,
            reaction_jitter: w.reaction_jitter,
            retreat: w.retreat,
        },
    })
}

fn skill_name(skill: bots::mind::Skill) -> &'static str {
    use bots::mind::Skill;
    match skill {
        Skill::Easy => "easy",
        Skill::Normal => "normal",
        Skill::Heroic => "heroic",
        Skill::Legendary => "legendary",
    }
}

fn brain_view(view: bots::BrainView) -> BotBrain {
    let p = view.personality;
    let favorite_weapon = bots::stats::role_name(p.favorite_weapon);
    let m = view.mood;
    BotBrain {
        skill: skill_name(view.skill).into(),
        profile: profile_view(view),
        personality: BotPersonality {
            aggression: p.aggression,
            caution: p.caution,
            teamwork: p.teamwork,
            objective_focus: p.objective_focus,
            composure: p.composure,
            patience: p.patience,
            showboating: p.showboating,
            weapon_loyalty: p.weapon_loyalty,
            chattiness: p.chattiness,
            favorite_weapon: favorite_weapon.into(),
        },
        mood: BotMood {
            confidence: m.confidence,
            tilt: m.tilt,
            fear: m.fear,
            streak: m.streak,
            revenge_target: m.revenge_target.map(|id| format!("player.{id}")),
        },
    }
}

/// Read a snapshot of the brain-driven phantom seats, including those waiting to join.
pub(crate) fn stats(
    params: serde_json::Value,
    phantoms: &bots::phantom::Phantoms,
) -> Result<serde_json::Value, agent_proto::RpcError> {
    if !(params.is_null() || params.as_object().is_some_and(|p| p.is_empty())) {
        return Err(agent_proto::RpcError::new(
            agent_proto::code::INVALID_PARAMS,
            "bots.stats takes no parameters",
        ));
    }
    let bots: Vec<_> = phantoms
        .list()
        .into_iter()
        .filter_map(|seat| {
            let (combat, brain) = phantoms.combat_state(seat.handle)?;
            Some(serde_json::json!({
                "name": seat.identity.gamertag,
                "handle": seat.handle.0,
                "player": seat.player.map(|id| format!("player.{id}")),
                "skill": skill_name(brain.skill),
                "profile": profile_view(brain),
                "combat": combat,
                "accuracy": crate::bot_accuracy::rows(seat.handle),
            }))
        })
        .collect();
    Ok(serde_json::json!({ "bots": bots, "maintenance": phantoms.maintenance() }))
}

/// A unit's level pair word: the word of its one pair against a fixed
/// body (`live_telemetry::havok_agents`), which a forced crouch leaves at
/// -3 (docs/game/superbounce.md, "The pair word"). `None` when the unit
/// has no single such pair.
pub(crate) fn read_pair_word(m: &dyn guest_layout::GuestMemory, unit: u32) -> Option<u32> {
    live_telemetry::havok_agents::fixed_pair_word(m, unit).ok().flatten()
}

/// Gives the bots the pair-word reader (once; idempotent).
pub(crate) fn install_pair_reader() {
    bots::set_pair_reader(read_pair_word);
}

fn invalid(msg: impl Into<String>) -> agent_proto::RpcError {
    agent_proto::RpcError::new(agent_proto::code::INVALID_PARAMS, msg.into())
}

fn params<T: serde::de::DeserializeOwned>(
    params: serde_json::Value,
    method: &str,
) -> Result<T, agent_proto::RpcError> {
    serde_json::from_value(params).map_err(|e| invalid(format!("{method}: {e}")))
}

/// `superbounce.spots`: the spots the bots know for a map (the measured
/// ones, then the nav worker's scan).
pub(crate) fn spots(raw: serde_json::Value) -> Result<serde_json::Value, agent_proto::RpcError> {
    use agent_proto::superbounce::{SpotRecord, Spots, SpotsParams};
    let p: SpotsParams = if raw.is_null() {
        SpotsParams::default()
    } else {
        params(raw, "superbounce.spots")?
    };
    let kind = match p.kind.as_deref() {
        None => None,
        Some(k @ ("bounce" | "crouch")) => Some(k),
        Some(k) => return Err(invalid(format!("kind {k:?}: bounce or crouch"))),
    };
    let map = p.map.unwrap_or_else(bots::autonav::loaded);
    let spots = bots::spots::listing(&map)
        .into_iter()
        .filter(|l| kind.is_none_or(|k| l.kind == k))
        .map(|l| SpotRecord {
            kind: l.kind.into(),
            source: l.source.into(),
            edge: l.spot.edge,
            stand: l.spot.stand,
            fall: l.spot.fall,
            launch: l.spot.launch,
            drift: l.spot.drift,
            score: l.spot.score,
            need: l.spot.need,
            status: l.spot.status.name().into(),
            out: l.spot.out,
            lip: l.spot.lip.edges().to_vec(),
            shape: l.spot.shape.name().into(),
            learned: l.learned.as_ref().map(learned_view),
        })
        .collect();
    let out = Spots {
        scanned: bots::spots::has_scan(&map),
        map,
        spots,
    };
    Ok(serde_json::to_value(out).expect("spots serialise"))
}

/// A bounce site's knobs as the agent API gives them: the set ones, by
/// the learned file's keys (docs/game/bounce-search.md).
fn knobs_view(k: &superbounce::learned::Knobs) -> agent_proto::bot_bounce::LearnedKnobs {
    agent_proto::bot_bounce::LearnedKnobs {
        how: k.how.map(|h| h.name().into()),
        fx: k.from.map(|f| f[0]),
        fy: k.from.map(|f| f[1]),
        fz: k.from.map(|f| f[2]),
        ax: k.aim.map(|a| a[0]),
        ay: k.aim.map(|a| a[1]),
        yaw: k.yaw,
        back: k.back,
        run: k.run,
        early: k.early,
        gain: k.gain,
        letgo: k.letgo,
        drift: k.drift,
        still: k.still,
    }
}

/// A spot's line of the map's learned file, for `superbounce.spots`.
fn learned_view(e: &superbounce::learned::Entry) -> agent_proto::bot_bounce::SpotLearned {
    agent_proto::bot_bounce::SpotLearned {
        state: e.state.name().into(),
        knobs: knobs_view(&e.knobs),
        tries: e.tries,
        hits: e.hits,
        n: e.n,
        ok: e.ok,
        score: e.score,
        best: e.best,
        why: e.why.clone(),
    }
}

/// The brain-driven seat a request names: by handle, gamertag (any case)
/// or `player.<n>`.
fn seat(
    phantoms: &bots::phantom::Phantoms,
    bot: &agent_proto::bot_bounce::BotRef,
) -> Result<bots::phantom::PhantomInfo, agent_proto::RpcError> {
    use agent_proto::bot_bounce::BotRef;
    phantoms
        .list()
        .into_iter()
        .find(|b| match bot {
            BotRef::Handle(h) => b.handle.0 == *h,
            BotRef::Name(n) => {
                b.identity.gamertag.eq_ignore_ascii_case(n)
                    || b.player.is_some_and(|p| *n == format!("player.{p}"))
            }
        })
        .ok_or_else(|| invalid(format!("no such bot {bot:?}; bots.stats lists them")))
}

/// `bots.bounce_to`: asks a brain-driven bot for a bounce route to a
/// point (`bots::bounce_route`).
pub(crate) fn bounce_to(
    raw: serde_json::Value,
    phantoms: &bots::phantom::Phantoms,
) -> Result<serde_json::Value, agent_proto::RpcError> {
    use agent_proto::bot_bounce::{BounceAccepted, BounceToParams};
    let p: BounceToParams = params(raw, "bots.bounce_to")?;
    if !p.to.iter().all(|v| v.is_finite()) {
        return Err(invalid("to: three finite numbers"));
    }
    let mode = match p.mode.as_deref() {
        None => bots::bounce_route::Mode::Help,
        Some(m) => bots::bounce_route::Mode::parse(m)
            .ok_or_else(|| invalid(format!("mode {m:?}: help or fun")))?,
    };
    let bot = seat(phantoms, &p.bot)?;
    if !phantoms.order_bounce(bot.handle, p.to, mode) {
        return Err(invalid(format!(
            "{} is not brain-driven: only a brain takes a bounce route",
            bot.identity.gamertag
        )));
    }
    Ok(serde_json::to_value(BounceAccepted {
        bot: bot.identity.gamertag,
        handle: bot.handle.0,
        mode: mode.name().into(),
        to: p.to,
    })
    .expect("accepted serialises"))
}

/// `bots.bounce_status`: where a bot's bounce route stands.
pub(crate) fn bounce_status(
    raw: serde_json::Value,
    phantoms: &bots::phantom::Phantoms,
) -> Result<serde_json::Value, agent_proto::RpcError> {
    use agent_proto::bot_bounce::{BounceLog, BounceStatus, BounceStatusParams};
    let p: BounceStatusParams = params(raw, "bots.bounce_status")?;
    let bot = seat(phantoms, &p.bot)?;
    let v = phantoms
        .order_view(bot.handle)
        .ok_or_else(|| invalid(format!("{} is not brain-driven", bot.identity.gamertag)))?;
    Ok(serde_json::to_value(BounceStatus {
        bot: bot.identity.gamertag,
        handle: bot.handle.0,
        state: v.state.into(),
        to: v.to,
        mode: v.mode.map(|m| m.name().into()),
        why: v.why.map(|w| w.name().into()),
        steps: v.steps.iter().map(|s| (*s).into()).collect(),
        step: v.step,
        phase: v.phase.into(),
        tries: v.tries,
        landing: v.landing.map(|t| landing_view(&t, v.site)),
        log: v
            .log
            .iter()
            .map(|l| BounceLog {
                tick: l.tick,
                step: l.step.into(),
                what: l.what.into(),
                at: l.at,
            })
            .collect(),
        primed: v.primed,
        primed_read: v.primed_read,
        error: v.error.map(Into::into),
        knobs: v
            .site
            .filter(|s| !s.knobs.is_default())
            .map(|s| knobs_view(&s.knobs)),
        practice: v.practice.map(|p| agent_proto::bot_bounce::BouncePractice {
            state: p.state.into(),
            site: p.site,
            attempts: p.attempts,
            bounces: p.bounces,
            spots: p
                .spots
                .into_iter()
                .map(|t| agent_proto::bot_bounce::PracticeSpot {
                    stand: t.stand,
                    attempts: t.attempts,
                    bounces: t.bounces,
                    why: t.why.into_iter().map(|(k, n)| (k.to_string(), n)).collect(),
                })
                .collect(),
            train: p.train.map(|t| agent_proto::bot_bounce::PracticeTrain {
                site: t.site,
                stage: t.stage.into(),
                axis: t.axis.map(Into::into),
                slot: t.slot,
                tries: t.tries,
            }),
        }),
    })
    .expect("status serialises"))
}

/// A route's touchdown as `bots.bounce_status` gives it, named against the
/// site's slot.
fn landing_view(
    t: &bots::bounce_route::Touchdown,
    site: Option<bots::spots::Site>,
) -> agent_proto::bot_bounce::BounceLanding {
    let need = site.map_or(0.0, |s| s.need);
    agent_proto::bot_bounce::BounceLanding {
        tick: t.tick,
        at: t.at,
        speed_in: t.speed_in,
        vz_out: t.vz_out,
        miss: t.miss,
        result: t.result(need).into(),
    }
}

impl Bots for SimBots {
    fn add_phantom(
        &mut self,
        identity: &Identity,
        team: Option<i32>,
        src: PadSource,
    ) -> Result<u32, String> {
        if !self.allow_add {
            return Err("light host bots are off; start with --host-bots".into());
        }
        let id = bots::phantom::Identity {
            xuid: xuid(&identity.xuid)?,
            gamertag: identity.gamertag.clone(),
        };
        let team = team
            .map(|t| u8::try_from(t).map_err(|_| format!("team {t}")))
            .transpose()?;
        let src = source(src, &id)?;
        bots::phantom::add(id, team, src)
            .map(|h| h.0)
            .map_err(|e| e.to_string())
    }

    fn phantom_player(&self, handle: u32) -> Option<u32> {
        bots::phantom::player(bots::phantom::PhantomHandle(handle))
    }

    /// Every phantom on the books that has a player: clankers the lobby added
    /// as well as those `sim.player.add` made.
    fn joined_phantoms(&self) -> u32 {
        bots::phantom::list().iter().filter(|p| p.player.is_some()).count() as u32
    }

    fn brain_view(&self, bot: Bot) -> Option<BotBrain> {
        bots::phantom::global()
            .brain_view(bots::phantom::PhantomHandle(bot.handle))
            .map(brain_view)
    }

    fn clankers(&self, guest: Option<&dyn guest_layout::GuestMemory>) -> Vec<ClankerView> {
        bots::phantom::list()
            .into_iter()
            .filter_map(|bot| {
                let brain = brain_view(bots::phantom::global().brain_view(bot.handle)?);
                Some(ClankerView {
                    name: bot.identity.gamertag,
                    kind: PlayerKind::Phantom,
                    player: bot.player.map(|id| format!("player.{id}")),
                    aim: guest
                        .and_then(|m| bots::phantom::global().aim_view(m, bot.handle))
                        .map(|a| agent_proto::sim::PhantomAim {
                            tick: a.tick,
                            stick: a.stick,
                            arrived_stick: a.arrived_stick,
                            trigger: a.trigger,
                            arrived_trigger: a.arrived_trigger,
                            action_deg: a.action_deg,
                            fire_primary: a.fire_primary,
                            assist_strength: a.assist_strength,
                            unit_forward: a.unit_forward,
                            assist_player: a.assist_player.map(|i| format!("player.{i}")),
                        }),
                    brain,
                })
            })
            .collect()
    }

    fn remove(&mut self, bot: Bot) -> Result<(), String> {
        bots::phantom::remove(bots::phantom::PhantomHandle(bot.handle)).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_lists_only_brain_seats_and_rejects_unrecognised_params() {
        use bots::phantom::{Identity, Phantoms};
        use serde_json::json;

        let seats = Phantoms::default();
        assert_eq!(stats(json!(null), &seats).unwrap(), json!({
            "bots": [], "maintenance": {
                "frames": 0, "unchanged_lobby_frames": 0, "total_ns": 0, "max_ns": 0,
                "write_frames": 0, "props_writes": 0, "identity_writes": 0, "handicap_writes": 0,
                "team_moves": 0,
                "props_diff_chunks": 0, "facing_releases": 0
            }
        }));
        seats.add(
            Identity { xuid: 1, gamertag: "Brain".into() },
            None,
            Box::new(bots::Brain::default().with_name_profile("Brain")),
        ).unwrap();
        seats.add(
            Identity { xuid: 2, gamertag: "Tape".into() },
            None,
            Box::new(bots::pad_source::Recorded::new(Vec::new())),
        ).unwrap();
        let view = stats(json!({}), &seats).unwrap();
        assert_eq!(view["bots"].as_array().unwrap().len(), 1);
        let bot = &view["bots"][0];
        assert_eq!(bot["name"], "Brain");
        let p = bots::name_profile::NameProfile::for_name("Brain");
        assert_eq!(bot["profile"]["summary"], p.summary());
        assert_eq!(bot["profile"]["weights"], json!(p.weights(bots::mind::Skill::default())));
        assert!(bot["player"].is_null());
        assert!(bot["combat"]["think_ms"].is_null());
        assert_eq!(bot["combat"]["melee_presses"], 0);
        assert!(bot["accuracy"].is_array());
        assert_eq!(stats(json!({"name": "Brain"}), &seats).unwrap_err().code,
                   agent_proto::code::INVALID_PARAMS);
    }

    #[test]
    fn a_bounce_route_is_asked_of_a_brain_by_handle_name_or_player_and_read_back() {
        use bots::phantom::{Identity, Phantoms};
        use serde_json::json;

        let seats = Phantoms::default();
        let brain = seats
            .add(
                Identity { xuid: 1, gamertag: "Brain".into() },
                None,
                Box::new(bots::Brain::default()),
            )
            .unwrap();
        seats
            .add(
                Identity { xuid: 2, gamertag: "Tape".into() },
                None,
                Box::new(bots::pad_source::Recorded::new(Vec::new())),
            )
            .unwrap();
        let code = |r: Result<serde_json::Value, agent_proto::RpcError>| r.unwrap_err().code;
        let invalid = agent_proto::code::INVALID_PARAMS;

        let ok = bounce_to(json!({"bot": "brain", "to": [1.0, 2.0, 3.0]}), &seats).unwrap();
        assert_eq!(ok["bot"], "Brain");
        assert_eq!(ok["handle"], brain.0);
        assert_eq!(ok["mode"], "help");
        let s = bounce_status(json!({"bot": brain.0}), &seats).unwrap();
        assert_eq!((s["state"].as_str(), s["to"].clone()), (Some("queued"), json!([1.0, 2.0, 3.0])));
        let ok = bounce_to(json!({"bot": brain.0, "to": [1, 2, 3], "mode": "fun"}), &seats).unwrap();
        assert_eq!(ok["mode"], "fun");
        assert_eq!(bounce_status(json!({"bot": "Brain"}), &seats).unwrap()["mode"], "fun");

        // Not brain-driven, unknown, malformed.
        assert_eq!(code(bounce_to(json!({"bot": "Tape", "to": [0, 0, 0]}), &seats)), invalid);
        assert_eq!(code(bounce_to(json!({"bot": "Nobody", "to": [0, 0, 0]}), &seats)), invalid);
        assert_eq!(code(bounce_to(json!({"bot": "Brain"}), &seats)), invalid);
        assert_eq!(code(bounce_to(json!({"bot": "Brain", "to": [0, 0, 0], "mode": "loud"}), &seats)), invalid);
        assert_eq!(code(bounce_to(json!({"bot": "Brain", "to": [0, 0, 0], "via": 1}), &seats)), invalid);
        assert_eq!(code(bounce_status(json!({"bot": "Tape"}), &seats)), invalid);
    }

    #[test]
    fn the_spots_listing_filters_by_kind_and_names_its_map() {
        use serde_json::json;

        let all = spots(json!({"map": "ascension"})).unwrap();
        assert_eq!(all["map"], "ascension");
        assert_eq!(all["spots"][0]["source"], "measured");
        assert_eq!(all["spots"][0]["kind"], "bounce");
        let crouch = spots(json!({"map": "ascension", "kind": "crouch"})).unwrap();
        assert!(crouch["spots"].as_array().unwrap().iter().all(|s| s["kind"] == "crouch"));
        let none = spots(json!({"map": "not_a_map"})).unwrap();
        assert_eq!((none["scanned"].clone(), none["spots"].clone()), (json!(false), json!([])));
        assert_eq!(spots(json!({"kind": "wall"})).unwrap_err().code, agent_proto::code::INVALID_PARAMS);
        assert_eq!(spots(json!({"nope": 1})).unwrap_err().code, agent_proto::code::INVALID_PARAMS);
        assert!(spots(serde_json::Value::Null).is_ok());
    }

    #[test]
    fn a_phantom_goes_on_the_process_wide_books() {
        // Pending until a hosted game runs (docs/game/bots.md §5.1).
        let mut b = SimBots::new(false);
        assert!(
            b.add_phantom(
                &Identity {
                    xuid: "000e0000c0ffee01".into(),
                    gamertag: "Clanker".into(),
                },
                Some(1),
                PadSource::Brain,
            )
            .unwrap_err()
            .contains("--host-bots")
        );
        let mut b = SimBots::new(true);
        let h = b
            .add_phantom(
                &Identity {
                    xuid: "000e0000c0ffee01".into(),
                    gamertag: "Clanker".into(),
                },
                Some(1),
                PadSource::Brain,
            )
            .unwrap();
        assert_eq!(b.phantom_player(h), None, "no player outside a game");
        b.remove(Bot { handle: h }).unwrap();
        assert!(
            b.add_phantom(
                &Identity {
                    xuid: "zz".into(),
                    gamertag: "x".into()
                },
                None,
                PadSource::Brain,
            )
            .unwrap_err()
            .contains("hex")
        );
    }

    #[test]
    fn a_script_supplies_its_pad_to_the_phantom() {
        use pad_source::PadSource as _;

        let guest = guest_layout::testing::FakeGuest::new();
        let remote = Remote::new();
        let pad = Gamepad {
            buttons: input::gamepad::Buttons::START,
            ..Default::default()
        };
        remote.replace([input::backend::remote::Step {
            pad,
            ..Default::default()
        }]);
        let mut source = ScriptPad(remote);
        assert!(
            source
                .pad(&Frame {
                    frame: 0,
                    tick: 100,
                    machine: [7; 6],
                    guest: &guest,
                })
                .buttons
                .contains(input::host::HostButton::Start)
        );
    }
}
