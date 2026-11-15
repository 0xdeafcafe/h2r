//! The DirectSound object model: `IDirectSound`, `IDirectSoundBuffer` and
//! `IDirectSoundStream` as host state keyed by the guest pointers handed out
//! to the game, mapped onto `audio::Mixer`.
//!
//! Every method returns `Result<u32, Stop>`: `Ok` carries the guest's `EAX`
//! (an HRESULT, or a count for `Release`), `Err` is a stop that must reach
//! the harness. Unimplemented behaviour is always a [`Stop::Unimplemented`],
//! never a quiet `S_OK`. The few settings that only change timbre or routing
//! (filters, LFO/EG, mix bins, headroom, effect data) are stored and counted
//! in [`DirectSound::ignored`] so a run can report exactly what was dropped.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use audio::dsound::{BufferId, MixIn, Mode3d, PacketId, PacketStatus, Source3d, StreamId, Vec3};
use audio::mixer::MixInId;
use audio::{Mixer, SoundBuffer, Stream, WaveFormat};

use crate::abi::*;
use crate::host::{Host, MAX_GUEST_COPY, MemFault, StreamCallback, at};

/// Why the shim cannot continue. Distinct from an HRESULT failure, which the
/// game is allowed to see and handle.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Stop {
    #[error("DirectSound: {what} is not implemented")]
    Unimplemented { what: &'static str },
    #[error("DirectSound: {ptr:#010x} is not a live {expected}")]
    BadHandle { ptr: u32, expected: &'static str },
    #[error(transparent)]
    Memory(#[from] MemFault),
    #[error(transparent)]
    Consumer(#[from] ConsumerConflict),
}

pub type R = Result<u32, Stop>;

/// What renders the mixer, and so drives audio time. Exactly one may hold
/// the mixer: two would each render it and time would run twice. See
/// `docs/dsound.md` "Audio clock".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Consumer {
    /// `set_frames_per_do_work`: the game's own `DirectSoundDoWork` calls.
    DoWork,
    /// `advance_audio` / `Hle::tick` by hand.
    Manual,
    /// `Hle::spawn_pump(.., render: true)`.
    Pump,
    /// `Hle::spawn_output`: the host audio output.
    Output,
    /// Something outside this crate (the app's `AudioOut` device or
    /// discard renderer). Claim it so the shim's own consumers refuse.
    External,
}

/// A second consumer asked for the mixer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("DirectSound: the mixer is already consumed by {held:?} ({wanted:?} refused)")]
pub struct ConsumerConflict {
    pub held: Consumer,
    pub wanted: Consumer,
}

fn unimpl(what: &'static str) -> Stop {
    Stop::Unimplemented { what }
}

/// Addresses and sizes the guest-visible objects depend on.
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    /// Written to offset 0 of every `IDirectSoundStream` object so the game's
    /// virtual calls (`AddRef, Release, GetInfo, GetStatus, Process,
    /// Discontinuity, Flush`) land on guest addresses the override layer
    /// replaces. Halo 2 5849: the static `CDirectSoundStream` vtable.
    pub stream_vtable: u32,
    /// `dwEffectCount` of the dry effects image. Halo 2 5849: 15 (its init
    /// at 0x19142f only takes the effect-state path when `desc[0] == 0xF`).
    pub effect_count: u32,
}

/// Offset of the first `DSEFFECTMAP` in the returned `DSEFFECTIMAGEDESC`
/// (`0x37e39e`: `desc + 8 + index * 32`; the dword at `+0` is the effect
/// count).
const EFFECT_MAPS_OFFSET: u32 = 8;
/// Size of one `DSEFFECTMAP` (`0x37e3b3`: `index << 5`).
const EFFECT_MAP_SIZE: u32 = 32;
/// Offset of `lpvStateSegment` in a map: `GetEffectData` copies from
/// `[map + 8] + offset` (`0x37e5f2`) and the XHV engine dereferences it
/// (`0x33337a`).
const EFFECT_MAP_STATE: u32 = 8;
/// Bytes of parameter state backing each effect in the dry model (offsets and
/// sizes beyond are `E_INVALIDARG`).
const EFFECT_STATE_MAX: u32 = 0x1000;
/// The dword at state `+0x14` the XHV engine reads for its effects
/// (`0x333380`: `(value - 0x1400) >> 5`). The real value is the DSP address of
/// the effect's parameter block, which a dry image has no equivalent of; the
/// base makes the derived index 0. Invented, not evidenced.
const EFFECT_STATE_DSP_BASE_FIELD: u32 = 0x14;
const EFFECT_STATE_DSP_BASE: u32 = 0x1400;

/// `XAudioSetEffectData` descriptor types (the switch at 0x37bab1).
const EFFECT_TYPE_4: u32 = 4;
const EFFECT_TYPE_10: u32 = 10;
/// `DSI3DL2LISTENER` reverb parameters.
const EFFECT_TYPE_I3DL2: u32 = 12;
/// State bytes the I3DL2 path reads before converting (0x37bad6), and the
/// converted block it writes after them.
const EFFECT_I3DL2_READ: u32 = 0x118;
const EFFECT_I3DL2_BLOCK: u32 = 0x118;
const EFFECT_I3DL2_BLOCK_LEN: u32 = 0x108;

/// A descriptor `XAudioSetEffectData` accepted: its type and parameter
/// dwords (floats as bit patterns), after the type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectDesc {
    pub ty: u32,
    pub words: Vec<u32>,
}

#[derive(Clone, Copy, Debug)]
struct EffectsImage {
    desc: u32,
    count: u32,
    /// Base of the `count * EFFECT_STATE_MAX` bytes of effect state in guest
    /// memory; effect `i` is at `state + i * EFFECT_STATE_MAX` and is what its
    /// map's `lpvStateSegment` points at.
    state: u32,
}

impl EffectsImage {
    fn state_of(&self, index: u32) -> u32 {
        self.state
            .wrapping_add(index.wrapping_mul(EFFECT_STATE_MAX))
    }
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            stream_vtable: 0x0041_7170,
            effect_count: 15,
        }
    }
}

const OBJECT_SIZE: u32 = 0x40;

#[derive(Debug)]
struct Root {
    refs: u32,
}

#[derive(Debug)]
struct GuestData {
    ptr: u32,
    size: u32,
    /// Base and length of the allocation this shim made and must free.
    alloc: Option<(u32, u32)>,
}

/// Pause state, mirroring bits `0x4` (paused) and `0x40` (held until
/// `SynchPlayback`) of the real voice state word (`CMcpxVoiceClient+0x12`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Gate {
    Open,
    Paused,
    Held,
}

/// How to restart a voice that was stopped by a pause or a synchronised
/// start.
#[derive(Clone, Copy, Debug)]
struct Resume {
    looping: bool,
    from_start: bool,
}

#[derive(Debug)]
struct BufferObj {
    refs: u32,
    flags: u32,
    mix_in: bool,
    mix_in_playing: bool,
    format: Option<WaveFormat>,
    data: Option<GuestData>,
    audio: Option<BufferId>,
    volume_mb: i32,
    frequency: Option<u32>,
    headroom: i32,
    mix_bins: Vec<(u32, i32)>,
    input_mix_bin: u32,
    gate: Gate,
    resume: Option<Resume>,
    looping: bool,
    position: Option<[f32; 3]>,
    min_distance: f32,
    max_distance: f32,
    /// `SetRolloffFactor` on the buffer (multiplies the listener's).
    rolloff: f32,
    /// `SetDopplerFactor` on the buffer: stored, Doppler is not modelled.
    doppler: f32,
    /// `SetConeOutsideVolume`, millibels.
    cone_outside_mb: i32,
    /// The mixer submix standing in for a mix-in buffer.
    bus: Option<MixInId>,
}

#[derive(Debug, Clone, Copy)]
struct PacketRecord {
    /// Identity of the packet's event reference (0: none).
    token: u64,
    size: u32,
    completed_size_ptr: u32,
    status_ptr: u32,
    event_or_context: u32,
}

/// One queued packet, in submission order. Zero-length packets never reach
/// the audio crate (it would complete them ahead of earlier packets), so
/// `audio` is `None` for them.
#[derive(Debug)]
struct Slot {
    audio: Option<PacketId>,
    rec: PacketRecord,
    done: Option<u32>,
}

#[derive(Debug)]
struct StreamObj {
    refs: u32,
    audio: StreamId,
    max_packets: u32,
    callback: u32,
    callback_context: u32,
    packets: VecDeque<Slot>,
    gate: Gate,
    /// Set by the first `Process`: the real voice becomes active then, which
    /// the status word's pause bit depends on.
    started: bool,
    volume_mb: i32,
    headroom: i32,
    mix_bins: Vec<(u32, i32)>,
    discontinuities: u32,
    /// Voice state bit 0 (`+0x12`): set by `Process`, cleared by a flush.
    /// `GetVoiceProperties` fails while it is clear.
    voice_active: bool,
    flags: u32,
    /// The fields `GetInfo` derives from the creation format.
    info: StreamInfoFormat,
}

/// What `CDirectSoundStream::GetInfo` reads out of the voice's flattened
/// format (0x37da87: PCM `0x37d9ee`, Xbox ADPCM `0x37da17`): channels and bits
/// per sample as bytes, and the low byte of `nBlockAlign`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct StreamInfoFormat {
    channels: u8,
    bits: u8,
    block_align: u8,
}

impl StreamInfoFormat {
    /// From the 16 leading `WAVEFORMATEX` bytes. ADPCM (tag `0x69`) reports
    /// 4 bits regardless of `wBitsPerSample`.
    fn from_wfx(b: &[u8]) -> Self {
        let tag = u16::from_le_bytes([b[0], b[1]]);
        Self {
            channels: b[2],
            bits: if tag == 0x69 { 4 } else { b[14] },
            block_align: b[12],
        }
    }
    /// `dwMaxLookahead` (0x379eeb): twice the larger of `nBlockAlign` and
    /// 32 sample frames, in bytes.
    fn max_lookahead(self) -> u32 {
        let frames32 = ((u32::from(self.bits) * u32::from(self.channels)) >> 3) << 5;
        frames32.max(u32::from(self.block_align)) * 2
    }
}

