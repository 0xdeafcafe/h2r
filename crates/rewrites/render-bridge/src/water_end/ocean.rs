//! The BSP ocean of the water end (Code, `0x0003c3ff..0x0003c632`;
//! docs/graphics/retire-d3d-display.md, "The water end"), in Rust.
//!
//! [`run`] makes the body's steps in the guest's order, calls no guest
//! helper, and returns what the body leaves in EAX and ECX:
//!
//! | Step | Made |
//! |---|---|
//! | grid `0x0003ca90` | Rust ([`engine::ocean_grid`]), its aspect widened to [`grid_lens`] |
//! | gate `0x0003c270`, the ocean's occlusion query | [`Device::visible`]: Rust over the native flare query's count pass ([`gate`]) |
//! | gate `0x0012de70`, the texture block's residency | Rust ([`cache::resident`]); a closed gate (an empty block, or one still loading) goes to the stamps |
//! | the shader `[definition + 4]` to its implementation's flags `+0x118`, an `MTRP` or `3trp`'s template through its group's object (`0x00137bd0`) | Rust ([`shader`], [`template_block`]) |
//! | copy `0x0003bea0` (bit 20) | Rust ([`first_copy`]): the frame's first call restores the open shader, copies the bound colour into `Game(0x16)` through [`Host::copy_view`](crate::native_targets::Host::copy_view) and sets the frame's flags |
//! | pass `0x00131ed0`, `0x0003bd00` (bit 19) | Rust ([`reflection::pass`]): the sky drawn into `Game(0x15)` from the camera mirrored about the water's height |
//! | alpha clear `0x0003c515` | `native_targets::cut_clear(0x10, 0x00ffffff, 0, 0)` on the bound target |
//! | the draw state, shader begin `0x0001bbf0`, quads `0x0001c8a0` | [`Device::draw`]: the native ocean program from the map over the frame's record ([`Draw`], `frame_scene::late::Ocean`) |
//! | stamps `0x0003bcb0` | Rust per image ([`stamps`], [`cache::stamp`]) |
//!
//! No device state of the draw is written: the stream record
//! `0x0051f0f0`, the block at `0x004c1a90`, the shader cache
//! `0x00485b48` and the stage caches keep what the draw before left.
//!
//! What the stage cannot make is a cause, `unimplemented!("water ocean:
//! <cause>")`, or for the copies `unimplemented!("water ocean copy:
//! <cause>")` (`copy_cache`, `copy_restore`, `copy_target`, and the
//! reflection's, [`reflection`]):
//! `unread`, `unwritable`, `host`, `clear`,
//! `structure` (the upload's float model refused), `quads` (no keyed
//! view), `program` (the load compiled no ocean pass), `stream` (the
//! grid's vertices do not read or match the program), `target` (a
//! sampled target not made or aliased), `draw` (the native draw refused),
//! `flare_host`, `placed` (the gate's count pass not placed). A texture
//! block still loading closes the gate for the frame and an image the
//! texture cache does not hold yet is left unstamped ([`cache`]). The
//! grid's are
//! `unimplemented!("water ocean grid: <cause>")`: `unread`, `unwritable`,
//! `simd` (an unmasked SSE exception), `x87` (an unmasked x87 exception
//! or an operation the model stops on).

use std::sync::atomic::{AtomicU32, Ordering};

use engine::light_constants::Error as FloatError;
use engine::memory::{GuestMemory, MemFault};
use engine::ocean_grid;

use super::{BSP, WATER_COUNT};
use crate::native_targets::Host;

pub mod cache;
pub mod gate;
pub mod reflection;
#[cfg(test)]
mod fake;

/// A step the stage cannot make.
pub type Cause = &'static str;

/// The tag table's pointer: entries of 16 bytes, the group at `+0`, the
/// data at `+8`.
pub const TAGS: u32 = 0x004e_3b44;
/// The groups whose template is a virtual call (`0x00137bd0`, `[vtable +
/// 0x28]`), and the static objects it picks for them (vtables
/// `0x004535f0` and `0x00453598`).
pub const GROUP_MTRP: u32 = 0x5052_544d;
pub const GROUP_3TRP: u32 = 0x7072_7433;
pub const OBJECT_3TRP: u32 = 0x0047_9868;
pub const OBJECT_MTRP: u32 = 0x0047_9874;
/// The definition's words.
pub const SHADER: u32 = 4;
pub const GEOMETRY: u32 = 0xc;
pub const TEXTURE_BLOCK: u32 = 0x10;
/// The geometry's words: its stream record, the quads' index count and
/// list.
pub const GEOMETRY_STATE: u32 = 0x3c;
pub const QUAD_COUNT: u32 = 0x20;
pub const QUAD_INDICES: u32 = 0x24;
/// The frame's structure target copy flag (`0x0003bea0` sets it; the frame
/// step clears it) and the game target the copy is made into.
pub const TARGET_COPIED: u32 = 0x0050_9430;
pub const COPY_INTO: u32 = 0x16;
/// The implementation flags' copy bits.
pub const COPY_BIT: u32 = 20;
pub const PASS_BIT: u32 = 19;

