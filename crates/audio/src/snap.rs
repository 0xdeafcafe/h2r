//! Machine snapshots (docs/runtime/snapshots.md, phase 4): the `audio`
//! section, the whole [`Mixer`].
//!
//! Every struct is destructured without `..`, so a field added later does
//! not compile until someone decides it:
//!
//! * **Save**: master volume, listener, every buffer, stream and mix-in
//!   under its id, the id counter, the render clock, the reverb and the
//!   samples tapped bins hold. A buffer keeps its copy of the guest's bytes
//!   (the copy was taken at `Unlock` or `Play`, and guest memory may have
//!   moved on since); a stream keeps its queued packets as decoded frames,
//!   since nothing remembers where in guest memory they came from. Effect
//!   state (clocks, envelope start, Doppler, filter and shelf memory, the
//!   pitch offset), the reverb and the binaural filters are saved bit for bit, so
//!   a load renders exactly the PCM the saving mixer would have.
//! * **Check**: the sample rate and the binaural and distance-reverb settings.
//! * **Rebuild**: the decoded copy of a buffer's bytes (decoded again),
//!   cached filter coefficients (recomputed from the same inputs on the
//!   next frame, so bit-equal), and the render scratch.
//! * **Ignore**: [`crate::StreamHealth`], diagnostics. The loading mixer
//!   keeps its counts.
//!
//! The encoders for the parameter types are public: `dsound` keeps copies
//! of voice effects and formats in its own section.
use std::collections::BTreeMap;

use snapshot::codec::{Reader, Writer};
use snapshot::{Codec as SectionCodec, Snapshot, SnapshotWriter, Tag};

use crate::dsound::{
    BufferId, HISTORY_FRAMES, Listener, MixIn, Mode3d, Packet, PacketId, PacketStatus, SoundBuffer,
    Source3d, Stream, StreamId, Vec3, VoiceOutput, VoiceParams,
};
use crate::format::{Codec, WaveFormat};
use crate::fx::{
    BiquadState, EnvSpec, FilterKind, FilterSpec, FxState, I3dl2Source, LfoSpec, Stage, VoiceFx,
};
use crate::hrtf::{Hrtf, Parameters};
use crate::mixer::{MixInId, Mixer, Tap};
use crate::reverb::{Allpass, Delay, I3dl2Listener, Reverb};

/// The section's tag.
pub const SECTION: Tag = Tag::new("audio");
// Master wrote version 1 for stereo and version 2 for binaural. Both
// predate per-voice room filter memory. Version 3 records the switches.
const VERSION: u32 = 3;

type Error = snapshot::Error;

/// Why a load did not happen.
#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    /// The section is damaged or missing.
    #[error(transparent)]
    Codec(#[from] snapshot::Error),
    /// The snapshot was saved by a mixer made differently.
    #[error("audio snapshot does not fit this mixer: {0}")]
    Mismatch(String),
}

fn corrupt(what: impl Into<String>) -> Error {
    Error::Corrupt("audio", what.into())
}

impl BufferId {
    /// The id as a number, for whoever keeps it in a snapshot.
    pub const fn raw(self) -> u32 {
        self.0
    }
    /// An id saved with [`BufferId::raw`].
    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }
}

impl StreamId {
    /// The id as a number, for whoever keeps it in a snapshot.
    pub const fn raw(self) -> u32 {
        self.0
    }
    /// An id saved with [`StreamId::raw`].
    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }
}

impl MixInId {
    /// The id as a number, for whoever keeps it in a snapshot.
    pub const fn raw(self) -> u32 {
        self.0
    }
    /// An id saved with [`MixInId::raw`].
    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }
}

// ---- scalars ---------------------------------------------------------------

/// An `f32` by its bits.
pub fn put_f32(w: &mut Writer, v: f32) {
    w.u32(v.to_bits());
}

pub fn get_f32(r: &mut Reader<'_>) -> Result<f32, Error> {
    Ok(f32::from_bits(r.u32()?))
}

fn put_f64(w: &mut Writer, v: f64) {
    w.u64(v.to_bits());
}

fn get_f64(r: &mut Reader<'_>) -> Result<f64, Error> {
    Ok(f64::from_bits(r.u64()?))
}

fn put_i32(w: &mut Writer, v: i32) {
    w.u32(v as u32);
}

fn get_i32(r: &mut Reader<'_>) -> Result<i32, Error> {
    Ok(r.u32()? as i32)
}

fn get_usize(r: &mut Reader<'_>) -> Result<usize, Error> {
    usize::try_from(r.u64()?).map_err(|_| corrupt("size too large"))
}

/// A presence byte, then the value.
pub fn put_opt<T>(w: &mut Writer, v: &Option<T>, f: impl FnOnce(&mut Writer, &T)) {
    w.bool(v.is_some());
    if let Some(v) = v {
        f(w, v);
    }
}

