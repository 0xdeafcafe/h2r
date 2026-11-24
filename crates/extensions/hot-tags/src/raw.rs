//! Live writes to raw data: section vertices and indices, bitmap pixels
//! and the metadata region of a structure BSP that is not loaded
//! (docs/editor/realtime-edits.md, "Four write primitives" and "Inactive
//! BSPs").
//!
//! A raw write names its **owner**, the record whose raw pointer locates
//! the bytes in a map file: a section (`sbsp` cluster or instanced
//! definition, `mode` section), one LOD chain of a bitmap image, or one
//! sample chunk of a `snd!` permutation (its record in the map's `ugh!`
//! gestalt; docs/formats/sound.md), or one of the scenario's structure BSP
//! descriptors, whose file offset and size
//! locate that BSP's region. The
//! editor resolves the pointer from the guest's own tag bytes, then at one
//! boundary adds an overlay entry ([`RawFiles`], the file view both the
//! guest's page-ins and the native loaders read) and writes the resident
//! copy when the owner's section is paged in. An owner that is not
//! resident takes only the overlay entry: its next page-in reads it.
//!
//! Every write is journalled with the overlay runs it replaced and the
//! resident bytes it overwrote, so `edit.revert` puts both back.

use super::*;
use map::{RawPointer, ResourceFile};

/// The file view the guest's page-ins and the native loaders read, and the
/// overlay over it (`vfs::RawOverlay` in the game, through
/// `crates/extensions/game/src/hot_glue.rs`). Offsets are file offsets.
pub trait RawFiles: Send {
    /// Why both readers can't serve the overlay now (`Ok` when they can).
    fn ready(&mut self, g: &Guest) -> std::result::Result<(), String>;
    /// `len` bytes of `file` at `offset` as the next page-in reads them:
    /// the file through the overlay.
    fn read(
        &mut self,
        g: &Guest,
        file: ResourceFile,
        offset: u64,
        len: usize,
    ) -> std::result::Result<Vec<u8>, String>;
    /// The overlay's own runs inside the range, for [`Self::restore`].
    fn held(&mut self, g: &Guest, file: ResourceFile, offset: u64, len: u64) -> Vec<(u64, Vec<u8>)>;
    /// Replaces bytes of `file` in the overlay.
    fn put(&mut self, g: &Guest, file: ResourceFile, offset: u64, bytes: &[u8]);
    /// Drops the overlay's runs inside the range and puts `runs` back.
    fn restore(&mut self, g: &Guest, file: ResourceFile, offset: u64, len: u64, runs: &[(u64, Vec<u8>)]);
    /// Reserves and fills an extent past the end of `file`; its offset.
    fn append(
        &mut self,
        g: &Guest,
        file: ResourceFile,
        bytes: &[u8],
        align: u64,
    ) -> std::result::Result<u64, String>;
    /// Drops an extent [`Self::append`] made.
    fn drop_appended(&mut self, g: &Guest, file: ResourceFile, offset: u64) -> bool;
    /// Drops everything: the map changed.
    fn clear(&mut self);
    /// `{entries, bytes, appended, appended_bytes}`.
    fn summary(&self) -> Value;
}

/// What the native renderer must re-read after a raw edit
/// (`render_bridge::native_edits::Edit`, which the glue converts this to).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refresh {
    /// A structure section: `kind` 0 cluster, 1 instanced definition;
    /// `grown` when its counts changed (it was evicted).
    Section { bsp: u32, kind: u8, index: u32, grown: bool },
    /// A render model, any of its sections.
    Model { tag: u32 },
    /// One image of a bitmap.
    Image { bitmap: u32, image: u32 },
}

/// Takes the refreshes a batch or a revert needs, after it applied.
pub type RefreshSink = Box<dyn FnMut(Refresh) + Send>;

/// Which section block an owner indexes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SectionKind {
    Cluster,
    InstanceDef,
    Mode,
}

impl SectionKind {
    fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "cluster" => SectionKind::Cluster,
            "instance_def" => SectionKind::InstanceDef,
            "mode" => SectionKind::Mode,
            other => {
                return Err(Error::invalid(format!(
                    "kind: {other:?} is not cluster, instance_def or mode"
                )));
            }
        })
    }
    fn text(self) -> &'static str {
        match self {
            SectionKind::Cluster => "cluster",
            SectionKind::InstanceDef => "instance_def",
            SectionKind::Mode => "mode",
        }
    }
    /// The owning tag's group, its section block's offset and stride, and
    /// the geometry block info's offset in an element
    /// (docs/formats/structure-bsp.md; `tags::model`).
    fn layout(self) -> (&'static [u8; 4], u32, u32, u32) {
        match self {
            SectionKind::Cluster => (b"sbsp", 0x9c, 0xb0, 0x28),
            SectionKind::InstanceDef => (b"sbsp", 0x138, 0xc8, 0x28),
            SectionKind::Mode => (b"mode", 0x24, 0x5c, 0x38),
        }
    }
}

