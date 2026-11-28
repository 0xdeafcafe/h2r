//! The simulation's lightmap sample (docs/engine/sim-render-split.md,
//! "6. Render scratch and stamps", the effect tint).
//!
//! The tick reads lighting into an effect's `+0x104` and `+0x108`: from
//! the object's render-state record (`0x001796e2`, `0x0017b63f`) or from
//! its own sample at the effect's location (`0x0017b6e7`). The guest's
//! sample, `0x000d1850` -> `0x000d2a50` -> the ray `0x0014af40` ->
//! `0x000d1e10`, reads the BSP's lightmap through the geometry cache
//! (`0x0012de70`), the lightmap bitmaps through the texture cache, and
//! the PRT table the render rebuilds at each view (`0x004b89b0`), so its
//! answer follows the render. Here:
//!
//! - [`store`]: every geometry block of the active structure and its
//!   lightmap, and the lightmap bitmaps' pixels, read from the map file
//!   at the first sample after a map load or a structure switch, as the
//!   clip store holds the clips;
//! - [`view`]: guest memory with the store over a token window, and the
//!   three residency seams;
//! - [`sample`]: `0x000d1e10` and its callees in Rust over a [`View`];
//! - [`object`]: `0x000d1850` and its callees but the sampler in Rust:
//!   the directions `0x000d2a50`, the ray `0x0014af40` through entry J
//!   (`0x001697c0`, natively), its structure hit `0x0014ac60` and its
//!   hit on an object whose render model has lighting blocks
//!   `0x0014a8e0`, the sky `0x000d1a30`;
//! - [`answer`]: the `0x000d1e10` hook's body, for every caller;
//! - [`record_call`]: the `0x0003ddd0` hook's body, for every caller
//!   ([`crate::render::object_lighting::record`]);
//! - [`Fill`]: the tick's record. The record's sample is made only in the
//!   frame step (`0x0003dfe0`, from the object setup), so under the
//!   render scratch store the tick's copy never has one. At the two tick
//!   reads, after `0x0003ddd0` gives the record, [`tick_sample`] samples
//!   the object at its centre (the port of `0x000d1850`, as `0x0003e146`
//!   calls it) into the tick's record and marks it (`+2`), so the read
//!   finds it. No guest code runs.
//!
//! Nothing here is guest state: the store is derived from the map file
//! and the tags, and a map load or structure switch drops it
//! ([`invalidate`]).
use crate::sim_native::Memory;
use runtime::{CpuAbi, GuestFault, TransferAbi};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

pub mod object;
pub mod sample;
pub mod store;
pub mod view;

pub use store::{BuildError, Info, Resource, Store};
pub use view::{View, WINDOW, WINDOW_LEN, is_token};

/// The sampler: collision result, then three arguments; `ret 0x10`.
pub const SAMPLE: u32 = 0x000d_1e10;
/// The object sample `0x000d1850`: EDI the object, stack (first, out);
/// AL true when it sampled; `ret 8`.
pub const OBJECT_SAMPLE: u32 = 0x000d_1850;
/// The record get: stack (the object's render-state handle); EAX the
/// record's handle or -1; `ret 4`.
pub const RECORD: u32 = 0x0003_ddd0;

/// The active structure and lightmap (docs/formats/structure-bsp.md).
const ACTIVE_BSP: u32 = 0x004e_0348;
const ACTIVE_LIGHTMAP: u32 = 0x004e_0344;
const TAG_INDEX: u32 = crate::tags::TAG_INDEX_GLOBAL;
const TAG_ENTRIES: u32 = crate::tags::TAG_ENTRIES_GLOBAL;
/// The render-state cache, `0x100`-byte records.
const RECORDS: u32 = 0x0050_9434;

/// Structure: clusters (`+0x9c` count, `+0xa0` address, `0xb0` each) and
/// instanced geometry definitions (`+0x138`, `+0x13c`, `0xc8` each), the
/// block info at `+0x28` of both (`0x000d1d70`, `0x00041040`).
const CLUSTERS: (u32, u32) = (0x9c, 0xb0);
const DEFINITIONS: (u32, u32) = (0x138, 0xc8);
const GEOMETRY_INFO: u32 = 0x28;
/// Lightmap: groups (`+0x80`, `0x68` each); a group's buckets (`+0x40`,
/// `0x38` each, the info at `+0x0c`) and its bitmap (the tag reference
/// at `+0x18`, the datum at `+0x1c`).
const GROUPS: (u32, u32) = (0x80, 0x68);
const BUCKETS: (u32, u32) = (0x40, 0x38);
const BUCKET_INFO: u32 = 0x0c;
const GROUP_BITMAP: u32 = 0x1c;
/// Bitmap: images (`+0x44`, `0x74` each); an image's level-0 raw pointer
/// `+0x1c` and size `+0x34` (`tags::rgba_bitmap`).
const IMAGES: (u32, u32) = (0x44, 0x74);
const IMAGE_POINTER: u32 = 0x1c;
const IMAGE_SIZE: u32 = 0x34;
/// Bound on a block of tag elements: a larger count is not tag data.
const MAX_ELEMENTS: u32 = 0x1_0000;

