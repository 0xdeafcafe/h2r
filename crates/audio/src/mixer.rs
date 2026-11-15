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

use std::collections::BTreeMap;

use crate::dsound::{
    BufferId, Listener, MixIn, SoundBuffer, Spatial, Stream, StreamId, VoiceParams, mb_to_gain,
};
use crate::route::{self, Dest};

/// Identifies a mix-in inside a [`Mixer`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MixInId(pub(crate) u32);

/// Mixes voices to interleaved stereo `f32` at a fixed host rate.
#[derive(Debug)]
pub struct Mixer {
    sample_rate: u32,
    master_mb: i32,
    listener: Listener,
    buffers: BTreeMap<BufferId, SoundBuffer>,
    streams: BTreeMap<StreamId, Stream>,
    mix_ins: BTreeMap<MixInId, MixIn>,
    next_id: u32,
    /// One voice rendered at unity gain, reused across renders.
    scratch: Vec<f32>,
}

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
        }
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
    /// not clipped; the output stage clamps.
    pub fn render(&mut self, out: &mut [f32]) {
        out.fill(0.0);
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

        let ids: Vec<BufferId> = self.buffers.iter().filter(|(_, b)| b.is_playing()).map(|(&id, _)| id).collect();
        for id in ids {
            scratch.fill(0.0);
            let b = self.buffers.get_mut(&id).expect("listed above");
            b.render(&mut scratch, rate, (1.0, 1.0));
            let channels = b.format().channels() as usize;
            let params = b.params().clone();
            route_voice(&self.listener, master, &params, channels, &scratch, out, &mut buses);
        }
        let ids: Vec<StreamId> = self.streams.iter().filter(|(_, s)| s.is_audible()).map(|(&id, _)| id).collect();
        for id in ids {
            scratch.fill(0.0);
            let s = self.streams.get_mut(&id).expect("listed above");
            s.render(&mut scratch, rate, (1.0, 1.0));
            let channels = s.format().channels() as usize;
            let params = s.params().clone();
            route_voice(&self.listener, master, &params, channels, &scratch, out, &mut buses);
        }
        for i in 0..buses.len() {
            buses[i].done = true;
            let input = std::mem::take(&mut buses[i].input);
            for (frame, v) in scratch.as_chunks_mut::<2>().0.iter_mut().zip(&input) {
                *frame = [*v, *v];
            }
            let params = self.mix_ins[&buses[i].id].params().clone();
            route_voice(&self.listener, master, &params, 1, &scratch, out, &mut buses);
        }
        self.scratch = scratch;
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

/// Mixes one voice, rendered at unity gain into `src` (interleaved stereo; a
/// mono voice is in both channels), into `out` and the mix-in `buses`.
fn route_voice(
    listener: &Listener,
    master: f32,
    p: &VoiceParams,
    channels: usize,
    src: &[f32],
    out: &mut [f32],
    buses: &mut [Bus],
) {
    let spatial = p.spatial.as_ref().map(|s| listener.spatial(s));
    let Some(bins) = &p.mix_bins else {
        // A plain DirectSound voice: volume and pan, then the 3D panning.
        let s = spatial.unwrap_or(Spatial::DRY);
        let (l, r) = p.gains();
        add(out, src, 0, 0, l * s.gain * s.left * master);
        add(out, src, 1, 1, r * s.gain * s.right * master);
        return;
    };
    let volume = mb_to_gain(p.volume_mb);
    for (i, &(bin, mb)) in bins.iter().enumerate() {
        let g = volume * mb_to_gain(mb);
        if g == 0.0 {
            continue;
        }
        let ch = route::channel_of(i, channels);
        match route::dest(bin) {
            Dest::Out { left, right } => {
                add(out, src, ch, 0, g * left * master);
                add(out, src, ch, 1, g * right * master);
            }
            Dest::Crosstalk { left, front } => {
                let w = match spatial {
                    Some(s) => s.gain * if left { s.left } else { s.right } * if front { s.front } else { 1.0 - s.front },
                    None => 1.0,
                };
                add(out, src, ch, if left { 0 } else { 1 }, g * w * master);
            }
            Dest::Send(bin) => {
                for bus in buses.iter_mut().filter(|b| b.bin == bin && !b.done) {
                    for (acc, frame) in bus.input.iter_mut().zip(src.as_chunks::<2>().0) {
                        *acc += frame[ch] * g;
                    }
                }
            }
            Dest::Dropped => {}
        }
    }
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
