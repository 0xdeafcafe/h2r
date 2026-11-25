//! `host.window.resize` (docs/runtime/agent-api.md, "Resize the window"):
//! set the game window's size, or take it in or out of fullscreen, over the
//! agent API, so an aspect or field-of-view check can drive a live window
//! through the shapes it covers.
//!
//! The window shell (`app::shell`) [`attach`](Hub::attach)es to the
//! process-wide [`Hub`] ([`hub`]) with a waker, takes each
//! [`request`](Hub::take_request), applies it to its window, and
//! [`observe`](Hub::observe)s the window's state after applying it and on
//! every resize. The method ([`Hub::resize`]) waits until an observed state
//! matches the request. Sizes are the window's inner size in physical
//! pixels, the size the game renders at. This module has no game
//! dependencies: the shell links it without the agent.

use std::sync::{Condvar, Mutex, MutexGuard};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::{RpcError, code};

pub mod method {
    /// Resize the window or change its fullscreen
    /// ([`super::ResizeParams`], [`super::WindowState`]).
    pub const RESIZE: &str = "host.window.resize";
    pub const ALL: &[&str] = &[RESIZE];
}

/// How long `host.window.resize` waits for the window to take the shape
/// asked for. A fullscreen change animates on macOS.
pub const WAIT: Duration = Duration::from_secs(5);
/// The smallest and largest width or height asked for, in physical pixels.
pub const MIN_SIDE: u32 = 64;
pub const MAX_SIDE: u32 = 16384;

/// `host.window.resize`'s params: a size, a fullscreen state, or both
/// with `fullscreen: false`. A size leaves fullscreen first.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResizeParams {
    /// Inner width in physical pixels; given with `height`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    /// Inner height in physical pixels; given with `width`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// Borderless fullscreen on the window's display (true) or windowed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fullscreen: Option<bool>,
}

/// What the window asked for, once the params are checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Request {
    /// The inner size, in physical pixels.
    pub size: Option<(u32, u32)>,
    /// Fullscreen on (true) or off; a size implies off.
    pub fullscreen: bool,
}

impl ResizeParams {
    /// The request these params make, or why they make none.
    pub fn request(self) -> Result<Request, RpcError> {
        let invalid = |m: &str| Err(RpcError::new(code::INVALID_PARAMS, m));
        let size = match (self.width, self.height) {
            (Some(w), Some(h)) => Some((w, h)),
            (None, None) => None,
            _ => return invalid("give width and height together"),
        };
        if let Some((w, h)) = size
            && ![w, h].iter().all(|side| (MIN_SIDE..=MAX_SIDE).contains(side))
        {
            return invalid(format!("width and height are {MIN_SIDE} to {MAX_SIDE} pixels").as_str());
        }
        match (size, self.fullscreen) {
            (None, None) => invalid("give {width, height}, fullscreen, or both with fullscreen false"),
            (Some(_), Some(true)) => {
                invalid("a fullscreen window takes its display's size: give a size or fullscreen true, not both")
            }
            (Some(size), _) => Ok(Request { size: Some(size), fullscreen: false }),
            (None, Some(fullscreen)) => Ok(Request { size: None, fullscreen }),
        }
    }
}

/// The window's shape: `host.window.resize`'s result.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WindowState {
    /// Inner width in physical pixels.
    pub width: u32,
    /// Inner height in physical pixels.
    pub height: u32,
    /// The window is fullscreen.
    pub fullscreen: bool,
    /// Physical pixels per logical point (2 on a Retina display).
    pub scale: f64,
}

impl WindowState {
    /// Whether the window has the shape `r` asks for.
    pub fn matches(&self, r: &Request) -> bool {
        self.fullscreen == r.fullscreen && r.size.is_none_or(|(w, h)| (self.width, self.height) == (w, h))
    }
}

type Waker = Box<dyn Fn() + Send>;

