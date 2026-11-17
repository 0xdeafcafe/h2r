//! Halo 1 Xbox compressed animation data: keyframed channels.
//!
//! An animation whose flags (`+0x3a`) have bit 0 set keeps its data in this
//! form at `frame data + [+0x88]`, instead of one record per frame. Every
//! animated channel is a list of keyframes (frame number and value); frame 0
//! is a per-node "static" value, and frames between keys are interpolated:
//! linearly for translations and scales, normalised-linearly for rotations.
//! Layout and evidence: `docs/formats/h1-models.md` ("Compressed
//! animations").

use crate::h1::{Transform, normalize, u16_at, u32_at};
use crate::{Error, Result};

/// Size of the fixed header; the rotation channel table follows it.
pub const HEADER: usize = 0x2c;

/// The 48-bit rotation: four signed 12-bit components `i, j, k, w`, most
/// significant first across three little-endian u16s. Each is widened to 16
/// bits by repeating its top four bits below it and divided by 32767, as the
/// engine does. Not normalised.
pub fn unpack_quat(b: &[u8]) -> [f32; 4] {
    let bits =
        u64::from(u16_at(b, 0)) << 32 | u64::from(u16_at(b, 2)) << 16 | u64::from(u16_at(b, 4));
    std::array::from_fn(|i| {
        let v = (bits >> (36 - 12 * i)) & 0xfff;
        let wide = ((v << 4) | (v >> 8)) as u16 as i16;
        f32::from(wide) / 32767.0
    })
}

/// The 48-bit form of `q` (each component in [-1, 1]); the inverse of
/// [`unpack_quat`] up to rounding.
pub fn pack_quat(q: [f32; 4]) -> [u8; 6] {
    let mut bits = 0u64;
    for (i, x) in q.into_iter().enumerate() {
        let v = (x.clamp(-1.0, 1.0) * 2047.0).round() as i64 as u64 & 0xfff;
        bits |= v << (36 - 12 * i);
    }
    let w = [(bits >> 32) as u16, (bits >> 16) as u16, bits as u16];
    let mut out = [0u8; 6];
    for (i, w) in w.into_iter().enumerate() {
        out[2 * i..2 * i + 2].copy_from_slice(&w.to_le_bytes());
    }
    out
}

/// One channel: keyframe times (all after frame 0) and values, and the
/// value at frame 0.
struct Track<V> {
    start: V,
    times: Vec<u16>,
    values: Vec<V>,
}

impl<V: Copy> Track<V> {
    /// The value at integer frame `f`. Before the first key it blends from
    /// the frame-0 value; on a key it is that key; past the last key it
    /// holds (never seen on the disc: every channel's last key is the last
    /// frame).
    fn sample(&self, f: u16, lerp: impl Fn(V, V, f32) -> V) -> V {
        let Some(&first) = self.times.first() else {
            return self.start;
        };
        if f < first {
            return lerp(self.start, self.values[0], f32::from(f) / f32::from(first));
        }
        let k = self.times.partition_point(|&t| t <= f) - 1;
        let t0 = self.times[k];
        if t0 == f || k + 1 == self.times.len() {
            return self.values[k];
        }
        let t1 = self.times[k + 1];
        lerp(
            self.values[k],
            self.values[k + 1],
            f32::from(f - t0) / f32::from(t1 - t0),
        )
    }
}

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    std::array::from_fn(|i| a[i] * (1.0 - t) + b[i] * t)
}

/// Blends two (unnormalised) rotations the engine's way: the shorter arc,
/// componentwise, normalised afterwards by the caller.
fn nlerp(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    let dot: f32 = (0..4).map(|i| a[i] * b[i]).sum();
    let tb = if dot < 0.0 { -t } else { t };
    std::array::from_fn(|i| a[i] * (1.0 - t) + b[i] * tb)
}

/// A region of the blob between two header offsets.
fn region<'a>(b: &'a [u8], from: usize, to: usize, what: &str) -> Result<&'a [u8]> {
    if from > to || to > b.len() {
        return Err(Error::Invalid(format!(
            "compressed {what}: bytes {from:#x}..{to:#x} of {:#x}",
            b.len()
        )));
    }
    Ok(&b[from..to])
}

