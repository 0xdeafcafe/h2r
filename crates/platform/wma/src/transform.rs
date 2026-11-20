//! Rotation / complex FFT / folded output from XBE 0x3913cd.
//! Trigonometric recurrence seeds are supplied by the runtime table loader.
pub(crate) fn transform(data: &mut [f32], r: &[f32; 11]) {
    let n = data.len();
    let m = n / 2;
    let scale = 2.0 / n as f32;
    let mut complex = vec![[0f32; 2]; m];
    let mut c = scale * r[1];
    let mut s = -scale * r[0];
    let mut previous_c = scale * r[3];
    let mut previous_s = scale * r[2];
    let step = -r[10];
    for (k, v) in complex.iter_mut().enumerate() {
        let a = data[2 * k];
        let b = data[n - 1 - 2 * k];
        *v = [a * c - b * s, a * s + b * c];
        let next_c = previous_c - step * s;
        let next_s = previous_s + step * c;
        previous_c = c;
        previous_s = s;
        c = next_c;
        s = next_s;
    }
    fft(&mut complex);
    let mut c = 1.;
    let mut s = 0.;
    let mut previous_c = r[5];
    let mut previous_s = r[4];
    for k in 0..n / 4 {
        let [a, b] = complex[k];
        let [x, y] = complex[m - 1 - k];
        data[2 * k] = a * c - b * s;
        data[n - 1 - 2 * k] = -a * s - b * c;
        let next_c = previous_c - step * s;
        let next_s = previous_s + step * c;
        previous_c = c;
        previous_s = s;
        c = next_c;
        s = next_s;
        data[2 * k + 1] = x * c + y * s;
        data[n - 2 - 2 * k] = -x * s + y * c;
    }
}
pub(crate) fn fft(v: &mut [[f32; 2]]) {
    let n = v.len();
    let shift = usize::BITS - n.ilog2();
    for i in 0..n {
        let j = i.reverse_bits() >> shift;
        if j > i {
            v.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let angle = -std::f64::consts::TAU / len as f64;
        for chunk in v.chunks_exact_mut(len) {
            for j in 0..len / 2 {
                let (s, c) = (angle * j as f64).sin_cos();
                let c = c as f32;
                let s = s as f32;
                let a = chunk[j];
                let b = chunk[j + len / 2];
                let real = b[0] * c - b[1] * s;
                let imag = b[0] * s + b[1] * c;
                chunk[j] = [a[0] + real, a[1] + imag];
                chunk[j + len / 2] = [a[0] - real, a[1] - imag];
            }
        }
        len *= 2;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rotation_fft_matches_independent_cosine_definition() {
        // Synthetic rotation seeds, computed analytically; no XBE fixtures.
        let n = 128;
        let angle = std::f64::consts::PI / n as f64;
        let r = [
            (angle / 4.).sin() as f32,
            (angle / 4.).cos() as f32,
            (3. * angle / 4.).sin() as f32,
            (3. * angle / 4.).cos() as f32,
            angle.sin() as f32,
            angle.cos() as f32,
            (2. * angle).sin() as f32,
            (2. * angle).cos() as f32,
            (angle / 2.).sin() as f32,
            (angle / 2.).cos() as f32,
            2. * angle.sin() as f32,
        ];
        let original: Vec<_> = (0..n).map(|i| ((i * 17 % 31) as f32 - 15.) / 16.).collect();
        let mut actual = original.clone();
        transform(&mut actual, &r);
        for (i, &v) in actual.iter().enumerate() {
            let expected = original
                .iter()
                .enumerate()
                .map(|(k, &x)| x as f64 * (angle * (i as f64 + 0.5) * (k as f64 + 0.5)).cos())
                .sum::<f64>()
                * 2.
                / n as f64;
            assert!(
                (v as f64 - expected).abs() < 1e-5,
                "sample {i}: {v} vs {expected}"
            );
        }
    }
}
