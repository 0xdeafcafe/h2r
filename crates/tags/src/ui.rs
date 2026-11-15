//! Halo 2 user-interface tags: screen definitions (`wgit`), the per-map UI
//! globals (`wgtz`), the shared UI globals (`wigl`) and list skins (`skin`).
//!
//! Like `sound`, this module is self-contained: hand-written fixed codecs,
//! no schema/build.rs involvement, so it merges independently of other groups.
//! Every interpreted offset was read off the retail engine's UI code, not off a
//! community definition; the addresses and the evidence are in `docs/ui.md`
//! and `docs/engine-map.md`. Bytes nobody has traced stay raw.
//!
//! Model: a `wgit` screen owns *panes*; the engine builds one pane at a time.
//! A pane owns blocks of widget definitions: buttons, lists, text blocks,
//! bitmaps, model scenes, shader bitmaps and tables. Lists and tables draw
//! their items from `skin` tags, which own the same widget kinds. Animation
//! indices point into the `wigl` animation collections.
//!
//! Coordinates are 16-bit UI units with the origin at the screen centre and
//! Y pointing **up**: a rectangle's `top` is greater than its `bottom`.
//!
//! Block data can alias: a block address may point into another tag's payload
//! (two retail screens share one pane array). Blocks are therefore resolved
//! against the whole ordinary metadata region, not only the owning tag.
use crate::{Error, Result, wire::StringId};
use map::{FourCC, Map, TagId};
use std::collections::HashMap;
use std::io::{Read, Seek};
use std::rc::Rc;

pub const SCREEN: FourCC = FourCC(*b"wgit");
pub const UI_GLOBALS: FourCC = FourCC(*b"wgtz");
pub const SHARED_GLOBALS: FourCC = FourCC(*b"wigl");
pub const SKIN: FourCC = FourCC(*b"skin");
pub const STRINGS: FourCC = FourCC(*b"unic");
pub const BITMAP: FourCC = FourCC(*b"bitm");
pub const SHADER: FourCC = FourCC(*b"shad");
pub const SOUND: FourCC = FourCC(*b"snd!");
pub const LOOPING_SOUND: FourCC = FourCC(*b"lsnd");
pub const VARIANT_SETTINGS: FourCC = FourCC(*b"goof");

/// Is this one of the four groups this module decodes?
pub fn supports(group: FourCC) -> bool {
    [SCREEN, UI_GLOBALS, SHARED_GLOBALS, SKIN].contains(&group)
}

// ---------------------------------------------------------------------------
// Byte source

/// A view into a shared, immutable payload. Splitting a block into elements
/// borrows from it instead of copying, so aliased blocks cost no memory.
#[derive(Clone)]
pub struct Bytes {
    data: Rc<[u8]>,
    start: usize,
    len: usize,
    owner: Option<TagId>,
}
impl Bytes {
    /// `data[start..start + len]`, or `None` if that range is out of bounds.
    /// `owner` is the tag whose payload holds the range, when known.
    pub fn new(data: Rc<[u8]>, start: usize, len: usize, owner: Option<TagId>) -> Option<Self> {
        (start.checked_add(len)? <= data.len()).then_some(Self {
            data,
            start,
            len,
            owner,
        })
    }
    /// The tag whose payload these bytes came from, if the source knows it.
    pub fn owner(&self) -> Option<TagId> {
        self.owner
    }
}
impl std::ops::Deref for Bytes {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.data[self.start..self.start + self.len]
    }
}

/// Where tag metadata comes from. [`MapSource`] reads a cache file; tests
/// supply an in-memory image.
pub trait Source {
    /// Read `size` bytes at a metadata virtual address. The whole range must
    /// lie inside one tag payload.
    fn read(&mut self, address: u32, size: u32) -> Result<Bytes>;
    /// Group of a live tag, or `None` if the datum does not resolve.
    fn group(&self, id: TagId) -> Option<FourCC>;
    /// Payload address of a live tag, or `None` if it has no payload.
    fn address(&self, id: TagId) -> Option<u32>;
    /// The map's string-ID table, used to check every [`StringId`].
    fn strings(&self) -> &[Option<String>];
    /// Bytes of metadata this source can serve. One decode may follow at most
    /// this many bytes and `budget() / 4` elements (4 is the smallest stride),
    /// counting an aliased block every time it is followed ([`Error::Budget`]).
    fn budget(&self) -> u64;
}

/// [`Source`] over a cache file's ordinary metadata. Build it once per map and
/// decode many tags with [`MapSource::decode_tag`]: the span index is built
/// once and payloads are read lazily and cached. BSP/lightmap regions are
/// excluded because their address spaces overlap ordinary metadata.
///
/// Payload spans may nest or overlap. A virtual address maps to one file
/// offset, so every span that contains a range holds the same bytes for it;
/// a read uses the containing span with the greatest start address.
pub struct MapSource<'a, R> {
    map: &'a mut Map<R>,
    /// (address, size, tag), sorted by address.
    spans: Vec<(u32, u32, TagId)>,
    /// `max_end[i]` is the largest span end among `spans[..=i]`.
    max_end: Vec<u64>,
    budget: u64,
    cache: HashMap<TagId, Rc<[u8]>>,
}
impl<'a, R: Read + Seek> MapSource<'a, R> {
    pub fn new(map: &'a mut Map<R>) -> Self {
        let space = map.metadata_space();
        let mut spans: Vec<_> = map
            .tags()
            .iter()
            .filter_map(|t| {
                let l = t.location?;
                (Some(l.space) == space && l.size > 0).then_some((l.address, l.size, t.id))
            })
            .collect();
        spans.sort_unstable_by_key(|s| (s.0, s.1));
        let mut max_end = Vec::with_capacity(spans.len());
        let mut end = 0u64;
        for &(a, s, _) in &spans {
            end = end.max(u64::from(a) + u64::from(s));
            max_end.push(end);
        }
        let budget = spans.iter().map(|s| u64::from(s.1)).sum();
        Self {
            map,
            spans,
            max_end,
            budget,
            cache: HashMap::new(),
        }
    }
    /// The span holding `address..address + size` whole, if any.
    fn span(&self, address: u32, size: u32) -> Option<(u32, u32, TagId)> {
        span_containing(&self.spans, &self.max_end, address, size)
    }
    /// The tag whose payload contains `address`, if any.
    pub fn owner(&self, address: u32) -> Option<TagId> {
        self.span(address, 1).map(|s| s.2)
    }
    /// Number of payload spans that start inside an earlier span.
    pub fn overlapping_spans(&self) -> usize {
        (1..self.spans.len())
            .filter(|&i| self.max_end[i - 1] > u64::from(self.spans[i].0))
            .count()
    }
    pub fn map(&mut self) -> &mut Map<R> {
        self.map
    }
    pub fn map_ref(&self) -> &Map<R> {
        self.map
    }
    /// Decode one UI tag, reusing this source's index and payload cache.
    pub fn decode_tag(&mut self, id: TagId) -> Result<Decoded> {
        let group = self.map.tag(id)?.group;
        let address = self.address(id).ok_or(map::Error::NoTagData)?;
        decode(self, group, address)
    }
}
/// Span lookup over `spans` sorted by start, with `max_end` the running
/// maximum end. Walks back from the last span starting at or before
/// `address` while an earlier span could still reach it.
fn span_containing(
    spans: &[(u32, u32, TagId)],
    max_end: &[u64],
    address: u32,
    size: u32,
) -> Option<(u32, u32, TagId)> {
    let end = u64::from(address) + u64::from(size);
    let i = spans.partition_point(|s| s.0 <= address);
    (0..i)
        .rev()
        .take_while(|&j| max_end[j] > u64::from(address))
        .map(|j| spans[j])
        .find(|&(a, s, _)| end <= u64::from(a) + u64::from(s))
}
impl<R: Read + Seek> Source for MapSource<'_, R> {
    fn read(&mut self, address: u32, size: u32) -> Result<Bytes> {
        let bounds = Error::Bounds {
            address,
            size: size.into(),
        };
        let Some((start, _, id)) = self.span(address, size) else {
            return Err(bounds);
        };
        let data = match self.cache.get(&id) {
            Some(d) => d.clone(),
            None => {
                let d: Rc<[u8]> = self.map.read_tag_bytes(id)?.into();
                self.cache.insert(id, d.clone());
                d
            }
        };
        Bytes::new(data, (address - start) as usize, size as usize, Some(id)).ok_or(bounds)
    }
    fn group(&self, id: TagId) -> Option<FourCC> {
        self.map.tag(id).ok().map(|t| t.group)
    }
    fn address(&self, id: TagId) -> Option<u32> {
        self.map.tag(id).ok()?.location.map(|l| l.address)
    }
    fn strings(&self) -> &[Option<String>] {
        self.map.strings()
    }
    fn budget(&self) -> u64 {
        self.budget
    }
}

