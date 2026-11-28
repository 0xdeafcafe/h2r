//! The announcer speaks our medals (docs/audio/audio.md, "Our medals'
//! announcer lines"): when the HUD shows one of our medal lines
//! ([`announce`], from `superbounce_hud`), its clip from `art/announcer`
//! is queued and played the way the game's announcer plays a multi-kill.
//!
//! The game's announcer is a queue in guest memory (`announcer_queue`):
//! one line at a time, each after the last has finished. Our clips have no
//! sound tag, so they cannot join that queue as lines of its own. They
//! take turns with it instead: a clip of ours starts only when the game's
//! queue is idle ([`game_idle`]), and while it plays the feature holds the
//! game's queue (a `before` hook on `ANNOUNCER_QUEUE_UPDATE` that skips
//! the update), so a retail line queued meanwhile waits for ours to end.
//! The clip plays as a plain 2D voice in DirectSound's mixer, under its
//! master volume.
//!
//! The clips are embedded at build time (`include_bytes!`). They are in
//! Git LFS: a checkout without them (`GIT_LFS_SKIP_SMUDGE`) builds, and
//! each clip that is still an LFS pointer is named once at run time and
//! stays silent.

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use guest_layout::memory::{GuestMemory, MemFault};
use guest_layout::points::{self, announcer_queue};
use hud_kit_core::views::superbounce::{MEDALS, Medal};

/// The feature's id.
pub const FEATURE: &str = "medal-announcer";

/// Our medals, game types and Firefight's lines (docs/game/firefight.md,
/// "The announcer"), by id: the file `art/announcer/<id>.wav`.
/// Bounced has none, so it is silent. `incel.wav` is not embedded: no line
/// in the game shows the Incel medal (docs/game/teabag.md), so nothing
/// would speak it.
pub const CLIPS: [(&str, &[u8]); 41] = [
    ("superbounce", include_bytes!("../../../../art/announcer/superbounce.wav")),
    ("double-bounce", include_bytes!("../../../../art/announcer/double-bounce.wav")),
    ("triple-bounce", include_bytes!("../../../../art/announcer/triple-bounce.wav")),
    ("quad-bounce", include_bytes!("../../../../art/announcer/quad-bounce.wav")),
    ("chain-bounce", include_bytes!("../../../../art/announcer/chain-bounce.wav")),
    ("bouncetacular", include_bytes!("../../../../art/announcer/bouncetacular.wav")),
    ("monster-bounce", include_bytes!("../../../../art/announcer/monster-bounce.wav")),
    ("firefight", include_bytes!(concat!(env!("OUT_DIR"), "/firefight.wav"))),
    ("siege", include_bytes!(concat!(env!("OUT_DIR"), "/siege.wav"))),
    ("new-wave", include_bytes!("../../../../art/announcer/new-wave.wav")),
    ("wave-complete", include_bytes!("../../../../art/announcer/wave-complete.wav")),
    ("all-skulls-on", include_bytes!("../../../../art/announcer/all-skulls-on.wav")),
    ("envy-on", include_bytes!("../../../../art/announcer/envy-on.wav")),
    ("grunt-birthday-party-on", include_bytes!("../../../../art/announcer/grunt-birthday-party-on.wav")),
    ("assassins-on", include_bytes!("../../../../art/announcer/assassins-on.wav")),
    ("thunderstorm-on", include_bytes!("../../../../art/announcer/thunderstorm-on.wav")),
    ("famine-on", include_bytes!("../../../../art/announcer/famine-on.wav")),
    ("iwhbyd-on", include_bytes!("../../../../art/announcer/iwhbyd-on.wav")),
    ("blind-on", include_bytes!("../../../../art/announcer/blind-on.wav")),
    ("ghost-on", include_bytes!("../../../../art/announcer/ghost-on.wav")),
    ("black-eye-on", include_bytes!("../../../../art/announcer/black-eye-on.wav")),
    ("catch-on", include_bytes!("../../../../art/announcer/catch-on.wav")),
    ("sputnik-on", include_bytes!("../../../../art/announcer/sputnik-on.wav")),
    ("iron-on", include_bytes!("../../../../art/announcer/iron-on.wav")),
    ("mythic-on", include_bytes!("../../../../art/announcer/mythic-on.wav")),
    ("angry-on", include_bytes!("../../../../art/announcer/angry-on.wav")),
    ("bandana-on", include_bytes!("../../../../art/announcer/bandana-on.wav")),
    ("round-complete", include_bytes!("../../../../art/announcer/round-complete.wav")),
    ("set-complete", include_bytes!("../../../../art/announcer/set-complete.wav")),
    ("bonus-round", include_bytes!("../../../../art/announcer/bonus-round.wav")),
    ("bonus-round-complete", include_bytes!("../../../../art/announcer/bonus-round-complete.wav")),
    ("lives-added", include_bytes!("../../../../art/announcer/lives-added.wav")),
    ("last-life", include_bytes!("../../../../art/announcer/last-life.wav")),
    ("game-over", include_bytes!("../../../../art/announcer/game-over.wav")),
    ("skulls-off", include_bytes!("../../../../art/announcer/skulls-off.wav")),
    ("attack", include_bytes!("../../../../art/announcer/attack.wav")),
    ("defend", include_bytes!("../../../../art/announcer/defend.wav")),
    ("final-round", include_bytes!("../../../../art/announcer/final-round.wav")),
    ("site-captured", include_bytes!("../../../../art/announcer/site-captured.wav")),
    ("reinforcements", include_bytes!("../../../../art/announcer/reinforcements.wav")),
    ("round-over", include_bytes!("../../../../art/announcer/round-over.wav")),
];

