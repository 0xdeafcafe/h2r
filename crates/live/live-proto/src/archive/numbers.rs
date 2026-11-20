//! Each column chooses a constant, a finite difference, or a fitted
//! sinusoid plus *all* integer corrections. Only the encoder fits a curve.
use super::motion::sin;
use super::{Error, Reader, Result, put};
use std::f64::consts::FRAC_PI_2;

fn zz(x: i64) -> u64 {
    ((x << 1) ^ (x >> 63)) as u64
}
fn unzz(x: u64) -> i64 {
    (x >> 1) as i64 ^ -((x & 1) as i64)
}
fn cost(x: i64) -> u64 {
    let u = zz(x);
    if u == 0 { 1 } else { 4 + 2 * u.ilog2() as u64 }
}

#[derive(Clone, Copy)]
struct Wave {
    freq: i64,
    coef: [i64; 3],
}

impl Wave {
    fn at(self, i: usize) -> i64 {
        let w = i as f64 * (self.freq as f64 / 4294967296.0);
        ((self.coef[0] as f64
            + self.coef[1] as f64 * sin(w + FRAC_PI_2)
            + self.coef[2] as f64 * sin(w))
            / 128.0
            + 0.5)
            .floor() as i64
    }
}

// Solve mean + a*cos(w*t) + b*sin(w*t) by least squares. Near a
// singularity the finite-difference candidate is preferable.
fn fit(v: &[i64], w: f64, step: usize) -> Option<(f64, [f64; 3])> {
    let mut mat = [[0.; 4]; 3];
    for i in (0..v.len()).step_by(step) {
        let (s, c) = (w * i as f64).sin_cos();
        let b = [1., c, s];
        for j in 0..3 {
            for k in 0..3 {
                mat[j][k] += b[j] * b[k];
            }
            mat[j][3] += b[j] * v[i] as f64;
        }
    }
    for j in 0..3 {
        let d = mat[j][j];
        if d.abs() < 1e-8 {
            return None;
        }
        for a in &mut mat[j] {
            *a /= d;
        }
        let pivot = mat[j];
        for (k, row) in mat.iter_mut().enumerate() {
            if k == j {
                continue;
            }
            let b = row[j];
            for t in 0..4 {
                row[t] -= b * pivot[t];
            }
        }
    }
    let cs = [mat[0][3], mat[1][3], mat[2][3]];
    let mut err = 0.;
    for i in (0..v.len()).step_by(step) {
        let (s, c) = (w * i as f64).sin_cos();
        err += (v[i] as f64 - (cs[0] + cs[1] * c + cs[2] * s)).powi(2);
    }
    Some((err, cs))
}

fn wave(v: &[i64]) -> Option<Wave> {
    let n = v.len();
    if n < 64 || v.iter().any(|x| x.unsigned_abs() > 1 << 30) {
        return None;
    }
    let sample = (n / 96).max(1);
    let mut best = f64::INFINITY;
    let mut freq = 0.;
    for k in [2, 4, 8, 16, 32] {
        if 4 * k > n {
            break;
        }
        let mut xx = 0.;
        let mut xy = 0.;
        let mut xsum = 0.;
        let mut ysum = 0.;
        for i in k..n - k {
            let x = (v[i] - v[k]) as f64;
            let y = ((v[i - k] - v[0]) + (v[i + k] - v[2 * k])) as f64 * 0.5;
            xsum += x;
            ysum += y;
            xx += x * x;
            xy += x * y;
        }
        let count = (n - 2 * k) as f64;
        let co = (xy - xsum * ysum / count) / (xx - xsum * xsum / count);
        if co.abs() >= 1. || !co.is_finite() {
            continue;
        }
        let w = co.acos() / k as f64;
        if let Some((e, _)) = fit(v, w, sample) {
            if e < best {
                best = e;
                freq = w;
            }
        }
    }
    if !best.is_finite() {
        return None;
    }
    // Reject irregular tracks before refining a frequency. Near the noise
    // floor additional iterations would not affect the reconstructed values.
    if best / ((n / sample) as f64) > 256. {
        return None;
    }
    let mut delta = 0.0001;
    for _ in 0..8 {
        for w in [freq - delta, freq + delta] {
            if w <= 0. {
                continue;
            }
            if let Some((e, _)) = fit(v, w, sample) {
                if e < best {
                    best = e;
                    freq = w;
                }
            }
        }
        delta *= 0.35;
    }
    let freq = (freq * 4294967296.).round() as i64;
    let (_, c) = fit(v, freq as f64 / 4294967296., 1)?;
    if c.iter().any(|x| x.abs() > 1e11 || !x.is_finite()) {
        return None;
    }
    Some(Wave {
        freq,
        coef: c.map(|x| (x * 128.).round() as i64),
    })
}

pub(super) fn encode(v: &[i64], out: &mut Vec<u8>) {
    if v.iter().all(|x| *x == v[0]) {
        out.push(0);
        put(out, zz(v.first().copied().unwrap_or(0)));
        return;
    }
    let mut scores = [0u64; 4];
    let mut prev = [0i64; 3];
    for &x in v {
        let mut d = x;
        for j in 0..4 {
            scores[j] += cost(d);
            if j < 3 {
                let next = d.wrapping_sub(prev[j]);
                prev[j] = d;
                d = next;
            }
        }
    }
    let order = (0..4).min_by_key(|&j| scores[j]).unwrap();
    if let Some(w) = wave(v) {
        let score = v
            .iter()
            .enumerate()
            .map(|(i, x)| cost(x.wrapping_sub(w.at(i))))
            .sum::<u64>()
            + w.coef.into_iter().chain([w.freq]).map(cost).sum::<u64>()
            + 32;
        if score < scores[order] {
            out.push(5);
            put(out, zz(w.freq));
            for c in w.coef {
                put(out, zz(c));
            }
            for (i, &x) in v.iter().enumerate() {
                put(out, zz(x.wrapping_sub(w.at(i))));
            }
            return;
        }
    }
    out.push(order as u8 + 1);
    prev.fill(0);
    for &x in v {
        let mut d = x;
        for p in &mut prev[..order] {
            let next = d.wrapping_sub(*p);
            *p = d;
            d = next;
        }
        put(out, zz(d));
    }
}

pub(super) fn decode(r: &mut Reader<'_>, n: usize) -> Result<Vec<i64>> {
    let mode = r.byte()?;
    if mode == 0 {
        return Ok(vec![unzz(r.var()?); n]);
    }
    let w = if mode == 5 {
        let freq = unzz(r.var()?);
        let coef = [unzz(r.var()?), unzz(r.var()?), unzz(r.var()?)];
        if !(0..=13500000000).contains(&freq) || coef.iter().any(|x| x.unsigned_abs() > 1 << 44) {
            return Err(Error::Invalid);
        }
        Some(Wave { freq, coef })
    } else if (1..=4).contains(&mode) {
        None
    } else {
        return Err(Error::Invalid);
    };
    let mut prev = [0i64; 3];
    (0..n)
        .map(|i| {
            let mut d = unzz(r.var()?);
            if let Some(w) = w {
                return Ok(w.at(i).wrapping_add(d));
            }
            for p in prev[..mode as usize - 1].iter_mut().rev() {
                d = d.wrapping_add(*p);
                *p = d;
            }
            Ok(d)
        })
        .collect()
}