pub fn get_opt<'a, T>(
    r: &mut Reader<'a>,
    f: impl FnOnce(&mut Reader<'a>) -> Result<T, Error>,
) -> Result<Option<T>, Error> {
    if r.bool()? { f(r).map(Some) } else { Ok(None) }
}

/// A count, then each sample's bits.
pub fn put_f32s(w: &mut Writer, v: &[f32]) {
    w.list(v, |w, s| {
        w.u32(s.to_bits());
    });
}

pub fn get_f32s(r: &mut Reader<'_>) -> Result<Vec<f32>, Error> {
    r.list(4, |r| get_f32(r))
}

fn put_vec3(w: &mut Writer, v: &Vec3) {
    let Vec3 { x, y, z } = v;
    put_f32(w, *x);
    put_f32(w, *y);
    put_f32(w, *z);
}

fn get_vec3(r: &mut Reader<'_>) -> Result<Vec3, Error> {
    Ok(Vec3::new(get_f32(r)?, get_f32(r)?, get_f32(r)?))
}

// ---- formats and parameters ------------------------------------------------

pub fn put_format(w: &mut Writer, f: &WaveFormat) {
    w.u8(match f.codec() {
        Codec::Pcm8 => 0,
        Codec::Pcm16 => 1,
        Codec::XboxAdpcm => 2,
    });
    w.u32(u32::from(f.channels())).u32(f.sample_rate());
}

pub fn get_format(r: &mut Reader<'_>) -> Result<WaveFormat, Error> {
    let codec = match r.u8()? {
        0 => Codec::Pcm8,
        1 => Codec::Pcm16,
        2 => Codec::XboxAdpcm,
        c => return Err(corrupt(format!("codec {c}"))),
    };
    let channels = u16::try_from(r.u32()?).map_err(|_| corrupt("channel count"))?;
    let rate = r.u32()?;
    WaveFormat::new(codec, channels, rate).map_err(|e| corrupt(format!("format: {e}")))
}

fn put_stage(w: &mut Writer, s: &Stage) {
    let Stage {
        kind,
        hz,
        q,
        gain_db,
    } = s;
    w.u8(match kind {
        FilterKind::LowPass => 0,
        FilterKind::HighPass => 1,
        FilterKind::BandPass => 2,
        FilterKind::Notch => 3,
        FilterKind::Peaking => 4,
    });
    put_f32(w, *hz);
    put_f32(w, *q);
    put_f32(w, *gain_db);
}

fn get_stage(r: &mut Reader<'_>) -> Result<Stage, Error> {
    let kind = match r.u8()? {
        0 => FilterKind::LowPass,
        1 => FilterKind::HighPass,
        2 => FilterKind::BandPass,
        3 => FilterKind::Notch,
        4 => FilterKind::Peaking,
        k => return Err(corrupt(format!("filter kind {k}"))),
    };
    Ok(Stage {
        kind,
        hz: get_f32(r)?,
        q: get_f32(r)?,
        gain_db: get_f32(r)?,
    })
}

fn put_lfo(w: &mut Writer, l: &LfoSpec) {
    let LfoSpec {
        delay_s,
        rate_hz,
        pitch_oct,
        filter_oct,
        amp_db,
    } = l;
    for v in [delay_s, rate_hz, pitch_oct, filter_oct, amp_db] {
        put_f32(w, *v);
    }
}

fn get_lfo(r: &mut Reader<'_>) -> Result<LfoSpec, Error> {
    Ok(LfoSpec {
        delay_s: get_f32(r)?,
        rate_hz: get_f32(r)?,
        pitch_oct: get_f32(r)?,
        filter_oct: get_f32(r)?,
        amp_db: get_f32(r)?,
    })
}

fn put_env(w: &mut Writer, e: &EnvSpec) {
    let EnvSpec {
        start_stage,
        delay,
        attack,
        hold,
        decay,
        release,
        sustain,
    } = e;
    for v in [start_stage, delay, attack, hold, decay, release, sustain] {
        w.u32(*v);
    }
}

fn get_env(r: &mut Reader<'_>) -> Result<EnvSpec, Error> {
    Ok(EnvSpec {
        start_stage: r.u32()?,
        delay: r.u32()?,
        attack: r.u32()?,
        hold: r.u32()?,
        decay: r.u32()?,
        release: r.u32()?,
        sustain: r.u32()?,
    })
}

fn put_i3dl2_source(w: &mut Writer, s: &I3dl2Source) {
    let I3dl2Source {
        direct,
        direct_hf,
        room,
        room_hf,
        room_rolloff,
        obstruction_hf,
        obstruction_lf_ratio,
        occlusion_hf,
        occlusion_lf_ratio,
    } = s;
    for v in [direct, direct_hf, room, room_hf] {
        put_i32(w, *v);
    }
    put_f32(w, *room_rolloff);
    put_i32(w, *obstruction_hf);
    put_f32(w, *obstruction_lf_ratio);
    put_i32(w, *occlusion_hf);
    put_f32(w, *occlusion_lf_ratio);
}

