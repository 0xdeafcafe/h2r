//! Read-only trace of the title's matchmaking state (docs/runtime/agent-api.md).
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};

use agent_proto::{RpcError, code, method};
use bots::{lobby, phantom::session::at};
use guest_layout::{memory::GuestMemory, points};
use serde::Serialize;
use serde_json::json;

const CAPACITY: usize = 128;
const MAX_MEMBERS: u32 = 16;
const MATCHMAKING_RESULT: u32 = 0x0052_7500 + 0x978;
const ARBITRATION_STATE: u32 = 0x0051_0518;
const ARBITRATION_RESULT: u32 = 0x0051_0514;
const KEY_ID: u32 = 0x1c;
const HAS_KEY: u32 = 0x24;
const HOPPER: u32 = 0x49c8;
const HAS_ARBITRATION_ROUND: u32 = 0x49e8;
const HAS_ROSTER: u32 = 0x49fd;
const ROSTER_PARTIES: u32 = 0x4a00;
const ROSTER_PLAYERS: u32 = ROSTER_PARTIES + 0x184;
const ROSTER_IDS: u32 = ROSTER_PARTIES + 0x188;
const PLAYER_ID_SIZE: usize = 12;
const PEER_ARBITRATION: u32 = 0xec;

#[derive(Debug, PartialEq, Eq, Serialize)]
struct State {
    lifecycle: u32,
    result: Option<u32>,
    arbitration_state: Option<u32>,
    arbitration_result: Option<u32>,
    squad: Option<Session>,
    target: Option<Session>,
}

impl State {
    fn read(m: &dyn GuestMemory, lifecycle: u32) -> Self {
        Self {
            lifecycle,
            result: m.u32(MATCHMAKING_RESULT).ok(),
            arbitration_state: m.u32(ARBITRATION_STATE).ok(),
            arbitration_result: m.u32(ARBITRATION_RESULT).ok(),
            squad: Session::read(m, lobby::SESSION),
            target: Session::read(m, lobby::TARGET_SESSION),
        }
    }
}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct Session {
    id: Option<String>,
    state: u32,
    class: Option<u32>,
    hopper: Option<u32>,
    host_peer: Option<u32>,
    local_peer: Option<u32>,
    players: Option<u32>,
    player_limit: Option<u32>,
    roster_players: Option<u32>,
    roster_parties: Option<u32>,
    roster_ids_present: Option<u32>,
    arbitration_round: Option<bool>,
    countdown: Option<i32>,
    peers: Vec<Peer>,
}

impl Session {
    fn read(m: &dyn GuestMemory, pointer: u32) -> Option<Self> {
        let s = m.u32(pointer).ok()?;
        if s == 0 {
            return None;
        }
        let state = m.u32(s.checked_add(at::STATE)?).ok()?;
        if state == 0 {
            return None;
        }
        let read = |offset| m.u32(s + offset).ok();
        let hopper = read(HOPPER).filter(|h| *h < 16);
        let mut key_id = [0; 8];
        let id = (m.u8(s + HAS_KEY).is_ok_and(|v| v != 0) && m.read(s + KEY_ID, &mut key_id).is_ok())
            .then(|| live_proto::sign::hex(&key_id));
        let has_roster = m.u8(s + HAS_ROSTER).is_ok_and(|v| v != 0);
        let roster_players = has_roster.then(|| read(ROSTER_PLAYERS)).flatten();
        let peer_count = m.i32(s + at::PEER_COUNT).unwrap_or(0).clamp(0, MAX_MEMBERS as i32);
        let peers = (0..peer_count as u32)
            .map(|i| {
                let p = s + at::PEERS + i * at::PEER_SIZE;
                let mut mac = [0; 6];
                let phantom = m
                    .read(p + at::PEER_MAC, &mut mac)
                    .ok()
                    .map(|()| bots::phantom::handle_of(mac).is_some());
                Peer {
                    phantom,
                    map: m.u32(p + at::PEER_MAP_STATUS).ok(),
                    hopper: hopper.and_then(|h| m.u32(p + at::PEER_HOPPERS + h * 4).ok()),
                    arbitration: m.u32(p + PEER_ARBITRATION).ok(),
                }
            })
            .collect();
        Some(Self {
            id,
            state,
            class: read(at::CLASS),
            hopper,
            host_peer: read(at::HOST_PEER),
            local_peer: read(at::LOCAL_PEER),
            players: read(at::PLAYER_COUNT),
            player_limit: read(at::MAX_PLAYERS),
            roster_players,
            roster_parties: has_roster.then(|| read(ROSTER_PARTIES)).flatten(),
            roster_ids_present: roster_players.and_then(|n| roster_ids_present(m, s, n)),
            arbitration_round: m.u8(s + HAS_ARBITRATION_ROUND).ok().map(|v| v != 0),
            countdown: m
                .u8(s + lobby::COUNTDOWN)
                .is_ok_and(|v| v != 0)
                .then(|| m.i32(s + lobby::COUNTDOWN + 4).ok())
                .flatten(),
            peers,
        })
    }
}

