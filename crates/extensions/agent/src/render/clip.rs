//! A bounded capture across guest ticks. The render thread writes the guest
//! present and each interpolated present in order.
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use agent_proto::{RpcError, code, method};
use serde::{Deserialize, Serialize};

use super::HOSTED;
use crate::{Access, Agent};

pub static CLIPS: ClipCapture = ClipCapture::new();

pub struct ClipCapture {
    active: AtomicBool,
    state: Mutex<State>,
}

struct State {
    output: Option<(PathBuf, u32)>,
    id: u64,
    clip: Option<Clip>,
}

struct Clip {
    first_tick: u64,
    ticks: u64,
    slots: u32,
    first_present: Option<u64>,
    bound_ticks: u64,
    written: u64,
    path: PathBuf,
    error: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Start {
    tick: u64,
    ticks: u64,
    fps: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Status {
    id: u64,
}

#[derive(Serialize)]
struct Progress {
    id: u64,
    path: PathBuf,
    frames: u64,
    written: u64,
}

impl Clip {
    fn progress(&self, id: u64) -> Progress {
        Progress {
            id,
            path: self.path.clone(),
            frames: self.ticks * u64::from(self.slots),
            written: self.written,
        }
    }

    fn contains(&self, present: u64) -> bool {
        self.first_present
            .is_some_and(|first| present >= first && present - first < self.bound_ticks)
    }
}

impl ClipCapture {
    const fn new() -> Self {
        Self {
            active: AtomicBool::new(false),
            state: Mutex::new(State {
                output: None,
                id: 0,
                clip: None,
            }),
        }
    }

    pub fn set_writer(&self, dir: PathBuf, max_slots: u32) {
        self.state.lock().unwrap().output = Some((dir, max_slots));
    }

    fn start(&self, p: Start, last_tick: Option<u64>) -> Result<Progress, RpcError> {
        let invalid = |message| RpcError::new(code::INVALID_PARAMS, message);
        let slots = match p.fps {
            30 | 60 | 120 => p.fps / 30,
            _ => return Err(invalid("fps must be 30, 60 or 120")),
        };
        if !(1..=300).contains(&p.ticks) || p.tick.checked_add(p.ticks).is_none() {
            return Err(invalid("ticks must be between 1 and 300"));
        }
        let Some(last) = last_tick else {
            return Err(RpcError::new(
                code::UNAVAILABLE,
                "no game tick has ended yet",
            ));
        };
        if p.tick <= last || p.tick - last > 240 {
            return Err(invalid("tick must be 1 to 240 ticks ahead"));
        }
        let mut state = self.state.lock().unwrap();
        let Some((dir, max_slots)) = &state.output else {
            return Err(RpcError::new(
                code::UNAVAILABLE,
                "clip capture needs headless GPU rendering",
            ));
        };
        if slots > *max_slots {
            return Err(RpcError::new(
                code::UNAVAILABLE,
                "clip capture above 30 fps needs interpolated replay on the render thread and H2_FPS at least as high as fps",
            ));
        }
        if self.active.load(Ordering::Relaxed) {
            return Err(RpcError::new(
                code::BUSY,
                "a clip capture is already in progress",
            ));
        }
        let mut id = state.id + 1;
        let path = loop {
            let path = dir.join(format!("clip-{id}"));
            match std::fs::create_dir(&path) {
                Ok(()) => break path,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => id += 1,
                Err(e) => {
                    return Err(RpcError::new(
                        code::UNAVAILABLE,
                        format!("{}: {e}", path.display()),
                    ));
                }
            }
        };
        let clip = Clip {
            first_tick: p.tick,
            ticks: p.ticks,
            slots,
            first_present: None,
            bound_ticks: 0,
            written: 0,
            path,
            error: None,
        };
        let progress = clip.progress(id);
        state.id = id;
        state.clip = Some(clip);
        self.active.store(true, Ordering::Release);
        Ok(progress)
    }

    fn status(&self, id: u64) -> Result<Progress, RpcError> {
        let state = self.state.lock().unwrap();
        let clip = state
            .clip
            .as_ref()
            .filter(|_| state.id == id)
            .ok_or_else(|| RpcError::new(code::INVALID_PARAMS, "unknown clip capture id"))?;
        if let Some(error) = &clip.error {
            return Err(RpcError::new(code::UNAVAILABLE, error.clone()));
        }
        Ok(clip.progress(id))
    }

    /// Called before the guest present is sent to the render thread.
    pub fn present(&self, present: u64) {
        if self.active.load(Ordering::Acquire) {
            self.bind(present, HOSTED.last_tick());
        }
    }

    fn bind(&self, present: u64, tick: Option<u64>) {
        let Some(tick) = tick else { return };
        let mut state = self.state.lock().unwrap();
        let Some(clip) = state.clip.as_mut() else {
            return;
        };
        if clip.error.is_some() || clip.bound_ticks == clip.ticks || tick < clip.first_tick {
            return;
        }
        let expected = clip.first_tick + clip.bound_ticks;
        if tick != expected {
            clip.error = Some(format!(
                "expected one guest present per tick, got tick {tick} at present {present} (expected tick {expected})"
            ));
            self.active.store(false, Ordering::Release);
            return;
        }
        clip.first_present.get_or_insert(present);
        clip.bound_ticks += 1;
    }

    /// The number of evenly spaced display samples for this guest present.
    /// Zero leaves the ordinary display clock in charge.
    pub fn slots(&self, present: u64) -> u32 {
        if !self.active.load(Ordering::Acquire) {
            return 0;
        }
        let state = self.state.lock().unwrap();
        state
            .clip
            .as_ref()
            .filter(|clip| clip.contains(present) && clip.error.is_none())
            .map_or(0, |clip| clip.slots)
    }

    /// Only the display hooks call this, in order on the render thread.
    /// A later guest present exposes any missing interpolated slots.
    pub fn write(&self, present: u64, slot: u32, save: impl FnOnce(&Path) -> Result<(), String>) {
        if !self.active.load(Ordering::Acquire) {
            return;
        }
        let (id, path) = {
            let mut state = self.state.lock().unwrap();
            let id = state.id;
            let Some(clip) = state.clip.as_mut() else {
                return;
            };
            let Some(first) = clip.first_present else {
                return;
            };
            if clip.error.is_some() || present < first || slot >= clip.slots {
                return;
            }
            let index = (present - first) * u64::from(clip.slots) + u64::from(slot);
            if index != clip.written {
                clip.error = Some(format!(
                    "present {present} slot {slot} arrived after only {} of {index} display frames were written; see render.stats for interpolation holds",
                    clip.written
                ));
                self.active.store(false, Ordering::Release);
                return;
            }
            if !clip.contains(present) {
                return;
            }
            (id, clip.path.join(format!("frame-{index:05}.png")))
        };

        let result = save(&path);
        let mut state = self.state.lock().unwrap();
        if state.id != id {
            return;
        }
        let clip = state.clip.as_mut().unwrap();
        if let Err(error) = result {
            clip.error
                .get_or_insert_with(|| format!("{}: {error}", path.display()));
        } else {
            clip.written += 1;
        }
        if clip.error.is_some() || clip.written == clip.ticks * u64::from(clip.slots) {
            self.active.store(false, Ordering::Release);
        }
    }
}

pub fn add_methods(agent: &Agent) {
    let invalid = |e: serde_json::Error| RpcError::new(code::INVALID_PARAMS, e.to_string());
    agent.add_game_method(method::RENDER_CAPTURE_CLIP, Access::Act, move |v, _| {
        let params = serde_json::from_value(v).map_err(invalid)?;
        let progress = CLIPS.start(params, HOSTED.last_tick())?;
        serde_json::to_value(progress).map_err(invalid)
    });
    agent.add_method(method::RENDER_CAPTURE_CLIP_STATUS, Access::Read, move |v| {
        let params: Status = serde_json::from_value(v).map_err(invalid)?;
        serde_json::to_value(CLIPS.status(params.id)?).map_err(invalid)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capture_in(name: &str) -> (PathBuf, ClipCapture) {
        let dir = std::env::temp_dir().join(format!("h2-clip-{name}-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let capture = ClipCapture::new();
        capture.set_writer(dir.clone(), 4);
        (dir, capture)
    }

    #[test]
    fn a_clip_writes_each_slot_in_order_and_ignores_other_presents() {
        let (dir, capture) = capture_in("order");
        let p = capture
            .start(
                Start {
                    tick: 20,
                    ticks: 2,
                    fps: 120,
                },
                Some(10),
            )
            .unwrap();
        capture.bind(80, Some(19));
        assert_eq!(capture.slots(80), 0);
        capture.write(80, 0, |_| panic!("before the clip"));
        capture.bind(81, Some(20));
        capture.bind(82, Some(21));
        assert_eq!(capture.slots(81), 4);
        assert_eq!(capture.slots(82), 4);
        for present in 81..=82 {
            for slot in 0..4 {
                capture.write(present, slot, |path| {
                    let index = (present - 81) * 4 + u64::from(slot);
                    assert_eq!(path, p.path.join(format!("frame-{index:05}.png")));
                    Ok(())
                });
            }
        }
        capture.write(83, 0, |_| panic!("after the clip"));
        let done = capture.status(p.id).unwrap();
        assert_eq!((done.written, done.frames), (8, 8));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_held_frame_fails_the_capture_instead_of_repeating_the_image() {
        let (dir, capture) = capture_in("held");
        let p = capture
            .start(
                Start {
                    tick: 20,
                    ticks: 1,
                    fps: 120,
                },
                Some(10),
            )
            .unwrap();
        capture.bind(81, Some(20));
        capture.write(81, 0, |_| Ok(()));
        capture.bind(82, Some(21));
        capture.write(82, 0, |_| panic!("clip missed slots"));
        assert!(
            capture
                .status(p.id)
                .err()
                .unwrap()
                .message
                .contains("interpolation holds")
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
