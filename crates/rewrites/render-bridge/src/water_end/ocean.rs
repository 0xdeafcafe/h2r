//! The BSP ocean of the water end (Code, `0x0003c3ff..0x0003c632`;
//! docs/graphics/retire-d3d-display.md, "The water end"), stepped in Rust.
//!
//! [`Ocean::next`] makes each of the body's steps in the guest's order and
//! hands back the next guest helper to call, with the registers and stack
//! arguments the body gives it, until the return:
//!
//! | Step | Made |
//! |---|---|
//! | grid `0x0003ca90`, gates `0x0003c270` and `0x0012de70` | guest calls; a gate's AL 0 goes to the stamps |
//! | the shader `[definition + 4]` to its implementation's flags `+0x118` | Rust ([`shader`]) |
//! | copy `0x0003bea0` (bit 20) | Rust ([`first_copy`]): the frame's first call restores the open shader, copies the bound colour into `Game(0x16)` through [`Host::copy_view`](crate::native_targets::Host::copy_view) and sets the frame's flags |
//! | pass `0x00131ed0`, `0x0003bd00` (bit 19) | not made: the sky drawn into `Game(0x15)`, `copy_pass` |
//! | alpha clear `0x0003c515` | `native_targets::cut_clear(0x10, 0x00ffffff, 0, 0)` on the bound target |
//! | vertex type `0x0001cbb0`, grid constants `0x000151c0`, stream `0x0001cda0` | Rust over [`Device::host`] ([`state`]) |
//! | stream commit `0x0001c710` | guest call |
//! | the 30 words at `0x004c1a90` zeroed, `0x0001bd50`, `0x0001cdd0` | Rust ([`state`]) |
//! | shader begin `0x0001bbf0` | guest call |
//! | viewport `0x00012d50`, structure constants `0x0003bf00`, the wrapper's stage commit `0x0001cf50` | Rust ([`state`]) |
//! | quads `0x0001c8a0(8, [geometry + 0x24], [geometry + 0x20])` | [`Device::draw`]: `IndexedPass::water_quads` over the scene's `WaterComposite` |
//! | stamps `0x0003bcb0` | a guest call per image ([`stamps`]) |
//!
//! What the stage cannot make is a cause, `unimplemented!("water ocean:
//! <cause>")`, or for the copies `unimplemented!("water ocean copy:
//! <cause>")` (`copy_cache`, `copy_restore`, `copy_target`, `copy_pass`):
//! `unread`, `unwritable`, `shader_group` (a shader of group
//! `MTRP` or `3trp`, whose template is a virtual call), `host`, `clear`,
//! `device` (a draw-state call refused), `vertex_kind`,
//! `position_compression`, `structure` (the upload's float model
//! refused), `quads` (no keyed view's composite), `draw` (the native draw
//! refused).

use engine::memory::{GuestMemory, MemFault};

use super::{BSP, WATER_COUNT};
use crate::native_targets::Host;

pub mod state;
#[cfg(test)]
mod fake;

/// A step the stage cannot make.
pub type Cause = &'static str;

/// The guest helpers the body calls.
pub const GRID: u32 = 0x0003_ca90;
pub const VISIBLE: u32 = 0x0003_c270;
pub const TEXTURE: u32 = 0x0012_de70;
pub const STREAM_COMMIT: u32 = 0x0001_c710;
pub const SHADER_BEGIN: u32 = 0x0001_bbf0;
pub const STAMP: u32 = 0x0003_bcb0;

/// The tag table's pointer: entries of 16 bytes, the group at `+0`, the
/// data at `+8`.
pub const TAGS: u32 = 0x004e_3b44;
/// The groups whose template is a virtual call (`0x00137bd0`, `[vtable +
/// 0x28]`).
pub const GROUP_MTRP: u32 = 0x5052_544d;
pub const GROUP_3TRP: u32 = 0x7072_7433;
/// The definition's words.
pub const SHADER: u32 = 4;
pub const GEOMETRY: u32 = 0xc;
pub const TEXTURE_BLOCK: u32 = 0x10;
/// The geometry's words: the state word, the quads' index count and list.
pub const GEOMETRY_STATE: u32 = 0x3c;
pub const QUAD_COUNT: u32 = 0x20;
pub const QUAD_INDICES: u32 = 0x24;
/// The block `0x0001bd50` reads, zeroed first (30 words).
pub const STATE_BLOCK: u32 = 0x004c_1a90;
pub const STATE_BLOCK_WORDS: u32 = 0x1e;
/// The frame's structure target copy flag (`0x0003bea0` sets it; the frame
/// step clears it) and the game target the copy is made into.
pub const TARGET_COPIED: u32 = 0x0050_9430;
pub const COPY_INTO: u32 = 0x16;
/// The implementation flags' copy bits.
pub const COPY_BIT: u32 = 20;
pub const PASS_BIT: u32 = 19;