fn get_i3dl2_source(r: &mut Reader<'_>) -> Result<I3dl2Source, Error> {
    Ok(I3dl2Source {
        direct: get_i32(r)?,
        direct_hf: get_i32(r)?,
        room: get_i32(r)?,
        room_hf: get_i32(r)?,
        room_rolloff: get_f32(r)?,
        obstruction_hf: get_i32(r)?,
        obstruction_lf_ratio: get_f32(r)?,
        occlusion_hf: get_i32(r)?,
        occlusion_lf_ratio: get_f32(r)?,
    })
}

/// Every effect setting of a voice.
pub fn put_voice_fx(w: &mut Writer, fx: &VoiceFx) {
    let VoiceFx {
        filter,
        lfo,
        env,
        headroom_mb,
        i3dl2,
    } = fx;
    let FilterSpec { stages } = filter;
    for s in stages {
        put_opt(w, s, put_stage);
    }
    for l in lfo {
        put_opt(w, l, put_lfo);
    }
    put_opt(w, env, put_env);
    put_i32(w, *headroom_mb);
    put_opt(w, i3dl2, put_i3dl2_source);
}

pub fn get_voice_fx(r: &mut Reader<'_>) -> Result<VoiceFx, Error> {
    let mut fx = VoiceFx::default();
    for s in &mut fx.filter.stages {
        *s = get_opt(r, get_stage)?;
    }
    for l in &mut fx.lfo {
        *l = get_opt(r, get_lfo)?;
    }
    fx.env = get_opt(r, get_env)?;
    fx.headroom_mb = get_i32(r)?;
    fx.i3dl2 = get_opt(r, get_i3dl2_source)?;
    Ok(fx)
}

/// A voice's host output.
pub fn put_output(w: &mut Writer, o: VoiceOutput) {
    w.u8(match o {
        VoiceOutput::Main => 0,
        VoiceOutput::Both => 1,
        VoiceOutput::Side => 2,
    });
}

pub fn get_output(r: &mut Reader<'_>) -> Result<VoiceOutput, Error> {
    match r.u8()? {
        0 => Ok(VoiceOutput::Main),
        1 => Ok(VoiceOutput::Both),
        2 => Ok(VoiceOutput::Side),
        o => Err(corrupt(format!("voice output {o}"))),
    }
}

fn put_source(w: &mut Writer, s: &Source3d) {
    let Source3d {
        position,
        velocity,
        min_distance,
        max_distance,
        rolloff_factor,
        doppler_factor,
        cone_inside,
        cone_outside,
        cone_orientation,
        cone_outside_volume_mb,
        mode,
    } = s;
    put_vec3(w, position);
    put_vec3(w, velocity);
    for v in [min_distance, max_distance, rolloff_factor, doppler_factor] {
        put_f32(w, *v);
    }
    w.u32(*cone_inside).u32(*cone_outside);
    put_vec3(w, cone_orientation);
    put_i32(w, *cone_outside_volume_mb);
    w.u8(match mode {
        Mode3d::Normal => 0,
        Mode3d::HeadRelative => 1,
        Mode3d::Disabled => 2,
    });
}

fn get_source(r: &mut Reader<'_>) -> Result<Source3d, Error> {
    Ok(Source3d {
        position: get_vec3(r)?,
        velocity: get_vec3(r)?,
        min_distance: get_f32(r)?,
        max_distance: get_f32(r)?,
        rolloff_factor: get_f32(r)?,
        doppler_factor: get_f32(r)?,
        cone_inside: r.u32()?,
        cone_outside: r.u32()?,
        cone_orientation: get_vec3(r)?,
        cone_outside_volume_mb: get_i32(r)?,
        mode: match r.u8()? {
            0 => Mode3d::Normal,
            1 => Mode3d::HeadRelative,
            2 => Mode3d::Disabled,
            m => return Err(corrupt(format!("3D mode {m}"))),
        },
    })
}

fn put_bins(w: &mut Writer, bins: &[(u32, i32)]) {
    w.list(bins, |w, &(bin, mb)| {
        w.u32(bin).u32(mb as u32);
    });
}

fn get_bins(r: &mut Reader<'_>) -> Result<Vec<(u32, i32)>, Error> {
    r.list(8, |r| Ok((r.u32()?, get_i32(r)?)))
}

fn put_params(w: &mut Writer, p: &VoiceParams) {
    let VoiceParams {
        frequency,
        volume_mb,
        pan_mb,
        spatial,
        mix_bins,
        fx,
        output,
    } = p;
    w.u32(*frequency);
    put_i32(w, *volume_mb);
    put_i32(w, *pan_mb);
    put_opt(w, spatial, put_source);
    put_opt(w, mix_bins, |w, bins| put_bins(w, bins));
    put_voice_fx(w, fx);
    put_output(w, *output);
}

