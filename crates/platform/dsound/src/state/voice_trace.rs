//! The ordered DirectSound component of a full sim parity report.
//! Each event carries the output frame since the match began. Multiple
//! observations of a game tick retain all its events in dispatch order.

use std::fmt::{Arguments, Write};

use super::{BufferId, DirectSound, Mixer, Object};

#[derive(Default)]
pub(super) struct VoiceTrace {
    frame: u64,
    tick: Option<u32>,
    pending: String,
    current: String,
}

impl VoiceTrace {
    pub(super) fn event(&mut self, what: Arguments<'_>) {
        writeln!(&mut self.pending, "{} {what}", self.frame).expect("writing to a String");
    }

    fn sample(&mut self, tick: u32) -> String {
        if self.tick != Some(tick) {
            self.current.clear();
            self.tick = Some(tick);
        }
        self.current.push_str(&self.pending);
        self.pending.clear();
        self.current.clone()
    }
}

impl DirectSound {
    /// Start a fresh series for a full sim with arena verification enabled.
    pub fn begin_voice_trace(&mut self) {
        self.voice_trace = Some(VoiceTrace::default());
    }

    /// The dispatch sequence since the last sampled tick, including repeated
    /// observations of this tick. An empty string is a recorded, quiet tick.
    pub fn voice_events(&mut self, tick: u32) -> Option<String> {
        Some(self.voice_trace.as_mut()?.sample(tick))
    }

    pub(crate) fn voice_event(&mut self, what: Arguments<'_>) {
        if let Some(trace) = &mut self.voice_trace {
            trace.event(what);
        }
    }

    /// The mixer visits playing buffers by ID. A buffer that ends while it
    /// renders has no packet or callback to announce its completion.
    pub(super) fn playing_for_trace(&self, mixer: &Mixer) -> Vec<(BufferId, u32)> {
        if self.voice_trace.is_none() {
            return Vec::new();
        }
        let mut playing: Vec<_> = self.objects.iter().filter_map(|(&ptr, obj)| {
            let Object::Buffer(b) = obj else { return None };
            let id = b.audio?;
            mixer.buffer(id)?.is_playing().then_some((id, ptr))
        }).collect();
        playing.sort_unstable();
        playing
    }

    pub(super) fn trace_render(&mut self, mixer: &Mixer, playing: Vec<(BufferId, u32)>, frames: u64) {
        if let Some(trace) = &mut self.voice_trace {
            trace.frame += frames;
            for (id, ptr) in playing {
                if mixer.buffer(id).is_some_and(|b| !b.is_playing()) {
                    trace.event(format_args!("buffer-end {ptr:x}"));
                }
            }
        }
    }
}