/// A bitmap's image block (`bitm+0x44`, `BitmapImage` records) and the
/// image's three LOD pointers and sizes.
const BITMAP_IMAGES: u32 = 0x44;
const IMAGE_STRIDE: u32 = 0x74;
const IMAGE_LOD_POINTERS: u32 = 0x1c;
const IMAGE_LOD_SIZES: u32 = 0x34;

/// The sound gestalt's (`ugh!`) blocks: pitch ranges `[4]`,
/// permutations `[5]` and chunks `[8]`, each an 8-byte `{count, address}`
/// in the header, and their strides (docs/formats/sound.md, "Tables").
const GESTALT_RANGES: (u32, u32) = (4 * 8, 12);
const GESTALT_PERMUTATIONS: (u32, u32) = (5 * 8, 16);
const GESTALT_CHUNKS: (u32, u32) = (8 * 8, 12);
/// A `snd!`'s first pitch range (u16) and range count (u8).
const SOUND_FIRST_RANGE: u32 = 0x8;
const SOUND_RANGE_COUNT: u32 = 0xa;
/// A pitch range's first permutation and count (u16 each).
const RANGE_FIRST_PERMUTATION: u32 = 8;
/// A permutation's first chunk and count (u16 each).
const PERMUTATION_FIRST_CHUNK: u32 = 12;
/// A chunk's raw pointer, size (bit 31 a flag, masked) and runtime word:
/// `0xffffffff` in the file, the guest sound cache's handle once the
/// chunk is loaded (**S**: the handle is not read with `h2 disasm` yet).
const CHUNK_SIZE: u32 = 4;
const CHUNK_RUNTIME: u32 = 8;

/// Geometry block info fields (docs/formats/structure-bsp.md, "Geometry
/// block info").
const INFO_RUNTIME_OFFSET: u32 = 0x1c;
const INFO_LOADED: u32 = 0x1e;
const INFO_HANDLE: u32 = 0x20;
/// The resident buffer starts with the block's 8-byte `blkh` header; the
/// runtime section pointer names the section struct after it.
const BLOCK_HEADER: u32 = 8;

/// The scenario's structure BSP block and its descriptors: file offset at
/// `+0`, size at `+4`, virtual base at `+8` (docs/formats/map-format.md,
/// "BSP and lightmap metadata").
const SCENARIO_BSPS: u32 = 0x210;
const BSP_STRIDE: u32 = 0x44;
const BSP_FILE_OFFSET: u32 = 0;
const BSP_SIZE: u32 = 4;
const BSP_VIRTUAL_BASE: u32 = 8;

/// The loaded structure BSP's index in the scenario's block (`-1` before
/// one loads; `engine::sim_collision::layout::BSP_INDEX`): the one BSP
/// whose region is in guest memory. `None` when it can't be read.
pub fn loaded_bsp(g: &Guest) -> Option<i16> {
    g.u16(engine::sim_collision::layout::BSP_INDEX).ok().map(|v| v as i16)
}

/// A raw write's or eviction's owner, by datum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    Section { tag: u32, kind: SectionKind, index: u32 },
    Pixels { tag: u32, image: u32, lod: u32 },
    /// One chunk of a `snd!` permutation's samples: `range` and
    /// `permutation` count within the sound, `chunk` within the
    /// permutation.
    Sound { tag: u32, range: u32, permutation: u32, chunk: u32 },
    /// The region of the scenario's structure BSP `index`, from its first
    /// byte; only while another BSP is loaded.
    Bsp { index: u32 },
}

impl Owner {
    pub fn json(&self) -> Value {
        match *self {
            Owner::Section { tag, kind, index } => json!({"section": {
                "tag": format!("{tag:#010x}"), "kind": kind.text(), "index": index}}),
            Owner::Pixels { tag, image, lod } => json!({"pixels": {
                "tag": format!("{tag:#010x}"), "image": image, "lod": lod}}),
            Owner::Sound { tag, range, permutation, chunk } => json!({"sound": {
                "tag": format!("{tag:#010x}"), "range": range,
                "permutation": permutation, "chunk": chunk}}),
            Owner::Bsp { index } => json!({"bsp": {"index": index}}),
        }
    }