/// The hosting update (0x719c5) resolves each expected ID against membership.
fn roster_ids_present(m: &dyn GuestMemory, s: u32, count: u32) -> Option<u32> {
    if count > MAX_MEMBERS {
        return None;
    }
    let mask = m.u32(s + at::PLAYER_MASK).ok()?;
    let mut members = [[0; PLAYER_ID_SIZE]; MAX_MEMBERS as usize];
    for slot in (0..MAX_MEMBERS).filter(|slot| mask & (1 << slot) != 0) {
        m.read(
            s + at::PLAYERS + slot * at::PLAYER_SIZE,
            &mut members[slot as usize],
        )
        .ok()?;
    }
    let mut present = 0;
    for i in 0..count {
        let mut id = [0; PLAYER_ID_SIZE];
        m.read(s + ROSTER_IDS + i * PLAYER_ID_SIZE as u32, &mut id).ok()?;
        if (0..MAX_MEMBERS).any(|slot| mask & (1 << slot) != 0 && members[slot as usize] == id) {
            present += 1;
        }
    }
    Some(present)
}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct Peer {
    phantom: Option<bool>,
    map: Option<u32>,
    hopper: Option<u32>,
    arbitration: Option<u32>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum Event {
    Frame,
    MatchmakingUpdate,
    ReleaseKey { caller: Option<u32> },
}

#[derive(Serialize)]
struct Row {
    sequence: u64,
    #[serde(flatten)]
    event: Event,
    state: State,
}

#[derive(Default)]
struct Trace {
    rows: VecDeque<Row>,
    sequence: u64,
    in_matchmaking: bool,
    /// The matchmaking result after the last update, for [`Trace::ended`].
    result: Option<u32>,
}

/// What a matchmaking result other than 2 (done) or 3 (active) means: the
/// branch of the update `0x706f0` or its checks that set it. Any such
/// result returns the party to its lobby (`0x70942`) and the title shows
/// its matchmaking failure.
fn result_reason(result: u32) -> &'static str {
    match result {
        1 => "the matchmaking setup failed (0x72330)",
        4 => "the party leader cancelled (0x72260, squad +0x49fc)",
        5 => "a party console failed to load the map (0x707d0)",
        6 => "the party changed or left the hopper (0x72540)",
        7 => "the party or the target has no hopper or roster for it (0x66050, 0x718b1)",
        0xe => "the target session left its lobby states (0x717b6)",
        0x11 => "this console's arbitration registration failed (0x72260)",
        0x13 => "the update's +0x10 flag is clear (0x70765)",
        _ => "an untraced branch",
    }
}

impl Trace {
    /// After a matchmaking update: the log line when the result leaves 3
    /// (active) for a failure, else `None`.
    fn ended(&mut self, m: &dyn GuestMemory) -> Option<String> {
        let now = m.u32(MATCHMAKING_RESULT).ok();
        let before = std::mem::replace(&mut self.result, now);
        let now = now?;
        if before != Some(3) || matches!(now, 2 | 3) {
            return None;
        }
        let arbitration = |at| m.u32(at).map_or("?".into(), |v| v.to_string());
        Some(format!(
            "live: matchmaking ended with result {now:#x}: {} (arbitration state {}, result {})",
            result_reason(now),
            arbitration(ARBITRATION_STATE),
            arbitration(ARBITRATION_RESULT),
        ))
    }

