//! Software mixer: sums active buffers and streams into a host buffer.
//!
//! The mixer is plain data and a [`render`](Mixer::render) call. It knows
//! nothing about devices or threads: the platform layer calls `render` from
//! its audio callback (usually behind a lock, see [`crate::output`]).

use std::collections::BTreeMap;

use crate::dsound::{
    BufferId, Listener, SoundBuffer, Stream, StreamId, VoiceParams, mb_to_gain,
};

/// Mixes voices to interleaved stereo `f32` at a fixed host rate.
#[derive(Debug)]
pub struct Mixer {
    sample_rate: u32,
    master_mb: i32,
    listener: Listener,
    buffers: BTreeMap<BufferId, SoundBuffer>,
    streams: BTreeMap<StreamId, Stream>,
    next_id: u32,
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
            next_id: 1,
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
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
    pub fn remove_buffer(&mut self, id: BufferId) -> Option<SoundBuffer> {
        self.buffers.remove(&id)
    }
    pub fn remove_stream(&mut self, id: StreamId) -> Option<Stream> {
        self.streams.remove(&id)
    }

    /// Number of buffers currently playing plus streams with queued data.
    pub fn active_voices(&self) -> usize {
        self.buffers.values().filter(|b| b.is_playing()).count()
            + self.streams.values().filter(|s| s.pending() > 0).count()
    }

    fn gains(&self, p: &VoiceParams) -> (f32, f32) {
        let mut p = p.clone();
        let mut spatial = 1.0;
        if let Some(src) = &p.spatial {
            let (g, pan) = self.listener.spatialize(src);
            spatial = g;
            p.pan_mb = (p.pan_mb + pan).clamp(-10_000, 10_000);
        }
        let m = mb_to_gain(self.master_mb) * spatial;
        let (l, r) = p.gains();
        (l * m, r * m)
    }

    /// Renders `out.len() / 2` stereo frames, overwriting `out`. The sum is
    /// not clipped; the output stage clamps.
    pub fn render(&mut self, out: &mut [f32]) {
        out.fill(0.0);
        let rate = self.sample_rate;
        let buffer_gains: Vec<_> = self.buffers.iter().map(|(&id, b)| (id, self.gains(b.params()))).collect();
        for (id, g) in buffer_gains {
            if let Some(b) = self.buffers.get_mut(&id) {
                b.render(out, rate, g);
            }
        }
        let stream_gains: Vec<_> = self.streams.iter().map(|(&id, s)| (id, self.gains(s.params()))).collect();
        for (id, g) in stream_gains {
            if let Some(s) = self.streams.get_mut(&id) {
                s.render(out, rate, g);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsound::{Source3d, Vec3};
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
