//! Preserve the retail idle/movie sequence when the attract clip is ready.
//! If the decoder cannot prepare it, keep the menu music gate open and
//! consume the deadline on the UI clock (docs/audio/bink-hle.md).

use std::sync::{Arc, Mutex, MutexGuard};

use agent_proto::{RpcError, code, method};
use bink_hle::addr::{ST_ACTIVE, ST_BINK, ST_MOVIE_OPEN};
use engine::points::{
    CACHE_COPY_PENDING, UI_ATTRACT_RESET, UI_ATTRACT_START, UI_ATTRACT_TIMER, UI_CLOCK,
    UI_LAST_INPUT, UI_MOVIE_RETURN, UI_MUSIC_GATE,
};
use serde::Serialize;
use serde_json::json;

use crate::menu_music::MenuMusic;
use crate::movies_glue::{ATTRACT_NAME, AssetStatus, AttractAsset};

#[derive(Clone)]
pub struct Attract {
    hle: Arc<bink_hle::BinkHle>,
    asset: AttractAsset,
    state: Arc<Mutex<State>>,
}

#[derive(Clone, Copy)]
struct TimerSample {
    now: u32,
    origin: u32,
    due: bool,
    present: u64,
    wrapper_block: Option<&'static str>,
}

#[derive(Clone, Serialize)]
struct Attempt {
    ui_ms: u32,
    idle_ms: u32,
    present: u64,
    outcome: Outcome,
    returned_ui_ms: Option<u32>,
    #[serde(skip)]
    previous_request: Option<usize>,
}

#[derive(Clone, Serialize)]
#[serde(tag = "kind", content = "reason", rename_all = "snake_case")]
enum Outcome {
    Starting,
    Started,
    Unavailable(String),
    Failed(String),
}

#[derive(Default)]
struct State {
    gate_on_entry: bool,
    cycle: Option<(u32, Result<(), String>)>,
    last_attempt: Option<Attempt>,
    logged_unavailable: bool,
}

impl State {
    fn timer(
        &mut self,
        sample: TimerSample,
        previous_request: Option<usize>,
        prepare: impl FnOnce() -> Result<(), String>,
    ) -> bool {
        if self
            .cycle
            .as_ref()
            .is_none_or(|(origin, _)| *origin != sample.origin)
        {
            self.cycle = Some((sample.origin, prepare()));
        }
        if let Some((_, prepared @ Ok(()))) = &mut self.cycle
            && let Some(reason) = sample.wrapper_block
        {
            *prepared = Err(reason.into());
        }
        let (_, prepared) = self.cycle.as_ref().expect("idle cycle has a decision");
        if sample.due {
            self.last_attempt = Some(Attempt {
                ui_ms: sample.now,
                idle_ms: sample.now.wrapping_sub(sample.origin),
                present: sample.present,
                outcome: match prepared {
                    Ok(()) => Outcome::Starting,
                    Err(reason) => Outcome::Unavailable(reason.clone()),
                },
                previous_request,
                returned_ui_ms: None,
            });
        }
        prepared.is_err()
    }

    fn started(&mut self, playing: bool, request: Option<bink_hle::RequestOutcome>) {
        let Some(attempt) = self
            .last_attempt
            .as_mut()
            .filter(|a| matches!(a.outcome, Outcome::Starting))
        else {
            return;
        };
        attempt.outcome = if playing {
            Outcome::Started
        } else {
            let failure = request
                .filter(|r| attempt.previous_request.is_none_or(|seq| r.sequence > seq))
                .and_then(|r| r.failure)
                .map_or_else(
                    || "the movie wrapper did not start playback".into(),
                    |e| e.to_string(),
                );
            Outcome::Failed(failure)
        };
        self.cycle = None;
    }

    fn returned(&mut self, now: u32) {
        if let Some(attempt) = self.last_attempt.as_mut()
            && matches!(attempt.outcome, Outcome::Started)
            && attempt.returned_ui_ms.is_none()
        {
            attempt.returned_ui_ms = Some(now);
        }
    }

    fn log_unavailable(&mut self) {
        if !self.logged_unavailable
            && let Some(Outcome::Unavailable(reason) | Outcome::Failed(reason)) =
                self.last_attempt.as_ref().map(|attempt| &attempt.outcome)
        {
            eprintln!("movies: attract unavailable ({reason})");
            self.logged_unavailable = true;
        }
    }

    fn phase(&self, menu: Option<MenuMusic>) -> &'static str {
        let Some(menu) = menu else {
            return "outside_menu";
        };
        if menu.movie_active
            && self.last_attempt.as_ref().is_some_and(|a| {
                matches!(a.outcome, Outcome::Started) && a.returned_ui_ms.is_none()
            })
        {
            return "playing";
        }
        match &self.cycle {
            Some((_, Ok(()))) => "fading",
            Some((_, Err(_))) => "keeping_music",
            None => "idle",
        }
    }
}