/// The clip id of `medal`, when it has a clip.
pub fn clip_id(medal: Medal) -> Option<&'static str> {
    CLIPS
        .iter()
        .find(|(id, _)| *id == medal.id())
        .map(|(id, _)| *id)
}

/// The embedded bytes of clip `id`.
pub fn clip_bytes(id: &str) -> Option<&'static [u8]> {
    CLIPS.iter().find(|(i, _)| *i == id).map(|(_, b)| *b)
}

/// The clip id for a HUD line `text`: the medal whose line it is
/// (`Medal::line`), when that medal has a clip. Any other line is `None`.
pub fn clip_for_line(text: &str) -> Option<&'static str> {
    MEDALS
        .into_iter()
        .find(|m| m.line() == text)
        .and_then(clip_id)
}

// ---- the clips ----

/// Why a clip cannot be played.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClipError {
    /// The file is a Git LFS pointer: the checkout did not fetch it.
    LfsPointer,
    /// Not a RIFF WAVE file.
    NotWave,
    /// A WAVE file, but not 16-bit PCM in one or two channels.
    Format(String),
}

impl std::fmt::Display for ClipError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClipError::LfsPointer => {
                f.write_str("a Git LFS pointer, not the WAV (run `git lfs pull` and build again)")
            }
            ClipError::NotWave => f.write_str("not a RIFF WAVE file"),
            ClipError::Format(why) => write!(f, "not 16-bit PCM: {why}"),
        }
    }
}

/// A clip read from a WAV file: its channels, rate and 16-bit samples.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Clip {
    pub channels: u16,
    pub rate: u32,
    /// Interleaved little-endian 16-bit samples, whole frames.
    pub data: Vec<u8>,
}