fn get_params(r: &mut Reader<'_>) -> Result<VoiceParams, Error> {
    Ok(VoiceParams {
        frequency: r.u32()?,
        volume_mb: get_i32(r)?,
        pan_mb: get_i32(r)?,
        spatial: get_opt(r, get_source)?,
        mix_bins: get_opt(r, get_bins)?,
        fx: get_voice_fx(r)?,
        output: get_output(r)?,
    })
}

fn put_listener(w: &mut Writer, l: &Listener) {
    let Listener {
        position,
        front,
        top,
        velocity,
        distance_factor,
        rolloff_factor,
        doppler_factor,
    } = l;
    for v in [position, front, top, velocity] {
        put_vec3(w, v);
    }
    for v in [distance_factor, rolloff_factor, doppler_factor] {
        put_f32(w, *v);
    }
}

fn get_listener(r: &mut Reader<'_>) -> Result<Listener, Error> {
    Ok(Listener {
        position: get_vec3(r)?,
        front: get_vec3(r)?,
        top: get_vec3(r)?,
        velocity: get_vec3(r)?,
        distance_factor: get_f32(r)?,
        rolloff_factor: get_f32(r)?,
        doppler_factor: get_f32(r)?,
    })
}

// ---- voice state -----------------------------------------------------------

fn put_fx_state(w: &mut Writer, f: &FxState) {
    let FxState {
        clock,
        env_start,
        doppler,
        bq,
        // Rebuilt: recomputed from the same filter, cutoff step and rate.
        coeffs: _,
        cached: _,
        shelf_z,
        room_z,
        phase_off,
    } = f;
    w.u64(*clock).u64(*env_start);
    put_f64(w, *doppler);
    for s in bq {
        let BiquadState { z1, z2 } = s;
        for v in z1.iter().chain(z2) {
            put_f64(w, *v);
        }
    }
    for v in shelf_z.iter().chain(room_z) {
        put_f64(w, *v);
    }
    put_f64(w, *phase_off);
}

fn get_fx_state(r: &mut Reader<'_>, version: u32) -> Result<FxState, Error> {
    let mut f = FxState {
        clock: r.u64()?,
        env_start: r.u64()?,
        doppler: get_f64(r)?,
        ..FxState::default()
    };
    for s in &mut f.bq {
        for v in s.z1.iter_mut().chain(s.z2.iter_mut()) {
            *v = get_f64(r)?;
        }
    }
    for v in &mut f.shelf_z {
        *v = get_f64(r)?;
    }
    if version >= 3 {
        for v in &mut f.room_z {
            *v = get_f64(r)?;
        }
    }
    f.phase_off = get_f64(r)?;
    Ok(f)
}

fn put_buffer(w: &mut Writer, b: &SoundBuffer) {
    let SoundBuffer {
        format,
        data,
        // Rebuilt from `data`.
        size_bytes: _,
        decoded: _,
        frames: _,
        params,
        playing,
        looping,
        loop_region,
        pos,
        fx_state,
    } = b;
    put_format(w, format);
    w.blob(data);
    put_params(w, params);
    w.bool(*playing).bool(*looping);
    put_opt(w, loop_region, |w, &(start, end)| {
        w.u64(start as u64).u64(end as u64);
    });
    put_f64(w, *pos);
    put_fx_state(w, fx_state);
}

fn get_buffer(r: &mut Reader<'_>, version: u32) -> Result<SoundBuffer, Error> {
    let format = get_format(r)?;
    let data = r.blob()?.to_vec();
    let mut b =
        SoundBuffer::from_data(format, data).map_err(|e| corrupt(format!("buffer data: {e}")))?;
    b.params = get_params(r)?;
    b.playing = r.bool()?;
    b.looping = r.bool()?;
    b.loop_region = get_opt(r, |r| Ok((get_usize(r)?, get_usize(r)?)))?;
    let frames = b.frames;
    if b.loop_region
        .is_some_and(|(start, end)| start >= end || end > frames)
    {
        return Err(corrupt("loop region outside its buffer"));
    }
    b.pos = get_f64(r)?;
    b.fx_state = get_fx_state(r, version)?;
    Ok(b)
}

fn put_status(w: &mut Writer, s: PacketStatus) {
    w.u8(match s {
        PacketStatus::Success => 0,
        PacketStatus::Flushed => 1,
    });
}

fn get_status(r: &mut Reader<'_>) -> Result<PacketStatus, Error> {
    match r.u8()? {
        0 => Ok(PacketStatus::Success),
        1 => Ok(PacketStatus::Flushed),
        s => Err(corrupt(format!("packet status {s}"))),
    }
}