/// `DSMIXBINS` slots in a voice (hardware limit): `GetVoiceProperties`
/// reports at most this many pairs.
const VOICE_MIX_BIN_SLOTS: usize = 8;
/// Bytes `GetVoiceProperties` writes (`0x3818e5`: fields up to `+0x58`).
const VOICE_PROPS_SIZE: u32 = 0x5c;
/// Unused `GetVoiceProperties` slot: bin `-1`, volume `-10000` (`0xFFFFD8F0`).
const VOICE_PROPS_UNUSED: (u32, u32) = (0xffff_ffff, 0xffff_d8f0);

/// The routing a voice gets when it is created, or `SetMixBins`, without a
/// `DSMIXBINS` (default tables at `0x38590c`..`0x38595c`, picked by `0x37bc89`
/// from the voice's flags and `channels >> 1`). Volumes are 0.
fn default_mix_bins(flags: u32, channels: usize) -> Vec<(u32, i32)> {
    let bins: &[u32] = if flags & 0x0020_0010 != 0 {
        &[6, 8, 7, 9, 10]
    } else {
        match channels >> 1 {
            0 | 1 => &[0, 1],
            2 => &[0, 1, 4, 5],
            _ => &[0, 1, 2, 3, 4, 5],
        }
    };
    bins.iter().map(|&b| (b, 0)).collect()
}

#[derive(Debug)]
enum Object {
    Root(Root),
    Buffer(BufferObj),
    Stream(StreamObj),
}

/// The listener/world-level settings kept for `IDirectSound`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Listener3d {
    pub position: [f32; 3],
    pub front: [f32; 3],
    pub top: [f32; 3],
    pub distance_factor: f32,
    pub doppler_factor: f32,
    pub rolloff_factor: f32,
}

impl Default for Listener3d {
    fn default() -> Self {
        Self {
            position: [0.0; 3],
            front: [0.0, 0.0, 1.0],
            top: [0.0, 1.0, 0.0],
            distance_factor: 1.0,
            doppler_factor: 1.0,
            rolloff_factor: 1.0,
        }
    }
}

/// The guest-facing DirectSound. One per process; owns the objects, shares
/// the mixer with the output thread.
pub struct DirectSound {
    mixer: Arc<Mutex<Mixer>>,
    layout: Layout,
    objects: HashMap<u32, Object>,
    root: Option<u32>,
    mix_bin_headroom: [u32; 32],
    effect_desc: BTreeMap<u32, EffectDesc>,
    effects: Option<EffectsImage>,
    listener: Listener3d,
    light_hrtf_4_channel: bool,
    frames_per_do_work: Option<u64>,
    frames_rendered: u64,
    consumer: Option<Consumer>,
    next_token: u64,
    ignored: BTreeMap<&'static str, u64>,
}

fn f(bits: u32) -> f32 {
    f32::from_bits(bits)
}

impl DirectSound {
    pub fn new(mixer: Arc<Mutex<Mixer>>, layout: Layout) -> Self {
        Self {
            mixer,
            layout,
            objects: HashMap::new(),
            root: None,
            mix_bin_headroom: [0; 32],
            effect_desc: BTreeMap::new(),
            effects: None,
            listener: Listener3d::default(),
            light_hrtf_4_channel: false,
            frames_per_do_work: None,
            frames_rendered: 0,
            consumer: None,
            next_token: 0,
            ignored: BTreeMap::new(),
        }
    }

    pub fn mixer(&self) -> &Arc<Mutex<Mixer>> {
        &self.mixer
    }