/// Reads a RIFF WAVE file of 16-bit PCM, mono or stereo.
pub fn parse_wav(bytes: &[u8]) -> Result<Clip, ClipError> {
    if bytes.starts_with(b"version https://git-lfs") {
        return Err(ClipError::LfsPointer);
    }
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(ClipError::NotWave);
    }
    let le16 = |b: &[u8], at: usize| u16::from_le_bytes([b[at], b[at + 1]]);
    let le32 = |b: &[u8], at: usize| u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]);
    let mut fmt: Option<(u16, u16, u32, u16)> = None;
    let mut data: Option<&[u8]> = None;
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let len = le32(bytes, at + 4) as usize;
        let body = &bytes[at + 8..(at + 8).saturating_add(len).min(bytes.len())];
        match id {
            b"fmt " if body.len() >= 16 => {
                fmt = Some((le16(body, 0), le16(body, 2), le32(body, 4), le16(body, 14)));
            }
            b"data" => data = Some(body),
            _ => {}
        }
        // Chunks are padded to an even length.
        at = (at + 8).saturating_add(len).saturating_add(len & 1);
    }
    let Some((tag, channels, rate, bits)) = fmt else {
        return Err(ClipError::Format("no fmt chunk".into()));
    };
    if tag != 1 || bits != 16 {
        return Err(ClipError::Format(format!("format tag {tag}, {bits} bits")));
    }
    if !(1..=2).contains(&channels) || rate == 0 {
        return Err(ClipError::Format(format!("{channels} channels at {rate} Hz")));
    }
    let data = data.ok_or_else(|| ClipError::Format("no data chunk".into()))?;
    let frame = 2 * usize::from(channels);
    let whole = data.len() / frame * frame;
    Ok(Clip {
        channels,
        rate,
        data: data[..whole].to_vec(),
    })
}

// ---- the queue ----

/// What plays our lines: the mixer in the game, a fake in the tests.
pub trait Speaker {
    /// Starts clip `id`; false when it cannot be played (no audio, or a
    /// clip that does not read).
    fn start(&mut self, id: &'static str) -> bool;
    /// Whether the line last started is still sounding.
    fn sounding(&mut self) -> bool;
}

/// Lines waiting at most. With the one playing that is five, the game's
/// own announcer queue's length; past it the oldest waiting line gives way,
/// so the newest medal of a run of bounces is always spoken.
pub const MAX_WAITING: usize = 4;

/// Ticks a line of ours may hold the game's queue: five seconds, over
/// twice the longest clip. A line the mixer never finishes (its output
/// stalled) lets the game's announcer go on after that.
pub const MAX_HOLD_TICKS: u32 = 150;

/// Our announcer queue: one line at a time, in the order the HUD showed
/// them, each starting only when the line before it has ended and the
/// game's announcer is idle.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Queue {
    waiting: VecDeque<&'static str>,
    speaking: Option<&'static str>,
    /// Ticks the line playing has held the game's queue.
    held: u32,
}

impl Queue {
    pub const fn new() -> Queue {
        Queue {
            waiting: VecDeque::new(),
            speaking: None,
            held: 0,
        }
    }

    /// Queues clip `id`.
    pub fn push(&mut self, id: &'static str) {
        if self.waiting.len() >= MAX_WAITING {
            self.waiting.pop_front();
        }
        self.waiting.push_back(id);
    }

    /// One tick: forgets the line playing once it has ended (or has held
    /// the game's queue for [`MAX_HOLD_TICKS`]), then, with none playing
    /// and the game's announcer idle (`game_idle`), starts the next line
    /// that plays. Returns the line started.
    pub fn step(&mut self, speaker: &mut dyn Speaker, game_idle: bool) -> Option<&'static str> {
        if self.speaking.is_some() {
            self.held += 1;
            if !speaker.sounding() || self.held > MAX_HOLD_TICKS {
                self.speaking = None;
            }
        }
        if self.speaking.is_some() || !game_idle {
            return None;
        }
        while let Some(id) = self.waiting.pop_front() {
            if speaker.start(id) {
                self.speaking = Some(id);
                self.held = 0;
                return Some(id);
            }
        }
        None
    }

    /// Whether a line of ours is playing: the game's queue is held.
    pub fn holds(&self) -> bool {
        self.speaking.is_some()
    }

    /// The line playing.
    pub fn speaking(&self) -> Option<&'static str> {
        self.speaking
    }

    /// The lines waiting, oldest first.
    pub fn waiting(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.waiting.iter().copied()
    }

    /// Drops the lines waiting (a new game); the one playing plays out.
    pub fn clear_waiting(&mut self) {
        self.waiting.clear();
    }
}

