use std::collections::BTreeMap;

use live_proto::game::{
    Frame, FramePlayer, GameEnded, MAX_FRAME_SHOTS, PlayerKilled, PlayerLeft, ScoreChanged,
};

use crate::event::{GameEvent, GameInfo, ObjectState, Output, PlayerState, Snapshot};
use crate::memory::yaw;
use crate::tags::TagNames;

/// Shots a player's held weapon fired since their previous frame.
#[derive(Debug, Clone, Copy, PartialEq)]
struct PendingShots {
    weapon: u32,
    count: u32,
}

struct Match {
    game_id: String,
    tick: u64,
    tick_rate: u64,
    team_play: bool,
    players: Vec<PlayerState>,
    last_emitted_scores: Vec<live_proto::game::PlayerScore>,
    last_emitted_teams: Vec<live_proto::game::TeamScore>,
    last_score_tick: u64,
    ended: bool,
    /// The loaded map's biped and weapon names, when the host found them.
    tags: Option<TagNames>,
    /// By player index; reset when the held weapon changes or the player
    /// has no weapon (dead, or none held).
    shots: BTreeMap<u32, PendingShots>,
}

pub struct Session {
    host: String,
    console: Option<String>,
    /// The Live account signed in on local player slot 0, if any: that
    /// player's roster entries carry its XUID.
    signed_in: Option<live_proto::Xuid>,
    current: Option<Match>,
    output: Vec<Output>,
}

impl Session {
    pub fn new(host: impl Into<String>, console: Option<String>) -> Self {
        Self {
            host: host.into(),
            console,
            signed_in: None,
            current: None,
            output: Vec::new(),
        }
    }

    /// Links the player on local slot 0 to the signed-in Live account.
    pub fn with_signed_in(mut self, xuid: Option<live_proto::Xuid>) -> Self {
        self.signed_in = xuid;
        self
    }

    /// Sets the signed-in account's XUID on the player who plays from local
    /// slot 0. Other players keep whatever the game said (nothing, today).
    fn link(&self, players: &mut [PlayerState]) {
        let Some(xuid) = self.signed_in else { return };
        for p in players.iter_mut().filter(|p| p.local_user == Some(0)) {
            p.player.xuid = Some(xuid);
        }
    }

    pub fn take_output(&mut self) -> Vec<Output> {
        std::mem::take(&mut self.output)
    }

    #[cfg(test)]
    pub fn game_id(&self) -> Option<&str> {
        self.current.as_ref().map(|m| m.game_id.as_str())
    }

    #[cfg(test)]
    pub fn start(&mut self, info: &GameInfo, players: Vec<PlayerState>) {
        self.start_with_teams(info, players, Vec::new());
    }

    pub fn start_with_teams(
        &mut self,
        info: &GameInfo,
        players: Vec<PlayerState>,
        teams: Vec<live_proto::game::TeamScore>,
    ) {
        let mut players = players;
        self.link(&mut players);
        self.end_previous();
        if !info.is_match() {
            return;
        }
        let game_id = live_proto::sign::random_id();
        let game = live_proto::game::NewGame {
            console: self.console.clone(),
            map: info.map.clone(),
            mode: info.mode.clone(),
            variant: info.variant.clone(),
            team_play: info.team_play,
            score_limit: info.score_limit,
            time_limit_s: info.time_limit_s,
            tick: Some(info.tick),
            players: players.iter().map(|p| p.wire_player()).collect(),
        };
        self.output.push(Output::Event(GameEvent::new(
            game_id.clone(),
            live_proto::game::GAME_STARTED,
            live_proto::game::GameStarted {
                game_id: game_id.clone(),
                host: self.host.clone(),
                game,
            },
        )));
        self.current = Some(Match {
            game_id,
            tick: info.tick,
            tick_rate: u64::from(info.tick_rate.max(1)),
            team_play: info.team_play,
            last_emitted_scores: players
                .iter()
                .map(|p| p.stats.wire(p.player.index))
                .collect(),
            last_emitted_teams: teams,
            last_score_tick: info.tick,
            players,
            ended: false,
            tags: None,
            shots: BTreeMap::new(),
        });
    }