/// What a decode found, for reports and tests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Keyframes over every channel.
    pub keys: usize,
    /// Animated channels with no keys (constant at their frame-0 value).
    pub constant_channels: usize,
    /// Channels whose last key is before the last frame (held).
    pub short_channels: usize,
    /// Zero rotations (statics or keys), replaced by identity on output.
    pub zero_rotations: usize,
}

/// Reads one kind of channel: `count` table entries at `table`, each a u32
/// of key count (low 12 bits) and first key index (high 20 bits) into
/// `times` (u16) and `values` (`size` bytes each).
#[allow(clippy::too_many_arguments)]
fn tracks<V: Copy>(
    table: &[u8],
    times: &[u8],
    values: &[u8],
    size: usize,
    starts: &[V],
    frame_count: u16,
    what: &str,
    stats: &mut Stats,
    read: impl Fn(&[u8]) -> V,
) -> Result<Vec<Track<V>>> {
    let mut out = Vec::with_capacity(starts.len());
    for (c, &start) in starts.iter().enumerate() {
        let e = u32_at(table, 4 * c);
        let (n, first) = ((e & 0xfff) as usize, (e >> 12) as usize);
        if (first + n) * 2 > times.len() || (first + n) * size > values.len() {
            return Err(Error::Invalid(format!(
                "compressed {what} channel {c}: keys {first}+{n} past the data"
            )));
        }
        let t: Vec<u16> = (first..first + n).map(|k| u16_at(times, 2 * k)).collect();
        if t.first() == Some(&0) || t.windows(2).any(|w| w[0] >= w[1]) {
            return Err(Error::Invalid(format!(
                "compressed {what} channel {c}: key times {t:?}"
            )));
        }
        match t.last() {
            None => stats.constant_channels += 1,
            Some(&l) if l + 1 < frame_count => stats.short_channels += 1,
            _ => {}
        }
        stats.keys += n;
        out.push(Track {
            start,
            times: t,
            values: (first..first + n)
                .map(|k| read(&values[k * size..]))
                .collect(),
        });
    }
    Ok(out)
}

