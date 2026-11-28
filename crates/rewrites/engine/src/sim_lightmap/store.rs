//! The lightmap store's bytes: every geometry block the sampler reads of
//! the active structure and its lightmap, and the lightmap bitmaps'
//! pixels, read from the map file once per structure
//! (docs/engine/sim-render-split.md, "6. Render scratch and stamps").
//!
//! The guest reads these through its geometry cache (`0x0012de70`, which
//! loads a block and runs the fixup `0x00223c70` on it) and its texture
//! cache, so which bytes it can read depends on what the render asked for
//! last. The store holds them all, laid out as the caches lay them out
//! once resident, at token addresses ([`WINDOW`] up): a block's section
//! has its resource fields pointed at the store's copy of each resource,
//! as `0x00223c70` points them into the cache. The bytes are the map
//! file's; the window is never mapped in the guest.
use super::view::{WINDOW, WINDOW_LEN};
use runtime::GuestFault;
use std::collections::HashMap;
use std::sync::RwLock;

/// Bases are aligned to this: the cache's own alignment for a block.
pub const ALIGN: u32 = 16;
/// Largest block or image level read, checked before reading
/// (`tags::geometry::MAX_BLOCK_SIZE`).
pub const MAX_BLOCK: u32 = 16 << 20;

/// The geometry block info's fields the fixup reads (`0x00223c70`;
/// docs/formats/structure-bsp.md, "Geometry block info").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Info {
    /// `+0`: the raw pointer, the file in the top two bits.
    pub raw: u32,
    /// `+4`: the whole block, `blkh` to `blkf`.
    pub size: u32,
    /// `+8`: the section struct after the 8-byte header.
    pub section: u32,
}

/// One block resource entry (`+0x10` of the info, 16 bytes each).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Resource {
    /// `+0`: 0 tag block, 1 data, 2 vertex buffer.
    pub kind: u8,
    /// `+4`: the field patched in the section.
    pub field: u16,
    /// `+6`: element size (0), `0xffff` (1), descriptor index or `0xffff` (2).
    pub secondary: u16,
    /// `+8`.
    pub size: u32,
    /// `+0xc`: offset in the resource area; -1 skips the entry.
    pub offset: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BuildError {
    /// The bytes read are not the info's size, or lack the framing.
    Framing { info: u32, why: &'static str },
    /// A resource the fixup would write outside the section, or a
    /// division the guest would fault on.
    Fixup { info: u32, resource: usize, why: &'static str },
    /// The map's blocks and pixels do not fit the window.
    TooLarge { bytes: u64 },
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Framing { info, why } => write!(f, "block info {info:#010x}: {why}"),
            Self::Fixup { info, resource, why } => {
                write!(f, "block info {info:#010x}, resource {resource}: {why}")
            }
            Self::TooLarge { bytes } => {
                write!(f, "{bytes} bytes of lightmap data do not fit the {WINDOW_LEN:#x}-byte window")
            }
        }
    }
}

impl std::error::Error for BuildError {}

/// Collects blocks and pixels; [`Builder::finish`] seals them.
#[derive(Default)]
pub struct Builder {
    /// The window offset of `bytes[0]`: 0 for the structure's store, the
    /// next free offset for a block or image loaded at its first ask.
    origin: u32,
    bytes: Vec<u8>,
    sections: HashMap<u32, u32>,
    images: HashMap<u32, u32>,
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().expect("four bytes"))
}

