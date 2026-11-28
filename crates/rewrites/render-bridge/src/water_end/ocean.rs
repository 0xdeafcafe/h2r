//! The BSP ocean of the water end (Code, `0x0003c3ff..0x0003c632`;
//! docs/graphics/retire-d3d-display.md, "The water end"), stepped in Rust.
//!
//! [`Ocean::next`] makes each of the body's steps in the guest's order and
//! hands back the next guest helper to call, with the registers and stack
//! arguments the body gives it, until the return:
//!
//! | Step | Made |
//! |---|---|
//! | grid `0x0003ca90` | guest call |
//! | gate `0x0003c270`, the ocean's occlusion query | [`Device::visible`]: Rust over the native flare query's count pass ([`gate`]) |
//! | gate `0x0012de70`, the texture block's residency | Rust ([`cache::resident`]); a closed gate goes to the stamps |
//! | the shader `[definition + 4]` to its implementation's flags `+0x118` | Rust ([`shader`]) |
//! | copies `0x0003bea0` (bit 20), `0x00131ed0` then `0x0003bd00` (bit 19) | guest calls; their binds land on `Game(0x16)` and `Game(0x15)` through the device's role table |
//! | alpha clear `0x0003c515` | `native_targets::cut_clear(0x10, 0x00ffffff, 0, 0)` on the bound target |
//! | vertex type `0x0001cbb0`, grid constants `0x000151c0`, stream `0x0001cda0` | Rust over [`Device::host`] ([`state`]) |
//! | stream commit `0x0001c710` | Rust over [`Device::host`] ([`state::stream_commit`]) |
//! | the 30 words at `0x004c1a90` zeroed, `0x0001bd50`, `0x0001cdd0` | Rust ([`state`]) |
//! | shader begin `0x0001bbf0` | guest call |
//! | viewport `0x00012d50`, structure constants `0x0003bf00`, the wrapper's stage commit `0x0001cf50` | Rust ([`state`]) |
//! | quads `0x0001c8a0(8, [geometry + 0x24], [geometry + 0x20])` | [`Device::draw`]: `IndexedPass::water_quads` over the scene's `WaterComposite` |
//! | stamps `0x0003bcb0` | Rust per image ([`stamps`], [`cache::stamp`]) |
//!
//! What the stage cannot make is a cause, `unimplemented!("water ocean:
//! <cause>")`: `unread`, `unwritable`, `shader_group` (a shader of group
//! `MTRP` or `3trp`, whose template is a virtual call), `host`, `clear`,
//! `device` (a draw-state call refused), `vertex_kind`,
//! `position_compression`, `structure` (the upload's float model
//! refused), `quads` (no keyed view's composite), `draw` (the native draw
//! refused), `flare_host`, `placed` (the gate's count pass not placed),
//! `vertex_declaration`, `vertex_format` (a stream format the commit
//! cannot build), `texture_cold` (a texture block to stream),
//! `texture_request` (an image the texture cache must load).

use engine::memory::{GuestMemory, MemFault};

use crate::native_targets::Host;

pub mod cache;
pub mod gate;
pub mod state;
#[cfg(test)]
mod fake;

/// A step the stage cannot make.
pub type Cause = &'static str;

/// The guest helpers the body calls.
pub const GRID: u32 = 0x0003_ca90;
pub const COPY_TARGET: u32 = 0x0003_bea0;
pub const PASS_SOURCE: u32 = 0x0013_1ed0;
pub const PASS_TARGET: u32 = 0x0003_bd00;
pub const SHADER_BEGIN: u32 = 0x0001_bbf0;

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
pub const PASS_BLOCK: u32 = 0x58;
pub const PASS_WORD: u32 = 0x84;
/// The geometry's words: the state word, the quads' index count and list.
pub const GEOMETRY_STATE: u32 = 0x3c;
pub const QUAD_COUNT: u32 = 0x20;
pub const QUAD_INDICES: u32 = 0x24;
/// The block `0x0001bd50` reads, zeroed first (30 words).
pub const STATE_BLOCK: u32 = 0x004c_1a90;
pub const STATE_BLOCK_WORDS: u32 = 0x1e;
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
    /// The gate `0x0003c270(definition)`: whether the ocean is seen
    /// ([`gate::visible`] over the native flare query).
    fn visible(&mut self, mem: &mut dyn GuestMemory, definition: u32) -> Result<bool, Cause>;
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Grid,
    Gates,
    PassSource,
    PassTarget,
    Clear,
    ShaderBegin,
    Draw,
}