// ---------------------------------------------------------------------------
// Shared value types

/// An 8-byte tag reference: group FourCC then full datum. A null datum means
/// "no tag"; retail data keeps a group on some null references.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TagRef {
    pub group: FourCC,
    pub id: Option<TagId>,
}
/// A rectangle in UI units, Y up (`top >= bottom`), origin at screen centre.
/// Wire order is top, left, bottom, right (engine bounding-box code at
/// `0x002bafa4` takes max of `top`, min of `left`, min of `bottom`, max of `right`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Rect {
    pub top: i16,
    pub left: i16,
    pub bottom: i16,
    pub right: i16,
}
impl Rect {
    pub fn width(self) -> i32 {
        i32::from(self.right) - i32::from(self.left)
    }
    pub fn height(self) -> i32 {
        i32::from(self.top) - i32::from(self.bottom)
    }
}
/// A point in UI units: X then Y, Y up.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Point {
    pub x: i16,
    pub y: i16,
}
/// A 1-based animation index as stored. Zero means none; otherwise it selects
/// `SharedGlobals::animations[value - 1]` (engine `0x0022e65e`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct AnimationRef(pub u16);
impl AnimationRef {
    pub fn index(self) -> Option<usize> {
        usize::from(self.0).checked_sub(1)
    }
}

/// Raw block descriptor: element count then virtual address.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Block {
    pub count: u32,
    pub address: u32,
}

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn i16_at(b: &[u8], o: usize) -> i16 {
    u16_at(b, o) as i16
}
fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().expect("four bytes"))
}
fn f32_at(b: &[u8], o: usize) -> f32 {
    f32::from_bits(u32_at(b, o))
}
fn f32s<const N: usize>(b: &[u8], o: usize) -> [f32; N] {
    std::array::from_fn(|i| f32_at(b, o + i * 4))
}
fn raw<const N: usize>(b: &[u8], o: usize) -> [u8; N] {
    b[o..o + N].try_into().expect("slice length")
}
fn rect_at(b: &[u8], o: usize) -> Rect {
    Rect {
        top: i16_at(b, o),
        left: i16_at(b, o + 2),
        bottom: i16_at(b, o + 4),
        right: i16_at(b, o + 6),
    }
}
fn point_at(b: &[u8], o: usize) -> Point {
    Point {
        x: i16_at(b, o),
        y: i16_at(b, o + 2),
    }
}
fn block_at(b: &[u8], o: usize) -> Block {
    Block {
        count: u32_at(b, o),
        address: u32_at(b, o + 4),
    }
}
fn field<T>(record: &'static str, name: &'static str, r: Result<T>) -> Result<T> {
    r.map_err(|e| Error::Field {
        record,
        field: name,
        source: Box::new(e),
    })
}

