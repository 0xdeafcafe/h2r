//! `host.pause` and `host.step` (docs/runtime/agent-api.md, "Window
//! pause and frame step"): the window's own pause, the one Cmd+P toggles,
//! asked for over the agent API, and N game ticks run while it holds.
//!
//! Three parties meet at one process-wide [`Hub`] ([`hub`]):
//!
//! - the window shell (`app::shell`) [`attach`](Hub::attach)es with a
//!   waker, takes each [`request`](Hub::take_request), applies it as Cmd+P
//!   does and [`report`](Hub::report)s the pause it now holds (Cmd+P
//!   reports too);
//! - the guest thread's pause wait (`app::Lifecycle`) runs while the
//!   [step gate](Hub::gate_open) is open and says when it is
//!   [held](Hub::set_held);
//! - the agent's per-tick ticker [counts](Hub::ticked) each game tick, and
//!   the methods ([`Hub::pause`], [`Hub::step`]) wait on the others.
//!
//! The methods are answered on the connection's own thread, since a held
//! guest thread answers nothing. This module has no game dependencies:
//! the shell links it without the agent.

use std::sync::{Condvar, Mutex, MutexGuard};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::{RpcError, code};

pub mod method {
    /// Pause or resume the window's game loop ([`super::PauseParams`]).
    pub const PAUSE: &str = "host.pause";
    /// Run N game ticks while paused ([`super::StepParams`]).
    pub const STEP: &str = "host.step";
    pub const ALL: &[&str] = &[PAUSE, STEP];
}

/// The most ticks one `host.step` runs (as `session.step`).
pub const MAX_STEP: u64 = 1800;
/// How long `host.pause` waits for the window to apply the request.
pub const SHELL_WAIT: Duration = Duration::from_secs(2);
/// How long `host.pause {on: true}` then waits for the guest to stop.
pub const HOLD_WAIT: Duration = Duration::from_secs(2);

/// `host.pause`'s params: `{}` reads the state, `{"on": bool}` changes it
/// (as `perf.overlay` reads and sets).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PauseParams {
    /// The pause to hold; absent, the state is read without changing it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<bool>,
}

/// `host.step`'s params.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StepParams {
    #[serde(default = "one")]
    pub ticks: u64,
}

fn one() -> u64 {
    1
}

/// `host.pause`'s and `host.step`'s result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PauseState {
    /// The window's pause (Cmd+P's) is on.
    pub paused: bool,
    /// The guest thread is stopped at its pause wait.
    pub held: bool,
    /// The game's tick at the last game tick the agent saw.
    pub tick: Option<u64>,
    /// `host.step` only: game ticks run by the step. More than asked when
    /// the game ran several ticks in its last frame before stopping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stepped: Option<u64>,
}

type Waker = Box<dyn Fn() + Send>;

#[derive(Default)]
struct State {
    /// A window shell is attached, and how to wake it.
    shell: Option<Waker>,
    /// The thread it runs on: a method called there (the in-process debug
    /// menu, from a key press) must not wait for it.
    shell_thread: Option<ThreadId>,
    /// A pause the shell has yet to apply.
    request: Option<bool>,
    /// The shell's pause (Cmd+P's), as last reported.
    paused: bool,
    /// Ticks the step gate still lets run.
    gate: u64,
    /// A step is under way (until the guest is held again).
    stepping: bool,
    /// Ticks run by the current step.
    ran: u64,
    /// The guest thread is held at its pause wait.
    held: bool,
    /// The game's tick at the last game tick.
    tick: Option<u64>,
    /// Bumped by each step and each resume, so a waiting step sees it was
    /// overtaken.
    epoch: u64,
}

/// The meeting point. [`hub`] is the process's; tests make their own.
#[derive(Default)]
pub struct Hub {
    state: Mutex<State>,
    cv: Condvar,
}

static HUB: Hub = Hub::new();

/// The process's hub.
pub fn hub() -> &'static Hub {
    &HUB
}

fn unavailable(msg: impl Into<String>) -> RpcError {
    RpcError::new(code::UNAVAILABLE, msg)
}

const NO_WINDOW: &str = "no window here: host.pause is the window's pause (Cmd+P); a headless \
                         game pauses with session.pause";