impl Attract {
    pub(crate) fn new(hle: Arc<bink_hle::BinkHle>, asset: AttractAsset) -> Self {
        Self {
            hle,
            asset,
            state: Arc::default(),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn install(
        &self,
        registry: &mut hooks::Registry,
        agent: Option<&agent::Agent>,
    ) -> Result<(), hooks::BuildError> {
        let enabled = registry.add(self)?;
        if let Some(agent) = agent {
            let attract = self.clone();
            agent.add_game_method(method::UI_ATTRACT, agent::Access::Read, move |params, ctx| {
                if !(params.is_null() || params.as_object().is_some_and(|p| p.is_empty())) {
                    return Err(RpcError::new(code::INVALID_PARAMS, "ui.attract takes no params"));
                }
                let mut menu = None;
                ctx.reader.with_guest(&mut |g| menu = MenuMusic::read(g).ok().flatten());
                let state = attract.state();
                Ok(json!({
                    "enabled": enabled,
                    "state": state.phase(menu),
                    "timer": menu,
                    "asset": attract.asset.status(),
                    "blocked_reason": state.cycle.as_ref().and_then(|(_, result)| result.as_ref().err()),
                    "last_attempt": state.last_attempt,
                }))
            });
        }
        Ok(())
    }

    fn prepare(&self, wrapper_block: Option<&'static str>) -> Result<(), String> {
        self.hle.discard_prepared(ATTRACT_NAME);
        match self.asset.status() {
            AssetStatus::Ready => match wrapper_block {
                Some(reason) => Err(reason.into()),
                None => self.hle.prepare(ATTRACT_NAME).map_err(|e| e.to_string()),
            },
            AssetStatus::Preparing => Err("the disc movie is still being prepared".into()),
            AssetStatus::Unavailable(reason) => Err(reason),
        }
    }

    fn after_timer(&self, c: &mut hooks::Call<'_>) -> Result<(), runtime::GuestFault> {
        let due = c.cpu.eax as u8 != 0;
        let mut state = self.state();
        let gate_open = c.ctx.memory().u8(UI_MUSIC_GATE.base)? != 0;
        if !due && gate_open {
            if state.cycle.take().is_some() {
                self.hle.discard_prepared(ATTRACT_NAME);
            }
            return Ok(());
        }
        // The retail timer sometimes returns early without touching
        // the gate (including while a movie plays). Act only on a
        // new closed gate or a true return from this invocation.
        if !due && !state.gate_on_entry {
            return Ok(());
        }
        let sample = {
            let m = c.ctx.memory();
            TimerSample {
                now: m.u32(UI_CLOCK.base)?,
                origin: m
                    .u32(UI_LAST_INPUT.base)?
                    .max(m.u32(UI_ATTRACT_RESET.base)?),
                due,
                present: crate::presents::presents(),
                wrapper_block: wrapper_block(&m),
            }
        };
        let previous_request = due
            .then(|| self.hle.last_request(ATTRACT_NAME))
            .flatten()
            .map(|r| r.sequence);
        if state.timer(sample, previous_request, || {
            self.prepare(sample.wrapper_block)
        }) {
            preserve_music(&mut c.ctx.memory(), &mut c.cpu.eax, sample)?;
            if due {
                self.hle.discard_prepared(ATTRACT_NAME);
                state.cycle = None;
                state.log_unavailable();
            }
        }
        Ok(())
    }
}

/// The wrapper checks these before dispatching BinkOpen (0x1560d6 and
/// 0x1560ee). Attract's flags leave the cache-copy guard enabled.
fn wrapper_block(m: &runtime::Guest) -> Option<&'static str> {
    if m.u8(ST_ACTIVE).ok() == Some(0) {
        Some("the movie wrapper is not initialised")
    } else if m.u32(crate::map_slots::STATE).is_ok_and(|state| state != 0)
        || m.u8(CACHE_COPY_PENDING.base)
            .is_ok_and(|pending| pending != 0)
    {
        Some("the map cache is busy")
    } else {
        None
    }
}

fn preserve_music(
    m: &mut runtime::Guest,
    eax: &mut u32,
    sample: TimerSample,
) -> Result<(), runtime::GuestFault> {
    m.set_u8(UI_MUSIC_GATE.base, 1)?;
    if sample.due {
        m.set_u32(UI_ATTRACT_RESET.base, sample.now)?;
        *eax &= !0xff;
    }
    Ok(())
}

impl hooks::Feature for Attract {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new(
            "attract",
            "Prepare the idle movie before the menu music fades",
        )
        .doc("docs/audio/bink-hle.md")
    }

    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        let attract = self.clone();
        b.raw()
            .before(&UI_ATTRACT_TIMER, "read the music gate", move |c| {
                attract.state().gate_on_entry = c.ctx.memory().u8(UI_MUSIC_GATE.base)? != 0;
                Ok(hooks::Flow::Continue)
            });
        let attract = self.clone();
        b.raw().after(
            &UI_ATTRACT_TIMER,
            "keep music if the attract movie is unavailable",
            move |c| attract.after_timer(c),
        );
        let attract = self.clone();
        b.raw().observe_after(
            &UI_ATTRACT_START,
            "record the attract movie start",
            move |o| {
                let mut state = attract.state();
                let playing = {
                    let m = o.guest();
                    m.u8(ST_ACTIVE).is_ok_and(|active| active != 0)
                        && m.u8(ST_MOVIE_OPEN).is_ok_and(|open| open != 0)
                        && m.u32(ST_BINK).is_ok_and(|handle| handle != 0)
                };
                state.started(playing, attract.hle.last_request(ATTRACT_NAME));
                if !playing {
                    attract.hle.discard_prepared(ATTRACT_NAME);
                }
                state.log_unavailable();
            },
        );
        let attract = self.clone();
        b.raw()
            .observe_after(&UI_MOVIE_RETURN, "record return to the menu", move |o| {
                let now = {
                    let m = o.guest();
                    (m.u32(ST_BINK).ok() == Some(0))
                        .then(|| m.u32(UI_CLOCK.base).ok())
                        .flatten()
                };
                if let Some(now) = now {
                    attract.state().returned(now);
                }
            });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::guest::Permissions;

