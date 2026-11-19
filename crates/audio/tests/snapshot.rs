//! The `audio` snapshot section (docs/runtime/snapshots.md, phase 4): a
//! mixer with every kind of voice and effect renders, is saved and renders
//! more; a fresh mixer loads the save and renders the same PCM, bit for
//! bit.
use audio::dsound::{MixIn, Source3d, StreamId, Vec3};
use audio::fx::{EnvSpec, FilterKind, FilterSpec, I3dl2Source, LfoSpec, Stage};
use audio::mixer::MixInId;
use audio::snap::SnapshotError;
use audio::{I3dl2Listener, Mixer, SoundBuffer, Stream, WaveFormat};
use snapshot::{BuildKey, Header, Snapshot, SnapshotWriter};

const RATE: u32 = 24_000;
/// Frames per render call.
const BLOCK: usize = 240;
/// The send bin the stream feeds the mix-in through, and the tapped one.
const SEND: u32 = 11;
const TAPPED: u32 = 12;

fn key() -> BuildKey {
    BuildKey {
        stamp: "audio-test".into(),
        switches: String::new(),
    }
}

fn save(m: &Mixer) -> Vec<u8> {
    let mut out = SnapshotWriter::new(Header {
        build: key(),
        name: "audio".into(),
        tick: 0,
        presents: 0,
        notes: Vec::new(),
    });
    m.save_snapshot(&mut out).unwrap();
    out.finish().unwrap()
}

/// `frames` frames of 16-bit PCM, `channels` interleaved, from `f`.
fn pcm(frames: usize, channels: usize, f: impl Fn(usize) -> f32) -> Vec<u8> {
    (0..frames * channels)
        .flat_map(|i| ((f(i) * 20_000.0) as i16).to_le_bytes())
        .collect()
}

fn reverb_env() -> I3dl2Listener {
    I3dl2Listener {
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
    }
}

/// Buffers (looping, filtered, enveloped, occluded, through mix bins and
/// the reverb; and a plain 3D one with Doppler), a stream with queued
/// packets and a pitch LFO sending into a 3D mix-in and a tapped bin, and
/// the reverb on. Returns the mixer and the stream and mix-in ids.
fn scene() -> (Mixer, StreamId, MixInId) {
    let mut m = Mixer::new(RATE);
    m.set_reverb(Some(reverb_env()));
    m.set_master_volume(-200);
    m.set_taps(&[TAPPED]);
    let l = m.listener_mut();
    l.position = Vec3::new(0.5, 0.0, 0.0);
    l.velocity = Vec3::new(3.0, 0.0, 1.0);

    let mono = WaveFormat::pcm16(1, 22_050).unwrap();
    let data = pcm(1000, 1, |i| (i as f32 * 0.07).sin());
    let a = m.add_buffer(SoundBuffer::from_data(mono, data).unwrap());
    let b = m.buffer_mut(a).unwrap();
    b.set_loop_region(100, 700).unwrap();
    b.set_filter(FilterSpec {
        stages: [
            Some(Stage {
                kind: FilterKind::LowPass,
                hz: 2000.0,
                q: 0.7071,
                gain_db: 0.0,
            }),
            Some(Stage {
                kind: FilterKind::Peaking,
                hz: 500.0,
                q: 1.0,
                gain_db: 4.0,
            }),
        ],
    });
    b.set_envelope(Some(EnvSpec {
        start_stage: 1,
        delay: 0,
        attack: 10,
        hold: 5,
        decay: 20,
        release: 0,
        sustain: 128,
    }));
    b.set_lfo(
        0,
        Some(LfoSpec {
            delay_s: 0.0,
            rate_hz: 4.0,
            pitch_oct: 0.0,
            filter_oct: 1.0,
            amp_db: 3.0,
        }),
    );
    b.set_i3dl2_source(Some(I3dl2Source {
        room_hf: -900,
        obstruction_hf: -1500,
        obstruction_lf_ratio: 0.2,
        ..I3dl2Source::default()
    }));
    b.set_mix_bins(Some(vec![(0, 0), (1, -300), (10, -600)]));
    b.play(true);

    let stereo = WaveFormat::pcm16(2, RATE).unwrap();
    let data = pcm(4000, 2, |i| ((i % 97) as f32 / 97.0) - 0.5);
    let c = m.add_buffer(SoundBuffer::from_data(stereo, data).unwrap());
    let b = m.buffer_mut(c).unwrap();
    b.set_source_3d(Some(Source3d {
        position: Vec3::new(4.0, 0.0, 2.0),
        velocity: Vec3::new(-20.0, 0.0, 0.0),
        ..Source3d::default()
    }));
    b.set_volume(-400);
    b.play(false);

    let mix = m.add_mix_in(MixIn::new(SEND));
    let x = m.mix_in_mut(mix).unwrap();
    x.set_mix_bins(Some(vec![(6, 0), (7, 0), (8, 0), (9, 0), (10, -300)]));
    x.set_source_3d(Some(Source3d {
        position: Vec3::new(-1.0, 0.0, 3.0),
        ..Source3d::default()
    }));
    x.set_filter(FilterSpec {
        stages: [
            Some(Stage {
                kind: FilterKind::HighPass,
                hz: 200.0,
                q: 0.7071,
                gain_db: 0.0,
            }),
            None,
        ],
    });
    x.play();

    let s = m.add_stream(Stream::new(WaveFormat::pcm16(1, 32_000).unwrap()).unwrap());
    let st = m.stream_mut(s).unwrap();
    st.set_lfo(
        1,
        Some(LfoSpec {
            delay_s: 0.01,
            rate_hz: 3.0,
            pitch_oct: 0.05,
            filter_oct: 0.0,
            amp_db: 0.0,
        }),
    );
    st.set_mix_bins(Some(vec![(SEND, 0), (0, -600), (TAPPED, -300)]));
    for p in 0..6 {
        st.submit(&pcm(300, 1, |i| ((i + 300 * p) as f32 * 0.013).cos()))
            .unwrap();
    }
    (m, s, mix)
}

