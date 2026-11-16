//! Sampling of Halo 2 animation data: the decoder behind
//! [`AnimationClip::sample`]. Plain `[f32; N]` math, no vector library.
//!
//! An animation's data blob (inside a cache resource) holds up to two
//! *streams* and two *flag sets*; see `docs/formats/animation.md` for the layout and
//! for which engine function each codec below was read from. In short:
//!
//! * the **static stream** (blob offset 0) holds one value per flagged node
//!   channel, constant over the animation (codec 1 on the disc);
//! * the **animated stream** follows it and holds per-frame or keyframed
//!   values (codecs 2..=8);
//! * each stream has three per-node bit arrays (rotation, translation,
//!   scale) saying which nodes have that channel in that stream; a node's
//!   index inside the stream is the number of flagged nodes before it.
//!
//! Rotations come out as `[x, y, z, w]` unit quaternions in the engine's own
//! component order (the first three i16 words are the vector part).
use crate::anim::{Animation, AnimationType, f32_at, u32_at};
use crate::{Error, Result};
use std::ops::Range;
use std::sync::Arc;

/// The per-stream compression scheme (first byte of a stream). Names are the
/// engine's own (assigned in its codec table, `docs/engine/engine-map.md`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Codec {
    UncompressedStatic = 1,
    UncompressedAnimated = 2,
    QuantizedRotation8 = 3,
    ByteKeyframe = 4,
    WordKeyframe = 5,
    ReverseByteKeyframe = 6,
    ReverseWordKeyframe = 7,
    BlendScreen = 8,
}
impl Codec {
    pub fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            1 => Self::UncompressedStatic,
            2 => Self::UncompressedAnimated,
            3 => Self::QuantizedRotation8,
            4 => Self::ByteKeyframe,
            5 => Self::WordKeyframe,
            6 => Self::ReverseByteKeyframe,
            7 => Self::ReverseWordKeyframe,
            8 => Self::BlendScreen,
            _ => return None,
        })
    }
    /// The engine's name for the codec.
    pub fn name(self) -> &'static str {
        match self {
            Self::UncompressedStatic => "uncompressed_static_data",
            Self::UncompressedAnimated => "uncompressed_animated_data",
            Self::QuantizedRotation8 => "8byte_quantized_rotation_only",
            Self::ByteKeyframe => "byte_keyframe_lightly_quantized",
            Self::WordKeyframe => "word_keyframe_lightly_quantized",
            Self::ReverseByteKeyframe => "reverse_byte_keyframe_lightly_quantized",
            Self::ReverseWordKeyframe => "reverse_word_keyframe_lightly_quantized",
            Self::BlendScreen => "blend_screen",
        }
    }
    fn key_width(self) -> Option<usize> {
        match self {
            Self::ByteKeyframe | Self::ReverseByteKeyframe => Some(1),
            Self::WordKeyframe | Self::ReverseWordKeyframe => Some(2),
            _ => None,
        }
    }
}

/// One node's transform at a sampled frame. Channels the animation does not
/// supply are identity and their `has_*` flag is false, so a caller can fall
/// back to the skeleton's own default for that channel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NodeTransform {
    /// Unit quaternion `[x, y, z, w]`.
    pub rotation: [f32; 4],
    pub translation: [f32; 3],
    pub scale: f32,
    pub has_rotation: bool,
    pub has_translation: bool,
    pub has_scale: bool,
}
impl Default for NodeTransform {
    fn default() -> Self {
        Self {
            rotation: [0.0, 0.0, 0.0, 1.0],
            translation: [0.0; 3],
            scale: 1.0,
            has_rotation: false,
            has_translation: false,
            has_scale: false,
        }
    }
}
impl NodeTransform {
    /// Column-major 4x4 matrix `T * R * S` (uniform scale), for direct upload.
    pub fn matrix(&self) -> [[f32; 4]; 4] {
        let [x, y, z, w] = self.rotation;
        let s = self.scale;
        let (xx, yy, zz) = (x * x, y * y, z * z);
        let (xy, xz, yz) = (x * y, x * z, y * z);
        let (wx, wy, wz) = (w * x, w * y, w * z);
        [
            [
                s * (1.0 - 2.0 * (yy + zz)),
                s * 2.0 * (xy + wz),
                s * 2.0 * (xz - wy),
                0.0,
            ],
            [
                s * 2.0 * (xy - wz),
                s * (1.0 - 2.0 * (xx + zz)),
                s * 2.0 * (yz + wx),
                0.0,
            ],
            [
                s * 2.0 * (xz + wy),
                s * 2.0 * (yz - wx),
                s * (1.0 - 2.0 * (xx + yy)),
                0.0,
            ],
            [
                self.translation[0],
                self.translation[1],
                self.translation[2],
                1.0,
            ],
        ]
    }
}