fn put_stream(w: &mut Writer, s: &Stream) {
    let Stream {
        format,
        params,
        queue,
        history,
        pos,
        paused,
        next_id,
        completed,
        fx_state,
        dry_at,
    } = s;
    put_format(w, format);
    put_params(w, params);
    w.u32(queue.len() as u32);
    for p in queue {
        let Packet { id, frames, .. } = p;
        w.u64(id.0);
        put_f32s(w, frames);
    }
    w.u32(history.len() as u32);
    for [left, right] in history {
        put_f32(w, *left);
        put_f32(w, *right);
    }
    put_f64(w, *pos);
    w.bool(*paused).u64(*next_id);
    w.u32(completed.len() as u32);
    for (id, status) in completed {
        w.u64(id.0);
        put_status(w, *status);
    }
    put_fx_state(w, fx_state);
    put_opt(w, dry_at, |w, at| {
        w.u64(*at);
    });
}

fn get_stream(r: &mut Reader<'_>, version: u32) -> Result<Stream, Error> {
    let format = get_format(r)?;
    let mut s = Stream::new(format).map_err(|e| corrupt(format!("stream: {e}")))?;
    s.params = get_params(r)?;
    s.queue = r
        .list(12, |r| {
            let id = PacketId(r.u64()?);
            let frames = get_f32s(r)?;
            let frame_count = frames.len() / format.channels() as usize;
            Ok(Packet { id, frames, frame_count })
        })?
        .into();
    let history = r.list(8, |r| Ok([get_f32(r)?, get_f32(r)?]))?;
    if history.len() > HISTORY_FRAMES {
        return Err(corrupt("stream history too long"));
    }
    // Into the history `Stream::new` sized, so rendering never allocates.
    s.history.extend(history);
    s.pos = get_f64(r)?;
    s.paused = r.bool()?;
    s.next_id = r.u64()?;
    s.completed = r.list(9, |r| Ok((PacketId(r.u64()?), get_status(r)?)))?;
    s.fx_state = get_fx_state(r, version)?;
    s.dry_at = get_opt(r, |r| r.u64())?;
    Ok(s)
}

fn put_mix_in(w: &mut Writer, m: &MixIn) {
    let MixIn {
        input_bin,
        params,
        playing,
        fx_state,
        hrtf,
    } = m;
    w.u32(*input_bin);
    put_params(w, params);
    w.bool(*playing);
    put_fx_state(w, fx_state);
    if let Some(hrtf) = hrtf {
        put_hrtf(w, hrtf);
    }
}

fn get_mix_in(
    r: &mut Reader<'_>,
    version: u32,
    hrtf: bool,
    rate: u32,
) -> Result<MixIn, Error> {
    let mut m = MixIn::new(r.u32()?);
    m.params = get_params(r)?;
    m.playing = r.bool()?;
    m.fx_state = get_fx_state(r, version)?;
    if hrtf {
        m.hrtf = Some(get_hrtf(r, rate)?);
    }
    Ok(m)
}

fn put_hrtf_parameters(w: &mut Writer, p: &Parameters) {
    let Parameters {
        delay,
        shadow,
        rear,
        front_weight,
        pinna_delay,
        pinna_mix,
    } = *p;
    for v in delay
        .into_iter()
        .chain(shadow)
        .chain([rear, front_weight, pinna_delay, pinna_mix])
    {
        put_f32(w, v);
    }
}

fn get_hrtf_parameters(r: &mut Reader<'_>, max_delay: f32) -> Result<Parameters, Error> {
    let p = Parameters {
        delay: [get_f32(r)?, get_f32(r)?],
        shadow: [get_f32(r)?, get_f32(r)?],
        rear: get_f32(r)?,
        front_weight: get_f32(r)?,
        pinna_delay: get_f32(r)?,
        pinna_mix: get_f32(r)?,
    };
    if !(0.0..=max_delay).contains(&p.pinna_delay)
        || p.delay.iter().any(|&d| {
            d < 0.0 || !(0.0..=max_delay).contains(&(d + p.pinna_delay))
        })
        || p.shadow.iter().any(|&v| !(-1.0..=2.0).contains(&v))
        || !(0.0..=0.35).contains(&p.rear)
        || !(0.0..=1.0).contains(&p.front_weight)
        || !(0.0..=0.3).contains(&p.pinna_mix)
    {
        return Err(corrupt("invalid binaural filter parameters"));
    }
    Ok(p)
}

fn put_hrtf(w: &mut Writer, h: &Hrtf) {
    let Hrtf {
        history,
        write,
        rear_low,
        ear_low,
        current,
        target,
        remaining,
        rate: _,
        shadow_coefficient: _,
        rear_coefficient: _,
        ramp_frames: _,
    } = h;
    put_f32s(w, history);
    w.u32(*write as u32);
    put_f32(w, *rear_low);
    put_f32(w, ear_low[0]);
    put_f32(w, ear_low[1]);
    put_opt(w, current, put_hrtf_parameters);
    put_opt(w, target, put_hrtf_parameters);
    w.u32(*remaining);
}