    fn record(&mut self, m: &dyn GuestMemory, event: Event) {
        let Ok(lifecycle) = m.u32(lobby::SESSION_TYPE) else {
            return;
        };
        let matchmaking = (6..=9).contains(&lifecycle)
            && m.u8(lobby::NETWORKING).is_ok_and(|v| v != 0);
        if !matchmaking && !self.in_matchmaking {
            return;
        }
        self.in_matchmaking = matchmaking;
        // Once the lobby has advanced to play or results, observe only
        // lifecycle changes and explicit releases.
        if matches!(event, Event::Frame)
            && matches!(lifecycle, 8 | 9)
            && self.rows.back().is_some_and(|row| row.state.lifecycle == lifecycle)
        {
            return;
        }
        let state = State::read(m, lifecycle);
        if !matches!(event, Event::ReleaseKey { .. })
            && self.rows.back().is_some_and(|row| row.state == state)
        {
            return;
        }
        if self.rows.len() == CAPACITY {
            self.rows.pop_front();
        }
        self.sequence += 1;
        self.rows.push_back(Row {
            sequence: self.sequence,
            event,
            state,
        });
    }

    fn release_key(&mut self, m: &dyn GuestMemory, session: u32, stack: u32) {
        if m.u32(lobby::TARGET_SESSION).ok() == Some(session)
            && session
                .checked_add(HAS_KEY)
                .is_some_and(|at| m.u8(at).is_ok_and(|v| v != 0))
        {
            self.record(
                m,
                Event::ReleaseKey {
                    caller: m.u32(stack).ok(),
                },
            );
        }
    }
}

#[derive(Clone, Default)]
struct Observer(Arc<Mutex<Trace>>);

impl Observer {
    fn lock(&self) -> MutexGuard<'_, Trace> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl hooks::Feature for Observer {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new("agent-matchmaking", "Agent API: matchmaking transitions")
            .doc("docs/runtime/agent-api.md")
    }

    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        let frame = self.clone();
        b.raw()
            .observe_before(&points::RENDER_FRAME, "observe matchmaking lifecycle", move |o| {
                let m = o.guest();
                frame.lock().record(&*m, Event::Frame);
            })
            .optional();
        let update = self.clone();
        b.raw()
            .observe_after(
                &points::MATCHMAKING_UPDATE,
                "observe the matchmaking decision",
                move |o| {
                    let m = o.guest();
                    let mut trace = update.lock();
                    trace.record(&*m, Event::MatchmakingUpdate);
                    if let Some(line) = trace.ended(&*m) {
                        eprintln!("{line}");
                    }
                },
            )
            .optional();
        let release = self.clone();
        b.raw()
            .observe_before(
                &points::SESSION_RELEASE_KEY,
                "observe the target key release",
                move |o| {
                    let m = o.guest();
                    release.lock().release_key(&*m, o.cpu.esi, o.cpu.esp);
                },
            )
            .optional();
        Ok(())
    }
}

