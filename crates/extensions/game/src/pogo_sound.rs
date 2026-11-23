//! The Pogo Stick's sounds (docs/game/pogo-stick.md, "Sounds"): the
//! piston going in, springing out and easing out, for the local players.
//!
//! The clips are ours (`art/pogo/*.wav`, synthesised by
//! `scripts/pogo-sounds.py`), embedded and played as plain 2D host
//! buffers through DirectSound's mixer, as the medals' announcer lines
//! are ([`crate::medal_announcer`]): no game sound tag, no position, and
//! nothing a remote player hears. `superbounce_mode` hands [`play`] the
//! cues of each tick ([`crate::pogo_ride::on_tick`]).
//!
//! A cue starts its clip at once, over whatever is still sounding, up to
//! [`VOICES`] at a time: a touchdown's clank and the bounce's spring three
//! ticks later overlap. Without audio (a headless run) cues are dropped.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::medal_announcer::parse_wav;
use crate::pogo::Cue;

/// Each cue's clip: the file `art/pogo/<id>.wav`.
pub const CLIPS: [(Cue, &str, &[u8]); 3] = [
    (
        Cue::Compress,
        "compress",
        include_bytes!("../../../../art/pogo/compress.wav"),
    ),
    (
        Cue::Spring,
        "spring",
        include_bytes!("../../../../art/pogo/spring.wav"),
    ),
    (
        Cue::Settle,
        "settle",
        include_bytes!("../../../../art/pogo/settle.wav"),
    ),
];

/// Clips sounding at once at most; one more stops the oldest.
pub const VOICES: usize = 3;

/// The clip id of `cue`.
pub fn clip_id(cue: Cue) -> &'static str {
    CLIPS
        .iter()
        .find(|(c, ..)| *c == cue)
        .map(|(_, id, _)| *id)
        .expect("every cue has a clip")
}

fn clip_bytes(cue: Cue) -> &'static [u8] {
    CLIPS
        .iter()
        .find(|(c, ..)| *c == cue)
        .map(|(.., b)| *b)
        .expect("every cue has a clip")
}

/// What plays the clips: the mixer in the game, a fake in the tests.
pub trait Voice {
    /// Starts `cue`'s clip and gives its handle; `None` when it cannot
    /// be played.
    fn start(&mut self, cue: Cue) -> Option<u32>;
    /// Whether the clip `handle` names is still sounding.
    fn sounding(&mut self, handle: u32) -> bool;
    /// Stops and forgets the clip.
    fn stop(&mut self, handle: u32);
}

/// The clips sounding, oldest first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Voices {
    sounding: Vec<u32>,
}

impl Voices {
    pub const fn new() -> Voices {
        Voices {
            sounding: Vec::new(),
        }
    }

    /// How many clips are held as sounding.
    pub fn len(&self) -> usize {
        self.sounding.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sounding.is_empty()
    }

    /// Starts each of `cues`, after forgetting the clips that have ended;
    /// past [`VOICES`] the oldest gives way.
    pub fn play(&mut self, voice: &mut dyn Voice, cues: &[Cue]) {
        if cues.is_empty() && self.sounding.is_empty() {
            return;
        }
        self.sounding.retain(|h| {
            let on = voice.sounding(*h);
            if !on {
                voice.stop(*h);
            }
            on
        });
        for cue in cues {
            while self.sounding.len() >= VOICES {
                voice.stop(self.sounding.remove(0));
            }
            if let Some(h) = voice.start(*cue) {
                self.sounding.push(h);
            }
        }
    }

    /// Stops everything.
    pub fn silence(&mut self, voice: &mut dyn Voice) {
        for h in self.sounding.drain(..) {
            voice.stop(h);
        }
    }
}