fn get_hrtf(r: &mut Reader<'_>, rate: u32) -> Result<Hrtf, Error> {
    let history = get_f32s(r)?;
    if history.len() != Hrtf::history_len(rate) || !history.iter().all(|s| s.is_finite()) {
        return Err(corrupt("invalid binaural filter history"));
    }
    let max_delay = (history.len() - 2) as f32;
    let mut h = Hrtf::with_history(rate, history);
    h.write = r.u32()? as usize;
    h.rear_low = get_f32(r)?;
    h.ear_low = [get_f32(r)?, get_f32(r)?];
    h.current = get_opt(r, |r| get_hrtf_parameters(r, max_delay))?;
    h.target = get_opt(r, |r| get_hrtf_parameters(r, max_delay))?;
    h.remaining = r.u32()?;
    if h.write >= h.history.len()
        || !h.rear_low.is_finite()
        || !h.ear_low.iter().all(|s| s.is_finite())
        || h.current.is_some() != h.target.is_some()
        || (h.current.is_none() && h.remaining != 0)
        || h.remaining > h.ramp_frames
    {
        return Err(corrupt("invalid binaural filter state"));
    }
    Ok(h)
}

// ---- reverb ----------------------------------------------------------------

fn put_i3dl2_listener(w: &mut Writer, l: &I3dl2Listener) {
    let I3dl2Listener {
        room,
        room_hf,
        room_rolloff,
        decay_time,
        decay_hf_ratio,
        reflections,
        reflections_delay,
        reverb,
        reverb_delay,
        diffusion,
        density,
        hf_reference,
    } = l;
    put_i32(w, *room);
    put_i32(w, *room_hf);
    for v in [room_rolloff, decay_time, decay_hf_ratio] {
        put_f32(w, *v);
    }
    put_i32(w, *reflections);
    put_f32(w, *reflections_delay);
    put_i32(w, *reverb);
    for v in [reverb_delay, diffusion, density, hf_reference] {
        put_f32(w, *v);
    }
}

fn get_i3dl2_listener(r: &mut Reader<'_>) -> Result<I3dl2Listener, Error> {
    Ok(I3dl2Listener {
        room: get_i32(r)?,
        room_hf: get_i32(r)?,
        room_rolloff: get_f32(r)?,
        decay_time: get_f32(r)?,
        decay_hf_ratio: get_f32(r)?,
        reflections: get_i32(r)?,
        reflections_delay: get_f32(r)?,
        reverb: get_i32(r)?,
        reverb_delay: get_f32(r)?,
        diffusion: get_f32(r)?,
        density: get_f32(r)?,
        hf_reference: get_f32(r)?,
    })
}

fn put_delay(w: &mut Writer, d: &Delay) {
    let Delay { buf, pos } = d;
    put_f32s(w, buf);
    w.u64(*pos as u64);
}

fn get_delay(r: &mut Reader<'_>) -> Result<Delay, Error> {
    let buf = get_f32s(r)?;
    let pos = get_usize(r)?;
    if pos >= buf.len() {
        return Err(corrupt("delay line position outside the line"));
    }
    Ok(Delay { buf, pos })
}

fn put_allpass(w: &mut Writer, a: &Allpass) {
    let Allpass { line, g } = a;
    put_delay(w, line);
    put_f32(w, *g);
}

fn get_allpass(r: &mut Reader<'_>) -> Result<Allpass, Error> {
    Ok(Allpass {
        line: get_delay(r)?,
        g: get_f32(r)?,
    })
}

/// The whole reverberator: the delay lines are the tail, and the derived
/// gains, poles and tap positions are saved rather than recomputed, so the
/// load does not depend on `configure`'s history.
fn put_reverb(w: &mut Writer, rv: &Reverb) {
    let Reverb {
        rate,
        params,
        er_line,
        er_taps,
        pre,
        pre_len,
        allpass,
        fdn,
        line_gain,
        line_pole,
        line_z,
        room_pole,
        room_z,
        er_level,
        late_level,
        sized_for,
    } = rv;
    w.u32(*rate);
    put_i3dl2_listener(w, params);
    put_delay(w, er_line);
    for t in er_taps {
        w.u64(*t as u64);
    }
    put_delay(w, pre);
    w.u64(*pre_len as u64);
    for a in allpass {
        put_allpass(w, a);
    }
    for l in fdn {
        put_delay(w, l);
    }
    for v in line_gain.iter().chain(line_pole).chain(line_z) {
        put_f32(w, *v);
    }
    put_f32(w, *room_pole);
    for v in room_z {
        put_f32(w, *v);
    }
    put_f32(w, *er_level);
    put_f32(w, *late_level);
    let (key_rate, reflections, reverb, density) = sized_for;
    w.u32(*key_rate);
    for v in [reflections, reverb, density] {
        put_i32(w, *v);
    }
}

