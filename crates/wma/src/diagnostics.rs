//! Reference-free measurements. These detect gross failures, not codec accuracy.
#[derive(Debug)]
pub struct Sanity {
    pub rms: f64,
    pub dc: [f64; 2],
    pub peak: i32,
    pub clipped_fraction: f64,
    pub boundary_mean_step: f64,
    pub interior_mean_step: f64,
    /// Fraction of spectral power above 11.025 kHz in three Hann windows per channel.
    pub high_frequency_fraction: f64,
}
pub fn sanity(pcm: &[i16], boundaries: &[usize]) -> Sanity {
    let mut sums = [0f64; 2];
    let mut energy = 0.;
    let mut peak = 0;
    let mut clipped = 0;
    for (i, &x) in pcm.iter().enumerate() {
        sums[i % 2] += f64::from(x);
        energy += f64::from(x).powi(2);
        peak = peak.max(i32::from(x).abs());
        clipped += usize::from(x == i16::MAX || x == i16::MIN);
    }
    let frames = pcm.len() / 2;
    let mut boundary = 0.;
    let mut interior = 0.;
    let mut bn = 0;
    let mut inn = 0;
    for i in 1..frames {
        for ch in 0..2 {
            let d =
                f64::from((i32::from(pcm[i * 2 + ch]) - i32::from(pcm[(i - 1) * 2 + ch])).abs());
            if boundaries.binary_search(&i).is_ok() {
                boundary += d;
                bn += 1;
            } else {
                interior += d;
                inn += 1;
            }
        }
    }
    let (mut high, mut total) = (0., 0.);
    if frames >= 1024 {
        for start in [0, (frames - 1024) / 2, frames - 1024] {
            for ch in 0..2 {
                let mut v: Vec<_> = (0..1024)
                    .map(|i| {
                        [
                            pcm[(start + i) * 2 + ch] as f32
                                * (0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / 1023.).cos()),
                            0.,
                        ]
                    })
                    .collect();
                crate::transform::fft(&mut v);
                for (k, x) in v.iter().take(513).enumerate() {
                    let e = f64::from(x[0]).powi(2) + f64::from(x[1]).powi(2);
                    total += e;
                    if k >= 256 {
                        high += e;
                    }
                }
            }
        }
    }
    Sanity {
        rms: (energy / pcm.len().max(1) as f64).sqrt(),
        dc: sums.map(|v| v / frames.max(1) as f64),
        peak,
        clipped_fraction: clipped as f64 / pcm.len().max(1) as f64,
        boundary_mean_step: boundary / bn.max(1) as f64,
        interior_mean_step: interior / inn.max(1) as f64,
        high_frequency_fraction: high / total.max(f64::MIN_POSITIVE),
    }
}
#[derive(Debug)]
pub struct Comparison {
    pub compared_samples: usize,
    pub native_samples: usize,
    pub reference_samples: usize,
    pub max_abs_error: i32,
    pub snr_db: f64,
    pub rmse: f64,
}
/// Compare without alignment, gain fitting, or truncation hidden from the report.
/// Length mismatch remains explicit even though the common prefix is measured.
pub fn compare(native: &[i16], reference: &[i16]) -> Comparison {
    let mut error = 0.;
    let mut signal = 0.;
    let mut maximum = 0;
    for (&a, &b) in native.iter().zip(reference) {
        let e = i32::from(a) - i32::from(b);
        maximum = maximum.max(e.abs());
        error += f64::from(e).powi(2);
        signal += f64::from(b).powi(2);
    }
    let n = native.len().min(reference.len());
    let snr = if error == 0. {
        f64::INFINITY
    } else if signal == 0. {
        f64::NEG_INFINITY
    } else {
        10. * (signal / error).log10()
    };
    Comparison {
        compared_samples: n,
        native_samples: native.len(),
        reference_samples: reference.len(),
        max_abs_error: maximum,
        snr_db: snr,
        rmse: (error / n.max(1) as f64).sqrt(),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn comparison_reports_full_scale_difference_and_length() {
        let c = compare(&[i16::MIN, 10, 9], &[i16::MAX, 10]);
        assert_eq!(c.max_abs_error, 65535);
        assert_eq!(c.native_samples, 3);
        assert_eq!(c.reference_samples, 2);
        assert!(c.snr_db < 0.);
        assert!(compare(&[1, 2], &[1, 2]).snr_db.is_infinite());
    }
    #[test]
    fn spectrum_distinguishes_low_and_high_tones() {
        let tone = |bin: f64| -> Vec<i16> {
            (0..2048)
                .flat_map(|i| {
                    let x =
                        (10000. * (std::f64::consts::TAU * bin * i as f64 / 1024.).sin()) as i16;
                    [x, x]
                })
                .collect()
        };
        assert!(sanity(&tone(32.), &[1024]).high_frequency_fraction < 0.001);
        assert!(sanity(&tone(400.), &[1024]).high_frequency_fraction > 0.999);
    }
}