/// All node transforms at one instant, indexed like the graph's node block.
#[derive(Clone, Debug, PartialEq)]
pub struct Pose {
    pub nodes: Vec<NodeTransform>,
}

/// Normalise a quaternion; a zero or non-finite one is damaged data, not
/// something to paper over with identity.
fn normalize(q: [f32; 4]) -> Result<[f32; 4]> {
    let l = q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3];
    if l > 0.0 && l.is_finite() {
        let r = l.sqrt().recip();
        Ok([q[0] * r, q[1] * r, q[2] * r, q[3] * r])
    } else {
        Err(bad("degenerate (zero or non-finite) rotation quaternion"))
    }
}
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Column-major 4x4 matrix, `m[column][row]`.
pub type Mat4 = [[f32; 4]; 4];

/// `a * b` for column-major matrices.
pub fn mat_mul(a: &Mat4, b: &Mat4) -> Mat4 {
    std::array::from_fn(|c| std::array::from_fn(|r| (0..4).map(|k| a[k][r] * b[c][k]).sum()))
}

impl Pose {
    /// Compose the local transforms up the node tree into world matrices
    /// (`world = parent_world * local`, column-major). `parents` holds each
    /// node's parent index, negative for a root, as in
    /// `AnimationGraph::nodes[i].parent`. Nodes may appear in any order; a
    /// cycle or an out-of-range parent is an error.
    pub fn world_matrices(&self, parents: &[i16]) -> Result<Vec<Mat4>> {
        let n = self.nodes.len();
        if parents.len() != n {
            return Err(bad("parent list does not match the pose's node count"));
        }
        let mut world: Vec<Option<Mat4>> = vec![None; n];
        for start in 0..n {
            // Walk up to the first node with a known world matrix (or a root).
            let mut chain = Vec::new();
            let mut cur = Some(start);
            while let Some(c) = cur {
                if world[c].is_some() {
                    break;
                }
                if chain.len() > n {
                    return Err(bad("node tree contains a cycle"));
                }
                chain.push(c);
                cur = match parents[c] {
                    p if p < 0 => None,
                    p if (p as usize) < n => Some(p as usize),
                    _ => return Err(bad("node parent index out of range")),
                };
            }
            for &c in chain.iter().rev() {
                let local = self.nodes[c].matrix();
                world[c] = Some(match parents[c] {
                    p if p >= 0 => mat_mul(&world[p as usize].expect("parent resolved"), &local),
                    _ => local,
                });
            }
        }
        Ok(world.into_iter().map(|m| m.expect("resolved")).collect())
    }
}

/// Per-channel index of each node inside a stream (`None`: not in the stream).
type Ranks = Vec<Option<u32>>;

#[derive(Clone, Debug)]
struct Stream {
    codec: Codec,
    /// Offset of the stream inside the clip's data, and its length. Every read
    /// made on behalf of the stream must fall inside `base..base + len`.
    base: usize,
    len: usize,
    /// Header bytes 1..=3: channel counts for rotation, translation, scale.
    counts: [u8; 3],
    rot: Ranks,
    trans: Ranks,
    scale: Ranks,
}