    /// What the native renderer re-reads after a write; nothing for sounds,
    /// whose only host reader (DirectSound) re-reads guest sample memory at
    /// `Unlock` and `Play`. A BSP that is not loaded needs nothing: the
    /// switch to it loads the world from the overlaid file.
    fn refresh(&self, grown: bool) -> Option<Refresh> {
        Some(match *self {
            Owner::Section { tag, kind: SectionKind::Mode, .. } => Refresh::Model { tag },
            Owner::Section { tag, kind, index } => Refresh::Section {
                bsp: tag,
                kind: u8::from(kind == SectionKind::InstanceDef),
                index,
                grown,
            },
            Owner::Pixels { tag, image, .. } => Refresh::Image { bitmap: tag, image },
            Owner::Sound { .. } | Owner::Bsp { .. } => return None,
        })
    }
}

/// An owner's raw block where the guest's tag bytes put it now.
#[derive(Clone, Debug)]
pub(crate) struct Located {
    pub owner: Owner,
    pub file: ResourceFile,
    /// The block's file offset and size.
    pub base: u32,
    pub size: u32,
    /// The geometry block info's address (sections).
    pub info: Option<u32>,
    /// The chunk record's runtime word, the sound cache's handle (sounds).
    pub handle: Option<u32>,
}

/// The old bytes a raw write is checked against: the bytes, or their
/// FNV-1a 64.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Was {
    Bytes(Vec<u8>),
    Hash(u64),
}

impl Was {
    fn holds(&self, bytes: &[u8]) -> bool {
        match self {
            Was::Bytes(b) => b == bytes,
            Was::Hash(h) => fnv1a64(bytes) == *h,
        }
    }
}

/// FNV-1a 64 of `bytes`, the `{hash}` form of `was`.
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// One raw write of an `edit.batch`, its bytes decoded.
#[derive(Clone, Debug)]
pub struct RawWrite {
    /// `{section: {tag, kind, index}}` or `{pixels: {tag, image, lod}}`;
    /// `tag` a datum or a name.
    pub owner: Value,
    /// From the start of the raw block or LOD chain.
    pub offset: u32,
    pub bytes: Vec<u8>,
    pub was: Option<Was>,
}

/// What a batch carries besides field writes.
#[derive(Clone, Debug, Default)]
pub struct RawBatch {
    pub raw: Vec<RawWrite>,
    /// Owners to evict: `{section: ...}` only.
    pub evict: Vec<Value>,
    /// Blocks to copy into the edit arena ([`crate::arena`]).
    pub moves: Vec<crate::Move>,
}

/// A journalled raw act.
#[derive(Clone, Debug)]
pub(crate) enum RawChange {
    Write {
        id: u64,
        scenario: u32,
        owner: Owner,
        file: ResourceFile,
        /// File offset of the first byte.
        at: u64,
        new: Vec<u8>,
        /// The overlay's runs over the range before the write.
        held: Vec<(u64, Vec<u8>)>,
        /// The block offset of the first byte, and the resident bytes
        /// overwritten when the owner was resident.
        offset: u32,
        resident: Option<Vec<u8>>,
    },
    Evict { id: u64, scenario: u32, owner: Owner },
    Append { id: u64, scenario: u32, file: ResourceFile, at: u64, len: u64 },
}

impl RawChange {
    pub(crate) fn id(&self) -> u64 {
        match self {
            RawChange::Write { id, .. } | RawChange::Evict { id, .. } | RawChange::Append { id, .. } => *id,
        }
    }
    fn scenario(&self) -> u32 {
        match self {
            RawChange::Write { scenario, .. }
            | RawChange::Evict { scenario, .. }
            | RawChange::Append { scenario, .. } => *scenario,
        }
    }
    fn json(&self) -> Value {
        match self {
            RawChange::Write { id, owner, offset, new, resident, .. } => json!({"id": id,
                "raw": owner.json(), "offset": offset, "len": new.len(),
                "resident": resident.is_some()}),
            RawChange::Evict { id, owner, .. } => json!({"id": id, "evict": owner.json()}),
            RawChange::Append { id, file, at, len, .. } => json!({"id": id,
                "append": file_text(*file), "pointer": pointer(*file, *at), "len": len}),
        }
    }
}

pub(crate) fn file_text(f: ResourceFile) -> &'static str {
    match f {
        ResourceFile::Local => "local",
        ResourceFile::MainMenu => "mainmenu",
        ResourceFile::Shared => "shared",
        ResourceFile::SinglePlayerShared => "single_player_shared",
    }
}

/// `edit.raw.append`'s `file`: the selector a raw pointer carries.
pub fn parse_file(s: &str) -> Result<ResourceFile> {
    Ok(match s {
        "local" => ResourceFile::Local,
        "mainmenu" => ResourceFile::MainMenu,
        "shared" => ResourceFile::Shared,
        "single_player_shared" => ResourceFile::SinglePlayerShared,
        other => {
            return Err(Error::invalid(format!(
                "file: {other:?} is not local, mainmenu, shared or single_player_shared"
            )));
        }
    })
}

