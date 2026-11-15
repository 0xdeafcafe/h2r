//! Deterministic, externally clocked input. Polling never advances playback.
use std::sync::{Arc, Mutex};

use serde::Deserialize;

use crate::HostBackend;
use crate::gamepad::Rumble;
use crate::host::{HostButton, HostPad, HostPadId, HostPadState};

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScriptButton {
    A,
    B,
    X,
    Y,
    Black,
    White,
    LeftTrigger,
    RightTrigger,
    DpadUp,
    DpadDown,
    DpadLeft,
    DpadRight,
    Start,
    Back,
    LeftThumb,
    RightThumb,
}

/// A complete replacement state, held until the next frame. Omitted controls
/// are released. Axis units match the Xbox report; +Y is up.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScriptFrame {
    /// Absolute guest time in 100 ns ticks (10,000 ticks = 1 ms).
    pub at_ticks: u64,
    #[serde(default = "connected")]
    pub connected: bool,
    #[serde(default)]
    pub buttons: Vec<ScriptButton>,
    /// Optional exact pressures A/B/X/Y/Black/White/LT/RT; overrides buttons.
    pub analog: Option<[u8; 8]>,
    #[serde(default)]
    pub left_stick: [i16; 2],
    #[serde(default)]
    pub right_stick: [i16; 2],
}

fn connected() -> bool {
    true
}

impl ScriptFrame {
    fn state(&self) -> HostPadState {
        let mut state = HostPadState::default();
        let mut analog = [0u8; 8];
        for button in &self.buttons {
            use ScriptButton::*;
            let digital = match button {
                A => {
                    analog[0] = 255;
                    None
                }
                B => {
                    analog[1] = 255;
                    None
                }
                X => {
                    analog[2] = 255;
                    None
                }
                Y => {
                    analog[3] = 255;
                    None
                }
                Black => {
                    analog[4] = 255;
                    None
                }
                White => {
                    analog[5] = 255;
                    None
                }
                LeftTrigger => {
                    analog[6] = 255;
                    None
                }
                RightTrigger => {
                    analog[7] = 255;
                    None
                }
                DpadUp => Some(HostButton::DpadUp),
                DpadDown => Some(HostButton::DpadDown),
                DpadLeft => Some(HostButton::DpadLeft),
                DpadRight => Some(HostButton::DpadRight),
                Start => Some(HostButton::Start),
                Back => Some(HostButton::Back),
                LeftThumb => Some(HostButton::LeftStick),
                RightThumb => Some(HostButton::RightStick),
            };
            if let Some(button) = digital {
                state.buttons.set(button, true);
            }
        }
        let analog = self.analog.unwrap_or(analog).map(|v| f32::from(v) / 255.0);
        state.analog_buttons = Some(analog[..6].try_into().expect("six pressures"));
        state.left_trigger = analog[6];
        state.right_trigger = analog[7];
        let axis = |v: i16| f32::from(v) / if v < 0 { 32768.0 } else { 32767.0 };
        state.left_stick = (axis(self.left_stick[0]), axis(self.left_stick[1]));
        state.right_stick = (axis(self.right_stick[0]), axis(self.right_stick[1]));
        state.prefiltered = true;
        state
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ScriptError {
    #[error("invalid input timeline: {0}")]
    Toml(String),
    #[error("timeline must contain frames in strictly increasing at_ticks order")]
    Order,
    #[error("input clock cannot move backwards; create a new playback to restart")]
    Rewind,
}

/// Validated, reusable timeline. Create a separate playback for each run.
#[derive(Clone, Debug)]
pub struct Timeline(Arc<[ScriptFrame]>);

impl Timeline {
    pub fn new(frames: Vec<ScriptFrame>) -> Result<Self, ScriptError> {
        if frames.is_empty() || frames.windows(2).any(|w| w[0].at_ticks >= w[1].at_ticks) {
            return Err(ScriptError::Order);
        }
        Ok(Self(frames.into()))
    }

    pub fn from_toml(text: &str) -> Result<Self, ScriptError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct File {
            frame: Vec<ScriptFrame>,
        }
        let file: File = toml::from_str(text).map_err(|e| ScriptError::Toml(e.to_string()))?;
        Self::new(file.frame)
    }

    pub fn playback(&self) -> ScriptedInput {
        let input = ScriptedInput {
            timeline: self.clone(),
            shared: Arc::new(Mutex::new(Playback {
                ticks: 0,
                next: 0,
                connected: true,
                generation: 0,
                state: HostPadState::default(),
                rumble: Rumble::default(),
            })),
        };
        input.advance_to(0).expect("initial clock");
        input
    }
}

#[derive(Debug)]
struct Playback {
    ticks: u64,
    next: usize,
    connected: bool,
    generation: u64,
    state: HostPadState,
    rumble: Rumble,
}

/// Clone one handle into InputManager and retain another in the runner.
/// advance_to only selects state; sync_input/pump_input publishes it.
#[derive(Clone, Debug)]
pub struct ScriptedInput {
    timeline: Timeline,
    shared: Arc<Mutex<Playback>>,
}

impl ScriptedInput {
    /// Next state boundary, so a runner can avoid stepping over a short press.
    pub fn next_transition_ticks(&self) -> Option<u64> {
        let p = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        self.timeline.0.get(p.next).map(|frame| frame.at_ticks)
    }

