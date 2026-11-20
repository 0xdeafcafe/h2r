//! Process inbox for URLs delivered before or after the game creates its HLE.
//! The game thread owns all work after it claims a request.
use live_proto::JoinLink;
use serde::Serialize;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Request {
    pub id: u64,
    #[serde(flatten)]
    pub link: JoinLink,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum Phase {
    Queued,
    Resolving,
    SigningIn,
    Confirm {
        #[serde(skip_serializing_if = "Option::is_none")]
        host: Option<String>,
    },
    Joining,
    Joined,
    Cancelled,
    Failed {
        reason: String,
    },
}

impl Phase {
    fn finished(&self) -> bool {
        matches!(self, Self::Joined | Self::Cancelled | Self::Failed { .. })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Status {
    #[serde(flatten)]
    pub request: Request,
    #[serde(flatten)]
    pub phase: Phase,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SubmitError {
    #[error("Another Halo 2 join is in progress.")]
    Busy,
    #[error("Halo 2 session links are unavailable in this run.")]
    Unavailable,
}

#[derive(Default)]
struct Inbox {
    disabled: bool,
    last: Option<Status>,
}

impl Inbox {
    fn submit(&mut self, link: JoinLink) -> Result<u64, SubmitError> {
        if self.disabled {
            return Err(SubmitError::Unavailable);
        }
        if let Some(current) = &self.last
            && !current.phase.finished()
        {
            if current.request.link == link {
                return Ok(current.request.id);
            }
            return Err(SubmitError::Busy);
        }
        let id = self.last.as_ref().map_or(1, |v| v.request.id + 1);
        self.last = Some(Status {
            request: Request { id, link },
            phase: Phase::Queued,
        });
        Ok(id)
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.disabled = !enabled;
        if self.disabled
            && let Some(status) = &mut self.last
            && !status.phase.finished()
        {
            status.phase = Phase::Failed {
                reason: SubmitError::Unavailable.to_string(),
            };
        }
    }

    fn take_pending(&mut self) -> Option<Request> {
        let current = self.last.as_mut()?;
        if current.phase != Phase::Queued {
            return None;
        }
        current.phase = Phase::Resolving;
        Some(current.request.clone())
    }

    fn set_phase(&mut self, id: u64, phase: Phase) {
        if let Some(current) = &mut self.last
            && current.request.id == id
            && !current.phase.finished()
        {
            current.phase = phase;
        }
    }
}

// Read by the UI (and headless hosts) at every tick. Publish changes under
// the inbox lock, so a new OS delivery cannot race the consumer clearing it.
static PENDING: AtomicBool = AtomicBool::new(false);
static INBOX: Mutex<Inbox> = Mutex::new(Inbox {
    last: None,
    disabled: false,
});

fn inbox() -> std::sync::MutexGuard<'static, Inbox> {
    INBOX
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Duplicate OS deliveries of the active URL share the same request.
pub fn submit(link: JoinLink) -> Result<u64, SubmitError> {
    let mut inbox = inbox();
    let id = inbox.submit(link)?;
    if inbox
        .last
        .as_ref()
        .is_some_and(|status| status.phase == Phase::Queued)
    {
        PENDING.store(true, Ordering::Release);
    }
    Ok(id)
}

/// A cold URL can arrive before the boot selects its role and features.
pub fn set_enabled(enabled: bool) {
    let mut inbox = inbox();
    inbox.set_enabled(enabled);
    if !enabled {
        PENDING.store(false, Ordering::Release);
    }
}

pub fn take_pending() -> Option<Request> {
    if !PENDING.load(Ordering::Acquire) {
        return None;
    }
    let mut inbox = inbox();
    let request = inbox.take_pending();
    PENDING.store(false, Ordering::Release);
    request
}

pub fn set_phase(id: u64, phase: Phase) {
    inbox().set_phase(id, phase);
}

/// Includes the terminal outcome until another join is requested.
pub fn status() -> Option<Status> {
    inbox().last.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(id: &str) -> JoinLink {
        format!("h2recomp://join?server=https%3A%2F%2Flive.example&session={id}")
            .parse()
            .unwrap()
    }

    #[test]
    fn duplicate_delivery_and_competing_requests_preserve_the_prompt() {
        let mut q = Inbox::default();
        let a = link("8000000000000001");
        let b = link("8000000000000002");
        let first = q.submit(a.clone()).unwrap();
        assert_eq!(q.submit(a.clone()), Ok(first));
        assert_eq!(q.take_pending().unwrap().link, a);
        assert_eq!(q.take_pending(), None);
        let confirm = Phase::Confirm {
            host: Some("Example".into()),
        };
        q.set_phase(first, confirm.clone());
        assert!(q.submit(b.clone()).is_err());
        assert_eq!(q.last.as_ref().unwrap().phase, confirm);
        q.set_phase(first, Phase::Cancelled);
        let second = q.submit(b).unwrap();
        q.set_phase(first, Phase::Joined);
        assert_eq!(q.take_pending().unwrap().id, second);
        let fail = Phase::Failed {
            reason: "This session is full.".into(),
        };
        q.set_phase(second, fail.clone());
        q.set_phase(second, Phase::Joined);
        assert_eq!(q.last.unwrap().phase, fail);
    }

    #[test]
    fn an_unavailable_player_finishes_an_early_url_and_declines_later_ones() {
        let mut q = Inbox::default();
        let a = link("8000000000000001");
        q.submit(a.clone()).unwrap();
        q.set_enabled(false);
        assert!(matches!(
            q.last.as_ref().unwrap().phase,
            Phase::Failed { .. }
        ));
        assert_eq!(q.take_pending(), None);
        assert_eq!(q.submit(a.clone()), Err(SubmitError::Unavailable));
        q.set_enabled(true);
        assert!(q.submit(a).is_ok());
    }

    #[test]
    fn snapshot_carries_the_id_service_and_outcome_without_a_live_runtime() {
        let mut q = Inbox::default();
        let id = q.submit(link("8000000000000001")).unwrap();
        assert_eq!(
            serde_json::to_value(&q.last).unwrap(),
            serde_json::json!({
                "id": id, "server": "https://live.example", "session": "8000000000000001", "phase": "queued"
            })
        );
        q.take_pending().unwrap();
        q.set_phase(id, Phase::Joining);
        q.set_phase(id, Phase::Joined);
        assert_eq!(serde_json::to_value(&q.last).unwrap()["phase"], "joined");
    }
}