/// A raw pointer: the selector in the top two bits, the offset below
/// (`map::RawPointer`).
fn pointer(file: ResourceFile, at: u64) -> u32 {
    let sel = match file {
        ResourceFile::Local => 0,
        ResourceFile::MainMenu => 1,
        ResourceFile::Shared => 2,
        ResourceFile::SinglePlayerShared => 3,
    };
    (sel << 30) | (at as u32 & 0x3fff_ffff)
}

fn guest_u32(g: &Guest, at: u32) -> Result<u32> {
    g.u32(at).map_err(fault)
}

fn guest_u16(g: &Guest, at: u32) -> Result<u32> {
    g.u16(at).map(u32::from).map_err(fault)
}

/// The address of element `i` of the run `first..first + count` of the
/// gestalt's block at header offset `block`, checked against both the run
/// and the block.
fn gestalt_element(
    g: &Guest,
    gestalt: u32,
    (block, stride): (u32, u32),
    (first, count): (u32, u32),
    i: u32,
    what: &str,
) -> Result<u32> {
    if i >= count {
        return Err(Error::invalid(format!("owner: {what} {i} past the sound's {count}")));
    }
    let len = guest_u32(g, gestalt + block)?;
    let at = first + i;
    if at >= len {
        return Err(Error::invalid(format!(
            "owner: {what} {i} is the gestalt's element {at}, past its {len}"
        )));
    }
    Ok(guest_u32(g, gestalt + block + 4)? + at * stride)
}

/// The resident copy's address of block offset 0: the paged-in buffer,
/// found through the block info's loaded flag and runtime section block.
fn resident_base(g: &Guest, info: u32) -> Result<Option<u32>> {
    if g.u8(info + INFO_LOADED).map_err(fault)? == 0 {
        return Ok(None);
    }
    let rel = g.u16(info + INFO_RUNTIME_OFFSET).map_err(fault)? as i16;
    let block = info.wrapping_add_signed(i32::from(rel));
    let (count, section) = (guest_u32(g, block)?, guest_u32(g, block + 4)?);
    Ok((count >= 1 && section >= BLOCK_HEADER).then(|| section - BLOCK_HEADER))
}

impl Editor {
    /// Live raw writes go to `files`; refreshes to `sink`.
    pub fn with_raw(mut self, files: Box<dyn RawFiles>, sink: RefreshSink) -> Editor {
        self.raw = Some(files);
        self.refresh = Some(sink);
        self
    }

    fn raw_files(&mut self) -> Result<&mut Box<dyn RawFiles>> {
        self.raw
            .as_mut()
            .ok_or_else(|| Error::unavailable("live raw writes are not installed in this game"))
    }

    fn push_refresh(&mut self, r: Refresh) {
        if let Some(sink) = self.refresh.as_mut() {
            sink(r);
        }
    }