/// The mixer's voice.
pub struct MixerVoice {
    mixer: Arc<Mutex<audio::Mixer>>,
    /// Each clip decoded once; `None` for one that did not read.
    clips: BTreeMap<&'static str, Option<(audio::WaveFormat, Vec<u8>)>>,
    /// The buffers started, by the handle given for each.
    buffers: BTreeMap<u32, audio::BufferId>,
    next: u32,
}

impl MixerVoice {
    pub fn new(mixer: Arc<Mutex<audio::Mixer>>) -> MixerVoice {
        MixerVoice {
            mixer,
            clips: BTreeMap::new(),
            buffers: BTreeMap::new(),
            next: 0,
        }
    }

    fn clip(&mut self, cue: Cue) -> Option<(audio::WaveFormat, Vec<u8>)> {
        let id = clip_id(cue);
        self.clips
            .entry(id)
            .or_insert_with(|| {
                parse_wav(clip_bytes(cue))
                    .map_err(|e| e.to_string())
                    .and_then(|c| {
                        audio::WaveFormat::pcm16(c.channels, c.rate)
                            .map(|f| (f, c.data))
                            .map_err(|e| e.to_string())
                    })
                    .map_err(|e| eprintln!("pogo: art/pogo/{id}.wav: {e}; it stays silent"))
                    .ok()
            })
            .clone()
    }
}