/// Where a channel's keys are and which one governs a frame.
struct KeyPos {
    /// Index of the channel's first key in the shared key arrays.
    first: usize,
    /// Greatest key (relative to `first`) with time <= the frame, or key 0.
    lo: usize,
    /// Time of key `lo`.
    t_lo: usize,
    /// Time of the next key, when the frame lies strictly between two keys.
    /// `None` means hold key `lo` (frame at or past the last key, or before
    /// the first key).
    t_hi: Option<usize>,
}

/// Animation data ready to be sampled. Build it with [`AnimationClip::new`]
/// (or via `AnimationGraph::clip_from_resource` and friends) from the
/// animation record and its data.
#[derive(Clone, Debug)]
pub struct AnimationClip {
    node_count: usize,
    frame_count: usize,
    anim_type: AnimationType,
    /// The animation's declared bytes: `blob[range]`. The buffer may be a
    /// whole resource shared with the clips of every other animation in it.
    blob: Arc<[u8]>,
    range: Range<usize>,
    /// Static stream (engine: only used when the animation type is base), animated stream.
    streams: [Option<Stream>; 2],
}

fn bad(msg: &'static str) -> Error {
    Error::Invalid(msg)
}

/// Bytes of one per-node bit array: the engine's `((nodes + 31) >> 3) & !3`.
pub fn flag_array_bytes(nodes: usize) -> usize {
    ((nodes + 31) >> 3) & !3
}

/// Offsets of an animation's declared sections, relative to its first byte.
struct Layout {
    static_size: usize,
    animated_size: usize,
    static_flags: usize,
    animated_flags: usize,
    have_static: bool,
    have_animated: bool,
    /// One past the last declared byte.
    end: usize,
}

impl Layout {
    fn of(anim: &Animation) -> Result<Self> {
        let s = anim.sizes;
        // The engine reads the static size as a signed 16-bit value (movsx at
        // 0x279bd2 and 0x279c72); a negative one is never valid.
        if s.static_size >= 0x8000 {
            return Err(bad("static stream size has its sign bit set"));
        }
        let (static_size, animated_size) = (usize::from(s.static_size), s.animated_size as usize);
        let a = flag_array_bytes(usize::from(anim.node_count));
        let streams_end = static_size
            .checked_add(animated_size)
            .ok_or_else(|| bad("stream sizes overflow"))?;
        // Flag sets follow the streams: static set, then animated set. The engine
        // adds the static set's size (a signed byte) to find the animated set.
        let animated_flags = streams_end
            .checked_add_signed(isize::from(s.static_flags as i8))
            .ok_or_else(|| bad("flag set offset overflows"))?;
        let have_static = anim.anim_type == AnimationType::Base && static_size != 0;
        let have_animated = animated_size != 0;
        let end = [have_static, have_animated]
            .iter()
            .zip([streams_end, animated_flags])
            .filter(|(h, _)| **h)
            .map(|(_, o)| o + 3 * a)
            .max()
            .unwrap_or(0)
            .max(streams_end);
        Ok(Self {
            static_size,
            animated_size,
            static_flags: streams_end,
            animated_flags,
            have_static,
            have_animated,
            end,
        })
    }
}

impl AnimationClip {
    /// Number of bytes of an animation's data (streams and flag sets) that a
    /// clip needs; the data may extend past it.
    pub fn declared_len(anim: &Animation) -> Result<usize> {
        Ok(Layout::of(anim)?.end)
    }

    /// `blob` starts at the animation's first data byte (the resource offset
    /// `data_offset` already applied) and may extend past its end; bytes past
    /// the declared span are dropped.
    pub fn new(anim: &Animation, mut blob: Vec<u8>) -> Result<Self> {
        blob.truncate(Layout::of(anim)?.end);
        Self::from_shared(anim, Arc::from(blob), 0)
    }

