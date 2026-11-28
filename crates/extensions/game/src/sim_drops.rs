//! Seam drops in sim mode (docs/runtime/sim.md, "Seam drops"): the glue
//! between the game and the pure job ([`sim_bench::drops::Job`]).
//!
//! A `before` on the game tick (`sim: drops`, last in the chain, so after
//! the agent's guest calls have made last tick's teleports) reads every
//! bound seat's unit: origin, velocity, airborne bit, and every pair its
//! bodies have against a fixed body, with one walk of the islands
//! ([`live_telemetry::havok_agents::level_pairs`]). The job answers an
//! act a seat, carried out before the tick runs:
//!
//! - the pair word over every level pair whose word differs
//!   ([`live_telemetry::havok_agents::set_level_words`]): 0 while
//!   settling, -3 on the still body and on every tick of the fall;
//! - the body's velocity zeroed (the object's and each Havok motion's),
//!   or only its horizontal part (a straight drop's fall rows);
//! - the pad held for the seat's phantom (its brain is ignored: no walk,
//!   no fight), the walk turned into the stick against its facing. A
//!   seat whose pad can't be held is a note in the report: its brain's
//!   stick would steer the fall;
//! - a teleport through the agent's calls queue (`object_teleport`'s
//!   body, `agent::sim::calls`), its outcome handed back the tick it
//!   lands;
//! - the body's contacts read at the touchdown.
//!
//! At the end the run's proven targets go into its copy of the map's
//! verified file (`bots::spots::store_verified`), and the records into
//! the report's `drops` block.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard};

use agent::sim::calls::{self, Call, PendingCall, Shared};
use guest_layout::GuestMemory;
use live_telemetry::havok_agents::{self as hk, LevelPairs};
use sim_bench::drops::{ContactPoint, DropsRun, Job, PairSense, SeatSense, Sense};

/// Where an object's origin is in its body (`+0x64`): the point a
/// teleport places (`calls::Call::Teleport`). Not the bounding sphere's
/// centre at `+0x30`, which a biped's bounds update keeps about 0.4 units
/// over the origin and which moves with its pose and facing.
const OBJECT_POSITION: u32 = guest_layout::Object::POSITION.offset;

/// A bound seat: its logical seat, its player index and its phantom.
#[derive(Clone, Copy, Debug)]
pub struct Seat {
    pub seat: u8,
    pub player: u8,
    pub handle: Option<bots::phantom::PhantomHandle>,
}