/// Decoding state: the byte source, coverage counters and the remaining
/// budget. One `Ctx` spans a whole decode, including nested skins resolved by
/// [`resolve_skin`], so aliasing cannot multiply work past the budget.
struct Ctx<'s> {
    src: &'s mut dyn Source,
    stats: Stats,
    /// Tag owning the root being decoded, to recognise aliased blocks.
    root_owner: Option<TagId>,
    bytes_left: u64,
    elements_left: u64,
}
impl<'s> Ctx<'s> {
    fn new(src: &'s mut dyn Source) -> Self {
        let budget = src.budget();
        Self {
            src,
            stats: Stats::default(),
            root_owner: None,
            bytes_left: budget,
            elements_left: budget / 4,
        }
    }
    /// Charge a read of `size` bytes holding `count` elements to the budget.
    fn charge(&mut self, size: u64, count: u64) -> Result<()> {
        let limit = self.src.budget();
        self.bytes_left = self.bytes_left.checked_sub(size).ok_or(Error::Budget {
            kind: "bytes",
            limit,
        })?;
        self.elements_left = self.elements_left.checked_sub(count).ok_or(Error::Budget {
            kind: "elements",
            limit: limit / 4,
        })?;
        self.stats.bytes += size;
        Ok(())
    }
    /// Read a tag root of `size` bytes at `address`.
    fn root(&mut self, address: u32, size: usize) -> Result<Bytes> {
        self.charge(size as u64, 1)?;
        let b = self.src.read(address, size as u32)?;
        self.root_owner = b.owner();
        Ok(b)
    }
    /// Follow a block descriptor: check it, charge it and return its bytes.
    /// Elements are `stride`-byte slices of the result.
    fn elements(&mut self, b: &[u8], at: usize, stride: u32) -> Result<Bytes> {
        let blk = block_at(b, at);
        self.stats.blocks += 1;
        if blk.count == 0 {
            return Ok(Bytes::new(Rc::from([]), 0, 0, None).expect("empty"));
        }
        let size = u64::from(blk.count) * u64::from(stride);
        let size32 = u32::try_from(size).map_err(|_| Error::Bounds {
            address: blk.address,
            size,
        })?;
        if !blk.address.is_multiple_of(4) {
            return Err(Error::Invalid("misaligned block address"));
        }
        self.charge(size, blk.count.into())?;
        let bytes = self.src.read(blk.address, size32)?;
        if bytes.owner().is_some() && bytes.owner() != self.root_owner {
            self.stats.aliased_blocks += 1;
            // Retail keyframe blocks alias into non-UI tags too (3 on the
            // disc), so the target group is counted, not enforced.
            if !bytes
                .owner()
                .and_then(|o| self.src.group(o))
                .is_some_and(supports)
            {
                self.stats.foreign_aliases += 1;
            }
        }
        self.stats.nonempty_blocks += 1;
        self.stats.elements += blk.count as usize;
        Ok(bytes)
    }
    /// Decode a block of `T` records with field context.
    fn records<T>(
        &mut self,
        b: &[u8],
        at: usize,
        stride: u32,
        rec: &'static str,
        name: &'static str,
        f: impl Fn(&mut Self, &[u8]) -> Result<T>,
    ) -> Result<Vec<T>> {
        let r = self.elements(b, at, stride).and_then(|es| {
            es.chunks_exact(stride as usize)
                .map(|e| f(self, e))
                .collect()
        });
        field(rec, name, r)
    }
    /// A block the engine never reads in the reviewed paths and the retail
    /// disc never populates: refuse a populated one instead of guessing.
    fn empty(&mut self, b: &[u8], at: usize, rec: &'static str, name: &'static str) -> Result<()> {
        self.stats.blocks += 1;
        if block_at(b, at).count != 0 {
            return Err(Error::Field {
                record: rec,
                field: name,
                source: Box::new(Error::UnsupportedBlock(name)),
            });
        }
        Ok(())
    }
    fn tag_ref(
        &mut self,
        b: &[u8],
        at: usize,
        expect: &[FourCC],
        rec: &'static str,
        name: &'static str,
    ) -> Result<TagRef> {
        let group = FourCC::from_u32(u32_at(b, at));
        let datum = u32_at(b, at + 4);
        self.stats.references += 1;
        if datum == u32::MAX {
            return Ok(TagRef { group, id: None });
        }
        let id = TagId(datum);
        let check = || -> Result<()> {
            let actual = self
                .src
                .group(id)
                .ok_or(Error::Invalid("tag reference does not resolve"))?;
            if actual != group {
                return Err(Error::Invalid(
                    "tag reference group differs from the referenced tag",
                ));
            }
            if !expect.is_empty() && !expect.contains(&actual) {
                return Err(Error::UnsupportedGroup(actual));
            }
            Ok(())
        };
        field(rec, name, check())?;
        Ok(TagRef {
            group,
            id: Some(id),
        })
    }
    fn string_id(
        &mut self,
        b: &[u8],
        at: usize,
        rec: &'static str,
        name: &'static str,
    ) -> Result<StringId> {
        let s = StringId(u32_at(b, at));
        self.stats.string_ids += 1;
        field(rec, name, s.resolve(self.src.strings()).map(|_| s))
    }
}

/// Structural coverage counters for validation reports.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub blocks: usize,
    pub nonempty_blocks: usize,
    pub elements: usize,
    pub references: usize,
    pub string_ids: usize,
    /// Bytes charged to the budget (roots and followed blocks).
    pub bytes: u64,
    /// Nonempty blocks whose data lies in another tag's payload.
    pub aliased_blocks: usize,
    /// Of those, blocks whose data lies in a tag outside the four UI groups.
    pub foreign_aliases: usize,
}

// ---------------------------------------------------------------------------
// Animations (wigl +0x120 and skin +0x14)

/// One animation keyframe, `0x14` bytes. Keyframes are evenly spaced over the
/// owning [`Animation::period_ms`]; the engine interpolates alpha and offset
/// linearly between neighbours (`0x0022e3cd`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Keyframe {
    /// +0, not read by the interpolator.
    pub unknown_00: u32,
    /// +4, widget alpha multiplier.
    pub alpha: f32,
    /// +8, offset added to the widget position (x, y, z).
    pub offset: [f32; 3],
}
/// A keyframe track: total period in milliseconds, then its keyframes.
#[derive(Clone, Debug, PartialEq)]
pub struct Animation {
    pub period_ms: u32,
    pub keyframes: Vec<Keyframe>,
}
fn keyframe(_: &mut Ctx, e: &[u8]) -> Result<Keyframe> {
    Ok(Keyframe {
        unknown_00: u32_at(e, 0),
        alpha: f32_at(e, 4),
        offset: f32s(e, 8),
    })
}
fn animation(
    c: &mut Ctx,
    b: &[u8],
    at: usize,
    rec: &'static str,
    name: &'static str,
) -> Result<Animation> {
    Ok(Animation {
        period_ms: u32_at(b, at),
        keyframes: c.records(b, at + 4, 0x14, rec, name, keyframe)?,
    })
}

/// `wigl` animation collection, `0x2c` bytes, selected by a widget's
/// [`AnimationRef`]. The engine picks a track by transition kind
/// (`0x0022e65e`): kind 0 plays `primary` forwards, 2 plays it backwards,
/// 3 plays `secondary` forwards, 1 backwards, and kind 4 loops `ambient`.
#[derive(Clone, Debug, PartialEq)]
pub struct AnimationCollection {
    pub flags: u32,
    pub primary: Animation,
    pub secondary: Animation,
    pub ambient: Animation,
    /// +0x20, ambient loop style; the engine maps 0→0, 1→3, 2→1, 3→0 into its
    /// playback mode. Mode meanings are not traced.
    pub ambient_loop: i16,
    pub unknown_22: u16,
}
const ANIMATION: &str = "AnimationCollection";
fn animation_collection(c: &mut Ctx, e: &[u8]) -> Result<AnimationCollection> {
    Ok(AnimationCollection {
        flags: u32_at(e, 0),
        primary: animation(c, e, 4, ANIMATION, "primary")?,
        secondary: animation(c, e, 0x10, ANIMATION, "secondary")?,
        ambient: Animation {
            period_ms: u32_at(e, 0x1c),
            keyframes: c.records(e, 0x24, 0x14, ANIMATION, "ambient", keyframe)?,
        },
        ambient_loop: i16_at(e, 0x20),
        unknown_22: u16_at(e, 0x22),
    })
}

// ---------------------------------------------------------------------------
// Widget definitions