/// Reads a raw resource of the loaded map: `size` bytes at the raw
/// pointer `pointer` (the file in the top two bits), as the caches would.
/// The game extension supplies it ([`set_raw_files`]).
pub trait RawFiles: Send {
    /// Take what the reads need from the loaded map (its files' paths),
    /// at each store load.
    fn bind(&mut self, g: &runtime::Guest);
    /// `size` bytes at `pointer`, of the map bound last.
    fn read_raw(&mut self, pointer: u32, size: u32) -> Result<Vec<u8>, String>;
}

/// Why a sample is left to the guest's body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Gap {
    NoFiles,
    /// The structure's store failed to load; the message names why.
    Load(Arc<str>),
    /// No structure or lightmap is active.
    NoStructure,
}

impl std::fmt::Display for Gap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoFiles => write!(f, "no map file reader is installed"),
            Self::Load(why) => write!(f, "the structure's lightmap did not load: {why}"),
            Self::NoStructure => write!(f, "no structure and lightmap are active"),
        }
    }
}

fn fault_text(e: GuestFault) -> String {
    format!("tag data unreadable: {e}")
}

fn elements(g: &impl Memory, owner: u32, (field, stride): (u32, u32)) -> Result<Vec<u32>, String> {
    let count = g.u32(owner.wrapping_add(field)).map_err(fault_text)?;
    if count > MAX_ELEMENTS {
        return Err(format!("{owner:#010x}+{field:#x}: {count} elements is not tag data"));
    }
    let first = g.u32(owner.wrapping_add(field + 4)).map_err(fault_text)?;
    Ok((0..count).map(|i| first.wrapping_add(i * stride)).collect())
}

fn info_at(g: &impl Memory, info: u32) -> Result<(Info, Vec<Resource>), GuestFault> {
    let head = Info {
        raw: g.u32(info)?,
        size: g.u32(info.wrapping_add(4))?,
        section: g.u32(info.wrapping_add(8))?,
    };
    let count = g.u32(info.wrapping_add(0x10))?.min(MAX_ELEMENTS);
    let first = g.u32(info.wrapping_add(0x14))?;
    let mut resources = Vec::with_capacity(count as usize);
    for i in 0..count {
        let r = first.wrapping_add(i * 0x10);
        resources.push(Resource {
            kind: g.u8(r)?,
            field: g.u16(r.wrapping_add(4))?,
            secondary: g.u16(r.wrapping_add(6))?,
            size: g.u32(r.wrapping_add(8))?,
            offset: g.u32(r.wrapping_add(0xc))?,
        });
    }
    Ok((head, resources))
}

/// The tag a datum names (`[0x004e3b44] + 16 * index + 8`), 0 for none.
fn tag_address(g: &impl Memory, datum: u32) -> Result<u32, GuestFault> {
    if datum == u32::MAX {
        return Ok(0);
    }
    let entries = g.u32(TAG_ENTRIES)?;
    g.u32(entries.wrapping_add((datum & 0xffff) << 4).wrapping_add(8))
}