/// Decodes compressed animation data into every frame's node transforms,
/// the same shape the uncompressed reader produces. `animated` holds the
/// rotation, translation and scale flags, already masked to `node_count`.
/// Channels a node does not animate take the per-node static rotation and
/// translation, and scale 1 (as the engine does).
pub fn decode(
    blob: &[u8],
    frame_count: u16,
    node_count: usize,
    animated: [u64; 3],
) -> Result<(Vec<Vec<Transform>>, Stats)> {
    if blob.len() < HEADER {
        return Err(Error::Truncated("compressed animation header"));
    }
    let o: [usize; 11] = std::array::from_fn(|i| u32_at(blob, 4 * i) as usize);
    let [r, t, s] = animated.map(|b| b.count_ones() as usize);
    if o[0] != HEADER + 4 * r {
        return Err(Error::Invalid(format!(
            "compressed: rotation times at {:#x}, want {:#x} for {r} channels",
            o[0],
            HEADER + 4 * r
        )));
    }
    let n = node_count;
    let rot_table = region(blob, HEADER, o[0], "rotation table")?;
    let rot_times = region(blob, o[0], o[1], "rotation times")?;
    let rot_static = region(blob, o[1], o[2], "rotation statics")?;
    let rot_values = region(blob, o[2], o[3], "rotation keys")?;
    let tr_table = region(blob, o[3], o[4], "translation table")?;
    let tr_times = region(blob, o[4], o[5], "translation times")?;
    let tr_static = region(blob, o[5], o[6], "translation statics")?;
    let tr_values = region(blob, o[6], o[7], "translation keys")?;
    let sc_table = region(blob, o[7], o[8], "scale table")?;
    let sc_times = region(blob, o[8], o[9], "scale times")?;
    let sc_static = region(blob, o[9], o[10], "scale statics")?;
    let sc_values = region(blob, o[10], blob.len(), "scale keys")?;
    for (got, want, what) in [
        (rot_static.len(), 6 * n, "rotation statics"),
        (tr_table.len(), 4 * t, "translation table"),
        (tr_static.len(), 12 * n, "translation statics"),
        (sc_table.len(), 4 * s, "scale table"),
        (sc_static.len(), 4 * s, "scale statics"),
    ] {
        if got != want {
            return Err(Error::Invalid(format!(
                "compressed {what}: {got} bytes, want {want}"
            )));
        }
    }
    let mut stats = Stats::default();
    let f3 = |b: &[u8]| -> [f32; 3] { std::array::from_fn(|i| f32::from_bits(u32_at(b, 4 * i))) };
    let f1 = |b: &[u8]| f32::from_bits(u32_at(b, 0));
    let quat = |b: &[u8]| unpack_quat(b);

    // Statics: rotations and translations per node, scales per channel.
    let node_rot: Vec<[f32; 4]> = (0..n).map(|i| quat(&rot_static[6 * i..])).collect();
    let node_tr: Vec<[f32; 3]> = (0..n).map(|i| f3(&tr_static[12 * i..])).collect();
    let on = |c: usize| -> Vec<usize> { (0..n).filter(|&i| animated[c] >> i & 1 != 0).collect() };
    let (rot_nodes, tr_nodes, sc_nodes) = (on(0), on(1), on(2));
    let rot_starts: Vec<_> = rot_nodes.iter().map(|&i| node_rot[i]).collect();
    let tr_starts: Vec<_> = tr_nodes.iter().map(|&i| node_tr[i]).collect();
    let sc_starts: Vec<f32> = (0..s).map(|c| f1(&sc_static[4 * c..])).collect();

    let rots = tracks(
        rot_table,
        rot_times,
        rot_values,
        6,
        &rot_starts,
        frame_count,
        "rotation",
        &mut stats,
        quat,
    )?;
    let trs = tracks(
        tr_table,
        tr_times,
        tr_values,
        12,
        &tr_starts,
        frame_count,
        "translation",
        &mut stats,
        f3,
    )?;
    let scs = tracks(
        sc_table,
        sc_times,
        sc_values,
        4,
        &sc_starts,
        frame_count,
        "scale",
        &mut stats,
        f1,
    )?;

    let is_zero = |q: &[f32; 4]| q.iter().all(|&x| x == 0.0);
    stats.zero_rotations = node_rot.iter().filter(|q| is_zero(q)).count()
        + rots
            .iter()
            .flat_map(|t| &t.values)
            .filter(|q| is_zero(q))
            .count();

    let mut base = vec![Transform::default(); n];
    for (i, b) in base.iter_mut().enumerate() {
        b.rotation = normalize(node_rot[i]);
        b.translation = node_tr[i];
    }
    let frames = (0..frame_count)
        .map(|f| {
            let mut pose = base.clone();
            for (track, &i) in rots.iter().zip(&rot_nodes) {
                pose[i].rotation = normalize(track.sample(f, nlerp));
            }
            for (track, &i) in trs.iter().zip(&tr_nodes) {
                pose[i].translation = track.sample(f, lerp3);
            }
            for (track, &i) in scs.iter().zip(&sc_nodes) {
                pose[i].scale = track.sample(f, |a, b, t| a * (1.0 - t) + b * t);
            }
            pose
        })
        .collect();
    Ok((frames, stats))
}

