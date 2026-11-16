//! A DirectSound-shaped model: buffers, streams and a 3D stub.
//!
//! This mirrors the *semantics* the Xbox SDK's `IDirectSoundBuffer` and
//! `IDirectSoundStream` expose, in typed Rust. There is no COM, no vtable and
//! no APU voice allocation; a guest-side shim (owned elsewhere) translates
//! calls into these methods. Units follow the SDK on purpose so the shim is a
//! thin mapping: volume and pan are in millibels (1/100 dB), frequency in Hz,
//! positions in bytes at the buffer API and frames internally.

use std::collections::VecDeque;

use crate::{
    AudioError,
    format::{Codec, WaveFormat},
    fx::{EnvSpec, FilterSpec, FxState, I3dl2Source, LfoSpec, MAX_PHASE_OFFSET, VoiceFx},
};

/// `DSBVOLUME_MIN`. At or below this the voice is silent.
pub const VOLUME_MIN_MB: i32 = -10_000;
/// `DSBPAN_LEFT` / `DSBPAN_RIGHT` extremes.
pub const PAN_MAX_MB: i32 = 10_000;

/// Identifies a buffer inside a [`crate::Mixer`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BufferId(pub(crate) u32);

/// Identifies a stream inside a [`crate::Mixer`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StreamId(pub(crate) u32);

/// Millibels to linear amplitude: `10^(mb / 2000)`.
pub fn mb_to_gain(mb: i32) -> f32 {
    if mb <= VOLUME_MIN_MB {
        0.0
    } else {
        10f32.powf(mb.min(0) as f32 / 2000.0)
    }
}

/// Parameters common to buffers and streams.
#[derive(Clone, Debug, PartialEq)]
pub struct VoiceParams {
    /// Playback rate in Hz; defaults to the format's rate.
    pub frequency: u32,
    /// Volume in millibels, `-10000..=0`.
    pub volume_mb: i32,
    /// Pan in millibels, `-10000` (left) to `10000` (right). The side away
    /// from the pan direction is attenuated, as in DirectSound; the near side
    /// stays at full volume.
    pub pan_mb: i32,
    /// Optional 3D source; see [`Listener`].
    pub spatial: Option<Source3d>,
    /// Xbox mix-bin routing, `(bin, volume in millibels)`; see [`crate::route`].
    /// `None` mixes the voice straight to stereo with `volume_mb` and `pan_mb`
    /// (and the 3D panning, if any), as a plain DirectSound voice.
    pub mix_bins: Option<Vec<(u32, i32)>>,
    /// Filter, LFOs, envelope, headroom and I3DL2 source; see [`crate::fx`].
    pub fx: VoiceFx,
    /// Which host output hears the voice: the main mix, a second "side"
    /// device (a controller speaker), or both. See [`VoiceOutput`].
    pub output: VoiceOutput,
}

/// Where a voice is heard when the mixer renders a side output (see
/// `Mixer::render_split`). Without a side output every voice is heard on the
/// main mix, whatever this says, so a sound routed to an absent controller
/// is never lost.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum VoiceOutput {
    /// The main mix only (the default).
    #[default]
    Main,
    /// The main mix and, as mono, the side output.
    Both,
    /// The side output only, as mono.
    Side,
}

impl VoiceParams {
    fn new(format: &WaveFormat) -> Self {
        Self::with_frequency(format.sample_rate())
    }

    fn with_frequency(frequency: u32) -> Self {
        Self {
            frequency,
            volume_mb: 0,
            pan_mb: 0,
            spatial: None,
            mix_bins: None,
            fx: VoiceFx::default(),
            output: VoiceOutput::Main,
        }
    }

    /// Left and right linear gains from volume and pan (no 3D).
    pub fn gains(&self) -> (f32, f32) {
        let v = mb_to_gain(self.volume_mb);
        let pan = self.pan_mb.clamp(-PAN_MAX_MB, PAN_MAX_MB);
        let l = if pan > 0 { mb_to_gain(-pan) } else { 1.0 };
        let r = if pan < 0 { mb_to_gain(pan) } else { 1.0 };
        (v * l, v * r)
    }
}

macro_rules! voice_setters {
    () => {
        /// `SetFrequency`. Clamped to the SDK's 100..=200000 Hz range.
        pub fn set_frequency(&mut self, hz: u32) {
            self.params.frequency = hz.clamp(100, 200_000);
        }
        /// `SetVolume`, millibels.
        pub fn set_volume(&mut self, mb: i32) {
            self.params.volume_mb = mb.clamp(VOLUME_MIN_MB, 0);
        }
        /// `SetPan`, millibels.
        pub fn set_pan(&mut self, mb: i32) {
            self.params.pan_mb = mb.clamp(-PAN_MAX_MB, PAN_MAX_MB);
        }
        /// Attaches (or with `None` detaches) a 3D source.
        pub fn set_source_3d(&mut self, src: Option<Source3d>) {
            self.params.spatial = src;
        }
        /// `SetMixBins`/`SetMixBinVolumes`: routes the voice through Xbox mix
        /// bins (see [`crate::route`]); `None` mixes it straight to stereo.
        pub fn set_mix_bins(&mut self, bins: Option<Vec<(u32, i32)>>) {
            self.params.mix_bins = bins;
        }
        pub fn params(&self) -> &VoiceParams {
            &self.params
        }
        /// Chooses the host output that hears this voice ([`VoiceOutput`]).
        pub fn set_output(&mut self, output: VoiceOutput) {
            self.params.output = output;
        }
        /// Replaces every effect setting at once (the shim re-applies its
        /// stored copy this way); the voice's clock and memory are kept.
        pub fn set_fx(&mut self, fx: VoiceFx) {
            self.params.fx = fx;
        }
        /// `SetFilter`.
        pub fn set_filter(&mut self, filter: FilterSpec) {
            self.params.fx.filter = filter;
        }
        /// `SetLFO`: `index` 0 is the filter/amplitude LFO, 1 the pitch LFO.
        pub fn set_lfo(&mut self, index: usize, lfo: Option<LfoSpec>) {
            self.params.fx.lfo[index & 1] = lfo;
        }
        /// `SetEG` for the amplitude envelope; it starts from now.
        pub fn set_envelope(&mut self, env: Option<EnvSpec>) {
            self.params.fx.env = env;
            self.fx_state.restart_envelope();
        }
        /// `SetHeadroom`, millibels (see [`crate::fx::headroom_gain`]).
        pub fn set_headroom(&mut self, mb: i32) {
            self.params.fx.headroom_mb = mb.clamp(0, crate::fx::MAX_VOICE_HEADROOM_MB);
        }
        /// `SetI3DL2Source`: occlusion, obstruction and the room send level.
        pub fn set_i3dl2_source(&mut self, src: Option<I3dl2Source>) {
            self.params.fx.i3dl2 = src;
        }
        /// Frames this voice has produced since it (re)started.
        pub(crate) fn fx_clock(&self) -> u64 {
            self.fx_state.clock
        }
        /// The I3DL2 direct path of `src` into `dst`; false if the source has
        /// no direct-path shelf (the direct path is then `src`).
        pub(crate) fn direct_shelf(&mut self, src: &[f32], dst: &mut [f32], rate: u32) -> bool {
            self.fx_state.direct_shelf(&self.params.fx, src, dst, rate)
        }
        /// Sets the Doppler pitch ratio for the next render.
        #[allow(dead_code)]
        pub(crate) fn set_doppler_pitch(&mut self, ratio: f64) {
            self.fx_state.doppler = ratio;
        }
        /// Filters and envelopes `buf` (frames rendered at clocks `c0..`).
        pub(crate) fn apply_fx(&mut self, buf: &mut [f32], c0: u64, rate: u32) {
            self.fx_state.process(&self.params.fx, buf, c0, rate);
        }
    };
}

