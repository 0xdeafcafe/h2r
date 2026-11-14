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

use audio::dsound::{BufferId, Mode3d, PacketId, PacketStatus, Source3d, StreamId, Vec3};
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
}

pub type R = Result<u32, Stop>;

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

/// Bytes allocated for the returned `DSEFFECTIMAGEDESC`. Only the leading
/// `dwEffectCount` is evidenced as read; the rest (the effect maps) is zero
/// padding sized for 15 entries of up to 16 bytes.
const EFFECT_IMAGE_DESC_SIZE: u32 = 4 + 15 * 16;
/// Per-effect parameter bytes the dry model keeps (offsets and sizes
/// beyond are `E_INVALIDARG`).
const EFFECT_STATE_MAX: u32 = 0x1000;

#[derive(Clone, Copy, Debug)]
struct EffectsImage {
    desc: u32,
    count: u32,
}

impl Default for Layout {
    fn default() -> Self {
        Self { stream_vtable: 0x0041_7170, effect_count: 15 }
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
}

#[derive(Debug)]
struct PacketRecord {
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
    effect_data: BTreeMap<u32, Vec<u8>>,
    effects: Option<EffectsImage>,
    listener: Listener3d,
    light_hrtf_4_channel: bool,
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
            effect_data: BTreeMap::new(),
            effects: None,
            listener: Listener3d::default(),
            light_hrtf_4_channel: false,
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
        let Some(ptr) = h.alloc(OBJECT_SIZE, 16) else { return Err(E_OUTOFMEMORY) };
        if matches!(obj, Object::Stream(_)) && h.write_u32(ptr, self.layout.stream_vtable).is_err() {
            h.free(ptr);
            return Err(E_FAIL);
        }
        self.objects.insert(ptr, obj);
        Ok(ptr)
    }

    fn buf(&mut self, ptr: u32) -> Result<&mut BufferObj, Stop> {
        match self.objects.get_mut(&ptr) {
            Some(Object::Buffer(b)) => Ok(b),
            _ => Err(Stop::BadHandle { ptr, expected: "IDirectSoundBuffer" }),
        }
    }

    fn stream(&mut self, ptr: u32) -> Result<&mut StreamObj, Stop> {
        match self.objects.get_mut(&ptr) {
            Some(Object::Stream(s)) => Ok(s),
            _ => Err(Stop::BadHandle { ptr, expected: "IDirectSoundStream" }),
        }
    }

