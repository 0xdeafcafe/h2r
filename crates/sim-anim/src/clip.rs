//! A clip: one animation's resource blob, copied out of guest memory once,
//! with each stream's flag arrays turned into node lists.
//!
//! The blob is what the guest's evaluation reads (docs/engine/anim-sampling.md
//! "Data"): a static stream at offset 0, an animated stream after it, then
//! the per-stream flag arrays (rotation, translation, scale; one bit per
//! node, `((nodes + 31) >> 3) & !3` bytes each). The clip keeps the bytes
//! exactly as the guest sees them and reads values from them the way the
//! guest's codec functions do, so a clip cannot disagree with the guest.
//! What decoding buys is everything around the values: the flag walk
//! becomes a list of `(node, track)` pairs, the header is read once, and
//! sampling makes no guest-memory accesses at all.

/// The animation's data-sizes record (`element + 0x3c`), the first 16
/// bytes, which the clip layout and the store's validation use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Sizes(pub [u8; 16]);

impl Sizes {
    /// Signed byte at `+0`: where the animated stream's flag arrays start,
    /// after the static stream's.
    pub fn animated_flags(&self) -> i32 {
        i32::from(self.0[0] as i8)
    }
    /// Signed word at `+6`: the static stream's size.
    pub fn static_len(&self) -> i32 {
        i32::from(i16::from_le_bytes([self.0[6], self.0[7]]))
    }
    /// Dword at `+0xc`: the animated stream's size.
    pub fn animated_len(&self) -> u32 {
        u32::from_le_bytes(self.0[12..16].try_into().unwrap())
    }
}

/// Everything about an animation the clip depends on besides its blob.
/// The store compares it on every lookup, so a stale key rebuilds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Shape {
    pub sizes: Sizes,
    /// The view's node count (`view + 8`), which sizes the flag arrays.
    pub nodes: u8,
}

/// Where each part of the blob lies, from [`Shape`] alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub static_len: u32,
    pub animated_len: u32,
    /// Offset of the static stream's three flag arrays.
    pub static_flags: u32,
    /// Offset of the animated stream's three flag arrays.
    pub animated_flags: u32,
    /// Bytes per flag array.
    pub flag_bytes: u32,
    /// Bytes to copy: through the end of the last flag array.
    pub len: u32,
}

/// The largest blob the clip store copies. Larger (or negative) sizes are
/// left to the guest.
pub const MAX_BLOB: u32 = 16 << 20;

impl Layout {
    pub fn new(shape: &Shape) -> Option<Layout> {
        let s = &shape.sizes;
        let static_len = u32::try_from(s.static_len()).ok()?;
        let animated_len = s.animated_len();
        let offset = u32::try_from(s.animated_flags()).ok()?;
        let flag_bytes = ((u32::from(shape.nodes) + 31) >> 3) & !3;
        let static_flags = static_len.checked_add(animated_len)?;
        let animated_flags = static_flags.checked_add(offset)?;
        let len = static_flags
            .max(animated_flags)
            .checked_add(3 * flag_bytes)?;
        (len <= MAX_BLOB).then_some(Layout {
            static_len,
            animated_len,
            static_flags,
            animated_flags,
            flag_bytes,
            len,
        })
    }
}

/// The streams a clip holds, in the order the guest evaluates them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Which {
    Static = 0,
    Animated = 1,
}

/// The three channel kinds, in the guest's evaluation order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    Rotation = 0,
    Translation = 1,
    Scale = 2,
}

/// One stream: where it starts, its header words, and per channel the
/// flagged nodes with their track (the node's rank among flagged nodes,
/// which is the index the guest's walk counts in `0x5044b4/b8/bc`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stream {
    /// Offset of the stream in the blob.
    pub base: u32,
    /// The codec byte (stream `+0`).
    pub codec: u8,
    /// `(node, track)` per channel, in node order.
    pub flagged: [Vec<(u16, u16)>; 3],
}

/// A decoded animation. Location-independent: the guest may cache the
/// blob at another address later and the clip stays valid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Clip {
    pub shape: Shape,
    pub layout: Layout,
    bytes: Box<[u8]>,
    streams: [Option<Stream>; 2],
}

impl Clip {
    /// Build from the blob the guest would read: `bytes` are
    /// `layout.len` bytes at the view's data pointer.
    pub fn decode(shape: Shape, bytes: Box<[u8]>) -> Option<Clip> {
        let layout = Layout::new(&shape)?;
        if bytes.len() != layout.len as usize {
            return None;
        }
        let stream = |base: u32, len: u32, flags: u32| -> Option<Stream> {
            if len == 0 {
                return None;
            }
            let codec = *bytes.get(base as usize)?;
            let flagged = std::array::from_fn(|c| {
                let at = (flags + c as u32 * layout.flag_bytes) as usize;
                let array = &bytes[at..at + layout.flag_bytes as usize];
                let mut list = Vec::new();
                let mut track = 0u16;
                for node in 0..u16::from(shape.nodes) {
                    let byte = array.get(usize::from(node >> 3)).copied().unwrap_or(0);
                    if byte >> (node & 7) & 1 != 0 {
                        list.push((node, track));
                        track += 1;
                    }
                }
                list
            });
            Some(Stream {
                base,
                codec,
                flagged,
            })
        };
        let streams = [
            stream(0, layout.static_len, layout.static_flags),
            stream(
                layout.static_len,
                layout.animated_len,
                layout.animated_flags,
            ),
        ];
        Some(Clip {
            shape,
            layout,
            bytes,
            streams,
        })
    }

    pub fn stream(&self, which: Which) -> Option<&Stream> {
        self.streams[which as usize].as_ref()
    }

    /// The blob, as the guest laid it out.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Bytes the clip holds, for the store's accounting.
    pub fn footprint(&self) -> usize {
        self.bytes.len()
            + self
                .streams
                .iter()
                .flatten()
                .flat_map(|s| s.flagged.iter())
                .map(|l| l.len() * 4)
                .sum::<usize>()
    }

    /// A dword at a stream-relative offset, computed with the guest's
    /// 32-bit wrap. `None` past the blob: the caller declines and lets the
    /// guest read it.
    #[inline]
    pub(crate) fn u32_at(&self, s: &Stream, offset: u32) -> Option<u32> {
        self.slice::<4>(s, offset).map(u32::from_le_bytes)
    }

    #[inline]
    pub(crate) fn slice<const N: usize>(&self, s: &Stream, offset: u32) -> Option<[u8; N]> {
        let at = s.base.wrapping_add(offset) as usize;
        self.bytes.get(at..at.checked_add(N)?)?.try_into().ok()
    }

    /// The offset of a stream-relative location from the blob start, for
    /// alignment checks against the guest address.
    #[inline]
    pub(crate) fn blob_offset(&self, s: &Stream, offset: u32) -> u32 {
        s.base.wrapping_add(offset)
    }
}