    fn sample(now: u32, due: bool) -> TimerSample {
        TimerSample {
            now,
            origin: 100,
            due,
            present: 2300,
            wrapper_block: None,
        }
    }

    #[test]
    fn prepared_attract_leaves_the_gate_and_start_to_retail() {
        let mut state = State::default();
        assert!(!state.timer(sample(78_100, false), None, || Ok(())));
        assert!(state.last_attempt.is_none());
        assert!(!state.timer(sample(82_100, true), Some(4), || panic!(
            "prepared during fade"
        )));
        let attempt = state.last_attempt.as_ref().unwrap();
        assert!(matches!(attempt.outcome, Outcome::Starting));
        assert_eq!(attempt.idle_ms, 82_000);
        assert_eq!(attempt.present, 2300);
        state.started(true, None);
        state.returned(85_100);
        let attempt = state.last_attempt.unwrap();
        assert!(matches!(attempt.outcome, Outcome::Started));
        assert_eq!(attempt.returned_ui_ms, Some(85_100));
    }

    #[test]
    fn missing_movie_keeps_the_gate_open_and_consumes_only_the_deadline() {
        let mut g = runtime::Guest::new();
        for addr in [UI_MUSIC_GATE.base, UI_ATTRACT_RESET.base] {
            g.map_zeroed(addr & !0xfff, 0x1000, Permissions::RW)
                .unwrap();
        }
        let mut state = State::default();
        let mut eax = 0x1234_0000;
        let fade = sample(78_100, false);
        assert!(state.timer(fade, None, || Err("disc movie not found".into())));
        preserve_music(&mut g, &mut eax, fade).unwrap();
        assert_eq!(g.u8(UI_MUSIC_GATE.base).unwrap(), 1);
        assert_eq!(g.u32(UI_ATTRACT_RESET.base).unwrap(), 0);
        assert!(state.last_attempt.is_none());

        g.set_u8(UI_MUSIC_GATE.base, 0).unwrap();
        eax |= 1;
        let deadline = sample(82_100, true);
        assert!(state.timer(deadline, None, || panic!("one preflight per cycle")));
        preserve_music(&mut g, &mut eax, deadline).unwrap();
        assert_eq!(g.u8(UI_MUSIC_GATE.base).unwrap(), 1);
        assert_eq!(g.u32(UI_ATTRACT_RESET.base).unwrap(), deadline.now);
        assert_eq!(eax, 0x1234_0000);
        assert!(matches!(
            state.last_attempt.unwrap().outcome,
            Outcome::Unavailable(_)
        ));
    }

    #[test]
    fn a_later_idle_cycle_can_play_after_preparation_finishes() {
        let mut state = State::default();
        assert!(state.timer(sample(82_100, true), None, || Err("preparing".into())));
        let next = TimerSample {
            origin: 82_100,
            ..sample(160_100, false)
        };
        assert!(!state.timer(next, None, || Ok(())));
        assert!(!state.timer(
            TimerSample {
                now: 164_100,
                due: true,
                ..next
            },
            None,
            || { panic!("second cycle was prepared") }
        ));
        assert!(matches!(
            state.last_attempt.unwrap().outcome,
            Outcome::Starting
        ));
    }
}