/// A static sound buffer (`IDirectSoundBuffer`).
///
/// Holds the raw bytes in the guest's format, exactly what `Lock` exposes,
/// plus a decoded float cache refreshed whenever a [`Lock`] is dropped
/// (the `Unlock` point).
#[derive(Debug)]
pub struct SoundBuffer {
    format: WaveFormat,
    data: Vec<u8>,
    decoded: Vec<f32>,
    frames: usize,
    params: VoiceParams,
    playing: bool,
    looping: bool,
    /// Loop region in frames, half-open. Defaults to the whole buffer.
    loop_region: Option<(usize, usize)>,
    /// Play cursor in frames.
    pos: f64,
    fx_state: FxState,
}

impl SoundBuffer {
    /// Creates a buffer of `size_bytes`, filled with silence. `size_bytes`
    /// must be a whole number of blocks (and for ADPCM, of 36-byte blocks
    /// per channel).
    pub fn new(format: WaveFormat, size_bytes: usize) -> Result<Self, AudioError> {
        let fill = if format.codec() == Codec::Pcm8 { 0x80 } else { 0 };
        Self::from_data(format, vec![fill; size_bytes])
    }

    /// Creates a buffer that owns `data`.
    pub fn from_data(format: WaveFormat, data: Vec<u8>) -> Result<Self, AudioError> {
        let decoded = format.decode(&data)?;
        Ok(Self {
            frames: format.frames_in(data.len()),
            params: VoiceParams::new(&format),
            format,
            data,
            decoded,
            playing: false,
            looping: false,
            loop_region: None,
            pos: 0.0,
            fx_state: FxState::default(),
        })
    }

    pub fn format(&self) -> &WaveFormat {
        &self.format
    }
    pub fn size_bytes(&self) -> usize {
        self.data.len()
    }
    pub fn frames(&self) -> usize {
        self.frames
    }
    /// Raw bytes as the guest sees them.
    pub fn bytes(&self) -> &[u8] {
        &self.data
    }

    voice_setters!();

    /// `Lock(offset, len)`. Returns a guard exposing up to two regions (the
    /// second is non-empty only if the range wraps past the end, as in
    /// DirectSound). Dropping the guard is `Unlock`. Offset and length must be
    /// multiples of the block align; `len` may not exceed the buffer.
    pub fn lock(&mut self, offset: usize, len: usize) -> Result<Lock<'_>, AudioError> {
        let size = self.data.len();
        let align = self.format.block_align();
        if offset >= size.max(1) || len > size {
            return Err(AudioError::OutOfRange { offset, len, size });
        }
        if !offset.is_multiple_of(align) || !len.is_multiple_of(align) {
            return Err(AudioError::BadLength { len: offset | len, unit: align, what: "lock alignment" });
        }
        Ok(Lock { buf: self, offset, len })
    }

    /// `Lock` with `DSBLOCK_ENTIREBUFFER`.
    pub fn lock_all(&mut self) -> Lock<'_> {
        Lock { offset: 0, len: self.data.len(), buf: self }
    }

    /// Re-decodes after the guest wrote bytes. Cannot fail: alignment was
    /// checked when the buffer was created.
    fn refresh(&mut self) {
        if let Ok(d) = self.format.decode(&self.data) {
            self.decoded = d;
        }
    }

    /// `SetLoopRegion`, in frames. The region must be non-empty and lie inside
    /// the buffer; otherwise the previous region is kept and an error returned.
    pub fn set_loop_region(&mut self, start: usize, len: usize) -> Result<(), AudioError> {
        let bad = AudioError::OutOfRange { offset: start, len, size: self.frames };
        let end = start.checked_add(len).ok_or(bad.clone())?;
        if len == 0 || end > self.frames {
            return Err(bad);
        }
        self.loop_region = Some((start, end));
        Ok(())
    }

    /// Restores looping over the whole buffer.
    pub fn clear_loop_region(&mut self) {
        self.loop_region = None;
    }

    fn region(&self) -> (usize, usize) {
        self.loop_region.unwrap_or((0, self.frames))
    }

    /// `Play`. With `looping` the loop region repeats until [`stop`](Self::stop).
    pub fn play(&mut self, looping: bool) {
        if !self.playing && self.pos == 0.0 {
            // A sound starting from its beginning starts its effects over.
            self.fx_state.reset();
        }
        self.playing = self.frames > 0;
        self.looping = looping;
    }

    /// `Stop`. The play cursor stays where it is.
    pub fn stop(&mut self) {
        self.playing = false;
    }

    pub fn is_playing(&self) -> bool {
        self.playing
    }

    /// `SetCurrentPosition`, bytes (block-aligned down).
    pub fn set_current_position(&mut self, bytes: usize) {
        let blocks = bytes / self.format.block_align();
        self.pos = (blocks * self.format.frames_per_block()).min(self.frames) as f64;
    }

    /// `GetCurrentPosition` play cursor, bytes.
    pub fn current_position(&self) -> usize {
        let blocks = self.pos as usize / self.format.frames_per_block();
        blocks * self.format.block_align()
    }

    /// Play cursor in fractional frames.
    pub fn position_frames(&self) -> f64 {
        self.pos
    }

    /// Mixes into `out` (interleaved stereo), advancing the cursor.
    pub(crate) fn render(&mut self, out: &mut [f32], host_rate: u32, gain: (f32, f32)) {
        if !self.playing {
            return;
        }
        let base_step = self.params.frequency as f64 / host_rate as f64;
        let ch = self.format.channels() as usize;
        let (loop_start, loop_end) = if self.looping { self.region() } else { (0, self.frames) };
        let end = loop_end.max(1);
        let fetch = |i: usize, looping: bool| -> [f32; 2] {
            let i = if i >= end {
                if looping { loop_start + (i - end) % (end - loop_start).max(1) } else { end - 1 }
            } else {
                i
            };
            if ch == 1 {
                let s = self.decoded[i];
                [s, s]
            } else {
                [self.decoded[i * 2], self.decoded[i * 2 + 1]]
            }
        };
        for frame in out.as_chunks_mut::<2>().0 {
            let step = base_step * self.fx_state.pitch_mult(&self.params.fx, host_rate);
            let i = self.pos.floor() as usize;
            let t = (self.pos - i as f64) as f32;
            let a = fetch(i, self.looping);
            let b = fetch(i + 1, self.looping);
            frame[0] += (a[0] + (b[0] - a[0]) * t) * gain.0;
            frame[1] += (a[1] + (b[1] - a[1]) * t) * gain.1;
            self.fx_state.clock += 1;
            self.pos += step;
            if self.pos >= end as f64 {
                if self.looping {
                    self.pos = loop_start as f64 + (self.pos - end as f64) % (end - loop_start).max(1) as f64;
                } else {
                    self.playing = false;
                    self.pos = 0.0;
                    break;
                }
            }
        }
    }
}