/// One ocean's steps.
#[derive(Clone, Debug)]
pub struct Ocean {
    definition: u32,
    clear: bool,
    regs: Regs,
    step: Step,
    shader: Option<Shader>,
}

impl Ocean {
    /// The ocean of `definition`; `entry` the registers the body was
    /// entered with.
    pub fn new(definition: u32, clear: bool, entry: Regs) -> Self {
        Self { definition, clear, regs: Regs { ebp: definition, ..entry }, step: Step::Grid, shader: None }
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
        match self.step {
            Step::Grid => self.call(Step::Gates, GRID, Regs { eax: d, ..r }, Vec::new()),
            Step::Gates => {
                if !device.visible(mem, d)? || !cache::resident(mem, d.wrapping_add(TEXTURE_BLOCK))? {
                    return self.to_stamps(mem, r);
                }
                let index = word(mem, d.wrapping_add(SHADER))?;
                if index == u32::MAX {
                    return self.to_stamps(mem, r);
                }
                let s = shader(mem, index)?;
                self.shader = Some(s);
                let r = Regs { eax: s.flags >> COPY_BIT, ecx: s.ecx, edx: s.edx, ebx: s.ebx(), esi: s.esi, edi: s.word, ..r };
                if s.copies() {
                    return self.call(Step::PassSource, COPY_TARGET, r, Vec::new());
                }
                self.regs = r;
                self.step = Step::PassSource;
                self.next(mem, device, r)
            }
            Step::PassSource if self.shader.is_some_and(|s| s.passes()) => {
                self.call(Step::PassTarget, PASS_SOURCE, Regs { eax: d.wrapping_add(PASS_BLOCK), ..r }, vec![d.wrapping_add(PASS_BLOCK)])
            }
            Step::PassSource => {
                self.step = Step::Clear;
                self.next(mem, device, r)
            }
            Step::PassTarget => {
                let pass = word(mem, d.wrapping_add(PASS_WORD))?;
                self.call(Step::Clear, PASS_TARGET, Regs { ecx: pass, ..r }, vec![pass, returned.eax])
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
                state::stream_commit(mem, host, state::STREAMS)?;
                mem.write(STATE_BLOCK, &[0; 4 * STATE_BLOCK_WORDS as usize]).map_err(|_| "unwritable")?;
                state::state_block(mem, STATE_BLOCK)?;
                state::position_decompression(mem, host, device.scratch(), 0, 0)?;
                let edi = self.shader.map_or(0, |s| s.index);
                self.call(Step::ShaderBegin, SHADER_BEGIN, Regs { esi: geometry, edi, ..r }, vec![0, 3, 0, 0, 0x47c3_5000])
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
        }
    }

    /// The stamps, then the return.
    fn to_stamps(&mut self, mem: &mut dyn GuestMemory, r: Regs) -> Result<Next, Cause> {
        let index = word(mem, self.definition.wrapping_add(SHADER))?;
        if index == u32::MAX {
            return Ok(Next::Return { eax: (index & !0xff) | 1, ecx: r.ecx });
        }
        let (images, count) = stamps(mem, index)?;
        for image in images {
            cache::stamp(mem, image)?;
        }
        let ended = count.max(0) as u32;
        Ok(Next::Return { eax: (ended & !0xff) | 1, ecx: count as u32 })
    }
}

#[cfg(test)]
mod tests;