    /// Like [`Self::new`] but sharing `data` (typically a whole resource) with
    /// other clips; the animation's data starts at byte `offset` of it.
    pub fn from_shared(anim: &Animation, data: Arc<[u8]>, offset: usize) -> Result<Self> {
        let node_count = usize::from(anim.node_count);
        let frame_count =
            usize::try_from(anim.frame_count).map_err(|_| bad("negative frame count"))?;
        if frame_count == 0 {
            return Err(bad("animation has no frames"));
        }
        let l = Layout::of(anim)?;
        let range = offset
            .checked_add(l.end)
            .filter(|&e| e <= data.len())
            .map(|e| offset..e)
            .ok_or_else(|| bad("animation blob shorter than its declared sections"))?;
        let mut clip = Self {
            node_count,
            frame_count,
            anim_type: anim.anim_type,
            blob: data,
            range,
            streams: [None, None],
        };
        if l.have_static {
            clip.streams[0] = Some(clip.parse_stream(0, l.static_size, l.static_flags)?);
        }
        if l.have_animated {
            clip.streams[1] =
                Some(clip.parse_stream(l.static_size, l.animated_size, l.animated_flags)?);
        }
        // Surface structural damage now rather than at sampling time.
        clip.sample_frame(0)?;
        clip.sample_frame(frame_count - 1)?;
        Ok(clip)
    }

    fn data(&self) -> &[u8] {
        &self.blob[self.range.clone()]
    }

    fn parse_stream(&self, base: usize, len: usize, flags: usize) -> Result<Stream> {
        if len < 0x20 {
            return Err(bad("animation stream shorter than its header"));
        }
        let d = self.data();
        let codec = Codec::from_u8(d[base]).ok_or_else(|| bad("unknown animation codec"))?;
        let counts = [d[base + 1], d[base + 2], d[base + 3]];
        let a = flag_array_bytes(self.node_count);
        let ranks = |array: usize| -> Ranks {
            let mut next = 0u32;
            (0..self.node_count)
                .map(|n| {
                    let bit = d[flags + array * a + n / 8] >> (n % 8) & 1;
                    (bit != 0).then(|| {
                        next += 1;
                        next - 1
                    })
                })
                .collect()
        };
        let stream = Stream {
            codec,
            base,
            len,
            counts,
            rot: ranks(0),
            trans: ranks(1),
            scale: ranks(2),
        };
        // Retail data: the header's channel counts always equal the flag popcounts.
        let n = |r: &Ranks| r.iter().flatten().count();
        if [n(&stream.rot), n(&stream.trans), n(&stream.scale)] != counts.map(usize::from) {
            return Err(bad("stream channel counts differ from its flag arrays"));
        }
        Ok(stream)
    }

    pub fn node_count(&self) -> usize {
        self.node_count
    }
    pub fn frame_count(&self) -> usize {
        self.frame_count
    }
    pub fn anim_type(&self) -> AnimationType {
        self.anim_type
    }
    /// Codecs of the static and animated streams, where present.
    pub fn codecs(&self) -> [Option<Codec>; 2] {
        [
            self.streams[0].as_ref().map(|s| s.codec),
            self.streams[1].as_ref().map(|s| s.codec),
        ]
    }
    /// True for codec 8 (blend screens): the "frames" of such a clip are the
    /// cells of an aim grid, selected by yaw/pitch weights, not points in
    /// time. A viewer should not play it as a time series.
    pub fn is_aim_grid(&self) -> bool {
        self.streams
            .iter()
            .flatten()
            .any(|s| s.codec == Codec::BlendScreen)
    }
    /// Number of flagged channels `[rotation, translation, scale]` in a stream
    /// (0 = static, 1 = animated), counted from the bit arrays.
    pub fn channel_counts(&self, stream: usize) -> Option<[usize; 3]> {
        let s = self.streams.get(stream)?.as_ref()?;
        let n = |r: &Ranks| r.iter().flatten().count();
        Some([n(&s.rot), n(&s.trans), n(&s.scale)])
    }
    /// The channel counts the stream header itself declares (header bytes 1..=3).
    pub fn declared_counts(&self, stream: usize) -> Option<[u8; 3]> {
        Some(self.streams.get(stream)?.as_ref()?.counts)
    }

