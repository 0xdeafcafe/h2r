//! Software mixer: sums active buffers, streams and mix-ins into a host buffer.
//!
//! The mixer is plain data and a [`render`](Mixer::render) call. It knows
//! nothing about devices or threads: the platform layer calls `render` from
//! its audio callback (usually behind a lock, see [`crate::output`]).
//!
//! A voice with mix bins ([`VoiceParams::mix_bins`]) is routed through them
//! (see [`crate::route`]): straight to the stereo output, through the 3D
//! panner, or into the [`MixIn`] submixes that read its FX sends. Mix-ins are
//! mixed after every buffer and stream, in dependency order, so a chain of
//! submixes lands in the same render. A send into a mix-in that is already
//! mixed (only possible in a routing cycle) is dropped.
//!
//! Each voice's effects ([`crate::fx`]: filter, LFOs, envelope) run on its
//! own rendered frames before routing, voice headroom scales its sends, and
//! the I3DL2 send bin feeds one [`Reverb`] whose output joins the stereo
//! mix after every voice and mix-in. Effects are functions of voice-local
//! frame counts and mixer state only, so a render's output does not depend on
//! how the frames are split across calls.

use std::collections::BTreeMap;

use crate::dsound::{
    BufferId, Listener, MixIn, SoundBuffer, Spatial, Stream, StreamId, VoiceOutput, VoiceParams,
    mb_to_gain,
};
use crate::fx::headroom_gain;
use crate::reverb::{I3dl2Listener, Reverb};
use crate::route::{self, Dest};

/// Identifies a mix-in inside a [`Mixer`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MixInId(pub(crate) u32);

/// Mixes voices to interleaved stereo `f32` at a fixed host rate.
#[derive(Debug)]
pub struct Mixer {
    pub(crate) sample_rate: u32,
    pub(crate) master_mb: i32,
    pub(crate) listener: Listener,
    pub(crate) buffers: BTreeMap<BufferId, SoundBuffer>,
    pub(crate) streams: BTreeMap<StreamId, Stream>,
    pub(crate) mix_ins: BTreeMap<MixInId, MixIn>,
    pub(crate) next_id: u32,
    /// One voice rendered at unity gain, reused across renders.
    pub(crate) scratch: Vec<f32>,
    /// The environment behind the I3DL2 send bin, once the game sets one.
    pub(crate) reverb: Option<Reverb>,
    /// The reverb's mono input and a voice's I3DL2 direct path, reused across
    /// renders (see [`Mixer::reserve_block`]).
    pub(crate) reverb_in: Vec<f32>,
    pub(crate) direct: Vec<f32>,
    /// Frames rendered since creation: the clock [`StreamHealth`] uses.
    pub(crate) clock: u64,
    pub(crate) health: StreamHealth,
    /// Bins whose sends are captured rather than mixed ([`Mixer::set_taps`]).
    pub(crate) taps: Vec<Tap>,
}

/// A captured bin: the mono sum of every send to it, one sample per frame
/// rendered, waiting for [`Mixer::take_tap`].
#[derive(Debug)]
pub(crate) struct Tap {
    pub(crate) bin: u32,
    pub(crate) samples: Vec<f32>,
}

/// How streams fared, for diagnostics ([`Mixer::stream_health`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StreamHealth {
    /// A stream ran out of packets mid-play and got more within
    /// [`STARVE_WINDOW_MS`]: the game fed it late, and the gap was heard.
    pub starves: u64,
    /// The frames of silence those gaps added.
    pub starve_frames: u64,
    /// Frames streams consumed (and so completed packets for) whose mix
    /// bins reached nothing: an empty bin list, or sends only to mix-ins
    /// that are not playing. The game thinks those played.
    pub unrouted_frames: u64,
}

/// A stream that runs dry and is fed again within this long counts as a
/// [`StreamHealth::starves`]; a longer wait is a new sound.
pub const STARVE_WINDOW_MS: u64 = 250;

impl Mixer {
    /// `sample_rate` is the host output rate in Hz.
    pub fn new(sample_rate: u32) -> Self {
        assert!(sample_rate > 0);
        Self {
            sample_rate,
            master_mb: 0,
            listener: Listener::default(),
            buffers: BTreeMap::new(),
            streams: BTreeMap::new(),
            mix_ins: BTreeMap::new(),
            next_id: 1,
            scratch: Vec::new(),
            reverb: None,
            reverb_in: Vec::new(),
            direct: Vec::new(),
            clock: 0,
            health: StreamHealth::default(),
            taps: Vec::new(),
        }
    }

    /// Captures the FX sends to `bins` instead of leaving them to mix-ins:
    /// the DSP effects that read those bins on an Xbox run on the host
    /// instead (DirectSound's XHV voice effects, which feed a headset, not
    /// the speakers). Each tap gets one mono sample per frame rendered, the
    /// sum of the voices sent to it at their volumes, headroom and bin
    /// volume but not the master volume, silence included, so it runs on
    /// the mixer's clock. Replaces the previous set; their samples go.
    pub fn set_taps(&mut self, bins: &[u32]) {
        self.taps = bins.iter().map(|&bin| Tap { bin, samples: Vec::new() }).collect();
    }

    /// The samples captured on `bin` since the last call, at
    /// [`Mixer::sample_rate`]; empty when `bin` is not tapped. A tap nobody
    /// takes keeps only the last second.
    pub fn take_tap(&mut self, bin: u32) -> Vec<f32> {
        self.taps.iter_mut().find(|t| t.bin == bin).map(|t| std::mem::take(&mut t.samples)).unwrap_or_default()
    }