/// The registers a helper is called with, or a call left.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Regs {
    pub eax: u32,
    pub ecx: u32,
    pub edx: u32,
    pub ebx: u32,
    pub esi: u32,
    pub edi: u32,
    pub ebp: u32,
}

/// A guest helper to call: its address, registers and stack arguments
/// (first argument first).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    pub target: u32,
    pub regs: Regs,
    pub args: Vec<u32>,
}

/// What the stage does next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Next {
    Call(Call),
    /// The body's `mov al, 1` and return, EAX and ECX as the guest leaves
    /// them; EBX, ESI, EDI and EBP are the caller's.
    Return { eax: u32, ecx: u32 },
}

/// The device side: boot's glue in a game, a recorder in the tests.
pub trait Device {
    /// The device calls of the clear and the draw state.
    fn host(&self) -> &dyn Host;
    /// 64 bytes of guest stack the draw state stages its words in.
    fn scratch(&self) -> u32;
    /// The quads: `count` indices at `indices`, a quad list.
    fn draw(&mut self, mem: &mut dyn GuestMemory, indices: u32, count: u32) -> Result<(), Cause>;
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
    /// EBX from the lookup: `flags >> 19`, BL masked to bit 0.
    fn ebx(&self) -> u32 {
        let b = self.flags >> PASS_BIT;
        (b & !0xff) | (b & 1)
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
    if let Some(open) = &open {
        if !host.restore(mem, &open.writes) {
            return Err("copy_restore");
        }
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

/// The lookup from the shader `index` to its implementation's flags
/// (`0x0003c439..0x0003c4d6`).
pub fn shader(mem: &dyn GuestMemory, index: u32) -> Result<Shader, Cause> {
    let table = word(mem, TAGS)?;
    // `movsx ecx, ax`: the group by the signed index.
    let group = word(mem, table.wrapping_add(((index as u16 as i16 as i32) << 4) as u32))?;
    if group == GROUP_MTRP || group == GROUP_3TRP {
        return Err("shader_group");
    }
    let block = word(mem, tag_data(mem, index)?.wrapping_add(0x24))?;
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

/// One stamp's call: the image, and the loop's registers at it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub image: u32,
    pub bitmap: u32,
    pub n: u32,
    pub block: u32,
}

/// The stamps of the shader `index` (`0x0003c5a9..0x0003c630`): each
/// image of each bitmap of its block, and the bitmap count the loop ends
/// on (signed, the guest's `jle`).
pub fn stamps(mem: &dyn GuestMemory, index: u32) -> Result<(Vec<Stamp>, i32), Cause> {
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
                out.push(Stamp { image, bitmap: data, n, block });
            }
        }
    }
    Ok((out, count))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Grid,
    Visible,
    Texture,
    Shader,
    Clear,
    StreamCommit,
    ShaderBegin,
    Draw,
    Stamp(usize),
}

/// One ocean's steps.
#[derive(Clone, Debug)]
pub struct Ocean {
    definition: u32,
    clear: bool,
    regs: Regs,
    step: Step,
    shader: Option<Shader>,
    stamps: Vec<Stamp>,
    count: i32,
}

impl Ocean {
    /// The ocean of `definition`; `entry` the registers the body was
    /// entered with.
    pub fn new(definition: u32, clear: bool, entry: Regs) -> Self {
        Self { definition, clear, regs: Regs { ebp: definition, ..entry }, step: Step::Grid, shader: None, stamps: Vec::new(), count: 0 }
    }

    fn call(&mut self, next: Step, target: u32, regs: Regs, args: Vec<u32>) -> Result<Next, Cause> {
        self.step = next;
        self.regs = regs;
        Ok(Next::Call(Call { target, regs, args }))
    }