impl Voice for MixerVoice {
    fn start(&mut self, cue: Cue) -> Option<u32> {
        let (format, data) = self.clip(cue)?;
        let mut buffer = match audio::SoundBuffer::from_data(format, data) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("pogo: art/pogo/{}.wav: {e}", clip_id(cue));
                return None;
            }
        };
        buffer.play(false);
        let id = lock(&self.mixer).add_buffer(buffer);
        self.next = self.next.wrapping_add(1);
        self.buffers.insert(self.next, id);
        Some(self.next)
    }

    fn sounding(&mut self, handle: u32) -> bool {
        // A buffer the mixer no longer has (a snapshot load) has ended.
        self.buffers.get(&handle).is_some_and(|id| {
            lock(&self.mixer)
                .buffer(*id)
                .is_some_and(|b| b.is_playing())
        })
    }

    fn stop(&mut self, handle: u32) {
        if let Some(id) = self.buffers.remove(&handle) {
            lock(&self.mixer).remove_buffer(id);
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// On once the `pogo-view` feature is built.
static ON: AtomicBool = AtomicBool::new(false);
/// The mixer's voice; `None` without audio, where cues are dropped.
static VOICE: Mutex<Option<MixerVoice>> = Mutex::new(None);
static VOICES_NOW: Mutex<Voices> = Mutex::new(Voices::new());

/// Gives the sounds DirectSound's mixer to play into (boot, once the
/// audio is up).
pub fn connect(mixer: Arc<Mutex<audio::Mixer>>) {
    *lock(&VOICE) = Some(MixerVoice::new(mixer));
}

/// Turns the sounds on: the `pogo-view` feature was built.
pub fn enable() {
    ON.store(true, Ordering::Relaxed);
}

/// The cues of a tick, from the superbounce mode. Called every tick
/// while the mode is on, with or without cues, so ended clips leave the
/// mixer.
pub fn play(cues: &[Cue]) {
    if !ON.load(Ordering::Relaxed) {
        return;
    }
    let mut voice = lock(&VOICE);
    let Some(v) = voice.as_mut() else {
        return;
    };
    lock(&VOICES_NOW).play(v, cues);
}

/// Stops the clips: the game ended.
pub fn reset() {
    let mut voice = lock(&VOICE);
    if let Some(v) = voice.as_mut() {
        lock(&VOICES_NOW).silence(v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A voice that counts: each clip sounds until it is ended by hand.
    #[derive(Default)]
    struct Fake {
        started: Vec<Cue>,
        live: Vec<u32>,
        stopped: Vec<u32>,
        broken: bool,
    }

    impl Voice for Fake {
        fn start(&mut self, cue: Cue) -> Option<u32> {
            if self.broken {
                return None;
            }
            self.started.push(cue);
            let h = self.started.len() as u32;
            self.live.push(h);
            Some(h)
        }
        fn sounding(&mut self, handle: u32) -> bool {
            self.live.contains(&handle)
        }
        fn stop(&mut self, handle: u32) {
            self.live.retain(|h| *h != handle);
            self.stopped.push(handle);
        }
    }

    #[test]
    fn every_cue_has_a_clip_that_reads_or_is_an_lfs_pointer() {
        for cue in [Cue::Compress, Cue::Spring, Cue::Settle] {
            let id = clip_id(cue);
            assert!(!id.is_empty());
            // A checkout without LFS holds pointers; the game logs that
            // and stays silent.
            match parse_wav(clip_bytes(cue)) {
                Ok(c) => {
                    assert_eq!((c.channels, c.rate), (1, 44_100), "{id}");
                    let seconds = c.data.len() as f32 / 2.0 / c.rate as f32;
                    assert!((0.1..0.6).contains(&seconds), "{id}: {seconds} s");
                    // Not silence, and not clipped.
                    let peak = c
                        .data
                        .chunks_exact(2)
                        .map(|s| i16::from_le_bytes([s[0], s[1]]).unsigned_abs())
                        .max()
                        .unwrap();
                    assert!((8_000..32_000).contains(&peak), "{id}: peak {peak}");
                }
                Err(e) => assert!(
                    clip_bytes(cue).starts_with(b"version https://git-lfs"),
                    "{id}: {e}"
                ),
            }
        }
        let mut ids: Vec<_> = CLIPS.iter().map(|(_, id, _)| *id).collect();
        ids.dedup();
        assert_eq!(ids.len(), 3);
    }

    #[test]
    fn a_cue_starts_its_clip_over_the_one_still_sounding() {
        let mut fake = Fake::default();
        let mut voices = Voices::new();
        voices.play(&mut fake, &[Cue::Compress]);
        voices.play(&mut fake, &[]);
        voices.play(&mut fake, &[Cue::Spring]);
        assert_eq!(fake.started, [Cue::Compress, Cue::Spring]);
        assert_eq!(voices.len(), 2);
        assert!(fake.stopped.is_empty());
    }

    #[test]
    fn ended_clips_are_forgotten_and_leave_the_mixer() {
        let mut fake = Fake::default();
        let mut voices = Voices::new();
        voices.play(&mut fake, &[Cue::Compress, Cue::Spring]);
        // The first ends.
        fake.live.retain(|h| *h != 1);
        voices.play(&mut fake, &[]);
        assert_eq!(voices.len(), 1);
        assert_eq!(fake.stopped, [1]);
        fake.live.clear();
        voices.play(&mut fake, &[]);
        assert!(voices.is_empty());
        assert_eq!(fake.stopped, [1, 2]);
        // Nothing sounding and no cue: the voice is not asked.
        voices.play(&mut fake, &[]);
        assert_eq!(fake.stopped, [1, 2]);
    }

    #[test]
    fn past_the_voices_the_oldest_gives_way() {
        let mut fake = Fake::default();
        let mut voices = Voices::new();
        for _ in 0..(VOICES + 2) {
            voices.play(&mut fake, &[Cue::Compress]);
        }
        assert_eq!(voices.len(), VOICES);
        assert_eq!(fake.stopped, [1, 2]);
        assert_eq!(fake.live, [3, 4, 5]);
        voices.silence(&mut fake);
        assert!(voices.is_empty());
        assert!(fake.live.is_empty());
    }

    #[test]
    fn a_clip_that_cannot_play_is_dropped() {
        let mut fake = Fake {
            broken: true,
            ..Fake::default()
        };
        let mut voices = Voices::new();
        voices.play(&mut fake, &[Cue::Spring, Cue::Settle]);
        assert!(voices.is_empty());
        assert!(fake.started.is_empty());
    }
}
