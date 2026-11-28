//! The host mixer as the table's host ([`TableHost`]): each started sound
//! becomes a voice in the shared `audio::Mixer`, playing the permutation
//! the table drew at the gain and pitch it drew, placed by the scene
//! (docs/engine/sound-instances.md, "Mixer feed").
//!
//! | Sound | Voice |
//! |---|---|
//! | one-shot | a `SoundBuffer` of the decoded clip ([`Clips::get`]); a clip still decoding starts late at the elapsed offset, or is dropped past [`LATE_LIMIT_MS`] |
//! | loop track | an `audio::Stream` fed chunk by chunk ([`Clips::stream`]), [`STREAM_AHEAD_MS`] ahead; a segment that follows another queues behind it on the same stream, so a loop or a music track plays with no gap |
//!
//! Each frame the service hands the feed the tick's events in the table's
//! order, then the [`Scene`]; the feed then places every voice, ramps the
//! fading ones and pumps the streams: the guest frame's order (`0x21b0e0`,
//! `0x1290d0`, `0x128020`, `0x1282d0`, `0x21ee80`). The feed reads the
//! events and the scene, never the table, so the table's hash is the same
//! with it, with [`sim_sound::feed::NullTableHost`] or with nothing.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};

use audio::dsound::{Mode3d, PacketId, Source3d, Vec3};
use sim_sound::events::{TICKS_PER_SECOND, TableEvent};
use sim_sound::feed::{Scene, TableHost, heard_at};
use sim_sound::instances::{Handle, SourceParams, TagIndex};
use sim_sound::owners::Reason;
use sound_clips::{ClipError, ClipKey, Got};

/// A clip still decoding when its sound starts plays from the elapsed
/// offset if it is ready inside this; later it is dropped and counted.
pub const LATE_LIMIT_MS: u32 = 250;
/// Voices the feed plays at once. A start past it is culled and counted;
/// the table runs the sound's ticks and callbacks all the same.
pub const MAX_VOICES: usize = 128;
/// How far ahead of the play cursor a track's stream is kept fed.
pub const STREAM_AHEAD_MS: u32 = 500;
/// The most a following segment may wait behind the one before it: the
/// table starts a segment on its predecessor's rounded end tick, so the
/// stream can run ahead of the sim. Past this the new segment is cut in
/// by the excess and the cut counted (`feed.seam_cuts`).
pub const SEAM_LEAD_MS: u32 = 2 * 1000 / TICKS_PER_SECOND;

/// Decoded clips for the feed: [`sound_clips::ClipSource`] in the game.
pub trait Clips: Send {
    /// The whole clip, or `Pending` while it decodes.
    fn get(&mut self, key: ClipKey) -> Result<Got, ClipError>;
    /// The clip a chunk at a time, for a track's stream.
    fn stream(&mut self, key: ClipKey) -> Result<Box<dyn Chunks>, ClipError>;
}

/// A permutation's samples in runs: [`sound_clips::ChunkReader`].
pub trait Chunks: Send {
    fn channels(&self) -> u16;
    fn rate(&self) -> u32;
    /// The next run of interleaved samples, `None` at the end.
    fn next_chunk(&mut self) -> Result<Option<Vec<i16>>, ClipError>;
}

impl Chunks for sound_clips::ChunkReader {
    fn channels(&self) -> u16 {
        self.channels
    }
    fn rate(&self) -> u32 {
        self.rate
    }
    fn next_chunk(&mut self) -> Result<Option<Vec<i16>>, ClipError> {
        sound_clips::ChunkReader::next_chunk(self).map(|c| c.map(|c| c.pcm))
    }
}

impl Clips for sound_clips::ClipSource {
    fn get(&mut self, key: ClipKey) -> Result<Got, ClipError> {
        sound_clips::ClipSource::get(self, key)
    }
    fn stream(&mut self, key: ClipKey) -> Result<Box<dyn Chunks>, ClipError> {
        sound_clips::ClipSource::stream(self, key).map(|r| Box::new(r) as Box<dyn Chunks>)
    }
}