fn put(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

impl Builder {
    /// Room for `len` bytes at the next aligned base: its offset.
    fn reserve(&mut self, len: u32) -> Result<usize, BuildError> {
        let base = (self.bytes.len() as u64).next_multiple_of(u64::from(ALIGN));
        let end = base + u64::from(len);
        if u64::from(self.origin) + end > u64::from(WINDOW_LEN) {
            return Err(BuildError::TooLarge { bytes: end });
        }
        self.bytes.resize(end as usize, 0);
        Ok(base as usize)
    }

    fn token(&self, offset: usize) -> u32 {
        WINDOW.wrapping_add(self.origin).wrapping_add(offset as u32)
    }

    /// Whether the block info at guest `info` is held already.
    pub fn contains(&self, info: u32) -> bool {
        self.sections.contains_key(&info)
    }

    /// Add the block `info_addr` names: `block` is its raw bytes, `blkh`
    /// to `blkf`. The section is fixed up as `0x00223c70` does with the
    /// buffer at the block's token:
    ///
    /// - the section starts 8 bytes in; each resource's data at the
    ///   section's end plus its offset (`0x00223cb7`..`0x00223cc0`);
    /// - kind 0: the field gets the count `size / element` and the data
    ///   pointer (`0x00223d40`..`0x00223d52`);
    /// - kind 1: the size and the data pointer (`0x00223d35`);
    /// - kind 2: the descriptor (the field itself for `0xffff`, else
    ///   entry `secondary` of the 0x20-byte array the field points at)
    ///   gets `+2` = `(size - [+8]) / byte [+1]`, `+0xc` = data - `[+4]`,
    ///   `+0x10` = itself `+0x14`, `+0x14` = 1, `+0x18` = (data - `[+4]`)
    ///   & `0x0fffffff`, `+0x1c` = 0 (`0x00223ccf`..`0x00223d30`).
    ///
    /// The info's own runtime fields (the runtime block at `+0x1c`, the
    /// loaded flag, the cache handle) are the guest's and stay untouched:
    /// the store answers [`Store::section`] in their place.
    pub fn add_block(
        &mut self,
        info_addr: u32,
        info: Info,
        resources: &[Resource],
        block: &[u8],
    ) -> Result<(), BuildError> {
        let framing = |why| BuildError::Framing { info: info_addr, why };
        if block.len() != info.size as usize {
            return Err(framing("the bytes read are not the block's size"));
        }
        if block.len() < 12 || &block[..4] != b"hklb" && &block[..4] != b"blkh" {
            return Err(framing("no blkh header"));
        }
        let section = info.section as usize;
        if 8 + section > block.len() {
            return Err(framing("the section runs past the block"));
        }
        let base = self.reserve(info.size)?;
        self.bytes[base..base + block.len()].copy_from_slice(block);
        let s = base + 8;
        let area = s + section;
        let end = base + block.len();
        for (i, r) in resources.iter().enumerate() {
            if r.offset == u32::MAX {
                continue;
            }
            let fixup = |why| BuildError::Fixup { info: info_addr, resource: i, why };
            let data = area + r.offset as usize;
            if data > end {
                return Err(fixup("the resource starts past the block"));
            }
            let field = s + usize::from(r.field);
            let token = self.token(data);
            match r.kind {
                0 | 1 => {
                    if field + 8 > area {
                        return Err(fixup("the field is outside the section"));
                    }
                    let count = if r.kind == 0 {
                        let element = i32::from(r.secondary);
                        if element == 0 {
                            return Err(fixup("a tag block of zero-byte elements"));
                        }
                        (r.size as i32).wrapping_div(element) as u32
                    } else {
                        r.size
                    };
                    put(&mut self.bytes, field, count);
                    put(&mut self.bytes, field + 4, token);
                }
                _ => {
                    let at = if r.secondary == u16::MAX {
                        field
                    } else {
                        if field + 8 > area {
                            return Err(fixup("the field is outside the section"));
                        }
                        let array = u32_at(&self.bytes, field + 4);
                        let Some(array) = self.offset_of(array) else {
                            return Err(fixup("the descriptor array is not in the store"));
                        };
                        array + 0x20 * usize::from(r.secondary)
                    };
                    if at + 0x20 > self.bytes.len() {
                        return Err(fixup("the descriptor runs past the store"));
                    }
                    let stride = i32::from(self.bytes[at + 1]);
                    if stride == 0 {
                        return Err(fixup("a vertex buffer of zero-byte vertices"));
                    }
                    let count = (r.size.wrapping_sub(u32_at(&self.bytes, at + 8)) as i32).wrapping_div(stride);
                    let rel = token.wrapping_sub(u32_at(&self.bytes, at + 4));
                    put(&mut self.bytes, at + 0xc, rel);
                    self.bytes[at + 2..at + 4].copy_from_slice(&(count as u16).to_le_bytes());
                    let own = self.token(at + 0x14);
                    put(&mut self.bytes, at + 0x10, own);
                    put(&mut self.bytes, at + 0x14, 1);
                    put(&mut self.bytes, at + 0x18, rel & 0x0fff_ffff);
                    put(&mut self.bytes, at + 0x1c, 0);
                }
            }
        }
        let section_token = self.token(s);
        self.sections.insert(info_addr, section_token);
        Ok(())
    }

    /// Add the pixels of the bitmap image record at guest `image`: its
    /// highest level as the map file holds it.
    pub fn add_pixels(&mut self, image: u32, pixels: &[u8]) -> Result<(), BuildError> {
        if self.images.contains_key(&image) {
            return Ok(());
        }
        let base = self.reserve(pixels.len() as u32)?;
        self.bytes[base..base + pixels.len()].copy_from_slice(pixels);
        let pixels_token = self.token(base);
        self.images.insert(image, pixels_token);
        Ok(())
    }

    fn offset_of(&self, token: u32) -> Option<usize> {
        let o = token.wrapping_sub(WINDOW).wrapping_sub(self.origin) as usize;
        (super::view::is_token(token) && o < self.bytes.len()).then_some(o)
    }

    fn chunk(mut self) -> Chunk {
        self.bytes.shrink_to_fit();
        Chunk {
            origin: self.origin,
            bytes: self.bytes.into_boxed_slice(),
            sections: self.sections,
            images: self.images,
        }
    }

    pub fn finish(self) -> Store {
        let main = self.chunk();
        let next = (main.origin as u64 + main.bytes.len() as u64).next_multiple_of(u64::from(ALIGN)) as u32;
        Store {
            main,
            late: RwLock::new(Late { next, chunks: Vec::new() }),
        }
    }
}

/// One span of the window: its bytes and what they hold.
struct Chunk {
    origin: u32,
    bytes: Box<[u8]>,
    /// Guest block info address -> its section's token.
    sections: HashMap<u32, u32>,
    /// Guest bitmap image record address -> its pixels' token.
    images: HashMap<u32, u32>,
}

impl Chunk {
    fn read(&self, token: u32, dst: &mut [u8]) -> Option<()> {
        let o = token.wrapping_sub(WINDOW).checked_sub(self.origin)? as usize;
        let b = self.bytes.get(o..o.checked_add(dst.len())?)?;
        dst.copy_from_slice(b);
        Some(())
    }
}

/// Blocks and images loaded at their first ask: a model's sections and
/// the shaders' base maps, which the structure's load does not list.
/// Their bytes are the map file's whenever they are read, so when they
/// load does not change what a sample returns.
struct Late {
    next: u32,
    chunks: Vec<Chunk>,
}

/// The active structure's lightmap data. What it holds never changes once
/// held; dropped as a whole at the next map load or structure switch.
pub struct Store {
    main: Chunk,
    late: RwLock<Late>,
}

fn unmapped(addr: u32, len: usize) -> GuestFault {
    GuestFault {
        addr,
        len,
        access: runtime::guest::Access::Read,
        reason: runtime::guest::FaultReason::Unmapped,
    }
}

impl Store {
    pub fn builder() -> Builder {
        Builder::default()
    }

    /// An empty store: every lookup answers `None`.
    pub fn empty() -> Self {
        Builder::default().finish()
    }

    fn late(&self) -> std::sync::RwLockReadGuard<'_, Late> {
        self.late.read().unwrap_or_else(|e| e.into_inner())
    }

    /// The section token for the block info at `info`.
    pub fn section(&self, info: u32) -> Option<u32> {
        self.main.sections.get(&info).copied().or_else(|| {
            self.late().chunks.iter().find_map(|c| c.sections.get(&info).copied())
        })
    }

    /// The pixels token for the bitmap image record at `image`.
    pub fn pixels(&self, image: u32) -> Option<u32> {
        self.main.images.get(&image).copied().or_else(|| {
            self.late().chunks.iter().find_map(|c| c.images.get(&image).copied())
        })
    }

    /// Add one block or image at its first ask: `fill` adds it to a
    /// builder placed after everything held. A second ask for the same
    /// record finds the first one's.
    pub fn add_late(&self, fill: impl FnOnce(&mut Builder) -> Result<(), BuildError>) -> Result<(), BuildError> {
        let mut late = self.late.write().unwrap_or_else(|e| e.into_inner());
        let mut b = Builder { origin: late.next, ..Builder::default() };
        fill(&mut b)?;
        let chunk = b.chunk();
        late.next = (u64::from(chunk.origin) + chunk.bytes.len() as u64).next_multiple_of(u64::from(ALIGN)) as u32;
        late.chunks.push(chunk);
        Ok(())
    }

    /// `dst.len()` bytes at `token`; a fault outside what is held.
    pub fn read(&self, token: u32, dst: &mut [u8]) -> Result<(), GuestFault> {
        if self.main.read(token, dst).is_some() {
            return Ok(());
        }
        let late = self.late();
        late.chunks
            .iter()
            .find_map(|c| c.read(token, dst))
            .ok_or_else(|| unmapped(token, dst.len()))
    }

    /// Blocks held.
    pub fn blocks(&self) -> usize {
        self.main.sections.len() + self.late().chunks.iter().map(|c| c.sections.len()).sum::<usize>()
    }

    /// Images held.
    pub fn images(&self) -> usize {
        self.main.images.len() + self.late().chunks.iter().map(|c| c.images.len()).sum::<usize>()
    }

    /// Host bytes held.
    pub fn footprint(&self) -> usize {
        self.main.bytes.len() + self.late().chunks.iter().map(|c| c.bytes.len()).sum::<usize>()
    }
}