    /// The owner `v` names, by datum.
    pub(crate) fn owner(&mut self, g: &Guest, idx: &index::TagIndex, v: &Value) -> Result<Owner> {
        let num = |o: &Value, k: &str| {
            o.get(k)
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| Error::invalid(format!("owner: {k} must be a 32-bit index")))
        };
        let tag_of = |me: &mut Editor, o: &Value, group: &[u8; 4]| -> Result<u32> {
            let tag = o["tag"]
                .as_str()
                .ok_or_else(|| Error::invalid("owner: tag (a datum or a name)"))?;
            let sel = TagSel {
                tag: tag.into(),
                group: None,
            };
            let e = me.select_in(g, idx, &sel, &[FourCC(*group)])?;
            if e.group != FourCC(*group) {
                return Err(Error::invalid(format!(
                    "owner: {tag} is a {} tag, not {}",
                    group_text(e.group),
                    group_text(FourCC(*group))
                )));
            }
            Ok(e.datum)
        };
        if let Some(s) = v.get("section") {
            let kind = SectionKind::parse(
                s["kind"]
                    .as_str()
                    .ok_or_else(|| Error::invalid("owner: section kind"))?,
            )?;
            let tag = tag_of(self, s, kind.layout().0)?;
            return Ok(Owner::Section { tag, kind, index: num(s, "index")? });
        }
        if let Some(p) = v.get("pixels") {
            let tag = tag_of(self, p, b"bitm")?;
            let lod = num(p, "lod")?;
            if lod > 2 {
                return Err(Error::invalid("owner: lod is 0, 1 or 2"));
            }
            return Ok(Owner::Pixels { tag, image: num(p, "image")?, lod });
        }
        if let Some(s) = v.get("sound") {
            let tag = tag_of(self, s, b"snd!")?;
            return Ok(Owner::Sound {
                tag,
                range: num(s, "range")?,
                permutation: num(s, "permutation")?,
                chunk: num(s, "chunk")?,
            });
        }
        if let Some(b) = v.get("bsp") {
            return Ok(Owner::Bsp { index: num(b, "index")? });
        }
        Err(Error::invalid(
            "owner: {section: {tag, kind, index}}, {pixels: {tag, image, lod}}, \
             {sound: {tag, range, permutation, chunk}} or {bsp: {index}}",
        ))
    }

    /// Where `owner`'s raw block is, from its tag's bytes now.
    pub(crate) fn locate_owner(&self, g: &Guest, idx: &index::TagIndex, owner: Owner) -> Result<Located> {
        let tag = match owner {
            Owner::Section { tag, .. } | Owner::Pixels { tag, .. } | Owner::Sound { tag, .. } => tag,
            Owner::Bsp { index } => {
                let d = bsp_descriptor(g, idx, index)?;
                let (base, size) = (guest_u32(g, d + BSP_FILE_OFFSET)?, guest_u32(g, d + BSP_SIZE)?);
                if base == 0 || size == 0 {
                    return Err(Error::invalid(format!("owner: BSP {index} has no region in this map")));
                }
                return Ok(Located { owner, file: ResourceFile::Local, base, size, info: None });
            }
        };
        let e = index::by_datum(g, idx, tag).map_err(Error::invalid)?;
        let element = |block: u32, stride: u32, i: u32, what: &str| -> Result<u32> {
            let count = guest_u32(g, e.address + block)?;
            if i >= count {
                return Err(Error::invalid(format!("owner: {what} {i} past the block's {count}")));
            }
            Ok(guest_u32(g, e.address + block + 4)? + i * stride)
        };
        let (pointer_at, size_at, info, handle) = match owner {
            Owner::Section { kind, index, .. } => {
                let (_, block, stride, info) = kind.layout();
                let info = element(block, stride, index, kind.text())? + info;
                (info, info + 4, Some(info), None)
            }
            Owner::Pixels { image, lod, .. } => {
                let el = element(BITMAP_IMAGES, IMAGE_STRIDE, image, "image")?;
                (el + IMAGE_LOD_POINTERS + 4 * lod, el + IMAGE_LOD_SIZES + 4 * lod, None, None)
            }
            Owner::Sound { range, permutation, chunk, .. } => {
                let gestalt = gestalt(g, idx)?;
                let first = guest_u16(g, e.address + SOUND_FIRST_RANGE)?;
                let count = u32::from(g.u8(e.address + SOUND_RANGE_COUNT).map_err(fault)?);
                let r = gestalt_element(g, gestalt, GESTALT_RANGES, (first, count), range, "range")?;
                let run = (
                    guest_u16(g, r + RANGE_FIRST_PERMUTATION)?,
                    guest_u16(g, r + RANGE_FIRST_PERMUTATION + 2)?,
                );
                let p = gestalt_element(g, gestalt, GESTALT_PERMUTATIONS, run, permutation, "permutation")?;
                let run = (
                    guest_u16(g, p + PERMUTATION_FIRST_CHUNK)?,
                    guest_u16(g, p + PERMUTATION_FIRST_CHUNK + 2)?,
                );
                let c = gestalt_element(g, gestalt, GESTALT_CHUNKS, run, chunk, "chunk")?;
                (c, c + CHUNK_SIZE, None, Some(c + CHUNK_RUNTIME))
            }
            Owner::Bsp { .. } => unreachable!("located above"),
        };
        let raw = RawPointer::decode(guest_u32(g, pointer_at)?)
            .ok_or_else(|| Error::invalid("owner: it has no raw block"))?;
        let mut size = guest_u32(g, size_at)?;
        if handle.is_some() {
            // A chunk's size has a flag in bit 31.
            size &= 0x7fff_ffff;
        }
        if size == 0 {
            return Err(Error::invalid("owner: its raw block is empty"));
        }
        Ok(Located { owner, file: raw.file, base: raw.offset, size, info, handle })
    }

    /// The raw half of `edit.batch`: everything checked, nothing written.
    /// Returns the planned writes and evictions, or the mismatches.
    pub(crate) fn plan_raw(
        &mut self,
        g: &Guest,
        idx: &index::TagIndex,
        batch: &RawBatch,
        mismatches: &mut Vec<Value>,
    ) -> Result<(Vec<Planned>, Vec<Located>)> {
        if batch.raw.is_empty() && batch.evict.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }
        self.raw_files()?.ready(g).map_err(Error::unavailable)?;
        let mut planned: Vec<Planned> = Vec::new();
        for w in &batch.raw {
            let owner = self.owner(g, idx, &w.owner)?;
            if let Owner::Bsp { index } = owner
                && loaded_bsp(g).is_some_and(|l| i64::from(l) == i64::from(index))
            {
                // Its region is guest memory now; the overlay alone would
                // leave the game reading the old bytes until the next switch.
                return Err(Error::invalid(format!(
                    "owner: BSP {index} is the loaded one: write its fields with at"
                )));
            }
            let loc = self.locate_owner(g, idx, owner)?;
            let len = w.bytes.len() as u32;
            if len == 0 || u64::from(w.offset) + u64::from(len) > u64::from(loc.size) {
                return Err(Error::invalid(format!(
                    "raw: {} bytes at {:#x} are outside the owner's {:#x}-byte block",
                    len, w.offset, loc.size
                )));
            }
            let at = u64::from(loc.base) + u64::from(w.offset);
            if planned.iter().any(|p| {
                p.loc.file == loc.file && p.at < at + u64::from(len) && at < p.at + p.new.len() as u64
            }) {
                return Err(Error::invalid("overlapping raw writes in edit.batch"));
            }
            let current = self.raw_files()?.read(g, loc.file, at, len as usize).map_err(Error::unavailable)?;
            let resident_at = match loc.info {
                Some(info) => resident_base(g, info)?.map(|b| b + w.offset),
                None => None,
            };
            let resident = match resident_at {
                Some(addr) => {
                    g.check(addr, len as usize, runtime::guest::Access::Write, 1).map_err(fault)?;
                    Some((addr, read_bytes(g, addr, len)?))
                }
                None => None,
            };
            // The file view and the resident copy each hold the old bytes
            // (or already the new ones: a resend).
            let ok = |b: &[u8]| {
                b == w.bytes.as_slice()
                    || match &w.was {
                        Some(was) => was.holds(b),
                        None => b == current.as_slice(),
                    }
            };
            let file_ok = ok(&current);
            let resident_ok = resident.as_ref().is_none_or(|(_, b)| ok(b));
            if !file_ok || !resident_ok {
                mismatches.push(json!({"owner": owner.json(), "offset": w.offset,
                    "holds_hash": format!("{:#018x}", fnv1a64(&current)),
                    "resident_holds_hash": resident.as_ref().map(|(_, b)| format!("{:#018x}", fnv1a64(b))),
                    "why": if file_ok { "the resident copy differs" } else { "the file view differs" }}));
            }
            planned.push(Planned { loc, at, offset: w.offset, new: w.bytes.clone(), resident });
        }
        let mut evict = Vec::new();
        for v in &batch.evict {
            let owner = self.owner(g, idx, v)?;
            if let Owner::Pixels { .. } = owner {
                return Err(Error::invalid("evict: only a section or a sound chunk can be evicted"));
            }
            let loc = self.locate_owner(g, idx, owner)?;
            match (loc.info, loc.handle) {
                (Some(info), _) => {
                    g.check(info + INFO_LOADED, 6, runtime::guest::Access::Write, 1).map_err(fault)?;
                }
                (None, Some(handle)) => {
                    g.check(handle, 4, runtime::guest::Access::Write, 1).map_err(fault)?;
                }
                (None, None) => unreachable!("a section has block info and a chunk a handle"),
            }
            evict.push(loc);
        }
        Ok((planned, evict))
    }

    /// Applies what [`Self::plan_raw`] checked, journalled under `id`.
    /// Returns the reply's `raw` entries and `evicted` owners.
    pub(crate) fn apply_raw(
        &mut self,
        g: &mut Guest,
        scenario: u32,
        id: u64,
        planned: Vec<Planned>,
        evict: Vec<Located>,
    ) -> Result<(Vec<Value>, Vec<Value>)> {
        let mut replies = Vec::new();
        let mut refreshes: Vec<Refresh> = Vec::new();
        let grown = |o: &Owner| evict.iter().any(|e| e.owner == *o);
        for p in planned {
            let len = p.new.len() as u64;
            let files = self.raw_files()?;
            let held = files.held(g, p.loc.file, p.at, len);
            files.put(g, p.loc.file, p.at, &p.new);
            let resident = match p.resident {
                Some((addr, old)) => {
                    g.write_bytes(addr, &p.new, 1).map_err(fault)?;
                    Some(old)
                }
                None => None,
            };
            let refresh = p.loc.owner.refresh(grown(&p.loc.owner));
            replies.push(json!({"owner": p.loc.owner.json(), "offset": p.offset,
                "resident": resident.is_some(), "refresh": refresh.map(refresh_json)}));
            if let Some(refresh) = refresh
                && !refreshes.contains(&refresh)
            {
                refreshes.push(refresh);
            }
            self.raw_journal.push(RawChange::Write {
                id,
                scenario,
                owner: p.loc.owner,
                file: p.loc.file,
                at: p.at,
                new: p.new,
                held,
                offset: p.offset,
                resident,
            });
        }
        let mut evicted = Vec::new();
        for loc in evict {
            evict_located(g, &loc)?;
            evicted.push(loc.owner.json());
            if let Some(refresh) = loc.owner.refresh(true)
                && !refreshes.contains(&refresh)
            {
                refreshes.push(refresh);
            }
            self.raw_journal.push(RawChange::Evict { id, scenario, owner: loc.owner });
        }
        for r in refreshes {
            self.push_refresh(r);
        }
        Ok((replies, evicted))
    }

    /// `edit.raw.append`: an extent past the end of `file`, invisible
    /// until a tag points at it. Journalled; revert drops it.
    pub fn raw_append(&mut self, g: &Guest, file: ResourceFile, bytes: &[u8], align: u64) -> Result<Value> {
        if bytes.is_empty() {
            return Err(Error::invalid("bytes: nothing to append"));
        }
        let idx = self.index(g)?;
        let files = self.raw_files()?;
        files.ready(g).map_err(Error::unavailable)?;
        let at = files.append(g, file, bytes, align).map_err(Error::unavailable)?;
        let len = bytes.len() as u64;
        if at + len > 0x4000_0000 {
            files.drop_appended(g, file, at);
            return Err(Error::invalid("the extent would end past a raw pointer's 30-bit offset"));
        }
        self.next_id += 1;
        let id = self.next_id;
        self.raw_journal.push(RawChange::Append { id, scenario: idx.scenario, file, at, len });
        Ok(json!({"id": id, "pointer": pointer(file, at), "len": len}))
    }

    /// `edit.overlay`: what the overlay holds and the owners written.
    pub fn overlay(&self) -> Value {
        let mut out = self
            .raw
            .as_ref()
            .map_or_else(|| json!({"entries": 0, "bytes": 0, "appended": 0}), |r| r.summary());
        let mut owners: Vec<Value> = Vec::new();
        for c in &self.raw_journal {
            if let RawChange::Write { owner, .. } = c {
                let o = owner.json();
                if !owners.contains(&o) {
                    owners.push(o);
                }
            }
        }
        out["owners"] = json!(owners);
        out
    }

    /// The raw journal for `edit.changes`.
    pub(crate) fn raw_changes(&self) -> Vec<Value> {
        self.raw_journal.iter().map(RawChange::json).collect()
    }

    /// Reverts the raw journal's entries of `id` (all with `None`), newest
    /// first: overlay runs put back, resident bytes restored where they
    /// still hold the written ones, evicted sections evicted again so the
    /// guest pages in the restored block, appended extents dropped.
    pub(crate) fn revert_raw(
        &mut self,
        g: &mut Guest,
        id: Option<u64>,
        reverted: &mut Vec<Value>,
        dropped: &mut Vec<Value>,
    ) -> Result<()> {
        let idx = index::index(g).ok();
        let scenario = idx.map(|i| i.scenario);
        let mut refreshes = Vec::new();
        let mut i = self.raw_journal.len();
        while i > 0 {
            i -= 1;
            if id.is_some_and(|want| self.raw_journal[i].id() != want) {
                continue;
            }
            let change = self.raw_journal.remove(i);
            if scenario != Some(change.scenario()) {
                dropped.push(json!({"id": change.id(), "raw": change.json(), "why": "made on another map"}));
                continue;
            }
            let idx = idx.expect("a scenario was read");
            match change {
                RawChange::Write { id, owner, file, at, new, held, offset, resident, .. } => {
                    let len = new.len() as u64;
                    self.raw_files()?.restore(g, file, at, len, &held);
                    let mut restored = false;
                    let mut why = None;
                    if let Owner::Bsp { index } = owner
                        && loaded_bsp(g).is_some_and(|l| i64::from(l) == i64::from(index))
                    {
                        // Switched to since: guest memory holds the region
                        // as the switch read it, the write included.
                        let old = self
                            .raw_files()?
                            .read(g, file, at, len as usize)
                            .map_err(Error::unavailable)?;
                        let addr = bsp_descriptor(g, &idx, index)
                            .and_then(|d| guest_u32(g, d + BSP_VIRTUAL_BASE))?
                            + offset;
                        if read_bytes(g, addr, len as u32)? == new {
                            g.write_bytes(addr, &old, 1).map_err(fault)?;
                            restored = true;
                        } else {
                            why = Some("the loaded BSP's bytes changed since; the overlay was put back");
                        }
                    }
                    if let Some(old) = resident {
                        // Find the resident copy again: a BSP switch can move it.
                        let now = self
                            .locate_owner(g, &idx, owner)
                            .ok()
                            .and_then(|l| l.info)
                            .map(|info| resident_base(g, info))
                            .transpose()?
                            .flatten();
                        // Paged out since (None): the next page-in reads the restored file view.
                        if let Some(base) = now {
                            let addr = base + offset;
                            if read_bytes(g, addr, len as u32)? == new {
                                g.write_bytes(addr, &old, 1).map_err(fault)?;
                                restored = true;
                            } else {
                                why = Some("the resident copy changed since; the overlay was put back");
                            }
                        }
                    }
                    if let Some(r) = owner.refresh(false)
                        && !refreshes.contains(&r)
                    {
                        refreshes.push(r);
                    }
                    reverted.push(json!({"id": id, "raw": owner.json(), "offset": offset,
                        "resident": restored, "note": why}));
                }
                RawChange::Evict { id, owner, .. } => {
                    if let Ok(loc) = self.locate_owner(g, &idx, owner) {
                        evict_located(g, &loc)?;
                    }
                    if let Some(r) = owner.refresh(true)
                        && !refreshes.contains(&r)
                    {
                        refreshes.push(r);
                    }
                    reverted.push(json!({"id": id, "evict": owner.json()}));
                }
                RawChange::Append { id, file, at, len, .. } => {
                    self.raw_files()?.drop_appended(g, file, at);
                    reverted.push(json!({"id": id, "append": file_text(file),
                        "pointer": pointer(file, at), "len": len}));
                }
            }
        }
        for r in refreshes {
            self.push_refresh(r);
        }
        Ok(())
    }

    /// A new map: the raw journal and the overlay go.
    pub(crate) fn clear_raw(&mut self) {
        self.raw_journal.clear();
        if let Some(r) = self.raw.as_mut() {
            r.clear();
        }
    }
}