    /// The next step, `returned` the registers the last helper left (only
    /// EAX, ECX and EDX are read; the callee keeps the rest).
    pub fn next(&mut self, mem: &mut dyn GuestMemory, device: &mut dyn Device, returned: Regs) -> Result<Next, Cause> {
        let d = self.definition;
        let r = Regs { eax: returned.eax, ecx: returned.ecx, edx: returned.edx, ..self.regs };
        let al = returned.eax & 0xff != 0;
        match self.step {
            Step::Grid => self.call(Step::Visible, GRID, Regs { eax: d, ..r }, Vec::new()),
            Step::Visible => self.call(Step::Texture, VISIBLE, r, vec![d]),
            Step::Texture if !al => self.to_stamps(mem, r),
            Step::Texture => self.call(Step::Shader, TEXTURE, Regs { eax: 3, ecx: d.wrapping_add(TEXTURE_BLOCK), ..r }, Vec::new()),
            Step::Shader if !al => self.to_stamps(mem, r),
            Step::Shader => {
                let index = word(mem, d.wrapping_add(SHADER))?;
                if index == u32::MAX {
                    return self.to_stamps(mem, r);
                }
                let s = shader(mem, index)?;
                self.shader = Some(s);
                let r = Regs { eax: s.flags >> COPY_BIT, ecx: s.ecx, edx: s.edx, ebx: s.ebx(), esi: s.esi, edi: s.word, ..r };
                // The pass into target 0x15 draws the sky; the copy goes first.
                if s.copies() {
                    first_copy(mem, device)?;
                }
                if s.passes() {
                    return Err("copy_pass");
                }
                self.regs = r;
                self.step = Step::Clear;
                self.next(mem, device, r)
            }
            Step::Clear => {
                let (host, scratch) = (device.host(), device.scratch());
                if self.clear {
                    crate::native_targets::cut_clear(mem, host, 0x10, 0x00ff_ffff, 0, 0, scratch)
                        .map_err(|cause| if cause == "device" { "clear" } else { cause })?;
                }
                let geometry = word(mem, d.wrapping_add(GEOMETRY))?;
                // 0x0001cbb0(eax 1, edi the implementation's vertex type, 1).
                state::vertex_type(mem, host, r.edi, 1, 1)?;
                state::grid_constants(mem, host)?;
                let stream = word(mem, geometry.wrapping_add(GEOMETRY_STATE))?;
                state::stream(mem, stream)?;
                self.call(Step::StreamCommit, STREAM_COMMIT, Regs { esi: geometry, ..r }, vec![state::STREAMS])
            }
            Step::StreamCommit => {
                let host = device.host();
                mem.write(STATE_BLOCK, &[0; 4 * STATE_BLOCK_WORDS as usize]).map_err(|_| "unwritable")?;
                state::state_block(mem, STATE_BLOCK)?;
                state::position_decompression(mem, host, device.scratch(), 0, 0)?;
                let edi = self.shader.map_or(0, |s| s.index);
                self.call(Step::ShaderBegin, SHADER_BEGIN, Regs { edi, ..r }, vec![0, 3, 0, 0, 0x47c3_5000])
            }
            Step::ShaderBegin => {
                let (host, scratch) = (device.host(), device.scratch());
                state::viewport(mem, host, scratch)?;
                state::structure(mem, host, scratch, d)?;
                state::commit_stages(mem, host)?;
                self.step = Step::Draw;
                self.next(mem, device, r)
            }
            Step::Draw => {
                let (count, indices) = (word(mem, r.esi.wrapping_add(QUAD_COUNT))?, word(mem, r.esi.wrapping_add(QUAD_INDICES))?);
                device.draw(mem, indices, count)?;
                self.to_stamps(mem, Regs { eax: 1, ..r })
            }
            Step::Stamp(i) => self.stamp(i, r),
        }
    }

    fn to_stamps(&mut self, mem: &dyn GuestMemory, r: Regs) -> Result<Next, Cause> {
        let index = word(mem, self.definition.wrapping_add(SHADER))?;
        if index == u32::MAX {
            return Ok(Next::Return { eax: (index & !0xff) | 1, ecx: r.ecx });
        }
        (self.stamps, self.count) = stamps(mem, index)?;
        self.regs = r;
        self.stamp(0, r)
    }

    fn stamp(&mut self, i: usize, r: Regs) -> Result<Next, Cause> {
        let Some(&s) = self.stamps.get(i) else {
            let ended = self.count.max(0) as u32;
            return Ok(Next::Return { eax: (ended & !0xff) | 1, ecx: self.count as u32 });
        };
        let regs = Regs { eax: s.image, esi: s.bitmap, edi: s.n, ebx: s.n * 0x74, ebp: s.block, ..r };
        self.call(Step::Stamp(i + 1), STAMP, regs, Vec::new())
    }
}

#[cfg(test)]
mod tests;