fn get_reverb(r: &mut Reader<'_>) -> Result<Reverb, Error> {
    let rate = r.u32()?;
    let params = get_i3dl2_listener(r)?;
    let er_line = get_delay(r)?;
    let mut er_taps = [0usize; 7];
    for t in &mut er_taps {
        *t = get_usize(r)?;
    }
    let pre = get_delay(r)?;
    let pre_len = get_usize(r)?;
    let allpass = [get_allpass(r)?, get_allpass(r)?];
    let fdn = [get_delay(r)?, get_delay(r)?, get_delay(r)?, get_delay(r)?];
    let mut lines = [[0f32; 4]; 3];
    for v in lines.iter_mut().flatten() {
        *v = get_f32(r)?;
    }
    let [line_gain, line_pole, line_z] = lines;
    let room_pole = get_f32(r)?;
    let room_z = [get_f32(r)?, get_f32(r)?];
    let er_level = get_f32(r)?;
    let late_level = get_f32(r)?;
    let sized_for = (r.u32()?, get_i32(r)?, get_i32(r)?, get_i32(r)?);
    Ok(Reverb {
        rate,
        params,
        er_line,
        er_taps,
        pre,
        pre_len,
        allpass,
        fdn,
        line_gain,
        line_pole,
        line_z,
        room_pole,
        room_z,
        er_level,
        late_level,
        sized_for,
    })
}

// ---- the mixer -------------------------------------------------------------

/// The `audio` section, decoded and checked against a mixer
/// ([`Mixer::check_snapshot`]) and not yet applied.
pub struct LoadedMixer {
    sample_rate: u32,
    master_mb: i32,
    listener: Listener,
    buffers: BTreeMap<BufferId, SoundBuffer>,
    streams: BTreeMap<StreamId, Stream>,
    mix_ins: BTreeMap<MixInId, MixIn>,
    next_id: u32,
    reverb: Option<Reverb>,
    clock: u64,
    taps: Vec<Tap>,
}

impl Mixer {
    /// Adds the `audio` section: everything a render reads, so a mixer
    /// that loads it renders the same PCM as this one from here on. Call it
    /// while nothing renders (the output gated or drained).
    pub fn save_snapshot(&self, out: &mut SnapshotWriter) -> Result<(), SnapshotError> {
        let mut w = Writer::new();
        self.put(&mut w);
        out.section(SECTION, VERSION, SectionCodec::Zstd, w.into_bytes())?;
        Ok(())
    }

    fn put(&self, w: &mut Writer) {
        let Mixer {
            // Checked: the loading mixer runs at the host's rate.
            sample_rate,
            master_mb,
            hrtf,
            listener,
            buffers,
            streams,
            mix_ins,
            next_id,
            // Render scratch.
            scratch: _,
            reverb,
            reverb_in: _,
            direct: _,
            room: _,
            distance_reverb,
            clock,
            // Diagnostics.
            health: _,
            packet_clock: _,
            taps,
        } = self;
        w.bool(*hrtf).bool(*distance_reverb);
        w.u32(*sample_rate);
        put_i32(w, *master_mb);
        put_listener(w, listener);
        w.u32(buffers.len() as u32);
        for (id, b) in buffers {
            w.u32(id.0);
            put_buffer(w, b);
        }
        w.u32(streams.len() as u32);
        for (id, s) in streams {
            w.u32(id.0);
            put_stream(w, s);
        }
        w.u32(mix_ins.len() as u32);
        for (id, m) in mix_ins {
            w.u32(id.0);
            put_mix_in(w, m);
        }
        w.u32(*next_id);
        put_opt(w, reverb, put_reverb);
        w.u64(*clock);
        w.u32(taps.len() as u32);
        for t in taps {
            let Tap { bin, samples } = t;
            w.u32(*bin);
            put_f32s(w, samples);
        }
    }

