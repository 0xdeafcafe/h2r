//! Host capture and playback (docs/live/live-voice.md, 3.2): 20 ms mono
//! frames at 48 kHz. macOS implements these over cpal in slice 4;
//! [`FakeVoiceIo`] scripts the microphone and records the output.

use crate::HOST_FRAME;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, MutexGuard},
};

/// One 20 ms mono frame at 48 kHz.
pub type Frame = [i16; HOST_FRAME];

pub trait Capture: Send {
    /// The next 20 ms, after input volume. False when nothing was captured
    /// (the frame is then silence).
    fn read(&mut self, frame: &mut Frame) -> bool;
}

pub trait Playback: Send {
    /// One mixed 20 ms frame, before output volume.
    fn write(&mut self, frame: &Frame);
}

/// The host's voice devices. One microphone and one output for everyone on
/// the machine (section 6, decision 2).
pub trait VoiceIo: Send {
    fn capture(&mut self) -> &mut dyn Capture;
    fn playback(&mut self) -> &mut dyn Playback;
}

#[derive(Default)]
struct Script {
    input: VecDeque<Frame>,
    output: Vec<Frame>,
}

/// Scripted microphone frames in, recorded output frames out. Clones share
/// the script, so a test keeps one to feed and inspect.
#[derive(Clone, Default)]
pub struct FakeVoiceIo {
    script: Arc<Mutex<Script>>,
}

impl FakeVoiceIo {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue a frame for the microphone.
    pub fn speak(&self, frame: Frame) {
        self.lock().input.push_back(frame);
    }

    /// Frames the microphone has not delivered yet.
    pub fn pending(&self) -> usize {
        self.lock().input.len()
    }

    /// Everything played so far, oldest first; clears the record.
    pub fn take_played(&self) -> Vec<Frame> {
        std::mem::take(&mut self.lock().output)
    }

    fn lock(&self) -> MutexGuard<'_, Script> {
        self.script.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Capture for FakeVoiceIo {
    fn read(&mut self, frame: &mut Frame) -> bool {
        match self.lock().input.pop_front() {
            Some(f) => {
                *frame = f;
                true
            }
            None => {
                frame.fill(0);
                false
            }
        }
    }
}

impl Playback for FakeVoiceIo {
    fn write(&mut self, frame: &Frame) {
        self.lock().output.push(*frame);
    }
}

impl VoiceIo for FakeVoiceIo {
    fn capture(&mut self) -> &mut dyn Capture {
        self
    }
    fn playback(&mut self) -> &mut dyn Playback {
        self
    }
}

/// No devices: the microphone is silent and the output is dropped.
#[derive(Clone, Copy, Debug, Default)]
pub struct NullVoiceIo;

impl Capture for NullVoiceIo {
    fn read(&mut self, frame: &mut Frame) -> bool {
        frame.fill(0);
        false
    }
}

impl Playback for NullVoiceIo {
    fn write(&mut self, _frame: &Frame) {}
}

impl VoiceIo for NullVoiceIo {
    fn capture(&mut self) -> &mut dyn Capture {
        self
    }
    fn playback(&mut self) -> &mut dyn Playback {
        self
    }
}

/// Opens the host's voice devices: the microphone's host id (`None` for
/// the system default) and the output's (`None`: no separate output).
/// The host supplies it (boot's `Host::voice_io`), so boot itself needs no
/// device backend.
pub type OpenIo = Arc<dyn Fn(Option<&str>, Option<&str>) -> Box<dyn VoiceIo> + Send + Sync>;
