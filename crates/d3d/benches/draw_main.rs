//! Guest-thread (main-thread) microseconds per draw for a synthetic,
//! retail-free 2000-draw frame (`tests/draw_frame`), the way the game runs
//! the device: behind the render thread with the real shader judge
//! (`ThreadedBackend` over a backend judged by `Nv2aJudge`), and for
//! reference the device alone. Each runs with draw sharing on and off
//! (`Device::set_draw_sharing`, `H2_D3D_DRAW_SHARING`), alternating rounds
//! so a loaded machine slows both alike; the best round's mean and the
//! median frame are reported.
//!
//! `cargo bench -p d3d --bench draw_main`. Env: `H2_BENCH_FRAMES` (per
//! round, default 40), `H2_BENCH_DRAWS` (default 2000), `H2_BENCH_ROUNDS`
//! (default 5).

#[path = "../tests/common/mod.rs"]
mod common;
#[path = "../tests/draw_frame/mod.rs"]
mod draw_frame;

use std::time::Instant;

use d3d::{Backend, Device, SparseMemory, ThreadedBackend};
use draw_frame::{NullBackend, frame, setup};

/// Per-frame seconds for `frames` frames after a warm-up.
fn times<B: Backend>(d: &mut Device<B>, m: &mut SparseMemory, frames: u32, draws: u32) -> Vec<f64> {
    for f in 0..3 {
        frame(d, m, draws, f);
    }
    (0..frames)
        .map(|f| {
            let t = Instant::now();
            frame(d, m, draws, f);
            t.elapsed().as_secs_f64()
        })
        .collect()
}

#[derive(Default)]
struct Best {
    mean: f64,
    median: f64,
}

impl Best {
    fn add(&mut self, mut t: Vec<f64>) {
        let mean = t.iter().sum::<f64>() / t.len() as f64;
        t.sort_by(f64::total_cmp);
        let median = t[t.len() / 2];
        if self.mean == 0.0 || mean < self.mean {
            self.mean = mean;
        }
        if self.median == 0.0 || median < self.median {
            self.median = median;
        }
    }
}

fn report(name: &str, draws: u32, off: &Best, on: &Best) {
    let us = |s: f64| s * 1e6 / draws as f64;
    println!(
        "{name}: sharing off {:.3} us/draw (median frame {:.3} ms), on {:.3} us/draw (median frame {:.3} ms): {:+.1}%",
        us(off.mean),
        off.median * 1e3,
        us(on.mean),
        on.median * 1e3,
        100.0 * (on.mean / off.mean - 1.0),
    );
}

fn main() {
    let env = |k, v: u32| {
        std::env::var(k)
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(v)
    };
    let frames = env("H2_BENCH_FRAMES", 40);
    let draws = env("H2_BENCH_DRAWS", 2000);
    let rounds = env("H2_BENCH_ROUNDS", 5);

    let (mut off, mut on) = (Best::default(), Best::default());
    for _ in 0..rounds {
        for sharing in [false, true] {
            let (mut d, mut m) = setup(
                ThreadedBackend::new(NullBackend::judged()).unwrap(),
                sharing,
            );
            let t = times(&mut d, &mut m, frames, draws);
            assert_eq!(d.backend.misses(), 0);
            if sharing { on.add(t) } else { off.add(t) }
        }
    }
    report(
        "render thread + shader judge (guest side, as in the game)",
        draws,
        &off,
        &on,
    );

    let (mut off, mut on) = (Best::default(), Best::default());
    for _ in 0..rounds {
        for sharing in [false, true] {
            let (mut d, mut m) = setup(NullBackend::default(), sharing);
            let t = times(&mut d, &mut m, frames, draws);
            if sharing { on.add(t) } else { off.add(t) }
        }
    }
    report(
        "device alone (no render thread, no judge)",
        draws,
        &off,
        &on,
    );
}