/// The announcer: our queue, fed from the HUD's lines while the feature is
/// on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Announcer {
    /// The feature is on: with it off, no line is queued.
    pub on: bool,
    pub queue: Queue,
}

impl Announcer {
    pub const fn new() -> Announcer {
        Announcer {
            on: false,
            queue: Queue::new(),
        }
    }

    /// The HUD showed `text` to a local player: when the announcer is on
    /// and it is one of our medal lines with a clip, the clip is queued.
    /// Returns the clip queued.
    pub fn announce(&mut self, text: &str) -> Option<&'static str> {
        let id = clip_for_line(text).filter(|_| self.on)?;
        self.queue.push(id);
        Some(id)
    }
}

// ---- the game's announcer queue ----

/// Whether the game's announcer is idle: no line queued, or only the head
/// with its delay run out, finished playing and about to be dropped
/// (`0x0023_f000`: a head whose delay is below zero and whose playing byte
/// is clear is taken off the queue).
pub fn game_idle(m: &dyn GuestMemory) -> Result<bool, MemFault> {
    let count = m.u32(announcer_queue::COUNT)? as i32;
    if count <= 0 {
        return Ok(true);
    }
    if count > 1 {
        return Ok(false);
    }
    let playing = m.u8(announcer_queue::HEAD_PLAYING)? != 0;
    let delay = m.u16(announcer_queue::HEAD_DELAY)? as i16;
    Ok(!playing && delay < 0)
}

// ---- the mixer ----

/// Plays clips as 2D voices in DirectSound's mixer, one at a time.
pub struct MixerSpeaker {
    mixer: Arc<Mutex<audio::Mixer>>,
    current: Option<audio::BufferId>,
    /// Each clip read once: its format and bytes, or why it does not play
    /// (said once, when first asked for).
    clips: BTreeMap<&'static str, Option<(audio::WaveFormat, Vec<u8>)>>,
}

impl MixerSpeaker {
    pub fn new(mixer: Arc<Mutex<audio::Mixer>>) -> MixerSpeaker {
        MixerSpeaker {
            mixer,
            current: None,
            clips: BTreeMap::new(),
        }
    }

    fn clip(&mut self, id: &'static str) -> Option<(audio::WaveFormat, Vec<u8>)> {
        self.clips
            .entry(id)
            .or_insert_with(|| {
                let read = clip_bytes(id)
                    .ok_or_else(|| "no such clip".to_string())
                    .and_then(|b| parse_wav(b).map_err(|e| e.to_string()))
                    .and_then(|c| {
                        audio::WaveFormat::pcm16(c.channels, c.rate)
                            .map(|f| (f, c.data))
                            .map_err(|e| e.to_string())
                    });
                read.map_err(|e| eprintln!("announcer: art/announcer/{id}.wav: {e}; it stays silent"))
                    .ok()
            })
            .clone()
    }
}

impl Speaker for MixerSpeaker {
    fn start(&mut self, id: &'static str) -> bool {
        let Some((format, data)) = self.clip(id) else {
            return false;
        };
        let mut buffer = match audio::SoundBuffer::from_data(format, data) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("announcer: art/announcer/{id}.wav: {e}");
                return false;
            }
        };
        buffer.play(false);
        let mut mixer = lock(&self.mixer);
        if let Some(old) = self.current.take() {
            mixer.remove_buffer(old);
        }
        self.current = Some(mixer.add_buffer(buffer));
        true
    }

    fn sounding(&mut self) -> bool {
        let Some(id) = self.current else {
            return false;
        };
        let mut mixer = lock(&self.mixer);
        // A buffer the mixer no longer has (a snapshot load) has ended.
        let playing = mixer.buffer(id).is_some_and(|b| b.is_playing());
        if !playing {
            mixer.remove_buffer(id);
            self.current = None;
        }
        playing
    }
}