impl Hub {
    pub const fn new() -> Hub {
        Hub {
            state: Mutex::new(State {
                shell: None,
                shell_thread: None,
                request: None,
                paused: false,
                gate: 0,
                stepping: false,
                ran: 0,
                held: false,
                tick: None,
                epoch: 0,
            }),
            cv: Condvar::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn wait_until<'a>(
        &'a self,
        mut s: MutexGuard<'a, State>,
        deadline: Instant,
        mut done: impl FnMut(&State) -> bool,
    ) -> (MutexGuard<'a, State>, bool) {
        loop {
            if done(&s) {
                return (s, true);
            }
            let now = Instant::now();
            if now >= deadline {
                return (s, false);
            }
            s = self
                .cv
                .wait_timeout(s, deadline - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    fn view(s: &State, stepped: Option<u64>) -> PauseState {
        PauseState {
            paused: s.paused,
            held: s.held,
            tick: s.tick,
            stepped,
        }
    }

    // ---- the window shell ----

    /// A window shell is here, on this thread; `wake` gets it to call
    /// [`take_request`](Self::take_request) soon.
    pub fn attach(&self, wake: Waker) {
        let mut s = self.lock();
        s.shell = Some(wake);
        s.shell_thread = Some(std::thread::current().id());
        s.request = None;
        self.cv.notify_all();
    }

    /// The shell is gone: requests are refused, a waiting step ends.
    pub fn detach(&self) {
        let mut s = self.lock();
        s.shell = None;
        s.shell_thread = None;
        s.request = None;
        s.paused = false;
        s.gate = 0;
        s.stepping = false;
        s.epoch += 1;
        self.cv.notify_all();
    }

    /// The pause the agent asked for, once.
    pub fn take_request(&self) -> Option<bool> {
        self.lock().request.take()
    }

    /// The shell's pause after it applied a request or Cmd+P. A resume
    /// closes the step gate.
    pub fn report(&self, paused: bool) {
        let mut s = self.lock();
        if s.paused && !paused {
            s.gate = 0;
            s.stepping = false;
            s.epoch += 1;
        }
        s.paused = paused;
        self.cv.notify_all();
    }

    // ---- the guest thread's pause wait ----

    /// The guest may run while paused: a step has ticks left.
    pub fn gate_open(&self) -> bool {
        self.lock().gate > 0
    }

    /// The guest thread stopped at (true) or left (false) its pause wait.
    /// Held with the gate shut ends a step.
    pub fn set_held(&self, held: bool) {
        let mut s = self.lock();
        if s.held != held {
            s.held = held;
            if held && s.stepping && s.gate == 0 {
                s.stepping = false;
            }
            self.cv.notify_all();
        }
    }

    // ---- the agent's ticker ----

    /// A game tick ran; `tick` is the game's count.
    pub fn ticked(&self, tick: Option<u64>) {
        let mut s = self.lock();
        s.tick = tick.or(s.tick);
        if s.stepping {
            s.ran += 1;
            if s.gate > 0 {
                s.gate -= 1;
                if s.gate == 0 {
                    self.cv.notify_all();
                }
            }
        }
    }

    // ---- the methods ----

    /// The state now, without asking for anything; refused with no window,
    /// so a read never reports a pause nothing holds.
    pub fn read(&self) -> Result<PauseState, RpcError> {
        let s = self.lock();
        if s.shell.is_none() {
            return Err(unavailable(NO_WINDOW));
        }
        Ok(Self::view(&s, None))
    }

    /// The state now, without asking for anything.
    pub fn state(&self) -> PauseState {
        Self::view(&self.lock(), None)
    }

    fn on_shell_thread(s: &State) -> bool {
        s.shell_thread == Some(std::thread::current().id())
    }

    /// `host.pause {on}`: asks the shell, waits up to `shell_wait` for it
    /// to apply, then (pausing) up to `hold_wait` for the guest to stop.
    /// Called on the shell's own thread it can't wait: it answers at once
    /// with the pause asked for, which the shell applies when it next
    /// wakes, and `held` false.
    pub fn pause(
        &self,
        on: bool,
        shell_wait: Duration,
        hold_wait: Duration,
    ) -> Result<PauseState, RpcError> {
        let mut s = self.lock();
        if s.shell.is_none() {
            return Err(unavailable(NO_WINDOW));
        }
        if s.paused != on {
            s.request = Some(on);
            if let Some(wake) = &s.shell {
                wake();
            }
            if Self::on_shell_thread(&s) {
                return Ok(PauseState {
                    paused: on,
                    held: false,
                    ..Self::view(&s, None)
                });
            }
            let (g, ok) = self.wait_until(s, Instant::now() + shell_wait, |s| {
                s.paused == on || s.shell.is_none()
            });
            s = g;
            if s.shell.is_none() {
                return Err(unavailable(NO_WINDOW));
            }
            if !ok {
                s.request = None;
                return Err(RpcError::new(
                    code::TIMEOUT,
                    "the window did not apply the pause in time",
                ));
            }
        }
        if on && !Self::on_shell_thread(&s) {
            let (g, _) = self.wait_until(s, Instant::now() + hold_wait, |s| {
                s.held || !s.paused || s.gate > 0
            });
            s = g;
        }
        Ok(Self::view(&s, None))
    }

    /// `host.step {ticks}`: opens the gate for `ticks` game ticks and waits
    /// up to `wait` until they ran and the guest is held again. Called on
    /// the shell's own thread it answers at once, `stepped` absent: the
    /// step runs on, and a second one is `BUSY` until it ends.
    pub fn step(&self, ticks: u64, wait: Duration) -> Result<PauseState, RpcError> {
        if !(1..=MAX_STEP).contains(&ticks) {
            return Err(RpcError::new(
                code::INVALID_PARAMS,
                format!("ticks: 1 to {MAX_STEP}"),
            ));
        }
        let mut s = self.lock();
        if s.shell.is_none() {
            return Err(unavailable(NO_WINDOW));
        }
        if !s.paused {
            return Err(RpcError::new(
                code::INVALID_REQUEST,
                "not paused: host.pause {\"on\": true} first",
            ));
        }
        if s.stepping {
            return Err(RpcError::new(code::BUSY, "a step is already running"));
        }
        s.epoch += 1;
        let epoch = s.epoch;
        s.gate = ticks;
        s.ran = 0;
        s.stepping = true;
        s.held = false;
        self.cv.notify_all();
        if Self::on_shell_thread(&s) {
            return Ok(Self::view(&s, None));
        }
        let (mut s, ok) = self.wait_until(s, Instant::now() + wait, |s| {
            s.epoch != epoch || !s.stepping
        });
        if s.epoch != epoch {
            return Err(RpcError::new(
                code::INTERRUPTED,
                "the step ended early: resumed, or the window closed",
            ));
        }
        if !ok {
            s.gate = 0;
            s.stepping = false;
            let mut e = RpcError::new(
                code::TIMEOUT,
                "the step's ticks did not run in time (is the game loading, or the agent's \
                 tick observer off?)",
            );
            e.data = Some(serde_json::json!({ "stepped": s.ran }));
            return Err(e);
        }
        let ran = s.ran;
        Ok(Self::view(&s, Some(ran)))
    }
}

/// How long `host.step {ticks}` waits: a generous wall time per tick
/// (slow motion stretches ticks) plus slack, at most two minutes.
pub fn step_wait(ticks: u64) -> Duration {
    Duration::from_millis((ticks.saturating_mul(200)).saturating_add(5_000).min(120_000))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;

    fn leak() -> &'static Hub {
        Box::leak(Box::new(Hub::new()))
    }

    /// A shell attached from a thread of its own, so the test thread's
    /// calls wait as an agent connection's do.
    fn attach_off(h: &'static Hub, wake: Waker) {
        thread::spawn(move || h.attach(wake)).join().unwrap();
    }

    #[test]
    fn on_the_shells_own_thread_nothing_waits() {
        let h = leak();
        let wakes = Arc::new(AtomicUsize::new(0));
        let w = Arc::clone(&wakes);
        h.attach(Box::new(move || {
            w.fetch_add(1, Ordering::SeqCst);
        }));
        // The debug menu's key press: answered at once, applied when the
        // shell takes the request.
        let r = h.pause(true, SHELL_WAIT, HOLD_WAIT).unwrap();
        assert!(r.paused && !r.held);
        assert_eq!(wakes.load(Ordering::SeqCst), 1);
        assert_eq!(h.take_request(), Some(true));
        h.report(true);
        let r = h.step(3, step_wait(3)).unwrap();
        assert_eq!(r.stepped, None);
        assert!(h.gate_open());
        assert_eq!(h.step(1, step_wait(1)).unwrap_err().code, code::BUSY);
        // The step runs on: three ticks, then held, and it ends.
        for t in 0..3 {
            h.ticked(Some(t));
        }
        h.set_held(true);
        assert_eq!(h.step(1, step_wait(1)).unwrap().stepped, None, "a new one starts");
    }

    /// A shell on a thread: applies each request when woken and reports.
    fn shell(h: &'static Hub) -> Arc<AtomicUsize> {
        let wakes = Arc::new(AtomicUsize::new(0));
        let w = Arc::clone(&wakes);
        attach_off(h, Box::new(move || {
            w.fetch_add(1, Ordering::SeqCst);
            thread::spawn(move || {
                // The shell runs after the agent's lock is released.
                for _ in 0..200 {
                    if let Some(on) = h.take_request() {
                        h.report(on);
                        return;
                    }
                    thread::sleep(Duration::from_millis(1));
                }
            });
        }));
        wakes
    }

    #[test]
    fn without_a_window_both_refuse() {
        let h = leak();
        let e = h.pause(true, SHELL_WAIT, HOLD_WAIT).unwrap_err();
        assert_eq!(e.code, code::UNAVAILABLE);
        assert!(e.message.contains("session.pause"), "{e}");
        assert_eq!(h.step(1, step_wait(1)).unwrap_err().code, code::UNAVAILABLE);
    }

    #[test]
    fn a_pause_goes_through_the_shell_and_waits_for_the_hold() {
        let h = leak();
        let wakes = shell(h);
        // The guest stops at its wait shortly after the pause.
        let guest = thread::spawn(move || {
            for _ in 0..500 {
                if h.state().paused {
                    h.set_held(true);
                    return;
                }
                thread::sleep(Duration::from_millis(1));
            }
        });
        let r = h.pause(true, SHELL_WAIT, HOLD_WAIT).unwrap();
        guest.join().unwrap();
        assert!(r.paused);
        assert_eq!(wakes.load(Ordering::SeqCst), 1);
        // Already paused: answered without asking the shell again.
        assert!(h.pause(true, SHELL_WAIT, Duration::ZERO).unwrap().paused);
        assert_eq!(wakes.load(Ordering::SeqCst), 1);
        let r = h.pause(false, SHELL_WAIT, HOLD_WAIT).unwrap();
        assert!(!r.paused);
    }

    #[test]
    fn a_shell_that_never_applies_times_out_and_drops_the_request() {
        let h = leak();
        attach_off(h, Box::new(|| {}));
        let e = h
            .pause(true, Duration::from_millis(20), HOLD_WAIT)
            .unwrap_err();
        assert_eq!(e.code, code::TIMEOUT);
        assert_eq!(h.take_request(), None);
    }

    #[test]
    fn a_step_needs_a_pause_and_sane_ticks() {
        let h = leak();
        attach_off(h, Box::new(|| {}));
        assert_eq!(h.step(1, step_wait(1)).unwrap_err().code, code::INVALID_REQUEST);
        h.report(true);
        assert_eq!(h.step(0, step_wait(0)).unwrap_err().code, code::INVALID_PARAMS);
        assert_eq!(
            h.step(MAX_STEP + 1, step_wait(1)).unwrap_err().code,
            code::INVALID_PARAMS
        );
        assert!(!h.gate_open(), "nothing opened");
    }

    /// The guest loop as `app::Lifecycle` runs it: held while paused and
    /// the gate shut; each slice runs `per_slice` game ticks.
    fn guest(h: &'static Hub, per_slice: u64, slices: usize) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            let mut tick = 100;
            for _ in 0..slices {
                if h.state().paused && !h.gate_open() {
                    h.set_held(true);
                    thread::sleep(Duration::from_millis(1));
                    continue;
                }
                h.set_held(false);
                for _ in 0..per_slice {
                    tick += 1;
                    h.ticked(Some(tick));
                }
            }
        })
    }

    #[test]
    fn a_step_runs_its_ticks_and_answers_once_held_again() {
        let h = leak();
        attach_off(h, Box::new(|| {}));
        h.report(true);
        h.set_held(true);
        let g = guest(h, 1, 2000);
        let r = h.step(5, step_wait(5)).unwrap();
        assert_eq!(r.stepped, Some(5));
        assert!(r.paused && r.held);
        assert!(!h.gate_open());
        let r = h.step(1, step_wait(1)).unwrap();
        assert_eq!(r.stepped, Some(1));
        h.detach();
        g.join().unwrap();
    }

    #[test]
    fn a_frame_of_several_ticks_overshoots_and_says_so() {
        let h = leak();
        attach_off(h, Box::new(|| {}));
        h.report(true);
        h.set_held(true);
        let g = guest(h, 2, 2000);
        let r = h.step(3, step_wait(3)).unwrap();
        assert_eq!(r.stepped, Some(4));
        h.detach();
        g.join().unwrap();
    }

    #[test]
    fn a_resume_interrupts_a_step_and_a_stalled_one_times_out() {
        let h = leak();
        attach_off(h, Box::new(|| {}));
        h.report(true);
        let t = thread::spawn(move || h.step(10, step_wait(10)));
        while !h.gate_open() {
            thread::yield_now();
        }
        h.report(false);
        assert_eq!(t.join().unwrap().unwrap_err().code, code::INTERRUPTED);
        assert!(!h.gate_open());
        // No ticks ever come: a timeout, with the gate shut after it.
        h.report(true);
        let e = h.step(2, Duration::from_millis(20)).unwrap_err();
        assert_eq!(e.code, code::TIMEOUT);
        assert_eq!(e.data.unwrap()["stepped"], 0);
        assert!(!h.gate_open());
    }

    #[test]
    fn the_wire_shapes() {
        let p: StepParams = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(p.ticks, 1);
        let p: PauseParams = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(p.on, None, "{} reads the state");
        let p: PauseParams = serde_json::from_value(serde_json::json!({"on": true})).unwrap();
        assert_eq!(p.on, Some(true));
        let v = serde_json::to_value(PauseState {
            paused: true,
            held: true,
            tick: Some(7),
            stepped: None,
        })
        .unwrap();
        assert_eq!(v, serde_json::json!({"paused": true, "held": true, "tick": 7}));
    }
}