/// What the body leaves at its return: `mov al, 1` over EAX, and ECX.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ended {
    pub eax: u32,
    pub ecx: u32,
}

/// The grid's stream as the geometry names it (`[geometry + 0x3c]`, the
/// record `0x0001cda0` reads): its type in the format table `0x0043f408`
/// (`+0`), its vertex buffer (`+8`, a D3D resource) and byte offset
/// (`+0x10`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stream {
    pub kind: u8,
    pub buffer: u32,
    pub offset: u32,
}

impl Stream {
    pub fn read(mem: &dyn GuestMemory, at: u32) -> Result<Self, Cause> {
        Ok(Self { kind: mem.u8(at).map_err(unread)?, buffer: word(mem, at.wrapping_add(8))?, offset: word(mem, at.wrapping_add(0x10))? })
    }

    /// The first vertex's address: the buffer's data word (`+4`, physical)
    /// in the contiguous alias, plus the offset.
    pub fn vertices(&self, mem: &dyn GuestMemory) -> Result<u32, Cause> {
        Ok((word(mem, self.buffer.wrapping_add(4))? & 0x0fff_ffff | 0x8000_0000).wrapping_add(self.offset))
    }
}

/// One ocean draw: the frame's record, the quads (`count` indices at
/// `indices`, a quad list) and the grid's stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Draw {
    pub ocean: frame_scene::late::Ocean,
    pub indices: u32,
    pub count: u32,
    pub stream: Stream,
}

/// The device side: boot's glue in a game, a recorder in the tests.
pub trait Device {
    /// The device calls of the clear and the copy.
    fn host(&self) -> &dyn Host;
    /// 64 bytes of guest stack the clear stages its words in.
    fn scratch(&self) -> u32;
    /// The ocean's pass from the map over `draw`, on the bound target.
    fn draw(&mut self, mem: &mut dyn GuestMemory, draw: &Draw) -> Result<(), Cause>;
    /// The gate `0x0003c270(definition)`: whether the ocean is seen
    /// ([`gate::visible`] over the native flare query).
    fn visible(&mut self, mem: &mut dyn GuestMemory, definition: u32) -> Result<bool, Cause>;
    /// The sky `0x0003ede0` on the bound target with the device's view
    /// (the reflection's, [`reflection::pass`]): the native sky's cut.
    fn sky(&mut self, mem: &mut dyn GuestMemory) -> Result<(), Cause>;
    /// The caller's MXCSR, which the grid's sticky flags land in, and its
    /// x87 control word.
    fn fp(&mut self) -> (&mut u32, u16);
    /// The displayed lens's factor the grid's aspect widens by.
    fn lens(&self) -> f32 {
        grid_lens()
    }
}

/// The displayed lens's factor, as f32 bits: 1 (the retail grid) until
/// render-aspect sets its view lens.
static GRID_LENS: AtomicU32 = AtomicU32::new(0x3f80_0000);

/// Sets the factor the grid's aspect widens by
/// ([`ocean_grid::widen_aspect`]): render-aspect's view lens, the
/// fitted guest camera's or the device's per-draw lens.
pub fn set_grid_lens(factor: f32) {
    GRID_LENS.store(factor.to_bits(), Ordering::Relaxed);
}

pub fn grid_lens() -> f32 {
    f32::from_bits(GRID_LENS.load(Ordering::Relaxed))
}

/// The grid's cause.
fn grid_cause(e: FloatError) -> Cause {
    match e {
        FloatError::Memory(MemFault { write: true, .. }) => "unwritable",
        FloatError::Memory(_) => "unread",
        FloatError::Simd => "simd",
        FloatError::X87(_) => "x87",
        _ => "other",
    }
}

fn grid_unported(cause: Cause) -> ! {
    unimplemented!("water ocean grid: {cause}")
}

/// The shader's words the body reads (`0x0003c439..0x0003c4d6`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shader {
    /// `[definition + 4]`, the shader's tag index.
    pub index: u32,
    /// The implementation's `+0x100`, left in EDI.
    pub word: u32,
    /// The implementation's flags `+0x118`.
    pub flags: u32,
    /// ESI, ECX and EDX as the lookup leaves them.
    pub esi: u32,
    pub ecx: u32,
    pub edx: u32,
}

impl Shader {
    pub fn copies(&self) -> bool {
        (self.flags >> COPY_BIT) & 1 != 0
    }
    pub fn passes(&self) -> bool {
        (self.flags >> PASS_BIT) & 1 != 0
    }
}