#[derive(Default)]
struct State {
    /// A window shell is attached, and how to wake it.
    shell: Option<Waker>,
    /// The thread it runs on: a call there cannot wait for it.
    shell_thread: Option<ThreadId>,
    /// A request the shell has yet to take, with its number.
    request: Option<(u64, Request)>,
    /// Requests asked for so far.
    asked: u64,
    /// The newest request the shell has taken.
    taken: u64,
    /// The window's state as last observed, and the newest request taken
    /// before it was.
    observed: Option<(u64, WindowState)>,
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

const NO_WINDOW: &str = "no window here: host.window.resize changes a game window; a headless \
                         game renders at the size its command line gives";

impl Hub {
    pub const fn new() -> Hub {
        Hub {
            state: Mutex::new(State {
                shell: None,
                shell_thread: None,
                request: None,
                asked: 0,
                taken: 0,
                observed: None,
            }),
            cv: Condvar::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    // ---- the window shell ----

    /// A window shell is here, on this thread; `wake` gets it to call
    /// [`take_request`](Self::take_request) soon. It must not block: it is
    /// called with the hub locked.
    pub fn attach(&self, wake: Waker) {
        let mut s = self.lock();
        s.shell = Some(wake);
        s.shell_thread = Some(std::thread::current().id());
        s.request = None;
        s.observed = None;
        self.cv.notify_all();
    }

    /// The shell is gone: a waiting call ends, later calls are refused.
    pub fn detach(&self) {
        let mut s = self.lock();
        s.shell = None;
        s.shell_thread = None;
        s.request = None;
        s.observed = None;
        self.cv.notify_all();
    }

    /// The request the agent asked for, once.
    pub fn take_request(&self) -> Option<Request> {
        let mut s = self.lock();
        let (n, r) = s.request.take()?;
        s.taken = n;
        Some(r)
    }

    /// The window's state now: after the shell applied a request, and on
    /// every resize.
    pub fn observe(&self, state: WindowState) {
        let mut s = self.lock();
        s.observed = Some((s.taken, state));
        self.cv.notify_all();
    }

    // ---- the method ----

    /// `host.window.resize`: asks the shell for the shape `params` give
    /// and waits up to `wait` for the window to take it. A shape the
    /// window does not take in time (one larger than the display, say) is
    /// `TIMEOUT` with the window's state as `data`.
    pub fn resize(&self, params: ResizeParams, wait: Duration) -> Result<WindowState, RpcError> {
        let request = params.request()?;
        let mut s = self.lock();
        if s.shell.is_none() {
            return Err(RpcError::new(code::UNAVAILABLE, NO_WINDOW));
        }
        if s.shell_thread == Some(std::thread::current().id()) {
            return Err(RpcError::new(
                code::UNAVAILABLE,
                "host.window.resize waits for the window, so its own thread cannot ask",
            ));
        }
        s.asked += 1;
        let n = s.asked;
        s.request = Some((n, request));
        if let Some(wake) = &s.shell {
            wake();
        }
        let deadline = Instant::now() + wait;
        loop {
            if s.shell.is_none() {
                return Err(RpcError::new(code::UNAVAILABLE, NO_WINDOW));
            }
            if let Some((seen, state)) = s.observed
                && seen >= n
                && state.matches(&request)
            {
                return Ok(state);
            }
            let now = Instant::now();
            if now >= deadline {
                // Not taken yet: the next call asks again.
                if s.request.is_some_and(|(m, _)| m == n) {
                    s.request = None;
                }
                let mut e = RpcError::new(code::TIMEOUT, "the window did not take that shape in time");
                e.data = s
                    .observed
                    .map(|(_, state)| serde_json::to_value(state).expect("serialises"));
                return Err(e);
            }
            s = self
                .cv
                .wait_timeout(s, deadline - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn leak() -> &'static Hub {
        Box::leak(Box::new(Hub::new()))
    }

    fn params(v: serde_json::Value) -> Result<Request, RpcError> {
        serde_json::from_value::<ResizeParams>(v).unwrap().request()
    }

    fn state(width: u32, height: u32, fullscreen: bool) -> WindowState {
        WindowState { width, height, fullscreen, scale: 2.0 }
    }

    #[test]
    fn the_params_make_one_request_or_say_why_not() {
        use serde_json::json;
        assert_eq!(
            params(json!({"width": 1920, "height": 1080})).unwrap(),
            Request { size: Some((1920, 1080)), fullscreen: false }
        );
        assert_eq!(
            params(json!({"width": 800, "height": 600, "fullscreen": false})).unwrap(),
            Request { size: Some((800, 600)), fullscreen: false }
        );
        assert_eq!(params(json!({"fullscreen": true})).unwrap(), Request { size: None, fullscreen: true });
        for bad in [
            json!({}),
            json!({"width": 800}),
            json!({"height": 600}),
            json!({"width": 10, "height": 600}),
            json!({"width": 800, "height": 70000}),
            json!({"width": 800, "height": 600, "fullscreen": true}),
        ] {
            assert_eq!(params(bad.clone()).unwrap_err().code, code::INVALID_PARAMS, "{bad}");
        }
        assert!(serde_json::from_value::<ResizeParams>(json!({"w": 1})).is_err());
    }

    #[test]
    fn without_a_window_it_refuses() {
        let h = leak();
        let p = ResizeParams { fullscreen: Some(true), ..Default::default() };
        let e = h.resize(p, Duration::from_millis(10)).unwrap_err();
        assert_eq!(e.code, code::UNAVAILABLE);
        assert!(e.message.contains("headless"), "{e}");
    }

    /// A window shell on a thread of its own: attaches, reports `before`,
    /// then answers the first request with `answer`'s states.
    fn shell(
        h: &'static Hub,
        before: Option<WindowState>,
        answer: impl Fn(Request) -> Vec<WindowState> + Send + 'static,
    ) -> (std::thread::JoinHandle<()>, Arc<AtomicUsize>) {
        let wakes = Arc::new(AtomicUsize::new(0));
        let w = Arc::clone(&wakes);
        let (ready, attached) = std::sync::mpsc::channel();
        let t = std::thread::spawn(move || {
            h.attach(Box::new(move || {
                w.fetch_add(1, Ordering::SeqCst);
            }));
            if let Some(s) = before {
                h.observe(s);
            }
            ready.send(()).unwrap();
            loop {
                if let Some(r) = h.take_request() {
                    for s in answer(r) {
                        h.observe(s);
                    }
                    return;
                }
                std::thread::yield_now();
            }
        });
        attached.recv().unwrap();
        (t, wakes)
    }

    #[test]
    fn a_resize_answers_once_the_window_has_the_size() {
        let h = leak();
        // A state from before the request does not answer it, even one
        // that matches; the window takes a step to get there.
        let (t, wakes) = shell(h, Some(state(1920, 1080, false)), |r| {
            assert_eq!(r, Request { size: Some((1920, 1080)), fullscreen: false });
            vec![state(1600, 900, false), state(1920, 1080, false)]
        });
        let p = ResizeParams { width: Some(1920), height: Some(1080), fullscreen: None };
        assert_eq!(h.resize(p, Duration::from_secs(5)).unwrap(), state(1920, 1080, false));
        t.join().unwrap();
        assert_eq!(wakes.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn fullscreen_answers_with_the_displays_size() {
        let h = leak();
        let (t, _) = shell(h, None, |r| {
            assert_eq!(r, Request { size: None, fullscreen: true });
            vec![state(3024, 1964, true)]
        });
        let p = ResizeParams { fullscreen: Some(true), ..Default::default() };
        assert_eq!(h.resize(p, Duration::from_secs(5)).unwrap(), state(3024, 1964, true));
        t.join().unwrap();
    }

    #[test]
    fn a_shape_the_window_does_not_take_times_out_with_its_state() {
        let h = leak();
        // Clamped to the display.
        let (t, _) = shell(h, None, |_| vec![state(3024, 1890, false)]);
        let p = ResizeParams { width: Some(8000), height: Some(8000), fullscreen: None };
        let e = h.resize(p, Duration::from_millis(500)).unwrap_err();
        t.join().unwrap();
        assert_eq!(e.code, code::TIMEOUT);
        assert_eq!(e.data.unwrap()["width"], 3024);
    }

    #[test]
    fn the_shells_own_thread_cannot_wait_for_itself() {
        let h = leak();
        h.attach(Box::new(|| {}));
        let p = ResizeParams { fullscreen: Some(true), ..Default::default() };
        assert_eq!(h.resize(p, Duration::from_millis(10)).unwrap_err().code, code::UNAVAILABLE);
        assert!(h.take_request().is_none());
    }

    #[test]
    fn the_wire_shapes() {
        let v = serde_json::to_value(state(1920, 1080, true)).unwrap();
        assert_eq!(v, serde_json::json!({"width": 1920, "height": 1080, "fullscreen": true, "scale": 2.0}));
    }
}