/// Justification selected by text/button flag bits 0 and 1 (`0x00253765`,
/// `0x00253cff`): bit 0 gives mode 0, else bit 1 gives mode 1, else mode 2.
/// The values go to the font renderer; their visual meaning is not traced.
pub fn justification(flags: u32) -> u8 {
    if flags & 1 != 0 {
        0
    } else if flags & 2 != 0 {
        1
    } else {
        2
    }
}

/// Pane button, `0x3c` bytes; the runtime list item (widget type 3) reads it
/// by index (`0x00253cc8`, `0x00253cff`, draw `0x00253ea8`).
#[derive(Clone, Debug, PartialEq)]
pub struct Button {
    /// +0: bits 0/1 justification, bit 2 a font-renderer option.
    pub flags: u32,
    pub animation: AnimationRef,
    pub unknown_06: u16,
    pub unknown_08: u16,
    /// +0xa, font index.
    pub font: i16,
    pub unknown_0c: u32,
    /// +0x10, text colour (r, g, b).
    pub text_color: [f32; 3],
    pub rect: Rect,
    /// +0x24, optional bitmap drawn at `rect` top-left plus `bitmap_offset`.
    pub bitmap: TagRef,
    pub bitmap_offset: Point,
    /// +0x30, label, resolved through the screen's `unic` tag.
    pub string: StringId,
    /// +0x34, draw-order key (`0x002356d8` sorts by depth, then this).
    pub render_depth_bias: i16,
    pub unknown_36: u16,
    /// +0x38: bits 0 and 1 copied to the item's runtime flags.
    pub flags_38: u32,
}
fn button(c: &mut Ctx, e: &[u8]) -> Result<Button> {
    Ok(Button {
        flags: u32_at(e, 0),
        animation: AnimationRef(u16_at(e, 4)),
        unknown_06: u16_at(e, 6),
        unknown_08: u16_at(e, 8),
        font: i16_at(e, 0xa),
        unknown_0c: u32_at(e, 0xc),
        text_color: f32s(e, 0x10),
        rect: rect_at(e, 0x1c),
        bitmap: c.tag_ref(e, 0x24, &[BITMAP], "Button", "bitmap")?,
        bitmap_offset: point_at(e, 0x2c),
        string: c.string_id(e, 0x30, "Button", "string")?,
        render_depth_bias: i16_at(e, 0x34),
        unknown_36: u16_at(e, 0x36),
        flags_38: u32_at(e, 0x38),
    })
}

/// Pane list, `0x18` bytes. The list widget reads only the pane's first list
/// block (`0x0024bbf3`) and lays out `visible_items` items from the skin
/// (`0x0024bc12`).
#[derive(Clone, Debug, PartialEq)]
pub struct List {
    /// +0, bit 0 copied to the list widget (+0x7c).
    pub flags: u32,
    /// +4, index into [`SharedGlobals::skins`].
    pub skin: i16,
    pub visible_items: i16,
    /// +8, top-left of the first item.
    pub anchor: Point,
    pub animation: AnimationRef,
    /// +0xe, copied to every item widget (+0x74); retail values 0, 125, 250.
    pub item_value: i16,
}
fn list(c: &mut Ctx, e: &[u8]) -> Result<List> {
    c.empty(e, 0x10, "List", "unknown_10")?;
    Ok(List {
        flags: u32_at(e, 0),
        skin: i16_at(e, 4),
        visible_items: i16_at(e, 6),
        anchor: point_at(e, 8),
        animation: AnimationRef(u16_at(e, 0xc)),
        item_value: i16_at(e, 0xe),
    })
}

/// Text block, `0x2c` bytes (widget type 6; constructor `0x0022f9bf`,
/// initialisation `0x00253765`).
#[derive(Clone, Debug, PartialEq)]
pub struct TextBlock {
    /// +0: bits 0/1 justification, bit 2 a font option, bit 3 forces font 1
    /// and a `wigl` +0x4c text parameter, bit 4 selects the larger widget.
    pub flags: u32,
    pub animation: AnimationRef,
    /// +6, copied to the widget (+0x70).
    pub unknown_06: i16,
    pub unknown_08: u16,
    pub font: i16,
    pub unknown_0c: u32,
    pub text_color: [f32; 3],
    pub rect: Rect,
    /// +0x24, resolved through the screen's `unic` tag (`0x00230134`).
    pub string: StringId,
    pub render_depth_bias: i16,
    pub unknown_2a: u16,
}
fn text_block(c: &mut Ctx, e: &[u8]) -> Result<TextBlock> {
    Ok(TextBlock {
        flags: u32_at(e, 0),
        animation: AnimationRef(u16_at(e, 4)),
        unknown_06: i16_at(e, 6),
        unknown_08: u16_at(e, 8),
        font: i16_at(e, 0xa),
        unknown_0c: u32_at(e, 0xc),
        text_color: f32s(e, 0x10),
        rect: rect_at(e, 0x1c),
        string: c.string_id(e, 0x24, "TextBlock", "string")?,
        render_depth_bias: i16_at(e, 0x28),
        unknown_2a: u16_at(e, 0x2a),
    })
}

/// Bitmap block, `0x38` bytes (widget type 8; constructor `0x002b01eb`,
/// update `0x002b0328`, draw `0x002b0565` / `0x002b07e2`).
#[derive(Clone, Debug, PartialEq)]
pub struct BitmapBlock {
    /// +0: bit 2 makes this a meter: `extent` bounds it and the drawn width
    /// scales with a runtime 0..1 value.
    pub flags: u32,
    pub animation: AnimationRef,
    pub unknown_06: u16,
    /// +8: value 1 switches the draw call to a second mode (and clamps the
    /// texture-coordinate scale to 1). Other values are not distinguished.
    pub draw_mode: i16,
    /// +0xa, bitmap image used for the size; ignored if out of range.
    pub image: i16,
    pub top_left: Point,
    /// +0x10, texture-coordinate scroll per second (u, v), wrapped to [0, 1).
    pub scroll_rate: [f32; 2],
    pub bitmap: TagRef,
    pub render_depth_bias: i16,
    pub unknown_22: u16,
    /// +0x24, sprite frames per second; below 0.001 the image is static.
    pub sprite_fps: f32,
    /// +0x28, meter right/bottom (x = right, y = bottom).
    pub extent: Point,
    pub unknown_2c: [u8; 12],
}
impl BitmapBlock {
    pub const METER: u32 = 4;
    /// The initial rectangle as the constructor computes it. `image_size` is
    /// `(width, height)` of the selected bitmap image, if one resolves.
    pub fn rect(&self, image_size: Option<(i16, i16)>) -> Rect {
        let Point { x, y } = self.top_left;
        if self.flags & Self::METER != 0 {
            // Width starts at zero; the update grows `right` by the meter value.
            return Rect {
                top: y,
                left: x,
                bottom: self.extent.y,
                right: x,
            };
        }
        match image_size {
            Some((w, h)) => Rect {
                top: y,
                left: x,
                bottom: y.wrapping_sub(h),
                right: x.wrapping_add(w),
            },
            None => Rect::default(),
        }
    }
}
fn bitmap_block(c: &mut Ctx, e: &[u8]) -> Result<BitmapBlock> {
    Ok(BitmapBlock {
        flags: u32_at(e, 0),
        animation: AnimationRef(u16_at(e, 4)),
        unknown_06: u16_at(e, 6),
        draw_mode: i16_at(e, 8),
        image: i16_at(e, 0xa),
        top_left: point_at(e, 0xc),
        scroll_rate: f32s(e, 0x10),
        bitmap: c.tag_ref(e, 0x18, &[BITMAP], "BitmapBlock", "bitmap")?,
        render_depth_bias: i16_at(e, 0x20),
        unknown_22: u16_at(e, 0x22),
        sprite_fps: f32_at(e, 0x24),
        extent: point_at(e, 0x28),
        unknown_2c: raw(e, 0x2c),
    })
}

