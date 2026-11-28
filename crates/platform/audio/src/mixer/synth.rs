//! A render in two halves, so the second can run on another thread.
//!
//! [`Mixer::render_split`] does everything in one call. The same work splits
//! at the point where a voice's frames have been read out of its buffer:
//!
//! - **State half** ([`Mixer::prepare_job`]): moves every voice's play cursor,
//!   status, loop and packet queue on by one block, reads each playing
//!   voice's frames at unity gain, and returns them with the routing inputs
//!   as a [`MixJob`]. This is everything the guest can observe, so it runs
//!   where the guest's own calls run, in order with them.
//! - **Sample half** ([`Synth::mix`]): effects, filters, panning, 3D, the
//!   mix-in buses, HRTF and reverb over a job's frames. Nothing the guest
//!   reads depends on it, so it runs anywhere, as long as one `Synth` mixes
//!   the jobs of one mixer in the order they were prepared.
//!
//! The `Synth` owns the memory that only the sample half touches: each voice's
//! filter state, each mix-in's HRTF history and the reverb tail. Jobs carry
//! the rest by value. A voice that restarts ([`FxState::reset`]) bumps an
//! epoch the job carries, which clears its filter state in the `Synth`.
//!
//! `prepare_job` then `Synth::mix` gives the same samples as `render_split`
//! (`deferred_mix_equals_the_inline_render`). Taps ([`Mixer::set_taps`]) feed
//! guest memory (the XHV voice rings), so they are filled in the state half:
//! a voice whose bins reach only tapped bins is filtered, routed and tapped
//! there, in order, and left out of the job (`a_tap_only_voice_is_tapped_in_the_state_half`).
//! A voice that also reaches another bin, a mix-in on a tapped bin, or a
//! tapped voice on the side output makes the block mix inline
//! ([`Mixer::can_defer`]).

use std::collections::BTreeMap;
use std::ops::Range;

use super::{Bus, MixInId, Mixer, Route, STARVE_WINDOW_MS, begin_taps, route_voice, to_side};
use crate::dsound::{BufferId, Listener, StreamId, VoiceOutput, VoiceParams, mb_to_gain};
use crate::route::{self, Dest};
use crate::fx::FxState;
use crate::hrtf::Hrtf;
use crate::reverb::{I3dl2Listener, Reverb};

/// Which voice a job's block belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Key {
    Buffer(BufferId),
    Stream(StreamId),
    MixIn(MixInId),
}

/// One voice's frames for a block, before effects and routing.
#[derive(Debug)]
struct JobVoice {
    key: Key,
    params: VoiceParams,
    channels: usize,
    /// The voice clock of the block's first frame.
    c0: u64,
    fx: FxState,
    /// Where its frames are in [`MixJob::pcm`].
    pcm: Range<usize>,
}

/// A playing mix-in's parameters for a block.
#[derive(Debug)]
struct JobBus {
    id: MixInId,
    bin: u32,
    params: VoiceParams,
    c0: u64,
    fx: FxState,
}

/// What [`Synth::mix`] needs to turn one block of voice state into samples.
#[derive(Debug)]
pub struct MixJob {
    frames: usize,
    rate: u32,
    master: f32,
    hrtf: bool,
    distance_reverb: bool,
    listener: Listener,
    environment: Option<I3dl2Listener>,
    side: bool,
    voices: Vec<JobVoice>,
    /// Playing mix-ins in mixing order.
    buses: Vec<JobBus>,
    /// Every voice's frames, interleaved stereo, back to back.
    pcm: Vec<f32>,
}

impl MixJob {
    /// Frames the job mixes.
    pub fn frames(&self) -> usize {
        self.frames
    }

    /// The job was prepared for a side output ([`Synth::mix`] must be given
    /// one).
    pub fn wants_side(&self) -> bool {
        self.side
    }