fn unread(_: MemFault) -> Cause {
    "unread"
}

fn word(mem: &dyn GuestMemory, at: u32) -> Result<u32, Cause> {
    mem.u32(at).map_err(unread)
}

/// Tag `index`'s data pointer (`index & 0xffff`).
fn tag_data(mem: &dyn GuestMemory, index: u32) -> Result<u32, Cause> {
    let table = word(mem, TAGS)?;
    word(mem, table.wrapping_add((index & 0xffff) << 4).wrapping_add(8))
}

/// `0x0003bea0` (Code, `0x0003bea0..0x0003beff`): the frame's first call
/// (the flag [`TARGET_COPIED`] clear) writes the cache's restore list and
/// clears the cache (`0x00016b10`, `frame_step::layer::clear_cache`), with
/// water in the BSP copies the bound colour into `Game(`[`COPY_INTO`]`)`
/// (`0x000352e0(0x16, 1)`, then the view's target bound again), sets the
/// flag and the draw byte and clears the cache again. Later calls return
/// at once. `copy_cache`, `copy_restore` and `copy_target` are the refusals.
fn first_copy(mem: &mut dyn GuestMemory, device: &dyn Device) -> Result<(), Cause> {
    if mem.u8(TARGET_COPIED).map_err(unread)? != 0 {
        return Ok(());
    }
    let host = device.host();
    let open = crate::open_shader::read(&*mem).map_err(|_| "copy_cache")?;
    if let Some(open) = &open
        && !host.restore(mem, &open.writes)
    {
        return Err("copy_restore");
    }
    let bsp = word(mem, BSP)?;
    if word(mem, bsp.wrapping_add(WATER_COUNT))? as i32 > 0 && !host.copy_view(mem, COPY_INTO) {
        return Err("copy_target");
    }
    let unwritable = |_: MemFault| "unwritable";
    mem.set_u8(TARGET_COPIED, 1).map_err(unwritable)?;
    mem.set_u8(engine::render_loop::layer::DRAW_FLAG, 1).map_err(unwritable)?;
    crate::frame_step::layer::clear_cache(mem).map_err(unwritable)
}

/// The block whose first word is shader `index`'s template
/// (`0x0003c439..0x0003c47b`): a `shad`'s `+0x24`. For `MTRP` and `3trp`,
/// `0x00137bd0` hands the index to the group's static object (`[vtable +
/// 0x50]`, `0x002243a0`: the index at `+4`, its data at `+8`) and
/// `[vtable + 0x28]` gives the block: 3trp's (`0x002242a0`) its data's
/// `+0x90`, MTRP's (`0x00224d70`) the `+0x24` of the shader its data's
/// `+0x1c` names.
pub fn template_block(mem: &mut dyn GuestMemory, index: u32) -> Result<u32, Cause> {
    let table = word(&*mem, TAGS)?;
    // `movsx ecx, ax`: the group by the signed index.
    let group = word(&*mem, table.wrapping_add(((index as u16 as i16 as i32) << 4) as u32))?;
    let object = match group {
        GROUP_3TRP => OBJECT_3TRP,
        GROUP_MTRP => OBJECT_MTRP,
        _ => return word(&*mem, tag_data(&*mem, index)?.wrapping_add(0x24)),
    };
    let data = tag_data(&*mem, index)?;
    let unwritable = |_: MemFault| "unwritable";
    mem.set_u32(object.wrapping_add(4), index).map_err(unwritable)?;
    mem.set_u32(object.wrapping_add(8), data).map_err(unwritable)?;
    if group == GROUP_3TRP {
        word(&*mem, data.wrapping_add(0x90))
    } else {
        word(&*mem, tag_data(&*mem, word(&*mem, data.wrapping_add(0x1c))?)?.wrapping_add(0x24))
    }
}

/// The lookup from the shader `index` to its implementation's flags
/// (`0x0003c439..0x0003c4d6`).
pub fn shader(mem: &mut dyn GuestMemory, index: u32) -> Result<Shader, Cause> {
    let block = template_block(mem, index)?;
    let mem = &*mem;
    let template = tag_data(mem, word(mem, block)?)?;
    let c = word(mem, template.wrapping_add(0x5c))?;
    let select = u32::from(mem.u16(word(mem, c.wrapping_add(4))?).map_err(unread)?) & 0x1ff;
    let esi = word(mem, c.wrapping_add(0xc))?;
    let options = word(mem, c.wrapping_add(0x14))?;
    let option = u32::from(mem.u16(esi.wrapping_add(select * 2 + 6)).map_err(unread)?) & 0x1ff;
    let implementation = word(mem, options.wrapping_add(option * 10 + 4))?;
    let ecx = tag_data(mem, implementation)?;
    let edx = word(mem, ecx.wrapping_add(0x20))?;
    let p = word(mem, edx.wrapping_add(4))?;
    Ok(Shader { index, word: word(mem, p.wrapping_add(0x100))?, flags: word(mem, p.wrapping_add(0x118))?, esi, ecx, edx })
}