/// A scenario object name, `0x20` bytes, NUL padded.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ObjectName(pub [u8; 0x20]);
impl ObjectName {
    pub fn as_str(&self) -> String {
        let end = self.0.iter().position(|&b| b == 0).unwrap_or(self.0.len());
        String::from_utf8_lossy(&self.0[..end]).into_owned()
    }
}
impl std::fmt::Debug for ObjectName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.as_str())
    }
}
/// Model scene, `0x4c` bytes (widget type 7; constructor `0x002b0b5e`,
/// draw `0x002b0baa`): renders scenario objects into a viewport.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelScene {
    pub flags: u32,
    pub animation: AnimationRef,
    pub unknown_06: u16,
    pub render_depth_bias: i16,
    pub unknown_0a: u16,
    /// +0xc, scenario objects looked up by name (resolver `0x000bb760`).
    pub objects: Vec<ObjectName>,
    /// +0x14, a second name list (resolver `0x000c38e0`).
    pub secondary_objects: Vec<ObjectName>,
    /// +0x1c, multiplied with a widget vector and subtracted from `camera`.
    pub camera_scale: [f32; 3],
    pub camera: [f32; 3],
    /// +0x34, clamped to at least 1 and scaled before use.
    pub fov: f32,
    pub viewport: Rect,
    pub unknown_40: [u8; 12],
}
fn object_name(_: &mut Ctx, e: &[u8]) -> Result<ObjectName> {
    Ok(ObjectName(raw(e, 0)))
}
fn model_scene(c: &mut Ctx, e: &[u8]) -> Result<ModelScene> {
    Ok(ModelScene {
        flags: u32_at(e, 0),
        animation: AnimationRef(u16_at(e, 4)),
        unknown_06: u16_at(e, 6),
        render_depth_bias: i16_at(e, 8),
        unknown_0a: u16_at(e, 0xa),
        objects: c.records(e, 0xc, 0x20, "ModelScene", "objects", object_name)?,
        secondary_objects: c.records(
            e,
            0x14,
            0x20,
            "ModelScene",
            "secondary_objects",
            object_name,
        )?,
        camera_scale: f32s(e, 0x1c),
        camera: f32s(e, 0x28),
        fov: f32_at(e, 0x34),
        viewport: rect_at(e, 0x38),
        unknown_40: raw(e, 0x40),
    })
}

/// Bitmap drawn through a shader, `0x24` bytes (widget type 9; constructor
/// `0x002b1111`, draw `0x002b11c9`).
#[derive(Clone, Debug, PartialEq)]
pub struct ShaderBitmap {
    pub flags: u32,
    /// +4. Unlike the other widgets the constructor stores this without
    /// subtracting one; kept raw rather than guessing which is intended.
    pub animation_raw: u16,
    pub unknown_06: u16,
    pub render_depth_bias: i16,
    /// +0xa, passed to the draw call with the shader.
    pub unknown_0a: i16,
    pub bitmap: TagRef,
    pub shader: TagRef,
    pub rect: Rect,
}
fn shader_bitmap(c: &mut Ctx, e: &[u8]) -> Result<ShaderBitmap> {
    Ok(ShaderBitmap {
        flags: u32_at(e, 0),
        animation_raw: u16_at(e, 4),
        unknown_06: u16_at(e, 6),
        render_depth_bias: i16_at(e, 8),
        unknown_0a: i16_at(e, 0xa),
        bitmap: c.tag_ref(e, 0xc, &[BITMAP], "ShaderBitmap", "bitmap")?,
        shader: c.tag_ref(e, 0x14, &[SHADER], "ShaderBitmap", "shader")?,
        rect: rect_at(e, 0x1c),
    })
}

/// Grid of skinned cells, `0x18` bytes (widget type 10; constructors
/// `0x0022fa7d`, `0x002afd97`, layout `0x002afdbc`). One widget per cell.
#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    pub flags: u32,
    pub skin: TagRef,
    pub anchor: Point,
    /// +0x10: zero lays cells out by `rows`, nonzero by `columns`.
    pub order: i8,
    pub cells: i8,
    pub columns: i8,
    pub rows: i8,
    /// +0x14, Y step per cell.
    pub row_spacing: i16,
    /// +0x16, X step per cell.
    pub column_spacing: i16,
}
impl Table {
    /// Grid position `(a, b)` of cell `i` as `0x002afdbc` computes it, or
    /// `None` when the engine skips the cell. Both counts are clamped to at
    /// least 1; with `order` nonzero `(a, b) = (i / columns, i % columns)`,
    /// else `(i % rows, i / rows)`; the cell is placed only if
    /// `0 <= a < rows` and `0 <= b < columns`.
    pub fn cell_grid(&self, i: i32) -> Option<(i32, i32)> {
        let cols = i32::from(self.columns).max(1);
        let rows = i32::from(self.rows).max(1);
        let (a, b) = if self.order != 0 {
            (i / cols, i % cols)
        } else {
            (i % rows, i / rows)
        };
        ((0..rows).contains(&a) && (0..cols).contains(&b)).then_some((a, b))
    }
    /// Top-left of cell `i` relative to the table's parent: the anchor plus
    /// `column_spacing * a` in X and `row_spacing * b` in Y for the
    /// [`cell_grid`](Self::cell_grid) position. `None` if the engine skips
    /// the cell or the result does not fit in 16 bits (the engine's 16-bit
    /// arithmetic would wrap there).
    pub fn cell_origin(&self, i: i32) -> Option<Point> {
        let (a, b) = self.cell_grid(i)?;
        let x = i32::from(self.column_spacing)
            .checked_mul(a)?
            .checked_add(self.anchor.x.into())?;
        let y = i32::from(self.row_spacing)
            .checked_mul(b)?
            .checked_add(self.anchor.y.into())?;
        Some(Point {
            x: x.try_into().ok()?,
            y: y.try_into().ok()?,
        })
    }
}
fn table(c: &mut Ctx, e: &[u8]) -> Result<Table> {
    Ok(Table {
        flags: u32_at(e, 0),
        skin: c.tag_ref(e, 4, &[SKIN], "Table", "skin")?,
        anchor: point_at(e, 0xc),
        order: e[0x10] as i8,
        cells: e[0x11] as i8,
        columns: e[0x12] as i8,
        rows: e[0x13] as i8,
        row_spacing: i16_at(e, 0x14),
        column_spacing: i16_at(e, 0x16),
    })
}