/// Builds compressed data from keyframes, for tests: per node the frame-0
/// rotation and translation, and for each animated channel (node order) its
/// keys after frame 0. Scale channels carry their own frame-0 value.
#[cfg(test)]
pub(crate) fn encode(
    node_rot: &[[f32; 4]],
    node_tr: &[[f32; 3]],
    rot_keys: &[Vec<(u16, [f32; 4])>],
    tr_keys: &[Vec<(u16, [f32; 3])>],
    sc: &[(f32, Vec<(u16, f32)>)],
) -> Vec<u8> {
    fn table<V>(keys: &[Vec<(u16, V)>]) -> (Vec<u8>, Vec<u8>) {
        let (mut table, mut times, mut first) = (Vec::new(), Vec::new(), 0u32);
        for k in keys {
            table.extend((k.len() as u32 | first << 12).to_le_bytes());
            times.extend(k.iter().flat_map(|(t, _)| t.to_le_bytes()));
            first += k.len() as u32;
        }
        (table, times)
    }
    let f = |v: &[f32]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
    let sc_keys: Vec<Vec<(u16, f32)>> = sc.iter().map(|(_, k)| k.clone()).collect();
    let (rt, rtimes) = table(rot_keys);
    let (tt, ttimes) = table(tr_keys);
    let (st, stimes) = table(&sc_keys);
    let parts: [Vec<u8>; 12] = [
        rt,
        rtimes,
        node_rot.iter().flat_map(|q| pack_quat(*q)).collect(),
        rot_keys
            .iter()
            .flatten()
            .flat_map(|(_, q)| pack_quat(*q))
            .collect(),
        tt,
        ttimes,
        node_tr.iter().flat_map(|v| f(v)).collect(),
        tr_keys.iter().flatten().flat_map(|(_, v)| f(v)).collect(),
        st,
        stimes,
        sc.iter().flat_map(|(s, _)| f(&[*s])).collect(),
        sc_keys
            .iter()
            .flatten()
            .flat_map(|(_, s)| f(&[*s]))
            .collect(),
    ];
    let mut out = vec![0u8; HEADER];
    let mut at = HEADER;
    for (i, p) in parts.iter().enumerate() {
        at += p.len();
        if i < 11 {
            out[4 * i..4 * i + 4].copy_from_slice(&(at as u32).to_le_bytes());
        }
    }
    for p in parts {
        out.extend(p);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quaternion_bits_follow_the_engine() {
        // Components 0x7ff, 0x800, 0x001, 0xfff: i = max, j = min, k = +1
        // step, w = -1 step; each widened by repeating its top nibble.
        let bits: u64 = 0x7ff << 36 | 0x800 << 24 | 0x001 << 12 | 0xfff;
        let w = [(bits >> 32) as u16, (bits >> 16) as u16, bits as u16];
        let b: Vec<u8> = w.iter().flat_map(|w| w.to_le_bytes()).collect();
        let q = unpack_quat(&b);
        assert_eq!(q[0], f32::from(0x7ff7i16) / 32767.0);
        assert_eq!(q[1], f32::from(0x8008u16 as i16) / 32767.0);
        assert_eq!(q[2], f32::from(0x0010i16) / 32767.0);
        assert_eq!(q[3], f32::from(0xffffu16 as i16) / 32767.0);
        for q in [
            [0.0, 0.0, 0.0, 1.0],
            [0.5, -0.5, 0.5, -0.5],
            [0.1, 0.2, -0.3, 0.927],
        ] {
            let back = unpack_quat(&pack_quat(q));
            for i in 0..4 {
                assert!((back[i] - q[i]).abs() < 1e-3, "{q:?} -> {back:?}");
            }
        }
    }

    #[test]
    fn keys_interpolate_from_the_frame_zero_static() {
        const S: f32 = std::f32::consts::FRAC_1_SQRT_2;
        let id = [0.0, 0.0, 0.0, 1.0];
        let blob = encode(
            &[id, id],
            &[[0.0; 3], [1.0, 2.0, 3.0]],
            &[vec![(2, [0.0, 0.0, S, S]), (4, id)]],
            &[vec![(4, [5.0, 2.0, 3.0])]],
            &[(1.0, vec![(1, 2.0), (4, 2.0)])],
        );
        // Node 1 rotates and translates; node 0 scales.
        let (frames, stats) = decode(&blob, 5, 2, [0b10, 0b10, 0b01]).unwrap();
        assert_eq!(frames.len(), 5);
        assert_eq!(stats.keys, 5);
        assert_eq!(stats.short_channels, 0);
        // Static channels.
        assert_eq!(frames[3][0].rotation, id);
        assert_eq!(frames[3][0].translation, [0.0; 3]);
        assert_eq!(frames[3][1].scale, 1.0);
        // Translation: frame-0 static to the key at 4.
        let tx: Vec<f32> = frames.iter().map(|p| p[1].translation[0]).collect();
        assert_eq!(tx, [1.0, 2.0, 3.0, 4.0, 5.0]);
        // Scale: 1 at frame 0, 2 from frame 1 on.
        let sc: Vec<f32> = frames.iter().map(|p| p[0].scale).collect();
        assert_eq!(sc, [1.0, 2.0, 2.0, 2.0, 2.0]);
        // Rotation: identity, half way, the key, half way back, identity.
        let r = |f: usize| frames[f][1].rotation;
        assert!((r(0)[3] - 1.0).abs() < 1e-6);
        assert!((r(2)[2] - S).abs() < 1e-3 && (r(2)[3] - S).abs() < 1e-3);
        let half = normalize([0.0, 0.0, S / 2.0, (1.0 + S) / 2.0]);
        for f in [1, 3] {
            for i in 0..4 {
                assert!((r(f)[i] - half[i]).abs() < 1e-3, "{f}: {:?}", r(f));
            }
        }
        for p in &frames {
            for t in p {
                let l: f32 = t.rotation.iter().map(|x| x * x).sum();
                assert!((l - 1.0).abs() < 1e-5);
            }
        }
    }

    #[test]
    fn rotations_take_the_shorter_arc() {
        let id = [0.0, 0.0, 0.0, 1.0];
        let neg = [0.0, 0.0, 0.0, -1.0];
        let blob = encode(&[id], &[[0.0; 3]], &[vec![(2, neg)]], &[], &[]);
        let (frames, _) = decode(&blob, 3, 1, [1, 0, 0]).unwrap();
        // Same rotation both ends: the midpoint is not a zero quaternion.
        assert!((frames[1][0].rotation[3].abs() - 1.0).abs() < 1e-3);
    }

    #[test]
    fn channels_past_the_last_key_hold() {
        let blob = encode(
            &[[0.0, 0.0, 0.0, 1.0]],
            &[[0.0; 3]],
            &[],
            &[vec![(1, [1.0; 3])]],
            &[],
        );
        let (frames, stats) = decode(&blob, 4, 1, [0, 1, 0]).unwrap();
        assert_eq!(stats.short_channels, 1);
        assert_eq!(frames[3][0].translation, [1.0; 3]);
    }

    #[test]
    fn malformed_data_is_refused() {
        let id = [0.0, 0.0, 0.0, 1.0];
        let good = encode(&[id], &[[0.0; 3]], &[vec![(1, id)]], &[], &[]);
        assert!(decode(&good, 2, 1, [1, 0, 0]).is_ok());
        // Wrong channel count for the flags.
        assert!(decode(&good, 2, 1, [0, 0, 0]).is_err());
        // Wrong node count for the statics.
        assert!(decode(&good, 2, 2, [1, 0, 0]).is_err());
        // Truncated.
        assert!(decode(&good[..good.len() - 1], 2, 1, [1, 0, 0]).is_err());
        assert!(decode(&good[..8], 2, 1, [1, 0, 0]).is_err());
        // A key at frame 0, and keys out of order.
        let b = encode(&[id], &[[0.0; 3]], &[vec![(0, id)]], &[], &[]);
        assert!(decode(&b, 2, 1, [1, 0, 0]).is_err());
        let b = encode(&[id], &[[0.0; 3]], &[vec![(2, id), (1, id)]], &[], &[]);
        assert!(decode(&b, 3, 1, [1, 0, 0]).is_err());
        // A key index past the data.
        let mut b = good.clone();
        b[HEADER..HEADER + 4].copy_from_slice(&(1u32 | 5 << 12).to_le_bytes());
        assert!(decode(&b, 2, 1, [1, 0, 0]).is_err());
    }
}
