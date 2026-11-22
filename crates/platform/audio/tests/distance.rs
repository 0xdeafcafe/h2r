//! The DirectSound 3D model, per voice against the listener
//! (docs/audio/audio.md, "Positional model"). Synthetic sources only.

use std::f32::consts::TAU;

use audio::dsound::{Listener, MixIn, Mode3d, Source3d, Vec3, mb_to_gain};
use audio::route;
use audio::{Mixer, Stream, WaveFormat};

const RATE: u32 = 48_000;

/// A source straight ahead of the default listener (front is +z).
fn ahead(distance: f32, min: f32, max: f32, rolloff: f32) -> Source3d {
    Source3d {
        position: Vec3::new(0.0, 0.0, distance),
        min_distance: min,
        max_distance: max,
        rolloff_factor: rolloff,
        ..Source3d::default()
    }
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() <= 1e-6 * b.abs().max(1.0)
}

#[test]
fn inverse_distance_with_min_max_and_rolloff() {
    let l = Listener::default();
    // Min distance 2, max distance 16: unity to min, then
    // min / (min + rolloff * (d - min)), held at max.
    let table: [(f32, f32, f32); 13] = [
        (1.0, 1.0, 1.0),    // inside min
        (2.0, 1.0, 1.0),    // at min
        (4.0, 1.0, 0.5),    // 2x min: 2 / (2 + 2)
        (16.0, 1.0, 0.125), // at max: 2 / (2 + 14)
        (64.0, 1.0, 0.125), // past max: held
        (4.0, 0.0, 1.0),    // rolloff 0: no attenuation
        (16.0, 0.0, 1.0),
        (64.0, 0.0, 1.0),
        (2.0, 2.0, 1.0),         // rolloff 2 at min
        (4.0, 2.0, 1.0 / 3.0),   // 2 / (2 + 2 * 2)
        (16.0, 2.0, 1.0 / 15.0), // 2 / (2 + 2 * 14)
        (64.0, 2.0, 1.0 / 15.0), // past max: held
        (3.0, 0.5, 2.0 / 2.5),   // a gentler rolloff
    ];
    for (d, rolloff, want) in table {
        let src = ahead(d, 2.0, 16.0, rolloff);
        let s = l.spatial(&src);
        assert!(
            close(s.gain, want),
            "d {d} rolloff {rolloff}: {} != {want}",
            s.gain
        );
        let a = l.attenuation(&src).unwrap();
        assert!(close(a.distance, d), "{d}: {}", a.distance);
        assert!(close(a.distance_gain, want));
        assert_eq!(a.cone_gain, 1.0);
        // Straight ahead: centred, all front.
        assert_eq!((s.left, s.right, s.front), (1.0, 1.0, 1.0));
    }
}

#[test]
fn the_listener_rolloff_multiplies_the_sources() {
    let mut l = Listener {
        rolloff_factor: 2.0,
        ..Listener::default()
    };
    assert!(close(
        l.spatial(&ahead(4.0, 2.0, 16.0, 1.0)).gain,
        1.0 / 3.0
    ));
    l.rolloff_factor = 0.0;
    assert_eq!(l.spatial(&ahead(4.0, 2.0, 16.0, 1.0)).gain, 1.0);
}

#[test]
fn halo_parameters_leave_the_distance_term_at_unity() {
    // Halo 2's submixes: FLT_MAX min and max distance, rolloff 0 (and the
    // listener's rolloff 0, `0x21e572`). The game's own gain carries distance.
    let l = Listener {
        rolloff_factor: 0.0,
        ..Listener::default()
    };
    for d in [0.5, 10.0, 1.0e6] {
        assert_eq!(l.spatial(&ahead(d, f32::MAX, f32::MAX, 0.0)).gain, 1.0);
    }
}