    /// Stream starvation and routing counters since creation.
    pub fn stream_health(&self) -> StreamHealth {
        self.health
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Switches the host rate (after an output device change). Voices
    /// resample from their own format on every render, so playback carries on
    /// seamlessly at the new rate.
    pub fn set_sample_rate(&mut self, sample_rate: u32) {
        assert!(sample_rate > 0);
        self.sample_rate = sample_rate;
        if let Some(r) = &self.reverb {
            self.reverb = Some(Reverb::new(sample_rate, *r.params()));
        }
    }

    /// Sets the environment of the I3DL2 reverb send (`None` removes it and
    /// its tail). Levels and decay change in place; new delays clear the tail.
    pub fn set_reverb(&mut self, env: Option<I3dl2Listener>) {
        match (env, &mut self.reverb) {
            (Some(e), Some(r)) => r.configure(e),
            (Some(e), None) => self.reverb = Some(Reverb::new(self.sample_rate, e)),
            (None, _) => self.reverb = None,
        }
    }

    /// Reserves working memory for renders of up to `frames` frames, so a
    /// render on an audio callback does not allocate for the reverb or the
    /// I3DL2 direct path. Call it from the control side, with the largest
    /// block the output will ask for.
    pub fn reserve_block(&mut self, frames: usize) {
        self.scratch.reserve(frames * 2);
        self.direct.reserve(frames * 2);
        self.reverb_in.reserve(frames);
    }

    pub fn reverb(&self) -> Option<&I3dl2Listener> {
        self.reverb.as_ref().map(Reverb::params)
    }

    /// Master volume in millibels.
    pub fn set_master_volume(&mut self, mb: i32) {
        self.master_mb = mb.clamp(-10_000, 0);
    }

    pub fn listener(&self) -> &Listener {
        &self.listener
    }
    pub fn listener_mut(&mut self) -> &mut Listener {
        &mut self.listener
    }

    pub fn add_buffer(&mut self, buffer: SoundBuffer) -> BufferId {
        let id = BufferId(self.take_id());
        self.buffers.insert(id, buffer);
        id
    }

    pub fn add_stream(&mut self, stream: Stream) -> StreamId {
        let id = StreamId(self.take_id());
        self.streams.insert(id, stream);
        id
    }

    pub fn add_mix_in(&mut self, mix_in: MixIn) -> MixInId {
        let id = MixInId(self.take_id());
        self.mix_ins.insert(id, mix_in);
        id
    }

    fn take_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn buffer(&self, id: BufferId) -> Option<&SoundBuffer> {
        self.buffers.get(&id)
    }
    pub fn buffer_mut(&mut self, id: BufferId) -> Option<&mut SoundBuffer> {
        self.buffers.get_mut(&id)
    }
    pub fn stream_mut(&mut self, id: StreamId) -> Option<&mut Stream> {
        self.streams.get_mut(&id)
    }
    pub fn mix_in(&self, id: MixInId) -> Option<&MixIn> {
        self.mix_ins.get(&id)
    }
    pub fn mix_in_mut(&mut self, id: MixInId) -> Option<&mut MixIn> {
        self.mix_ins.get_mut(&id)
    }
    pub fn remove_buffer(&mut self, id: BufferId) -> Option<SoundBuffer> {
        self.buffers.remove(&id)
    }
    pub fn remove_stream(&mut self, id: StreamId) -> Option<Stream> {
        self.streams.remove(&id)
    }
    pub fn remove_mix_in(&mut self, id: MixInId) -> Option<MixIn> {
        self.mix_ins.remove(&id)
    }

    /// Number of buffers held by the mixer, playing or not.
    pub fn buffer_count(&self) -> usize {
        self.buffers.len()
    }

    /// Number of streams held by the mixer, with queued data or not.
    pub fn stream_count(&self) -> usize {
        self.streams.len()
    }

    /// Number of mix-ins held by the mixer, playing or not.
    pub fn mix_in_count(&self) -> usize {
        self.mix_ins.len()
    }

    /// Number of buffers currently playing plus streams with queued data.
    pub fn active_voices(&self) -> usize {
        self.buffers.values().filter(|b| b.is_playing()).count()
            + self.streams.values().filter(|s| s.pending() > 0).count()
    }

    /// Renders `out.len() / 2` stereo frames, overwriting `out`. The sum is
    /// not clipped; the output stage clamps. Every voice is heard here,
    /// whatever its [`VoiceOutput`].
    pub fn render(&mut self, out: &mut [f32]) {
        self.render_split(out, None);
    }

    /// [`Mixer::render`] with a second output, `side` (a controller speaker;
    /// interleaved stereo, the same frame count as `out`, overwritten).
    /// Voices marked [`VoiceOutput::Both`] or [`VoiceOutput::Side`] are
    /// added to `side` as mono, at their volume, headroom and the master
    /// volume, with no pan, 3D, mix bins or reverb: a controller speaker is a
    /// single small driver held in the hand. `Side` voices are left out of
    /// `out`. With `side` `None` every voice goes to `out`.
    pub fn render_split(&mut self, out: &mut [f32], mut side: Option<&mut [f32]>) {
        out.fill(0.0);
        if let Some(s) = side.as_deref_mut() {
            assert_eq!(s.len(), out.len(), "side output must match the main block");
            s.fill(0.0);
        }
        let frames = out.len() / 2;
        let rate = self.sample_rate;
        let master = mb_to_gain(self.master_mb);
        let mut scratch = std::mem::take(&mut self.scratch);
        scratch.resize(frames * 2, 0.0);
        // One mono accumulator per playing mix-in, in mixing order.
        let order = self.mix_in_order();
        let mut buses: Vec<Bus> = order
            .iter()
            .map(|&id| Bus { id, bin: self.mix_ins[&id].input_bin(), input: vec![0.0; frames], done: false })
            .collect();
        let reverb_live = self.reverb.as_ref().is_some_and(|r| !r.params().is_silent());
        let cap = rate as usize;
        let mut taps = std::mem::take(&mut self.taps);
        for t in &mut taps {
            let keep = cap.saturating_sub(frames.min(cap));
            if t.samples.len() > keep {
                t.samples.drain(..t.samples.len() - keep);
            }
            t.samples.resize(t.samples.len() + frames.min(cap), 0.0);
        }
        let mut rin = std::mem::take(&mut self.reverb_in);
        let mut direct = std::mem::take(&mut self.direct);
        direct.resize(frames * 2, 0.0);
        if reverb_live {
            rin.clear();
            rin.resize(frames, 0.0);
        }

        let ids: Vec<BufferId> = self.buffers.iter().filter(|(_, b)| b.is_playing()).map(|(&id, _)| id).collect();
        for id in ids {
            scratch.fill(0.0);
            let b = self.buffers.get_mut(&id).expect("listed above");
            let params = b.params().clone();
            b.set_doppler_pitch(params.spatial.as_ref().map_or(1.0, |s| self.listener.spatial(s).pitch));
            let c0 = b.fx_clock();
            b.render(&mut scratch, rate, (1.0, 1.0));
            let produced = (b.fx_clock() - c0) as usize;
            b.apply_fx(&mut scratch[..produced * 2], c0, rate);
            // Only the frames the voice produced: the shelf state advances with the
            // voice's own clock, so a block's padding never leaks a tail.
            let has_direct = b.direct_shelf(&scratch[..produced * 2], &mut direct[..produced * 2], rate);
            let channels = b.format().channels() as usize;
            if !to_side(side.as_deref_mut(), &params, &scratch, master) {
                let mut cx = Route { listener: &self.listener, master, buses: &mut buses, taps: &mut taps, frames, reverb_in: reverb_live.then_some(&mut rin) };
                route_voice(&mut cx, &params, channels, &scratch, has_direct.then_some(&direct[..produced * 2]), out);
            }
        }
        let ids: Vec<StreamId> = self.streams.iter().filter(|(_, s)| s.is_audible()).map(|(&id, _)| id).collect();
        let window = STARVE_WINDOW_MS * u64::from(rate) / 1000;
        for id in ids {
            scratch.fill(0.0);
            let s = self.streams.get_mut(&id).expect("listed above");
            if let Some(at) = s.dry_at.take() {
                let gap = self.clock.saturating_sub(at);
                if gap > 0 && gap <= window {
                    self.health.starves += 1;
                    self.health.starve_frames += gap;
                }
            }
            let params = s.params().clone();
            s.set_doppler_pitch(params.spatial.as_ref().map_or(1.0, |sp| self.listener.spatial(sp).pitch));
            let c0 = s.fx_clock();
            s.render(&mut scratch, rate, (1.0, 1.0));
            let produced = (s.fx_clock() - c0) as usize;
            s.apply_fx(&mut scratch[..produced * 2], c0, rate);
            // Only the frames the voice produced: the shelf state advances with the
            // voice's own clock, so a block's padding never leaks a tail.
            let has_direct = s.direct_shelf(&scratch[..produced * 2], &mut direct[..produced * 2], rate);
            let channels = s.format().channels() as usize;
            if s.pending() == 0 {
                // Ran out: dry from the first frame it did not fill.
                s.dry_at = Some(self.clock + produced as u64);
            }
            if !to_side(side.as_deref_mut(), &params, &scratch, master) {
                let mut cx = Route { listener: &self.listener, master, buses: &mut buses, taps: &mut taps, frames, reverb_in: reverb_live.then_some(&mut rin) };
                if !route_voice(&mut cx, &params, channels, &scratch, has_direct.then_some(&direct[..produced * 2]), out) {
                    self.health.unrouted_frames += produced as u64;
                }
            }
        }
        for i in 0..buses.len() {
            buses[i].done = true;
            let input = std::mem::take(&mut buses[i].input);
            for (frame, v) in scratch.as_chunks_mut::<2>().0.iter_mut().zip(&input) {
                *frame = [*v, *v];
            }
            let mix = self.mix_ins.get_mut(&buses[i].id).expect("listed above");
            let params = mix.params().clone();
            let c0 = mix.fx_clock();
            mix.apply_fx(&mut scratch, c0, rate);
            mix.advance_clock(frames as u64);
            let has_direct = mix.direct_shelf(&scratch, &mut direct, rate);
            let mut cx = Route { listener: &self.listener, master, buses: &mut buses, taps: &mut taps, frames, reverb_in: reverb_live.then_some(&mut rin) };
            route_voice(&mut cx, &params, 1, &scratch, has_direct.then_some(&direct[..]), out);
        }
        if let (Some(r), true) = (self.reverb.as_mut(), reverb_live) {
            scratch.fill(0.0);
            r.process(&rin, &mut scratch);
            for (o, w) in out.iter_mut().zip(&scratch) {
                *o += w * master;
            }
        }
        self.taps = taps;
        self.scratch = scratch;
        self.reverb_in = rin;
        self.direct = direct;
        self.clock += frames as u64;
    }

    /// Playing mix-ins, each after every mix-in that sends to it (Kahn's
    /// algorithm on the send graph; members of a cycle follow in id order).
    fn mix_in_order(&self) -> Vec<MixInId> {
        let playing: Vec<(MixInId, &MixIn)> = self.mix_ins.iter().filter(|(_, m)| m.is_playing()).map(|(&id, m)| (id, m)).collect();
        let sends = |m: &MixIn, target: &MixIn| {
            m.params().mix_bins.as_ref().is_some_and(|bins| bins.iter().any(|&(bin, _)| bin == target.input_bin()))
        };
        let mut order = Vec::with_capacity(playing.len());
        let mut left: Vec<usize> = (0..playing.len()).collect();
        while !left.is_empty() {
            let ready = left
                .iter()
                .position(|&t| !left.iter().any(|&s| s != t && sends(playing[s].1, playing[t].1)))
                .unwrap_or(0);
            order.push(playing[left.remove(ready)].0);
        }
        order
    }
}

/// A playing mix-in's input while the render is under way.
struct Bus {
    id: MixInId,
    bin: u32,
    input: Vec<f32>,
    /// Already mixed: later sends to it are dropped.
    done: bool,
}

/// What routing a voice needs from the mixer.
struct Route<'a> {
    listener: &'a Listener,
    master: f32,
    buses: &'a mut [Bus],
    /// The captured bins; this render's frames are each one's last `frames`
    /// samples.
    taps: &'a mut [Tap],
    frames: usize,
    /// The reverb's mono input, when an audible environment is set.
    reverb_in: Option<&'a mut Vec<f32>>,
}

