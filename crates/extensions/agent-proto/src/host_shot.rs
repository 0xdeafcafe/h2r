//! `host.screenshot` (docs/runtime/agent-api.md, "Window screenshots"):
//! the window's newest frame written as a PNG, asked for over the agent
//! API.
//!
//! Like [`crate::host_pause`], a hub the window shell and the agent
//! method meet at, but simpler: the shell [`attach`](Hub::attach)es a
//! capture that writes the frame it last showed to the path asked for
//! (its default when none), and the method ([`Hub::shot`]) runs that
//! capture on the asking connection's own thread, so the in-process
//! debug menu's call works while the game is paused as `h2 agent`'s
//! does. This module has no game dependencies: the shell links it
//! without the agent.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use serde::{Deserialize, Serialize};

use crate::{RpcError, code};

pub mod method {
    /// Write the window's newest frame as a PNG ([`super::ShotParams`]).
    pub const SHOT: &str = "host.screenshot";
    pub const ALL: &[&str] = &[SHOT];
}

/// `host.screenshot`'s params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShotParams {
    /// The PNG to write; the default is the window's screenshots
    /// directory (`AppConfig::screenshots`), a timestamped name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
}

/// `host.screenshot`'s result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shot {
    /// The PNG written.
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
}

/// What the shell registers: write the newest shown frame, to `path`
/// or a default of its own, and say where it landed.
type Capture = Box<dyn Fn(Option<PathBuf>) -> Result<Shot, String> + Send + Sync>;

/// The meeting point. [`hub`] is the process's; tests make their own.
#[derive(Default)]
pub struct Hub {
    capture: Mutex<Option<Capture>>,
}

static HUB: Hub = Hub::new();

/// The process's hub.
pub fn hub() -> &'static Hub {
    &HUB
}

const NO_WINDOW: &str = "no window here: host.screenshot is the window's own frame; a headless \
                         game captures frames with render.capture";

impl Hub {
    pub const fn new() -> Hub {
        Hub {
            capture: Mutex::new(None),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Option<Capture>> {
        self.capture.lock().unwrap_or_else(|e| e.into_inner())
    }

    // ---- the window shell ----

    /// A window shell is here: it takes each shot with `capture`.
    pub fn attach(&self, capture: Capture) {
        *self.lock() = Some(capture);
    }

    /// The shell is gone: shots are refused.
    pub fn detach(&self) {
        *self.lock() = None;
    }

    // ---- the method ----

    /// `host.screenshot {path?}`: the capture runs on the calling thread,
    /// under the lock, so two shots never interleave a file.
    pub fn shot(&self, path: Option<PathBuf>) -> Result<Shot, RpcError> {
        let s = self.lock();
        let Some(capture) = s.as_ref() else {
            return Err(RpcError::new(code::UNAVAILABLE, NO_WINDOW));
        };
        capture(path).map_err(|e| RpcError::new(code::UNAVAILABLE, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leak() -> &'static Hub {
        Box::leak(Box::new(Hub::new()))
    }

    fn shot(path: &str) -> Shot {
        Shot {
            path: PathBuf::from(path),
            width: 1280,
            height: 720,
        }
    }

    #[test]
    fn no_window_is_unavailable() {
        let h = leak();
        let e = h.shot(None).unwrap_err();
        assert_eq!(e.code, code::UNAVAILABLE);
        assert!(e.message.contains("render.capture"), "{e}");
    }

    #[test]
    fn the_capture_writes_what_was_asked_or_its_default() {
        let h = leak();
        h.attach(Box::new(|path| {
            Ok(Shot {
                path: path.unwrap_or_else(|| "/shots/h2-1.png".into()),
                width: 1280,
                height: 720,
            })
        }));
        assert_eq!(h.shot(None).unwrap(), shot("/shots/h2-1.png"));
        assert_eq!(
            h.shot(Some(PathBuf::from("/tmp/menu.png"))).unwrap(),
            shot("/tmp/menu.png")
        );
        h.detach();
        assert_eq!(h.shot(None).unwrap_err().code, code::UNAVAILABLE);
    }

    #[test]
    fn a_failed_write_is_the_error() {
        let h = leak();
        h.attach(Box::new(|_| Err("disk full".into())));
        let e = h.shot(None).unwrap_err();
        assert_eq!(e.code, code::UNAVAILABLE);
        assert_eq!(e.message, "disk full");
    }

    #[test]
    fn the_wire_shapes() {
        let p: ShotParams = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(p.path, None);
        let p: ShotParams =
            serde_json::from_value(serde_json::json!({"path": "/tmp/x.png"})).unwrap();
        assert_eq!(p.path, Some(PathBuf::from("/tmp/x.png")));
        assert!(serde_json::from_value::<ShotParams>(serde_json::json!({"dir": "/tmp"})).is_err());
        let v = serde_json::to_value(shot("/tmp/x.png")).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"path": "/tmp/x.png", "width": 1280, "height": 720})
        );
    }
}