fn render(m: &mut Mixer, blocks: usize) -> Vec<f32> {
    let mut out = vec![0.0; blocks * BLOCK * 2];
    for chunk in out.chunks_mut(BLOCK * 2) {
        m.render(chunk);
    }
    out
}

fn bits(v: &[f32]) -> Vec<u32> {
    v.iter().map(|s| s.to_bits()).collect()
}

/// More of the same after a load: render, feed the stream, render on.
fn carry_on(m: &mut Mixer, s: StreamId) -> (Vec<u32>, Vec<u32>, usize) {
    let mut out = render(m, 4);
    let st = m.stream_mut(s).unwrap();
    st.submit(&pcm(500, 1, |i| (i as f32 * 0.02).sin()))
        .unwrap();
    let completed = st.take_completed().len();
    out.extend(render(m, 12));
    (bits(&out), bits(&m.take_tap(TAPPED)), completed)
}

#[test]
fn a_loaded_mixer_renders_the_saved_ones_pcm_bit_for_bit() {
    let (mut a, s, _) = scene();
    let before = render(&mut a, 3);
    assert!(before.iter().any(|v| *v != 0.0), "the scene is audible");
    let file = save(&a);
    let want = carry_on(&mut a, s);

    // A mixer with other voices of its own, replaced whole by the load.
    let mut b = Mixer::new(RATE);
    let mono = WaveFormat::pcm16(1, RATE).unwrap();
    let other = b.add_buffer(SoundBuffer::from_data(mono, pcm(64, 1, |_| 0.5)).unwrap());
    b.buffer_mut(other).unwrap().play(true);
    render(&mut b, 2);
    b.load_snapshot(&Snapshot::open(&file, &key()).unwrap())
        .unwrap();
    assert_eq!(b.buffer_count(), 2);
    assert_eq!(b.stream_count(), 1);
    assert_eq!(b.mix_in_count(), 1);
    let got = carry_on(&mut b, s);
    assert_eq!(got.2, want.2, "packets completed");
    assert_eq!(got.1, want.1, "tapped samples");
    assert!(got.0 == want.0, "mixed PCM differs after the load");
    assert!(!want.1.is_empty());
}

#[test]
fn a_snapshot_loads_twice_into_one_mixer() {
    let (mut a, s, _) = scene();
    render(&mut a, 5);
    let file = save(&a);
    let want = carry_on(&mut a, s);
    let mut b = Mixer::new(RATE);
    for _ in 0..2 {
        b.load_snapshot(&Snapshot::open(&file, &key()).unwrap())
            .unwrap();
        assert_eq!(carry_on(&mut b, s), want);
    }
}

#[test]
fn a_mixer_at_another_rate_refuses_the_section_unchanged() {
    let (mut a, _, _) = scene();
    render(&mut a, 2);
    let file = save(&a);
    let mut b = Mixer::new(48_000);
    let err = b
        .load_snapshot(&Snapshot::open(&file, &key()).unwrap())
        .unwrap_err();
    assert!(matches!(err, SnapshotError::Mismatch(_)), "{err}");
    assert_eq!(b.buffer_count(), 0);
    assert_eq!(b.stream_count(), 0);
    assert!(b.reverb().is_none());
    assert!(render(&mut b, 1).iter().all(|v| *v == 0.0));
}

#[test]
fn headphone_filter_snapshot_keeps_history_and_an_in_progress_ramp() {
    for distance_reverb in [false, true] {
        let (mut source, stream, mix) = scene();
        source.set_hrtf(true);
        source.set_distance_reverb(distance_reverb);
        render(&mut source, 2);
        source.mix_in_mut(mix).unwrap().set_source_3d(Some(Source3d {
            position: Vec3::new(2.0, 1.0, -1.0),
            ..Source3d::default()
        }));
        source.render(&mut [0.0; 30]);
        let file = save(&source);
        let snapshot = Snapshot::open(&file, &key()).unwrap();
        let mut loaded = Mixer::new(RATE);
        assert!(matches!(
            loaded.load_snapshot(&snapshot),
            Err(SnapshotError::Mismatch(_))
        ));
        loaded.set_hrtf(true);
        if distance_reverb {
            assert!(matches!(
                loaded.load_snapshot(&snapshot),
                Err(SnapshotError::Mismatch(_))
            ));
            loaded.set_distance_reverb(true);
        }
        loaded.load_snapshot(&snapshot).unwrap();
        assert_eq!(carry_on(&mut loaded, stream), carry_on(&mut source, stream));
    }
}