    fn push_voice(
        &mut self,
        key: Key,
        params: VoiceParams,
        channels: usize,
        c0: u64,
        fx: FxState,
        frames: &[f32],
    ) {
        let start = self.pcm.len();
        self.pcm.extend_from_slice(frames);
        self.voices.push(JobVoice {
            key,
            params,
            channels,
            c0,
            fx,
            pcm: start..self.pcm.len(),
        });
    }
}

impl Mixer {
    /// Whether [`prepare_job`](Self::prepare_job) can stand in for a render:
    /// with no taps always; with taps, when every voice that reaches a tapped
    /// bin reaches nothing else (see the module docs), so the state half can
    /// fill the taps by itself.
    pub fn can_defer(&self) -> bool {
        if self.taps.is_empty() {
            return true;
        }
        let tapped = |bin: u32| self.taps.iter().any(|t| t.bin == bin);
        !self
            .mix_ins
            .values()
            .any(|m| tapped(m.input_bin()) || self.tap_use(m.params()) != TapUse::Not)
            && self
                .buffers
                .values()
                .filter(|b| b.is_playing())
                .all(|b| self.tap_use(b.params()) != TapUse::Mixed)
            && self
                .streams
                .values()
                .filter(|s| s.is_audible())
                .all(|s| self.tap_use(s.params()) != TapUse::Mixed)
    }

    /// How `p`'s mix bins meet the tapped bins.
    fn tap_use(&self, p: &VoiceParams) -> TapUse {
        let Some(bins) = &p.mix_bins else {
            return TapUse::Not;
        };
        let (mut tapped, mut other) = (false, false);
        for &(bin, _) in bins {
            match route::dest(bin) {
                Dest::Dropped => {}
                Dest::Send(b) if self.taps.iter().any(|t| t.bin == b) => tapped = true,
                _ => other = true,
            }
        }
        match (tapped, other) {
            (false, _) => TapUse::Not,
            (true, false) if p.output == VoiceOutput::Main => TapUse::Only,
            _ => TapUse::Mixed,
        }
    }

