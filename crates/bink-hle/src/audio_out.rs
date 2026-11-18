//! Where a movie's sound goes. The Bink player's own DirectSound back end
//! (`BinkSetSoundSystem`, `0x3e3c90`) never runs: `BinkOpen` asks the host
//! for a voice and the player feeds it PCM from the [`MovieStream`]. The
//! voice's play position is the movie's clock (`BinkWait`), as Bink slaves
//! its video to the sound.
//!
//! [`MixerAudio`] puts the voice in the `audio` crate's [`Mixer`], the same
//! one the DirectSound HLE renders, so movie sound goes out through the
//! game's audio output with no second device.
//!
//! [`MovieStream`]: crate::MovieStream
use crate::AudioInfo;
use audio::dsound::{PacketId, PacketStatus};
use audio::{Mixer, Stream, StreamId, WaveFormat};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// Opens voices for movie sound.
pub trait MovieAudioOut: Send + Sync {
    /// A voice for this format, or `None` (the movie then plays silent, on
    /// the wall clock).
    fn open(&self, info: AudioInfo) -> Option<Box<dyn MovieVoice>>;
}

/// One playing movie's sound. Dropping it stops and removes it.
pub trait MovieVoice: Send {
    /// Queue interleaved PCM, a whole number of frames. False if refused.
    fn queue(&mut self, pcm: &[i16]) -> bool;
    /// Frames the output has consumed since the voice opened or was last
    /// [flushed](Self::flush).
    fn played_frames(&mut self) -> u64;
    /// Drop everything queued and count from zero again (a loop restart).
    fn flush(&mut self);
}

/// Movie voices as streams in the shared [`Mixer`].
pub struct MixerAudio {
    mixer: Arc<Mutex<Mixer>>,
}

impl MixerAudio {
    pub fn new(mixer: Arc<Mutex<Mixer>>) -> Self {
        Self { mixer }
    }
}

impl MovieAudioOut for MixerAudio {
    fn open(&self, info: AudioInfo) -> Option<Box<dyn MovieVoice>> {
        let format = WaveFormat::pcm16(info.channels, info.sample_rate).ok()?;
        let stream = Stream::new(format).ok()?;
        let id = lock(&self.mixer).add_stream(stream);
        Some(Box::new(MixerVoice {
            mixer: Arc::clone(&self.mixer),
            id,
            channels: info.channels as usize,
            pending: VecDeque::new(),
            played: 0,
        }))
    }
}

fn lock(m: &Mutex<Mixer>) -> std::sync::MutexGuard<'_, Mixer> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

struct MixerVoice {
    mixer: Arc<Mutex<Mixer>>,
    id: StreamId,
    channels: usize,
    /// Submitted packets not yet reported complete, with their frames.
    pending: VecDeque<(PacketId, u64)>,
    played: u64,
}

impl MovieVoice for MixerVoice {
    fn queue(&mut self, pcm: &[i16]) -> bool {
        if pcm.is_empty() || !pcm.len().is_multiple_of(self.channels) {
            return false;
        }
        let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut m = lock(&self.mixer);
        let Some(s) = m.stream_mut(self.id) else {
            return false;
        };
        match s.submit(&bytes) {
            Ok(id) => {
                self.pending
                    .push_back((id, (pcm.len() / self.channels) as u64));
                true
            }
            Err(_) => false,
        }
    }
    fn played_frames(&mut self) -> u64 {
        let done = match lock(&self.mixer).stream_mut(self.id) {
            Some(s) => s.take_completed(),
            None => return self.played,
        };
        for (id, status) in done {
            if let Some(i) = self.pending.iter().position(|(p, _)| *p == id) {
                let (_, frames) = self.pending.remove(i).expect("found");
                if status == PacketStatus::Success {
                    self.played += frames;
                }
            }
        }
        self.played
    }
    fn flush(&mut self) {
        if let Some(s) = lock(&self.mixer).stream_mut(self.id) {
            s.flush();
            s.take_completed();
        }
        self.pending.clear();
        self.played = 0;
    }
}

impl Drop for MixerVoice {
    fn drop(&mut self) {
        lock(&self.mixer).remove_stream(self.id);
    }
}