/// The clip a started sound plays: its tag and the range and permutation
/// the table drew.
pub fn clip_key(tag: TagIndex, range: i8, permutation: i8) -> ClipKey {
    ClipKey { tag: map::TagId(tag.0), range: range as u8 as u16, permutation: permutation as u8 as u16 }
}

/// Millibels of a level in dB, clamped to the mixer's range.
pub fn millibels(db: f32) -> i32 {
    if db.is_finite() { ((db * 100.0).round() as i32).clamp(audio::dsound::VOLUME_MIN_MB, 0) } else { audio::dsound::VOLUME_MIN_MB }
}

/// A clip's rate shifted by a pitch in cents.
pub fn frequency(rate: u32, cents: f32) -> u32 {
    let f = f64::from(rate) * 2f64.powf(f64::from(cents) / 1200.0);
    if f.is_finite() { f.round().clamp(100.0, 200_000.0) as u32 } else { rate }
}

/// What the feed counts (`feed.*`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FeedCounters {
    /// Starts given a voice, playing or waiting for its clip.
    pub started: u64,
    /// Starts past [`MAX_VOICES`].
    pub culled: u64,
    /// Clips not ready inside [`LATE_LIMIT_MS`].
    pub clip_pending_dropped: u64,
    /// Clips the source refused by name (a codec stream with no tables).
    pub clip_refused: u64,
    /// Clips that failed to read or decode.
    pub clip_failed: u64,
    /// Chunks handed to the tracks' streams.
    pub streamed_chunks: u64,
    /// Following segments cut in by [`SEAM_LEAD_MS`]'s excess.
    pub seam_cuts: u64,
}

/// A voice as the mixer plays it, for tests and `audio.loops`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoiceView {
    pub frequency: u32,
    pub volume_mb: i32,
    /// Listener-relative right, up and front, for a positioned voice.
    pub position: Option<[f32; 3]>,
    /// The track stream it plays on, else a buffer.
    pub streamed: bool,
    /// Its clip is still decoding.
    pub waiting: bool,
    /// A buffer's play cursor, frames.
    pub cursor: f64,
}

#[derive(Clone, Copy, Debug)]
enum Plays {
    /// Its clip is decoding: started on `tick`.
    Waiting { key: ClipKey, tick: u32 },
    Buffer(audio::BufferId),
    /// A track stream, by its key in `MixerHost::tracks`.
    Track(u32),
}

#[derive(Clone, Copy, Debug)]
struct Voice {
    plays: Plays,
    /// `level + gain`, millibels.
    base_mb: i32,
    pitch: f32,
    distance: [f32; 2],
    source: SourceParams,
    /// A stop's ramp: since `tick`, over `ticks`.
    fade: Option<(u32, u32)>,
    /// Played out: a buffer is left to finish and swept.
    ended: bool,
}

/// One segment waiting to be streamed: its reader and the frames cut from
/// its start at the seam.
struct Pending {
    chunks: Box<dyn Chunks>,
    cut: u64,
}

struct TrackStream {
    stream: audio::StreamId,
    channels: u16,
    rate: u32,
    /// Packets submitted and not yet played out, with their frames.
    queued: VecDeque<(PacketId, u64)>,
    readers: VecDeque<Pending>,
    /// The sounds playing on it, the latest last: it sets the volume.
    members: Vec<u32>,
    /// Played out with no follower: freed once it drains.
    draining: bool,
}

impl TrackStream {
    /// Frames queued ahead of the play cursor.
    fn ahead(&self, stream: &audio::Stream) -> u64 {
        let total: u64 = self.queued.iter().map(|(_, n)| n).sum();
        let played = match (stream.packet_playhead(), self.queued.front()) {
            (Some((id, elapsed)), Some((front, _))) if id == *front => elapsed,
            _ => 0,
        };
        total.saturating_sub(played)
    }

