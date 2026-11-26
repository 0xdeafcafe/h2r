//! The host mixer as a sound [`Host`]: a started sound becomes a mixer
//! buffer, a stopped one is removed, an ended one is left to play out and is
//! swept on a later event. The feed reads the events and never the sim, so
//! the sim's hash is the same with the mixer, with [`sim_sound::feed::NullHost`]
//! or with nothing attached (docs/engine/sound-sim.md, "The host feed").

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use sim_sound::events::{SoundEvent, SoundId};
use sim_sound::feed::Host;

/// The PCM a sound tag plays. The host decodes it (the tag's permutation
/// choice is the host's too: it is presentation).
pub trait Clips: Send {
    fn clip(&mut self, tag: u32) -> Option<(audio::WaveFormat, Vec<u8>)>;
}

pub struct MixerHost<C: Clips> {
    mixer: Arc<Mutex<audio::Mixer>>,
    clips: C,
    buffers: BTreeMap<SoundId, audio::BufferId>,
}

impl<C: Clips> MixerHost<C> {
    pub fn new(mixer: Arc<Mutex<audio::Mixer>>, clips: C) -> Self {
        MixerHost { mixer, clips, buffers: BTreeMap::new() }
    }

    /// Mixer buffers still held for sounds the sim started.
    pub fn held(&self) -> usize {
        self.buffers.len()
    }

    fn mixer(&self) -> MutexGuard<'_, audio::Mixer> {
        self.mixer.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Drops the buffers of sounds that have played out.
    fn sweep(&mut self) {
        let mut mixer = self.mixer.lock().unwrap_or_else(|e| e.into_inner());
        self.buffers.retain(|_, id| match mixer.buffer(*id) {
            Some(b) if b.is_playing() => true,
            Some(_) => {
                mixer.remove_buffer(*id);
                false
            }
            None => false,
        });
    }
}

impl<C: Clips> Host for MixerHost<C> {
    fn event(&mut self, event: &SoundEvent) {
        match *event {
            SoundEvent::Started { id, tag, looping, .. } => {
                self.sweep();
                let Some((format, data)) = self.clips.clip(tag) else { return };
                let Ok(mut buffer) = audio::SoundBuffer::from_data(format, data) else { return };
                buffer.play(looping);
                let buffer = self.mixer().add_buffer(buffer);
                self.buffers.insert(id, buffer);
            }
            SoundEvent::Stopped { id, .. } => {
                if let Some(buffer) = self.buffers.remove(&id) {
                    self.mixer().remove_buffer(buffer);
                }
            }
            // The clip plays to its own end; the sweep frees the buffer.
            SoundEvent::Ended { .. } => {}
        }
    }
}