    /// Names for the current match's frame `species` and `weapon`; frames
    /// leave both out until (and unless) this is set.
    pub fn set_tag_names(&mut self, tags: TagNames) {
        if let Some(current) = &mut self.current {
            current.tags = Some(tags);
        }
    }

    fn end_previous(&mut self) {
        let Some(current) = self.current.take() else {
            return;
        };
        if current.ended {
            return;
        }
        let game_id = current.game_id.clone();
        Self::event_to(
            &mut self.output,
            game_id,
            live_proto::game::GAME_ENDED,
            GameEnded {
                tick: current.tick,
                reason: "quit".into(),
                winner: None,
                players: current
                    .players
                    .iter()
                    .map(|p| p.stats.wire(p.player.index))
                    .collect(),
            },
        );
    }

    fn event_to(
        output: &mut Vec<Output>,
        game_id: String,
        kind: &str,
        data: impl serde::Serialize,
    ) {
        output.push(Output::Event(GameEvent::new(game_id, kind, data)));
    }

    pub fn sample(&mut self, snapshot: &Snapshot) {
        let mut linked = snapshot.players.clone();
        self.link(&mut linked);
        let Some(current) = &mut self.current else {
            return;
        };
        if current.ended {
            return;
        }
        current.tick = snapshot.tick;
        let before: BTreeMap<u32, u32> = current
            .players
            .iter()
            .map(|p| (p.player.index, p.datum))
            .collect();
        let after: BTreeMap<u32, u32> = snapshot
            .players
            .iter()
            .map(|p| (p.player.index, p.datum))
            .collect();
        let left: Vec<u32> = before
            .iter()
            .filter(|(index, datum)| after.get(index) != Some(datum))
            .map(|(index, _)| *index)
            .collect();
        let joined: Vec<PlayerState> = linked
            .iter()
            .filter(|p| before.get(&p.player.index) != Some(&p.datum))
            .cloned()
            .collect();
        for index in left {
            Self::event_to(
                &mut self.output,
                current.game_id.clone(),
                live_proto::game::PLAYER_LEFT,
                PlayerLeft { index },
            );
        }
        for player in joined {
            Self::event_to(
                &mut self.output,
                current.game_id.clone(),
                live_proto::game::PLAYER_JOINED,
                live_proto::game::PlayerJoined {
                    player: player.wire_player(),
                },
            );
        }
        current.players = linked;

        let scores = snapshot.scores();
        if snapshot.has_stats
            && snapshot.tick >= current.last_score_tick + current.tick_rate
            && (scores != current.last_emitted_scores
                || snapshot.teams != current.last_emitted_teams)
        {
            Self::event_to(
                &mut self.output,
                current.game_id.clone(),
                live_proto::game::SCORE_CHANGED,
                ScoreChanged {
                    tick: snapshot.tick,
                    players: scores.clone(),
                    teams: (!snapshot.teams.is_empty()).then(|| snapshot.teams.clone()),
                },
            );
            current.last_emitted_scores = scores;
            current.last_emitted_teams = snapshot.teams.clone();
            current.last_score_tick = snapshot.tick;
        }

        let mut shots = BTreeMap::new();
        for (player, object) in snapshot.players.iter().zip(&snapshot.objects) {
            let Some(weapon) = object.as_ref().and_then(|o| o.weapon) else {
                continue;
            };
            let index = player.player.index;
            let mut pending = current
                .shots
                .get(&index)
                .copied()
                .filter(|p| p.weapon == weapon.datum)
                .unwrap_or(PendingShots {
                    weapon: weapon.datum,
                    count: 0,
                });
            let fired = snapshot.shots.get(&weapon.datum).copied().unwrap_or(0);
            pending.count = pending.count.saturating_add(fired);
            shots.insert(index, pending);
        }
        current.shots = shots;

        if snapshot.tick.is_multiple_of(2) {
            let game_id = current.game_id.clone();
            let tags = current.tags.as_ref();
            let players = snapshot
                .players
                .iter()
                .zip(&snapshot.objects)
                .map(|(player, object)| {
                    let alive = player.unit != u32::MAX;
                    let index = player.player.index;
                    let shots = current.shots.get_mut(&index).map(|p| {
                        let n = p.count.min(MAX_FRAME_SHOTS);
                        p.count = 0;
                        n
                    });
                    FramePlayer {
                        index,
                        alive,
                        pos: object.as_ref().map_or([0.0; 3], |o| o.pos),
                        yaw: object.as_ref().map_or(0.0, |o| yaw(o.forward)),
                        vehicle: object
                            .as_ref()
                            .and_then(|o| o.vehicle)
                            .map(|v| format!("{v:08x}")),
                        species: object
                            .as_ref()
                            .and_then(|o| o.definition)
                            .zip(tags)
                            .and_then(|(d, t)| t.species(d)),
                        aim: object.as_ref().and_then(|o| o.aim),
                        weapon: object
                            .as_ref()
                            .and_then(|o| o.weapon)
                            .zip(tags)
                            .and_then(|(w, t)| t.weapon(w.definition)),
                        shots: shots.filter(|n| *n > 0),
                    }
                })
                .collect();
            self.output.push(Output::Frame(Frame {
                game_id,
                tick: snapshot.tick,
                players,
            }));
        }
    }