/// Mixes one voice, rendered at unity gain into `src` (interleaved stereo; a
/// mono voice is in both channels), into `out`, the mix-in buses and the
/// reverb send. False when its routing reaches nothing at all (no bins, or
/// only sends to buses that are not there), whatever the volumes.
fn route_voice(
    cx: &mut Route<'_>,
    p: &VoiceParams,
    channels: usize,
    src: &[f32],
    direct_src: Option<&[f32]>,
    out: &mut [f32],
) -> bool {
    // The direct path (speakers and mix-in sends) may be shelved by an I3DL2
    // source's obstruction; the reverb send always takes `src`.
    let dsrc = direct_src.unwrap_or(src);
    let (listener, master) = (cx.listener, cx.master);
    let spatial = p.spatial.as_ref().map(|s| listener.spatial(s));
    // I3DL2 source shaping: the direct level applies to everything but the
    // reverb send, the room level to the reverb send alone.
    let (direct, room) = p.fx.i3dl2.map_or((1.0, 1.0), |i| (i.direct_gain(), i.room_gain()));
    let hr = headroom_gain(p.fx.headroom_mb);
    let Some(bins) = &p.mix_bins else {
        // A plain DirectSound voice: volume and pan, then the 3D panning.
        let s = spatial.unwrap_or(Spatial::DRY);
        let (l, r) = p.gains();
        add(out, dsrc, 0, 0, l * s.gain * s.left * master * direct * hr);
        add(out, dsrc, 1, 1, r * s.gain * s.right * master * direct * hr);
        return true;
    };
    let volume = mb_to_gain(p.volume_mb);
    let mut reached = false;
    for (i, &(bin, mb)) in bins.iter().enumerate() {
        let dest = route::dest(bin);
        reached |= match dest {
            Dest::Out { .. } | Dest::Crosstalk { .. } => true,
            Dest::Send(bin) => cx.buses.iter().any(|b| b.bin == bin && !b.done) || cx.taps.iter().any(|t| t.bin == bin),
            Dest::Reverb => cx.reverb_in.is_some(),
            Dest::Dropped => false,
        };
        let g = volume * mb_to_gain(mb) * hr;
        if g == 0.0 {
            continue;
        }
        let ch = route::channel_of(i, channels);
        match dest {
            Dest::Out { left, right } => {
                add(out, dsrc, ch, 0, g * left * master * direct);
                add(out, dsrc, ch, 1, g * right * master * direct);
            }
            Dest::Crosstalk { left, front } => {
                let w = match spatial {
                    Some(s) => s.gain * if left { s.left } else { s.right } * if front { s.front } else { 1.0 - s.front },
                    None => 1.0,
                };
                add(out, dsrc, ch, if left { 0 } else { 1 }, g * w * master * direct);
            }
            Dest::Send(bin) => {
                for tap in cx.taps.iter_mut().filter(|t| t.bin == bin) {
                    // A render longer than the cap keeps its last second.
                    let n = tap.samples.len().min(cx.frames);
                    let skip = cx.frames - n;
                    let start = tap.samples.len() - n;
                    for (acc, frame) in tap.samples[start..].iter_mut().zip(dsrc.as_chunks::<2>().0.iter().skip(skip)) {
                        *acc += frame[ch] * g * direct;
                    }
                }
                for bus in cx.buses.iter_mut().filter(|b| b.bin == bin && !b.done) {
                    for (acc, frame) in bus.input.iter_mut().zip(dsrc.as_chunks::<2>().0) {
                        *acc += frame[ch] * g * direct;
                    }
                }
            }
            Dest::Reverb => {
                if let Some(input) = cx.reverb_in.as_deref_mut() {
                    for (acc, frame) in input.iter_mut().zip(src.as_chunks::<2>().0) {
                        *acc += frame[ch] * g * room;
                    }
                }
            }
            Dest::Dropped => {}
        }
    }
    reached
}

/// Adds a voice rendered at unity gain (`src`, interleaved stereo) to the
/// side output as mono, if it has one and the voice is routed there. True
/// when the voice is heard only there, so the main mix must skip it.
fn to_side(side: Option<&mut [f32]>, p: &VoiceParams, src: &[f32], master: f32) -> bool {
    let Some(side) = side else {
        return false;
    };
    if p.output == VoiceOutput::Main {
        return false;
    }
    let g = mb_to_gain(p.volume_mb) * headroom_gain(p.fx.headroom_mb) * master * 0.5;
    if g != 0.0 {
        for (o, s) in side.as_chunks_mut::<2>().0.iter_mut().zip(src.as_chunks::<2>().0) {
            let m = (s[0] + s[1]) * g;
            o[0] += m;
            o[1] += m;
        }
    }
    p.output == VoiceOutput::Side
}