pub(super) fn install(registry: &mut hooks::Registry, agent: &agent::Agent) -> anyhow::Result<()> {
    let observer = Observer::default();
    if registry.add(&observer)? {
        agent.add_method(
            method::LIVE_MATCHMAKING_TRACE,
            agent::Access::Read,
            move |params| {
                if !params.is_null() && !params.as_object().is_some_and(|fields| fields.is_empty()) {
                    return Err(RpcError::new(code::INVALID_PARAMS, "Expected no parameters."));
                }
                Ok(json!({ "transitions": observer.lock().rows, "hopper_failures": crate::live_hopper::failures() }))
            },
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bots::phantom::session;
    use guest_layout::testing::FakeGuest;

    #[test]
    fn a_failed_result_after_an_active_one_is_named_once() {
        let mut m = FakeGuest::new();
        m.map(MATCHMAKING_RESULT, 4)
            .map(ARBITRATION_RESULT, 8)
            .put_u32(MATCHMAKING_RESULT, 3);
        let mut trace = Trace::default();
        assert_eq!(trace.ended(&m), None);
        assert_eq!(trace.ended(&m), None);
        // The poller's failure (0x73dc0(1)): arbitration state 0, result 1.
        m.put_u32(MATCHMAKING_RESULT, 0x11)
            .put_u32(ARBITRATION_STATE, 0)
            .put_u32(ARBITRATION_RESULT, 1);
        let line = trace.ended(&m).expect("the failure is named");
        assert!(line.contains("result 0x11"), "{line}");
        assert!(line.contains("arbitration registration failed"), "{line}");
        assert!(line.contains("arbitration state 0, result 1"), "{line}");
        assert_eq!(trace.ended(&m), None, "once per failure");

        // A started match (2) is no failure.
        m.put_u32(MATCHMAKING_RESULT, 3);
        assert_eq!(trace.ended(&m), None);
        m.put_u32(MATCHMAKING_RESULT, 2);
        assert_eq!(trace.ended(&m), None);
    }

    #[test]
    fn matchmaking_trace_keeps_the_full_target_and_its_teardown() {
        let squad = 0x100000;
        let target = 0x110000;
        let stack = 0x120000;
        let mut m = FakeGuest::new();
        m.map(lobby::NETWORKING, 0x1000)
            .map(ARBITRATION_RESULT, 8)
            .map(squad, 0x8000)
            .map(target, 0x8000)
            .map(stack, 4)
            .put_u8(lobby::NETWORKING, 1)
            .put_u32(lobby::SESSION_TYPE, 6)
            .put_u32(lobby::SESSION, squad)
            .put_u32(lobby::TARGET_SESSION, target)
            .put_u32(squad + at::STATE, 5)
            .put_u32(squad + at::CLASS, 11)
            .put_u32(squad + at::PLAYER_COUNT, 1)
            .put_u32(target + at::STATE, 5)
            .put_u32(target + at::CLASS, 1)
            .put_u32(target + at::MAX_PLAYERS, 8)
            .put_u32(target + HOPPER, 8)
            .put_u8(target + HAS_KEY, 1)
            .put(target + KEY_ID, &[1; 8])
            .put_u8(target + HAS_ROSTER, 1)
            .put_u32(target + at::MAX_PEERS, 8)
            .put_u32(target + at::PEER_COUNT, 1)
            .put_u32(target + at::PLAYER_COUNT, 1)
            .put_u32(target + at::PLAYER_MASK, 1)
            .put_u32(target + at::PEERS + at::PEER_SLOTS, 0)
            .put_u32(target + at::PEERS + at::PEER_MAP_STATUS, 4)
            .put_u32(target + at::PEERS + at::PEER_HOPPERS + 8 * 4, 1)
            .put_u32(target + ROSTER_PLAYERS, 1);
        let mut trace = Trace::default();
        trace.record(&m, Event::Frame);
        for i in 1..8u32 {
            let mac = bots::phantom::machine_id(bots::phantom::PhantomHandle(i));
            let id = session::player_id(u64::from(i), mac);
            let peer = session::add_peer(&mut m, target, mac, "Clanker").unwrap().unwrap();
            session::sync_props(&mut m, target, peer, "Clanker").unwrap();
            let config = session::config_in(&m, target, "Clanker", None).unwrap().unwrap();
            assert_eq!(
                session::add_player(&mut m, target, peer, id, &config).unwrap(),
                Some(i)
            );
            m.put_u32(target + ROSTER_PLAYERS, i + 1)
                .put(target + ROSTER_IDS + i * PLAYER_ID_SIZE as u32, &id);
            trace.record(&m, Event::Frame);
            trace.record(&m, Event::MatchmakingUpdate);
        }
        assert_eq!(trace.rows.len(), 8);
        // Model one title rejection to verify that its result survives
        // cleanup. This fixture does not identify the live rejection.
        m.put_u32(MATCHMAKING_RESULT, 17)
            .put_u32(ARBITRATION_RESULT, 1)
            .put_u32(stack, 0x5a5ad);
        trace.record(&m, Event::MatchmakingUpdate);
        trace.release_key(&m, target, stack);
        m.put_u32(lobby::SESSION_TYPE, 1)
            .put_u32(target + at::STATE, 0)
            .put_u8(target + HAS_KEY, 0);
        trace.record(&m, Event::Frame);
        trace.record(&m, Event::Frame);

        let output = json!({ "transitions": trace.rows });
        let rows = output["transitions"].as_array().unwrap();
        assert_eq!(rows.len(), 11);
        assert_eq!(rows[7]["state"]["target"]["players"], 8);
        assert_eq!(rows[7]["state"]["target"]["roster_players"], 8);
        assert_eq!(rows[7]["state"]["target"]["roster_ids_present"], 8);
        assert_eq!(rows[7]["state"]["target"]["peers"][7]["map"], 4);
        assert_eq!(rows[7]["state"]["target"]["peers"][7]["hopper"], 1);
        assert_eq!(rows[7]["state"]["target"]["peers"][7]["phantom"], true);
        assert_eq!(rows[8]["state"]["result"], 17);
        assert_eq!(rows[9]["event"], "release_key");
        assert_eq!(rows[9]["caller"], 0x5a5ad);
        assert_eq!(rows[9]["state"]["target"]["id"], "0101010101010101");
        assert_eq!(rows[10]["state"]["lifecycle"], 1);
        assert!(rows[10]["state"]["target"].is_null());
        assert_eq!(rows[10]["state"]["squad"]["players"], 1);
    }
}