    fn check_root(&self, ptr: u32) -> Result<(), Stop> {
        match self.objects.get(&ptr) {
            Some(Object::Root(_)) => Ok(()),
            _ => Err(Stop::BadHandle { ptr, expected: "IDirectSound" }),
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
            None => Err(Stop::BadHandle { ptr, expected: "DirectSound object" }),
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
            None => return Err(Stop::BadHandle { ptr, expected: "DirectSound object" }),
        };
        if refs == 0 {
            match self.objects.remove(&ptr) {
                Some(Object::Root(_)) => self.root = None,
                Some(Object::Buffer(b)) => {
                    if let Some(id) = b.audio {
                        self.mixer.lock().unwrap().remove_buffer(id);
                    }
                    if let Some((base, _)) = b.data.and_then(|d| d.alloc) {
                        h.free(base);
                    }
                }
                Some(Object::Stream(s)) => {
                    self.mixer.lock().unwrap().remove_stream(s.audio);
                    // Packets still attached never complete: the real stream
                    // drops them the same way. Their status stays PENDING.
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
        let Some(r) = b.resume.take() else { return Ok(()) };
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
    pub fn set_effect_data(&mut self, h: &mut dyn Host, ds: u32, index: u32, offset: u32, data: u32, size: u32) -> R {
        self.check_root(ds)?;
        let Some(img) = self.effects else { return Ok(DSERR_INVALIDCALL) };
        let Some(end) = offset.checked_add(size).filter(|&e| index < img.count && e <= EFFECT_STATE_MAX) else {
            return Ok(E_INVALIDARG);
        };
        let bytes = h.read_vec(data, size)?;
        let state = self.effect_data.entry(index).or_default();
        if state.len() < end as usize {
            state.resize(end as usize, 0);
        }
        state[offset as usize..end as usize].copy_from_slice(&bytes);
        self.note("SetEffectData");
        Ok(S_OK)
    }

    /// `GetEffectData`: bytes the game stored, and zeros elsewhere. The
    /// zeros stand in for the DSP image's initial parameter RAM, which is
    /// not reproduced (see `docs/dsound.md`, "Dry effects image").
    pub fn get_effect_data(&mut self, h: &mut dyn Host, ds: u32, index: u32, offset: u32, out: u32, size: u32) -> R {
        self.check_root(ds)?;
        let Some(img) = self.effects else { return Ok(DSERR_INVALIDCALL) };
        let Some(end) = offset.checked_add(size).filter(|&e| index < img.count && e <= EFFECT_STATE_MAX) else {
            return Ok(E_INVALIDARG);
        };
        let mut bytes = vec![0u8; size as usize];
        if let Some(state) = self.effect_data.get(&index) {
            let have = state.len().min(end as usize);
            if have > offset as usize {
                bytes[..have - offset as usize].copy_from_slice(&state[offset as usize..have]);
            }
        }
        h.write(out, &bytes)?;
        Ok(S_OK)
    }

    /// `XAudioDownloadEffectsImage(pszImageName, pImageLoc, dwFlags,
    /// ppImageDesc)`. A dry image: the DSP program is never run, but the
    /// descriptor Halo reads back is built so init proceeds. Flag 1 is the
    /// XBE-section form Halo uses (name `"DSPImage"`); loading from a file is
    /// a stop.
    ///
    /// `pImageLoc` is `{ dwI3DL2ReverbIndex, dwCrosstalkIndex }` (Halo passes
    /// 9 and 10); both must name effects in the image.
    pub fn download_effects_image(&mut self, h: &mut dyn Host, name: u32, loc: u32, flags: u32, pp_desc: u32) -> R {
        if flags != 1 {
            return Err(unimpl("XAudioDownloadEffectsImage from a file (only the XBE-section form is modelled)"));
        }
        if name == 0 || loc == 0 || pp_desc == 0 {
            return Ok(E_INVALIDARG);
        }
        let mut got = [0u8; 9];
        h.read(name, &mut got)?;
        if &got != b"DSPImage\0" {
            return Err(unimpl("XAudioDownloadEffectsImage with an image name other than \"DSPImage\""));
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
                let Some(desc) = h.alloc(EFFECT_IMAGE_DESC_SIZE, 16) else { return Ok(E_OUTOFMEMORY) };
                if let Err(e) = h.write_u32(desc, count) {
                    h.free(desc);
                    return Err(e.into());
                }
                self.effects = Some(EffectsImage { desc, count });
                desc
            }
        };
        h.write_u32(pp_desc, desc)?;
        self.note("XAudioDownloadEffectsImage (dry image)");
        Ok(S_OK)
    }

    /// `XAudioSetEffectData(dwEffectIndex, pDesc, pOut)`. The binary reads
    /// the descriptor, converts its parameters, updates effect state and may
    /// write through the third argument (an output pointer, not `dwApply`).
    /// None of that is modelled, so this is a stop rather than a fake success.
    pub fn xaudio_set_effect_data(&mut self, _index: u32) -> R {
        Err(unimpl("XAudioSetEffectData (descriptor conversion and output pointer not modelled)"))
    }

    // ---- creation --------------------------------------------------------

    /// `IDirectSound::CreateSoundBuffer(pdsbd, ppBuffer, pOuter)`.
    pub fn create_sound_buffer(&mut self, h: &mut dyn Host, ds: u32, desc: u32, pp: u32, outer: u32) -> R {
        self.check_root(ds)?;
        if outer != 0 {
            return Ok(E_INVALIDARG);
        }
        let d = BufferDesc::read(h, desc)?;
        if d.size != BufferDesc::SIZE {
            return Ok(E_INVALIDARG);
        }
        let mix_in = d.flags & DSBCAPS_MIXIN_LIKE_MASK != 0;
        let mix_bins = if d.mix_bins_ptr != 0 { read_mix_bins(h, d.mix_bins_ptr)? } else { Vec::new() };
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
            return Err(unimpl("data buffer created without a format (needs SetFormat)"));
        }
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
        };
        if let (Some(fmt), true) = (format, d.buffer_bytes != 0) {
            let align = fmt.block_align() as u32;
            if d.buffer_bytes % align != 0 || d.buffer_bytes > MAX_GUEST_COPY {
                return Ok(E_INVALIDARG);
            }
            let Some(p) = h.alloc(d.buffer_bytes, 16) else { return Ok(E_OUTOFMEMORY) };
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
            obj.data = Some(GuestData { ptr: p, size: d.buffer_bytes, alloc: Some((p, d.buffer_bytes)) });
        }
        let (audio, data) = (obj.audio, obj.data.as_ref().and_then(|d| d.alloc));
        let rollback = |this: &mut Self, h: &mut dyn Host| {
            if let Some(id) = audio {
                this.mixer.lock().unwrap().remove_buffer(id);
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
    pub fn create_sound_stream(&mut self, h: &mut dyn Host, ds: u32, desc: u32, pp: u32, outer: u32) -> R {
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
        let mix_bins = if d.mix_bins_ptr != 0 { read_mix_bins(h, d.mix_bins_ptr)? } else { Vec::new() };
        let stream = Stream::new(format).map_err(|_| unimpl("stream format"))?;
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
        sb.set_source_3d(match (b.flags & DSBCAPS_CTRL3D != 0, b.position) {
            (true, Some([x, y, z])) => Some(Source3d {
                position: Vec3::new(x, y, z),
                min_distance: b.min_distance,
                max_distance: b.max_distance,
                mode: Mode3d::Normal,
                ..Default::default()
            }),
            _ => None,
        });
    }

    fn with_audio<T>(&mut self, ptr: u32, op: impl FnOnce(&BufferObj, &mut SoundBuffer) -> T) -> Result<Option<T>, Stop> {
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
        let Some(fmt) = b.format else { return Err(unimpl("SetBufferData on a mix-in buffer")) };
        if b.audio.is_some_and(|id| mixer.lock().unwrap().buffer(id).is_some_and(|s| s.is_playing())) {
            return Ok(DSERR_INVALIDCALL);
        }
        if bytes == 0 || !bytes.is_multiple_of(fmt.block_align() as u32) {
            return Ok(E_INVALIDARG);
        }
        // Same pointer and size: the original does nothing. (Replacing would
        // free an owned allocation that the buffer then keeps using.)
        if b.data.as_ref().is_some_and(|d| d.ptr == pv && d.size == bytes) {
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
            Some((base, len)) if pv >= base && pv.checked_add(bytes).is_some_and(|e| e <= base + len) => {
                Some((base, len))
            }
            Some((base, _)) => {
                h.free(base);
                None
            }
            None => None,
        };
        b.data = Some(GuestData { ptr: pv, size: bytes, alloc });
        Ok(S_OK)
    }

    /// Re-reads the guest bytes into the audio buffer.
    fn resync(&mut self, h: &dyn crate::host::GuestMemory, ptr: u32) -> Result<(), Stop> {
        let mixer = Arc::clone(&self.mixer);
        let b = self.buf(ptr)?;
        let (Some(d), Some(id)) = (&b.data, b.audio) else { return Ok(()) };
        let bytes = h.read_vec(d.ptr, d.size)?;
        if let Some(sb) = mixer.lock().unwrap().buffer_mut(id) {
            sb.lock_all().write(&bytes).map_err(|_| unimpl("buffer resync size"))?;
        }
        Ok(())
    }

    /// `Lock(dwOffset, dwBytes, ppv1, pcb1, ppv2, pcb2, dwFlags)`. Returns
    /// pointers into the buffer's guest memory, as the hardware does.
    #[allow(clippy::too_many_arguments)]
    pub fn lock(&mut self, h: &mut dyn Host, ptr: u32, offset: u32, bytes: u32, pp1: u32, pcb1: u32, pp2: u32, pcb2: u32, flags: u32) -> R {
        let b = self.buf(ptr)?;
        let (Some(fmt), Some(d)) = (b.format, &b.data) else { return Ok(DSERR_INVALIDCALL) };
        if flags & !(DSBLOCK_FROMWRITECURSOR | DSBLOCK_ENTIREBUFFER) != 0 {
            return Err(unimpl("Lock with undocumented flags"));
        }
        let (base, size) = (d.ptr, d.size);
        let (offset, bytes) = if flags & DSBLOCK_ENTIREBUFFER != 0 { (0, size) } else { (offset, bytes) };
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
            self.note("mix-in buffer Play (no DSP, silent)");
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
            b.resume = Some(Resume { looping, from_start });
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
                        return Err(unimpl("Pause(3) on a voice that is not active (deferred start)"));
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
                    b.resume = (was_playing == Some(true)).then_some(Resume { looping, from_start: false });
                }
                self.buf(ptr)?.gate = want;
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
        let Some(pos) = pos else { return Ok(DSERR_INVALIDCALL) };
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
        self.with_audio(ptr, |b, sb| sb.set_volume(b.volume_mb))?;
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
        let v = if bins != 0 { read_mix_bins(h, bins)? } else { Vec::new() };
        self.buf(ptr)?.mix_bins = v;
        self.note("SetMixBins");
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

    /// Doppler, rolloff, cone volume and I3DL2 source settings are accepted
    /// and counted (the audio crate's 3D model has no such terms).
    pub fn buffer_set_3d_extra(&mut self, ptr: u32, what: &'static str) -> R {
        self.buf(ptr)?;
        self.note(what);
        Ok(S_OK)
    }

    fn refresh_3d(&mut self, ptr: u32) -> R {
        self.with_audio(ptr, Self::apply_params)?;
        Ok(S_OK)
    }

    // ---- streams ---------------------------------------------------------

    /// `Process(pSrc, pDst)`: queues one packet. `pDst` is unused by
    /// DirectSound streams (verified: the real `Process` never touches it).
    pub fn stream_process(&mut self, h: &mut dyn Host, ptr: u32, src: u32, _dst: u32) -> R {
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
            let st = m.stream_mut(audio).ok_or(Stop::BadHandle { ptr, expected: "IDirectSoundStream" })?;
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
            let st = m.stream_mut(audio).ok_or(Stop::BadHandle { ptr, expected: "IDirectSoundStream" })?;
            match st.submit(&bytes) {
                Ok(id) => Some(id),
                Err(_) => return Ok(E_INVALIDARG),
            }
        };
        let s = self.stream(ptr)?;
        s.started = true;
        s.packets.push_back(Slot {
            audio: id,
            rec: PacketRecord {
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
        let s = self.stream(ptr)?;
        let ready = s.max_packets == 0 || (s.packets.len() as u32) < s.max_packets;
        let mut st = if ready { XMO_STATUSF_ACCEPT_INPUT_DATA } else { 0 };
        if s.started && s.gate != Gate::Open {
            st |= XMO_STATUSF_PAUSED;
        }
        h.write_u32(out, st)?;
        Ok(S_OK)
    }

    /// `Flush` / `FlushEx(rtTimestamp, dwFlags)`: pending packets complete
    /// with `XMEDIAPACKET_STATUS_FLUSHED`. Deferred (timestamped) flushes
    /// are not modelled.
    pub fn stream_flush(&mut self, h: &mut dyn Host, ptr: u32, timestamp: u64, flags: u32) -> R {
        if timestamp != 0 {
            return Err(unimpl("FlushEx with a future timestamp"));
        }
        if flags != 0 {
            // Verified (0x37adED): non-zero flags leave the plain flush
            // (0x37fbfe) for CMcpxStream_Stop (0x37f51f), whose bits 2/4
            // select envelope-release and sync-stop paths. Not modelled.
            return Err(unimpl("FlushEx with flags (envelope/async stop path)"));
        }
        let mixer = Arc::clone(&self.mixer);
        let s = self.stream(ptr)?;
        // Empty packets behind a playing one never reached the audio crate:
        // abort them here so the flush covers them too.
        for slot in s.packets.iter_mut().filter(|p| p.audio.is_none() && p.done.is_none()) {
            slot.done = Some(XMEDIAPACKET_STATUS_FLUSHED);
        }
        let audio = s.audio;
        if let Some(st) = mixer.lock().unwrap().stream_mut(audio) {
            st.flush();
        }
        self.service_stream(h, ptr)?;
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
                    return Err(unimpl("Pause(3) on a stream that is not active (deferred start)"));
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
        let v = if bins != 0 { read_mix_bins(h, bins)? } else { Vec::new() };
        self.stream(ptr)?.mix_bins = v;
        self.note("SetMixBins");
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
            if let Some(slot) = self.stream(ptr)?.packets.iter_mut().find(|p| p.audio == Some(id)) {
                slot.done = Some(code);
            }
        }
        // Release from the front only, so completion order is submission
        // order even when an empty packet sits behind a playing one.
        loop {
            let s = self.stream(ptr)?;
            let Some(front) = s.packets.front_mut() else { break };
            if front.audio.is_none() && front.done.is_none() {
                front.done = Some(XMEDIAPACKET_STATUS_SUCCESS);
            }
            let Some(code) = front.done else { break };
            let Some(Slot { rec, .. }) = s.packets.pop_front() else { break };
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
            if callback != 0 {
                h.stream_callback(StreamCallback {
                    function: callback,
                    stream_context: ctx,
                    packet_context: rec.event_or_context,
                    status: code,
                });
            } else if rec.event_or_context != 0 {
                h.signal_event(rec.event_or_context);
            }
        }
        Ok(())
    }

    /// `DirectSoundDoWork`: services every stream's completions. Call it
    /// from the override and also after each host mix if completions should
    /// not wait for the game's next `DoWork`.
    pub fn do_work(&mut self, h: &mut dyn Host) -> R {
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
}