// ---- the feature ----

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A line of ours is playing: the game's queue update is skipped.
static HOLD: AtomicBool = AtomicBool::new(false);
/// On once the feature is built (switched on).
static ANNOUNCER: Mutex<Announcer> = Mutex::new(Announcer::new());
/// The mixer's speaker; `None` without audio (headless clock-only runs),
/// where lines are dropped.
static SPEAKER: Mutex<Option<MixerSpeaker>> = Mutex::new(None);

/// Gives the announcer DirectSound's mixer to play into (boot, once the
/// audio is up).
pub fn connect(mixer: Arc<Mutex<audio::Mixer>>) {
    *lock(&SPEAKER) = Some(MixerSpeaker::new(mixer));
}

/// The HUD showed `text` to a local player: [`Announcer::announce`]. Each
/// local player's line queues, as the game queues a multi-kill's sound for
/// each local player who earned it.
pub fn announce(text: &str) {
    lock(&ANNOUNCER).announce(text);
}

/// Queue the script type's start line on this console.
pub(crate) fn announce_clip(id: &'static str) {
    let mut a = lock(&ANNOUNCER);
    if a.on {
        a.queue.push(id);
    }
}

fn on_tick(o: &hooks::Obs) {
    let idle = {
        let guest = o.guest();
        let m: &dyn GuestMemory = &*guest;
        // The queue is in the image's data; unreadable means no game.
        game_idle(m).unwrap_or(true)
    };
    let mut speaker = lock(&SPEAKER);
    let mut a = lock(&ANNOUNCER);
    match speaker.as_mut() {
        Some(s) => {
            a.queue.step(s, idle);
        }
        None => a.queue = Queue::new(),
    }
    HOLD.store(a.queue.holds(), Ordering::Relaxed);
}

/// The announcer for our medals.
pub struct MedalAnnouncer;

impl hooks::Feature for MedalAnnouncer {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new(FEATURE, "The announcer speaks our medals")
            .doc("docs/audio/audio.md")
            // A tick observer and a hold on the announcer's sound queue,
            // which is this console's alone: nothing the simulation reads.
            .presentation_only()
    }

    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        // A line lasts a second or two and starts again from nothing: after a
        // load the queue goes on from the next tick.
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        lock(&ANNOUNCER).on = true;
        b.game().on_start("medal announcer: a new game", |_| {
            lock(&ANNOUNCER).queue.clear_waiting()
        });
        b.frame().on_tick("medal announcer: the queue", on_tick);
        let _ = b.raw().before(
            &points::ANNOUNCER_QUEUE_UPDATE,
            "hold the announcer while our line plays",
            |c| {
                if HOLD.load(Ordering::Relaxed) {
                    return Ok(hooks::Flow::Return(c.ret(0)?));
                }
                Ok(hooks::Flow::Continue)
            },
        );
        Ok(())
    }
}

/// Adds the feature to `registry` (`H2_FEATURES=-medal-announcer` leaves
/// it out, and the medals are silent).
pub fn install(registry: &mut hooks::Registry) -> anyhow::Result<()> {
    registry
        .add(&MedalAnnouncer)
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("medal announcer: {e}"))
}

#[cfg(test)]
#[path = "medal_announcer_tests.rs"]
mod tests;

/// Queue clip `id` as an announcer line, when the announcer is on and
/// the clip exists. The Firefight HUD's lines come here
/// (`firefight::hud::banners::lines`).
pub fn announce_id(id: &str) {
    if let Some(id) = CLIPS.iter().map(|(i, _)| *i).find(|i| *i == id) {
        announce_clip(id);
    }
}

/// The lines waiting in the process's announcer, oldest first, with the
/// announcer on; they are taken. A test that uses it holds
/// `rules::world::audio::SERIAL`.
#[cfg(test)]
pub(crate) fn take_waiting() -> Vec<&'static str> {
    let mut a = lock(&ANNOUNCER);
    a.on = true;
    let out: Vec<_> = a.queue.waiting().collect();
    a.queue.clear_waiting();
    out
}