struct State {
    map: String,
    job: Job,
    calls: Option<Shared>,
    tickets: BTreeMap<u8, PendingCall>,
    /// Teleports refused when asked, answered the next tick.
    refused: BTreeMap<u8, String>,
    last_tick: Option<u64>,
    saved: bool,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn lock() -> MutexGuard<'static, Option<State>> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// The run's job (`Sim::add_bots`).
pub fn set(map: &str, job: Job) {
    *lock() = Some(State {
        map: map.to_string(),
        job,
        calls: None,
        tickets: BTreeMap::new(),
        refused: BTreeMap::new(),
        last_tick: None,
        saved: false,
    });
}

/// The calls queue the teleports go through (`Sim::install_with_film`).
pub fn set_calls(q: Shared) {
    if let Some(s) = lock().as_mut() {
        s.calls = Some(q);
    }
}

/// Whether this run drops.
pub fn on() -> bool {
    lock().is_some()
}

/// The report block.
pub fn report() -> Option<DropsRun> {
    lock().as_ref().map(|s| s.job.report())
}

/// `match.status`'s `drops`: drops planned and done, the control's state
/// and the error, if any.
pub fn status() -> Option<serde_json::Value> {
    lock().as_ref().map(|s| {
        serde_json::json!({
            "planned": s.job.planned(),
            "done": s.job.records().len(),
            "control": s.job.gate().name(),
            "error": s.job.error(),
        })
    })
}

/// The named error that stopped the run (`control did not bounce: ...`).
pub fn error() -> Option<String> {
    lock()
        .as_ref()
        .and_then(|s| s.job.error().map(str::to_string))
}

/// What a seat's unit reads.
pub(crate) struct Body {
    pub(crate) unit: u32,
    pub(crate) base: u32,
    pub(crate) pos: Option<[f32; 3]>,
    pub(crate) vel: Option<[f32; 3]>,
    pub(crate) yaw: Option<f32>,
    pub(crate) airborne: bool,
}

pub(crate) fn body(m: &dyn GuestMemory, index: u32) -> Option<Body> {
    use live_telemetry::layout::{BIPED_FLAGS, BODY_VELOCITY, UNIT_AIM};
    let unit = agent::sim::state::unit_handle(m, index).ok()?;
    let p = agent::sim::state::player(m, index).ok()?;
    let base = agent::sim::state::unit(m, p).ok()??.addr;
    let f = |at: u32| {
        m.u32(base.wrapping_add(at))
            .ok()
            .map(f32::from_bits)
            .filter(|v| v.is_finite())
    };
    let v3 = |at: u32| Some([f(at)?, f(at + 4)?, f(at + 8)?]);
    let yaw = match (f(UNIT_AIM), f(UNIT_AIM + 4)) {
        (Some(x), Some(y)) if x != 0.0 || y != 0.0 => Some(y.atan2(x)),
        _ => None,
    };
    Some(Body {
        unit,
        base,
        pos: v3(OBJECT_POSITION),
        vel: v3(BODY_VELOCITY),
        yaw,
        airborne: m
            .u16(base.wrapping_add(BIPED_FLAGS))
            .is_ok_and(|flags| flags & 1 != 0),
    })
}

/// Zeroes the object's velocity and each of its Havok motions'.
pub(crate) fn stop(m: &mut dyn GuestMemory, b: &Body) {
    use live_telemetry::layout::{BODY_VELOCITY, MOTION_VELOCITY, RIGID_MOTION};
    let zero = [0u8; 12];
    let _ = m.write(b.base.wrapping_add(BODY_VELOCITY), &zero);
    let Ok(entities) = hk::object_entities(&*m, b.unit) else {
        return;
    };
    for e in entities {
        if let Ok(motion) = m.u32(e + RIGID_MOTION)
            && motion != 0
        {
            let _ = m.write(motion + MOTION_VELOCITY, &zero);
        }
    }
}

/// Writes the object's horizontal velocity and each of its Havok
/// motions' to `xy`, the vertical (the third float) left as it is.
pub(crate) fn level(m: &mut dyn GuestMemory, b: &Body, xy: [f32; 2]) {
    use live_telemetry::layout::{BODY_VELOCITY, MOTION_VELOCITY, RIGID_MOTION};
    let mut v = [0u8; 8];
    v[..4].copy_from_slice(&xy[0].to_bits().to_le_bytes());
    v[4..].copy_from_slice(&xy[1].to_bits().to_le_bytes());
    let _ = m.write(b.base.wrapping_add(BODY_VELOCITY), &v);
    let Ok(entities) = hk::object_entities(&*m, b.unit) else {
        return;
    };
    for e in entities {
        if let Ok(motion) = m.u32(e + RIGID_MOTION)
            && motion != 0
        {
            let _ = m.write(motion + MOTION_VELOCITY, &v);
        }
    }
}

/// The unit's contacts with anything, at most 8 points.
fn contacts(m: &dyn GuestMemory, unit: u32) -> Vec<ContactPoint> {
    let Ok(mine) = hk::object_entities(m, unit) else {
        return Vec::new();
    };
    let Ok(all) = live_telemetry::havok::contacts(m) else {
        return Vec::new();
    };
    let f3 = |v: [f64; 3]| [v[0] as f32, v[1] as f32, v[2] as f32];
    all.iter()
        .filter(|c| mine.contains(&c.a) || mine.contains(&c.b))
        .flat_map(|c| c.points.iter())
        .take(8)
        .map(|p| ContactPoint {
            pos: f3(p.pos),
            normal: f3(p.normal),
            distance: p.distance as f32,
            impulse: p.impulse as f32,
        })
        .collect()
}

pub(crate) fn pad(walk: [f32; 2], yaw: Option<f32>) -> input::host::HostPadState {
    let stick = crate::superbounce_verify::stick_for(walk, yaw.unwrap_or(0.0));
    let mut h = input::host::HostPadState::default();
    h.left_stick = (stick[0], stick[1]);
    h
}

/// One tick for `seats`, before the tick runs. True once the job is over
/// (the match should end).
pub fn tick(g: &mut runtime::Guest, seats: &[Seat]) -> bool {
    let mut guard = lock();
    let Some(st) = guard.as_mut() else {
        return false;
    };
    let Ok(tick) = crate::telemetry::game_tick(g) else {
        return st.job.finished();
    };
    // A tick's chain runs again after the agent's guest calls: once a tick.
    if st.last_tick == Some(tick) {
        return st.job.finished();
    }
    st.last_tick = Some(tick);
    let m: &mut dyn GuestMemory = g;
    let bodies: Vec<Option<Body>> = seats
        .iter()
        .map(|s| body(&*m, u32::from(s.player)))
        .collect();
    let units: Vec<u32> = bodies.iter().map(|b| b.as_ref().map_or(0, |b| b.unit)).collect();
    let pairs: Vec<LevelPairs> = hk::level_pairs(&*m, &units)
        .unwrap_or_else(|_| vec![LevelPairs::default(); units.len()]);
    let mut senses = Vec::with_capacity(seats.len());
    for (i, s) in seats.iter().enumerate() {
        let teleported = match st.refused.remove(&s.seat) {
            Some(e) => Some(Err(e)),
            None => match st.tickets.get(&s.seat).and_then(|t| t.outcome()) {
                Some(out) => {
                    st.tickets.remove(&s.seat);
                    Some(out.map(|_| ()).map_err(|e| e.message))
                }
                None => None,
            },
        };
        let b = bodies[i].as_ref();
        senses.push(SeatSense {
            seat: s.seat,
            sense: Sense {
                pos: b.and_then(|b| b.pos),
                vel: b.and_then(|b| b.vel),
                airborne: b.is_some_and(|b| b.airborne),
                pairs: pairs[i]
                    .pairs
                    .iter()
                    .map(|p| PairSense {
                        agent: p.agent,
                        word: p.word,
                    })
                    .collect(),
                asleep: pairs[i].asleep,
                teleported,
            },
        });
    }
    let acts = st.job.tick(tick, &senses);
    let phantoms = bots::phantom::global();
    for (i, s) in seats.iter().enumerate() {
        let act = acts
            .iter()
            .find(|a| a.seat == s.seat)
            .map(|a| a.act.clone())
            .unwrap_or_default();
        let b = bodies[i].as_ref();
        if let Some(w) = act.word
            && !pairs[i].pairs.is_empty()
        {
            let _ = hk::set_level_words(m, &pairs[i].pairs, w);
        }
        if act.stop
            && let Some(b) = b
        {
            stop(m, b);
        }
        if let Some(xy) = act.level
            && let Some(b) = b
        {
            level(m, b, xy);
        }
        if let Some(h) = s.handle
            && !phantoms.hold_pad(h, Some(pad(act.walk, b.and_then(|b| b.yaw))))
        {
            st.job.note(format!(
                "seat {}: its pad could not be held; its brain steered it",
                s.seat
            ));
        }
        if act.contacts
            && let Some(b) = b
        {
            let points = contacts(&*m, b.unit);
            st.job.contacts(s.seat, points);
        }
        if let Some((to, facing)) = act.teleport {
            let player = agent::sim::state::player_handle(&*m, u32::from(s.player)).ok();
            let submitted = match (&st.calls, player) {
                (Some(q), Some(player)) => calls::submit(
                    q,
                    Call::Teleport {
                        player,
                        pos: Some(to),
                        facing: Some([facing[0], facing[1], 0.0]),
                    },
                ),
                (None, _) => Err("no guest calls queue (agent-calls is off)".into()),
                (_, None) => Err(format!("no player.{}", s.player)),
            };
            match submitted {
                Ok(t) => {
                    st.tickets.insert(s.seat, t);
                }
                Err(e) => {
                    st.refused.insert(s.seat, e);
                }
            }
        }
    }
    st.job.finished()
}

/// Writes the run's proven targets into its copy of the map's verified
/// file, once. A note for the report, if any.
pub fn save() -> Option<String> {
    let mut guard = lock();
    let st = guard.as_mut()?;
    if st.saved {
        return None;
    }
    st.saved = true;
    let proven = st.job.proven();
    if proven.is_empty() {
        return None;
    }
    let entries: Vec<superbounce::verified::Entry> = proven
        .iter()
        .map(|p| superbounce::verified::Entry {
            kind: superbounce::Kind::Bounce,
            status: superbounce::Status::Proven,
            at: p.stand,
            offset: p.offset,
            yaw: None,
            height: Some(p.height),
            trials: p.trials,
            ok: p.ok,
            best: p.best,
        })
        .collect();
    let n = entries.len();
    Some(match bots::spots::store_verified(&st.map, &entries) {
        Ok(Some(path)) => format!("drops: {n} proven targets in {}", path.display()),
        Ok(None) => "drops: proven targets not saved: no bots data directory".into(),
        Err(e) => format!("drops: proven targets not saved: {e}"),
    })
}

#[cfg(test)]
mod tests {
    /// A seat's position is the origin the teleport writes, so a bounce
    /// trial's start (read from an earlier row) and its teleport's
    /// landing are the same point: read from the sphere centre, every
    /// trial landed about 0.4 units off.
    #[test]
    fn a_seat_reads_the_origin_a_teleport_places() {
        use guest_layout::Object;
        assert_eq!(super::OBJECT_POSITION, Object::POSITION.offset);
        assert_eq!(super::OBJECT_POSITION, 0x64);
        assert_ne!(super::OBJECT_POSITION, Object::BOUND_CENTER.offset);
        assert_eq!(
            super::OBJECT_POSITION,
            crate::telemetry::layout::BODY_POSITION
        );
    }

    #[test]
    fn the_drops_thresholds_are_the_huds_and_the_speed_models() {
        use sim_bench::drops as d;
        assert_eq!(d::TOUCHDOWN_VZ, crate::superbounce_charge::TOUCHDOWN_VZ);
        assert_eq!(d::LANDED_VZ, crate::superbounce_charge::LANDED_VZ);
        assert_eq!(d::GRAVITY, superbounce::speed::GRAVITY);
        assert_eq!(d::PRIMED, crate::superbounce_verify::trial::PRIMED);
        // Settled well before a body sleeps and its pair is rebuilt.
        assert!(d::SETTLE_STILL < crate::superbounce_charge::SLEEP_TICKS);
        assert!(d::STILL_SPEED > crate::superbounce_charge::STILL_SPEED);
        // The control's heights land clearly over the speed that bounces
        // (sim-bench's control test names 5.4).
        assert_eq!(superbounce::speed::BOUNCE_SPEED, 5.4);
        for h in d::CONTROL_HEIGHTS {
            let free = (2.0 * d::GRAVITY * h).sqrt();
            assert!(free * 0.97 > superbounce::speed::BOUNCE_SPEED + 0.3, "{h}: {free}");
        }
    }
}