/// One pane of a screen, `0x4c` bytes (`0x0022f899`, builder `0x0022fc08`).
#[derive(Clone, Debug, PartialEq)]
pub struct Pane {
    pub flags: u16,
    /// +2, copied to the screen widget when the pane is shown.
    pub animation: AnimationRef,
    pub buttons: Vec<Button>,
    pub lists: Vec<List>,
    pub text_blocks: Vec<TextBlock>,
    pub bitmaps: Vec<BitmapBlock>,
    pub model_scenes: Vec<ModelScene>,
    pub shader_bitmaps: Vec<ShaderBitmap>,
    pub tables: Vec<Table>,
}
const PANE: &str = "Pane";
fn pane(c: &mut Ctx, e: &[u8]) -> Result<Pane> {
    c.empty(e, 0x14, PANE, "unknown_14")?;
    c.empty(e, 0x34, PANE, "unknown_34")?;
    Ok(Pane {
        flags: u16_at(e, 0),
        animation: AnimationRef(u16_at(e, 2)),
        buttons: c.records(e, 4, 0x3c, PANE, "buttons", button)?,
        lists: c.records(e, 0xc, 0x18, PANE, "lists", list)?,
        text_blocks: c.records(e, 0x1c, 0x2c, PANE, "text_blocks", text_block)?,
        bitmaps: c.records(e, 0x24, 0x38, PANE, "bitmaps", bitmap_block)?,
        model_scenes: c.records(e, 0x2c, 0x4c, PANE, "model_scenes", model_scene)?,
        shader_bitmaps: c.records(e, 0x3c, 0x24, PANE, "shader_bitmaps", shader_bitmap)?,
        tables: c.records(e, 0x44, 0x18, PANE, "tables", table)?,
    })
}

/// A list of string IDs addressed as (list, item) by screen code (`0x00230195`).
#[derive(Clone, Debug, PartialEq)]
pub struct StringList {
    pub unknown_00: u32,
    pub strings: Vec<StringId>,
}
fn string_list(c: &mut Ctx, e: &[u8]) -> Result<StringList> {
    let strings = c.records(e, 4, 4, "StringList", "strings", |c, i| {
        c.string_id(i, 0, "StringList", "string")
    })?;
    Ok(StringList {
        unknown_00: u32_at(e, 0),
        strings,
    })
}
fn tag_ref_element(
    expect: FourCC,
    rec: &'static str,
) -> impl Fn(&mut Ctx, &[u8]) -> Result<TagRef> {
    move |c, e| c.tag_ref(e, 0, &[expect], rec, "element")
}

/// `wgit` screen definition, root `0x68` bytes.
#[derive(Clone, Debug, PartialEq)]
pub struct Screen {
    /// +0: bit 2 hides the header text; bits 4, 3 and 0 pick `wigl` header
    /// layouts 1, 2 and 3 (else 0) (`0x0022fda6`); bit 5 is copied to the
    /// screen widget (+0x5f4). Bit 1 builds the first pane of a multi-pane
    /// screen up front (`0x0022f8df`).
    pub flags: u32,
    /// +4, the identifier `wgtz` lookups match (`0x00148098`).
    pub screen_id: i16,
    pub unknown_06: i16,
    /// +8, four floats; looks like a colour, but no reader was found.
    pub unknown_08: [f32; 4],
    /// +0x18, string table for every string ID in this screen.
    pub strings: TagRef,
    pub panes: Vec<Pane>,
    /// +0x28, 1-based index into [`SharedGlobals::shape_groups`] (`0x0022facd`).
    pub shape_group: u16,
    pub unknown_2a: u16,
    /// +0x2c, header text.
    pub header: StringId,
    pub string_lists: Vec<StringList>,
    /// +0x38, bitmaps loaded with the screen (`0x0022f28c`, `0x0022f672`).
    pub preload_bitmaps: Vec<TagRef>,
    pub unknown_40: [u8; 0x28],
}
impl Screen {
    pub const SIZE: usize = 0x68;
    pub const NO_HEADER: u32 = 4;
    /// The `wigl` header layout slot this screen's flags select.
    pub fn header_layout(&self) -> usize {
        match self.flags {
            f if f & 1 != 0 => 3,
            f if f & 8 != 0 => 2,
            f if f & 0x10 != 0 => 1,
            _ => 0,
        }
    }
}
const SCREEN_REC: &str = "Screen";
fn screen(c: &mut Ctx, b: &[u8]) -> Result<Screen> {
    Ok(Screen {
        flags: u32_at(b, 0),
        screen_id: i16_at(b, 4),
        unknown_06: i16_at(b, 6),
        unknown_08: f32s(b, 8),
        strings: c.tag_ref(b, 0x18, &[STRINGS], SCREEN_REC, "strings")?,
        panes: c.records(b, 0x20, 0x4c, SCREEN_REC, "panes", pane)?,
        shape_group: u16_at(b, 0x28),
        unknown_2a: u16_at(b, 0x2a),
        header: c.string_id(b, 0x2c, SCREEN_REC, "header")?,
        string_lists: c.records(b, 0x30, 0xc, SCREEN_REC, "string_lists", string_list)?,
        preload_bitmaps: c.records(
            b,
            0x38,
            8,
            SCREEN_REC,
            "preload_bitmaps",
            tag_ref_element(BITMAP, SCREEN_REC),
        )?,
        unknown_40: raw(b, 0x40),
    })
}

/// Per-item animation in a skin, `0x10` bytes (`0x0024bd3b`).
#[derive(Clone, Debug, PartialEq)]
pub struct ItemAnimation {
    pub unknown_00: u32,
    pub animation: Animation,
}
fn item_animation(c: &mut Ctx, e: &[u8]) -> Result<ItemAnimation> {
    Ok(ItemAnimation {
        unknown_00: u32_at(e, 0),
        animation: animation(c, e, 4, "ItemAnimation", "keyframes")?,
    })
}