    /// The state half of [`render_split`](Self::render_split) for `frames`
    /// frames: advances every voice and the clock exactly as a render does,
    /// and returns what the sample half needs. `side` says a side output will
    /// be mixed. Call only when [`can_defer`](Self::can_defer).
    pub fn prepare_job(&mut self, frames: usize, side: bool) -> MixJob {
        debug_assert!(self.can_defer(), "a voice on a tap and elsewhere mixes inline");
        let rate = self.sample_rate;
        let mut scratch = std::mem::take(&mut self.scratch);
        scratch.resize(frames * 2, 0.0);
        let order = self.mix_in_order();
        let mut job = MixJob {
            frames,
            rate,
            master: mb_to_gain(self.master_mb),
            hrtf: self.hrtf,
            distance_reverb: self.distance_reverb,
            listener: self.listener.clone(),
            environment: self.reverb().copied(),
            side,
            voices: Vec::new(),
            buses: Vec::with_capacity(order.len()),
            pcm: Vec::new(),
        };
        let master = job.master;
        let environment = job.environment;
        let mut taps = std::mem::take(&mut self.taps);
        begin_taps(&mut taps, frames, rate as usize);
        let mut direct = std::mem::take(&mut self.direct);
        direct.resize(frames * 2, 0.0);
        let ids: Vec<BufferId> = self
            .buffers
            .iter()
            .filter(|(_, b)| b.is_playing())
            .map(|(&id, _)| id)
            .collect();
        for id in ids {
            scratch.fill(0.0);
            let tap_only = self.tap_use(self.buffers[&id].params()) == TapUse::Only;
            let b = self.buffers.get_mut(&id).expect("listed above");
            let params = b.params().clone();
            b.set_doppler_pitch(
                params
                    .spatial
                    .as_ref()
                    .map_or(1.0, |s| self.listener.spatial(s).pitch),
            );
            let c0 = b.fx_clock();
            b.render(&mut scratch, rate, (1.0, 1.0));
            let produced = (b.fx_clock() - c0) as usize;
            let channels = b.format().channels() as usize;
            if tap_only {
                b.apply_fx(&mut scratch[..produced * 2], c0, rate);
                let mut cx = Route {
                    listener: &self.listener,
                    direct: &mut direct,
                    room: &mut [],
                    distance_reverb: self.distance_reverb,
                    environment,
                    rate,
                    master,
                    buses: &mut [],
                    taps: &mut taps,
                    frames,
                    reverb_in: None,
                };
                let src = &scratch[..produced * 2];
                route_voice(&mut cx, &params, channels, src, &mut b.fx_state, &mut [], None);
                continue;
            }
            let fx = b.fx_state.clone();
            job.push_voice(
                Key::Buffer(id),
                params,
                channels,
                c0,
                fx,
                &scratch[..produced * 2],
            );
        }
        let ids: Vec<StreamId> = self
            .streams
            .iter()
            .filter(|(_, s)| s.is_audible())
            .map(|(&id, _)| id)
            .collect();
        let window = STARVE_WINDOW_MS * u64::from(rate) / 1000;
        for id in ids {
            scratch.fill(0.0);
            let tap_only = self.tap_use(self.streams[&id].params()) == TapUse::Only;
            let s = self.streams.get_mut(&id).expect("listed above");
            if let Some(at) = s.dry_at.take() {
                let gap = self.clock.saturating_sub(at);
                if gap > 0 && gap <= window {
                    self.health.starves += 1;
                    self.health.starve_frames += gap;
                }
            }
            let params = s.params().clone();
            s.set_doppler_pitch(
                params
                    .spatial
                    .as_ref()
                    .map_or(1.0, |sp| self.listener.spatial(sp).pitch),
            );
            let c0 = s.fx_clock();
            s.render(&mut scratch, rate, (1.0, 1.0));
            let produced = (s.fx_clock() - c0) as usize;
            let channels = s.format().channels() as usize;
            if s.pending() == 0 {
                // Ran out: dry from the first frame it did not fill.
                s.dry_at = Some(self.clock + produced as u64);
            }
            if tap_only {
                s.apply_fx(&mut scratch[..produced * 2], c0, rate);
                let mut cx = Route {
                    listener: &self.listener,
                    direct: &mut direct,
                    room: &mut [],
                    distance_reverb: self.distance_reverb,
                    environment,
                    rate,
                    master,
                    buses: &mut [],
                    taps: &mut taps,
                    frames,
                    reverb_in: None,
                };
                let src = &scratch[..produced * 2];
                if !route_voice(&mut cx, &params, channels, src, &mut s.fx_state, &mut [], None) {
                    self.health.unrouted_frames += produced as u64;
                }
                continue;
            }
            let fx = s.fx_state.clone();
            job.push_voice(
                Key::Stream(id),
                params,
                channels,
                c0,
                fx,
                &scratch[..produced * 2],
            );
        }
        for id in order {
            let mix = self.mix_ins.get_mut(&id).expect("listed above");
            job.buses.push(JobBus {
                id,
                bin: mix.input_bin(),
                params: mix.params().clone(),
                c0: mix.fx_clock(),
                fx: mix.fx_state.clone(),
            });
            mix.advance_clock(frames as u64);
        }
        self.taps = taps;
        self.direct = direct;
        self.scratch = scratch;
        self.clock += frames as u64;
        job
    }
}

/// How a voice's bins meet the tapped bins.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TapUse {
    /// None of them is tapped.
    Not,
    /// Tapped bins and bins nothing hears, on the main output.
    Only,
    /// Tapped bins and anything else (or the side output).
    Mixed,
}

/// A voice's filter state on the sample side.
struct Voice {
    fx: FxState,
    epoch: u32,
    /// [`Synth::clock`] when a job last carried it.
    seen: u64,
}

