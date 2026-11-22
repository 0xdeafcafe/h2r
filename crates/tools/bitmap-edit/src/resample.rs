//! Resampling for import: a box filter going down, bilinear going up, per
//! axis, in linear light with colour weighted by alpha (so a cut-out's
//! transparent texels do not bleed their colour in).

use crate::canvas::check_size;
use crate::color::{to_linear, to_srgb};
use crate::error::Error;

/// For each destination index, the source indices and their weights
/// (summing to 1).
fn weights(src: u32, dst: u32) -> Vec<Vec<(usize, f32)>> {
    let scale = src as f32 / dst as f32;
    (0..dst)
        .map(|d| {
            if scale > 1.0 {
                // Down: exact area coverage of [d * scale, (d + 1) * scale).
                let (a, b) = (d as f32 * scale, (d + 1) as f32 * scale);
                let first = a.floor() as usize;
                let last = ((b.ceil() as usize).min(src as usize)).max(first + 1);
                let mut v = Vec::new();
                let mut total = 0.0;
                for i in first..last {
                    let lo = a.max(i as f32);
                    let hi = b.min(i as f32 + 1.0);
                    let w = (hi - lo).max(0.0);
                    if w > 0.0 {
                        v.push((i.min(src as usize - 1), w));
                        total += w;
                    }
                }
                for e in &mut v {
                    e.1 /= total;
                }
                v
            } else {
                // Same size or up: bilinear at the texel centre.
                let c = (d as f32 + 0.5) * scale - 0.5;
                let i0 = c.floor();
                let t = c - i0;
                let lo = (i0.max(0.0) as usize).min(src as usize - 1);
                let hi = ((i0 + 1.0).max(0.0) as usize).min(src as usize - 1);
                if lo == hi || t <= 0.0 {
                    vec![(lo, 1.0)]
                } else {
                    vec![(lo, 1.0 - t), (hi, t)]
                }
            }
        })
        .collect()
}

/// `src` (`sw * sh` RGBA8) resampled to `dw * dh`.
pub fn resample(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Result<Vec<u8>, Error> {
    check_size(sw, sh)?;
    check_size(dw, dh)?;
    if src.len() != sw as usize * sh as usize * 4 {
        return Err(Error::Rect(format!(
            "{sw}x{sh} needs {} bytes, got {}",
            sw as usize * sh as usize * 4,
            src.len()
        )));
    }
    if (sw, sh) == (dw, dh) {
        return Ok(src.to_vec());
    }
    // Premultiplied linear light.
    let pre: Vec<f32> = src
        .chunks_exact(4)
        .flat_map(|p| {
            let a = f32::from(p[3]) / 255.0;
            [
                to_linear(p[0]) * a,
                to_linear(p[1]) * a,
                to_linear(p[2]) * a,
                a,
            ]
        })
        .collect();
    let wx = weights(sw, dw);
    let wy = weights(sh, dh);
    // Horizontal: sh rows of dw pixels.
    let mut mid = vec![0f32; dw as usize * sh as usize * 4];
    for y in 0..sh as usize {
        for (x, taps) in wx.iter().enumerate() {
            let mut acc = [0f32; 4];
            for &(i, w) in taps {
                let o = (y * sw as usize + i) * 4;
                for c in 0..4 {
                    acc[c] += pre[o + c] * w;
                }
            }
            let o = (y * dw as usize + x) * 4;
            mid[o..o + 4].copy_from_slice(&acc);
        }
    }
    // Vertical: dh rows.
    let mut out = Vec::with_capacity(dw as usize * dh as usize * 4);
    for taps in &wy {
        for x in 0..dw as usize {
            let mut acc = [0f32; 4];
            for &(j, w) in taps {
                let o = (j * dw as usize + x) * 4;
                for c in 0..4 {
                    acc[c] += mid[o + c] * w;
                }
            }
            let a = acc[3];
            if a <= 1e-6 {
                out.extend_from_slice(&[0, 0, 0, 0]);
            } else {
                out.push(to_srgb(acc[0] / a));
                out.push(to_srgb(acc[1] / a));
                out.push(to_srgb(acc[2] / a));
                out.push((a.clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
            }
        }
    }
    Ok(out)
}