    pub fn kill(
        &mut self,
        tick: u64,
        attacker: Option<u32>,
        victim: u32,
        betrayal: bool,
        object: Option<ObjectState>,
    ) {
        let kind = match attacker {
            None => "world",
            Some(killer) if killer == victim => "suicide",
            Some(_) if betrayal => "betrayal",
            Some(_) => "kill",
        };
        let Some(current) = &self.current else { return };
        if current.ended {
            return;
        }
        Self::event_to(
            &mut self.output,
            current.game_id.clone(),
            live_proto::game::PLAYER_KILLED,
            PlayerKilled {
                tick,
                victim,
                killer: attacker,
                weapon: None,
                kind: kind.into(),
                pos: object.map(|o| o.pos),
            },
        );
    }

    pub fn end(&mut self, snapshot: &Snapshot) {
        let Some(current) = &mut self.current else {
            return;
        };
        if current.ended {
            return;
        }
        current.ended = true;
        current.tick = snapshot.tick;
        let winner = snapshot.winner(current.team_play);
        Self::event_to(
            &mut self.output,
            current.game_id.clone(),
            live_proto::game::GAME_ENDED,
            GameEnded {
                tick: snapshot.tick,
                reason: "finished".into(),
                winner,
                players: snapshot.scores(),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Appearance, ObjectState, PlayerStats};

    fn info() -> GameInfo {
        GameInfo {
            map: "lockout".into(),
            mode: "multiplayer".into(),
            variant: Some("Slayer".into()),
            team_play: false,
            score_limit: Some(25),
            time_limit_s: Some(600),
            tick_rate: 30,
            tick: 0,
        }
    }

    fn player(index: u32, score: i64) -> PlayerState {
        PlayerState {
            datum: 0x80010000 | index,
            player: live_proto::game::Player {
                index,
                name: format!("P{index}"),
                xuid: None,
                team: 0,
                local: Some(index == 0),
                appearance: None,
            },
            appearance: Appearance::default(),
            local_user: (index == 0).then_some(0),
            unit: u32::MAX,
            stats: PlayerStats {
                score,
                ..PlayerStats::ZERO
            },
        }
    }

    fn snapshot(tick: u64, players: Vec<PlayerState>) -> Snapshot {
        Snapshot {
            tick,
            has_stats: true,
            objects: players.iter().map(|_| None).collect(),
            players,
            teams: Vec::new(),
            shots: BTreeMap::new(),
        }
    }

    fn kinds(session: &mut Session) -> Vec<String> {
        session
            .take_output()
            .into_iter()
            .filter_map(|o| match o {
                Output::Event(e) => Some(e.kind),
                Output::Frame(_) => None,
            })
            .collect()
    }

    #[test]
    fn start_roster_score_and_end_events() {
        let mut session = Session::new("test", None);
        session.start(&info(), vec![player(0, 0)]);
        assert_eq!(kinds(&mut session), ["GameStarted"]);
        session.sample(&snapshot(2, vec![player(0, 0), player(1, 0)]));
        assert_eq!(kinds(&mut session), ["PlayerJoined"]);
        session.sample(&snapshot(4, vec![player(0, 0)]));
        assert_eq!(kinds(&mut session), ["PlayerLeft"]);

        session.sample(&snapshot(15, vec![player(0, 1)]));
        assert!(
            kinds(&mut session).is_empty(),
            "score changes wait one second"
        );
        session.sample(&snapshot(30, vec![player(0, 2)]));
        assert_eq!(kinds(&mut session), ["ScoreChanged"]);
        session.sample(&snapshot(31, vec![player(0, 3)]));
        assert!(kinds(&mut session).is_empty());
        session.sample(&snapshot(60, vec![player(0, 4)]));
        assert_eq!(kinds(&mut session), ["ScoreChanged"]);

        session.end(&snapshot(61, vec![player(0, 5)]));
        let outputs = session.take_output();
        let Output::Event(event) = &outputs[0] else {
            panic!()
        };
        assert_eq!(event.kind, "GameEnded");
        assert_eq!(event.data["winner"]["player"], 0);
        assert_eq!(event.data["players"][0]["score"], 5);
        session.end(&snapshot(62, vec![player(0, 6)]));
        assert!(session.take_output().is_empty(), "end is idempotent");
    }

    #[test]
    fn roster_events_carry_appearance() {
        let mut session = Session::new("test", None);
        let mut host = player(0, 0);
        host.appearance = Appearance {
            primary: Some(5),
            secondary: Some(9),
            tertiary: Some(2),
            quaternary: Some(11),
            emblem_foreground: Some(30),
            emblem_background: Some(7),
        };
        session.start(&info(), vec![host, player(1, 0)]);
        let outputs = session.take_output();
        let Output::Event(started) = &outputs[0] else {
            panic!()
        };
        assert_eq!(started.kind, "GameStarted");
        assert_eq!(
            started.data["players"][0]["appearance"],
            serde_json::json!({
                "primary": 5, "secondary": 9, "tertiary": 2, "quaternary": 11,
                "emblem_foreground": 30, "emblem_background": 7
            })
        );
        assert!(
            started.data["players"][1].get("appearance").is_none(),
            "an unknown appearance is omitted entirely"
        );

        // A join carries the new player's appearance; an unchanged roster
        // never re-sends one, and there is no appearance anywhere else.
        let mut joiner = player(2, 0);
        joiner.appearance = Appearance {
            primary: Some(1),
            ..Appearance::default()
        };
        session.sample(&snapshot(2, vec![player(0, 0), player(1, 0), joiner]));
        let events = session
            .take_output()
            .into_iter()
            .filter_map(|o| match o {
                Output::Event(e) => Some(e),
                Output::Frame(_) => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "PlayerJoined");
        assert_eq!(events[0].data["player"]["index"], 2);
        assert_eq!(events[0].data["player"]["appearance"]["primary"], 1);

        session.sample(&snapshot(
            30,
            vec![player(0, 1), player(1, 0), player(2, 0)],
        ));
        let events = session
            .take_output()
            .into_iter()
            .filter_map(|o| match o {
                Output::Event(e) => Some(e),
                Output::Frame(_) => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "ScoreChanged");
        assert!(
            events[0].data.get("appearance").is_none(),
            "appearance never rides score or frame events"
        );
    }

    #[test]
    fn kills_include_kind_victim_position_and_attribution() {
        let mut session = Session::new("test", None);
        session.start(&info(), vec![player(0, 0), player(1, 0)]);
        session.take_output();
        let object = ObjectState {
            pos: [4.0, 5.0, 6.0],
            forward: [1.0, 0.0, 0.0],
            ..ObjectState::default()
        };
        session.kill(9, Some(0), 1, false, Some(object.clone()));
        session.kill(10, Some(0), 0, false, Some(object.clone()));
        session.kill(11, Some(0), 1, true, Some(object.clone()));
        session.kill(12, None, 1, false, Some(object));
        let events = session.take_output();
        let expected = [
            ("kill", 9),
            ("suicide", 10),
            ("betrayal", 11),
            ("world", 12),
        ];
        for (output, (kind, tick)) in events.into_iter().zip(expected) {
            let Output::Event(event) = output else {
                panic!()
            };
            assert_eq!(event.kind, "PlayerKilled");
            assert_eq!(event.data["kind"], kind);
            assert_eq!(event.data["tick"], tick);
            assert_eq!(event.data["pos"], serde_json::json!([4.0, 5.0, 6.0]));
        }
    }

    #[test]
    fn post_end_kills_are_ignored() {
        let mut session = Session::new("test", None);
        session.start(&info(), vec![player(0, 0), player(1, 0)]);
        session.end(&snapshot(30, vec![player(0, 0), player(1, 0)]));
        session.take_output();
        session.kill(31, Some(0), 1, false, None);
        assert!(session.take_output().is_empty());
    }

    #[test]
    fn replacement_and_nonmatch_close_the_previous_game() {
        let mut session = Session::new("test", None);
        session.start(&info(), vec![player(0, 0)]);
        session.take_output();
        session.start(&info(), vec![player(0, 0)]);
        assert_eq!(kinds(&mut session), ["GameEnded", "GameStarted"]);
        let mut menu = info();
        menu.mode = "main_menu".into();
        session.start(&menu, Vec::new());
        assert_eq!(kinds(&mut session), ["GameEnded"]);
        session.sample(&snapshot(2, vec![player(0, 0)]));
        assert!(session.take_output().is_empty());
    }

    #[test]
    fn frames_are_every_second_tick_with_yaw_and_vehicle() {
        let mut session = Session::new("test", None);
        let mut p = player(0, 0);
        p.unit = 0x90010000;
        session.start(&info(), vec![p.clone()]);
        session.take_output();
        let mut state = snapshot(1, vec![p.clone()]);
        state.objects[0] = Some(ObjectState {
            pos: [1.0, 2.0, 3.0],
            forward: [0.0, -1.0, 0.0],
            vehicle: Some(0x90020001),
            ..ObjectState::default()
        });
        session.sample(&state);
        assert!(session.take_output().is_empty());
        state.tick = 2;
        session.sample(&state);
        let outputs = session.take_output();
        let Output::Frame(frame) = &outputs[0] else {
            panic!()
        };
        assert_eq!(frame.players[0].yaw, -std::f64::consts::FRAC_PI_2);
        assert_eq!(frame.players[0].vehicle.as_deref(), Some("90020001"));
        assert!(frame.players[0].alive);
    }

    fn frames(session: &mut Session) -> Vec<Frame> {
        session
            .take_output()
            .into_iter()
            .filter_map(|o| match o {
                Output::Frame(f) => Some(f),
                Output::Event(_) => None,
            })
            .collect()
    }

    fn armed(weapon: u32, definition: u32) -> ObjectState {
        ObjectState {
            pos: [1.0, 2.0, 3.0],
            forward: [1.0, 0.0, 0.0],
            definition: Some(0xe100_0010),
            aim: Some([0.0, 0.6, 0.8]),
            weapon: Some(crate::event::Weapon {
                datum: weapon,
                definition,
            }),
            ..ObjectState::default()
        }
    }

    fn names() -> TagNames {
        TagNames::new(
            0xe000_0001,
            [
                (
                    0xe100_0010,
                    *b"bipd",
                    "objects\\characters\\elite\\elite_mp".to_string(),
                ),
                (
                    0xe100_0020,
                    *b"weap",
                    "objects\\weapons\\rifle\\battle_rifle\\battle_rifle".to_string(),
                ),
            ],
        )
    }

    #[test]
    fn frames_carry_species_aim_and_weapon_from_tag_names() {
        let mut session = Session::new("test", None);
        let mut p = player(0, 0);
        p.unit = 0x90010000;
        session.start(&info(), vec![p.clone(), player(1, 0)]);
        session.take_output();
        let mut state = snapshot(2, vec![p.clone(), player(1, 0)]);
        state.objects[0] = Some(armed(0x9005_0005, 0xe100_0020));

        // Without names, only the aim (read straight from memory) is sent.
        session.sample(&state);
        let frame = &frames(&mut session)[0];
        assert_eq!(frame.players[0].aim, Some([0.0, 0.6, 0.8]));
        assert_eq!(frame.players[0].species, None);
        assert_eq!(frame.players[0].weapon, None);

        session.set_tag_names(names());
        state.tick = 4;
        session.sample(&state);
        let frame = &frames(&mut session)[0];
        assert_eq!(frame.players[0].species.as_deref(), Some("elite"));
        assert_eq!(frame.players[0].weapon.as_deref(), Some("battle_rifle"));
        // A dead player (no unit, no object) has none of them.
        assert_eq!(
            frame.players[1],
            FramePlayer {
                index: 1,
                ..FramePlayer::default()
            }
        );
        assert_eq!(
            serde_json::to_value(&frame.players[0]).unwrap()["species"],
            "elite"
        );

        // An unknown definition sends nothing rather than a guess.
        state.objects[0] = Some(armed(0x9005_0005, 0xe100_0099));
        state.tick = 6;
        session.sample(&state);
        assert_eq!(frames(&mut session)[0].players[0].weapon, None);

        // A new match forgets the old map's names.
        session.start(&info(), vec![p.clone()]);
        session.take_output();
        let mut state = snapshot(8, vec![p]);
        state.objects[0] = Some(armed(0x9005_0005, 0xe100_0020));
        session.sample(&state);
        assert_eq!(frames(&mut session)[0].players[0].weapon, None);
    }

    #[test]
    fn shots_add_up_between_frames_and_reset_on_weapon_change_and_death() {
        let mut session = Session::new("test", None);
        let mut p = player(0, 0);
        p.unit = 0x90010000;
        session.start(&info(), vec![p.clone()]);
        session.take_output();
        let rifle = 0x9005_0005;
        let pistol = 0x9006_0006;
        let sample =
            |session: &mut Session, tick: u64, weapon: Option<u32>, fired: &[(u32, u32)]| {
                let mut state = snapshot(tick, vec![p.clone()]);
                state.objects[0] = weapon.map(|w| armed(w, 0xe100_0020));
                state.shots = fired.iter().copied().collect();
                session.sample(&state);
                frames(session).first().map(|f| f.players[0].shots)
            };

        // Odd ticks add up into the next even tick's frame.
        assert_eq!(sample(&mut session, 1, Some(rifle), &[(rifle, 2)]), None);
        assert_eq!(
            sample(&mut session, 2, Some(rifle), &[(rifle, 1), (pistol, 4)]),
            Some(Some(3)),
            "only the held weapon's shots count"
        );
        assert_eq!(
            sample(&mut session, 4, Some(rifle), &[]),
            Some(None),
            "0 is absent"
        );
        // Switching weapons drops shots counted for the old one.
        assert_eq!(sample(&mut session, 5, Some(rifle), &[(rifle, 5)]), None);
        assert_eq!(
            sample(&mut session, 6, Some(pistol), &[(pistol, 1)]),
            Some(Some(1))
        );
        // Death drops pending shots, and a dead player sends none.
        assert_eq!(sample(&mut session, 7, Some(pistol), &[(pistol, 2)]), None);
        assert_eq!(sample(&mut session, 8, None, &[(pistol, 1)]), Some(None));
        // Counts are capped at the protocol's limit.
        assert_eq!(
            sample(&mut session, 10, Some(rifle), &[(rifle, 500)]),
            Some(Some(MAX_FRAME_SHOTS))
        );
    }

    #[test]
    fn unchanged_initial_teams_do_not_create_a_score_event() {
        let mut session = Session::new("test", None);
        let teams = vec![live_proto::game::TeamScore { team: 0, score: 0 }];
        session.start_with_teams(&info(), vec![player(0, 0)], teams.clone());
        session.take_output();
        let mut state = snapshot(30, vec![player(0, 0)]);
        state.teams = teams;
        session.sample(&state);
        assert!(kinds(&mut session).is_empty());
    }

    #[test]
    fn team_winner_and_ties() {
        let mut state = snapshot(1, vec![player(0, 5), player(1, 2)]);
        state.teams = vec![
            live_proto::game::TeamScore { team: 0, score: 7 },
            live_proto::game::TeamScore { team: 1, score: 2 },
        ];
        assert_eq!(
            state.winner(true),
            Some(live_proto::game::Winner::Team { team: 0 })
        );
        state.teams[1].score = 7;
        assert_eq!(state.winner(true), None);
    }

    #[test]
    fn the_signed_in_account_is_the_player_on_local_slot_zero() {
        let xuid = live_proto::Xuid::parse("0009a1b2c3d4e5f6").unwrap();
        let mut session = Session::new("test", None).with_signed_in(Some(xuid));
        // Slot 0 is the signed-in local player; 1 is remote; 2 is a second
        // local player on another controller (not the signed-in account).
        let mut guest = player(2, 0);
        guest.local_user = Some(1);
        session.start(&info(), vec![player(0, 0), player(1, 0)]);
        let outputs = session.take_output();
        let Output::Event(started) = &outputs[0] else {
            panic!()
        };
        assert_eq!(started.data["players"][0]["xuid"], "0009a1b2c3d4e5f6");
        assert_eq!(
            started.data["players"][0]["name"], "P0",
            "the in-game name stays"
        );
        assert!(started.data["players"][1].get("xuid").is_none());

        session.sample(&snapshot(2, vec![player(0, 0), player(1, 0), guest]));
        let outputs = session.take_output();
        let joined: Vec<_> = outputs
            .iter()
            .filter_map(|o| match o {
                Output::Event(e) if e.kind == "PlayerJoined" => Some(e),
                _ => None,
            })
            .collect();
        assert_eq!(
            joined.len(),
            1,
            "linking does not make the roster look changed"
        );
        assert!(joined[0].data["player"].get("xuid").is_none());

        // A player who rejoins on slot 0 (a new datum) is linked again.
        let mut back = player(0, 0);
        back.datum ^= 0x0001_0000;
        session.sample(&snapshot(4, vec![back]));
        let rejoin = session
            .take_output()
            .into_iter()
            .find_map(|o| match o {
                Output::Event(e) if e.kind == "PlayerJoined" => Some(e),
                _ => None,
            })
            .unwrap();
        assert_eq!(rejoin.data["player"]["xuid"], "0009a1b2c3d4e5f6");
    }

    #[test]
    fn without_a_signed_in_account_players_have_no_xuid() {
        let mut session = Session::new("test", None);
        session.start(&info(), vec![player(0, 0)]);
        let outputs = session.take_output();
        let Output::Event(started) = &outputs[0] else {
            panic!()
        };
        assert!(started.data["players"][0].get("xuid").is_none());
    }
}