    /// Calls whose effect was accepted but not reproduced, by name.
    pub fn ignored(&self) -> &BTreeMap<&'static str, u64> {
        &self.ignored
    }

    pub fn listener(&self) -> &Listener3d {
        &self.listener
    }

    pub fn mix_bin_headroom(&self, mix_bin: usize) -> Option<u32> {
        self.mix_bin_headroom.get(mix_bin).copied()
    }

    pub fn light_hrtf_4_channel(&self) -> bool {
        self.light_hrtf_4_channel
    }

    /// Live guest objects (diagnostics and leak tests).
    pub fn live_objects(&self) -> usize {
        self.objects.len()
    }

    fn note(&mut self, what: &'static str) {
        *self.ignored.entry(what).or_default() += 1;
    }

    fn alloc_object(&mut self, h: &mut dyn Host, obj: Object) -> Result<u32, u32> {
        let Some(ptr) = h.alloc(OBJECT_SIZE, 16) else {
            return Err(E_OUTOFMEMORY);
        };
        if matches!(obj, Object::Stream(_)) && h.write_u32(ptr, self.layout.stream_vtable).is_err()
        {
            h.free(ptr);
            return Err(E_FAIL);
        }
        self.objects.insert(ptr, obj);
        Ok(ptr)
    }

    fn buf(&mut self, ptr: u32) -> Result<&mut BufferObj, Stop> {
        match self.objects.get_mut(&ptr) {
            Some(Object::Buffer(b)) => Ok(b),
            _ => Err(Stop::BadHandle {
                ptr,
                expected: "IDirectSoundBuffer",
            }),
        }
    }

    fn stream(&mut self, ptr: u32) -> Result<&mut StreamObj, Stop> {
        match self.objects.get_mut(&ptr) {
            Some(Object::Stream(s)) => Ok(s),
            _ => Err(Stop::BadHandle {
                ptr,
                expected: "IDirectSoundStream",
            }),
        }
    }

    fn check_root(&self, ptr: u32) -> Result<(), Stop> {
        match self.objects.get(&ptr) {
            Some(Object::Root(_)) => Ok(()),
            _ => Err(Stop::BadHandle {
                ptr,
                expected: "IDirectSound",
            }),
        }
    }

    // ---- IDirectSound ----------------------------------------------------

    /// `DirectSoundCreate(pguid, ppDS, pOuter)`. The real library keeps one
    /// `CDirectSound`; later calls AddRef it.
    pub fn direct_sound_create(&mut self, h: &mut dyn Host, pp_ds: u32) -> R {
        let ptr = match self.root {
            Some(p) => {
                if let Some(Object::Root(r)) = self.objects.get_mut(&p) {
                    r.refs += 1;
                }
                p
            }
            None => match self.alloc_object(h, Object::Root(Root { refs: 1 })) {
                Ok(p) => {
                    self.root = Some(p);
                    p
                }
                Err(hr) => return Ok(hr),
            },
        };
        h.write_u32(pp_ds, ptr)?;
        Ok(S_OK)
    }

    /// `AddRef` on any object; returns the new count.
    pub fn add_ref(&mut self, ptr: u32) -> R {
        match self.objects.get_mut(&ptr) {
            Some(Object::Root(r)) => {
                r.refs += 1;
                Ok(r.refs)
            }
            Some(Object::Buffer(b)) => {
                b.refs += 1;
                Ok(b.refs)
            }
            Some(Object::Stream(s)) => {
                s.refs += 1;
                Ok(s.refs)
            }
            None => Err(Stop::BadHandle {
                ptr,
                expected: "DirectSound object",
            }),
        }
    }

    /// `Release` on any object; returns the new count and frees at zero.
    pub fn release(&mut self, h: &mut dyn Host, ptr: u32) -> R {
        let refs = match self.objects.get_mut(&ptr) {
            Some(Object::Root(r)) => {
                r.refs -= 1;
                r.refs
            }
            Some(Object::Buffer(b)) => {
                b.refs -= 1;
                b.refs
            }
            Some(Object::Stream(s)) => {
                s.refs -= 1;
                s.refs
            }
            None => {
                return Err(Stop::BadHandle {
                    ptr,
                    expected: "DirectSound object",
                });
            }
        };
        if refs == 0 {
            match self.objects.remove(&ptr) {
                Some(Object::Root(_)) => self.root = None,
                Some(Object::Buffer(b)) => {
                    if let Some(id) = b.audio {
                        self.mixer.lock().unwrap().remove_buffer(id);
                    }
                    if let Some(id) = b.bus {
                        self.mixer.lock().unwrap().remove_mix_in(id);
                    }
                    if let Some((base, _)) = b.data.and_then(|d| d.alloc) {
                        h.free(base);
                    }
                }
                Some(Object::Stream(s)) => {
                    self.mixer.lock().unwrap().remove_stream(s.audio);
                    // Packets still attached never complete: the real stream
                    // drops them the same way. Their status stays PENDING.
                    // Their events are never signalled either, so the
                    // references taken at `Process` go back.
                    if s.callback == 0 {
                        for slot in &s.packets {
                            if slot.rec.event_or_context != 0 {
                                h.release_event(slot.rec.event_or_context, slot.rec.token);
                            }
                        }
                    }
                }
                None => {}
            }
            h.free(ptr);
        }
        Ok(refs)
    }

    /// `GetCaps(pDSCaps)`: `DSCAPS { Free2DBuffers, Free3DBuffers,
    /// FreeBufferSGEs, MemoryAllocated }`. Layout from the XDK header, not
    /// verified; Halo only checks the HRESULT.
    pub fn get_caps(&mut self, h: &mut dyn Host, ds: u32, caps: u32) -> R {
        self.check_root(ds)?;
        for (i, v) in [64u32, 64, 0x7FFF, 0].iter().enumerate() {
            h.write_u32(at(caps, 4 * i as u32)?, *v)?;
        }
        Ok(S_OK)
    }

    /// `GetSpeakerConfig(pdwConfig)`: always plain stereo (audio is stereo).
    pub fn get_speaker_config(&mut self, h: &mut dyn Host, ds: u32, out: u32) -> R {
        self.check_root(ds)?;
        h.write_u32(out, DSSPEAKER_STEREO)?;
        Ok(S_OK)
    }

    pub fn set_mix_bin_headroom(&mut self, ds: u32, mix_bin: u32, headroom: u32) -> R {
        self.check_root(ds)?;
        match self.mix_bin_headroom.get_mut(mix_bin as usize) {
            Some(slot) => *slot = headroom,
            None => return Ok(E_INVALIDARG),
        }
        self.note("SetMixBinHeadroom");
        Ok(S_OK)
    }

    /// `CommitDeferredSettings`: every 3D/voice change here is already
    /// immediate, so there is nothing deferred to commit.
    pub fn commit_deferred(&mut self, ds: u32) -> R {
        self.check_root(ds)?;
        Ok(S_OK)
    }

    /// `SynchPlayback`: starts every voice held by `Pause(2)` or
    /// `Play(DSBPLAY_SYNCHPLAYBACK)` together (verified: the real one walks
    /// the voices whose state word masks to `0x43` and releases bit `0x40`).
    pub fn synch_playback(&mut self, ds: u32) -> R {
        self.check_root(ds)?;
        // One lock across the whole group, so the audio callback sees every
        // held voice released or none of them.
        let mixer = Arc::clone(&self.mixer);
        let mut m = mixer.lock().unwrap();
        for obj in self.objects.values_mut() {
            match obj {
                Object::Buffer(b) if b.gate == Gate::Held => {
                    b.gate = Gate::Open;
                    let Some(r) = b.resume.take() else { continue };
                    b.looping = r.looping;
                    if let Some(sb) = b.audio.and_then(|id| m.buffer_mut(id)) {
                        if r.from_start {
                            sb.set_current_position(0);
                        }
                        sb.play(r.looping);
                    }
                }
                Object::Stream(s) if s.gate == Gate::Held => {
                    s.gate = Gate::Open;
                    if let Some(st) = m.stream_mut(s.audio) {
                        st.pause(false);
                    }
                }
                _ => {}
            }
        }
        Ok(S_OK)
    }

    /// Opens a buffer's gate and restarts it if it was playing (or was
    /// waiting for its first start).
    fn release_buffer_gate(&mut self, ptr: u32) -> Result<(), Stop> {
        let b = self.buf(ptr)?;
        b.gate = Gate::Open;
        if b.mix_in {
            return self.sync_buffer(ptr);
        }
        let Some(r) = b.resume.take() else {
            return Ok(());
        };
        b.looping = r.looping;
        self.with_audio(ptr, |_, sb| {
            if r.from_start {
                sb.set_current_position(0);
            }
            sb.play(r.looping);
        })?;
        Ok(())
    }

    fn release_stream_gate(&mut self, ptr: u32) -> Result<(), Stop> {
        let mixer = Arc::clone(&self.mixer);
        let s = self.stream(ptr)?;
        s.gate = Gate::Open;
        let audio = s.audio;
        if let Some(st) = mixer.lock().unwrap().stream_mut(audio) {
            st.pause(false);
        }
        Ok(())
    }

    pub fn use_light_hrtf_4_channel(&mut self) -> R {
        self.light_hrtf_4_channel = true;
        self.note("DirectSoundUseLightHRTF4Channel");
        Ok(S_OK)
    }

    pub fn set_distance_factor(&mut self, ds: u32, v: u32, _apply: u32) -> R {
        self.check_root(ds)?;
        self.listener.distance_factor = f(v);
        self.mixer.lock().unwrap().listener_mut().distance_factor = f(v);
        Ok(S_OK)
    }

    pub fn set_doppler_factor(&mut self, ds: u32, v: u32, _apply: u32) -> R {
        self.check_root(ds)?;
        self.listener.doppler_factor = f(v);
        self.mixer.lock().unwrap().listener_mut().doppler_factor = f(v);
        Ok(S_OK)
    }

    pub fn set_rolloff_factor(&mut self, ds: u32, v: u32, _apply: u32) -> R {
        self.check_root(ds)?;
        self.listener.rolloff_factor = f(v);
        self.mixer.lock().unwrap().listener_mut().rolloff_factor = f(v);
        Ok(S_OK)
    }

    pub fn set_listener_position(&mut self, ds: u32, x: u32, y: u32, z: u32, _apply: u32) -> R {
        self.check_root(ds)?;
        self.listener.position = [f(x), f(y), f(z)];
        self.mixer.lock().unwrap().listener_mut().position = Vec3::new(f(x), f(y), f(z));
        Ok(S_OK)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn set_listener_orientation(&mut self, ds: u32, v: [u32; 6], _apply: u32) -> R {
        self.check_root(ds)?;
        let [fx, fy, fz, tx, ty, tz] = v.map(f);
        self.listener.front = [fx, fy, fz];
        self.listener.top = [tx, ty, tz];
        let mut m = self.mixer.lock().unwrap();
        m.listener_mut().front = Vec3::new(fx, fy, fz);
        m.listener_mut().top = Vec3::new(tx, ty, tz);
        Ok(S_OK)
    }

    /// `SetEffectData(dwEffectIndex, dwOffset, pvData, dwDataSize, dwApply)`:
    /// the DSP image is not run, so the bytes are only kept for `GetEffectData`.
    pub fn set_effect_data(
        &mut self,
        h: &mut dyn Host,
        ds: u32,
        index: u32,
        offset: u32,
        data: u32,
        size: u32,
    ) -> R {
        self.check_root(ds)?;
        let Some(img) = self.effects else {
            return Ok(DSERR_INVALIDCALL);
        };
        // Verified (0x37e39e): an index past the image's effect count is
        // DSERR_INVALIDCALL. The offset/size cap is this model's own limit.
        if index >= img.count {
            return Ok(DSERR_INVALIDCALL);
        }
        if offset
            .checked_add(size)
            .is_none_or(|e| e > EFFECT_STATE_MAX)
        {
            return Ok(E_INVALIDARG);
        }
        let bytes = h.read_vec(data, size)?;
        h.write(at(img.state_of(index), offset)?, &bytes)?;
        self.note("SetEffectData");
        Ok(S_OK)
    }

    /// `GetEffectData`: bytes the game stored, and zeros elsewhere. The
    /// zeros stand in for the DSP image's initial parameter RAM, which is
    /// not reproduced (see `docs/dsound.md`, "Dry effects image").
    pub fn get_effect_data(
        &mut self,
        h: &mut dyn Host,
        ds: u32,
        index: u32,
        offset: u32,
        out: u32,
        size: u32,
    ) -> R {
        self.check_root(ds)?;
        let Some(img) = self.effects else {
            return Ok(DSERR_INVALIDCALL);
        };
        // Verified (0x37e39e): an index past the image's effect count is
        // DSERR_INVALIDCALL. The offset/size cap is this model's own limit.
        if index >= img.count {
            return Ok(DSERR_INVALIDCALL);
        }
        if offset
            .checked_add(size)
            .is_none_or(|e| e > EFFECT_STATE_MAX)
        {
            return Ok(E_INVALIDARG);
        }
        let bytes = h.read_vec(at(img.state_of(index), offset)?, size)?;
        h.write(out, &bytes)?;
        Ok(S_OK)
    }

    /// Writes the `DSEFFECTIMAGEDESC` and the state it points at. Every
    /// field the library or Halo reads is laid out as the binary reads it
    /// (count at `+0`; 32-byte maps from `+8`; `lpvStateSegment` at map
    /// `+8`); the unread fields are zero.
    fn build_effects_image(&self, h: &mut dyn Host, img: &EffectsImage) -> Result<(), MemFault> {
        h.write_u32(img.desc, img.count)?;
        for i in 0..img.count {
            let map = at(img.desc, EFFECT_MAPS_OFFSET + i * EFFECT_MAP_SIZE)?;
            h.write_u32(at(map, EFFECT_MAP_STATE)?, img.state_of(i))?;
            h.write_u32(
                at(img.state_of(i), EFFECT_STATE_DSP_BASE_FIELD)?,
                EFFECT_STATE_DSP_BASE,
            )?;
        }
        Ok(())
    }

    /// `XAudioDownloadEffectsImage(pszImageName, pImageLoc, dwFlags,
    /// ppImageDesc)`. A dry image: the DSP program is never run, but the
    /// descriptor Halo reads back is built so init proceeds. Flag 1 is the
    /// XBE-section form Halo uses (name `"DSPImage"`); loading from a file is
    /// a stop.
    ///
    /// `pImageLoc` is `{ dwI3DL2ReverbIndex, dwCrosstalkIndex }` (Halo passes
    /// 9 and 10); both must name effects in the image.
    pub fn download_effects_image(
        &mut self,
        h: &mut dyn Host,
        name: u32,
        loc: u32,
        flags: u32,
        pp_desc: u32,
    ) -> R {
        if flags != 1 {
            return Err(unimpl(
                "XAudioDownloadEffectsImage from a file (only the XBE-section form is modelled)",
            ));
        }
        if name == 0 || loc == 0 || pp_desc == 0 {
            return Ok(E_INVALIDARG);
        }
        let mut got = [0u8; 9];
        h.read(name, &mut got)?;
        if &got != b"DSPImage\0" {
            return Err(unimpl(
                "XAudioDownloadEffectsImage with an image name other than \"DSPImage\"",
            ));
        }
        let reverb = h.read_u32(loc)?;
        let crosstalk = h.read_u32(at(loc, 4)?)?;
        let count = self.layout.effect_count;
        if reverb >= count || crosstalk >= count {
            return Ok(E_INVALIDARG);
        }
        let desc = match self.effects {
            Some(img) => img.desc,
            None => {
                let desc_size = EFFECT_MAPS_OFFSET + count * EFFECT_MAP_SIZE;
                let Some(desc) = h.alloc(desc_size, 16) else {
                    return Ok(E_OUTOFMEMORY);
                };
                let Some(state) = h.alloc(count * EFFECT_STATE_MAX, 16) else {
                    h.free(desc);
                    return Ok(E_OUTOFMEMORY);
                };
                let img = EffectsImage { desc, count, state };
                if let Err(e) = self.build_effects_image(h, &img) {
                    h.free(state);
                    h.free(desc);
                    return Err(e.into());
                }
                self.effects = Some(img);
                desc
            }
        };
        h.write_u32(pp_desc, desc)?;
        self.note("XAudioDownloadEffectsImage (dry image)");
        Ok(S_OK)
    }

    /// `XAudioSetEffectData(dwEffectIndex, pDesc, pRawDesc)` (0x37ba6f, `ret
    /// 0Ch`), dry model. Verified against the binary:
    ///
    /// - With no `CDirectSound` (global `0x387198` clear) it returns
    ///   `DSERR_INVALIDCALL` before reading `pDesc`.
    /// - `*pDesc` is the effect type, and only 4, 10 and 12 are known; any
    ///   other type is `DSERR_INVALIDCALL`. The parameters follow it:
    ///   4: 7 dwords (floats), 10: 3 floats, 12: the 12 dwords of
    ///   `DSI3DL2LISTENER` (Halo's `0x21ece0` builds that, index 9).
    /// - An index at or above the image's effect count is
    ///   `DSERR_INVALIDCALL` (0x37e39e). Type 12 checks it before reading the
    ///   parameters; 4 and 10 read them first.
    /// - Type 12 reads the effect's first `0x118` state bytes, ORs 4 into the
    ///   dword at `+0x10` and writes it back; the converted DSP block
    ///   (`0x108` bytes at `+0x118`) comes from the I3DL2 converter
    ///   (0x3838a4), which is not reproduced. Types 4 and 10 write `0x2c` and
    ///   `0x14` converted bytes at `+0x1c` (biquad coefficients from
    ///   0x37a032), also not reproduced.
    /// - A non-null `pRawDesc` receives the type and then exactly the bytes
    ///   written to DSP state (4: 11 dwords, 10: 5 dwords, 12: `0x108`
    ///   bytes). Dry model: they are the effect state at the destination
    ///   range, so the output always equals `GetEffectData` of that range
    ///   afterwards. The DSP image is zeros here, so unwritten bytes are
    ///   zero. Halo passes null at both call sites (0x21ee6f, 0x21fa07).
    ///
    /// The input descriptor is kept for [`DirectSound::effect_desc`].
    pub fn xaudio_set_effect_data(
        &mut self,
        h: &mut dyn Host,
        index: u32,
        desc: u32,
        out: u32,
    ) -> R {
        if self.root.is_none() {
            return Ok(DSERR_INVALIDCALL);
        }
        let ty = h.read_u32(desc)?;
        let (in_dwords, dest_off, dest_len) = match ty {
            EFFECT_TYPE_4 => (7u32, 0x1c_u32, 0x2c_u32),
            EFFECT_TYPE_10 => (3, 0x1c, 0x14),
            EFFECT_TYPE_I3DL2 => (12, EFFECT_I3DL2_BLOCK, EFFECT_I3DL2_BLOCK_LEN),
            _ => return Ok(DSERR_INVALIDCALL),
        };
        let in_range = self.effects.is_some_and(|img| index < img.count);
        // Verified (0x37ba78, 0x37baef): the return local starts at 0 and a
        // failed state read in the type 12 path jumps straight to the
        // epilogue without storing the error. Invalid index, S_OK, nothing
        // read, written or output.
        if ty == EFFECT_TYPE_I3DL2 && !in_range {
            return Ok(S_OK);
        }
        let mut words = Vec::with_capacity(in_dwords as usize);
        for i in 0..in_dwords {
            words.push(h.read_u32(at(desc, 4 + 4 * i)?)?);
        }
        if !in_range {
            // Types 4 and 10 store the failed write's code (0x37bc4b) and
            // still copy their converted buffer to a non-null pRaw. The
            // buffer is never written to DSP state; the dry model has no
            // converted words, so it is zeros.
            if out != 0 {
                h.write_u32(out, ty)?;
                h.write(at(out, 4)?, &vec![0u8; dest_len as usize])?;
            }
            return Ok(DSERR_INVALIDCALL);
        }
        let base = self
            .effects
            .map(|img| img.state_of(index))
            .expect("in_range implies an image");
        if ty == EFFECT_TYPE_I3DL2 {
            // The first EFFECT_I3DL2_READ bytes are read (0x37bad6); the
            // flags dword at +0x10 gets 4 ORed in and is written back.
            let state = h.read_vec(base, EFFECT_I3DL2_READ)?;
            let flags = u32::from_le_bytes(state[0x10..0x14].try_into().expect("4 bytes")) | 4;
            h.write_u32(at(base, 0x10)?, flags)?;
        }
        let raw = h.read_vec(at(base, dest_off)?, dest_len)?;
        self.effect_desc.insert(index, EffectDesc { ty, words });
        self.note("XAudioSetEffectData (dry)");
        if out != 0 {
            h.write_u32(out, ty)?;
            h.write(at(out, 4)?, &raw)?;
        }
        Ok(S_OK)
    }

    /// The last descriptor `XAudioSetEffectData` accepted for an effect.
    pub fn effect_desc(&self, index: u32) -> Option<&EffectDesc> {
        self.effect_desc.get(&index)
    }

    // ---- creation --------------------------------------------------------

    /// `IDirectSound::CreateSoundBuffer(pdsbd, ppBuffer, pOuter)`.
    pub fn create_sound_buffer(
        &mut self,
        h: &mut dyn Host,
        ds: u32,
        desc: u32,
        pp: u32,
        outer: u32,
    ) -> R {
        self.check_root(ds)?;
        if outer != 0 {
            return Ok(E_INVALIDARG);
        }
        let d = BufferDesc::read(h, desc)?;
        if d.size != BufferDesc::SIZE {
            return Ok(E_INVALIDARG);
        }
        let mix_in = d.flags & DSBCAPS_MIXIN_LIKE_MASK != 0;
        let given_bins = if d.mix_bins_ptr != 0 {
            Some(read_mix_bins(h, d.mix_bins_ptr)?)
        } else {
            None
        };
        let format = if d.format_ptr != 0 {
            match read_wave_format(h, d.format_ptr)? {
                Ok(f) => Some(f),
                Err(_) => return Ok(0x8878_0064), // DSERR_BADFORMAT
            }
        } else {
            None
        };
        if mix_in && (d.buffer_bytes != 0 || format.is_some()) {
            return Err(unimpl("mix-in buffer with sample data or format"));
        }
        if !mix_in && format.is_none() {
            return Err(unimpl(
                "data buffer created without a format (needs SetFormat)",
            ));
        }
        let mix_bins = match (given_bins, mix_in) {
            (Some(v), _) => v,
            (None, true) => Vec::new(),
            (None, false) => default_mix_bins(d.flags, format.map_or(2, |f| f.channels() as usize)),
        };
        let mut obj = BufferObj {
            refs: 1,
            flags: d.flags,
            mix_in,
            mix_in_playing: false,
            format,
            data: None,
            audio: None,
            volume_mb: 0,
            frequency: None,
            headroom: 0,
            mix_bins,
            input_mix_bin: d.input_mix_bin,
            gate: Gate::Open,
            resume: None,
            looping: false,
            position: None,
            min_distance: 1.0,
            max_distance: 1.0e9,
            rolloff: 1.0,
            doppler: 1.0,
            cone_outside_mb: 0,
            bus: None,
        };
        if mix_in {
            obj.bus = Some(
                self.mixer
                    .lock()
                    .unwrap()
                    .add_mix_in(MixIn::new(d.input_mix_bin)),
            );
        }
        if let (Some(fmt), true) = (format, d.buffer_bytes != 0) {
            let align = fmt.block_align() as u32;
            if d.buffer_bytes % align != 0 || d.buffer_bytes > MAX_GUEST_COPY {
                return Ok(E_INVALIDARG);
            }
            let Some(p) = h.alloc(d.buffer_bytes, 16) else {
                return Ok(E_OUTOFMEMORY);
            };
            // Pcm8 silence is 0x80, not 0.
            if fmt.codec() == audio::Codec::Pcm8
                && let Err(e) = h.write(p, &vec![0x80; d.buffer_bytes as usize])
            {
                h.free(p);
                return Err(e.into());
            }
            let sb = match SoundBuffer::new(fmt, d.buffer_bytes as usize) {
                Ok(sb) => sb,
                Err(_) => {
                    h.free(p);
                    return Err(unimpl("buffer size"));
                }
            };
            obj.audio = Some(self.mixer.lock().unwrap().add_buffer(sb));
            obj.data = Some(GuestData {
                ptr: p,
                size: d.buffer_bytes,
                alloc: Some((p, d.buffer_bytes)),
            });
        }
        let (audio, bus, data) = (obj.audio, obj.bus, obj.data.as_ref().and_then(|d| d.alloc));
        {
            let mut m = self.mixer.lock().unwrap();
            if let Some(sb) = audio.and_then(|id| m.buffer_mut(id)) {
                Self::apply_params(&obj, sb);
            }
            if let Some(mi) = bus.and_then(|id| m.mix_in_mut(id)) {
                Self::apply_mix_in(&obj, mi);
            }
        }
        let rollback = |this: &mut Self, h: &mut dyn Host| {
            if let Some(id) = audio {
                this.mixer.lock().unwrap().remove_buffer(id);
            }
            if let Some(id) = bus {
                this.mixer.lock().unwrap().remove_mix_in(id);
            }
            if let Some((base, _)) = data {
                h.free(base);
            }
        };
        match self.alloc_object(h, Object::Buffer(obj)) {
            Ok(p) => match h.write_u32(pp, p) {
                Ok(()) => Ok(S_OK),
                Err(e) => {
                    // The guest never got the pointer: undo everything.
                    self.objects.remove(&p);
                    h.free(p);
                    rollback(self, h);
                    Err(e.into())
                }
            },
            Err(hr) => {
                rollback(self, h);
                Ok(hr)
            }
        }
    }

    /// `DirectSoundCreateBuffer(pdsbd, ppBuffer)`: the standalone form. The
    /// real one makes a temporary `CDirectSound`, creates the buffer and
    /// releases the temporary.
    pub fn direct_sound_create_buffer(&mut self, h: &mut dyn Host, desc: u32, pp: u32) -> R {
        let tmp = h.alloc(4, 4).ok_or(unimpl("scratch allocation"))?;
        let hr = self.direct_sound_create(h, tmp)?;
        if hr != S_OK {
            h.free(tmp);
            return Ok(hr);
        }
        let ds = h.read_u32(tmp)?;
        h.free(tmp);
        let r = self.create_sound_buffer(h, ds, desc, pp, 0);
        self.release(h, ds)?;
        r
    }

    /// `IDirectSound::CreateSoundStream(pdssd, ppStream, pOuter)`.
    pub fn create_sound_stream(
        &mut self,
        h: &mut dyn Host,
        ds: u32,
        desc: u32,
        pp: u32,
        outer: u32,
    ) -> R {
        self.check_root(ds)?;
        if outer != 0 {
            return Ok(E_INVALIDARG);
        }
        let d = StreamDesc::read(h, desc)?;
        if d.format_ptr == 0 {
            return Err(unimpl("stream created without a format"));
        }
        let format = match read_wave_format(h, d.format_ptr)? {
            Ok(f) => f,
            Err(_) => return Ok(0x8878_0064),
        };
        let info = StreamInfoFormat::from_wfx(&h.read_vec(d.format_ptr, 16)?);
        let mix_bins = if d.mix_bins_ptr != 0 {
            read_mix_bins(h, d.mix_bins_ptr)?
        } else {
            default_mix_bins(d.flags, format.channels() as usize)
        };
        let mut stream = Stream::new(format).map_err(|_| unimpl("stream format"))?;
        stream.set_mix_bins(Some(mix_bins.clone()));
        let audio = self.mixer.lock().unwrap().add_stream(stream);
        let obj = StreamObj {
            refs: 1,
            audio,
            max_packets: d.max_attached_packets,
            callback: d.callback,
            callback_context: d.callback_context,
            packets: VecDeque::new(),
            gate: Gate::Open,
            started: false,
            volume_mb: 0,
            headroom: 0,
            mix_bins,
            discontinuities: 0,
            voice_active: false,
            flags: d.flags,
            info,
        };
        match self.alloc_object(h, Object::Stream(obj)) {
            Ok(p) => match h.write_u32(pp, p) {
                Ok(()) => Ok(S_OK),
                Err(e) => {
                    self.objects.remove(&p);
                    h.free(p);
                    self.mixer.lock().unwrap().remove_stream(audio);
                    Err(e.into())
                }
            },
            Err(hr) => {
                self.mixer.lock().unwrap().remove_stream(audio);
                Ok(hr)
            }
        }
    }

    pub fn direct_sound_create_stream(&mut self, h: &mut dyn Host, desc: u32, pp: u32) -> R {
        let tmp = h.alloc(4, 4).ok_or(unimpl("scratch allocation"))?;
        let hr = self.direct_sound_create(h, tmp)?;
        if hr != S_OK {
            h.free(tmp);
            return Ok(hr);
        }
        let ds = h.read_u32(tmp)?;
        h.free(tmp);
        let r = self.create_sound_stream(h, ds, desc, pp, 0);
        self.release(h, ds)?;
        r
    }

    // ---- buffers ---------------------------------------------------------

    fn apply_params(b: &BufferObj, sb: &mut SoundBuffer) {
        sb.set_volume(b.volume_mb);
        if let Some(hz) = b.frequency {
            sb.set_frequency(hz);
        }
        sb.set_source_3d(Self::source_3d(b));
        sb.set_mix_bins(Some(b.mix_bins.clone()));
    }

    /// The mixer submix of a mix-in buffer: volume, routing, 3D and whether
    /// it passes audio (played, and not paused or held).
    fn apply_mix_in(b: &BufferObj, mi: &mut MixIn) {
        mi.set_volume(b.volume_mb);
        mi.set_source_3d(Self::source_3d(b));
        mi.set_mix_bins(Some(b.mix_bins.clone()));
        if b.mix_in_playing && b.gate == Gate::Open {
            mi.play();
        } else {
            mi.stop();
        }
    }

    /// A `DSBCAPS_CTRL3D` voice's source, once the game has positioned it.
    fn source_3d(b: &BufferObj) -> Option<Source3d> {
        match (b.flags & DSBCAPS_CTRL3D != 0, b.position) {
            (true, Some([x, y, z])) => Some(Source3d {
                position: Vec3::new(x, y, z),
                min_distance: b.min_distance,
                max_distance: b.max_distance,
                rolloff_factor: b.rolloff,
                doppler_factor: b.doppler,
                cone_outside_volume_mb: b.cone_outside_mb,
                mode: Mode3d::Normal,
                ..Default::default()
            }),
            _ => None,
        }
    }

    /// Pushes a buffer's stored parameters to its mixer voice or submix.
    fn sync_buffer(&mut self, ptr: u32) -> Result<(), Stop> {
        let mixer = Arc::clone(&self.mixer);
        let b = self.buf(ptr)?;
        let mut m = mixer.lock().unwrap();
        if let Some(sb) = b.audio.and_then(|id| m.buffer_mut(id)) {
            Self::apply_params(b, sb);
        }
        if let Some(mi) = b.bus.and_then(|id| m.mix_in_mut(id)) {
            Self::apply_mix_in(b, mi);
        }
        Ok(())
    }

    /// Pushes a stream's routing to its mixer voice.
    fn sync_stream_bins(&mut self, ptr: u32) -> Result<(), Stop> {
        let mixer = Arc::clone(&self.mixer);
        let s = self.stream(ptr)?;
        if let Some(st) = mixer.lock().unwrap().stream_mut(s.audio) {
            st.set_mix_bins(Some(s.mix_bins.clone()));
        }
        Ok(())
    }

    fn with_audio<T>(
        &mut self,
        ptr: u32,
        op: impl FnOnce(&BufferObj, &mut SoundBuffer) -> T,
    ) -> Result<Option<T>, Stop> {
        let mixer = Arc::clone(&self.mixer);
        let b = self.buf(ptr)?;
        let Some(id) = b.audio else { return Ok(None) };
        let mut m = mixer.lock().unwrap();
        Ok(m.buffer_mut(id).map(|sb| op(b, sb)))
    }

    /// `SetBufferData(pvBufferData, dwBufferBytes)`: the buffer now plays the
    /// guest memory at `pv` (not copied permanently: it is re-read at `Play`
    /// and `Unlock`, see `docs/dsound.md`).
    pub fn set_buffer_data(&mut self, h: &mut dyn Host, ptr: u32, pv: u32, bytes: u32) -> R {
        let mixer = Arc::clone(&self.mixer);
        let b = self.buf(ptr)?;
        let Some(fmt) = b.format else {
            return Err(unimpl("SetBufferData on a mix-in buffer"));
        };
        if b.audio.is_some_and(|id| {
            mixer
                .lock()
                .unwrap()
                .buffer(id)
                .is_some_and(|s| s.is_playing())
        }) {
            return Ok(DSERR_INVALIDCALL);
        }
        if bytes == 0 || !bytes.is_multiple_of(fmt.block_align() as u32) {
            return Ok(E_INVALIDARG);
        }
        // Same pointer and size: the original does nothing. (Replacing would
        // free an owned allocation that the buffer then keeps using.)
        if b.data
            .as_ref()
            .is_some_and(|d| d.ptr == pv && d.size == bytes)
        {
            return Ok(S_OK);
        }
        let data = h.read_vec(pv, bytes)?;
        let sb = SoundBuffer::from_data(fmt, data).map_err(|_| unimpl("SetBufferData contents"))?;
        let b = self.buf(ptr)?;
        let mut m = mixer.lock().unwrap();
        if let Some(old) = b.audio.take() {
            m.remove_buffer(old);
        }
        let mut sb = sb;
        Self::apply_params(b, &mut sb);
        b.audio = Some(m.add_buffer(sb));
        // Keep an owned allocation alive if the new range lies inside it;
        // otherwise free it now.
        let alloc = match b.data.take().and_then(|d| d.alloc) {
            Some((base, len))
                if pv >= base && pv.checked_add(bytes).is_some_and(|e| e <= base + len) =>
            {
                Some((base, len))
            }
            Some((base, _)) => {
                h.free(base);
                None
            }
            None => None,
        };
        b.data = Some(GuestData {
            ptr: pv,
            size: bytes,
            alloc,
        });
        Ok(S_OK)
    }

    /// Re-reads the guest bytes into the audio buffer.
    fn resync(&mut self, h: &dyn crate::host::GuestMemory, ptr: u32) -> Result<(), Stop> {
        let mixer = Arc::clone(&self.mixer);
        let b = self.buf(ptr)?;
        let (Some(d), Some(id)) = (&b.data, b.audio) else {
            return Ok(());
        };
        let bytes = h.read_vec(d.ptr, d.size)?;
        if let Some(sb) = mixer.lock().unwrap().buffer_mut(id) {
            sb.lock_all()
                .write(&bytes)
                .map_err(|_| unimpl("buffer resync size"))?;
        }
        Ok(())
    }

    /// `Lock(dwOffset, dwBytes, ppv1, pcb1, ppv2, pcb2, dwFlags)`. Returns
    /// pointers into the buffer's guest memory, as the hardware does.
    #[allow(clippy::too_many_arguments)]
    pub fn lock(
        &mut self,
        h: &mut dyn Host,
        ptr: u32,
        offset: u32,
        bytes: u32,
        pp1: u32,
        pcb1: u32,
        pp2: u32,
        pcb2: u32,
        flags: u32,
    ) -> R {
        let b = self.buf(ptr)?;
        let (Some(fmt), Some(d)) = (b.format, &b.data) else {
            return Ok(DSERR_INVALIDCALL);
        };
        if flags & !(DSBLOCK_FROMWRITECURSOR | DSBLOCK_ENTIREBUFFER) != 0 {
            return Err(unimpl("Lock with undocumented flags"));
        }
        let (base, size) = (d.ptr, d.size);
        let (offset, bytes) = if flags & DSBLOCK_ENTIREBUFFER != 0 {
            (0, size)
        } else {
            (offset, bytes)
        };
        let align = fmt.block_align() as u32;
        if flags & DSBLOCK_FROMWRITECURSOR != 0 {
            return Err(unimpl("Lock FROMWRITECURSOR"));
        }
        if offset >= size || bytes > size || offset % align != 0 || bytes % align != 0 {
            return Ok(E_INVALIDARG);
        }
        let first = bytes.min(size - offset);
        h.write_u32(pp1, at(base, offset)?)?;
        h.write_u32(pcb1, first)?;
        if pp2 != 0 {
            h.write_u32(pp2, if bytes > first { base } else { 0 })?;
        }
        if pcb2 != 0 {
            h.write_u32(pcb2, bytes - first)?;
        }
        Ok(S_OK)
    }

    /// `Unlock`. On Xbox it is a no-op returning `S_OK` (verified: the flat
    /// `IDirectSoundBuffer_Unlock` is `xor eax,eax; ret 14h`); here it is also
    /// the point where the guest's writes reach the decoded audio.
    pub fn unlock(&mut self, h: &mut dyn Host, ptr: u32) -> R {
        self.resync(h, ptr)?;
        Ok(S_OK)
    }

    /// `Play(rsvd1, rsvd2, dwFlags)`.
    pub fn play(&mut self, h: &mut dyn Host, ptr: u32, rsvd1: u32, rsvd2: u32, flags: u32) -> R {
        if rsvd1 != 0 || rsvd2 != 0 || flags & !7 != 0 {
            return Ok(E_INVALIDARG);
        }
        let b = self.buf(ptr)?;
        if b.mix_in {
            // Verified: the real Play forces looping for these (and drops
            // the other flags).
            b.mix_in_playing = true;
            b.gate = Gate::Open;
            self.note("mix-in buffer Play");
            self.sync_buffer(ptr)?;
            return Ok(S_OK);
        }
        if b.audio.is_none() {
            return Ok(DSERR_INVALIDCALL);
        }
        // The guest may have rewritten the data without Lock/Unlock.
        self.resync(h, ptr)?;
        let looping = flags & DSBPLAY_LOOPING != 0;
        let from_start = flags & DSBPLAY_FROMSTART != 0;
        let synch = flags & DSBPLAY_SYNCHPLAYBACK != 0;
        let running = self.with_audio(ptr, |_, sb| sb.is_playing())? == Some(true);
        let b = self.buf(ptr)?;
        b.looping = looping;
        if synch && !running && b.gate == Gate::Open {
            // Verified (0x382d4f): a synchronised Play on an idle voice sets
            // the "held" bit instead of starting; SynchPlayback starts it.
            b.gate = Gate::Held;
            b.resume = Some(Resume {
                looping,
                from_start,
            });
            return Ok(S_OK);
        }
        b.gate = Gate::Open;
        b.resume = None;
        self.with_audio(ptr, |_, sb| {
            if from_start {
                sb.set_current_position(0);
            }
            sb.play(looping);
        })?;
        Ok(S_OK)
    }

    /// `Stop` / `StopEx`. Envelope release is not modelled: always immediate.
    pub fn stop(&mut self, ptr: u32, ex_flags: u32) -> R {
        if ex_flags != 0 {
            self.note("StopEx(flags)");
        }
        let b = self.buf(ptr)?;
        b.gate = Gate::Open;
        b.resume = None;
        b.mix_in_playing = false;
        self.with_audio(ptr, |_, sb| sb.stop())?;
        self.sync_buffer(ptr)?;
        Ok(S_OK)
    }

    /// `Pause(dwPause)`, decoded from `CMcpxBuffer_Pause`/`CMcpxStream_Pause`:
    /// 0 resume, 1 pause (bit `0x4`), 2 pause and hold until `SynchPlayback`
    /// (bit `0x40`), 3 resume. Pausing an already paused voice keeps the
    /// original resume intent. Mode 3 on a voice that is not paused or held
    /// queues a deferred start in the real library (`0x20`): a stop here.
    pub fn pause(&mut self, ptr: u32, mode: u32) -> R {
        if mode > 3 {
            return Ok(E_INVALIDARG);
        }
        let b = self.buf(ptr)?;
        match mode {
            0 | 3 => {
                if mode == 3 && b.gate == Gate::Open && !b.mix_in {
                    let playing = self.with_audio(ptr, |_, sb| sb.is_playing())?;
                    if playing != Some(true) {
                        return Err(unimpl(
                            "Pause(3) on a voice that is not active (deferred start)",
                        ));
                    }
                }
                self.release_buffer_gate(ptr)?;
            }
            _ => {
                let want = if mode == 1 { Gate::Paused } else { Gate::Held };
                if b.gate == Gate::Open {
                    let looping = b.looping;
                    let was_playing = self.with_audio(ptr, |_, sb| {
                        let p = sb.is_playing();
                        sb.stop();
                        p
                    })?;
                    let b = self.buf(ptr)?;
                    b.resume = (was_playing == Some(true)).then_some(Resume {
                        looping,
                        from_start: false,
                    });
                }
                self.buf(ptr)?.gate = want;
                self.sync_buffer(ptr)?;
            }
        }
        Ok(S_OK)
    }

    pub fn get_status(&mut self, h: &mut dyn Host, ptr: u32, out: u32) -> R {
        let playing = self.with_audio(ptr, |_, sb| sb.is_playing())?;
        let b = self.buf(ptr)?;
        let mut st = 0;
        if b.mix_in {
            if b.mix_in_playing {
                st |= DSBSTATUS_PLAYING | DSBSTATUS_LOOPING;
            }
        } else if b.gate != Gate::Open {
            st |= DSBSTATUS_PAUSED;
        } else if playing == Some(true) {
            st |= DSBSTATUS_PLAYING;
        }
        if st != 0 && !b.mix_in && (b.looping || b.resume.is_some_and(|r| r.looping)) {
            st |= DSBSTATUS_LOOPING;
        }
        h.write_u32(out, st)?;
        Ok(S_OK)
    }

    /// `GetCurrentPosition(pdwPlay, pdwWrite)`: both cursors report the play
    /// cursor (the model has no write-ahead).
    pub fn get_current_position(&mut self, h: &mut dyn Host, ptr: u32, play: u32, write: u32) -> R {
        let pos = self.with_audio(ptr, |_, sb| sb.current_position())?;
        let Some(pos) = pos else {
            return Ok(DSERR_INVALIDCALL);
        };
        if play != 0 {
            h.write_u32(play, pos as u32)?;
        }
        if write != 0 {
            h.write_u32(write, pos as u32)?;
        }
        Ok(S_OK)
    }

    pub fn set_current_position(&mut self, ptr: u32, pos: u32) -> R {
        match self.with_audio(ptr, |_, sb| sb.set_current_position(pos as usize))? {
            Some(()) => Ok(S_OK),
            None => Ok(DSERR_INVALIDCALL),
        }
    }

    pub fn buffer_set_volume(&mut self, ptr: u32, mb: i32) -> R {
        if !(-10_000..=0).contains(&mb) {
            return Ok(E_INVALIDARG);
        }
        self.buf(ptr)?.volume_mb = mb;
        self.sync_buffer(ptr)?;
        Ok(S_OK)
    }

    /// `SetFrequency(dwFrequency)`; 0 (`DSBFREQUENCY_ORIGINAL`) restores the
    /// format's own rate.
    pub fn buffer_set_frequency(&mut self, ptr: u32, hz: u32) -> R {
        let b = self.buf(ptr)?;
        if hz != 0 && !(100..=200_000).contains(&hz) {
            return Ok(E_INVALIDARG);
        }
        b.frequency = (hz != 0).then_some(hz);
        let rate = b.format.map(|f| f.sample_rate());
        let eff = b.frequency.or(rate);
        if let Some(eff) = eff {
            self.with_audio(ptr, |_, sb| sb.set_frequency(eff))?;
        }
        Ok(S_OK)
    }

    /// `dwInputMixBin` the buffer was created with (mix-in routing, stored).
    pub fn buffer_input_mix_bin(&mut self, ptr: u32) -> Result<u32, Stop> {
        Ok(self.buf(ptr)?.input_mix_bin)
    }

    pub fn buffer_set_headroom(&mut self, ptr: u32, headroom: u32) -> R {
        self.buf(ptr)?.headroom = headroom as i32;
        self.note("SetHeadroom");
        Ok(S_OK)
    }

    pub fn buffer_set_mix_bins(&mut self, h: &mut dyn Host, ptr: u32, bins: u32) -> R {
        let given = if bins != 0 {
            Some(read_mix_bins(h, bins)?)
        } else {
            None
        };
        let b = self.buf(ptr)?;
        b.mix_bins = match given {
            Some(v) => v,
            None if b.mix_in => Vec::new(),
            None => default_mix_bins(b.flags, b.format.map_or(2, |f| f.channels() as usize)),
        };
        self.note("SetMixBins");
        self.sync_buffer(ptr)?;
        Ok(S_OK)
    }

    pub fn buffer_set_mix_bin_volumes(&mut self, h: &mut dyn Host, ptr: u32, bins: u32) -> R {
        let v = read_mix_bins(h, bins)?;
        let b = self.buf(ptr)?;
        for (bin, vol) in v {
            match b.mix_bins.iter_mut().find(|p| p.0 == bin) {
                Some(p) => p.1 = vol,
                None => b.mix_bins.push((bin, vol)),
            }
        }
        self.note("SetMixBinVolumes");
        self.sync_buffer(ptr)?;
        Ok(S_OK)
    }

    /// Filters, LFO and EG shape timbre only: accepted and counted.
    pub fn buffer_set_filter(&mut self, ptr: u32) -> R {
        self.buf(ptr)?;
        self.note("SetFilter");
        Ok(S_OK)
    }

    /// `SetPosition(x, y, z, dwApply)` on a buffer.
    pub fn buffer_set_position(&mut self, ptr: u32, x: u32, y: u32, z: u32) -> R {
        self.buf(ptr)?.position = Some([f(x), f(y), f(z)]);
        self.refresh_3d(ptr)
    }

    pub fn buffer_set_min_distance(&mut self, ptr: u32, v: u32) -> R {
        self.buf(ptr)?.min_distance = f(v);
        self.refresh_3d(ptr)
    }

    pub fn buffer_set_max_distance(&mut self, ptr: u32, v: u32) -> R {
        self.buf(ptr)?.max_distance = f(v);
        self.refresh_3d(ptr)
    }

    /// `SetRolloffFactor(flRolloffFactor, dwApply)` on a buffer: multiplies
    /// the listener's rolloff for this voice. Halo 2 passes 0 (it attenuates
    /// by distance itself through the voice volume).
    pub fn buffer_set_rolloff(&mut self, ptr: u32, v: u32) -> R {
        self.buf(ptr)?.rolloff = f(v);
        self.refresh_3d(ptr)
    }

    /// `SetConeOutsideVolume(lConeOutsideVolume, dwApply)`, millibels.
    pub fn buffer_set_cone_outside_volume(&mut self, ptr: u32, mb: u32) -> R {
        self.buf(ptr)?.cone_outside_mb = (mb as i32).clamp(-10_000, 0);
        self.refresh_3d(ptr)
    }

    /// `SetDopplerFactor(flDopplerFactor, dwApply)`: stored and counted;
    /// Doppler shift is not modelled.
    pub fn buffer_set_doppler(&mut self, ptr: u32, v: u32) -> R {
        self.buf(ptr)?.doppler = f(v);
        self.note("SetDopplerFactor");
        self.refresh_3d(ptr)
    }

    /// I3DL2 source settings (reverb, occlusion) are accepted and counted:
    /// reverb is not modelled.
    pub fn buffer_set_3d_extra(&mut self, ptr: u32, what: &'static str) -> R {
        self.buf(ptr)?;
        self.note(what);
        Ok(S_OK)
    }

    fn refresh_3d(&mut self, ptr: u32) -> R {
        self.sync_buffer(ptr)?;
        Ok(S_OK)
    }

    // ---- streams ---------------------------------------------------------

    /// `Process(pSrc, pDst)`: queues one packet. `pDst` is unused by
    /// DirectSound streams (verified: the real `Process` never touches it).
    pub fn stream_process(&mut self, h: &mut dyn Host, ptr: u32, src: u32, _dst: u32) -> R {
        // The real library completes packets from the APU interrupt, so a
        // game that polls status or capacity sees them without `DoWork`.
        self.service_stream(h, ptr)?;
        let mixer = Arc::clone(&self.mixer);
        let s = self.stream(ptr)?;
        let pkt = MediaPacket::read(h, src)?;
        let audio = s.audio;
        let at_capacity = s.max_packets != 0 && s.packets.len() as u32 >= s.max_packets;
        if at_capacity {
            return Ok(DSERR_INVALIDCALL);
        }
        let bytes = h.read_vec(pkt.buffer, pkt.max_size)?;
        let align = {
            let mut m = mixer.lock().unwrap();
            let st = m.stream_mut(audio).ok_or(Stop::BadHandle {
                ptr,
                expected: "IDirectSoundStream",
            })?;
            st.format().block_align()
        };
        if bytes.len() % align != 0 {
            return Ok(E_INVALIDARG);
        }
        // Writes first: nothing is queued if the guest's pointers fault.
        if pkt.status_ptr != 0 {
            h.write_u32(pkt.status_ptr, XMEDIAPACKET_STATUS_PENDING)?;
        }
        if pkt.completed_size_ptr != 0 {
            h.write_u32(pkt.completed_size_ptr, 0)?;
        }
        // Zero-length packets stay out of the audio crate (it would complete
        // them ahead of earlier packets); `service_stream` releases them in
        // order.
        let id = if bytes.is_empty() {
            None
        } else {
            let mut m = mixer.lock().unwrap();
            let st = m.stream_mut(audio).ok_or(Stop::BadHandle {
                ptr,
                expected: "IDirectSoundStream",
            })?;
            match st.submit(&bytes) {
                Ok(id) => Some(id),
                Err(_) => return Ok(E_INVALIDARG),
            }
        };
        self.next_token += 1;
        let token = self.next_token;
        let s = self.stream(ptr)?;
        s.started = true;
        s.voice_active = true;
        // Keep the kernel event alive until the packet completes. The signal
        // comes after every status write, whatever the guest does with the
        // handle meanwhile.
        let event_packet = s.callback == 0 && pkt.event_or_context != 0;
        if event_packet {
            h.retain_event(pkt.event_or_context, token);
        }
        s.packets.push_back(Slot {
            audio: id,
            rec: PacketRecord {
                token: if event_packet { token } else { 0 },
                size: pkt.max_size,
                completed_size_ptr: pkt.completed_size_ptr,
                status_ptr: pkt.status_ptr,
                event_or_context: pkt.event_or_context,
            },
            done: None,
        });
        self.service_stream(h, ptr)?;
        Ok(S_OK)
    }

    /// `GetStatus(pdwStatus)`, after `CMcpxStream_GetStatus` (0x37f591):
    /// bit 0 when another packet can be queued, and bit 17 (`0x20000`)
    /// when the active voice is paused or held. Halo tests that bit
    /// (0x2ae4c4) before calling `Pause(0)`. Bits 16, 18 and 19 (starved,
    /// hardware-paused, and the `0x2000` state flag) are not produced.
    pub fn stream_get_status(&mut self, h: &mut dyn Host, ptr: u32, out: u32) -> R {
        self.service_stream(h, ptr)?;
        let s = self.stream(ptr)?;
        let ready = s.max_packets == 0 || (s.packets.len() as u32) < s.max_packets;
        let mut st = if ready {
            XMO_STATUSF_ACCEPT_INPUT_DATA
        } else {
            0
        };
        if s.started && s.gate != Gate::Open {
            st |= XMO_STATUSF_PAUSED;
        }
        h.write_u32(out, st)?;
        Ok(S_OK)
    }

    /// `Flush` / `FlushEx(rtTimestamp, dwFlags)` (0x37aded): pending packets
    /// complete with `XMEDIAPACKET_STATUS_FLUSHED`.
    ///
    /// Flags 0 take the plain flush (0x37fbfe). Non-zero flags take
    /// `CMcpxStream_Stop` (0x37f51f), which only tests bits 2 and 4
    /// (`DSSTREAMFLUSHEX_ENVELOPE`, `_IMMEDIATE`): they choose how the voice
    /// is released (envelope, then a deferred "complete" command through the
    /// stream's schedule slot 7 at 0x37f8dc). `_ASYNC` (1) is not tested at
    /// all. The shim has no envelope and no deferral, so every flag value
    /// flushes at once, the same as flags 0; the packets complete before the
    /// call returns. Halo uses flags 1 and 3 with timestamp 0 (0x2ae4e2,
    /// 0x333ef6, 0x335744, 0x33598d, 0x335d13).
    ///
    /// A non-zero timestamp is scheduled by the real library
    /// (`[vtable+0x1c](4, lo, hi, flags)`, 0x37f9ee) against the DSP clock;
    /// no call site in the XBE passes one, so it stays a stop.
    pub fn stream_flush(&mut self, h: &mut dyn Host, ptr: u32, timestamp: u64, flags: u32) -> R {
        if timestamp != 0 {
            return Err(unimpl(
                "FlushEx with a future timestamp (scheduled on the DSP clock)",
            ));
        }
        if flags != 0 {
            self.note("FlushEx(flags)");
        }
        let mixer = Arc::clone(&self.mixer);
        let s = self.stream(ptr)?;
        // Empty packets behind a playing one never reached the audio crate:
        // abort them here so the flush covers them too.
        for slot in s
            .packets
            .iter_mut()
            .filter(|p| p.audio.is_none() && p.done.is_none())
        {
            slot.done = Some(XMEDIAPACKET_STATUS_FLUSHED);
        }
        // A flush stops the voice (0x381ef8 clears state bit 0); the next
        // `Process` starts it again.
        s.voice_active = false;
        let audio = s.audio;
        if let Some(st) = mixer.lock().unwrap().stream_mut(audio) {
            st.flush();
        }
        self.service_stream(h, ptr)?;
        Ok(S_OK)
    }

    /// `GetInfo(XMEDIAINFO*)` (0x37abd5): `{ dwFlags, dwInputSize,
    /// dwOutputSize, dwMaxLookahead }`. Flags are always 5
    /// (`XMO_STREAMF_FIXED_SAMPLE_SIZE | XMO_STREAMF_INPUT_ASYNC`); the input
    /// size is the low byte of `nBlockAlign` (the unit `Process` accepts),
    /// the output size 0 and the lookahead twice the larger of that and 32
    /// sample frames (0x379eeb). Nothing in the XBE reaches this slot (the
    /// `[reg+8]` virtual calls near the stream users belong to Halo's own
    /// objects); it is here because the flat vtable slot is overridden.
    pub fn stream_get_info(&mut self, h: &mut dyn Host, ptr: u32, out: u32) -> R {
        let info = self.stream(ptr)?.info;
        let words = [5, u32::from(info.block_align), 0, info.max_lookahead()];
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        h.write(out, &bytes)?;
        Ok(S_OK)
    }

    /// `Discontinuity`: marks the end of a segment. The software mixer keeps
    /// no cross-packet state to reset, so the call is counted and succeeds.
    pub fn stream_discontinuity(&mut self, ptr: u32) -> R {
        self.stream(ptr)?.discontinuities += 1;
        Ok(S_OK)
    }

    /// `Pause(dwPause)`; 0 resumes, 1 pauses (2 and 3 are the synchronised
    /// variants and act immediately).
    pub fn stream_pause(&mut self, ptr: u32, mode: u32) -> R {
        if mode > 3 {
            return Ok(E_INVALIDARG);
        }
        let mixer = Arc::clone(&self.mixer);
        let s = self.stream(ptr)?;
        match mode {
            0 | 3 => {
                if mode == 3 && s.gate == Gate::Open && !s.started {
                    return Err(unimpl(
                        "Pause(3) on a stream that is not active (deferred start)",
                    ));
                }
                self.release_stream_gate(ptr)?;
            }
            _ => {
                s.gate = if mode == 1 { Gate::Paused } else { Gate::Held };
                let audio = s.audio;
                if let Some(st) = mixer.lock().unwrap().stream_mut(audio) {
                    st.pause(true);
                }
            }
        }
        Ok(S_OK)
    }

    pub fn stream_set_volume(&mut self, ptr: u32, mb: i32) -> R {
        if !(-10_000..=0).contains(&mb) {
            return Ok(E_INVALIDARG);
        }
        let mixer = Arc::clone(&self.mixer);
        let s = self.stream(ptr)?;
        s.volume_mb = mb;
        let audio = s.audio;
        if let Some(st) = mixer.lock().unwrap().stream_mut(audio) {
            st.set_volume(mb);
        }
        Ok(S_OK)
    }

    /// `SetPitch(lPitch)`: 1/4096 octave about 48 kHz.
    pub fn stream_set_pitch(&mut self, ptr: u32, pitch: i32) -> R {
        let mixer = Arc::clone(&self.mixer);
        let audio = self.stream(ptr)?.audio;
        if let Some(st) = mixer.lock().unwrap().stream_mut(audio) {
            st.set_frequency(pitch_to_hz(pitch));
        }
        Ok(S_OK)
    }

    pub fn stream_set_headroom(&mut self, ptr: u32, headroom: u32) -> R {
        self.stream(ptr)?.headroom = headroom as i32;
        self.note("SetHeadroom");
        Ok(S_OK)
    }

    pub fn stream_set_mix_bins(&mut self, h: &mut dyn Host, ptr: u32, bins: u32) -> R {
        let given = if bins != 0 {
            Some(read_mix_bins(h, bins)?)
        } else {
            None
        };
        let s = self.stream(ptr)?;
        s.mix_bins = match given {
            Some(v) => v,
            None => default_mix_bins(s.flags, usize::from(s.info.channels)),
        };
        self.note("SetMixBins");
        self.sync_stream_bins(ptr)?;
        Ok(S_OK)
    }

    pub fn stream_set_mix_bin_volumes(&mut self, h: &mut dyn Host, ptr: u32, bins: u32) -> R {
        let v = read_mix_bins(h, bins)?;
        let s = self.stream(ptr)?;
        for (bin, vol) in v {
            match s.mix_bins.iter_mut().find(|p| p.0 == bin) {
                Some(p) => p.1 = vol,
                None => s.mix_bins.push((bin, vol)),
            }
        }
        self.note("SetMixBinVolumes");
        self.sync_stream_bins(ptr)?;
        Ok(S_OK)
    }

    /// Filter, LFO and EG on a stream: accepted and counted.
    pub fn stream_set_timbre(&mut self, ptr: u32, what: &'static str) -> R {
        self.stream(ptr)?;
        self.note(what);
        Ok(S_OK)
    }

    pub fn stream_discontinuity_count(&mut self, ptr: u32) -> Result<u32, Stop> {
        Ok(self.stream(ptr)?.discontinuities)
    }

    // ---- voice properties ------------------------------------------------

    /// `GetVoiceProperties` on a buffer or stream (0x3818e5), the output
    /// Halo reads back to diff mix-bin routing (`0x2215d0` compares the count
    /// and every pair with what it wants and calls `SetMixBins` or
    /// `SetMixBinVolumes` on a mismatch):
    ///
    /// ```text
    /// +0x00 dwMixBinCount
    /// +0x04 8 pairs { dwMixBin, lVolume }; unused slots are { -1, -10000 }
    /// +0x44 voice level word (hardware, not modelled: 0)
    /// +0x48 five dwords of the 3D/LFO state (0 unless set; not modelled)
    /// ```
    ///
    /// `0x5c` bytes in all. A voice that is not active (state bit 0 of
    /// `+0x12`, set by start and cleared by stop) is `DSERR_INVALIDCALL` and
    /// nothing is written, as in the library; Halo ignores the result and
    /// reads uninitialised stack in that case. The volumes are what the game
    /// set, in millibels; the hardware quantises them to 1/64 dB steps and
    /// that is not reproduced.
    fn write_voice_props(
        &self,
        h: &mut dyn Host,
        out: u32,
        bins: &[(u32, i32)],
    ) -> Result<(), Stop> {
        let mut words = [0u32; (VOICE_PROPS_SIZE / 4) as usize];
        let n = bins.len().min(VOICE_MIX_BIN_SLOTS);
        words[0] = n as u32;
        for i in 0..VOICE_MIX_BIN_SLOTS {
            let (bin, vol) = bins
                .get(i)
                .map_or(VOICE_PROPS_UNUSED, |&(b, v)| (b, v as u32));
            words[1 + 2 * i] = bin;
            words[2 + 2 * i] = vol;
        }
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        h.write(out, &bytes)?;
        Ok(())
    }

    pub fn buffer_get_voice_properties(&mut self, h: &mut dyn Host, ptr: u32, out: u32) -> R {
        let playing = self.with_audio(ptr, |_, sb| sb.is_playing())?;
        let b = self.buf(ptr)?;
        let active = if b.mix_in {
            b.mix_in_playing
        } else {
            playing == Some(true) || (b.gate != Gate::Open && b.resume.is_some())
        };
        if !active {
            return Ok(DSERR_INVALIDCALL);
        }
        let bins = b.mix_bins.clone();
        self.write_voice_props(h, out, &bins)?;
        Ok(S_OK)
    }

    pub fn stream_get_voice_properties(&mut self, h: &mut dyn Host, ptr: u32, out: u32) -> R {
        let s = self.stream(ptr)?;
        if !s.voice_active {
            return Ok(DSERR_INVALIDCALL);
        }
        let bins = s.mix_bins.clone();
        self.write_voice_props(h, out, &bins)?;
        Ok(S_OK)
    }

    // ---- completion ------------------------------------------------------

    /// Completes finished packets of one stream: status and completed size
    /// to guest memory, then the callback or event. Packets complete in
    /// submission order (the audio crate reports them that way).
    fn service_stream(&mut self, h: &mut dyn Host, ptr: u32) -> Result<(), Stop> {
        let mixer = Arc::clone(&self.mixer);
        let s = self.stream(ptr)?;
        let audio = s.audio;
        let done = match mixer.lock().unwrap().stream_mut(audio) {
            Some(st) => st.take_completed(),
            None => return Ok(()),
        };
        for (id, status) in done {
            let code = match status {
                PacketStatus::Success => XMEDIAPACKET_STATUS_SUCCESS,
                PacketStatus::Flushed => XMEDIAPACKET_STATUS_FLUSHED,
            };
            if let Some(slot) = self
                .stream(ptr)?
                .packets
                .iter_mut()
                .find(|p| p.audio == Some(id))
            {
                slot.done = Some(code);
            }
        }
        // Release from the front only, so completion order is submission
        // order even when an empty packet sits behind a playing one.
        loop {
            let s = self.stream(ptr)?;
            let Some(front) = s.packets.front_mut() else {
                break;
            };
            if front.audio.is_none() && front.done.is_none() {
                front.done = Some(XMEDIAPACKET_STATUS_SUCCESS);
            }
            let Some(code) = front.done else { break };
            // The record stays queued until both writes succeed: a fault
            // leaves the packet (and its event reference) pending, and the
            // next service retries the idempotent writes.
            let rec = front.rec;
            // Verified (0x37f850 -> 0x37f3a8): the completed size written is
            // always the packet's dwMaxSize, flushed or not.
            let size = rec.size;
            if rec.completed_size_ptr != 0 {
                h.write_u32(rec.completed_size_ptr, size)?;
            }
            if rec.status_ptr != 0 {
                h.write_u32(rec.status_ptr, code)?;
            }
            let (callback, ctx) = (s.callback, s.callback_context);
            s.packets.pop_front();
            if callback != 0 {
                h.stream_callback(StreamCallback {
                    function: callback,
                    stream_context: ctx,
                    packet_context: rec.event_or_context,
                    status: code,
                });
            } else if rec.event_or_context != 0 {
                h.signal_event(rec.event_or_context, rec.token);
            }
        }
        Ok(())
    }

    /// `DirectSoundDoWork`: services every stream's completions. Call it
    /// from the override and also after each host mix if completions should
    /// not wait for the game's next `DoWork`.
    ///
    /// With [`DirectSound::set_frames_per_do_work`] set it first advances the
    /// audio clock by that many frames, for a host with no mixer sink.
    pub fn do_work(&mut self, h: &mut dyn Host) -> R {
        if let Some(frames) = self.frames_per_do_work {
            self.render_discard(frames);
        }
        self.service_all(h)
    }

    fn service_all(&mut self, h: &mut dyn Host) -> R {
        let mut streams: Vec<u32> = self
            .objects
            .iter()
            .filter_map(|(p, o)| matches!(o, Object::Stream(_)).then_some(*p))
            .collect();
        streams.sort_unstable();
        for p in streams {
            self.service_stream(h, p)?;
        }
        Ok(S_OK)
    }

    /// Controlled audio clock for a host that has no mixer sink (headless
    /// runs, tests): renders `frames` output frames of the mixer, throws the
    /// samples away, then completes whatever that finished. Buffers play and
    /// stream packets complete exactly as a device would have driven them.
    /// Never call this while something else (a device callback, the app's
    /// discard renderer) also consumes the mixer: time would run twice.
    /// Returns the frames rendered.
    pub fn advance_audio(&mut self, h: &mut dyn Host, frames: u64) -> Result<u64, Stop> {
        self.advance_audio_as(h, frames, Consumer::Manual)
    }

    /// [`advance_audio`](Self::advance_audio) on behalf of `who`, which must
    /// hold (or be able to claim) the mixer.
    pub fn advance_audio_as(
        &mut self,
        h: &mut dyn Host,
        frames: u64,
        who: Consumer,
    ) -> Result<u64, Stop> {
        self.claim_consumer(who)?;
        self.render_discard(frames);
        self.service_all(h)?;
        Ok(frames)
    }

    /// Makes every `DirectSoundDoWork` advance the clock by `frames` output
    /// frames (for example `rate / 60` for a game that calls it once per
    /// frame). `None`, the default, leaves the mixer to its sink. Claims the
    /// mixer as [`Consumer::DoWork`] (and `None` gives it back).
    pub fn set_frames_per_do_work(&mut self, frames: Option<u64>) -> Result<(), ConsumerConflict> {
        match frames {
            Some(_) => self.claim_consumer(Consumer::DoWork)?,
            None => self.release_consumer(Consumer::DoWork),
        }
        self.frames_per_do_work = frames;
        Ok(())
    }

    /// Takes the mixer for `who`. Claiming again for the same consumer is a
    /// no-op; any other consumer is refused until it is released.
    pub fn claim_consumer(&mut self, who: Consumer) -> Result<(), ConsumerConflict> {
        match self.consumer {
            Some(held) if held != who => Err(ConsumerConflict { held, wanted: who }),
            _ => {
                self.consumer = Some(who);
                Ok(())
            }
        }
    }

    /// Gives the mixer back if `who` holds it.
    pub fn release_consumer(&mut self, who: Consumer) {
        if self.consumer == Some(who) {
            self.consumer = None;
        }
    }

    /// Who renders the mixer, if anyone has claimed it.
    pub fn consumer(&self) -> Option<Consumer> {
        self.consumer
    }

    fn render_discard(&mut self, frames: u64) {
        let mut scratch = vec![0.0f32; 2 * RENDER_CHUNK as usize];
        let mut left = frames;
        let mut mixer = self.mixer.lock().unwrap_or_else(|e| e.into_inner());
        while left > 0 {
            let n = left.min(RENDER_CHUNK);
            mixer.render(&mut scratch[..2 * n as usize]);
            left -= n;
        }
        self.frames_rendered += frames;
    }

    /// Frames the shim itself has rendered through [`DirectSound::advance_audio`]
    /// or `set_frames_per_do_work`.
    pub fn frames_rendered(&self) -> u64 {
        self.frames_rendered
    }
}

