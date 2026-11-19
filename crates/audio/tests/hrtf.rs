use std::f32::consts::TAU;

use audio::dsound::{MixIn, Mode3d, Source3d, Vec3, mb_to_gain};
use audio::mixer::MixInId;
use audio::route;
use audio::{Mixer, Stream, WaveFormat};

const RATE: u32 = 48_000;
const BIN: u32 = 11;
const FRONT: Vec3 = Vec3::new(1.0, 0.0, 0.0);
const LEFT: Vec3 = Vec3::new(0.0, 0.0, 1.0);
const RIGHT: Vec3 = Vec3::new(0.0, 0.0, -1.0);
const REAR: Vec3 = Vec3::new(-1.0, 0.0, 0.0);
const UP: Vec3 = Vec3::new(0.0, 1.0, 0.0);
const DOWN: Vec3 = Vec3::new(0.0, -1.0, 0.0);

fn source(position: Vec3) -> Source3d {
    Source3d {
        position,
        rolloff_factor: 0.0,
        ..Source3d::default()
    }
}

fn scene(
    enabled: bool,
    position: Option<Source3d>,
    send_mb: i32,
    samples: impl IntoIterator<Item = i16>,
) -> (Mixer, MixInId) {
    let mut mixer = Mixer::new(RATE);
    mixer.set_hrtf(enabled);
    mixer.listener_mut().front = FRONT;
    let mut mix = MixIn::new(BIN);
    mix.set_source_3d(position);
    mix.set_mix_bins(Some(vec![
        (route::XTLK_FRONT_LEFT, 0),
        (route::XTLK_BACK_LEFT, 0),
        (route::XTLK_FRONT_RIGHT, 0),
        (route::XTLK_BACK_RIGHT, 0),
        (route::I3DL2, 0),
    ]));
    mix.play();
    let mix_id = mixer.add_mix_in(mix);
    let stream_id = mixer.add_stream(Stream::new(WaveFormat::pcm16(1, RATE).unwrap()).unwrap());
    let stream = mixer.stream_mut(stream_id).unwrap();
    stream.set_mix_bins(Some(vec![(BIN, send_mb)]));
    let pcm: Vec<u8> = samples.into_iter().flat_map(i16::to_le_bytes).collect();
    stream.submit(&pcm).unwrap();
    (mixer, mix_id)
}

fn tone(hz: f32, frames: usize) -> impl Iterator<Item = i16> {
    (0..frames).map(move |i| ((TAU * hz * i as f32 / RATE as f32).sin() * 8192.0) as i16)
}

fn render(mixer: &mut Mixer, frames: usize) -> Vec<f32> {
    let mut out = vec![0.0; 2 * frames];
    mixer.render(&mut out);
    out
}

fn hf_energy(position: Vec3, hz: f32) -> [f32; 2] {
    let (mut mixer, _) = scene(true, Some(source(position)), 0, tone(hz, 1024));
    let out = render(&mut mixer, 1024);
    out.as_chunks::<2>().0[128..]
        .iter()
        .fold([0.0; 2], |[left, right], &[l, r]| {
            [left + l * l, right + r * r]
        })
}

#[test]
fn lateral_source_has_a_near_ear_lead_and_more_near_ear_high_frequencies() {
    let impulse = std::iter::once(8192).chain(std::iter::repeat_n(0, 127));
    let (mut mixer, _) = scene(true, Some(source(LEFT)), 0, impulse);
    let out = render(&mut mixer, 128);
    let arrival = |ear: usize| {
        out.as_chunks::<2>()
            .0
            .iter()
            .position(|frame| frame[ear] != 0.0)
    };
    assert_eq!(arrival(0), Some(0));
    // 8.75 cm / 343 m/s × (π/2 + 1) ≈ 0.656 ms, or 31.48 samples.
    assert_eq!(arrival(1), Some(31));
    let [left, right] = hf_energy(LEFT, 8000.0);
    assert!(left > 2.0 * right && right > 0.0, "{left} {right}");
}

#[test]
fn rear_shelf_and_elevation_comb_change_high_frequency_energy() {
    let [front, _] = hf_energy(FRONT, 8000.0);
    let [back, _] = hf_energy(REAR, 8000.0);
    assert!(back > 0.0 && back < 0.65 * front, "{front} {back}");
    let [up, _] = hf_energy(UP, 9000.0);
    let [down, _] = hf_energy(DOWN, 9000.0);
    assert!(up > 0.0 && up < 0.6 * down, "{up} {down}");
}