#[test]
fn cone_inside_between_and_outside() {
    let l = Listener::default();
    // The source is 4 units ahead; it sees the listener along -z.
    let coned = |orientation: Vec3| Source3d {
        cone_inside: 60,
        cone_outside: 120,
        cone_outside_volume_mb: -1200,
        cone_orientation: orientation,
        rolloff_factor: 0.0,
        ..ahead(4.0, 1.0, 100.0, 0.0)
    };
    let gain = |o| l.spatial(&coned(o)).gain;
    // Facing the listener: inside half the inner angle.
    assert_eq!(gain(Vec3::new(0.0, 0.0, -1.0)), 1.0);
    // Facing away: beyond half the outer angle, the outside volume.
    assert!(close(gain(Vec3::new(0.0, 0.0, 1.0)), mb_to_gain(-1200)));
    // 45 degrees off: halfway between 30 and 60, linear in millibels.
    let (s, c) = (45f32.to_radians().sin(), 45f32.to_radians().cos());
    let between = gain(Vec3::new(s, 0.0, -c));
    assert!((between / mb_to_gain(-600) - 1.0).abs() < 2e-3, "{between}");
    // The default 360 degree cone is no cone.
    let mut none = coned(Vec3::new(0.0, 0.0, 1.0));
    none.cone_inside = 360;
    none.cone_outside = 360;
    assert_eq!(l.spatial(&none).gain, 1.0);
    let a = l.attenuation(&coned(Vec3::new(0.0, 0.0, 1.0))).unwrap();
    assert!(close(a.cone_gain, mb_to_gain(-1200)));
    assert_eq!(a.distance_gain, 1.0);
}

#[test]
fn doppler_rises_approaching_and_falls_receding() {
    let moving = |v: f32| Source3d {
        velocity: Vec3::new(0.0, 0.0, v),
        ..ahead(10.0, 1.0, 100.0, 0.0)
    };
    let l = Listener::default();
    assert_eq!(l.spatial(&moving(0.0)).pitch, 1.0);
    // The source is ahead (+z): -z velocity approaches.
    assert!(l.spatial(&moving(-10.0)).pitch > 1.0);
    assert!(l.spatial(&moving(10.0)).pitch < 1.0);
    // A listener moving toward a resting source also raises it.
    let toward = Listener {
        velocity: Vec3::new(0.0, 0.0, 10.0),
        ..Listener::default()
    };
    assert!(toward.spatial(&moving(0.0)).pitch > 1.0);
    // Doppler factor 0 on either side turns it off.
    let off = Source3d {
        doppler_factor: 0.0,
        ..moving(-10.0)
    };
    assert_eq!(l.spatial(&off).pitch, 1.0);
}

#[test]
fn stereo_pan_follows_the_side() {
    let l = Listener::default();
    let side = |x: f32| {
        l.spatial(&Source3d {
            position: Vec3::new(x, 0.0, 0.0),
            rolloff_factor: 0.0,
            ..Source3d::default()
        })
    };
    let right = side(5.0);
    assert!(right.right == 1.0 && right.left < 0.3, "{right:?}");
    let left = side(-5.0);
    assert!(close(left.left, right.right) && close(left.right, right.left));
    assert!(close(right.front, 0.5));
}

fn tone(frames: usize) -> Vec<u8> {
    (0..frames)
        .map(|i| ((TAU * 500.0 * i as f32 / RATE as f32).sin() * 8192.0) as i16)
        .flat_map(i16::to_le_bytes)
        .collect()
}

fn rms(out: &[f32]) -> [f32; 2] {
    let n = (out.len() / 2) as f32;
    let [l, r] = out
        .as_chunks::<2>()
        .0
        .iter()
        .fold([0.0; 2], |[l, r], f| [l + f[0] * f[0], r + f[1] * f[1]]);
    [(l / n).sqrt(), (r / n).sqrt()]
}

const XTLK: [(u32, i32); 4] = [
    (route::XTLK_FRONT_LEFT, 0),
    (route::XTLK_BACK_LEFT, 0),
    (route::XTLK_FRONT_RIGHT, 0),
    (route::XTLK_BACK_RIGHT, 0),
];

/// A stream sent into a positioned submix, rendered; returns the RMS of the
/// second half of a 2048-frame render.
fn through_submix(src: Source3d, hrtf: bool) -> [f32; 2] {
    let mut m = Mixer::new(RATE);
    m.set_hrtf(hrtf);
    let mut mix = MixIn::new(route::SUBMIX);
    mix.set_source_3d(Some(src));
    mix.set_mix_bins(Some(XTLK.to_vec()));
    mix.play();
    let id = m.add_submix(mix);
    let bin = m.mix_in(id).unwrap().input_bin();
    assert!(bin >= route::HOST_SUBMIX_FIRST);
    let s = m.add_stream(Stream::new(WaveFormat::pcm16(1, RATE).unwrap()).unwrap());
    let stream = m.stream_mut(s).unwrap();
    stream.set_mix_bins(Some(vec![(bin, 0)]));
    stream.submit(&tone(2048)).unwrap();
    let mut out = vec![0.0; 2 * 2048];
    m.render(&mut out);
    rms(&out[2048..])
}