/// The address of descriptor `index` in the loaded scenario's structure
/// BSP block, as guest memory holds the scenario.
fn bsp_descriptor(g: &Guest, idx: &index::TagIndex, index: u32) -> Result<u32> {
    let scenario = index::by_datum(g, idx, idx.scenario).map_err(Error::unavailable)?;
    let count = guest_u32(g, scenario.address + SCENARIO_BSPS)?;
    if index >= count {
        return Err(Error::invalid(format!("owner: BSP {index} past the scenario's {count}")));
    }
    Ok(guest_u32(g, scenario.address + SCENARIO_BSPS + 4)? + index * BSP_STRIDE)
}

/// A checked raw write, ready to apply.
pub(crate) struct Planned {
    loc: Located,
    at: u64,
    offset: u32,
    new: Vec<u8>,
    resident: Option<(u32, Vec<u8>)>,
}

/// Marks a section not loaded: the loaded flag, the runtime section block
/// and the cache handle, so the guest's next resource lookup misses and
/// pages the block in again from the file view
/// (docs/editor/realtime-edits.md, "Geometry"; **S**: the cache's own
/// entry for the old buffer is left to its eviction).
fn evict_section(g: &mut Guest, info: u32) -> Result<()> {
    let rel = g.u16(info + INFO_RUNTIME_OFFSET).map_err(fault)? as i16;
    let block = info.wrapping_add_signed(i32::from(rel));
    g.set_u8(info + INFO_LOADED, 0).map_err(fault)?;
    g.set_u32(info + INFO_HANDLE, u32::MAX).map_err(fault)?;
    g.write_bytes(block, &[0; 8], 1).map_err(fault)?;
    Ok(())
}