/// Every geometry block of the active structure (`bsp`) and lightmap
/// (`ltmp`), and the pixels of every image of the lightmap groups'
/// bitmaps, read through `read` (raw pointer, size). A block info with
/// no raw block (`+0` -1) is not held: its section is in the tag
/// ([`View::section`] reads it there).
pub fn load(
    g: &impl Memory,
    bsp: u32,
    ltmp: u32,
    mut read: impl FnMut(u32, u32) -> Result<Vec<u8>, String>,
) -> Result<Store, String> {
    let mut infos = Vec::new();
    for owner in elements(g, bsp, CLUSTERS)?.into_iter().chain(elements(g, bsp, DEFINITIONS)?) {
        infos.push(owner.wrapping_add(GEOMETRY_INFO));
    }
    let groups = elements(g, ltmp, GROUPS)?;
    for &group in &groups {
        for bucket in elements(g, group, BUCKETS)? {
            infos.push(bucket.wrapping_add(BUCKET_INFO));
        }
    }
    let mut b = Store::builder();
    for info in infos {
        if b.contains(info) {
            continue;
        }
        let (head, resources) = info_at(g, info).map_err(fault_text)?;
        if head.raw == u32::MAX || head.size == 0 {
            continue;
        }
        if head.size > store::MAX_BLOCK {
            return Err(format!("block info {info:#010x}: a {}-byte block", head.size));
        }
        let bytes = read(head.raw, head.size).map_err(|why| format!("block info {info:#010x}: {why}"))?;
        b.add_block(info, head, &resources, &bytes).map_err(|e| e.to_string())?;
    }
    for group in groups {
        let bitmap = tag_address(g, g.u32(group.wrapping_add(GROUP_BITMAP)).map_err(fault_text)?)
            .map_err(fault_text)?;
        if bitmap == 0 {
            continue;
        }
        for image in elements(g, bitmap, IMAGES)? {
            let pointer = g.u32(image.wrapping_add(IMAGE_POINTER)).map_err(fault_text)?;
            let size = g.u32(image.wrapping_add(IMAGE_SIZE)).map_err(fault_text)?;
            if pointer == u32::MAX || size == 0 {
                continue;
            }
            if size > store::MAX_BLOCK {
                return Err(format!("image {image:#010x}: a {size}-byte level"));
            }
            let bytes = read(pointer, size).map_err(|why| format!("image {image:#010x}: {why}"))?;
            b.add_pixels(image, &bytes).map_err(|e| e.to_string())?;
        }
    }
    Ok(b.finish())
}

fn late_fault(addr: u32) -> impl Fn(String) -> GuestFault {
    move |why| {
        eprintln!("lightmap store: {addr:#010x} not read: {why}");
        GuestFault {
            addr,
            len: 1,
            access: runtime::guest::Access::Read,
            reason: runtime::guest::FaultReason::Unmapped,
        }
    }
}

/// The block `info` names, read and fixed up at its first ask; `None`
/// with no map file reader installed (a test), when the guest's false
/// follows.
pub(crate) fn late_block(g: &impl Memory, store: &Store, info: u32) -> Result<Option<u32>, GuestFault> {
    let mut files = FILES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(token) = store.section(info) {
        return Ok(Some(token));
    }
    let Some(files) = files.as_mut() else {
        return Ok(None);
    };
    let (head, resources) = info_at(g, info)?;
    if head.size == 0 || head.size > store::MAX_BLOCK {
        return Ok(None);
    }
    let bytes = files.read_raw(head.raw, head.size).map_err(late_fault(info))?;
    store
        .add_late(|b| b.add_block(info, head, &resources, &bytes))
        .map_err(|e| late_fault(info)(e.to_string()))?;
    Ok(store.section(info))
}

/// The pixels of the image at `image`, read at its first ask.
pub(crate) fn late_pixels(g: &impl Memory, store: &Store, image: u32) -> Result<Option<u32>, GuestFault> {
    let mut files = FILES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(token) = store.pixels(image) {
        return Ok(Some(token));
    }
    let Some(files) = files.as_mut() else {
        return Ok(None);
    };
    let pointer = g.u32(image.wrapping_add(IMAGE_POINTER))?;
    let size = g.u32(image.wrapping_add(IMAGE_SIZE))?;
    if pointer == u32::MAX || size == 0 || size > store::MAX_BLOCK {
        return Ok(None);
    }
    let bytes = files.read_raw(pointer, size).map_err(late_fault(image))?;
    store.add_late(|b| b.add_pixels(image, &bytes)).map_err(|e| late_fault(image)(e.to_string()))?;
    Ok(store.pixels(image))
}

/// Which structure a store belongs to. A snapshot load restores another
/// map's RAM without a map load, so every lookup compares it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Key {
    index: u32,
    bsp: u32,
    ltmp: u32,
    checksum: u32,
}

fn key(g: &impl Memory) -> Result<Option<Key>, GuestFault> {
    let bsp = g.u32(ACTIVE_BSP)?;
    let ltmp = g.u32(ACTIVE_LIGHTMAP)?;
    if bsp == 0 || ltmp == 0 {
        return Ok(None);
    }
    Ok(Some(Key {
        index: g.u32(TAG_INDEX)?,
        bsp,
        ltmp,
        checksum: g.u32(bsp.wrapping_add(8))?,
    }))
}

