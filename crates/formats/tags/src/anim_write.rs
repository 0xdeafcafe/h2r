//! The `jmad` animation encoder: the static stream (codec 1), the animated
//! streams (codec 3, i16 quaternions; codec 8, float quaternions) and the
//! per-node flag arrays, in the layouts [`crate::anim_sample`] decodes
//! (`docs/formats/animation.md`).

use crate::anim_sample::flag_array_bytes;

pub const CODEC_STATIC: u8 = 1;
pub const CODEC_QUANTIZED_ROTATION: u8 = 3;
/// Codec of an animation with a blend screen: float rotations (V: every
/// retail animation that names a screen, the vehicles' steering and aim
/// overlays included, is codec 8).
pub const CODEC_BLEND_SCREEN: u8 = 8;

/// One node's transform at one frame, in its parent's space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub rotation: [f32; 4],
    pub translation: [f32; 3],
    pub scale: f32,
}
impl Default for Transform {
    fn default() -> Self {
        Self {
            rotation: [0.0, 0.0, 0.0, 1.0],
            translation: [0.0; 3],
            scale: 1.0,
        }
    }
}

pub fn q16(q: [f32; 4]) -> [u8; 8] {
    let mut out = [0u8; 8];
    for (i, x) in q.iter().enumerate() {
        let v = (x.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        out[2 * i..2 * i + 2].copy_from_slice(&v.to_le_bytes());
    }
    out
}

pub fn put32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

/// Node channel sets: `[rotation, translation, scale]` bit masks.
pub type Channels = [u64; 3];

pub fn ranks(mask: u64, nodes: usize) -> Vec<usize> {
    (0..nodes).filter(|&n| mask >> n & 1 != 0).collect()
}

/// Codec 1: one value per channel.

/// Codec 1: one value per channel.
pub fn static_stream(pose: &[Transform], ch: Channels) -> Vec<u8> {
    let n = pose.len();
    let [r, t, s] = ch.map(|m| ranks(m, n));
    let trans_at = 0x20 + 8 * r.len();
    let scale_at = trans_at + 12 * t.len();
    let mut b = vec![0u8; scale_at + 4 * s.len()];
    b[0] = CODEC_STATIC;
    b[1..4].copy_from_slice(&[r.len() as u8, t.len() as u8, s.len() as u8]);
    put32(&mut b, 0xc, trans_at as u32);
    put32(&mut b, 0x10, scale_at as u32);
    for (i, &node) in r.iter().enumerate() {
        b[0x20 + 8 * i..0x28 + 8 * i].copy_from_slice(&q16(pose[node].rotation));
    }
    for (i, &node) in t.iter().enumerate() {
        for a in 0..3 {
            put32(
                &mut b,
                trans_at + 12 * i + 4 * a,
                pose[node].translation[a].to_bits(),
            );
        }
    }
    for (i, &node) in s.iter().enumerate() {
        put32(&mut b, scale_at + 4 * i, pose[node].scale.to_bits());
    }
    b
}

/// Codec 3: per channel, every frame; i16 rotations, float translations
/// and scales. Codec 8 (`blend_screen`, when `screen`) is the same with
/// float rotations, 16 bytes a frame (E: its rotation function
/// `0x0028c710` reads four floats at `+0x20 + i * [+0x14] + frame * 16`;
/// translation and scale are codec 3's functions).
pub fn animated_stream(frames: &[Vec<Transform>], ch: Channels, screen: bool) -> Vec<u8> {
    let n = frames[0].len();
    let f = frames.len();
    let [r, t, s] = ch.map(|m| ranks(m, n));
    let rot = if screen { 16 } else { 8 };
    let trans_at = 0x20 + rot * f * r.len();
    let scale_at = trans_at + 12 * f * t.len();
    let mut b = vec![0u8; scale_at + 4 * f * s.len()];
    b[0] = if screen {
        CODEC_BLEND_SCREEN
    } else {
        CODEC_QUANTIZED_ROTATION
    };
    b[1..4].copy_from_slice(&[r.len() as u8, t.len() as u8, s.len() as u8]);
    put32(&mut b, 0xc, trans_at as u32);
    put32(&mut b, 0x10, scale_at as u32);
    put32(&mut b, 0x14, (rot * f) as u32);
    put32(&mut b, 0x18, (12 * f) as u32);
    put32(&mut b, 0x1c, (4 * f) as u32);
    for (i, &node) in r.iter().enumerate() {
        for (k, pose) in frames.iter().enumerate() {
            let at = 0x20 + i * rot * f + k * rot;
            if screen {
                for (a, x) in pose[node].rotation.iter().enumerate() {
                    put32(&mut b, at + 4 * a, x.to_bits());
                }
            } else {
                b[at..at + 8].copy_from_slice(&q16(pose[node].rotation));
            }
        }
    }
    for (i, &node) in t.iter().enumerate() {
        for (k, pose) in frames.iter().enumerate() {
            for a in 0..3 {
                let at = trans_at + i * 12 * f + k * 12 + 4 * a;
                put32(&mut b, at, pose[node].translation[a].to_bits());
            }
        }
    }
    for (i, &node) in s.iter().enumerate() {
        for (k, pose) in frames.iter().enumerate() {
            put32(
                &mut b,
                scale_at + i * 4 * f + k * 4,
                pose[node].scale.to_bits(),
            );
        }
    }
    b
}

pub fn flag_set(ch: Channels, nodes: usize) -> Vec<u8> {
    let a = flag_array_bytes(nodes);
    let mut b = vec![0u8; 3 * a];
    for (k, mask) in ch.iter().enumerate() {
        for n in ranks(*mask, nodes) {
            b[k * a + n / 8] |= 1 << (n % 8);
        }
    }
    b
}