/// Marks a sound chunk not loaded: its runtime word, the sound cache's
/// handle, back to the file's `0xffffffff`, so the guest's next play of
/// the permutation misses and reads the chunk from the file view
/// (docs/editor/realtime-edits.md, "Sound samples"; **S**: the handle
/// and the cache's own entry for the old samples, left to its eviction).
/// A voice already playing keeps its buffer.
fn evict_sound(g: &mut Guest, handle: u32) -> Result<()> {
    g.set_u32(handle, u32::MAX).map_err(fault)
}

fn evict_located(g: &mut Guest, loc: &Located) -> Result<()> {
    match (loc.info, loc.handle) {
        (Some(info), _) => evict_section(g, info),
        (None, Some(handle)) => evict_sound(g, handle),
        (None, None) => Ok(()),
    }
}

/// The map's one sound gestalt (`ugh!`), by address.
fn gestalt(g: &Guest, idx: &index::TagIndex) -> Result<u32> {
    index::all(g, idx)
        .map_err(Error::invalid)?
        .into_iter()
        .find(|e| e.group == FourCC(*b"ugh!"))
        .map(|e| e.address)
        .ok_or_else(|| Error::invalid("owner: the loaded map has no sound gestalt (ugh!)"))
}

fn refresh_json(r: Refresh) -> Value {
    match r {
        Refresh::Section { bsp, kind, index, grown } => json!({"section": {
            "bsp": format!("{bsp:#010x}"), "kind": kind, "index": index, "grown": grown}}),
        Refresh::Model { tag } => json!({"model": format!("{tag:#010x}")}),
        Refresh::Image { bitmap, image } => json!({"image": {
            "bitmap": format!("{bitmap:#010x}"), "image": image}}),
    }
}

#[cfg(test)]
mod tests;
