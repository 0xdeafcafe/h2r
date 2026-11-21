//! Paired editor presentation (docs/runtime/agent-api.md, "`editor`: a paired editor").
//! The agent owns the lease, and a window reads it on the UI thread.

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use agent_proto::editor::{
    FocusResult, LINK_TTL_MS, LinkParams, LinkResult, Mode, TEXT_MAX, TTL_MS_RANGE, method,
};
use agent_proto::{RpcError, code};
use serde_json::Value;

use crate::{Access, Agent};

const NO_WINDOW: &str = "this host has no window";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Badge {
    pub editor: String,
    pub mode: Mode,
    pub text: Option<String>,
    pub expires: Instant,
}

type Wake = Arc<dyn Fn() + Send + Sync>;

#[derive(Default)]
struct State {
    badge: Option<Arc<Badge>>,
    scene: scene::Scene,
    wake: Option<Wake>,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    focus_requested: AtomicBool,
    badge_held: AtomicBool,
    marks_generation: AtomicU64,
    mark_camera: AtomicU8,
    drawn_mark_generation: AtomicU64,
    mark_capture_disabled: AtomicU8,
    no_tick_observer: AtomicBool,
}

/// Shared by the agent and the window consuming its presentation state.
#[derive(Clone, Default)]
pub struct EditorLink(Arc<Shared>);

impl std::fmt::Debug for EditorLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EditorLink").finish_non_exhaustive()
    }
}

impl EditorLink {
    fn state(&self) -> MutexGuard<'_, State> {
        self.0.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn set(&self, params: LinkParams, now: Instant) -> Result<LinkResult, RpcError> {
        let Some((editor, mode, ttl_ms)) = check(&params)? else {
            self.clear();
            return Ok(LinkResult {
                shown: false,
                why: None,
                expires_ms: 0,
            });
        };
        let badge = Badge {
            editor: editor.to_owned(),
            mode,
            text: params.text,
            expires: now + Duration::from_millis(ttl_ms),
        };
        self.replace(Some(Arc::new(badge)));
        Ok(LinkResult {
            shown: true,
            why: None,
            expires_ms: ttl_ms,
        })
    }

    pub fn clear(&self) {
        self.replace(None);
    }

    fn replace(&self, badge: Option<Arc<Badge>>) {
        let wake = {
            let mut state = self.state();
            self.0.badge_held.store(badge.is_some(), Ordering::Release);
            state.badge = badge;
            state.wake.clone()
        };
        if let Some(wake) = wake {
            wake();
        }
    }

    pub fn current(&self, now: Instant) -> Option<Arc<Badge>> {
        if !self.0.badge_held.load(Ordering::Acquire) {
            return None;
        }
        let mut state = self.state();
        let badge = state.badge.as_ref()?;
        if now < badge.expires {
            return Some(badge.clone());
        }
        state.badge = None;
        self.0.badge_held.store(false, Ordering::Release);
        None
    }

    /// Wakes the window when the lease changes or a focus request arrives.
    pub fn set_waker(&self, wake: Arc<dyn Fn() + Send + Sync>) {
        self.state().wake = Some(wake);
    }

    fn request_focus(&self) {
        self.0.focus_requested.store(true, Ordering::Relaxed);
        let wake = self.state().wake.clone();
        if let Some(wake) = wake {
            wake();
        }
    }

    pub fn take_focus_request(&self) -> bool {
        self.0.focus_requested.swap(false, Ordering::Relaxed)
    }
}

fn invalid(message: impl Into<String>) -> RpcError {
    RpcError::new(code::INVALID_PARAMS, message)
}

/// A checked set request, or `None` for a clear.
fn check(params: &LinkParams) -> Result<Option<(&str, Mode, u64)>, RpcError> {
    if let Some(clear) = params.clear {
        if clear
            && params.editor.is_none()
            && params.mode.is_none()
            && params.text.is_none()
            && params.ttl_ms.is_none()
        {
            return Ok(None);
        }
        return Err(invalid("clear must be true and sent on its own"));
    }
    let editor = params
        .editor
        .as_deref()
        .ok_or_else(|| invalid("missing editor"))?;
    let mode = params.mode.ok_or_else(|| invalid("missing mode"))?;
    if params
        .text
        .as_ref()
        .is_some_and(|text| text.chars().count() > TEXT_MAX)
    {
        return Err(invalid(format!("text is limited to {TEXT_MAX} characters")));
    }
    let ttl_ms = params.ttl_ms.unwrap_or(LINK_TTL_MS);
    if !TTL_MS_RANGE.contains(&ttl_ms) {
        return Err(invalid(format!(
            "ttl_ms must be {} to {}",
            TTL_MS_RANGE.start(),
            TTL_MS_RANGE.end()
        )));
    }
    Ok(Some((editor, mode, ttl_ms)))
}

/// Adds the presentation methods even on a headless host.
pub fn install(agent: &Agent, window: Option<EditorLink>) {
    install_with_clock(agent, window, Instant::now);
}

fn install_with_clock(
    agent: &Agent,
    window: Option<EditorLink>,
    now: impl Fn() -> Instant + Send + 'static,
) {
    let link = window.clone();
    agent.add_method(method::LINK, Access::Read, move |params| {
        if !params.is_object() {
            return Err(invalid("editor.link params must be an object"));
        }
        let params = serde_json::from_value(params).map_err(|e| invalid(e.to_string()))?;
        let result = if let Some(link) = &link {
            link.set(params, now())?
        } else {
            check(&params)?;
            LinkResult {
                shown: false,
                why: Some(NO_WINDOW.into()),
                expires_ms: 0,
            }
        };
        Ok(serde_json::to_value(result).expect("link result is JSON"))
    });
    agent.add_method(method::FOCUS, Access::Act, move |params| {
        if !params.is_null() && !params.as_object().is_some_and(|p| p.is_empty()) {
            return Err(invalid("editor.focus takes no parameters"));
        }
        if let Some(link) = &window {
            link.request_focus();
        }
        Ok(serde_json::to_value(FocusResult {
            focused: window.is_some(),
            why: window.is_none().then(|| NO_WINDOW.into()),
        })
        .expect("focus result is JSON"))
    });
}

mod scene;
pub(crate) use scene::MarkObserver;
pub use scene::{MarkAvailability, MarkFrame, MarkShape, install_scene};

#[cfg(test)]
mod tests;