/// The sample half of a render: owns the memory only effects and routing
/// touch, and mixes [`MixJob`]s in the order they were prepared.
#[derive(Default)]
pub struct Synth {
    rate: u32,
    voices: BTreeMap<Key, Voice>,
    hrtf: BTreeMap<MixInId, Hrtf>,
    reverb: Option<Reverb>,
    scratch: Vec<f32>,
    reverb_in: Vec<f32>,
    direct: Vec<f32>,
    room: Vec<f32>,
    /// Frames mixed.
    clock: u64,
    unrouted_frames: u64,
}

impl Synth {
    pub fn new() -> Self {
        Self::default()
    }

    /// Frames of stream voices whose routing reached nothing, the sample
    /// half's share of [`crate::StreamHealth::unrouted_frames`].
    pub fn unrouted_frames(&self) -> u64 {
        self.unrouted_frames
    }

    /// The filter state `key` carries into this block, with the clock, envelope,
    /// Doppler and phase the state half set. True when the voice restarted
    /// since the last block (its memory is cleared).
    fn voice<'a>(
        voices: &'a mut BTreeMap<Key, Voice>,
        key: Key,
        fx: &FxState,
        now: u64,
    ) -> (&'a mut FxState, bool) {
        let v = voices.entry(key).or_insert_with(|| Voice {
            fx: FxState::default(),
            epoch: fx.epoch,
            seen: now,
        });
        let restarted = v.epoch != fx.epoch;
        if restarted {
            v.fx = FxState::default();
            v.epoch = fx.epoch;
        }
        v.fx.clock = fx.clock;
        v.fx.env_start = fx.env_start;
        v.fx.doppler = fx.doppler;
        v.fx.phase_off = fx.phase_off;
        v.seen = now;
        (&mut v.fx, restarted)
    }

    /// Mixes `job` into `out` (interleaved stereo, `job.frames() * 2` long,
    /// overwritten) and, when the job wants one, `side`, as
    /// [`Mixer::render_split`] does for the same state.
    pub fn mix(&mut self, job: MixJob, out: &mut [f32], mut side: Option<&mut [f32]>) {
        let MixJob {
            frames,
            rate,
            master,
            hrtf: hrtf_on,
            distance_reverb,
            listener,
            environment,
            side: _,
            voices: jvoices,
            buses: jbuses,
            mut pcm,
        } = job;
        assert_eq!(out.len(), frames * 2, "output must match the job");
        out.fill(0.0);
        if let Some(s) = side.as_deref_mut() {
            assert_eq!(s.len(), out.len(), "side output must match the main block");
            s.fill(0.0);
        }
        if self.rate != rate {
            self.rate = rate;
            self.hrtf.clear();
            self.reverb = None;
        }
        match environment {
            Some(e) => match self.reverb.as_mut() {
                Some(r) => {
                    if *r.params() != e {
                        r.configure(e);
                    }
                }
                None => self.reverb = Some(Reverb::new(rate, e)),
            },
            None => self.reverb = None,
        }
        if !hrtf_on {
            self.hrtf.clear();
        }
        let reverb_live = self
            .reverb
            .as_ref()
            .is_some_and(|r| !r.params().is_silent());
        let now = self.clock;
        let mut scratch = std::mem::take(&mut self.scratch);
        scratch.resize(frames * 2, 0.0);
        let mut rin = std::mem::take(&mut self.reverb_in);
        let mut direct = std::mem::take(&mut self.direct);
        direct.resize(frames * 2, 0.0);
        let mut room = std::mem::take(&mut self.room);
        if reverb_live {
            room.resize(frames * 2, 0.0);
            rin.clear();
            rin.resize(frames, 0.0);
        }
        let mut states = std::mem::take(&mut self.voices);
        let mut buses: Vec<Bus> = jbuses
            .iter()
            .map(|b| Bus {
                id: b.id,
                bin: b.bin,
                input: vec![0.0; frames],
                done: false,
            })
            .collect();

        for v in &jvoices {
            let src = &mut pcm[v.pcm.clone()];
            let (fx, _) = Self::voice(&mut states, v.key, &v.fx, now);
            fx.process(&v.params.fx, src, v.c0, rate);
            if !to_side(side.as_deref_mut(), &v.params, src, master) {
                let mut cx = Route {
                    listener: &listener,
                    direct: &mut direct,
                    room: &mut room,
                    distance_reverb,
                    environment,
                    rate,
                    master,
                    buses: &mut buses,
                    taps: &mut [],
                    frames,
                    reverb_in: reverb_live.then_some(&mut rin),
                };
                let reached = route_voice(&mut cx, &v.params, v.channels, src, fx, out, None);
                if !reached && matches!(v.key, Key::Stream(_)) {
                    self.unrouted_frames += (src.len() / 2) as u64;
                }
            }
        }
        for (i, jb) in jbuses.iter().enumerate() {
            buses[i].done = true;
            let input = std::mem::take(&mut buses[i].input);
            for (frame, v) in scratch.as_chunks_mut::<2>().0.iter_mut().zip(&input) {
                *frame = [*v, *v];
            }
            let (fx, restarted) = Self::voice(&mut states, Key::MixIn(jb.id), &jb.fx, now);
            fx.process(&jb.params.fx, &mut scratch, jb.c0, rate);
            let hrtf = if hrtf_on {
                let h = self.hrtf.entry(jb.id).or_insert_with(|| Hrtf::new(rate));
                if restarted {
                    h.reset();
                }
                Some(h)
            } else {
                None
            };
            let mut cx = Route {
                listener: &listener,
                direct: &mut direct,
                room: &mut room,
                distance_reverb,
                environment,
                rate,
                master,
                buses: &mut buses,
                taps: &mut [],
                frames,
                reverb_in: reverb_live.then_some(&mut rin),
            };
            route_voice(&mut cx, &jb.params, 1, &scratch, fx, out, hrtf);
        }
        if let (Some(r), true) = (self.reverb.as_mut(), reverb_live) {
            scratch.fill(0.0);
            r.process(&rin, &mut scratch);
            for (o, w) in out.iter_mut().zip(&scratch) {
                *o += w * master;
            }
        }
        self.clock += frames as u64;
        // Ids are never reused, so a voice nothing has carried for a while is
        // gone; its filter memory has decayed to nothing either way.
        if states.len() > 64 {
            let keep = u64::from(rate) * 2;
            states.retain(|_, v| now.saturating_sub(v.seen) < keep);
        }
        self.voices = states;
        self.scratch = scratch;
        self.reverb_in = rin;
        self.direct = direct;
        self.room = room;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsound::{MixIn, SoundBuffer, VoiceOutput};
    use crate::format::WaveFormat;

    const RATE: u32 = 8000;
    const BLOCK: usize = 64;

    fn sine(n: usize, hz: f64) -> Vec<u8> {
        (0..n)
            .flat_map(|i| {
                let v = (2.0 * std::f64::consts::PI * hz * i as f64 / f64::from(RATE)).sin();
                ((v * 12000.0) as i16).to_le_bytes()
            })
            .collect()
    }

    fn buffer(hz: f64) -> SoundBuffer {
        SoundBuffer::from_data(WaveFormat::pcm16(1, RATE).unwrap(), sine(900, hz)).unwrap()
    }

    /// A mixer with looping voices at different pans and volumes, one on
    /// the side, one into a mix-in, and a reverb.
    fn scene() -> (Mixer, Vec<BufferId>) {
        let mut m = Mixer::new(RATE);
        let mut bus = MixIn::new(0x17);
        bus.set_mix_bins(Some(vec![(0, 0), (1, -300)]));
        bus.play();
        m.add_mix_in(bus);
        m.set_reverb(Some(I3dl2Listener {
            room: -1000,
            room_hf: -100,
            room_rolloff: 0.0,
            decay_time: 1.49,
            decay_hf_ratio: 0.83,
            reflections: -2602,
            reflections_delay: 0.007,
            reverb: 200,
            reverb_delay: 0.011,
            diffusion: 100.0,
            density: 100.0,
            hf_reference: 5000.0,
        }));
        let mut ids = Vec::new();
        for (i, hz) in [220.0, 330.0, 440.0, 550.0].into_iter().enumerate() {
            let id = m.add_buffer(buffer(hz));
            let b = m.buffer_mut(id).unwrap();
            b.set_volume(-200 * i as i32);
            b.set_pan(1500 * i as i32 - 2000);
            match i {
                1 => b.set_output(VoiceOutput::Both),
                2 => b.set_mix_bins(Some(vec![(0x17, 0)])),
                3 => b.set_output(VoiceOutput::Side),
                _ => {}
            }
            b.play(true);
            ids.push(id);
        }
        (m, ids)
    }

    /// Frames the guest can see: where each voice is and whether it plays.
    fn guest_view(m: &Mixer, ids: &[BufferId]) -> Vec<(usize, bool)> {
        ids.iter()
            .map(|&id| {
                let b = m.buffer(id).unwrap();
                (b.current_position(), b.is_playing())
            })
            .collect()
    }

    #[test]
    fn deferred_mix_equals_the_inline_render() {
        let (mut inline, ids) = scene();
        let (mut split, _) = scene();
        let mut synth = Synth::new();
        for block in 0..40 {
            if block == 10 {
                // A guest write between blocks reaches both the same way.
                for m in [&mut inline, &mut split] {
                    m.buffer_mut(ids[0]).unwrap().set_volume(-900);
                    m.buffer_mut(ids[1]).unwrap().stop();
                    m.buffer_mut(ids[1]).unwrap().set_current_position(0);
                }
            }
            if block == 20 {
                for m in [&mut inline, &mut split] {
                    m.buffer_mut(ids[1]).unwrap().play(true);
                }
            }
            let (mut a, mut a_side) = (vec![9.0; BLOCK * 2], vec![9.0; BLOCK * 2]);
            inline.render_split(&mut a, Some(&mut a_side));
            let job = split.prepare_job(BLOCK, true);
            assert_eq!(job.frames(), BLOCK);
            // The guest's view is already final: nothing waits on the mix.
            assert_eq!(guest_view(&inline, &ids), guest_view(&split, &ids));
            let (mut b, mut b_side) = (vec![7.0; BLOCK * 2], vec![7.0; BLOCK * 2]);
            synth.mix(job, &mut b, Some(&mut b_side));
            assert_eq!(a, b, "main, block {block}");
            assert_eq!(a_side, b_side, "side, block {block}");
        }
    }

    #[test]
    fn a_position_read_after_a_write_needs_no_mix() {
        let mut m = Mixer::new(RATE);
        let id = m.add_buffer(buffer(300.0));
        m.buffer_mut(id).unwrap().play(true);
        let first = m.prepare_job(BLOCK, false);
        assert_eq!(m.buffer(id).unwrap().current_position(), BLOCK * 2);
        // Seek, then read: the write is visible at once, and the next
        // block starts from it.
        m.buffer_mut(id).unwrap().set_current_position(200);
        assert_eq!(m.buffer(id).unwrap().current_position(), 200);
        let second = m.prepare_job(BLOCK, false);
        assert_eq!(m.buffer(id).unwrap().current_position(), 200 + BLOCK * 2);
        // Mixed later, in order, the blocks carry the frames read at the time.
        let mut synth = Synth::new();
        let (mut a, mut b) = (vec![0.0; BLOCK * 2], vec![0.0; BLOCK * 2]);
        synth.mix(first, &mut a, None);
        synth.mix(second, &mut b, None);
        let mut reference = Mixer::new(RATE);
        let rid = reference.add_buffer(buffer(300.0));
        reference.buffer_mut(rid).unwrap().play(true);
        let (mut ra, mut rb) = (vec![0.0; BLOCK * 2], vec![0.0; BLOCK * 2]);
        reference.render(&mut ra);
        reference.buffer_mut(rid).unwrap().set_current_position(200);
        reference.render(&mut rb);
        assert_eq!((a, b), (ra, rb));
    }

    #[test]
    fn a_restarted_voice_starts_its_filter_over() {
        // The same scene with a stop and a restart from the start, mixed
        // through one Synth, matches the inline render.
        let (mut inline, ids) = scene();
        let (mut split, _) = scene();
        let mut synth = Synth::new();
        for block in 0..12 {
            if block == 5 {
                for m in [&mut inline, &mut split] {
                    let b = m.buffer_mut(ids[0]).unwrap();
                    b.stop();
                    b.set_current_position(0);
                    b.play(true);
                }
            }
            let mut a = vec![0.0; BLOCK * 2];
            inline.render(&mut a);
            let job = split.prepare_job(BLOCK, false);
            let mut b = vec![0.0; BLOCK * 2];
            synth.mix(job, &mut b, None);
            assert_eq!(a, b, "block {block}");
        }
    }

    /// The scene plus the XHV voice rings' shape: two looping voices whose
    /// only bin is a tapped FX send (one a stream, one a buffer), at their
    /// own volumes.
    fn tapped_scene() -> Mixer {
        let (mut m, _) = scene();
        m.set_taps(&[0x1b, 0x1c]);
        let id = m.add_buffer(buffer(260.0));
        let b = m.buffer_mut(id).unwrap();
        b.set_mix_bins(Some(vec![(0x1b, -300), (3, 0)]));
        b.play(true);
        let sid = m.add_stream(crate::dsound::Stream::new(WaveFormat::pcm16(1, RATE).unwrap()).unwrap());
        m.stream_mut(sid)
            .unwrap()
            .set_mix_bins(Some(vec![(0x1c, -602)]));
        m.stream_mut(sid).unwrap().submit(&sine(BLOCK * 4, 600.0)).unwrap();
        m
    }

    #[test]
    fn a_tap_only_voice_is_tapped_in_the_state_half() {
        let mut inline = tapped_scene();
        let mut split = tapped_scene();
        assert!(split.can_defer(), "voices that reach only tapped bins defer");
        let mut synth = Synth::new();
        for block in 0..8 {
            let mut a = vec![9.0; BLOCK * 2];
            inline.render(&mut a);
            let job = split.prepare_job(BLOCK, false);
            // The ring's input is final before the sample half runs.
            for bin in [0x1b, 0x1c] {
                assert_eq!(inline.take_tap(bin), split.take_tap(bin), "tap {bin:#x}, block {block}");
            }
            let mut b = vec![7.0; BLOCK * 2];
            synth.mix(job, &mut b, None);
            assert_eq!(a, b, "main, block {block}");
        }
        assert_eq!(
            inline.stream_health().unrouted_frames,
            split.stream_health().unrouted_frames
        );
    }

    #[test]
    fn a_voice_on_a_tap_and_elsewhere_keeps_a_mixer_inline() {
        let mut m = Mixer::new(RATE);
        m.set_taps(&[0x20]);
        assert!(m.can_defer(), "no voice, nothing to fill");
        let id = m.add_buffer(buffer(300.0));
        m.buffer_mut(id).unwrap().set_mix_bins(Some(vec![(0x20, 0)]));
        m.buffer_mut(id).unwrap().play(true);
        assert!(m.can_defer());
        m.buffer_mut(id)
            .unwrap()
            .set_mix_bins(Some(vec![(0x20, 0), (0, 0)]));
        assert!(!m.can_defer(), "also on the speakers");
        m.buffer_mut(id).unwrap().set_mix_bins(Some(vec![(0x20, 0)]));
        m.buffer_mut(id).unwrap().set_output(VoiceOutput::Both);
        assert!(!m.can_defer(), "also on the side output");
        m.buffer_mut(id).unwrap().set_output(VoiceOutput::Main);
        let mut bus = MixIn::new(0x20);
        bus.play();
        m.add_mix_in(bus);
        assert!(!m.can_defer(), "a mix-in reads the tapped bin");
    }
}
