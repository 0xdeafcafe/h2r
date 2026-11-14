//! A DirectSound-shaped model: buffers, streams and a 3D stub.
//!
//! This mirrors the *semantics* the Xbox SDK's `IDirectSoundBuffer` and
//! `IDirectSoundStream` expose, in typed Rust. There is no COM, no vtable and
//! no APU voice allocation; a guest-side shim (owned elsewhere) translates
//! calls into these methods. Units follow the SDK on purpose so the shim is a
//! thin mapping: volume and pan are in millibels (1/100 dB), frequency in Hz,
//! positions in bytes at the buffer API and frames internally.

use std::collections::VecDeque;

use crate::{AudioError, format::{Codec, WaveFormat}};

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
}

impl VoiceParams {
    fn new(format: &WaveFormat) -> Self {
        Self { frequency: format.sample_rate(), volume_mb: 0, pan_mb: 0, spatial: None }
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
        pub fn params(&self) -> &VoiceParams {
            &self.params
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
        let step = self.params.frequency as f64 / host_rate as f64;
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
            let i = self.pos.floor() as usize;
            let t = (self.pos - i as f64) as f32;
            let a = fetch(i, self.looping);
            let b = fetch(i + 1, self.looping);
            frame[0] += (a[0] + (b[0] - a[0]) * t) * gain.0;
            frame[1] += (a[1] + (b[1] - a[1]) * t) * gain.1;
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
    /// Fractional frame position within the front packet.
    pos: f64,
    paused: bool,
    next_id: u64,
    completed: Vec<(PacketId, PacketStatus)>,
}

impl Stream {
    pub fn new(format: WaveFormat) -> Result<Self, AudioError> {
        Ok(Self {
            params: VoiceParams::new(&format),
            format,
            queue: VecDeque::new(),
            pos: 0.0,
            paused: false,
            next_id: 1,
            completed: Vec::new(),
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
    }

    /// Packets queued and not yet finished.
    pub fn pending(&self) -> usize {
        self.queue.len()
    }

    /// Takes the completion notifications since the last call, oldest first.
    pub fn take_completed(&mut self) -> Vec<(PacketId, PacketStatus)> {
        std::mem::take(&mut self.completed)
    }

    fn frame(&self, packet: usize, i: usize) -> Option<[f32; 2]> {
        let p = self.queue.get(packet)?;
        let ch = self.format.channels() as usize;
        let n = p.frames.len() / ch;
        (i < n).then(|| {
            if ch == 1 { [p.frames[i]; 2] } else { [p.frames[i * 2], p.frames[i * 2 + 1]] }
        })
    }

    pub(crate) fn render(&mut self, out: &mut [f32], host_rate: u32, gain: (f32, f32)) {
        if self.paused {
            return;
        }
        let step = self.params.frequency as f64 / host_rate as f64;
        let ch = self.format.channels() as usize;
        for frame in out.as_chunks_mut::<2>().0 {
            if self.queue.is_empty() {
                break;
            }
            let i = self.pos.floor() as usize;
            let t = (self.pos - i as f64) as f32;
            let a = self.frame(0, i).unwrap_or([0.0; 2]);
            // Interpolate toward the next packet's first frame at a boundary
            // so packet joins are seamless.
            let b = self.frame(0, i + 1).or_else(|| self.frame(1, 0)).unwrap_or(a);
            frame[0] += (a[0] + (b[0] - a[0]) * t) * gain.0;
            frame[1] += (a[1] + (b[1] - a[1]) * t) * gain.1;
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
                self.completed.push((done.id, PacketStatus::Success));
            }
            if self.queue.is_empty() {
                self.pos = 0.0;
            }
        }
    }
}

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

/// A positional sound source (`DS3DBUFFER`, reduced).
///
/// Stub: velocity, cones and `DSBCAPS_CTRL3D` min/max-distance hardware
/// behaviour are not modelled; see [`Listener::spatialize`].
#[derive(Clone, Debug, PartialEq)]
pub struct Source3d {
    pub position: Vec3,
    pub velocity: Vec3,
    pub min_distance: f32,
    pub max_distance: f32,
    pub mode: Mode3d,
}

impl Default for Source3d {
    fn default() -> Self {
        Self {
            position: Vec3::default(),
            velocity: Vec3::default(),
            min_distance: 1.0,
            max_distance: 1.0e9,
            mode: Mode3d::Normal,
        }
    }
}

/// The 3D listener (`IDirectSound::SetPosition` & co.).
///
/// # What is missing
///
/// This is a stub meant to make positional sounds audible, not to match the
/// hardware. Not modelled: Doppler shift (velocity is stored, never used),
/// inside/outside sound cones, HRTF and the XTalk crosstalk cancellation
/// (those ran on the MCPX DSP), occlusion/obstruction, reverb sends, and the
/// SDK's `DS3D_IMMEDIATE`/`DEFERRED` commit semantics (every change is
/// immediate). Distance attenuation is inverse-distance past
/// `min_distance` and panning is a plain left/right projection.
#[derive(Clone, Debug, PartialEq)]
pub struct Listener {
    pub position: Vec3,
    pub front: Vec3,
    pub top: Vec3,
    pub velocity: Vec3,
    /// World units per metre; scales distances.
    pub distance_factor: f32,
    /// 0 disables attenuation, 1 is normal inverse-distance.
    pub rolloff_factor: f32,
    /// Stored for the shim; unused.
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
    /// Returns the (linear gain, pan in millibels) a source gets from this
    /// listener. Pan is capped at +-4000 mB so a hard-side source is clearly
    /// lateral without going fully silent in one ear.
    pub fn spatialize(&self, src: &Source3d) -> (f32, i32) {
        if src.mode == Mode3d::Disabled {
            return (1.0, 0);
        }
        let rel = match src.mode {
            Mode3d::HeadRelative => src.position,
            _ => src.position.sub(self.position),
        };
        let dist = (rel.len() * self.distance_factor).min(src.max_distance);
        let min = src.min_distance.max(1e-6);
        let gain = if dist <= min {
            1.0
        } else {
            min / (min + self.rolloff_factor * (dist - min))
        };
        let right = self.top.cross(self.front);
        let rlen = right.len();
        let pan = if dist > 1e-6 && rlen > 1e-6 {
            (rel.dot(right) / (rlen * rel.len()) * 4000.0) as i32
        } else {
            0
        };
        (gain, pan)
    }
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
        let mut out = vec![0.0; 12 * 2];
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
        assert_eq!(l.spatialize(&near).0, 1.0);
        let far = Source3d { position: Vec3::new(5.0, 0.0, 0.0), ..Default::default() };
        let (g, pan) = l.spatialize(&far);
        assert!((g - 0.2).abs() < 1e-5); // 1 / (1 + 4)
        assert_eq!(pan, 4000); // hard right
        let left = Source3d { position: Vec3::new(-2.0, 0.0, 0.0), ..Default::default() };
        assert_eq!(l.spatialize(&left).1, -4000);
        let ahead = Source3d { position: Vec3::new(0.0, 0.0, 3.0), ..Default::default() };
        assert_eq!(l.spatialize(&ahead).1, 0);
        let off = Source3d { mode: Mode3d::Disabled, position: Vec3::new(9.0, 0.0, 0.0), ..Default::default() };
        assert_eq!(l.spatialize(&off), (1.0, 0));
    }
}