/// A mix-in (submix) voice: an Xbox buffer created with `DSBCAPS_MIXIN` or
/// `DSBCAPS_FXIN` that carries no samples of its own. Everything other voices
/// send to its `input_bin` (one of the FX send bins, see [`crate::route`]) is
/// summed to mono and then mixed like a voice: its volume, its own mix bins
/// and, for a 3D mix-in, the listener's panning and attenuation. Halo 2
/// positions its sounds this way: each voice's stream feeds a 3D mix-in that
/// the game moves with `SetPosition`.
///
/// Like the hardware voice it stands for, a mix-in passes audio only while
/// playing; input sent to a stopped mix-in is dropped.
#[derive(Debug)]
pub struct MixIn {
    input_bin: u32,
    params: VoiceParams,
    playing: bool,
    fx_state: FxState,
}

impl MixIn {
    pub fn new(input_bin: u32) -> Self {
        Self { input_bin, params: VoiceParams::with_frequency(0), playing: false, fx_state: FxState::default() }
    }

    voice_setters!();

    pub fn input_bin(&self) -> u32 {
        self.input_bin
    }
    pub fn play(&mut self) {
        if !self.playing {
            self.fx_state.reset();
        }
        self.playing = true;
    }
    /// Advances the effect clock by `frames` the mix-in passed audio for.
    pub(crate) fn advance_clock(&mut self, frames: u64) {
        self.fx_state.clock += frames;
    }
    pub fn stop(&mut self) {
        self.playing = false;
    }
    pub fn is_playing(&self) -> bool {
        self.playing
    }
}

/// Result of `Lock`; dropping it performs `Unlock`.
pub struct Lock<'a> {
    buf: &'a mut SoundBuffer,
    offset: usize,
    len: usize,
}

impl Lock<'_> {
    /// The locked memory as (first, second) regions. `second` is empty
    /// unless the range wrapped past the end of the buffer.
    pub fn regions(&mut self) -> (&mut [u8], &mut [u8]) {
        let size = self.buf.data.len();
        let first_len = self.len.min(size - self.offset);
        let second_len = self.len - first_len;
        let (head, tail) = self.buf.data.split_at_mut(self.offset);
        (&mut tail[..first_len], &mut head[..second_len])
    }

    /// Copies `bytes` across the regions. Fails if it is longer than the lock.
    pub fn write(&mut self, bytes: &[u8]) -> Result<(), AudioError> {
        if bytes.len() > self.len {
            return Err(AudioError::OutOfRange { offset: 0, len: bytes.len(), size: self.len });
        }
        let (a, b) = self.regions();
        let n = bytes.len().min(a.len());
        a[..n].copy_from_slice(&bytes[..n]);
        b[..bytes.len() - n].copy_from_slice(&bytes[n..]);
        Ok(())
    }
}

impl Drop for Lock<'_> {
    fn drop(&mut self) {
        self.buf.refresh();
    }
}

/// Outcome of a stream packet, mirroring `XMEDIAPACKET` status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PacketStatus {
    /// All frames were consumed by the mixer.
    Success,
    /// Dropped by [`Stream::flush`] before (fully) playing.
    Flushed,
}

/// Identifies a submitted packet, returned from [`Stream::submit`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PacketId(pub u64);

#[derive(Debug)]
struct Packet {
    id: PacketId,
    frames: Vec<f32>,
}

/// An `IDirectSoundStream` / XMO-style packet stream.
///
/// The game submits packets of audio (`Process`), the mixer consumes them in
/// order, and each is reported completed once fully played. Starvation plays
/// silence and does not advance anything.
#[derive(Debug)]
pub struct Stream {
    format: WaveFormat,
    params: VoiceParams,
    queue: VecDeque<Packet>,
    /// The last [`HISTORY_FRAMES`] frames of packets already completed, for
    /// read offsets that reach back past the front packet. Sized once, so
    /// rendering never allocates for it.
    history: VecDeque<[f32; 2]>,
    /// Fractional frame position within the front packet.
    pos: f64,
    paused: bool,
    next_id: u64,
    completed: Vec<(PacketId, PacketStatus)>,
    fx_state: FxState,
    /// The mixer frame at which it ran out of packets while playing, until
    /// it plays again (the mixer's starvation count).
    pub(crate) dry_at: Option<u64>,
}

impl Stream {
    pub fn new(format: WaveFormat) -> Result<Self, AudioError> {
        Ok(Self {
            params: VoiceParams::new(&format),
            format,
            queue: VecDeque::new(),
            history: VecDeque::with_capacity(HISTORY_FRAMES + 1),
            pos: 0.0,
            paused: false,
            next_id: 1,
            completed: Vec::new(),
            fx_state: FxState::default(),
            dry_at: None,
        })
    }

    voice_setters!();

    pub fn format(&self) -> &WaveFormat {
        &self.format
    }

    /// Queues a packet of raw bytes in the stream's format.
    pub fn submit(&mut self, data: &[u8]) -> Result<PacketId, AudioError> {
        let frames = self.format.decode(data)?;
        let id = PacketId(self.next_id);
        self.next_id += 1;
        if frames.is_empty() {
            self.completed.push((id, PacketStatus::Success));
        } else {
            self.queue.push_back(Packet { id, frames });
        }
        Ok(id)
    }

