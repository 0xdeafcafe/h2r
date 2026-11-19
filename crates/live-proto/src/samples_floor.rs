//! Re-sample the same continuous movers and the original 15 Hz event log.
//! Never advance the RNG that chose hits, shots or reloads a second time.
use super::*;
use crate::game::FrameCarry;
use crate::pack::{
    self,
    floor::{distance, summary, wire},
};

fn movers() -> (Vec<Mover>, Vec<Mover>) {
    let mut rng = Rng(0x5eed_0016);
    let mut make = |n| (0..n).map(|_| Mover::new(&mut rng, 40.)).collect();
    (make(16), make(4))
}

fn sample(frames: &[Frame], players: &[Mover], rides: &[Mover], tick: usize) -> Frame {
    let mut f = frames[tick / 2].clone();
    if tick % 2 != 0 {
        let t = tick as f64 / 30.;
        f.tick += 1;
        // Non-random fields use the original analytic fixture at this tick.
        // Its random outputs are discarded below.
        let mut scratch = Rng(1);
        for (p, m) in f.players.iter_mut().zip(players) {
            *p = player(p.index, m, t, f.tick, &mut scratch);
        }
        for (v, m) in f.vehicles.iter_mut().zip(rides) {
            *v = vehicle(
                u32::from_str_radix(&v.id, 16).unwrap(),
                &v.kind,
                m,
                t,
                v.occupied,
            );
        }
        *f.vehicles.last_mut().unwrap() = turret(0x9008_0008, 0x9004_0004, t);
        for p in &mut f.projectiles {
            let id = u32::from_str_radix(&p.id, 16).unwrap();
            let shot = ((id - 0xa00c_0000) / 0x10001) as usize;
            let at = shot as f64 * 3.;
            let tau = t - at;
            let origin = players[shot % 16].at(at).0;
            let heading = shot as f64 * 2.3;
            let vel = [6. * heading.cos(), 6. * heading.sin(), 4.];
            p.pos = r3([
                origin[0] + vel[0] * tau,
                origin[1] + vel[1] * tau,
                origin[2] + 0.5 + vel[2] * tau - 0.5 * GRAVITY * tau * tau,
            ]);
            p.vel = Some(r3([vel[0], vel[1], vel[2] - GRAVITY * tau]));
        }
    }
    f.hits.clear();
    for p in &mut f.players {
        p.shots = None;
        p.fire = None;
        p.actions = None;
        p.actions_ago = None;
    }
    f
}

fn ten_hz(frames: &[Frame], players: &[Mover], rides: &[Mover]) -> Vec<Frame> {
    let mut out = Vec::new();
    let mut cursor = 0;
    let end = frames.len() * 2 - 1;
    let mut ticks: Vec<_> = (0..=end).step_by(3).collect();
    if ticks.last() != Some(&end) {
        ticks.push(end);
    } // deliver trailing one-shots
    for tick in ticks {
        let mut f = sample(frames, players, rides, tick);
        let mut carry = FrameCarry::default();
        while cursor < frames.len() && frames[cursor].tick <= f.tick {
            let src = &frames[cursor];
            // The fixture omits reload offsets: its observations are on the
            // source sample tick. Stamp these before moving them to a later tick.
            let mut timed = src.clone();
            for p in &mut timed.players {
                if let Some(a) = &p.actions {
                    p.actions_ago = Some(vec![0; a.len()]);
                }
            }
            carry.add(&timed);
            cursor += 1;
        }
        f.absorb(carry);
        out.push(f);
    }
    assert_eq!(cursor, frames.len());
    out
}
fn counts(frames: &[Frame]) -> [usize; 4] {
    let mut tally = [0; 4];
    for f in frames {
        tally[0] += f.hits.len();
        for p in &f.players {
            tally[1] += p.shots.unwrap_or(0) as usize;
            tally[2] += p.fire.unwrap_or(0).count_ones() as usize;
            tally[3] += p.actions.as_ref().map_or(0, Vec::len);
        }
    }
    tally
}
/// Mirror web/src/state/frames.ts hermite; all these players are alive,
/// below SNAP_DISTANCE, and carry both endpoint velocities. Hold at match end.
fn interpolation(frames: &[Frame], players: &[Mover], ticks: usize) -> (f64, f64) {
    let mut errors = Vec::with_capacity(ticks * 16);
    let mut left = 0;
    let poses: Vec<_> = frames
        .iter()
        .map(|f| {
            f.players
                .iter()
                .map(|p| {
                    (
                        p.pos.map(|v| (v * 1024.).round() / 1024.),
                        p.vel.unwrap().map(|v| (v * 128.).round() / 128.),
                    )
                })
                .collect::<Vec<_>>()
        })
        .collect();
    for tick in 0..ticks {
        let at = 9000 + tick as u64;
        while left + 1 < frames.len() && frames[left + 1].tick <= at {
            left += 1;
        }
        let right = (left + 1).min(frames.len() - 1);
        let dt = (frames[right].tick - frames[left].tick) as f64;
        let t = if dt == 0. {
            0.
        } else {
            (at - frames[left].tick) as f64 / dt
        };
        for (i, m) in players.iter().enumerate() {
            let (a, va) = poses[left][i];
            let (b, vb) = poses[right][i];
            let p = std::array::from_fn(|j| {
                let t2 = t * t;
                let t3 = t2 * t;
                (2. * t3 - 3. * t2 + 1.) * a[j]
                    + (t3 - 2. * t2 + t) * va[j] * dt / 30.
                    + (-2. * t3 + 3. * t2) * b[j]
                    + (t3 - t2) * vb[j] * dt / 30.
            });
            errors.push(distance(p, r3(m.at(tick as f64 / 30.).0)));
        }
    }
    summary(&mut errors)
}
pub(crate) fn bench_floor_cadence(frames: &[Frame], baseline: usize) {
    let (players, rides) = movers();
    let slow = ten_hz(frames, &players, &rides);
    assert_eq!(counts(frames), counts(&slow), "short events changed");
    let bytes = wire(&slow, 100);
    // Last 15 Hz sample is at tick 17,998. Compare only bracketed ticks
    // shared by both cadences, with no tail extrapolation.
    let ticks = frames.len() * 2 - 1;
    for (hz, f, bytes) in [(15, frames, baseline), (10, slow.as_slice(), bytes)] {
        let (p99, max) = interpolation(f, &players, ticks);
        println!(
            "{hz} Hz, 10 s keys: {} samples {bytes} bytes {:.3} B/player/s; p99 {p99:.8} max {max:.8} world units over {} player-ticks; events {:?}",
            f.len(),
            bytes as f64 / 9600.,
            ticks * 16,
            counts(f)
        );
    }
}

#[test]
fn resampling_keeps_events_and_uses_original_trajectory() {
    let frames = multiplayer(50);
    let (p, r) = movers();
    let slow = ten_hz(&frames, &p, &r);
    assert_eq!(counts(&frames), counts(&slow));
    for f in slow {
        assert_eq!(
            f.players[3].pos,
            r3(p[3].at((f.tick - 9000) as f64 / 30.).0)
        );
        pack::encode_keyframe(&f).unwrap();
    }
}