enum State {
    Empty,
    Ready(Arc<Store>, Key),
    /// Loading failed for this structure; not retried until it changes.
    Failed(Arc<str>, Key),
}

static STATE: RwLock<State> = RwLock::new(State::Empty);
static FILES: Mutex<Option<Box<dyn RawFiles>>> = Mutex::new(None);

static ANSWERS: AtomicU64 = AtomicU64::new(0);
static GAPS: AtomicU64 = AtomicU64::new(0);
static FILLS: AtomicU64 = AtomicU64::new(0);
static LOADS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);
static BLOCKS: AtomicU64 = AtomicU64::new(0);

/// Counts since start, and the store now loaded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// `0x000d1e10` calls answered from the store.
    pub answers: u64,
    /// Calls left to the guest's body ([`Gap`]); each kind is named on
    /// stderr the first time.
    pub gaps: u64,
    /// Tick records sampled ([`Fill`]).
    pub fills: u64,
    pub loads: u64,
    pub bytes: u64,
    pub blocks: u64,
}

pub fn stats() -> Stats {
    Stats {
        answers: ANSWERS.load(Ordering::Relaxed),
        gaps: GAPS.load(Ordering::Relaxed),
        fills: FILLS.load(Ordering::Relaxed),
        loads: LOADS.load(Ordering::Relaxed),
        bytes: BYTES.load(Ordering::Relaxed),
        blocks: BLOCKS.load(Ordering::Relaxed),
    }
}

/// The map file reader the store loads through.
pub fn set_raw_files(files: Box<dyn RawFiles>) {
    *FILES.lock().unwrap_or_else(|e| e.into_inner()) = Some(files);
}

/// A map is loading, the structure switched, or a live edit wrote raw
/// geometry or bitmap data: drop the store. The next sample loads again.
pub fn invalidate() {
    *STATE.write().unwrap_or_else(|e| e.into_inner()) = State::Empty;
    BYTES.store(0, Ordering::Relaxed);
    BLOCKS.store(0, Ordering::Relaxed);
}

fn held(k: Key) -> Option<Result<Arc<Store>, Gap>> {
    match &*STATE.read().unwrap_or_else(|e| e.into_inner()) {
        State::Ready(s, held) if *held == k => Some(Ok(Arc::clone(s))),
        State::Failed(why, held) if *held == k => Some(Err(Gap::Load(Arc::clone(why)))),
        _ => None,
    }
}

fn failed(why: String, k: Key) -> Gap {
    eprintln!("lightmap store: not loaded: {why}");
    let why: Arc<str> = why.into();
    *STATE.write().unwrap_or_else(|e| e.into_inner()) = State::Failed(Arc::clone(&why), k);
    BYTES.store(0, Ordering::Relaxed);
    BLOCKS.store(0, Ordering::Relaxed);
    Gap::Load(why)
}

/// The store for the structure `g` has active, loading it on the first
/// call after a map load, a structure switch or a snapshot of another.
pub fn ensure(g: &runtime::Guest) -> Result<Arc<Store>, Gap> {
    let fault = |e: GuestFault| Gap::Load(format!("the map is unreadable: {e}").into());
    let Some(k) = key(g).map_err(fault)? else {
        return Err(Gap::NoStructure);
    };
    if let Some(held) = held(k) {
        return held;
    }
    let mut files = FILES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(held) = held(k) {
        return held;
    }
    let Some(files) = files.as_mut() else {
        return Err(Gap::NoFiles);
    };
    let window = (u64::from(WINDOW), u64::from(WINDOW) + u64::from(WINDOW_LEN));
    if let Some(m) = g.mappings().iter().find(|m| {
        let start = u64::from(m.virtual_start);
        start < window.1 && window.0 < start + u64::from(m.len)
    }) {
        return Err(failed(format!("a guest mapping at {:#010x} overlaps the token window", m.virtual_start), k));
    }
    files.bind(g);
    let started = std::time::Instant::now();
    match load(g, k.bsp, k.ltmp, |pointer, size| files.read_raw(pointer, size)) {
        Ok(s) => {
            LOADS.fetch_add(1, Ordering::Relaxed);
            BYTES.store(s.footprint() as u64, Ordering::Relaxed);
            BLOCKS.store(s.blocks() as u64, Ordering::Relaxed);
            eprintln!(
                "lightmap store: {} blocks, {} images, {:.1} MiB, loaded in {} ms",
                s.blocks(),
                s.images(),
                s.footprint() as f64 / (1 << 20) as f64,
                started.elapsed().as_millis()
            );
            let s = Arc::new(s);
            *STATE.write().unwrap_or_else(|e| e.into_inner()) = State::Ready(Arc::clone(&s), k);
            Ok(s)
        }
        Err(why) => Err(failed(why, k)),
    }
}