    /// Pauses or resumes consumption.
    pub fn pause(&mut self, paused: bool) {
        self.paused = paused;
    }

    /// `Flush`: drops everything queued, completing it as [`PacketStatus::Flushed`].
    pub fn flush(&mut self) {
        for p in self.queue.drain(..) {
            self.completed.push((p.id, PacketStatus::Flushed));
        }
        self.pos = 0.0;
        self.history.clear();
        self.dry_at = None;
        // What plays next is a new sound: effects start over.
        self.fx_state.reset();
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// Something would be heard: not paused and a packet is queued.
    pub fn is_audible(&self) -> bool {
        !self.paused && !self.queue.is_empty()
    }

    /// Packets queued and not yet finished.
    pub fn pending(&self) -> usize {
        self.queue.len()
    }

    /// Takes the completion notifications since the last call, oldest first.
    pub fn take_completed(&mut self) -> Vec<(PacketId, PacketStatus)> {
        std::mem::take(&mut self.completed)
    }

    /// Frame `i` counted from the start of the front packet, continuing into
    /// the packets behind it; the last queued frame if `i` runs past them all.
    fn frame_rel(&self, i: i64) -> [f32; 2] {
        if i < 0 {
            // Before the front packet: the tail of what already played, or
            // the front packet's first frame if there is not that much.
            let back = i.unsigned_abs() as usize;
            return match self.history.len().checked_sub(back) {
                Some(k) => self.history[k],
                None => self.history.front().copied().or_else(|| self.frame(0, 0)).unwrap_or([0.0; 2]),
            };
        }
        let mut i = i as usize;
        for k in 0..self.queue.len() {
            let n = self.queue[k].frames.len() / self.format.channels() as usize;
            if i < n {
                return self.frame(k, i).unwrap_or([0.0; 2]);
            }
            i -= n;
        }
        self.queue
            .back()
            .and_then(|p| {
                let n = p.frames.len() / self.format.channels() as usize;
                self.frame(self.queue.len() - 1, n.saturating_sub(1))
            })
            .unwrap_or([0.0; 2])
    }

    fn frame(&self, packet: usize, i: usize) -> Option<[f32; 2]> {
        let p = self.queue.get(packet)?;
        let ch = self.format.channels() as usize;
        let n = p.frames.len() / ch;
        (i < n).then(|| {
            if ch == 1 { [p.frames[i]; 2] } else { [p.frames[i * 2], p.frames[i * 2 + 1]] }
        })
    }

    /// Test access to the read-position offset pitch modulation has built up.
    #[cfg(test)]
    pub(crate) fn phase_offset(&self) -> f64 {
        self.fx_state.phase_off
    }

    pub(crate) fn render(&mut self, out: &mut [f32], host_rate: u32, gain: (f32, f32)) {
        if self.paused {
            return;
        }
        let base_step = self.params.frequency as f64 / host_rate as f64;
        let ch = self.format.channels() as usize;
        for frame in out.as_chunks_mut::<2>().0 {
            if self.queue.is_empty() {
                break;
            }
            // Pitch modulation (LFO, Doppler) never changes how fast packets
            // are consumed: `pos` advances at the nominal rate, so packets
            // complete exactly when they would without effects. The
            // modulation moves only the read position, by an offset that
            // accumulates `(ratio - 1)` per frame and is bounded.
            let m = self.fx_state.pitch_mult(&self.params.fx, host_rate);
            let off = self.fx_state.phase_off;
            let step = base_step;
            let (a, b, t) = if m == 1.0 && off == 0.0 {
                let i = self.pos.floor() as usize;
                let t = (self.pos - i as f64) as f32;
                let a = self.frame(0, i).unwrap_or([0.0; 2]);
                // Interpolate toward the next packet's first frame at a
                // boundary so packet joins are seamless.
                let b = self.frame(0, i + 1).or_else(|| self.frame(1, 0)).unwrap_or(a);
                (a, b, t)
            } else {
                let p = self.pos + off;
                let i = p.floor() as i64;
                (self.frame_rel(i), self.frame_rel(i + 1), (p - i as f64) as f32)
            };
            if m != 1.0 {
                self.fx_state.phase_off = (off + (m - 1.0) * base_step).clamp(-MAX_PHASE_OFFSET, MAX_PHASE_OFFSET);
            }
            frame[0] += (a[0] + (b[0] - a[0]) * t) * gain.0;
            frame[1] += (a[1] + (b[1] - a[1]) * t) * gain.1;
            self.fx_state.clock += 1;
            self.pos += step;
            // Consume every packet boundary crossed (downsampling can cross
            // several per output frame), keeping the fractional position.
            while let Some(front) = self.queue.front() {
                let n = front.frames.len() / ch;
                if self.pos < n as f64 {
                    break;
                }
                self.pos -= n as f64;
                let done = self.queue.pop_front().expect("front exists");
                let first = n.saturating_sub(HISTORY_FRAMES);
                for k in first..n {
                    if self.history.len() >= HISTORY_FRAMES {
                        self.history.pop_front();
                    }
                    self.history.push_back(if ch == 1 {
                        [done.frames[k]; 2]
                    } else {
                        [done.frames[k * 2], done.frames[k * 2 + 1]]
                    });
                }
                self.completed.push((done.id, PacketStatus::Success));
            }
            if self.queue.is_empty() {
                self.pos = 0.0;
            }
        }
    }
}

/// Frames of completed-packet history a stream keeps for pitch modulation: the
/// phase-offset bound plus room to interpolate.
pub const HISTORY_FRAMES: usize = MAX_PHASE_OFFSET as usize + 2;

/// A 3-vector in DirectSound's left-handed space (x right, y up, z forward).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }
    fn sub(self, o: Self) -> Self {
        Self::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
    fn dot(self, o: Self) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }
    fn cross(self, o: Self) -> Self {
        Self::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }
    fn len(self) -> f32 {
        self.dot(self).sqrt()
    }
    fn scale(self, k: f32) -> Self {
        Self::new(self.x * k, self.y * k, self.z * k)
    }
    /// Unit vector, or `None` for a (near) zero or non-finite vector.
    fn unit(self) -> Option<Self> {
        let l = self.len();
        (l.is_finite() && l > 1e-6).then(|| self.scale(1.0 / l))
    }
}

/// How a 3D source's position is interpreted (`DS3DMODE_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Mode3d {
    #[default]
    Normal,
    /// Position is relative to the listener.
    HeadRelative,
    /// 3D processing off; the voice is mixed as a plain voice.
    Disabled,
}

/// `DS3D_DEFAULTCONEANGLE`: a 360 degree cone, which means no cone at all.
pub const CONE_ANGLE_NONE: u32 = 360;