    fn frames_ms(&self, frames: u64) -> u64 {
        frames * 1000 / u64::from(self.rate.max(1))
    }
}

pub struct MixerHost<C: Clips> {
    mixer: Arc<Mutex<audio::Mixer>>,
    clips: C,
    /// By the table's raw handle.
    voices: BTreeMap<u32, Voice>,
    tracks: BTreeMap<u32, TrackStream>,
    next_track: u32,
    counters: FeedCounters,
}

fn lock(m: &Mutex<audio::Mixer>) -> MutexGuard<'_, audio::Mixer> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl<C: Clips> MixerHost<C> {
    pub fn new(mixer: Arc<Mutex<audio::Mixer>>, clips: C) -> Self {
        MixerHost { mixer, clips, voices: BTreeMap::new(), tracks: BTreeMap::new(), next_track: 0, counters: FeedCounters::default() }
    }

    pub fn feed_counters(&self) -> FeedCounters {
        self.counters
    }

    /// Voices held: playing, fading, waiting or playing out.
    pub fn held(&self) -> usize {
        self.voices.len()
    }

    /// The voice of a table handle, as the mixer plays it.
    pub fn voice(&self, handle: Handle) -> Option<VoiceView> {
        let v = self.voices.get(&handle.raw())?;
        let mixer = lock(&self.mixer);
        let params = match v.plays {
            Plays::Buffer(id) => mixer.buffer(id).map(|b| b.params().clone()),
            Plays::Track(t) => self.tracks.get(&t).and_then(|t| mixer.stream(t.stream)).map(|s| s.params().clone()),
            Plays::Waiting { .. } => None,
        };
        Some(VoiceView {
            frequency: params.as_ref().map_or(0, |p| p.frequency),
            volume_mb: params.as_ref().map_or(audio::dsound::VOLUME_MIN_MB, |p| p.volume_mb),
            position: params.as_ref().and_then(|p| p.spatial.as_ref().map(|s| [s.position.x, s.position.y, s.position.z])),
            streamed: matches!(v.plays, Plays::Track(_)),
            waiting: matches!(v.plays, Plays::Waiting { .. }),
            cursor: match v.plays {
                Plays::Buffer(id) => mixer.buffer(id).map_or(0.0, |b| b.position_frames()),
                _ => 0.0,
            },
        })
    }

    fn live(&self) -> usize {
        self.voices.values().filter(|v| !v.ended).count()
    }

    fn refuse(&mut self, e: &ClipError) {
        match e {
            ClipError::Refused(_) => self.counters.clip_refused += 1,
            ClipError::Failed(_) => self.counters.clip_failed += 1,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn start(
        &mut self,
        handle: u32,
        key: ClipKey,
        base_mb: i32,
        pitch: f32,
        distance: [f32; 2],
        source: SourceParams,
        track: bool,
        follows: Option<u32>,
        tick: u32,
    ) {
        if self.live() >= MAX_VOICES {
            self.counters.culled += 1;
            return;
        }
        let voice = |plays| Voice { plays, base_mb, pitch, distance, source, fade: None, ended: false };
        if track {
            let chunks = match self.clips.stream(key) {
                Ok(c) => c,
                Err(e) => return self.refuse(&e),
            };
            let Some((t, cut)) = self.track_for(follows, &*chunks) else { return };
            if let Some(stream) = self.tracks.get_mut(&t) {
                stream.members.push(handle);
                stream.draining = false;
                stream.readers.push_back(Pending { chunks, cut });
            }
            self.voices.insert(handle, voice(Plays::Track(t)));
            self.counters.started += 1;
            self.pump(t);
            return;
        }
        match self.clips.get(key) {
            Ok(Got::Ready(clip)) => {
                if let Some(id) = self.buffer(&clip, 0) {
                    self.voices.insert(handle, voice(Plays::Buffer(id)));
                    self.counters.started += 1;
                }
            }
            Ok(Got::Pending) => {
                self.voices.insert(handle, voice(Plays::Waiting { key, tick }));
                self.counters.started += 1;
            }
            Err(e) => self.refuse(&e),
        }
    }

    /// The stream a track segment plays on and the frames to cut from its
    /// start: its predecessor's stream when it follows one of the same
    /// format, cut by the lead past [`SEAM_LEAD_MS`]; else a new one.
    fn track_for(&mut self, follows: Option<u32>, chunks: &dyn Chunks) -> Option<(u32, u64)> {
        let (channels, rate) = (chunks.channels(), chunks.rate());
        let before = follows.and_then(|f| match self.voices.get(&f)?.plays {
            Plays::Track(t) => Some(t),
            _ => None,
        });
        if let Some(t) = before {
            // Played-out packets leave the count first.
            self.pump(t);
            let mixer = lock(&self.mixer);
            if let Some(stream) = self.tracks.get_mut(&t).filter(|s| (s.channels, s.rate) == (channels, rate)) {
                let ahead = mixer.stream(stream.stream).map_or(0, |s| stream.ahead(s));
                let lead = u64::from(SEAM_LEAD_MS) * u64::from(rate) / 1000;
                if stream.readers.is_empty() && ahead > lead {
                    self.counters.seam_cuts += 1;
                    return Some((t, ahead - lead));
                }
                return Some((t, 0));
            }
        }
        let format = audio::WaveFormat::pcm16(channels, rate).ok()?;
        let stream = audio::Stream::new(format).ok()?;
        let id = lock(&self.mixer).add_stream(stream);
        let t = self.next_track;
        self.next_track = self.next_track.wrapping_add(1);
        self.tracks.insert(t, TrackStream { stream: id, channels, rate, queued: VecDeque::new(), readers: VecDeque::new(), members: Vec::new(), draining: false });
        Some((t, 0))
    }

    /// A buffer of `clip`, playing from `offset` frames.
    fn buffer(&mut self, clip: &sound_clips::Clip, offset: usize) -> Option<audio::BufferId> {
        let format = audio::WaveFormat::pcm16(clip.channels, clip.rate).ok()?;
        let bytes: Vec<u8> = clip.pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut buffer = audio::SoundBuffer::from_data(format, bytes).ok()?;
        buffer.set_current_position(offset * usize::from(clip.channels.max(1)) * 2);
        buffer.play(false);
        Some(lock(&self.mixer).add_buffer(buffer))
    }

    /// Feed track `t`'s stream up to [`STREAM_AHEAD_MS`] ahead.
    fn pump(&mut self, t: u32) {
        let Some(track) = self.tracks.get_mut(&t) else { return };
        let mut mixer = lock(&self.mixer);
        let Some(stream) = mixer.stream_mut(track.stream) else { return };
        let done: Vec<PacketId> = stream.take_completed().into_iter().map(|(id, _)| id).collect();
        track.queued.retain(|(id, _)| !done.contains(id));
        let ahead = u64::from(STREAM_AHEAD_MS) * u64::from(track.rate) / 1000;
        let channels = u64::from(track.channels.max(1));
        while track.ahead(stream) < ahead {
            let Some(front) = track.readers.front_mut() else { break };
            match front.chunks.next_chunk() {
                Ok(Some(pcm)) => {
                    let frames = pcm.len() as u64 / channels;
                    let skip = front.cut.min(frames);
                    front.cut -= skip;
                    let pcm = &pcm[(skip * channels) as usize..];
                    if pcm.is_empty() {
                        continue;
                    }
                    let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
                    if let Ok(id) = stream.submit(&bytes) {
                        track.queued.push_back((id, frames - skip));
                        self.counters.streamed_chunks += 1;
                    }
                }
                Ok(None) => {
                    track.readers.pop_front();
                }
                Err(e) => {
                    track.readers.pop_front();
                    match e {
                        ClipError::Refused(_) => self.counters.clip_refused += 1,
                        ClipError::Failed(_) => self.counters.clip_failed += 1,
                    }
                }
            }
        }
    }

    /// Drop a voice; a track's stream goes with its last member, or drains
    /// first when that member played out.
    fn end(&mut self, handle: u32, reason: Reason) {
        let Some(plays) = self.voices.get(&handle).map(|v| v.plays) else { return };
        match plays {
            Plays::Buffer(_) if reason == Reason::PlayedOut => {
                if let Some(v) = self.voices.get_mut(&handle) {
                    v.ended = true;
                }
            }
            Plays::Buffer(id) => {
                self.voices.remove(&handle);
                lock(&self.mixer).remove_buffer(id);
            }
            Plays::Waiting { .. } => {
                self.voices.remove(&handle);
            }
            Plays::Track(t) => {
                self.voices.remove(&handle);
                let Some(track) = self.tracks.get_mut(&t) else { return };
                track.members.retain(|&m| m != handle);
                if !track.members.is_empty() {
                    return;
                }
                if reason == Reason::PlayedOut {
                    track.draining = true;
                } else if let Some(track) = self.tracks.remove(&t) {
                    lock(&self.mixer).remove_stream(track.stream);
                }
            }
        }
    }

    /// Volume, frequency and place of every voice for `scene`.
    fn place(&mut self, scene: &Scene) {
        let sources: BTreeMap<u32, SourceParams> = scene.sources.iter().map(|(h, s)| (h.raw(), *s)).collect();
        let mut mixer = lock(&self.mixer);
        for (handle, v) in self.voices.iter_mut() {
            if let Some(s) = sources.get(handle) {
                v.source = *s;
            }
            let fade = match v.fade {
                Some((since, ticks)) => {
                    let gone = scene.tick.saturating_sub(since) as f32 / ticks.max(1) as f32;
                    let left = (1.0 - gone).clamp(0.0, 1.0);
                    if left > 0.0 { (2000.0 * left.log10()).round() as i32 } else { audio::dsound::VOLUME_MIN_MB }
                }
                None => 0,
            };
            let at = heard_at(&v.source, scene.ear.as_ref());
            let far = at.is_some_and(|p| (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt() > v.distance[1]);
            let mb = if far { audio::dsound::VOLUME_MIN_MB } else { v.base_mb.saturating_add(fade) };
            let spatial = at.map(|p| Source3d {
                position: Vec3::new(p[0], p[1], p[2]),
                min_distance: v.distance[0].max(f32::MIN_POSITIVE),
                max_distance: v.distance[1].max(v.distance[0]),
                mode: Mode3d::HeadRelative,
                ..Source3d::default()
            });
            match v.plays {
                Plays::Buffer(id) => {
                    if let Some(b) = mixer.buffer_mut(id) {
                        let rate = b.format().sample_rate();
                        b.set_frequency(frequency(rate, v.pitch));
                        b.set_volume(mb);
                        b.set_source_3d(spatial);
                    }
                }
                Plays::Track(t) => {
                    let Some(track) = self.tracks.get(&t) else { continue };
                    // The latest member sets the stream.
                    if track.members.last() != Some(handle) {
                        continue;
                    }
                    if let Some(s) = mixer.stream_mut(track.stream) {
                        s.set_frequency(frequency(track.rate, v.pitch));
                        s.set_volume(mb);
                        s.set_source_3d(spatial);
                    }
                }
                Plays::Waiting { .. } => {}
            }
        }
    }

    /// Start the clips that finished decoding, at the elapsed offset; drop
    /// the ones later than [`LATE_LIMIT_MS`].
    fn late(&mut self, tick: u32) {
        let waiting: Vec<(u32, ClipKey, u32)> = self
            .voices
            .iter()
            .filter_map(|(h, v)| match v.plays {
                Plays::Waiting { key, tick } => Some((*h, key, tick)),
                _ => None,
            })
            .collect();
        for (handle, key, since) in waiting {
            let elapsed_ms = tick.saturating_sub(since) * 1000 / TICKS_PER_SECOND;
            let late = elapsed_ms > LATE_LIMIT_MS;
            match self.clips.get(key) {
                Ok(Got::Ready(clip)) if !late => {
                    let offset = elapsed_ms as usize * clip.rate as usize / 1000;
                    match self.buffer(&clip, offset) {
                        Some(id) => {
                            if let Some(v) = self.voices.get_mut(&handle) {
                                v.plays = Plays::Buffer(id);
                            }
                        }
                        None => {
                            self.voices.remove(&handle);
                        }
                    }
                }
                Ok(Got::Pending) if !late => {}
                Ok(_) => {
                    self.counters.clip_pending_dropped += 1;
                    self.voices.remove(&handle);
                }
                Err(e) => {
                    self.refuse(&e);
                    self.voices.remove(&handle);
                }
            }
        }
    }

    /// Free played-out buffers and drained streams.
    fn sweep(&mut self) {
        let mut mixer = lock(&self.mixer);
        self.voices.retain(|_, v| match v.plays {
            Plays::Buffer(id) if v.ended => {
                let playing = mixer.buffer(id).is_some_and(|b| b.is_playing());
                if !playing {
                    mixer.remove_buffer(id);
                }
                playing
            }
            _ => true,
        });
        self.tracks.retain(|_, t| {
            let drained = t.draining && t.readers.is_empty() && mixer.stream(t.stream).is_none_or(|s| s.pending() == 0);
            if drained {
                mixer.remove_stream(t.stream);
            }
            !drained
        });
    }
}

impl<C: Clips> TableHost for MixerHost<C> {
    fn event(&mut self, event: &TableEvent) {
        match *event {
            TableEvent::Started { handle, tag, selection, gain, pitch, source, level, distance, track, follows, tick, .. } => {
                let key = clip_key(tag, selection.range, selection.permutation);
                let base = millibels(level + gain);
                self.start(handle.raw(), key, base, pitch, distance, source, track.is_some(), follows.map(|f| f.raw()), tick);
            }
            TableEvent::Stopped { handle, fade_ticks, tick } => {
                if let Some(v) = self.voices.get_mut(&handle.raw()) {
                    v.fade = Some((tick, fade_ticks));
                }
            }
            TableEvent::Ended { handle, reason, .. } => self.end(handle.raw(), reason),
        }
    }

    fn frame(&mut self, scene: &Scene) {
        self.late(scene.tick);
        self.place(scene);
        let tracks: Vec<u32> = self.tracks.keys().copied().collect();
        for t in tracks {
            self.pump(t);
        }
        self.sweep();
    }

    fn clear(&mut self) {
        let mut mixer = lock(&self.mixer);
        for v in self.voices.values() {
            if let Plays::Buffer(id) = v.plays {
                mixer.remove_buffer(id);
            }
        }
        for t in self.tracks.values() {
            mixer.remove_stream(t.stream);
        }
        drop(mixer);
        self.voices.clear();
        self.tracks.clear();
    }

    fn counters(&self) -> Vec<(&'static str, u64)> {
        let c = self.counters;
        vec![
            ("feed.started", c.started),
            ("feed.culled", c.culled),
            ("feed.clip_pending_dropped", c.clip_pending_dropped),
            ("feed.clip_refused", c.clip_refused),
            ("feed.clip_failed", c.clip_failed),
            ("feed.streamed_chunks", c.streamed_chunks),
            ("feed.seam_cuts", c.seam_cuts),
        ]
    }
}

#[cfg(test)]
mod tests;