/// `out[2i + to] += src[2i + from] * g`.
fn add(out: &mut [f32], src: &[f32], from: usize, to: usize, g: f32) {
    if g == 0.0 {
        return;
    }
    for (o, s) in out.as_chunks_mut::<2>().0.iter_mut().zip(src.as_chunks::<2>().0) {
        o[to] += s[from] * g;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsound::{MixIn, Source3d, Vec3};
    use crate::route;
    use crate::format::WaveFormat;

    fn buf(samples: &[i16], rate: u32) -> SoundBuffer {
        let bytes = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        SoundBuffer::from_data(WaveFormat::pcm16(1, rate).unwrap(), bytes).unwrap()
    }

    #[test]
    fn silent_when_nothing_plays() {
        let mut m = Mixer::new(8000);
        let id = m.add_buffer(buf(&[1000; 8], 8000));
        let mut out = vec![1.0; 8];
        m.render(&mut out);
        assert_eq!(out, vec![0.0; 8]);
        assert_eq!(m.active_voices(), 0);
        m.buffer_mut(id).unwrap().play(false);
        assert_eq!(m.active_voices(), 1);
    }

    #[test]
    fn voices_sum() {
        let mut m = Mixer::new(8000);
        for v in [8192, 4096] {
            let id = m.add_buffer(buf(&[v; 8], 8000));
            m.buffer_mut(id).unwrap().play(true);
        }
        let mut out = vec![0.0; 8];
        m.render(&mut out);
        assert_eq!(out, vec![0.375; 8]);
    }

    #[test]
    fn side_output_takes_routed_voices() {
        use crate::dsound::VoiceOutput;
        // 0.25 on the main mix, 0.125 on the side, as mono.
        let mut m = Mixer::new(8000);
        let main = m.add_buffer(buf(&[8192; 8], 8000));
        let pad = m.add_buffer(buf(&[4096; 8], 8000));
        m.buffer_mut(main).unwrap().play(true);
        m.buffer_mut(pad).unwrap().play(true);
        m.buffer_mut(pad).unwrap().set_output(VoiceOutput::Side);
        let (mut out, mut side) = (vec![9.0; 8], vec![9.0; 8]);
        m.render_split(&mut out, Some(&mut side));
        assert_eq!(out, vec![0.25; 8]);
        assert_eq!(side, vec![0.125; 8]);

        // Both: heard in each.
        m.buffer_mut(pad).unwrap().set_output(VoiceOutput::Both);
        m.render_split(&mut out, Some(&mut side));
        assert_eq!(out, vec![0.375; 8]);
        assert_eq!(side, vec![0.125; 8]);
    }

    #[test]
    fn side_voices_fall_back_to_main_without_a_side_output() {
        use crate::dsound::VoiceOutput;
        let mut m = Mixer::new(8000);
        let pad = m.add_buffer(buf(&[4096; 8], 8000));
        m.buffer_mut(pad).unwrap().play(true);
        m.buffer_mut(pad).unwrap().set_output(VoiceOutput::Side);
        let mut out = vec![0.0; 8];
        m.render(&mut out);
        assert_eq!(out, vec![0.125; 8]);
    }

    #[test]
    fn side_output_applies_volume_and_master() {
        use crate::dsound::VoiceOutput;
        let mut m = Mixer::new(8000);
        m.set_master_volume(-600);
        let id = m.add_buffer(buf(&[16384; 8], 8000));
        let b = m.buffer_mut(id).unwrap();
        b.play(true);
        b.set_volume(-600);
        b.set_pan(-10000); // pan is a main-mix property; the side ignores it
        b.set_output(VoiceOutput::Side);
        let (mut out, mut side) = (vec![0.0; 8], vec![0.0; 8]);
        m.render_split(&mut out, Some(&mut side));
        assert_eq!(out, vec![0.0; 8]);
        let want = 0.5 * mb_to_gain(-600) * mb_to_gain(-600);
        for v in side {
            assert!((v - want).abs() < 1e-6, "{v} vs {want}");
        }
    }

    #[test]
    fn resamples_to_host_rate() {
        // 4000 Hz source on an 8000 Hz host: each source frame spans two
        // output frames, interpolated linearly.
        let mut m = Mixer::new(8000);
        let id = m.add_buffer(buf(&[0, 16384, 16384, 16384], 4000));
        m.buffer_mut(id).unwrap().play(false);
        let mut out = vec![0.0; 8];
        m.render(&mut out);
        let l: Vec<f32> = out.iter().step_by(2).copied().collect();
        assert_eq!(l, vec![0.0, 0.25, 0.5, 0.5]);
    }

    #[test]
    fn pan_and_master_volume() {
        let mut m = Mixer::new(8000);
        let id = m.add_buffer(buf(&[16384; 4], 8000));
        let b = m.buffer_mut(id).unwrap();
        b.set_pan(10_000); // full right: left side attenuated by 100 dB
        b.play(true);
        m.set_master_volume(-600);
        let mut out = vec![0.0; 4];
        m.render(&mut out);
        assert!(out[0].abs() < 1e-4);
        assert!((out[1] - 0.5 * mb_to_gain(-600)).abs() < 1e-5);
    }

    #[test]
    fn spatial_source_is_panned_right() {
        let mut m = Mixer::new(8000);
        let id = m.add_buffer(buf(&[16384; 4], 8000));
        let b = m.buffer_mut(id).unwrap();
        b.set_source_3d(Some(Source3d { position: Vec3::new(1.0, 0.0, 0.0), ..Default::default() }));
        b.play(true);
        let mut out = vec![0.0; 2];
        m.render(&mut out);
        assert!(out[1] > out[0]);
    }

    /// The default 3D routing Halo 2 gives a mix-in: crosstalk bins + I3DL2.
    const BINS_3D: [(u32, i32); 5] = [(6, 0), (8, 0), (7, 0), (9, 0), (10, 0)];

    fn first(m: &mut Mixer, frames: usize) -> (f32, f32) {
        let mut out = vec![0.0; frames * 2];
        m.render(&mut out);
        (out[0], out[1])
    }

    #[test]
    fn speaker_bins_fold_to_stereo() {
        let mut m = Mixer::new(8000);
        let id = m.add_buffer(buf(&[16384; 4], 8000));
        let b = m.buffer_mut(id).unwrap();
        b.play(true);
        b.set_mix_bins(Some(vec![(0, 0), (1, -600)]));
        let (l, r) = first(&mut m, 2);
        assert_eq!(l, 0.5);
        assert!((r - 0.5 * mb_to_gain(-600)).abs() < 1e-6);
        // Centre at -3 dB in both, LFE, reverb and an unread FX send dropped.
        m.buffer_mut(id).unwrap().set_mix_bins(Some(vec![(2, 0), (3, 0), (10, 0), (20, 0)]));
        let (l, r) = first(&mut m, 2);
        assert!((l - 0.5 * route::CENTER_GAIN).abs() < 1e-6 && l == r);
        // A stereo voice: even pairs carry left, odd pairs right.
        let bytes: Vec<u8> = [8192i16, -8192].repeat(4).iter().flat_map(|s| s.to_le_bytes()).collect();
        let st = SoundBuffer::from_data(WaveFormat::pcm16(2, 8000).unwrap(), bytes).unwrap();
        let sid = m.add_buffer(st);
        m.remove_buffer(id);
        let b = m.buffer_mut(sid).unwrap();
        b.play(true);
        b.set_mix_bins(Some(vec![(1, 0), (0, 0)])); // swapped
        assert_eq!(first(&mut m, 2), (-0.25, 0.25));
        // Voice volume scales every bin.
        m.buffer_mut(sid).unwrap().set_volume(-10_000);
        assert_eq!(first(&mut m, 2), (0.0, 0.0));
    }

    #[test]
    fn positioned_stream_through_a_3d_mix_in() {
        let mut m = Mixer::new(8000);
        let sid = m.add_stream(Stream::new(WaveFormat::pcm16(1, 8000).unwrap()).unwrap());
        let pcm: Vec<u8> = [16384i16; 64].iter().flat_map(|s| s.to_le_bytes()).collect();
        let s = m.stream_mut(sid).unwrap();
        s.set_mix_bins(Some(vec![(0x17, 0)]));
        let mut mix = MixIn::new(0x17);
        mix.set_mix_bins(Some(BINS_3D.to_vec()));
        mix.set_source_3d(Some(Source3d { position: Vec3::new(5.0, 0.0, 0.0), rolloff_factor: 0.0, ..Default::default() }));
        let mid = m.add_mix_in(mix);
        // A stopped mix-in passes nothing, and the packet still plays out.
        m.stream_mut(sid).unwrap().submit(&pcm).unwrap();
        assert_eq!(first(&mut m, 4), (0.0, 0.0));
        m.mix_in_mut(mid).unwrap().play();
        let (l, r) = first(&mut m, 4);
        assert_eq!(r, 0.5, "hard right, full right ear");
        assert!(l > 0.05 && l < 0.15, "far ear down ~16 dB: {l}");
        // Moving the source to the left mirrors the pan.
        m.mix_in_mut(mid).unwrap().set_source_3d(Some(Source3d { position: Vec3::new(-5.0, 0.0, 0.0), rolloff_factor: 0.0, ..Default::default() }));
        let (l2, r2) = first(&mut m, 4);
        assert_eq!((l2, r2), (r, l));
        // Mix-in volume and send volume both apply.
        m.mix_in_mut(mid).unwrap().set_volume(-600);
        m.stream_mut(sid).unwrap().set_mix_bins(Some(vec![(0x17, -600)]));
        let (l3, _) = first(&mut m, 4);
        assert!((l3 - 0.5 * mb_to_gain(-600) * mb_to_gain(-600)).abs() < 1e-6);
        // Distance attenuation through the listener.
        m.mix_in_mut(mid).unwrap().set_volume(0);
        m.stream_mut(sid).unwrap().set_mix_bins(Some(vec![(0x17, 0)]));
        m.mix_in_mut(mid).unwrap().set_source_3d(Some(Source3d { position: Vec3::new(0.0, 0.0, 5.0), ..Default::default() }));
        let (l4, r4) = first(&mut m, 4);
        assert!((l4 - 0.1).abs() < 1e-6 && l4 == r4, "0.5 * 1/(1+4): {l4}");
        assert!(m.stream_mut(sid).unwrap().pending() > 0);
    }

    #[test]
    fn front_and_back_crosstalk_bins_split_a_3d_voice() {
        let mut m = Mixer::new(8000);
        let id = m.add_buffer(buf(&[16384; 4], 8000));
        let b = m.buffer_mut(id).unwrap();
        b.play(true);
        // Back bins muted: a source behind is silent, one ahead is not.
        b.set_mix_bins(Some(vec![(6, 0), (8, -10_000), (7, 0), (9, -10_000)]));
        b.set_source_3d(Some(Source3d { position: Vec3::new(0.0, 0.0, -2.0), rolloff_factor: 0.0, ..Default::default() }));
        assert_eq!(first(&mut m, 2), (0.0, 0.0));
        m.buffer_mut(id).unwrap().set_source_3d(Some(Source3d { position: Vec3::new(0.0, 0.0, 2.0), rolloff_factor: 0.0, ..Default::default() }));
        assert_eq!(first(&mut m, 2), (0.5, 0.5));
        // Without a 3D source the crosstalk bins are plain left/right bins.
        m.buffer_mut(id).unwrap().set_source_3d(None);
        assert_eq!(first(&mut m, 2), (0.5, 0.5));
    }

    #[test]
    fn mix_in_chains_mix_in_one_render_whatever_their_order() {
        for reversed in [false, true] {
            let mut m = Mixer::new(8000);
            let mut outer = MixIn::new(0x18);
            outer.set_mix_bins(Some(vec![(0, 0), (1, 0)]));
            outer.play();
            let mut inner = MixIn::new(0x17);
            inner.set_mix_bins(Some(vec![(0x18, -600)]));
            inner.play();
            if reversed {
                m.add_mix_in(outer);
                m.add_mix_in(inner);
            } else {
                m.add_mix_in(inner);
                m.add_mix_in(outer);
            }
            let id = m.add_buffer(buf(&[16384; 4], 8000));
            let b = m.buffer_mut(id).unwrap();
            b.set_mix_bins(Some(vec![(0x17, 0)]));
            b.play(true);
            let (l, r) = first(&mut m, 2);
            assert!((l - 0.5 * mb_to_gain(-600)).abs() < 1e-6 && l == r, "{reversed}: {l}");
        }
        // A cycle terminates, and the send into the mix-in already mixed is dropped.
        let mut m = Mixer::new(8000);
        for (bin, to) in [(0x17, 0x18), (0x18, 0x17)] {
            let mut x = MixIn::new(bin);
            x.set_mix_bins(Some(vec![(to, 0), (0, 0)]));
            x.play();
            m.add_mix_in(x);
        }
        let id = m.add_buffer(buf(&[16384; 4], 8000));
        let b = m.buffer_mut(id).unwrap();
        b.set_mix_bins(Some(vec![(0x17, 0)]));
        b.play(true);
        assert_eq!(first(&mut m, 2), (1.0, 0.0), "0x17 then 0x18, both to the left");
        assert_eq!(m.mix_in_count(), 2);
    }

    use crate::fx::{self, EnvSpec, FilterSpec, I3dl2Source, LfoSpec};
    use crate::reverb::I3dl2Listener;

    /// A 1 s mono 16-bit sine at `hz` and 48 kHz, half amplitude.
    fn sine_pcm(hz: f64, secs: f64) -> Vec<u8> {
        (0..(48_000.0 * secs) as usize)
            .flat_map(|i| (((2.0 * std::f64::consts::PI * hz * i as f64 / 48_000.0).sin() * 16384.0) as i16).to_le_bytes())
            .collect()
    }

    fn sine_stream(hz: f64, secs: f64) -> (Mixer, StreamId, crate::dsound::PacketId) {
        let mut m = Mixer::new(48_000);
        let sid = m.add_stream(Stream::new(WaveFormat::pcm16(1, 48_000).unwrap()).unwrap());
        let pkt = m.stream_mut(sid).unwrap().submit(&sine_pcm(hz, secs)).unwrap();
        (m, sid, pkt)
    }

    /// Left-channel RMS of the second half of a 1 s render, as a gain against
    /// the half-amplitude input.
    fn tone_gain(m: &mut Mixer) -> f64 {
        let mut out = vec![0.0f32; 48_000 * 2];
        m.render(&mut out);
        let tail: Vec<f64> = out.chunks(2).skip(24_000).map(|f| f64::from(f[0])).collect();
        let rms = (tail.iter().map(|v| v * v).sum::<f64>() / tail.len() as f64).sqrt();
        rms * 2.0 * std::f64::consts::SQRT_2
    }

    #[test]
    fn stream_filter_shapes_the_tone() {
        let lp = FilterSpec::from_dsfilter(fx::FILTER_DLS2, 0, [fx::hz_to_coeff(1000.0), 46341, 0, 0]);
        for (hz, expect_db) in [(100.0, 0.0), (1000.0, -3.0), (4000.0, -24.0)] {
            let (mut m, sid, _) = sine_stream(hz, 1.0);
            m.stream_mut(sid).unwrap().set_filter(lp);
            let g = 20.0 * tone_gain(&mut m).log10();
            assert!((g - expect_db).abs() < 2.5, "{hz} Hz: {g} dB, want {expect_db}");
        }
        // High-pass through MULTI.
        let hp = FilterSpec::from_dsfilter(fx::FILTER_MULTI, 4, [fx::hz_to_coeff(2000.0), 46341, 0, 0]);
        let (mut m, sid, _) = sine_stream(200.0, 1.0);
        m.stream_mut(sid).unwrap().set_filter(hp);
        assert!(20.0 * tone_gain(&mut m).log10() < -30.0);
        // Removing the filter restores the signal.
        let (mut m, sid, _) = sine_stream(4000.0, 1.0);
        m.stream_mut(sid).unwrap().set_filter(lp);
        m.stream_mut(sid).unwrap().set_filter(FilterSpec::default());
        assert!((tone_gain(&mut m) - 1.0).abs() < 0.02);
    }

    #[test]
    fn buffer_filter_and_mix_in_occlusion() {
        let lp = FilterSpec::from_dsfilter(fx::FILTER_DLS2, 0, [fx::hz_to_coeff(500.0), 46341, 0, 0]);
        let mut m = Mixer::new(48_000);
        let id = m.add_buffer(SoundBuffer::from_data(WaveFormat::pcm16(1, 48_000).unwrap(), sine_pcm(3000.0, 1.0)).unwrap());
        let b = m.buffer_mut(id).unwrap();
        b.set_filter(lp);
        b.play(true);
        assert!(20.0 * tone_gain(&mut m).log10() < -30.0);
        // An occluded 3D mix-in: highs gone and the whole thing 5 dB down.
        let mut m = Mixer::new(48_000);
        let sid = m.add_stream(Stream::new(WaveFormat::pcm16(1, 48_000).unwrap()).unwrap());
        m.stream_mut(sid).unwrap().set_mix_bins(Some(vec![(0x17, 0)]));
        let mut mix = MixIn::new(0x17);
        mix.set_mix_bins(Some(vec![(0, 0), (1, 0)]));
        mix.set_i3dl2_source(Some(I3dl2Source { occlusion_hf: -2000, occlusion_lf_ratio: 0.25, ..Default::default() }));
        mix.play();
        m.add_mix_in(mix);
        m.stream_mut(sid).unwrap().submit(&sine_pcm(100.0, 1.0)).unwrap();
        let low = 20.0 * tone_gain(&mut m).log10();
        assert!((low + 5.0).abs() < 0.6, "LF level: {low}");
    }

    #[test]
    fn fx_do_not_change_packet_completion() {
        // Two identical streams, one with every effect on: the packets
        // complete on the same render, in the same order.
        let run = |fx_on: bool| {
            let mut m = Mixer::new(48_000);
            let sid = m.add_stream(Stream::new(WaveFormat::pcm16(1, 48_000).unwrap()).unwrap());
            if fx_on {
                let s = m.stream_mut(sid).unwrap();
                s.set_filter(FilterSpec::from_dsfilter(fx::FILTER_DLS2, 0, [fx::hz_to_coeff(500.0), 46341, 0, 0]));
                s.set_lfo(0, Some(LfoSpec::from_dslfo(0, 200, 0, 32, 64)));
                s.set_envelope(Some(EnvSpec { start_stage: 2, delay: 10, attack: 100, hold: 0, decay: 0, release: 50, sustain: 255 }));
                s.set_headroom(600);
            }
            let ids: Vec<_> = (0..4).map(|_| m.stream_mut(sid).unwrap().submit(&sine_pcm(440.0, 0.1)).unwrap()).collect();
            let mut done = Vec::new();
            for step in 0..22 {
                let mut out = vec![0.0f32; 2 * 1000];
                m.render(&mut out);
                for (id, st) in m.stream_mut(sid).unwrap().take_completed() {
                    done.push((step, ids.iter().position(|&i| i == id).unwrap(), st));
                }
            }
            done
        };
        let plain = run(false);
        assert_eq!(plain.len(), 4);
        assert_eq!(plain, run(true));
    }

    #[test]
    fn envelope_restarts_with_a_new_sound() {
        let (mut m, sid, _) = sine_stream(1000.0, 1.0);
        m.stream_mut(sid).unwrap().set_envelope(Some(EnvSpec { start_stage: 2, delay: 0, attack: 94, hold: 0, decay: 0, release: 0, sustain: 255 }));
        let mut out = vec![0.0f32; 2 * 4800];
        m.render(&mut out);
        let early = out.iter().step_by(2).take(200).map(|v| v.abs()).fold(0.0f32, f32::max);
        assert!(early < 0.1, "fading in: {early}");
        // Flush and start a second sound: the fade begins again.
        m.stream_mut(sid).unwrap().flush();
        m.stream_mut(sid).unwrap().submit(&sine_pcm(1000.0, 1.0)).unwrap();
        m.render(&mut out);
        let early2 = out.iter().step_by(2).take(200).map(|v| v.abs()).fold(0.0f32, f32::max);
        assert!(early2 < 0.1, "second fade-in: {early2}");
    }

    #[test]
    fn effects_do_not_depend_on_render_block_size() {
        let run = |block: usize| {
            let (mut m, sid, _) = sine_stream(700.0, 0.5);
            let s = m.stream_mut(sid).unwrap();
            s.set_filter(FilterSpec::from_dsfilter(fx::FILTER_DLS2, 0, [fx::hz_to_coeff(900.0), 20000, 0, 0]));
            s.set_lfo(0, Some(LfoSpec::from_dslfo(10, 300, 0, 24, 48)));
            s.set_lfo(1, Some(LfoSpec::from_dslfo(0, 120, 20, 0, 0)));
            s.set_envelope(Some(EnvSpec { start_stage: 2, delay: 0, attack: 20, hold: 0, decay: 20, release: 0, sustain: 128 }));
            s.set_mix_bins(Some(vec![(0, 0), (route::I3DL2, 0)]));
            m.set_reverb(Some(I3dl2Listener { reflections: -300, ..env_params() }));
            let mut all = Vec::new();
            while all.len() < 2 * 24_000 {
                let mut out = vec![0.0f32; 2 * block];
                m.render(&mut out);
                all.extend(out);
            }
            all.truncate(2 * 24_000);
            all
        };
        assert_eq!(run(24_000), run(97));
        assert_eq!(run(512), run(24_000));
    }

    fn env_params() -> I3dl2Listener {
        I3dl2Listener {
            room: 0,
            room_hf: 0,
            room_rolloff: 0.0,
            decay_time: 1.5,
            decay_hf_ratio: 0.8,
            reflections: -1000,
            reflections_delay: 0.01,
            reverb: -500,
            reverb_delay: 0.02,
            diffusion: 100.0,
            density: 100.0,
            hf_reference: 5000.0,
        }
    }

    #[test]
    fn voice_headroom_attenuates_every_send() {
        let level = |voice_mb: i32| {
            let mut m = Mixer::new(8000);
            let id = m.add_buffer(buf(&[16384; 4], 8000));
            let b = m.buffer_mut(id).unwrap();
            b.set_mix_bins(Some(vec![(0, 0)]));
            b.set_headroom(voice_mb);
            b.play(true);
            first(&mut m, 2).0
        };
        assert_eq!(level(0), 0.5, "Halo's setting: no change");
        assert!((level(600) - 0.25).abs() < 0.002, "-6 dB");
        assert!(level(6000) < 0.03);
        // A plain (no mix bins) voice is attenuated too.
        let mut m = Mixer::new(8000);
        let id = m.add_buffer(buf(&[16384; 4], 8000));
        let b = m.buffer_mut(id).unwrap();
        b.set_headroom(600);
        b.play(true);
        assert!((first(&mut m, 2).0 - 0.25).abs() < 0.002);
    }

    #[test]
    fn reverb_send_adds_a_tail_that_outlives_the_voice() {
        let render_tail = |env: Option<I3dl2Listener>| {
            let mut m = Mixer::new(48_000);
            m.set_reverb(env);
            let id = m.add_buffer(buf(&[16384; 480], 48_000));
            let b = m.buffer_mut(id).unwrap();
            b.set_mix_bins(Some(vec![(route::I3DL2, 0)])); // wet only
            b.play(false);
            let mut out = vec![0.0f32; 2 * 24_000];
            m.render(&mut out);
            assert!(!m.buffer(id).unwrap().is_playing(), "voice has finished");
            out
        };
        let dry = render_tail(None);
        assert!(dry.iter().all(|&v| v == 0.0), "no environment: the send is dropped");
        let wet = render_tail(Some(env_params()));
        assert!(wet.iter().any(|&v| v != 0.0));
        let late = wet[2 * 12_000..].iter().map(|v| f64::from(*v).powi(2)).sum::<f64>();
        assert!(late > 0.0, "the tail is still sounding half a second after the voice ended");
        assert_eq!(wet, render_tail(Some(env_params())), "deterministic");
        // An inaudible environment passes nothing, and a source's room level scales the send.
        assert!(render_tail(Some(I3dl2Listener { room: -10_000, ..env_params() })).iter().all(|&v| v == 0.0));
        let quiet = {
            let mut m = Mixer::new(48_000);
            m.set_reverb(Some(env_params()));
            let id = m.add_buffer(buf(&[16384; 480], 48_000));
            let b = m.buffer_mut(id).unwrap();
            b.set_mix_bins(Some(vec![(route::I3DL2, 0)]));
            b.set_i3dl2_source(Some(I3dl2Source { room: -2000, ..Default::default() }));
            b.play(false);
            let mut out = vec![0.0f32; 2 * 24_000];
            m.render(&mut out);
            out
        };
        let e = |v: &[f32]| v.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>();
        assert!((10.0 * (e(&quiet) / e(&wet)).log10() + 20.0).abs() < 0.2);
        // Master volume scales the wet signal too.
        let mut m = Mixer::new(48_000);
        m.set_reverb(Some(env_params()));
        m.set_master_volume(-10_000);
        let id = m.add_buffer(buf(&[16384; 480], 48_000));
        m.buffer_mut(id).unwrap().set_mix_bins(Some(vec![(route::I3DL2, 0)]));
        m.buffer_mut(id).unwrap().play(false);
        let mut out = vec![0.0f32; 2 * 4800];
        m.render(&mut out);
        assert!(out.iter().all(|&v| v == 0.0));
        // Removing the environment drops the tail.
        m.set_reverb(None);
        assert!(m.reverb().is_none());
    }

    #[test]
    fn doppler_shifts_pitch_and_respects_factors() {
        // A tone approaching the listener at 20% of the speed of sound is
        // heard 1 / (1 - 0.2) = 1.25 times higher.
        let zero_crossings = |velocity: f32, factor: f32, listener_factor: f32| {
            let mut m = Mixer::new(48_000);
            m.listener_mut().doppler_factor = listener_factor;
            let id = m.add_buffer(SoundBuffer::from_data(WaveFormat::pcm16(1, 48_000).unwrap(), sine_pcm(1000.0, 1.0)).unwrap());
            let b = m.buffer_mut(id).unwrap();
            b.set_source_3d(Some(Source3d {
                position: Vec3::new(0.0, 0.0, 50.0),
                velocity: Vec3::new(0.0, 0.0, -velocity),
                rolloff_factor: 0.0,
                doppler_factor: factor,
                ..Default::default()
            }));
            b.play(true);
            let mut out = vec![0.0f32; 2 * 24_000];
            m.render(&mut out);
            out.chunks(2).map(|f| f[0]).collect::<Vec<_>>().windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count()
        };
        let still = zero_crossings(0.0, 1.0, 1.0);
        assert!((499..=500).contains(&still), "1 kHz for half a second: {still}");
        let fast = zero_crossings(68.6, 1.0, 1.0);
        assert!((fast as f64 / still as f64 - 1.25).abs() < 0.01, "{fast}");
        assert_eq!(zero_crossings(68.6, 0.0, 1.0), still, "source factor 0 turns it off");
        assert_eq!(zero_crossings(68.6, 1.0, 0.0), still, "listener factor 0 turns it off");
        let half = zero_crossings(68.6, 0.5, 1.0);
        assert!(half > still && half < fast);
    }

    /// The output frame at which a stream's single packet completes, and the
    /// rendered audio, one frame per render call batch of 100.
    fn completion_frame(setup: impl FnOnce(&mut Stream)) -> (usize, Vec<f32>) {
        let mut m = Mixer::new(48_000);
        let sid = m.add_stream(Stream::new(WaveFormat::pcm16(1, 48_000).unwrap()).unwrap());
        setup(m.stream_mut(sid).unwrap());
        m.stream_mut(sid).unwrap().submit(&sine_pcm(440.0, 0.1)).unwrap();
        let mut all = Vec::new();
        for step in 0..60 {
            let mut out = vec![0.0f32; 2 * 100];
            m.render(&mut out);
            all.extend(out);
            if !m.stream_mut(sid).unwrap().take_completed().is_empty() {
                return ((step + 1) * 100, all);
            }
        }
        panic!("never completed");
    }

    #[test]
    fn pitch_lfo_and_doppler_do_not_move_packet_completion() {
        let (plain, plain_audio) = completion_frame(|_| {});
        assert_eq!(plain, 4800);
        // A deep, fast pitch LFO: the audio changes, the completion does not.
        let (lfo, lfo_audio) = completion_frame(|s| s.set_lfo(1, Some(LfoSpec::from_dslfo(0, 175, 127, 0, 0))));
        assert_eq!(lfo, plain);
        assert_ne!(lfo_audio, plain_audio, "the vibrato is audible");
        // A source approaching fast enough for a big Doppler shift, same story.
        let (dop, dop_audio) = completion_frame(|s| {
            s.set_source_3d(Some(Source3d {
                position: Vec3::new(0.0, 0.0, 50.0),
                velocity: Vec3::new(0.0, 0.0, -68.6),
                rolloff_factor: 0.0,
                ..Default::default()
            }))
        });
        assert_eq!(dop, plain);
        assert_ne!(dop_audio[..2 * 4800], plain_audio[..2 * 4800]);
        assert!(lfo_audio.iter().chain(&dop_audio).all(|v| v.is_finite()));
    }

    /// Drive a stream under a constant Doppler velocity in 100-frame blocks and
    /// return the (min, max) read-position offset it reached.
    fn doppler_phase_range(vz: f32) -> (f64, f64) {
        let mut m = Mixer::new(48_000);
        let sid = m.add_stream(Stream::new(WaveFormat::pcm16(1, 48_000).unwrap()).unwrap());
        m.stream_mut(sid).unwrap().set_source_3d(Some(Source3d {
            position: Vec3::new(0.0, 0.0, 50.0),
            velocity: Vec3::new(0.0, 0.0, vz),
            rolloff_factor: 0.0,
            ..Default::default()
        }));
        m.stream_mut(sid).unwrap().submit(&sine_pcm(300.0, 1.0)).unwrap();
        let (mut lo, mut hi) = (0.0f64, 0.0f64);
        for _ in 0..30 {
            let mut out = vec![0.0f32; 2 * 100];
            m.render(&mut out);
            assert!(out.iter().all(|v| v.is_finite() && v.abs() <= 1.0));
            let off = m.stream_mut(sid).unwrap().phase_offset();
            lo = lo.min(off);
            hi = hi.max(off);
        }
        (lo, hi)
    }

    #[test]
    fn pitch_modulation_keeps_the_phase_offset_bounded() {
        // Receding drifts about -0.47 frame per frame and approaching (ratio
        // clamped at 8) +7, so unclamped both would leave the window within a
        // few thousand frames. They must saturate exactly at the bound.
        let bound = crate::fx::MAX_PHASE_OFFSET;
        let (lo, hi) = doppler_phase_range(343.0);
        assert!(hi <= 1e-9, "receding should not drift forward: {hi}");
        assert_eq!(lo, -bound);
        let (lo, hi) = doppler_phase_range(-343.0);
        assert!(lo >= -1e-9, "approaching should not drift back: {lo}");
        assert_eq!(hi, bound);
    }

    /// A shelf-obstructed source that ends (or starves) mid-render: the tail
    /// must not depend on how the output is chunked.
    fn shelf_render(block: usize, make: impl Fn(&mut Mixer) -> StreamId, frames: usize) -> Vec<f32> {
        let mut m = Mixer::new(48_000);
        let sid = make(&mut m);
        let s = m.stream_mut(sid).unwrap();
        s.set_i3dl2_source(Some(I3dl2Source { obstruction_hf: -2500, obstruction_lf_ratio: 0.2, ..Default::default() }));
        let mut all = Vec::new();
        while all.len() < 2 * frames {
            let mut out = vec![0.0f32; 2 * block];
            m.render(&mut out);
            all.extend(out);
        }
        all.truncate(2 * frames);
        all
    }

    #[test]
    fn direct_shelf_does_not_depend_on_render_block_size() {
        let ends = |m: &mut Mixer| {
            let sid = m.add_stream(Stream::new(WaveFormat::pcm16(1, 48_000).unwrap()).unwrap());
            m.stream_mut(sid).unwrap().submit(&sine_pcm(900.0, 0.01)).unwrap();
            sid
        };
        let a = shelf_render(64, ends, 4096);
        let b = shelf_render(512, ends, 4096);
        assert_eq!(a, b);
        assert!(a.iter().any(|v| v.abs() > 0.01));
    }

    #[test]
    fn direct_shelf_survives_starvation_and_resume_identically() {
        let run = |block: usize| {
            let mut m = Mixer::new(48_000);
            let sid = m.add_stream(Stream::new(WaveFormat::pcm16(1, 48_000).unwrap()).unwrap());
            m.stream_mut(sid).unwrap().set_i3dl2_source(Some(I3dl2Source {
                obstruction_hf: -2500,
                obstruction_lf_ratio: 0.2,
                ..Default::default()
            }));
            let mut all = Vec::new();
            let mut render = |m: &mut Mixer, frames: usize| {
                let mut left = frames;
                while left > 0 {
                    let n = left.min(block);
                    let mut out = vec![0.0f32; 2 * n];
                    m.render(&mut out);
                    all.extend(out);
                    left -= n;
                }
            };
            m.stream_mut(sid).unwrap().submit(&sine_pcm(900.0, 0.01)).unwrap();
            render(&mut m, 1536);
            m.stream_mut(sid).unwrap().submit(&sine_pcm(1300.0, 0.01)).unwrap();
            render(&mut m, 1536);
            all
        };
        assert_eq!(run(64), run(512));
    }

    #[test]
    fn packet_boundaries_do_not_change_a_stream_under_receding_doppler() {
        let ramp = |lo: usize, hi: usize| -> Vec<u8> {
            (lo..hi).flat_map(|i| ((i as i16) * 4).to_le_bytes()).collect()
        };
        let run = |split: bool| {
            let mut m = Mixer::new(48_000);
            let sid = m.add_stream(Stream::new(WaveFormat::pcm16(1, 48_000).unwrap()).unwrap());
            let s = m.stream_mut(sid).unwrap();
            s.set_source_3d(Some(Source3d {
                position: Vec3::new(0.0, 0.0, 50.0),
                velocity: Vec3::new(0.0, 0.0, 343.0),
                rolloff_factor: 0.0,
                ..Default::default()
            }));
            if split {
                s.submit(&ramp(0, 2000)).unwrap();
                s.submit(&ramp(2000, 4000)).unwrap();
            } else {
                s.submit(&ramp(0, 4000)).unwrap();
            }
            let mut out = vec![0.0f32; 2 * 3000];
            m.render(&mut out);
            out
        };
        let (one, two) = (run(false), run(true));
        assert_eq!(one, two);
        // The receding read position is behind nominal by now, so the join at
        // frame 2000 was read through history rather than clamped.
        assert!(one.iter().any(|v| *v != 0.0));
    }

    #[test]
    fn obstruction_leaves_the_wet_send_alone() {
        // A wet-only source (reverb bin only) with an obstructed direct path:
        // changing obstruction must not change the wet signal.
        let wet_energy = |obstruction_hf: i32| {
            let mut m = Mixer::new(48_000);
            m.set_reverb(Some(env_params()));
            let id = m.add_buffer(SoundBuffer::from_data(WaveFormat::pcm16(1, 48_000).unwrap(), sine_pcm(5000.0, 0.2)).unwrap());
            let b = m.buffer_mut(id).unwrap();
            b.set_mix_bins(Some(vec![(route::I3DL2, 0)]));
            b.set_i3dl2_source(Some(I3dl2Source { obstruction_hf, obstruction_lf_ratio: 0.0, ..Default::default() }));
            b.play(false);
            let mut out = vec![0.0f32; 2 * 24_000];
            m.render(&mut out);
            out.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>()
        };
        let open = wet_energy(0);
        assert!(open > 0.0);
        assert_eq!(wet_energy(-2000), open);
        // The direct path of the same source still darkens.
        let direct_energy = |obstruction_hf: i32| {
            let mut m = Mixer::new(48_000);
            let id = m.add_buffer(SoundBuffer::from_data(WaveFormat::pcm16(1, 48_000).unwrap(), sine_pcm(5000.0, 0.2)).unwrap());
            let b = m.buffer_mut(id).unwrap();
            b.set_mix_bins(Some(vec![(0, 0)]));
            b.set_i3dl2_source(Some(I3dl2Source { obstruction_hf, obstruction_lf_ratio: 0.0, ..Default::default() }));
            b.play(false);
            let mut out = vec![0.0f32; 2 * 9600];
            m.render(&mut out);
            out.iter().skip(2 * 4800).map(|v| f64::from(*v).powi(2)).sum::<f64>()
        };
        assert!(direct_energy(-2000) < direct_energy(0) * 0.05);
    }

    #[test]
    fn extreme_source_and_doppler_values_stay_finite() {
        let nan = f32::NAN;
        let inf = f32::INFINITY;
        for (factor, listener_factor, vel, pos) in [
            (inf, 1.0, 1.0e30, 5.0),
            (1.0, inf, -1.0e30, 5.0),
            (nan, nan, nan, 5.0),
            (1.0, 1.0, 1.0, nan),
            (1.0e30, 1.0e30, inf, 0.0),
        ] {
            let mut m = Mixer::new(48_000);
            m.listener_mut().doppler_factor = listener_factor;
            m.listener_mut().velocity = Vec3::new(0.0, 0.0, vel);
            let id = m.add_buffer(SoundBuffer::from_data(WaveFormat::pcm16(1, 48_000).unwrap(), sine_pcm(440.0, 0.5)).unwrap());
            let b = m.buffer_mut(id).unwrap();
            b.set_source_3d(Some(Source3d {
                position: Vec3::new(0.0, 0.0, pos),
                velocity: Vec3::new(0.0, 0.0, vel),
                doppler_factor: factor,
                min_distance: nan,
                ..Default::default()
            }));
            b.play(true);
            let sid = m.add_stream(Stream::new(WaveFormat::pcm16(1, 48_000).unwrap()).unwrap());
            m.stream_mut(sid).unwrap().submit(&sine_pcm(440.0, 0.1)).unwrap();
            m.stream_mut(sid).unwrap().set_source_3d(Some(Source3d {
                position: Vec3::new(0.0, 0.0, pos),
                velocity: Vec3::new(0.0, 0.0, vel),
                doppler_factor: factor,
                ..Default::default()
            }));
            let mut out = vec![0.0f32; 2 * 4800];
            m.render(&mut out);
            assert!(out.iter().all(|v| v.is_finite()), "{factor} {listener_factor} {vel} {pos}");
            let l = Listener { doppler_factor: listener_factor, velocity: Vec3::new(0.0, 0.0, vel), ..Default::default() };
            let p = l.spatial(&Source3d { position: Vec3::new(0.0, 0.0, 5.0), velocity: Vec3::new(0.0, 0.0, vel), doppler_factor: factor, ..Default::default() }).pitch;
            assert!(p.is_finite() && p > 0.0, "{p}");
        }
    }

    #[test]
    fn doppler_ratio_formula() {
        let l = Listener::default();
        let src = |vel: Vec3| Source3d { position: Vec3::new(0.0, 0.0, 10.0), velocity: vel, ..Default::default() };
        // Source receding from the listener at 10% c: 1 / 1.1.
        let away = l.spatial(&src(Vec3::new(0.0, 0.0, 34.3))).pitch;
        assert!((away - 1.0 / 1.1).abs() < 1e-4, "{away}");
        // The listener moving toward the source at 10% c: 1.1.
        let mut l2 = Listener { velocity: Vec3::new(0.0, 0.0, 34.3), ..Default::default() };
        assert!((l2.spatial(&src(Vec3::default())).pitch - 1.1).abs() < 1e-4);
        // Distance factor converts world units to metres.
        l2.distance_factor = 0.5;
        assert!((l2.spatial(&src(Vec3::default())).pitch - 1.05).abs() < 1e-4);
        // Sideways motion has no radial component; absurd speeds stay finite.
        assert_eq!(l.spatial(&src(Vec3::new(100.0, 0.0, 0.0))).pitch, 1.0);
        let wild = l.spatial(&src(Vec3::new(0.0, 0.0, -1.0e9))).pitch;
        assert!(wild.is_finite() && wild < 11.0, "{wild}");
        // Head-relative sources have none.
        let hr = Source3d { mode: crate::dsound::Mode3d::HeadRelative, velocity: Vec3::new(0.0, 0.0, -100.0), ..src(Vec3::default()) };
        assert_eq!(l.spatial(&hr).pitch, 1.0);
    }

    #[test]
    fn stream_health_counts_late_feeds_and_unrouted_play() {
        let mut m = Mixer::new(8000);
        let sid = m.add_stream(Stream::new(WaveFormat::pcm16(1, 8000).unwrap()).unwrap());
        let pcm: Vec<u8> = [8192i16; 64].iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut block = vec![0.0; 2 * 100];
        // 64 frames into a 100-frame block: dry 36 frames before its end.
        m.stream_mut(sid).unwrap().submit(&pcm).unwrap();
        m.render(&mut block);
        // Fed again before the next block: a 36-frame gap, one starve.
        m.stream_mut(sid).unwrap().submit(&pcm).unwrap();
        m.render(&mut block);
        let h = m.stream_health();
        assert_eq!((h.starves, h.starve_frames, h.unrouted_frames), (1, 36, 0));
        // Fed again a second later: a new sound, not a starve.
        for _ in 0..80 {
            m.render(&mut block);
        }
        m.stream_mut(sid).unwrap().submit(&pcm).unwrap();
        m.render(&mut block);
        assert_eq!(m.stream_health().starves, 1);
        // A flush in between forgets it ran dry.
        m.stream_mut(sid).unwrap().flush();
        m.stream_mut(sid).unwrap().submit(&pcm).unwrap();
        m.render(&mut block);
        assert_eq!(m.stream_health().starves, 1);
        // Routed only to a mix-in that is not playing: consumed, not heard.
        m.stream_mut(sid).unwrap().set_mix_bins(Some(vec![(0x17, 0)]));
        m.add_mix_in(MixIn::new(0x17));
        m.stream_mut(sid).unwrap().submit(&pcm).unwrap();
        m.render(&mut block);
        assert_eq!(m.stream_health().unrouted_frames, 64);
        // An empty bin list is the same; -10000 mB on a real bin is not
        // (that is the game's own silence).
        m.stream_mut(sid).unwrap().set_mix_bins(Some(Vec::new()));
        m.stream_mut(sid).unwrap().submit(&pcm).unwrap();
        m.render(&mut block);
        m.stream_mut(sid).unwrap().set_mix_bins(Some(vec![(0, -10_000)]));
        m.stream_mut(sid).unwrap().submit(&pcm).unwrap();
        m.render(&mut block);
        assert_eq!(m.stream_health().unrouted_frames, 128);
    }

    #[test]
    fn a_tapped_bin_is_captured_instead_of_heard() {
        // Two voice streams on two tapped FX sends (the XHV voice effects'
        // input bins), one on a bin nobody taps.
        let mut m = Mixer::new(8000);
        m.set_taps(&[0x1b, 0x1c]);
        let pcm = |v: i16| -> Vec<u8> { [v; 40].iter().flat_map(|s| s.to_le_bytes()).collect() };
        let a = m.add_stream(Stream::new(WaveFormat::pcm16(1, 8000).unwrap()).unwrap());
        m.stream_mut(a).unwrap().set_mix_bins(Some(vec![(0x1b, 0)]));
        m.stream_mut(a).unwrap().submit(&pcm(8192)).unwrap();
        let b = m.add_stream(Stream::new(WaveFormat::pcm16(1, 8000).unwrap()).unwrap());
        // -602 mB is half.
        m.stream_mut(b).unwrap().set_mix_bins(Some(vec![(0x1c, -602)]));
        m.stream_mut(b).unwrap().submit(&pcm(8192)).unwrap();
        m.set_master_volume(-602);
        let mut out = vec![9.0; 2 * 50];
        m.render(&mut out);
        assert_eq!(out, vec![0.0; 100], "a tapped send is not on the speakers");
        // Every rendered frame is in the tap, silence included, so the tap
        // runs on the mixer's clock. The master volume is the speakers'.
        let ta = m.take_tap(0x1b);
        assert_eq!(ta.len(), 50);
        assert!(ta[..40].iter().all(|&v| v == 0.25) && ta[40..].iter().all(|&v| v == 0.0));
        let tb = m.take_tap(0x1c);
        assert!(tb[..40].iter().all(|&v| (v - 0.125).abs() < 1e-3), "{:?}", &tb[..4]);
        assert!(m.take_tap(0x1b).is_empty(), "taken");
        assert!(m.take_tap(0x1d).is_empty(), "not a tap");
        assert_eq!(m.stream_health().unrouted_frames, 0, "a tap is a destination");
        // Undrained taps keep at most a second.
        for _ in 0..200 {
            m.render(&mut out);
        }
        assert_eq!(m.take_tap(0x1b).len(), 8000);
        // No taps: the bin is an FX send nobody reads again.
        m.set_taps(&[]);
        m.render(&mut out);
        assert!(m.take_tap(0x1b).is_empty());
    }

    #[test]
    fn streams_mix_with_buffers() {
        let mut m = Mixer::new(8000);
        let id = m.add_buffer(buf(&[8192; 4], 8000));
        m.buffer_mut(id).unwrap().play(true);
        let sid = m.add_stream(Stream::new(WaveFormat::pcm16(1, 8000).unwrap()).unwrap());
        let pcm: Vec<u8> = [8192i16; 2].iter().flat_map(|s| s.to_le_bytes()).collect();
        let pkt = m.stream_mut(sid).unwrap().submit(&pcm).unwrap();
        let mut out = vec![0.0; 8];
        m.render(&mut out);
        assert_eq!(&out[..4], &[0.5; 4]);
        assert_eq!(&out[4..], &[0.25; 4]);
        assert_eq!(m.stream_mut(sid).unwrap().take_completed().len(), 1);
        let _ = pkt;
    }

    #[test]
    fn removed_handles_disappear() {
        let mut m = Mixer::new(8000);
        let id = m.add_buffer(buf(&[1; 2], 8000));
        assert!(m.remove_buffer(id).is_some());
        assert!(m.buffer(id).is_none());
    }

    #[test]
    fn counts_include_stopped_voices() {
        let mut m = Mixer::new(8000);
        assert_eq!((m.buffer_count(), m.stream_count()), (0, 0));
        let id = m.add_buffer(buf(&[1; 2], 8000));
        let sid = m.add_stream(Stream::new(WaveFormat::pcm16(1, 8000).unwrap()).unwrap());
        assert_eq!((m.buffer_count(), m.stream_count()), (1, 1));
        assert_eq!(m.active_voices(), 0, "stopped voices are still counted");
        m.remove_buffer(id);
        m.remove_stream(sid);
        assert_eq!((m.buffer_count(), m.stream_count()), (0, 0));
    }

    #[test]
    fn adpcm_buffer_plays_through_mixer() {
        use crate::adpcm::tests::encode;
        let pcm: Vec<i16> = (0..64).map(|i| ((i as f32 * 0.2).sin() * 8000.0) as i16).collect();
        let data = encode(&pcm, 1);
        let b = SoundBuffer::from_data(WaveFormat::xbox_adpcm(1, 8000).unwrap(), data).unwrap();
        let mut m = Mixer::new(8000);
        let id = m.add_buffer(b);
        m.buffer_mut(id).unwrap().play(false);
        let mut out = vec![0.0; 128];
        m.render(&mut out);
        // First frame is the exact header sample; the tone is audible.
        assert_eq!(out[0], pcm[0] as f32 / 32768.0);
        assert!(out.iter().any(|v| v.abs() > 0.1));
        assert!(!m.buffer(id).unwrap().is_playing());
    }
}