    fn get(r: &mut Reader<'_>, version: u32, hrtf: bool) -> Result<LoadedMixer, Error> {
        let sample_rate = r.u32()?;
        let master_mb = get_i32(r)?;
        let listener = get_listener(r)?;
        let buffers = r.list(5, |r| Ok((BufferId(r.u32()?), get_buffer(r, version)?)))?;
        let streams = r.list(5, |r| Ok((StreamId(r.u32()?), get_stream(r, version)?)))?;
        let mix_ins = r.list(5, |r| {
            Ok((MixInId(r.u32()?), get_mix_in(r, version, hrtf, sample_rate)?))
        })?;
        let next_id = r.u32()?;
        let reverb = get_opt(r, get_reverb)?;
        let clock = r.u64()?;
        let taps = r.list(8, |r| {
            Ok(Tap {
                bin: r.u32()?,
                samples: get_f32s(r)?,
            })
        })?;
        let saved = LoadedMixer {
            sample_rate,
            master_mb,
            listener,
            buffers: buffers.into_iter().collect(),
            streams: streams.into_iter().collect(),
            mix_ins: mix_ins.into_iter().collect(),
            next_id,
            reverb,
            clock,
            taps,
        };
        let ids = saved.buffers.keys().map(|i| i.0);
        let ids = ids.chain(saved.streams.keys().map(|i| i.0));
        if ids
            .chain(saved.mix_ins.keys().map(|i| i.0))
            .any(|i| i >= saved.next_id)
        {
            return Err(corrupt("a voice id at or past the next id"));
        }
        Ok(saved)
    }

    /// Replaces every voice, the reverb and the clock with the `audio`
    /// section's. Decoded and checked first: on any error the mixer is
    /// unchanged.
    pub fn load_snapshot(&mut self, snapshot: &Snapshot<'_>) -> Result<(), SnapshotError> {
        let s = self.check_snapshot(snapshot)?;
        self.apply_snapshot(s);
        Ok(())
    }

    /// The first half of [`Mixer::load_snapshot`], for a caller that must
    /// check other sections too before it changes anything: decodes the
    /// section and checks it fits this mixer. Changes nothing.
    pub fn check_snapshot(&self, snapshot: &Snapshot<'_>) -> Result<LoadedMixer, SnapshotError> {
        let (version, bytes) = snapshot
            .section(SECTION)?
            .ok_or(Error::MissingSection(SECTION))?;
        if !(1..=VERSION).contains(&version) {
            return Err(Error::SectionVersion(SECTION, version).into());
        }
        let mut r = Reader::new(&bytes, "audio");
        let (hrtf, distance_reverb) = match version {
            1 => (false, false),
            2 => (true, false),
            _ => (r.bool()?, r.bool()?),
        };
        if hrtf != self.hrtf || distance_reverb != self.distance_reverb {
            return Err(SnapshotError::Mismatch("different audio feature settings".into()));
        }
        let s = Self::get(&mut r, version, hrtf)?;
        r.finish()?;
        if s.sample_rate != self.sample_rate {
            return Err(SnapshotError::Mismatch(format!(
                "saved at {} Hz, this mixer runs at {} Hz",
                s.sample_rate, self.sample_rate
            )));
        }
        if s.reverb.as_ref().is_some_and(|rv| rv.rate != s.sample_rate) {
            return Err(corrupt("reverb at another rate than its mixer").into());
        }
        Ok(s)
    }

    /// The second half of [`Mixer::load_snapshot`]: `s` must come from
    /// this mixer's [`Mixer::check_snapshot`].
    pub fn apply_snapshot(&mut self, s: LoadedMixer) {
        let Mixer {
            sample_rate: _,
            master_mb,
            hrtf: _,
            listener,
            buffers,
            streams,
            mix_ins,
            next_id,
            scratch: _,
            reverb,
            reverb_in: _,
            direct: _,
            room: _,
            // Host configuration, checked before loading.
            distance_reverb: _,
            clock,
            health: _,
            packet_clock: _,
            taps,
        } = self;
        *master_mb = s.master_mb;
        *listener = s.listener;
        *buffers = s.buffers;
        *streams = s.streams;
        *mix_ins = s.mix_ins;
        *next_id = s.next_id;
        *reverb = s.reverb;
        *clock = s.clock;
        *taps = s.taps;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_mix_in_reads_binaural_history_immediately_after_the_pitch_offset() {
        for version in [1, 2] {
            let mut w = Writer::new();
            let mix = MixIn::new(17);
            w.u32(mix.input_bin());
            put_params(&mut w, mix.params());
            w.bool(true);
            // Both legacy versions end FxState with eight biquad delays,
            // two direct filter delays and the stream pitch offset.
            w.u64(19).u64(0);
            put_f64(&mut w, 1.0);
            for _ in 0..10 {
                put_f64(&mut w, 0.0);
            }
            put_f64(&mut w, 2.5);
            if version == 2 {
                let mut h = Hrtf::new(48_000);
                h.history[0] = 0.75;
                put_hrtf(&mut w, &h);
            }
            let bytes = w.into_bytes();
            let mut r = Reader::new(&bytes, "audio");
            let loaded = get_mix_in(&mut r, version, version == 2, 48_000).unwrap();
            r.finish().unwrap();
            assert_eq!(loaded.fx_state.phase_off, 2.5);
            assert_eq!(loaded.fx_state.room_z, [0.0; 2]);
            assert_eq!(loaded.hrtf.map(|h| h.history[0]), (version == 2).then_some(0.75));
        }
    }
}