/// Install `store` for the structure `g` has active (a test fixture).
pub fn install(g: &impl Memory, store: Store) -> Result<Arc<Store>, GuestFault> {
    let k = key(g)?.unwrap_or(Key { index: 0, bsp: 0, ltmp: 0, checksum: 0 });
    let s = Arc::new(store);
    *STATE.write().unwrap_or_else(|e| e.into_inner()) = State::Ready(Arc::clone(&s), k);
    Ok(s)
}

static NAMED: Mutex<Vec<std::mem::Discriminant<Gap>>> = Mutex::new(Vec::new());

/// Count a gap and name its kind on stderr the first time.
pub fn gap(g: &Gap) {
    GAPS.fetch_add(1, Ordering::Relaxed);
    let kind = std::mem::discriminant(g);
    let mut named = NAMED.lock().unwrap_or_else(|e| e.into_inner());
    if !named.contains(&kind) {
        named.push(kind);
        eprintln!("lightmap store: 0x000d1e10 left to the guest: {g}");
    }
}

/// `0x000d1e10` from `store`, at its entry: the port runs over a
/// [`View`], its stores outside its frame are committed in order, EAX
/// takes its result and the call returns (`ret 0x10`). ECX and EDX are
/// the guest's scratch and are left as they were; EBX, ESI, EDI and EBP
/// are the callee's to keep, and the port changes none.
pub fn answer(c: &mut CpuAbi, g: &mut runtime::Guest, store: &Store) -> Result<TransferAbi, GuestFault> {
    let ret = g.u32(c.esp)?;
    let mut args = [0u32; 4];
    for (i, a) in args.iter_mut().enumerate() {
        *a = g.u32(c.esp.wrapping_add(4 + 4 * i as u32))?;
    }
    let sampled = sample::sample(&View::new(&*g, store), args)?;
    // A store into a section the store holds (`0x000d35a0`'s `[rec+4]`,
    // `[rec+8]`) stays out of it: the store is the map file's bytes, and
    // the port sets both again before it reads them.
    for (addr, bytes) in sampled.writes.iter().filter(|(a, _)| !is_token(*a)) {
        g.write_bytes(*addr, bytes, 1)?;
    }
    c.eax = sampled.eax;
    c.esp = c.esp.wrapping_add(4 + 0x10);
    ANSWERS.fetch_add(1, Ordering::Relaxed);
    Ok(TransferAbi::ret(ret))
}

/// The tick's two record reads: the return address of their `0x0003ddd0`
/// call, and the register holding the object there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Site {
    /// The effect update's step 5 (`engine::effect_update::alive`): ESI.
    Update,
    /// The effect new's `0x0017b5d0`: EDI.
    New,
}

impl Site {
    pub const UPDATE: u32 = 0x0017_96e2;
    pub const NEW: u32 = 0x0017_b61f;

    pub fn of(ret: u32) -> Option<Self> {
        match ret {
            Self::UPDATE => Some(Self::Update),
            Self::NEW => Some(Self::New),
            _ => None,
        }
    }

    pub fn object(self, c: &CpuAbi) -> u32 {
        match self {
            Self::Update => c.esi,
            Self::New => c.edi,
        }
    }
}

/// `0x0003ddd0` at its entry, for every caller: the record of the object
/// the stack names, by [`crate::render::object_lighting::record`] under
/// the caller's MXCSR and x87 control word. EAX takes the handle (-1 for
/// none) and ESP is past the return address and the argument, as `ret 4`
/// leaves them. ECX, EDX and the flags are the guest's scratch, which no
/// caller reads after the call; EBX, ESI, EDI and EBP are unchanged, as
/// the guest restores them. The return address comes back for the
/// transfer.
pub fn record_call(c: &mut CpuAbi, g: &mut dyn crate::GuestMemory) -> Result<u32, GuestFault> {
    let fault = |f: crate::MemFault| GuestFault {
        addr: f.addr,
        len: f.len as usize,
        access: if f.write { runtime::guest::Access::Write } else { runtime::guest::Access::Read },
        reason: runtime::guest::FaultReason::Unmapped,
    };
    let ret = g.u32(c.esp).map_err(fault)?;
    let object = g.u32(c.esp.wrapping_add(4)).map_err(fault)?;
    let fpu = crate::light_constants::Fpu {
        mxcsr: c.mxcsr,
        x87_control: c.x87.control,
    };
    use crate::light_constants::Error;
    c.eax = match crate::render::object_lighting::record(g, object, fpu) {
        Ok(handle) => handle,
        Err(Error::Memory(f)) => return Err(fault(f)),
        Err(e) => unimplemented!("lightmap: record of {object:#010x}: {e:?}"),
    };
    c.esp = c.esp.wrapping_add(8);
    Ok(ret)
}