/// The images the shader `index` stamps (`0x0003c5a9..0x0003c630`): each
/// image of each bitmap of its block, and the bitmap count the loop ends
/// on (signed, the guest's `jle`).
pub fn stamps(mem: &dyn GuestMemory, index: u32) -> Result<(Vec<u32>, i32), Cause> {
    let block = word(mem, tag_data(mem, index)?.wrapping_add(0x24))?;
    let count = word(mem, block.wrapping_add(4))? as i32;
    let mut out = Vec::new();
    for i in 0..count.max(0) as u32 {
        let elements = word(mem, block.wrapping_add(8))?;
        let bitmap = word(mem, elements.wrapping_add(i * 0xc))?;
        if bitmap == u32::MAX {
            continue;
        }
        let data = tag_data(mem, bitmap)?;
        let images = word(mem, data.wrapping_add(0x44))? as i32;
        for n in 0..images.max(0) as u32 {
            let image = word(mem, data.wrapping_add(0x48))?.wrapping_add(n * 0x74);
            if image != 0 {
                out.push(image);
            }
        }
    }
    Ok((out, count))
}

/// The ocean of `definition`, the bound target's alpha cleared first when
/// `clear`, entered with ECX `ecx`: the body's steps, then what it
/// returns.
pub fn run(mem: &mut dyn GuestMemory, device: &mut dyn Device, definition: u32, clear: bool, ecx: u32) -> Result<Ended, Cause> {
    let d = definition;
    let (mxcsr, control) = device.fp();
    let mut mx = *mxcsr;
    let grid = ocean_grid::grid(mem, d, &mut mx, control, device.lens());
    *device.fp().0 = mx;
    // A built grid leaves ECX 0; a skipped one the entry's.
    let ecx = match grid.unwrap_or_else(|e| grid_unported(grid_cause(e))) {
        ocean_grid::Grid::Built { widened, .. } => {
            crate::scene_step::count_ocean_grid(widened);
            0
        }
        ocean_grid::Grid::Skipped => ecx,
    };
    if !device.visible(mem, d)? || !cache::resident(mem, d.wrapping_add(TEXTURE_BLOCK))? {
        return stamps_then_return(mem, d, ecx);
    }
    let index = word(mem, d.wrapping_add(SHADER))?;
    if index == u32::MAX {
        return stamps_then_return(mem, d, ecx);
    }
    let s = shader(mem, index)?;
    // The copy into target 0x16 goes first, then the reflection into 0x15.
    if s.copies() {
        first_copy(mem, device)?;
    }
    if s.passes() {
        reflection::pass(mem, device, d)?;
    }
    if clear {
        let (host, scratch) = (device.host(), device.scratch());
        crate::native_targets::cut_clear(mem, host, 0x10, 0x00ff_ffff, 0, 0, scratch)
            .map_err(|cause| if cause == "device" { "clear" } else { cause })?;
    }
    let draw = draw_of(mem, d, index, s.flags, clear)?;
    device.draw(mem, &draw)?;
    stamps_then_return(mem, d, ecx)
}

/// The draw of `definition` with shader `index` (flags `flags`): the
/// record (`crate::scene_step::ocean_record`), the quads and the stream.
pub fn draw_of(mem: &dyn GuestMemory, definition: u32, index: u32, flags: u32, clear: bool) -> Result<Draw, Cause> {
    let geometry = word(mem, definition.wrapping_add(GEOMETRY))?;
    let stream = Stream::read(mem, word(mem, geometry.wrapping_add(GEOMETRY_STATE))?)?;
    let (count, indices) = (word(mem, geometry.wrapping_add(QUAD_COUNT))?, word(mem, geometry.wrapping_add(QUAD_INDICES))?);
    let ocean = crate::scene_step::ocean_record(mem, definition, index, flags, clear).ok_or("structure")?;
    Ok(Draw { ocean, indices, count, stream })
}

/// The stamps, then the return (`ecx` what ECX holds when the shader is
/// -1 and no loop runs).
fn stamps_then_return(mem: &mut dyn GuestMemory, definition: u32, ecx: u32) -> Result<Ended, Cause> {
    let index = word(mem, definition.wrapping_add(SHADER))?;
    if index == u32::MAX {
        return Ok(Ended { eax: (index & !0xff) | 1, ecx });
    }
    let (images, count) = stamps(mem, index)?;
    for image in images {
        cache::stamp(mem, image)?;
    }
    let ended = count.max(0) as u32;
    Ok(Ended { eax: (ended & !0xff) | 1, ecx: count as u32 })
}

#[cfg(test)]
mod tests;