/// `skin` list skin, root `0x3c` bytes: the widgets drawn for one list item
/// or table cell, relative to the item's origin (`0x002bacbc`).
#[derive(Clone, Debug, PartialEq)]
pub struct Skin {
    pub flags: u32,
    pub bitmap: TagRef,
    /// +0xc, two points not followed in the reviewed paths.
    pub unknown_0c: [Point; 2],
    pub item_animations: Vec<ItemAnimation>,
    pub text_blocks: Vec<TextBlock>,
    pub bitmaps: Vec<BitmapBlock>,
    pub shader_bitmaps: Vec<ShaderBitmap>,
    pub tables: Vec<Table>,
}
impl Skin {
    pub const SIZE: usize = 0x3c;
}
fn skin(c: &mut Ctx, b: &[u8]) -> Result<Skin> {
    Ok(Skin {
        flags: u32_at(b, 0),
        bitmap: c.tag_ref(b, 4, &[BITMAP], "Skin", "bitmap")?,
        unknown_0c: [point_at(b, 0xc), point_at(b, 0x10)],
        item_animations: c.records(b, 0x14, 0x10, "Skin", "item_animations", item_animation)?,
        text_blocks: c.records(b, 0x1c, 0x2c, "Skin", "text_blocks", text_block)?,
        bitmaps: c.records(b, 0x24, 0x38, "Skin", "bitmaps", bitmap_block)?,
        shader_bitmaps: c.records(b, 0x2c, 0x24, "Skin", "shader_bitmaps", shader_bitmap)?,
        tables: c.records(b, 0x34, 0x18, "Skin", "tables", table)?,
    })
}

/// `wgtz` UI globals, `0x20` bytes. Selected from `matg`'s interface block by
/// scenario type (`0x001482e8`).
#[derive(Clone, Debug, PartialEq)]
pub struct UiGlobals {
    pub shared: TagRef,
    /// +8, every screen, searched by [`Screen::screen_id`] (`0x00148098`).
    pub screens: Vec<TagRef>,
    /// +0x10 (`0x00236a7f`).
    pub variant_settings: TagRef,
    pub strings: TagRef,
}
impl UiGlobals {
    pub const SIZE: usize = 0x20;
}
fn ui_globals(c: &mut Ctx, b: &[u8]) -> Result<UiGlobals> {
    Ok(UiGlobals {
        shared: c.tag_ref(b, 0, &[SHARED_GLOBALS], "UiGlobals", "shared")?,
        screens: c.records(
            b,
            8,
            8,
            "UiGlobals",
            "screens",
            tag_ref_element(SCREEN, "UiGlobals"),
        )?,
        variant_settings: c.tag_ref(
            b,
            0x10,
            &[VARIANT_SETTINGS],
            "UiGlobals",
            "variant_settings",
        )?,
        strings: c.tag_ref(b, 0x18, &[STRINGS], "UiGlobals", "strings")?,
    })
}

/// `wigl` shape group, `0x18` bytes, chosen by [`Screen::shape_group`].
#[derive(Clone, Debug, PartialEq)]
pub struct ShapeGroup {
    pub unknown_00: [u8; 8],
    pub model_scenes: Vec<ModelScene>,
    pub bitmaps: Vec<BitmapBlock>,
}
fn shape_group(c: &mut Ctx, e: &[u8]) -> Result<ShapeGroup> {
    Ok(ShapeGroup {
        unknown_00: raw(e, 0),
        model_scenes: c.records(e, 8, 0x4c, "ShapeGroup", "model_scenes", model_scene)?,
        bitmaps: c.records(e, 0x10, 0x38, "ShapeGroup", "bitmaps", bitmap_block)?,
    })
}

/// Header placement for one layout slot: the header text rectangle, then a
/// second rectangle used for the screen's other built-in text widget.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct HeaderLayout {
    pub header: Rect,
    pub secondary: Rect,
}

/// `wigl` shared UI globals, root `0x1c4` bytes. Only fields with an engine
/// reader are interpreted; the rest stay raw.
#[derive(Clone, Debug, PartialEq)]
pub struct SharedGlobals {
    pub unknown_000: Vec<u8>,
    /// +0x4c, applied to text blocks with flag bit 3 (`0x00253765`).
    pub text_parameter: f32,
    /// +0x50, ARGB fill colour of meter bitmaps (`0x002b07e2`).
    pub meter_color: [f32; 4],
    pub unknown_060: [u8; 0x28],
    /// +0x88, read by `0x0023661f`; layout not reviewed. Populated on the
    /// disc, so it is kept as an **untrusted** raw descriptor: it is neither
    /// followed nor bounds-checked, and its count and address may be garbage.
    pub unknown_088: Block,
    /// +0x90, UI sounds by index (`0x00236299`).
    pub sounds: [TagRef; 16],
    /// +0x110 (`0x00236235`).
    pub bitmap: TagRef,
    /// +0x118 (`0x0023620d`).
    pub strings: TagRef,
    pub animations: Vec<AnimationCollection>,
    pub shape_groups: Vec<ShapeGroup>,
    /// +0x130, no reader found. Populated on the disc; an **untrusted** raw
    /// descriptor like `unknown_088`.
    pub unknown_130: Block,
    /// +0x138, list skins by [`List::skin`] (`0x0014837a`).
    pub skins: Vec<TagRef>,
    pub strings_140: TagRef,
    /// +0x148 (`0x0022d456`).
    pub strings_148: TagRef,
    pub reference_150: TagRef,
    /// +0x158, read by `0x00149ead`; layout not reviewed. Populated on the
    /// disc; an **untrusted** raw descriptor like `unknown_088`.
    pub unknown_158: Block,
    /// +0x160, header font per layout slot.
    pub header_fonts: [i16; 4],
    /// +0x168, header text colour (a, r, g, b).
    pub header_color: [f32; 4],
    /// +0x178, one per layout slot (see [`Screen::header_layout`]).
    pub header_layouts: [HeaderLayout; 4],
    /// +0x1b8 (`0x00235b46`).
    pub looping_sound: TagRef,
    pub unknown_1c0: u32,
}
impl SharedGlobals {
    pub const SIZE: usize = 0x1c4;
    /// The animation collection a widget's [`AnimationRef`] selects; `None`
    /// for "no animation" or an index past the list.
    pub fn animation(&self, r: AnimationRef) -> Option<&AnimationCollection> {
        self.animations.get(r.index()?)
    }
    /// The skin a [`List::skin`] index selects, if in range.
    pub fn skin(&self, index: i16) -> Option<TagRef> {
        self.skins.get(usize::try_from(index).ok()?).copied()
    }
    /// The shape group a 1-based [`Screen::shape_group`] selects; `None` for
    /// zero or an index past the list (the engine then skips it, `0x0022facd`).
    pub fn shape_group(&self, index: u16) -> Option<&ShapeGroup> {
        self.shape_groups.get(usize::from(index).checked_sub(1)?)
    }
}
const SHARED: &str = "SharedGlobals";
fn shared_globals(c: &mut Ctx, b: &[u8]) -> Result<SharedGlobals> {
    let mut sounds = [TagRef {
        group: FourCC::from_u32(u32::MAX),
        id: None,
    }; 16];
    for (i, s) in sounds.iter_mut().enumerate() {
        *s = c.tag_ref(b, 0x90 + i * 8, &[SOUND], SHARED, "sounds")?;
    }
    Ok(SharedGlobals {
        unknown_000: b[..0x4c].to_vec(),
        text_parameter: f32_at(b, 0x4c),
        meter_color: f32s(b, 0x50),
        unknown_060: raw(b, 0x60),
        unknown_088: block_at(b, 0x88),
        sounds,
        bitmap: c.tag_ref(b, 0x110, &[BITMAP], SHARED, "bitmap")?,
        strings: c.tag_ref(b, 0x118, &[STRINGS], SHARED, "strings")?,
        animations: c.records(b, 0x120, 0x2c, SHARED, "animations", animation_collection)?,
        shape_groups: c.records(b, 0x128, 0x18, SHARED, "shape_groups", shape_group)?,
        unknown_130: block_at(b, 0x130),
        skins: c.records(b, 0x138, 8, SHARED, "skins", tag_ref_element(SKIN, SHARED))?,
        strings_140: c.tag_ref(b, 0x140, &[STRINGS], SHARED, "strings_140")?,
        strings_148: c.tag_ref(b, 0x148, &[STRINGS], SHARED, "strings_148")?,
        reference_150: c.tag_ref(b, 0x150, &[], SHARED, "reference_150")?,
        unknown_158: block_at(b, 0x158),
        header_fonts: std::array::from_fn(|i| i16_at(b, 0x160 + i * 2)),
        header_color: f32s(b, 0x168),
        header_layouts: std::array::from_fn(|i| HeaderLayout {
            header: rect_at(b, 0x178 + i * 16),
            secondary: rect_at(b, 0x180 + i * 16),
        }),
        looping_sound: c.tag_ref(b, 0x1b8, &[LOOPING_SOUND], SHARED, "looping_sound")?,
        unknown_1c0: u32_at(b, 0x1c0),
    })
}