/// Largest single `Mixer::render` the discard clock asks for, in frames.
const RENDER_CHUNK: u64 = 1024;

/// Turns elapsed host time into whole audio frames at a fixed rate, carrying
/// the fraction, and caps a long stall so a stuck host does not owe seconds of
/// audio in one go. It holds no clock of its own: the caller supplies time, so
/// a test or a deterministic host controls it exactly.
#[derive(Clone, Debug)]
pub struct AudioClock {
    rate: u64,
    /// Nanoseconds not yet converted to a whole frame, scaled by `rate`.
    carry: u128,
    max_frames: u64,
}

impl AudioClock {
    /// `rate` frames per second, with at most `max_catch_up` of audio owed per
    /// call to [`advance`](Self::advance).
    pub fn new(rate: u32, max_catch_up: std::time::Duration) -> Self {
        let rate = u64::from(rate.max(1));
        let max_frames = ((u128::from(rate) * max_catch_up.as_nanos()) / 1_000_000_000) as u64;
        Self {
            rate,
            carry: 0,
            max_frames: max_frames.max(1),
        }
    }

    /// The frames due after `elapsed` more host time.
    pub fn advance(&mut self, elapsed: std::time::Duration) -> u64 {
        self.carry += elapsed.as_nanos() * u128::from(self.rate);
        let frames = (self.carry / 1_000_000_000) as u64;
        self.carry %= 1_000_000_000;
        if frames > self.max_frames {
            self.carry = 0;
            self.max_frames
        } else {
            frames
        }
    }
}