/// A positional sound source (`DS3DBUFFER`).
///
/// See [`Listener::spatial`] for what each field does to the mix.
#[derive(Clone, Debug, PartialEq)]
pub struct Source3d {
    pub position: Vec3,
    /// Units per second; with the doppler factors it sets the pitch shift
    /// (see [`Listener::spatial`]).
    pub velocity: Vec3,
    /// Inside this distance the source is at full volume.
    pub min_distance: f32,
    /// Beyond this distance the source gets no quieter.
    pub max_distance: f32,
    /// Per-source rolloff, multiplied with the listener's. 0 turns distance
    /// attenuation off (Halo 2 does its own and passes 0).
    pub rolloff_factor: f32,
    /// Per-source Doppler scale, multiplied with the listener's.
    pub doppler_factor: f32,
    /// Inside and outside cone angles in degrees (`SetConeAngles`).
    pub cone_inside: u32,
    pub cone_outside: u32,
    /// Direction the cone points (`SetConeOrientation`).
    pub cone_orientation: Vec3,
    /// Volume outside the outer cone, millibels (`SetConeOutsideVolume`).
    pub cone_outside_volume_mb: i32,
    pub mode: Mode3d,
}

impl Default for Source3d {
    fn default() -> Self {
        Self {
            position: Vec3::default(),
            velocity: Vec3::default(),
            min_distance: 1.0,
            max_distance: 1.0e9,
            rolloff_factor: 1.0,
            doppler_factor: 1.0,
            cone_inside: CONE_ANGLE_NONE,
            cone_outside: CONE_ANGLE_NONE,
            cone_orientation: Vec3::new(0.0, 0.0, 1.0),
            cone_outside_volume_mb: 0,
            mode: Mode3d::Normal,
        }
    }
}

/// What a 3D source gets from the listener: see [`Listener::spatial`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spatial {
    /// Distance and cone attenuation, linear.
    pub gain: f32,
    /// Panning gains, linear, each at most 1 (1 and 1 for a centred source).
    pub left: f32,
    pub right: f32,
    /// 1 straight ahead, 0 straight behind, 0.5 to the side: how much of the
    /// voice goes to the front crosstalk mix bins rather than the back ones.
    pub front: f32,
    /// Doppler playback-rate ratio (1 for a source at rest relative to the
    /// listener); see [`Listener::spatial`].
    pub pitch: f64,
}

impl Spatial {
    /// No 3D processing.
    pub const DRY: Self = Self { gain: 1.0, left: 1.0, right: 1.0, front: 1.0, pitch: 1.0 };
}

/// The speed of sound Doppler uses, metres per second.
pub const SPEED_OF_SOUND: f32 = 343.0;

/// How much of a hard-left or hard-right source still reaches the far ear:
/// the pan position is scaled by this before the pan law, so a lateral source
/// leaves the far channel about 16 dB down instead of silent (heads do not
/// fully shadow sound; this stands in for the HRTF the MCPX DSP ran).
pub const PAN_WIDTH: f32 = 0.8;

/// The 3D listener (`IDirectSound::SetPosition` & co.).
///
/// The Xbox ran HRTF and crosstalk cancellation for 3D voices on the MCPX
/// DSP. That is not emulated; [`spatial`](Self::spatial) is a simple,
/// documented stand-in. Not modelled: HRTF elevation and front/back colouring,
/// and the SDK's `DS3D_DEFERRED` commit semantics (every change is
/// immediate). Occlusion, obstruction and reverb sends are per-voice
/// ([`crate::fx::I3dl2Source`]) and per-mixer ([`crate::reverb`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Listener {
    pub position: Vec3,
    pub front: Vec3,
    pub top: Vec3,
    pub velocity: Vec3,
    /// Metres per world unit. In DirectSound 3D this only scales velocities
    /// for Doppler.
    pub distance_factor: f32,
    /// Global rolloff, multiplied with each source's. 0 disables attenuation.
    pub rolloff_factor: f32,
    /// Global Doppler scale, multiplied with each source's. 0 turns the
    /// shift off.
    pub doppler_factor: f32,
}

impl Default for Listener {
    fn default() -> Self {
        Self {
            position: Vec3::default(),
            front: Vec3::new(0.0, 0.0, 1.0),
            top: Vec3::new(0.0, 1.0, 0.0),
            velocity: Vec3::default(),
            distance_factor: 1.0,
            rolloff_factor: 1.0,
            doppler_factor: 1.0,
        }
    }
}