/// The tick record's sample: `0x000d1850` by the port
/// ([`object::sample`]) as `0x0003e146` calls it, EDI the object and the
/// arguments [`Fill::args`], its frame below `entry` (the record get's
/// ESP at its entry), entry J natively ([`object::collide`]) under `c`'s
/// float state; then [`Fill::finish`]. With no store the sample cannot be
/// made, and stops by name.
pub fn tick_sample(c: &CpuAbi, g: &mut runtime::Guest, entry: u32, fill: Fill) -> Result<bool, GuestFault> {
    let store = match ensure(g) {
        Ok(store) => store,
        Err(gap) => unimplemented!("lightmap: the tick record's sample: {gap}"),
    };
    let fpu = crate::light_constants::Fpu {
        mxcsr: c.mxcsr,
        x87_control: c.x87.control,
    };
    let [out, first] = fill.args();
    // The two pushes and the call: the sample's return address 12 below.
    let at = entry.wrapping_sub(12);
    let cpu = *c;
    let sampled = {
        let j = |g: &mut runtime::Guest, esp| object::collide(&cpu, g, esp);
        let mut r = object::Run::new(&mut *g, &store, fpu, j);
        object::push_args(&mut r, at, 0x0003_e14b, &[first, out])?;
        object::sample(&mut r, at, fill.object, [first, out])?
    };
    fill.finish(g, sampled)?;
    Ok(sampled)
}

/// A tick record to sample: after `0x0003ddd0` returned `handle` for the
/// object `object`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fill {
    pub object: u32,
    /// The record's address.
    pub record: u32,
}

/// The sample lands at `+0x30`, 0x14 bytes, as `0x0003e13c` passes it.
pub const RECORD_SAMPLE: u32 = 0x30;
/// The copy the tick reads: `+0x1c..+0x30`, words 3 and 4 at `+0x28`,
/// `+0x2c` (`0x0003e28a`).
pub const RECORD_LIGHT: u32 = 0x1c;
/// The record's sampled byte (`0x0003e14f`).
pub const RECORD_SAMPLED: u32 = 2;

impl Fill {
    /// The record `handle` names, `None` for -1.
    pub fn new(g: &impl Memory, object: u32, handle: u32) -> Result<Option<Self>, GuestFault> {
        if handle == u32::MAX {
            return Ok(None);
        }
        let base = g.u32(g.u32(RECORDS)?.wrapping_add(0x44))?;
        Ok(Some(Self {
            object,
            record: base.wrapping_add((handle & 0xffff) << 8),
        }))
    }

    /// The call of `0x000d1850` to make: EDI the object, the first-sample
    /// byte 1 and the record's `+0x30` pushed as `0x0003e13f`..`0x0003e146`
    /// push them. The caller pushes [`Self::args`] in order and calls.
    pub fn args(&self) -> [u32; 2] {
        [self.record.wrapping_add(RECORD_SAMPLE), 1]
    }

    /// After `0x000d1850` returned `sampled` (AL): the sample's copy at
    /// `+0x1c` and the sampled byte, as `0x0003e14f` and `0x0003e28a` make
    /// them for a first sample. No blend: the frame step's pacing
    /// (`0x004ba034` against `+8`) is the render's, and the tick has none.
    pub fn finish(&self, g: &mut impl Memory, sampled: bool) -> Result<(), GuestFault> {
        if !sampled {
            return Ok(());
        }
        let mut sample = [0u8; 0x14];
        g.read_bytes(self.record.wrapping_add(RECORD_SAMPLE), &mut sample, 1)?;
        g.write_bytes(self.record.wrapping_add(RECORD_LIGHT), &sample, 1)?;
        g.set_u8(self.record.wrapping_add(RECORD_SAMPLED), 1)?;
        FILLS.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