    /// `len` bytes at `off` of the clip's data, which must lie inside stream `s`.
    fn bytes(&self, s: &Stream, off: usize, len: usize) -> Result<&[u8]> {
        match off.checked_add(len) {
            Some(e) if off >= s.base && e <= s.base + s.len => Ok(&self.data()[off..e]),
            _ => Err(bad("animation read outside its stream")),
        }
    }
    fn u32(&self, s: &Stream, off: usize) -> Result<u32> {
        Ok(u32_at(self.bytes(s, off, 4)?, 0))
    }
    fn f32s<const N: usize>(&self, s: &Stream, off: usize) -> Result<[f32; N]> {
        let b = self.bytes(s, off, N * 4)?;
        Ok(std::array::from_fn(|i| f32_at(b, i * 4)))
    }
    fn quant_raw(&self, s: &Stream, off: usize) -> Result<[f32; 4]> {
        let b = self.bytes(s, off, 8)?;
        let h = |i: usize| f32::from(i16::from_le_bytes([b[i * 2], b[i * 2 + 1]]));
        Ok([h(0), h(1), h(2), h(3)])
    }
    fn quant(&self, s: &Stream, off: usize) -> Result<[f32; 4]> {
        normalize(self.quant_raw(s, off)?)
    }
    /// Header offset `h` of a stream: `base + u32 at base + h` lookups.
    fn hdr(&self, s: &Stream, h: usize) -> Result<usize> {
        s.base
            .checked_add(self.u32(s, s.base + h)? as usize)
            .ok_or_else(|| bad("stream header offset overflows"))
    }

    /// Key word of channel `idx`: `(count, first key)`.
    fn key_word(&self, s: &Stream, words: usize, idx: u32) -> Result<(usize, usize)> {
        let word = self.u32(s, words + idx as usize * 4)?;
        Ok(((word & 0xfff) as usize, (word >> 12) as usize))
    }
    /// Time of key `k` (counted from the start of the time array `t_base`).
    fn key_time(&self, s: &Stream, t_base: usize, k: usize) -> Result<usize> {
        let width = s.codec.key_width().expect("keyframe codec");
        let b = self.bytes(s, t_base + k * width, width)?;
        Ok(if width == 1 {
            usize::from(b[0])
        } else {
            usize::from(u16::from_le_bytes([b[0], b[1]]))
        })
    }