#[test]
fn straight_ahead_matches_centre_pan_with_the_stream_send_gain() {
    let render_front = |enabled, send_mb| {
        let (mut mixer, _) = scene(enabled, Some(source(FRONT)), send_mb, tone(8000.0, 512));
        render(&mut mixer, 512)
    };
    let plain = render_front(false, -600);
    let binaural = render_front(true, -600);
    let unity = render_front(true, 0);
    assert!(plain.iter().any(|s| s.abs() > 0.1));
    assert!(binaural.as_chunks::<2>().0.iter().all(|f| f[0] == f[1]));
    assert_eq!(binaural, plain);
    assert!(
        binaural
            .iter()
            .zip(unity)
            .all(|(a, b)| (*a - b * mb_to_gain(-600)).abs() < 1e-6)
    );
}

#[test]
fn disabling_hrtf_restores_the_original_pan_bit_for_bit() {
    let (mut plain, _) = scene(false, Some(source(LEFT)), -600, [8192; 128]);
    let (mut toggled, _) = scene(true, Some(source(LEFT)), -600, [8192; 128]);
    let spatial = plain.listener().spatial(&source(LEFT));
    let input = 0.25 * mb_to_gain(-600);
    let expected = [input * spatial.left, input * spatial.right];
    let before = render(&mut plain, 64);
    assert!(before.as_chunks::<2>().0.iter().all(|f| *f == expected));
    assert_ne!(render(&mut toggled, 64), before);
    toggled.set_hrtf(false);
    let off = render(&mut toggled, 64);
    assert_eq!(
        off.iter().map(|s| s.to_bits()).collect::<Vec<_>>(),
        before.iter().map(|s| s.to_bits()).collect::<Vec<_>>()
    );
}

#[test]
fn unpositioned_and_3d_disabled_mix_ins_bypass_hrtf() {
    for position in [
        None,
        Some(Source3d {
            mode: Mode3d::Disabled,
            ..source(LEFT)
        }),
    ] {
        let (mut plain, _) = scene(false, position.clone(), 0, tone(8000.0, 512));
        let (mut enabled, _) = scene(true, position, 0, tone(8000.0, 512));
        assert_eq!(render(&mut enabled, 512), render(&mut plain, 512));
    }
}

#[test]
fn moving_sources_ramp_across_render_boundaries() {
    const BLOCK: usize = 240;
    let positions = [LEFT, RIGHT, DOWN, UP, REAR, FRONT];
    let run = |blocks_per_position| {
        let (mut mixer, mix) = scene(
            true,
            Some(source(LEFT)),
            0,
            tone(1000.0, BLOCK * positions.len()),
        );
        let mut out = Vec::new();
        for position in positions {
            mixer
                .mix_in_mut(mix)
                .unwrap()
                .set_source_3d(Some(source(position)));
            for _ in 0..blocks_per_position {
                out.extend(render(&mut mixer, BLOCK / blocks_per_position));
            }
        }
        out
    };
    let out = run(1);
    assert_eq!(out, run(4));
    // The unfiltered tone changes by up to 0.033 per frame. Once the signal
    // is running, allow for the near-ear boost and changing delay.
    for pair in out.as_chunks::<2>().0.windows(2).skip(BLOCK - 1) {
        for ear in 0..2 {
            let delta = (pair[1][ear] - pair[0][ear]).abs();
            assert!(delta < 0.075, "sample jump: {delta}");
        }
    }
}

#[test]
fn replaying_a_mix_in_clears_the_previous_sounds_delay_line() {
    let (mut mixer, mix) = scene(true, Some(source(LEFT)), 0, [8192]);
    let audible = render(&mut mixer, 1);
    assert!(audible[0] > 0.0 && audible[1] == 0.0);
    let bus = mixer.mix_in_mut(mix).unwrap();
    bus.stop();
    bus.play();
    assert!(render(&mut mixer, 128).iter().all(|s| *s == 0.0));
}

#[test]
fn unequal_front_and_back_bin_gains_follow_the_direction_ramp() {
    let (mut mixer, mix) = scene(true, Some(source(FRONT)), 0, [8192; 1024]);
    let bus = mixer.mix_in_mut(mix).unwrap();
    bus.set_mix_bins(Some(vec![
        (route::XTLK_FRONT_LEFT, 0),
        (route::XTLK_FRONT_RIGHT, 0),
    ]));
    assert_eq!(render(&mut mixer, 256).last(), Some(&0.25));
    mixer
        .mix_in_mut(mix)
        .unwrap()
        .set_source_3d(Some(source(REAR)));
    let out = render(&mut mixer, 512);
    assert!(out[0] > 0.24);
    assert_eq!(&out[2 * 479..], &[0.0; 2 * (512 - 479)]);
}