impl Listener {
    /// The listener's right, up and front unit axes. The game's front and top
    /// need be neither unit length nor exactly orthogonal; a degenerate pair
    /// falls back to the default orientation.
    fn axes(&self) -> (Vec3, Vec3, Vec3) {
        let default = (Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 0.0, 1.0));
        let Some(front) = self.front.unit() else { return default };
        // Left-handed: right = top x front (front +z, top +y gives +x).
        let Some(right) = self.top.cross(front).unit() else { return default };
        (right, front.cross(right), front)
    }

    /// The panning and attenuation a source gets. The model, in full:
    ///
    /// - **Direction.** The source position relative to the listener (or as
    ///   given, for [`Mode3d::HeadRelative`]) is projected on the listener's
    ///   right axis: `p = x / distance` in `-1..=1` (front and back fold
    ///   together). With `p' = p * PAN_WIDTH` and `t = (p' + 1) * pi / 4`,
    ///   `left = min(1, sqrt(2) cos t)` and `right = min(1, sqrt(2) sin t)`:
    ///   the sine/cosine equal-power law, scaled so a centred source keeps
    ///   unity gain in both channels. A source at the listener is centred.
    /// - **Front/back.** `front = (1 + z / distance) / 2` splits the voice
    ///   between the front and back crosstalk mix bins (see `crate::route`).
    /// - **Distance.** The DirectSound 3D inverse model, in world units:
    ///   `d = min(distance, max_distance)`; full volume up to
    ///   `min_distance`, then `min / (min + rolloff * (d - min))` with
    ///   `rolloff = listener.rolloff_factor * source.rolloff_factor`.
    /// - **Cone.** With angle `a` between the cone orientation and the
    ///   direction from the source to the listener: 0 mB inside half the
    ///   inner angle, `cone_outside_volume_mb` beyond half the outer angle,
    ///   linear in millibels between (DirectSound3D's cone model). Only a
    ///   360 degree inner angle (the default) or a zero orientation means no
    ///   cone; a 360 degree outer angle with a narrower inner one fades all
    ///   the way round, reaching the outside volume directly behind.
    /// - **Doppler.** With `u` the unit vector from the source to the
    ///   listener, `vl` and `vs` the listener's and the source's velocities
    ///   along `u` converted to metres per second (times `distance_factor`)
    ///   and `k = listener.doppler_factor * source.doppler_factor`, the
    ///   playback-rate ratio is `(c - k vl) / (c - k vs)` with `c` =
    ///   [`SPEED_OF_SOUND`]; speeds are limited to `0.9 c / k` so the ratio
    ///   stays finite. A head-relative or 3D-disabled source has none.
    pub fn spatial(&self, src: &Source3d) -> Spatial {
        if src.mode == Mode3d::Disabled {
            return Spatial::DRY;
        }
        let rel = match src.mode {
            Mode3d::HeadRelative => src.position,
            _ => src.position.sub(self.position),
        };
        let dist = rel.len();
        let (right_axis, _, front_axis) = self.axes();
        let (p, front) = match rel.unit() {
            Some(dir) => (dir.dot(right_axis).clamp(-1.0, 1.0), (1.0 + dir.dot(front_axis)) * 0.5),
            None => (0.0, 1.0),
        };
        // In f64 so a centred source comes out at exactly 1.
        let t = (f64::from(p * PAN_WIDTH) + 1.0) * std::f64::consts::FRAC_PI_4;
        let left = (std::f64::consts::SQRT_2 * t.cos()).min(1.0) as f32;
        let right = (std::f64::consts::SQRT_2 * t.sin()).min(1.0) as f32;

        let min = src.min_distance.max(1e-6);
        let d = if dist.is_finite() { dist.min(src.max_distance.max(min)) } else { min };
        let rolloff = (self.rolloff_factor * src.rolloff_factor).max(0.0);
        let distance_gain = if d <= min { 1.0 } else { min / (min + rolloff * (d - min)) };

        let pitch = self.doppler(src, rel);
        let out = Spatial { gain: distance_gain * cone_gain(src, rel), left, right, front: front.clamp(0.0, 1.0), pitch };
        // A NaN position, distance or cone from the guest must not reach the
        // mix: such a source is heard unprocessed.
        if [out.gain, out.left, out.right, out.front].iter().all(|v| v.is_finite()) {
            out
        } else {
            Spatial { pitch, ..Spatial::DRY }
        }
    }
}

impl Listener {
    /// The Doppler term of [`Listener::spatial`]; `rel` points from the
    /// listener to the source.
    fn doppler(&self, src: &Source3d, rel: Vec3) -> f64 {
        // DirectSound limits the factor to 0..=10; anything else (NaN,
        // infinity) is clamped or means none.
        let clean = |v: f32, max: f32| if v.is_nan() { 0.0 } else { v.clamp(0.0, max) };
        let k = clean(self.doppler_factor, 10.0) * clean(src.doppler_factor, 10.0);
        if k == 0.0 || src.mode != Mode3d::Normal {
            return 1.0;
        }
        let Some(to_listener) = rel.scale(-1.0).unit() else { return 1.0 };
        let limit = 0.9 * SPEED_OF_SOUND / k;
        let metres = clean(self.distance_factor, 1.0e6);
        let along = |v: Vec3| {
            let d = v.dot(to_listener) * metres;
            if d.is_nan() { 0.0 } else { d.clamp(-limit, limit) }
        };
        let ratio = f64::from((SPEED_OF_SOUND - k * along(self.velocity)) / (SPEED_OF_SOUND - k * along(src.velocity)));
        if ratio.is_finite() && ratio > 0.0 { ratio.clamp(0.125, 8.0) } else { 1.0 }
    }
}