    /// Key search shared by the keyframe codecs.
    fn keys(&self, s: &Stream, words: usize, times: usize, idx: u32, f: usize) -> Result<KeyPos> {
        let width = s.codec.key_width().expect("keyframe codec");
        let (count, first) = self.key_word(s, words, idx)?;
        if count == 0 {
            return Err(bad("keyframe channel without keys"));
        }
        let t_base = self.hdr(s, times)? + first * width;
        // Greatest key whose time is <= f (engine: bisection on [0, count)).
        let (mut lo, mut hi) = (0usize, count);
        while hi > lo + 1 {
            let mid = (lo + hi) / 2;
            if self.key_time(s, t_base, mid)? <= f {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let t_lo = self.key_time(s, t_base, lo)?;
        // Blend only when the frame is strictly inside a key interval. Before
        // the first key (t_lo > f) the first key is held, where the engine
        // would extrapolate with a negative weight (0x28cdb0); past the last
        // key the engine reads the time at index `count` unchecked (0x28ce1e),
        // which we do not: the last key is held.
        let t_hi = if t_lo < f && lo + 1 < count {
            Some(self.key_time(s, t_base, lo + 1)?).filter(|&th| th > f)
        } else {
            None
        };
        Ok(KeyPos {
            first,
            lo,
            t_lo,
            t_hi,
        })
    }

    /// Audit of keyframe channels: `[channels, strictly ascending, first key at
    /// frame 0, last key at the final frame]`. Zero for non-keyframe streams.
    /// The disc check (`anim_survey`) uses it to confirm the key order the
    /// engine's bisection assumes.
    pub fn key_audit(&self) -> Result<[usize; 4]> {
        let mut out = [0usize; 4];
        for s in self.streams.iter().flatten() {
            let Some(width) = s.codec.key_width() else {
                continue;
            };
            let channels = [
                (&s.rot, s.base + 0x30, 0x14usize),
                (&s.trans, self.hdr(s, 0xc)?, 0x18),
                (&s.scale, self.hdr(s, 0x10)?, 0x1c),
            ];
            for (ranks, words, times) in channels {
                for &i in ranks.iter().flatten() {
                    let (count, first) = self.key_word(s, words, i)?;
                    let t_base = self.hdr(s, times)? + first * width;
                    let t = (0..count)
                        .map(|k| self.key_time(s, t_base, k))
                        .collect::<Result<Vec<usize>>>()?;
                    out[0] += 1;
                    out[1] += usize::from(t.windows(2).all(|w| w[0] < w[1]));
                    out[2] += usize::from(t.first() == Some(&0));
                    out[3] += usize::from(t.last() == Some(&(self.frame_count - 1)));
                }
            }
        }
        Ok(out)
    }

    fn rotation(&self, s: &Stream, i: u32, f: usize) -> Result<[f32; 4]> {
        let hdr14 = || Ok::<usize, Error>(self.u32(s, s.base + 0x14)? as usize);
        Ok(match s.codec {
            Codec::UncompressedStatic => self.quant(s, s.base + 0x20 + i as usize * 8)?,
            Codec::UncompressedAnimated | Codec::BlendScreen => {
                normalize(self.f32s::<4>(s, s.base + 0x20 + i as usize * hdr14()? + f * 16)?)?
            }
            Codec::QuantizedRotation8 => {
                self.quant(s, s.base + 0x20 + i as usize * hdr14()? + f * 8)?
            }
            _ => {
                // Rotation key words are an inline array at +0x30, not behind a header offset.
                let k = self.keys(s, s.base + 0x30, 0x14, i, f)?;
                let values = self.hdr(s, 0x20)? + (k.first + k.lo) * 8;
                let q0 = self.quant_raw(s, values)?;
                match k.t_hi {
                    None => normalize(q0)?,
                    Some(th) => {
                        let q1 = self.quant_raw(s, values + 8)?;
                        let (a, b) = ((th - f) as f32, (f - k.t_lo) as f32);
                        normalize(std::array::from_fn(|c| q0[c] * a + q1[c] * b))?
                    }
                }
            }
        })
    }
    fn translation(&self, s: &Stream, i: u32, f: usize) -> Result<[f32; 3]> {
        Ok(match s.codec {
            Codec::UncompressedStatic => self.f32s::<3>(s, self.hdr(s, 0xc)? + i as usize * 12)?,
            Codec::UncompressedAnimated | Codec::BlendScreen | Codec::QuantizedRotation8 => {
                let stride = self.u32(s, s.base + 0x18)? as usize;
                self.f32s::<3>(s, self.hdr(s, 0xc)? + i as usize * stride + f * 12)?
            }
            _ => {
                let k = self.keys(s, self.hdr(s, 0xc)?, 0x18, i, f)?;
                let values = self.hdr(s, 0x24)? + (k.first + k.lo) * 12;
                let v0 = self.f32s::<3>(s, values)?;
                match k.t_hi {
                    None => v0,
                    Some(th) => {
                        let v1 = self.f32s::<3>(s, values + 12)?;
                        let t = (f - k.t_lo) as f32 / (th - k.t_lo) as f32;
                        std::array::from_fn(|c| lerp(v0[c], v1[c], t))
                    }
                }
            }
        })
    }

    fn scale(&self, s: &Stream, i: u32, f: usize) -> Result<f32> {
        Ok(match s.codec {
            Codec::UncompressedStatic => self.f32s::<1>(s, self.hdr(s, 0x10)? + i as usize * 4)?[0],
            Codec::UncompressedAnimated | Codec::BlendScreen | Codec::QuantizedRotation8 => {
                let stride = self.u32(s, s.base + 0x1c)? as usize;
                self.f32s::<1>(s, self.hdr(s, 0x10)? + i as usize * stride + f * 4)?[0]
            }
            _ => {
                let k = self.keys(s, self.hdr(s, 0x10)?, 0x1c, i, f)?;
                let values = self.hdr(s, 0x28)? + (k.first + k.lo) * 4;
                let v0 = self.f32s::<1>(s, values)?[0];
                match k.t_hi {
                    None => v0,
                    Some(th) => {
                        let v1 = self.f32s::<1>(s, values + 4)?[0];
                        lerp(v0, v1, (f - k.t_lo) as f32 / (th - k.t_lo) as f32)
                    }
                }
            }
        })
    }

    /// Transforms at an integer frame (`0..frame_count`).
    pub fn sample_frame(&self, frame: usize) -> Result<Pose> {
        let mut pose = Pose { nodes: Vec::new() };
        self.sample_frame_into(frame, &mut pose)?;
        Ok(pose)
    }

    /// Like [`Self::sample_frame`], reusing `pose`'s storage. On error the
    /// pose's contents are unspecified.
    pub fn sample_frame_into(&self, frame: usize, pose: &mut Pose) -> Result<()> {
        if frame >= self.frame_count {
            return Err(bad("frame out of range"));
        }
        pose.nodes.clear();
        pose.nodes.resize(self.node_count, NodeTransform::default());
        // Engine order: the static stream is applied first, the animated one after.
        for s in self.streams.iter().flatten() {
            for (n, node) in pose.nodes.iter_mut().enumerate() {
                if let Some(i) = s.rot[n] {
                    node.rotation = self.rotation(s, i, frame)?;
                    node.has_rotation = true;
                }
                if let Some(i) = s.trans[n] {
                    node.translation = self.translation(s, i, frame)?;
                    node.has_translation = true;
                }
                if let Some(i) = s.scale[n] {
                    node.scale = self.scale(s, i, frame)?;
                    node.has_scale = true;
                }
            }
        }
        Ok(())
    }

    /// Transforms at a fractional frame, clamped to `[0, frame_count - 1]`,
    /// linearly interpolating neighbouring frames (rotations are blended along
    /// the shorter arc and re-normalised).
    pub fn sample(&self, frame: f32) -> Result<Pose> {
        let mut pose = Pose { nodes: Vec::new() };
        self.sample_into(frame, &mut pose)?;
        Ok(pose)
    }

    /// Like [`Self::sample`], writing into `pose` (its storage is reused).
    pub fn sample_into(&self, frame: f32, pose: &mut Pose) -> Result<()> {
        let last = self.frame_count - 1;
        let f = if frame.is_nan() {
            0.0
        } else {
            frame.clamp(0.0, last as f32)
        };
        let i0 = f.floor() as usize;
        let t = f - i0 as f32;
        self.sample_frame_into(i0, pose)?;
        if t <= 0.0 || i0 >= last {
            return Ok(());
        }
        let b = self.sample_frame(i0 + 1)?;
        for (p, q) in pose.nodes.iter_mut().zip(&b.nodes) {
            let dot: f32 = p.rotation.iter().zip(&q.rotation).map(|(x, y)| x * y).sum();
            let sign = if dot < 0.0 { -1.0 } else { 1.0 };
            p.rotation = normalize(std::array::from_fn(|k| {
                lerp(p.rotation[k], sign * q.rotation[k], t)
            }))?;
            p.translation = std::array::from_fn(|k| lerp(p.translation[k], q.translation[k], t));
            p.scale = lerp(p.scale, q.scale, t);
        }
        Ok(())
    }
}