#[test]
fn the_mixer_applies_the_distance_per_voice_with_and_without_hrtf() {
    for hrtf in [false, true] {
        let near = through_submix(ahead(2.0, 2.0, 16.0, 1.0), hrtf);
        let twice = through_submix(ahead(4.0, 2.0, 16.0, 1.0), hrtf);
        let past = through_submix(ahead(64.0, 2.0, 16.0, 1.0), hrtf);
        assert!(near[0] > 0.01, "{hrtf}: {near:?}");
        for ch in 0..2 {
            assert!(
                (twice[ch] / near[ch] - 0.5).abs() < 1e-3,
                "{hrtf}: {twice:?} {near:?}"
            );
            assert!(
                (past[ch] / near[ch] - 0.125).abs() < 1e-3,
                "{hrtf}: {past:?}"
            );
        }
    }
}

#[test]
fn a_submix_hears_only_the_voices_given_its_own_bin() {
    let mut m = Mixer::new(RATE);
    let mut ids = Vec::new();
    for x in [5.0, -5.0] {
        let mut mix = MixIn::new(route::SUBMIX);
        mix.set_source_3d(Some(Source3d {
            position: Vec3::new(x, 0.0, 0.0),
            rolloff_factor: 0.0,
            ..Source3d::default()
        }));
        mix.set_mix_bins(Some(XTLK.to_vec()));
        mix.play();
        ids.push(m.add_submix(mix));
    }
    let right_bin = m.mix_in(ids[0]).unwrap().input_bin();
    assert_ne!(right_bin, m.mix_in(ids[1]).unwrap().input_bin());
    let s = m.add_stream(Stream::new(WaveFormat::pcm16(1, RATE).unwrap()).unwrap());
    m.stream_mut(s)
        .unwrap()
        .set_mix_bins(Some(vec![(right_bin, 0)]));
    m.stream_mut(s).unwrap().submit(&tone(2048)).unwrap();
    let mut out = vec![0.0; 2 * 2048];
    m.render(&mut out);
    let [l, r] = rms(&out[2048..]);
    // Only the right-hand submix carries it; the left one would have made
    // the left channel as loud as the right.
    assert!(l < 0.3 * r, "{l} {r}");
    // The guest's shared bin reaches neither.
    assert_eq!(route::dest(route::SUBMIX), route::Dest::Dropped);
}

/// Renders a plain stream with the given listener.
fn two_d(listener: Listener, bins: Option<Vec<(u32, i32)>>) -> (Vec<f32>, bool) {
    let mut m = Mixer::new(RATE);
    *m.listener_mut() = listener;
    let s = m.add_stream(Stream::new(WaveFormat::pcm16(1, RATE).unwrap()).unwrap());
    let stream = m.stream_mut(s).unwrap();
    stream.set_mix_bins(bins);
    stream.set_volume(-600);
    stream.submit(&tone(1024)).unwrap();
    let unpositioned = m.voice_levels().iter().all(|v| v.spatial.is_none());
    let mut out = vec![0.0; 2 * 1024];
    m.render(&mut out);
    (out, unpositioned)
}

#[test]
fn a_2d_voice_is_unchanged_by_the_listener() {
    let moved = Listener {
        position: Vec3::new(100.0, -20.0, 3.0),
        front: Vec3::new(1.0, 0.0, 0.0),
        velocity: Vec3::new(0.0, 0.0, 30.0),
        rolloff_factor: 10.0,
        doppler_factor: 10.0,
        ..Listener::default()
    };
    for bins in [
        None,
        Some(vec![(route::FRONT_LEFT, 0), (route::FRONT_RIGHT, -300)]),
        Some(XTLK.to_vec()),
    ] {
        let (still, a) = two_d(Listener::default(), bins.clone());
        let (other, b) = two_d(moved.clone(), bins.clone());
        assert!(a && b, "a 2D voice has no 3D terms");
        assert_eq!(still, other, "{bins:?}");
        assert!(still.iter().any(|&v| v != 0.0));
    }
    // Disabled 3D is 2D too.
    let off = Source3d {
        mode: Mode3d::Disabled,
        ..ahead(50.0, 1.0, 100.0, 1.0)
    };
    assert_eq!(Listener::default().attenuation(&off), None);
    assert_eq!(Listener::default().spatial(&off).gain, 1.0);
}