/// The cone term of [`Listener::spatial`]. `rel` points from the listener to
/// the source, so the source "sees" the listener along `-rel`.
fn cone_gain(src: &Source3d, rel: Vec3) -> f32 {
    let outside = src.cone_outside.min(CONE_ANGLE_NONE);
    let inside = src.cone_inside.min(outside);
    if inside >= CONE_ANGLE_NONE {
        return 1.0;
    }
    let (Some(axis), Some(to_listener)) = (src.cone_orientation.unit(), rel.scale(-1.0).unit()) else {
        return 1.0;
    };
    let angle = axis.dot(to_listener).clamp(-1.0, 1.0).acos().to_degrees();
    let (half_in, half_out) = (inside as f32 / 2.0, outside as f32 / 2.0);
    let mb = if angle <= half_in {
        0.0
    } else if angle >= half_out || half_out <= half_in {
        src.cone_outside_volume_mb as f32
    } else {
        src.cone_outside_volume_mb as f32 * (angle - half_in) / (half_out - half_in)
    };
    mb_to_gain(mb.round() as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mono16(samples: &[i16]) -> Vec<u8> {
        samples.iter().flat_map(|s| s.to_le_bytes()).collect()
    }

    fn render(b: &mut SoundBuffer, frames: usize, rate: u32) -> Vec<f32> {
        let mut out = vec![0.0; frames * 2];
        let g = b.params().gains();
        b.render(&mut out, rate, g);
        out
    }

    #[test]
    fn gains_millibels() {
        assert_eq!(mb_to_gain(0), 1.0);
        assert_eq!(mb_to_gain(-10_000), 0.0);
        // -600 mB = -6 dB ~ half amplitude.
        assert!((mb_to_gain(-600) - 0.501).abs() < 0.01);
        let mut p = VoiceParams::new(&WaveFormat::pcm16(1, 8000).unwrap());
        p.pan_mb = 2000;
        let (l, r) = p.gains();
        assert_eq!(r, 1.0);
        assert!((l - 0.1).abs() < 1e-4);
        p.pan_mb = -2000;
        let (l, r) = p.gains();
        assert_eq!(l, 1.0);
        assert!((r - 0.1).abs() < 1e-4);
    }

    #[test]
    fn lock_wraps_into_second_region() {
        let mut b = SoundBuffer::new(WaveFormat::pcm16(1, 8000).unwrap(), 16).unwrap();
        {
            let mut l = b.lock(12, 8).unwrap();
            let (a, c) = l.regions();
            assert_eq!((a.len(), c.len()), (4, 4));
            l.write(&[1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
        }
        assert_eq!(&b.bytes()[12..], &[1, 2, 3, 4]);
        assert_eq!(&b.bytes()[..4], &[5, 6, 7, 8]);
    }

    #[test]
    fn lock_validates() {
        let mut b = SoundBuffer::new(WaveFormat::pcm16(2, 8000).unwrap(), 16).unwrap();
        assert!(b.lock(16, 4).is_err());
        assert!(b.lock(0, 17).is_err());
        assert!(b.lock(2, 4).is_err()); // not frame aligned (4 bytes)
        let mut a = SoundBuffer::new(WaveFormat::xbox_adpcm(1, 8000).unwrap(), 72).unwrap();
        assert!(a.lock(0, 36).is_ok());
        assert!(a.lock(0, 20).is_err());
    }

    #[test]
    fn unlock_refreshes_decoded_audio() {
        let mut b = SoundBuffer::new(WaveFormat::pcm16(1, 8000).unwrap(), 4).unwrap();
        b.lock_all().write(&mono16(&[16384, 16384])).unwrap();
        b.play(false);
        let out = render(&mut b, 1, 8000);
        assert_eq!(out[0], 0.5);
    }

    #[test]
    fn pcm8_new_buffer_is_silent() {
        let mut b = SoundBuffer::new(WaveFormat::pcm8(1, 8000).unwrap(), 4).unwrap();
        b.play(false);
        assert_eq!(render(&mut b, 2, 8000), vec![0.0; 4]);
    }

    #[test]
    fn plays_once_then_stops_and_rewinds() {
        let mut b = SoundBuffer::from_data(WaveFormat::pcm16(1, 8000).unwrap(), mono16(&[8192; 4])).unwrap();
        b.play(false);
        assert!(b.is_playing());
        let out = render(&mut b, 8, 8000);
        assert!(!b.is_playing());
        assert_eq!(b.current_position(), 0);
        assert_eq!(&out[..8], &[0.25; 8]);
        assert_eq!(&out[8..], &[0.0; 8]);
    }

    #[test]
    fn looping_wraps_within_loop_region() {
        let mut b =
            SoundBuffer::from_data(WaveFormat::pcm16(1, 8000).unwrap(), mono16(&[0, 1000, 2000, 3000])).unwrap();
        b.set_loop_region(1, 2).unwrap();
        b.set_current_position(2);
        b.play(true);
        let out = render(&mut b, 6, 8000);
        let l: Vec<f32> = out.iter().step_by(2).map(|v| (v * 32768.0).round()).collect();
        // Starts at frame 1, plays 1,2 then loops 1,2,1,2.
        assert_eq!(l, vec![1000.0, 2000.0, 1000.0, 2000.0, 1000.0, 2000.0]);
        assert!(b.is_playing());
    }

    #[test]
    fn frequency_scales_playback_speed() {
        let mut b = SoundBuffer::from_data(WaveFormat::pcm16(1, 8000).unwrap(), mono16(&[0, 100, 200, 300, 400, 500, 600, 700])).unwrap();
        b.set_frequency(16000); // 2x
        b.play(false);
        let out = render(&mut b, 3, 8000);
        let l: Vec<f32> = out.iter().step_by(2).map(|v| (v * 32768.0).round()).collect();
        assert_eq!(l, vec![0.0, 200.0, 400.0]);
    }

    #[test]
    fn position_roundtrips_in_bytes() {
        let mut b = SoundBuffer::new(WaveFormat::xbox_adpcm(2, 44100).unwrap(), 72 * 4).unwrap();
        b.set_current_position(72 * 2 + 10);
        assert_eq!(b.current_position(), 72 * 2);
        assert_eq!(b.position_frames(), 128.0);
    }

    #[test]
    fn stream_completes_packets_in_order() {
        let mut s = Stream::new(WaveFormat::pcm16(1, 8000).unwrap()).unwrap();
        let a = s.submit(&mono16(&[16384; 4])).unwrap();
        let b = s.submit(&mono16(&[-16384; 4])).unwrap();
        let mut out = [0.0; 12 * 2];
        let g = s.params().gains();
        s.render(&mut out[..2 * 5], 8000, g); // 5 frames: all of a, 1 of b
        assert_eq!(s.take_completed(), vec![(a, PacketStatus::Success)]);
        s.render(&mut out[2 * 5..], 8000, g);
        assert_eq!(s.take_completed(), vec![(b, PacketStatus::Success)]);
        assert_eq!(out[0], 0.5);
        assert_eq!(out[2 * 4], -0.5);
        // Starved: the last 5 frames stay silent.
        assert!(out[2 * 8..].iter().all(|&v| v == 0.0));
    }

    #[test]
    fn empty_or_out_of_range_loop_regions_are_rejected() {
        let mut b = SoundBuffer::from_data(WaveFormat::pcm16(1, 8000).unwrap(), mono16(&[0, 1, 2, 3])).unwrap();
        assert!(b.set_loop_region(4, 1).is_err());
        assert!(b.set_loop_region(1, usize::MAX).is_err());
        assert!(b.set_loop_region(2, 0).is_err());
        assert!(b.set_loop_region(3, 2).is_err());
        b.play(true);
        let out = render(&mut b, 8, 8000); // must not panic
        assert_eq!(out.len(), 16);
        b.set_loop_region(3, 1).unwrap();
        b.clear_loop_region();
    }

    #[test]
    fn stream_downsampling_crosses_several_packets_per_frame() {
        let mut s = Stream::new(WaveFormat::pcm16(1, 48000).unwrap()).unwrap();
        for v in [1000i16, 2000, 3000, 4000] {
            s.submit(&mono16(&[v])).unwrap();
        }
        let mut out = vec![0.0; 4];
        s.render(&mut out, 24000, (1.0, 1.0));
        let l: Vec<f32> = out.iter().step_by(2).map(|v| (v * 32768.0).round()).collect();
        assert_eq!(l, vec![1000.0, 3000.0]);
        assert_eq!(s.pending(), 0);
        assert_eq!(s.take_completed().len(), 4);
    }

    #[test]
    fn stream_pause_and_flush() {
        let mut s = Stream::new(WaveFormat::pcm16(1, 8000).unwrap()).unwrap();
        let a = s.submit(&mono16(&[1000; 8])).unwrap();
        s.pause(true);
        let mut out = vec![0.0; 8];
        s.render(&mut out, 8000, (1.0, 1.0));
        assert!(out.iter().all(|&v| v == 0.0));
        assert_eq!(s.pending(), 1);
        s.flush();
        assert_eq!(s.take_completed(), vec![(a, PacketStatus::Flushed)]);
        assert_eq!(s.pending(), 0);
        assert!(s.submit(&[0]).is_err()); // half a frame
    }

    #[test]
    fn listener_attenuates_and_pans() {
        let l = Listener::default();
        let near = Source3d { position: Vec3::new(0.5, 0.0, 0.0), ..Default::default() };
        assert_eq!(l.spatial(&near).gain, 1.0);
        let far = Source3d { position: Vec3::new(5.0, 0.0, 0.0), ..Default::default() };
        let s = l.spatial(&far);
        assert!((s.gain - 0.2).abs() < 1e-5); // 1 / (1 + 4)
        // Hard right: right ear full, left ear sqrt(2) cos(0.45 pi) ~ -16 dB.
        assert_eq!(s.right, 1.0);
        assert!((s.left - 0.2212).abs() < 1e-3, "{s:?}");
        assert!((s.front - 0.5).abs() < 1e-6);
        let left = l.spatial(&Source3d { position: Vec3::new(-2.0, 0.0, 0.0), ..Default::default() });
        assert_eq!(left.left, 1.0);
        assert!((left.right - 0.2212).abs() < 1e-3);
        let ahead = l.spatial(&Source3d { position: Vec3::new(0.0, 0.0, 3.0), ..Default::default() });
        assert_eq!((ahead.left, ahead.right, ahead.front), (1.0, 1.0, 1.0));
        let behind = l.spatial(&Source3d { position: Vec3::new(0.0, 0.0, -3.0), ..Default::default() });
        assert_eq!((behind.left, behind.right, behind.front), (1.0, 1.0, 0.0));
        let off = Source3d { mode: Mode3d::Disabled, position: Vec3::new(9.0, 0.0, 0.0), ..Default::default() };
        assert_eq!(l.spatial(&off), Spatial::DRY);
        let here = l.spatial(&Source3d::default());
        assert_eq!((here.gain, here.left, here.right), (1.0, 1.0, 1.0));
    }

    #[test]
    fn listener_orientation_and_position_rotate_the_pan() {
        // Listener at (10, 0, 0) facing +x with y up: right is -z.
        let l = Listener {
            position: Vec3::new(10.0, 0.0, 0.0),
            front: Vec3::new(2.0, 0.0, 0.0), // not unit length
            top: Vec3::new(0.0, 3.0, 0.0),
            ..Default::default()
        };
        let src = |x, z| Source3d { position: Vec3::new(x, 0.0, z), ..Default::default() };
        let r = l.spatial(&src(10.0, -4.0));
        assert_eq!(r.right, 1.0);
        assert!(r.left < 0.3);
        let ahead = l.spatial(&src(14.0, 0.0));
        assert_eq!((ahead.left, ahead.right, ahead.front), (1.0, 1.0, 1.0));
        // A degenerate orientation falls back to the default axes.
        let flat = Listener { front: Vec3::default(), ..Default::default() };
        assert_eq!(flat.spatial(&src(3.0, 0.0)).right, 1.0);
    }

    #[test]
    fn distance_model_respects_rolloff_and_max_distance() {
        let l = Listener::default();
        let mut s = Source3d { position: Vec3::new(0.0, 0.0, 9.0), min_distance: 1.0, max_distance: 5.0, ..Default::default() };
        // Clamped at max: 1 / (1 + 4).
        assert!((l.spatial(&s).gain - 0.2).abs() < 1e-6);
        s.rolloff_factor = 0.0; // Halo 2's setting: no attenuation.
        assert_eq!(l.spatial(&s).gain, 1.0);
        s.rolloff_factor = 2.0;
        assert!((l.spatial(&s).gain - 1.0 / 9.0).abs() < 1e-6);
        let quiet = Listener { rolloff_factor: 0.5, ..Default::default() };
        assert!((quiet.spatial(&s).gain - 0.2).abs() < 1e-6);
    }

    #[test]
    fn cone_attenuates_outside_listeners() {
        let l = Listener::default();
        // Source 4 units ahead of the listener; the listener is at angle 0
        // from a cone pointing back at it (-z).
        let mut s = Source3d {
            position: Vec3::new(0.0, 0.0, 4.0),
            rolloff_factor: 0.0,
            cone_inside: 90,
            cone_outside: 180,
            cone_orientation: Vec3::new(0.0, 0.0, -1.0),
            cone_outside_volume_mb: -2000,
            ..Default::default()
        };
        assert_eq!(l.spatial(&s).gain, 1.0);
        // Pointing away: the listener is outside the outer cone.
        s.cone_orientation = Vec3::new(0.0, 0.0, 1.0);
        assert!((l.spatial(&s).gain - mb_to_gain(-2000)).abs() < 1e-6);
        // Pointing sideways: 90 degrees, half way between 45 and 90.
        s.cone_orientation = Vec3::new(1.0, 0.0, 0.0);
        assert!((l.spatial(&s).gain - mb_to_gain(-2000)).abs() < 1e-6);
        s.cone_orientation = Vec3::new(1.0, 0.0, -1.0); // 45 degrees off: inside
        assert!((l.spatial(&s).gain - 1.0).abs() < 1e-6);
        s.cone_orientation = Vec3::new(0.577_35, 0.0, -1.0); // 30 deg: inside
        assert_eq!(l.spatial(&s).gain, 1.0);
        let tan_67_5 = 2.414_213_6;
        s.cone_orientation = Vec3::new(tan_67_5, 0.0, -1.0); // 67.5: halfway
        assert!((l.spatial(&s).gain - mb_to_gain(-1000)).abs() < 1e-4, "{:?}", l.spatial(&s));
        // The default 360 degree cone, or no orientation, means no cone.
        s.cone_inside = CONE_ANGLE_NONE;
        s.cone_outside = CONE_ANGLE_NONE;
        assert_eq!(l.spatial(&s).gain, 1.0);
        s.cone_inside = 90;
        s.cone_outside = 180;
        s.cone_orientation = Vec3::default();
        assert_eq!(l.spatial(&s).gain, 1.0);
    }

    #[test]
    fn a_full_outer_cone_still_fades_outside_a_narrow_inner_one() {
        let l = Listener::default();
        // Source ahead of the listener, cone pointing away: the listener is
        // directly behind it (180 degrees).
        let mut s = Source3d {
            position: Vec3::new(0.0, 0.0, 1.0),
            rolloff_factor: 0.0,
            cone_inside: 90,
            cone_outside: CONE_ANGLE_NONE,
            cone_orientation: Vec3::new(0.0, 0.0, 1.0),
            cone_outside_volume_mb: -6000,
            ..Default::default()
        };
        assert!((l.spatial(&s).gain - mb_to_gain(-6000)).abs() < 1e-6, "{:?}", l.spatial(&s));
        // Sideways (90 degrees): 45/135 of the way from 45 to 180.
        s.cone_orientation = Vec3::new(1.0, 0.0, 0.0);
        let want = mb_to_gain((-6000.0f32 * 45.0 / 135.0).round() as i32);
        assert!((l.spatial(&s).gain - want).abs() < 1e-4, "{:?}", l.spatial(&s));
        // Facing the listener: inside.
        s.cone_orientation = Vec3::new(0.0, 0.0, -1.0);
        assert_eq!(l.spatial(&s).gain, 1.0);
    }
}