// ---------------------------------------------------------------------------
// Entry points

/// A decoded UI tag.
#[derive(Clone, Debug, PartialEq)]
pub enum UiTag {
    Screen(Screen),
    UiGlobals(UiGlobals),
    SharedGlobals(Box<SharedGlobals>),
    Skin(Skin),
}
/// A decoded tag plus its structural coverage.
#[derive(Clone, Debug, PartialEq)]
pub struct Decoded {
    pub tag: UiTag,
    pub stats: Stats,
}

/// Decode a UI tag whose payload starts at `address`. The whole decode is
/// bounded by [`Source::budget`].
pub fn decode(src: &mut dyn Source, group: FourCC, address: u32) -> Result<Decoded> {
    let mut c = Ctx::new(src);
    let tag = decode_in(&mut c, group, address)?;
    Ok(Decoded {
        tag,
        stats: c.stats,
    })
}
fn decode_in(c: &mut Ctx, group: FourCC, address: u32) -> Result<UiTag> {
    let size = match group {
        SCREEN => Screen::SIZE,
        UI_GLOBALS => UiGlobals::SIZE,
        SHARED_GLOBALS => SharedGlobals::SIZE,
        SKIN => Skin::SIZE,
        g => return Err(Error::UnsupportedGroup(g)),
    };
    let outer = c.root_owner;
    let root = c.root(address, size)?;
    let tag = match group {
        SCREEN => UiTag::Screen(screen(c, &root)?),
        UI_GLOBALS => UiTag::UiGlobals(ui_globals(c, &root)?),
        SHARED_GLOBALS => UiTag::SharedGlobals(Box::new(shared_globals(c, &root)?)),
        _ => UiTag::Skin(skin(c, &root)?),
    };
    c.root_owner = outer;
    Ok(tag)
}

/// Read and decode one UI tag from a map. This builds a fresh [`MapSource`];
/// to decode many tags, build one and call [`MapSource::decode_tag`].
pub fn read<R: Read + Seek>(map: &mut Map<R>, id: TagId) -> Result<Decoded> {
    MapSource::new(map).decode_tag(id)
}

/// Deepest skin nesting [`resolve_skin`] follows: a skin is depth 1, a skin
/// named by one of its tables is depth 2, and so on. Retail data nests at
/// most two deep (see `docs/ui.md`); the engine itself has no recorded limit,
/// so this bound is ours and keeps a reference cycle from recursing forever.
pub const MAX_SKIN_DEPTH: usize = 4;

/// A skin with the skins its tables name, resolved recursively.
#[derive(Clone, Debug, PartialEq)]
pub struct SkinTree {
    pub id: TagId,
    pub skin: Skin,
    /// One entry per [`Skin::tables`] element; `None` for a null skin reference.
    pub tables: Vec<Option<SkinTree>>,
}
impl SkinTree {
    /// Nesting depth: 1 for a skin whose tables name no skins.
    pub fn depth(&self) -> usize {
        1 + self
            .tables
            .iter()
            .flatten()
            .map(SkinTree::depth)
            .max()
            .unwrap_or(0)
    }
}

/// Decode skin `id` and every skin its tables reach, at most
/// [`MAX_SKIN_DEPTH`] deep. One budget covers the whole tree.
pub fn resolve_skin(src: &mut dyn Source, id: TagId) -> Result<(SkinTree, Stats)> {
    let mut c = Ctx::new(src);
    let tree = skin_tree(&mut c, id, 1)?;
    Ok((tree, c.stats))
}
fn skin_tree(c: &mut Ctx, id: TagId, depth: usize) -> Result<SkinTree> {
    if depth > MAX_SKIN_DEPTH {
        return Err(Error::Invalid("skin nesting exceeds MAX_SKIN_DEPTH"));
    }
    if c.src.group(id) != Some(SKIN) {
        return Err(Error::Invalid("table skin is not a skin tag"));
    }
    let address = c.src.address(id).ok_or(map::Error::NoTagData)?;
    let UiTag::Skin(skin) = decode_in(c, SKIN, address)? else {
        unreachable!("decode_in(SKIN) returns a skin")
    };
    let tables = skin
        .tables
        .iter()
        .map(|t| t.skin.id.map(|s| skin_tree(c, s, depth + 1)).transpose())
        .collect::<Result<_>>()?;
    Ok(SkinTree { id, skin, tables })
}

#[path = "ui_check.rs"]
pub mod check;

#[cfg(test)]
#[path = "ui_tests.rs"]
mod tests;