    pub fn advance_to(&self, ticks: u64) -> Result<(), ScriptError> {
        let mut p = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        if ticks < p.ticks {
            return Err(ScriptError::Rewind);
        }
        while let Some(frame) = self.timeline.0.get(p.next).filter(|f| f.at_ticks <= ticks) {
            if frame.connected && !p.connected {
                p.generation = p.generation.wrapping_add(1);
            }
            if !frame.connected {
                p.rumble = Rumble::default();
            }
            p.connected = frame.connected;
            p.state = frame.state();
            p.next += 1;
        }
        p.ticks = ticks;
        Ok(())
    }

    pub fn rumble(&self) -> Rumble {
        self.shared.lock().unwrap_or_else(|e| e.into_inner()).rumble
    }
}

impl HostBackend for ScriptedInput {
    fn name(&self) -> &str {
        "scripted"
    }

    fn poll(&mut self) -> Vec<HostPad> {
        let p = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        if !p.connected {
            return vec![];
        }
        vec![HostPad {
            id: HostPadId(p.generation),
            name: "scripted".into(),
            state: p.state,
            physical: false,
            supports_rumble: true,
        }]
    }

    fn set_rumble(&mut self, id: HostPadId, rumble: Rumble) {
        let mut p = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        if p.connected && id.0 == p.generation {
            p.rumble = rumble;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_ambiguous_timeline_and_unknown_controls() {
        for text in [
            "frame = []",
            "[[frame]]\nat_ticks=2\n[[frame]]\nat_ticks=1",
            "[[frame]]\nat_ticks=0\n[[frame]]\nat_ticks=0",
            "[[frame]]\nat_ticks=0\nbuttons=['jump']",
            "[[frame]]\nat_ticks=0\nleft_stick=[32768,0]",
            "[[frame]]\nat_ticks=0\nbutton=['a']",
        ] {
            assert!(Timeline::from_toml(text).is_err(), "{text}");
        }
    }

    #[test]
    fn polling_is_idempotent_and_clock_is_monotonic() {
        let timeline =
            Timeline::from_toml("[[frame]]\nat_ticks=10\nbuttons=['a']\n[[frame]]\nat_ticks=20")
                .unwrap();
        let mut input = timeline.playback();
        assert_eq!(input.next_transition_ticks(), Some(10));
        assert_eq!(input.poll()[0].state, HostPadState::default());
        input.advance_to(10).unwrap();
        assert_eq!(input.next_transition_ticks(), Some(20));
        let held = input.poll()[0].state;
        assert_eq!(input.poll()[0].state, held);
        assert_eq!(timeline.playback().poll()[0].state, HostPadState::default());
        assert!(matches!(input.advance_to(9), Err(ScriptError::Rewind)));
        assert_eq!(input.poll()[0].state, held);
        input.advance_to(20).unwrap();
        assert_eq!(input.next_transition_ticks(), None);
        assert_eq!(input.poll()[0].state.analog_buttons, Some([0.0; 6]));
    }

    #[test]
    fn skipped_disconnect_reconnect_changes_identity() {
        let mut input =
            Timeline::from_toml("[[frame]]\nat_ticks=10\nconnected=false\n[[frame]]\nat_ticks=20")
                .unwrap()
                .playback();
        let id = input.poll()[0].id;
        input.advance_to(30).unwrap();
        assert_ne!(input.poll()[0].id, id);
    }
}
