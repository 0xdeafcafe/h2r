//! `native-targets`: the scene's target select `0x0001cd30` and clear
//! `0x00014b60` answered without their guest bodies (native-only plan,
//! `nor-8b`; docs/graphics/framescene.md, "Slice 6"). On with the native
//! renderer.
//!
//! Both are thin wrappers of a D3D entry. The cut makes the wrapper's
//! decision in Rust, the clear's prelude `0x00014ac0` too, and hands the
//! device the bind or the clear through the [`Host`] (boot's glue over the
//! shared device), so a game view's binds and clears make no guest D3D
//! entry call:
//!
//! | Guest | Decision | Device |
//! |---|---|---|
//! | `0x0001cd30(esi colour, eax depth, [esp+4] flag)`, `ret 4`, `al = 1` | the cached pair `0x0051f3f4`/`0x0051f3f8` unchanged: nothing; a colour of the cached one's format and size (`+0`, `+0xc`, `+0x10`): the same-size bind `0x003f7b30`; else `0x003f7870` | [`Host::bind`] |
//! | `0x00014b60(eax flags, colour, z, stencil byte)`, `ret 0xc`, `eax = 0` | the game's flags as D3D clear flags ([`clear_flags`]); none: nothing | [`Host::clear`] |
//! | `0x00013b90()`, `ret`: the frame tail | [`crate::frame_tail`]: the freeze frame's copy and composite as overlay items | `frame_tail::Host::place` |
//! | `0x000132d0(view)`, `ret 4`, and `0x00013c20()`, `ret`: the view setup and view end | [`crate::view_setup`]: the context, the transform, the target, the clear, the fog | this host's other calls |
//! | `0x00013cd0()`, `ret`, `eax = 0`: the frame's finish | the four stage slots cleared and committed against their cache, the index buffer's device reference dropped, the pixel shader none ([`cut_finish`]) | [`Host::texture`], [`Host::pixel_shader`] |
//!
//! No guest body runs: a call the cut cannot make (no host, a refused
//! device call, a word that does not read or write, the finish's last
//! index-buffer reference) panics, named `<call>: <cause>`
//! ([`unhandled`]). Each call is counted `<call>_calls` in `render.scene`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use engine::memory::{GuestMemory, MemFault};

use hooks::{BuildError, Builder, Feature, Manifest};

pub const ID: &str = "native-targets";

/// The target select's cache of the bound pair and the call's flag byte.
pub const CACHE_COLOUR: u32 = 0x0051_f3f4;
pub const CACHE_DEPTH: u32 = 0x0051_f3f8;
pub const CACHE_FLAG: u32 = 0x0051_f3fc;

/// The clear's prelude gate: `0x00014ac0` runs when `[0x00485607]` is set
/// and `[0x004858b8]` is zero.
pub const CLEAR_PRELUDE_ON: u32 = 0x0048_5607;
pub const CLEAR_PRELUDE_TARGET: u32 = 0x0048_58b8;

/// The finish's globals (Code, `0x00013cd0..0x00013d43`): the engine's
/// four requested textures, the four it last committed (`0x0001cf50`
/// sets a stage whose two differ), the word it sets to -1, the word it
/// clears, and the device pointer.
pub const STAGE_REQUESTED: u32 = 0x0051_f3d8;
pub const STAGE_COMMITTED: u32 = 0x0051_f3c8;
pub const FINISH_UNSET: u32 = 0x0046_7004;
pub const FINISH_CLEARED: u32 = 0x0040_8338;
pub const DEVICE: u32 = 0x0040_7488;
/// The guest device's index buffer and base vertex (`BASE_VERTEX`).
pub const DEVICE_INDICES: u32 = 0x7a0;
pub const DEVICE_BASE_VERTEX: u32 = 0x1c;
/// A resource's device references: `+0x80000` each, live while any bit
/// of [`LIVE_REFS`] is set.
pub const DEVICE_REF: u32 = 0x0008_0000;
pub const LIVE_REFS: u32 = 0x0078_ffff;

/// Which bind the target select makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bind {
    /// The pair is the cached one: no bind.
    Kept,
    /// `0x003f7b30`: same size and format, the viewport left.
    SameSize,
    /// `0x003f7870`: the full bind, the viewport reset.
    Full,
}

/// The bind `0x0001cd30` makes for `colour` and `depth`.
pub fn select(mem: &dyn GuestMemory, colour: u32, depth: u32) -> Result<Bind, MemFault> {
    let cached = mem.u32(CACHE_COLOUR)?;
    if cached == colour && mem.u32(CACHE_DEPTH)? == depth {
        return Ok(Bind::Kept);
    }
    if cached != 0 && colour != 0 {
        let same = |at: u32| -> Result<bool, MemFault> { Ok(mem.u32(colour + at)? == mem.u32(cached + at)?) };
        if same(0)? && same(0xc)? && same(0x10)? {
            return Ok(Bind::SameSize);
        }
    }
    Ok(Bind::Full)
}

/// The D3D clear flags `0x00014b60` passes for the game's `flags`: bit 0
/// all four colour channels, bits 1..4 one channel each (`<< 3`), bits 5
/// and 6 depth and stencil.
pub fn clear_flags(flags: u32) -> u32 {
    let all = if flags & 1 != 0 { 0xf0 } else { 0 };
    all | (flags & 0x1e) << 3 | (flags >> 5) & 3
}

/// What answers the device's side: boot's glue over the shared device.
pub trait Host: Send + Sync {
    /// Binds `colour` and `depth` as `bind` says (never [`Bind::Kept`]).
    /// `false`: the device refused, nothing changed.
    fn bind(&self, mem: &mut dyn GuestMemory, bind: Bind, colour: u32, depth: u32) -> bool;
    /// `D3DDevice_Clear(0, null, flags, colour, z, stencil)`. `false`: refused.
    fn clear(&self, mem: &mut dyn GuestMemory, flags: u32, colour: u32, z: u32, stencil: u32) -> bool;
    /// The bound target.
    fn bound(&self) -> Option<[u32; 8]>;
    /// `D3DDevice_SetTexture(stage, texture)`. `false`: refused.
    fn texture(&self, _mem: &mut dyn GuestMemory, _stage: u32, _texture: u32) -> bool {
        false
    }
    /// `D3DDevice_SetPixelShader(handle)`. `false`: refused.
    fn pixel_shader(&self, _mem: &mut dyn GuestMemory, _handle: u32) -> bool {
        false
    }
    /// A `SetRenderState_*` setter for guest slot `slot`. `false`: refused.
    fn render_state(&self, _mem: &mut dyn GuestMemory, _slot: u32, _value: u32) -> bool {
        false
    }
    /// `SetRenderState_Simple` (`0x003f5cf0`): the push header and the
    /// value. `false`: refused.
    fn simple_state(&self, _mem: &mut dyn GuestMemory, _header: u32, _value: u32) -> bool {
        false
    }
    /// What guest render-state slot `slot` holds.
    fn stored_state(&self, _mem: &dyn GuestMemory, _slot: u32) -> Option<u32> {
        None
    }
    /// `D3DDevice_SetVertexShaderConstantNotInlineFast(register + 96, data,
    /// dwords)`. `false`: refused.
    fn constants(&self, _mem: &mut dyn GuestMemory, _register: u32, _data: u32, _dwords: u32) -> bool {
        false
    }
    /// `D3DDevice_SetVertexShaderConstant1` / `4` (`0x003f71d0`,
    /// `0x003f7280`): `count` registers from `register` (ECX) at `data`,
    /// the guest shadow updated. `false`: refused.
    fn registers(&self, _mem: &mut dyn GuestMemory, _register: u32, _data: u32, _count: u32) -> bool {
        false
    }
    /// `D3DDevice_SetTextureState_BumpEnv` (`0x003f5e30`): stage `stage`'s
    /// state `state` to `value`. `false`: refused.
    fn texture_stage_state(&self, _mem: &mut dyn GuestMemory, _stage: u32, _state: u32, _value: u32) -> bool {
        false
    }
    /// `D3DDevice_LoadVertexShaderProgram(function, address)`
    /// (`0x003f6c60`). `false`: refused.
    fn vertex_program(&self, _mem: &mut dyn GuestMemory, _function: u32, _address: u32) -> bool {
        false
    }
    /// `D3DDevice_SetVertexShaderInput(handle, count, inputs)`
    /// (`0x003f7440`). `false`: refused.
    fn vertex_input(&self, _mem: &mut dyn GuestMemory, _handle: u32, _count: u32, _inputs: u32) -> bool {
        false
    }
    /// `D3DDevice_SelectVertexShaderDirect(vaf, address)` (`0x003f6cf0`,
    /// ECX the format, EAX the program's address). `false`: refused.
    fn select_vertex_program(&self, _mem: &mut dyn GuestMemory, _vaf: u32, _address: u32) -> bool {
        false
    }
    /// Constant register `r` (hardware order, register + 96) as bits.
    fn constant(&self, _r: usize) -> Option<[u32; 4]> {
        None
    }
    /// `D3D_MakeRequestedSpace(minimum, requested)`: the new put.
    fn make_space(&self, _mem: &mut dyn GuestMemory, _minimum: u32, _requested: u32) -> Option<u32> {
        None
    }
    /// The viewport push fragment `0x003f6190` at `put`: the new put.
    fn push_viewport(&self, _mem: &mut dyn GuestMemory, _put: u32, _device: u32) -> Option<u32> {
        None
    }
    /// `D3DDevice_SetViewport(viewport)`. `false`: refused.
    /// The width and height of the surface at `header`, as
    /// `D3DSurface_GetDesc` gives them; `None` when it does not read.
    fn surface_size(&self, _mem: &dyn GuestMemory, _header: u32) -> Option<(u32, u32)> {
        None
    }
    fn viewport(&self, _mem: &mut dyn GuestMemory, _viewport: u32) -> bool {
        false
    }
    /// The bound colour copied into the colour of game target `target`
    /// (`Game(target)`, the guest's `0x000352e0(target, 1)`), ordered with
    /// the draws that follow it. `false`: refused.
    fn copy_view(&self, _mem: &mut dyn GuestMemory, _target: u32) -> bool {
        false
    }
    /// The open shader's restore list as `0x00018900` writes it
    /// ([`crate::open_shader`]). `false`: a state the device does not take.
    fn restore(&self, _mem: &mut dyn GuestMemory, _writes: &[crate::open_shader::Write]) -> bool {
        false
    }
}

static HOST: Mutex<Option<Arc<dyn Host>>> = Mutex::new(None);

/// Installs the host (boot). Without one every call panics.
pub fn set_host(host: Arc<dyn Host>) {
    *HOST.lock().unwrap_or_else(|e| e.into_inner()) = Some(host);
}

pub(crate) fn host() -> Option<Arc<dyn Host>> {
    HOST.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// A call's count.
pub struct Arm {
    name: &'static str,
    calls: AtomicU64,
}

impl Arm {
    pub const fn new(name: &'static str) -> Self {
        Self { name, calls: AtomicU64::new(0) }
    }

    pub(crate) fn call(&self) {
        self.calls.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn counters(&self) -> Vec<(String, u64)> {
        vec![(format!("{}_calls", self.name), self.calls.load(Ordering::Relaxed))]
    }
}

/// A call the cut cannot make: there is no guest body to run instead.
pub(crate) fn unhandled(call: &str, cause: &str) -> ! {
    unimplemented!("{call}: {cause}")
}

/// The installed host; none is [`unhandled`].
pub(crate) fn installed(call: &str) -> Arc<dyn Host> {
    host().unwrap_or_else(|| unhandled(call, "host"))
}

pub static TARGET_SELECT: Arm = Arm::new("target_select");
pub static SCENE_CLEAR: Arm = Arm::new("scene_clear");
pub static FRAME_FINISH: Arm = Arm::new("frame_finish");

/// The counters `render.scene` reports.
pub fn counters() -> Vec<(String, u64)> {
    TARGET_SELECT.counters().into_iter().chain(SCENE_CLEAR.counters()).chain(FRAME_FINISH.counters())
        .chain(crate::frame_tail::counters()).chain(crate::view_setup::counters()).chain(crate::water_end::counters()).collect()
}

/// Halo's render-target table (`0x004b4b58`, stride `0x98`): entry `n`'s
/// texture header at `+4`, its surfaces at `+0x18 + 0x18 * level` for
/// each level below its count `+0x78` (`0x0001dcf0` the first,
/// `0x0001dd20` any).
pub const TARGETS: u32 = 0x004b_4b58;
pub const TARGET_STRIDE: u32 = 0x98;
pub const TARGET_TEXTURE: u32 = 4;
pub const TARGET_SURFACE: u32 = 0x18;
pub const TARGET_LEVEL_STRIDE: u32 = 0x18;
/// The surfaces an entry has room for before its count at `+0x78`.
pub const TARGET_LEVELS: u32 = 4;
/// The entries [`role`] names. The target select `0x00014bc0` names
/// entries up to `0x1b` (Code, its tables `0x00014f30` and `0x00014ed0`).
pub const TARGET_COUNT: u32 = 0x20;

/// The globals the target select `0x00014bc0` reads a target from besides
/// the table and the device's own buffers (`0x0050935c`, `0x00509360`,
/// `0x00509364`), with the select's indices (Code `0x00014be5..0x00014d1c`):
///
/// | Global | Index | Made by |
/// |---|---|---|
/// | `0x00509370` | 3 | `0x00014600`: a colour surface over the depth buffer's memory |
/// | `0x00509378`, `0x00509380` | `0x10`/`0x1d`, `0x11` | `0x00014850`: level 0 of the 64 x 64 textures `0x00509374`, `0x0050937c` (`0x000158f0`, `0x003fb7a0`) |
/// | `0x00509384` | 9 | `0x00014850`: the static surface `0x00484f50`, over entry 9's memory |
/// | `0x00509390` to `0x005093a4`, depth `0x005093a8` | `0x21` to `0x26` | no code names them as written |
pub const TARGET_SLOTS: [u32; 10] =
    [0x0050_9370, 0x0050_9378, 0x0050_9380, 0x0050_9384, 0x0050_9390, 0x0050_9394, 0x0050_9398, 0x0050_939c, 0x0050_93a0, 0x0050_93a4];
/// The depth the select binds with `0x00509390` to `0x005093a4`.
pub const TARGET_SLOT_DEPTH: u32 = 0x0050_93a8;

/// The role of the surface at `header`, the device's role table
/// (docs/graphics/retire-d3d-display.md, "The
/// graph's targets"), `word` a reader of guest words: a table entry's
/// first surface is `Game(n)`, its others `GameLevel`; a header one of
/// [`TARGET_SLOTS`] (or [`TARGET_SLOT_DEPTH`]) holds is `Slot` of that
/// global. The swap chain's buffers and depth are the device's own
/// (`Display`, `DisplayDepth`). Anything else has none.
pub fn role(header: u32, word: &dyn Fn(u32) -> Option<u32>) -> Option<frame_types::TargetRole> {
    use frame_types::TargetRole;
    if let Some(off) = header.checked_sub(TARGETS + TARGET_SURFACE) {
        let (n, within) = (off / TARGET_STRIDE, off % TARGET_STRIDE);
        let level = within / TARGET_LEVEL_STRIDE;
        if n < TARGET_COUNT && within % TARGET_LEVEL_STRIDE == 0 && level < TARGET_LEVELS {
            return Some(match level {
                0 => TargetRole::Game(n as u8),
                l => TargetRole::GameLevel { target: n as u8, level: l as u8 },
            });
        }
    }
    if header == 0 {
        return None;
    }
    TARGET_SLOTS.into_iter().chain([TARGET_SLOT_DEPTH]).find(|g| word(*g) == Some(header)).map(TargetRole::Slot)
}

/// The surface header of `role`, [`role`]'s inverse for a table entry's
/// surfaces: the frame graph's surface for that role is the one the
/// device holds for the header. `None` for any other role.
pub fn surface(role: frame_types::TargetRole) -> Option<u32> {
    use frame_types::TargetRole;
    let at = |n: u8, level: u8| {
        (u32::from(n) < TARGET_COUNT && u32::from(level) < TARGET_LEVELS)
            .then(|| TARGETS + TARGET_STRIDE * u32::from(n) + TARGET_SURFACE + TARGET_LEVEL_STRIDE * u32::from(level))
    };
    match role {
        TargetRole::Game(n) => at(n, 0),
        TargetRole::GameLevel { target, level } if level > 0 => at(target, level),
        _ => None,
    }
}

/// The target select's cut: the bind through `host` and the cache words,
/// `eax` 1. `Err` names the call it could not make (nothing written).
pub fn cut_select(mem: &mut dyn GuestMemory, host: &dyn Host, colour: u32, depth: u32, flag: u8) -> Result<Bind, &'static str> {
    let bind = select(&*mem, colour, depth).map_err(|_| "unread")?;
    if bind == Bind::Kept {
        return Ok(bind);
    }
    if !host.bind(mem, bind, colour, depth) {
        return Err("device");
    }
    let flag_word = mem.u32(CACHE_FLAG).map_err(|_| "unread")? & !0xff | u32::from(flag);
    for (at, v) in [(CACHE_COLOUR, colour), (CACHE_DEPTH, depth), (CACHE_FLAG, flag_word)] {
        mem.set_u32(at, v).map_err(|_| "unwritable")?;
    }
    Ok(bind)
}

/// The clear's cut (`0x00014b60`, Code), staging the prelude's viewports
/// in the guest stack at `scratch`. `Err` names the call it could not make.
pub fn cut_clear(mem: &mut dyn GuestMemory, host: &dyn Host, flags: u32, colour: u32, z: u32, stencil: u8, scratch: u32) -> Result<(), &'static str> {
    let d3d = clear_flags(flags);
    if d3d == 0 {
        return Ok(());
    }
    if mem.u8(CLEAR_PRELUDE_ON).map_err(|_| "unread")? != 0 && mem.u32(CLEAR_PRELUDE_TARGET).map_err(|_| "unread")? == 0 {
        prelude(mem, host, scratch)?;
    }
    if !host.clear(mem, d3d, colour, z, u32::from(stencil)) {
        return Err("device");
    }
    Ok(())
}

/// The device's viewport (`x, y, width, height, min_z, max_z`).
pub const DEVICE_VIEWPORT: u32 = 0xee0;
/// `1.0f`, the guest's constant.
const ONE: u32 = 0x0045_dbc0;

/// The clear's prelude `0x00014ac0` (Code `0x00014ac0..0x00014b57`): the
/// whole 640 x 480 screen's colour cleared to black, the device's viewport
/// set to it and back.
pub fn prelude(mem: &mut dyn GuestMemory, host: &dyn Host, scratch: u32) -> Result<(), &'static str> {
    let device = mem.u32(DEVICE).map_err(|_| "unread")?;
    let saved = mem.bytes(device.wrapping_add(DEVICE_VIEWPORT), 24).map_err(|_| "unread")?;
    let screen = [0, 0, 0x280, 0x1e0, 0, mem.u32(ONE).map_err(|_| "unread")?];
    let staged: Vec<u8> = screen.iter().flat_map(|w: &u32| w.to_le_bytes()).collect();
    mem.write(scratch, &staged).map_err(|_| "unwritable")?;
    if !host.viewport(mem, scratch) || !host.clear(mem, 0xf0, 0, 0, 0) {
        return Err("device");
    }
    mem.write(scratch, &saved).map_err(|_| "unwritable")?;
    if !host.viewport(mem, scratch) {
        return Err("device");
    }
    Ok(())
}

/// The finish's cut (`0x00013cd0`, Code): the stage requests cleared and
/// `[0x00467004]` set to -1; each stage whose committed texture is not
/// null set to null through `host` and its cache cleared (`0x0001cf50`);
/// `[0x00408338]` cleared; the device's index buffer, when it has one,
/// loses its device reference and the device's index buffer and base
/// vertex are cleared; the pixel shader none through `host`. `Err` names
/// what the cut could not make: `release` when the reference is the index
/// buffer's last, whose destroy (`D3D_DestroyResource` `0x003fb520`) is
/// not made here.
pub fn cut_finish(mem: &mut dyn GuestMemory, host: &dyn Host) -> Result<(), &'static str> {
    let unwritable = |_| "unwritable";
    for stage in 0..4 {
        mem.set_u32(STAGE_REQUESTED + 4 * stage, 0).map_err(unwritable)?;
    }
    mem.set_u32(FINISH_UNSET, u32::MAX).map_err(unwritable)?;
    for stage in 0..4 {
        let at = STAGE_COMMITTED + 4 * stage;
        if mem.u32(at).map_err(|_| "unread")? == 0 {
            continue;
        }
        if !host.texture(mem, stage, 0) {
            return Err("device");
        }
        mem.set_u32(at, 0).map_err(unwritable)?;
    }
    let device = mem.u32(DEVICE).map_err(|_| "unread")?;
    let indices = mem.u32(device.wrapping_add(DEVICE_INDICES)).map_err(|_| "unread")?;
    mem.set_u32(FINISH_CLEARED, 0).map_err(unwritable)?;
    if indices != 0 {
        let refs = mem.u32(indices).map_err(|_| "unread")?.wrapping_sub(DEVICE_REF);
        if refs & LIVE_REFS == 0 {
            return Err("release");
        }
        mem.set_u32(indices, refs).map_err(unwritable)?;
    }
    mem.set_u32(device.wrapping_add(DEVICE_INDICES), 0).map_err(unwritable)?;
    mem.set_u32(device.wrapping_add(DEVICE_BASE_VERTEX), 0).map_err(unwritable)?;
    if !host.pixel_shader(mem, 0) {
        return Err("device");
    }
    Ok(())
}

fn finish_call(cpu: &mut runtime::CpuAbi, ctx: &mut runtime::ExecCtx, _addr: u32) -> Result<runtime::TransferAbi, runtime::GuestFault> {
    FRAME_FINISH.call();
    let host = installed("frame_finish");
    let rt = Arc::clone(ctx.runtime());
    let made = cut_finish(&mut *rt.memory(), &*host);
    if let Err(cause) = made {
        unhandled("frame_finish", cause);
    }
    cpu.eax = 0;
    ctx.memory().ret(cpu, 0)
}

/// Whether the native targets are placed: with the native renderer on,
/// unless `native-targets=off`.
pub fn on(switches: &hooks::Switches) -> bool {
    switches.feature(ID, true)
}

fn select_call(cpu: &mut runtime::CpuAbi, ctx: &mut runtime::ExecCtx, _addr: u32) -> Result<runtime::TransferAbi, runtime::GuestFault> {
    TARGET_SELECT.call();
    let host = installed("target_select");
    let (colour, depth) = (cpu.esi, cpu.eax);
    let rt = Arc::clone(ctx.runtime());
    let made = {
        let mut mem = rt.memory();
        let flag = mem.u8(cpu.esp.wrapping_add(4)).unwrap_or_else(|_| unhandled("target_select", "unread"));
        cut_select(&mut *mem, &*host, colour, depth, flag)
    };
    if let Err(cause) = made {
        unhandled("target_select", cause);
    }
    // `mov al, 1` after the bind's `eax = 0`, or over the cached colour.
    cpu.eax = 1;
    ctx.memory().ret(cpu, 4)
}

/// The words of guest stack below the call the clear stages the
/// prelude's viewports in.
const SCRATCH: u32 = 0x100;

fn clear_call(cpu: &mut runtime::CpuAbi, ctx: &mut runtime::ExecCtx, _addr: u32) -> Result<runtime::TransferAbi, runtime::GuestFault> {
    SCENE_CLEAR.call();
    let host = installed("scene_clear");
    let rt = Arc::clone(ctx.runtime());
    let made = {
        let mut mem = rt.memory();
        let word = |at: u32| mem.u32(cpu.esp.wrapping_add(at)).unwrap_or_else(|_| unhandled("scene_clear", "unread"));
        let (colour, z, stencil) = (word(4), word(8), word(0xc) as u8);
        cut_clear(&mut *mem, &*host, cpu.eax, colour, z, stencil, cpu.esp.wrapping_sub(SCRATCH))
    };
    if let Err(cause) = made {
        unhandled("scene_clear", cause);
    }
    cpu.eax = 0;
    ctx.memory().ret(cpu, 0xc)
}

pub struct NativeTargets;

impl Feature for NativeTargets {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "The scene's target select and clear and the frame's finish answered without their guest bodies").doc("docs/graphics/framescene.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        // No session state: the calls are counted only.
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        if !on(b.switches()) {
            return Ok(());
        }
        b.raw().implement(&engine::points::SCENE_TARGET_SELECT, "native_targets_select", select_call).context_only();
        b.raw().implement(&engine::points::SCENE_CLEAR, "native_targets_clear", clear_call).context_only();
        b.raw().implement(&engine::points::RENDER_FRAME_FINISH, "native_targets_finish", finish_call).context_only();
        // The frame tail before it (docs/graphics/retire-d3d-display.md,
        // "The frame tail").
        crate::frame_tail::place(b);
        // The view setup and view end every frame producer reaches D3D
        // through ("Frames outside a game view").
        crate::view_setup::place(b);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::memory::FlatMemory;

    /// The table's entries and the select's globals have the same role
    /// in every frame; nothing else has one.
    #[test]
    fn a_game_target_is_named_by_its_table_entry_or_its_global() {
        use frame_types::TargetRole;
        let none = |_: u32| -> Option<u32> { None };
        let surface = |n: u32, level: u32| TARGETS + TARGET_STRIDE * n + TARGET_SURFACE + TARGET_LEVEL_STRIDE * level;
        assert_eq!(role(surface(0, 0), &none), Some(TargetRole::Game(0)));
        assert_eq!(role(surface(0x12, 0), &none), Some(TargetRole::Game(0x12)), "the freeze frame's target");
        assert_eq!(role(surface(0x1b, 0), &none), Some(TargetRole::Game(0x1b)));
        assert_eq!(role(surface(0xd, 3), &none), Some(TargetRole::GameLevel { target: 0xd, level: 3 }), "a cube map's face");
        assert_eq!(role(surface(1, 0) + 4, &none), None, "inside a header");
        assert_eq!(role(surface(1, TARGET_LEVELS), &none), None, "the count, not a surface");
        assert_eq!(role(surface(TARGET_COUNT, 0), &none), None, "past the table");
        assert_eq!(role(TARGETS, &none), None);
        // Ascension's boot: level 0 of the 64 x 64 texture at 0x00509374.
        let (heap, depth_alias) = (0x1000_0c90, 0x1000_1000);
        let globals = |at: u32| match at {
            0x0050_9378 => Some(heap),
            0x0050_9370 => Some(depth_alias),
            0x0050_9384 => Some(0x0048_4f50),
            _ => Some(0),
        };
        assert_eq!(role(heap, &globals), Some(TargetRole::Slot(0x0050_9378)));
        assert_eq!(role(depth_alias, &globals), Some(TargetRole::Slot(0x0050_9370)));
        assert_eq!(role(0x0048_4f50, &globals), Some(TargetRole::Slot(0x0050_9384)));
        assert_eq!(role(0x1000_2000, &globals), None, "a header no global holds");
        assert_eq!(role(0, &globals), None, "an unset global names nothing");
        for r in [TargetRole::Game(0), TargetRole::Game(0x13), TargetRole::GameLevel { target: 0xd, level: 2 }] {
            assert_eq!(super::surface(r).map(|h| role(h, &none)), Some(Some(r)));
        }
        assert_eq!(super::surface(TargetRole::Game(TARGET_COUNT as u8)), None);
        assert_eq!(super::surface(TargetRole::Slot(0x0050_9378)), None);
    }

    #[derive(Default)]
    struct Device {
        binds: Mutex<Vec<(Bind, u32, u32)>>,
        clears: Mutex<Vec<[u32; 5]>>,
        textures: Mutex<Vec<(u32, u32)>>,
        shaders: Mutex<Vec<u32>>,
        viewports: Mutex<Vec<[u32; 6]>>,
        refuse: std::sync::atomic::AtomicBool,
    }

    impl Host for Device {
        fn bind(&self, _: &mut dyn GuestMemory, bind: Bind, colour: u32, depth: u32) -> bool {
            if self.refuse.load(Ordering::Relaxed) {
                return false;
            }
            self.binds.lock().unwrap().push((bind, colour, depth));
            true
        }
        fn clear(&self, _: &mut dyn GuestMemory, flags: u32, colour: u32, z: u32, stencil: u32) -> bool {
            self.clears.lock().unwrap().push([0, flags, colour, z, stencil]);
            true
        }
        fn bound(&self) -> Option<[u32; 8]> {
            None
        }
        fn texture(&self, _: &mut dyn GuestMemory, stage: u32, texture: u32) -> bool {
            if self.refuse.load(Ordering::Relaxed) {
                return false;
            }
            self.textures.lock().unwrap().push((stage, texture));
            true
        }
        fn pixel_shader(&self, _: &mut dyn GuestMemory, handle: u32) -> bool {
            self.shaders.lock().unwrap().push(handle);
            true
        }
        fn viewport(&self, mem: &mut dyn GuestMemory, at: u32) -> bool {
            self.viewports.lock().unwrap().push(std::array::from_fn(|i| mem.u32(at + 4 * i as u32).unwrap()));
            true
        }
    }

    const A: u32 = 0x0060_0000;
    const B: u32 = 0x0060_0040;
    const C: u32 = 0x0060_0080;
    const Z: u32 = 0x0060_00c0;

    fn mem() -> FlatMemory {
        let mut m = FlatMemory::new(0x0048_0000, 0x0020_0000);
        // A and B share format and size; C is larger.
        for (s, size) in [(A, 0x0011_f27f), (B, 0x0011_f27f), (C, 0x002c_f4ff)] {
            m.set_u32(s, 0x0004_0001).unwrap();
            m.set_u32(s + 0xc, 0x0000_0006).unwrap();
            m.set_u32(s + 0x10, size).unwrap();
        }
        m
    }

    /// A game view's binds and clears go to the device through the host,
    /// never through the guest's D3D entries.
    #[test]
    fn a_game_view_makes_no_target_bind() {
        let dev = Device::default();
        let mut m = mem();
        assert_eq!(cut_select(&mut m, &dev, A, Z, 1), Ok(Bind::Full), "nothing cached yet");
        assert_eq!((m.u32(CACHE_COLOUR).unwrap(), m.u32(CACHE_DEPTH).unwrap(), m.u8(CACHE_FLAG).unwrap()), (A, Z, 1));
        assert_eq!(cut_select(&mut m, &dev, A, Z, 0), Ok(Bind::Kept));
        assert_eq!(cut_select(&mut m, &dev, B, Z, 0), Ok(Bind::SameSize));
        assert_eq!(cut_select(&mut m, &dev, C, 0, 0), Ok(Bind::Full), "another size");
        assert_eq!(*dev.binds.lock().unwrap(), [(Bind::Full, A, Z), (Bind::SameSize, B, Z), (Bind::Full, C, 0)]);
        assert_eq!(cut_clear(&mut m, &dev, 1 | 1 << 5 | 1 << 6, 0xff00_0000, 0x3f80_0000, 0, SCRATCH_AT), Ok(()));
        assert_eq!(cut_clear(&mut m, &dev, 0, 0, 0, 0, SCRATCH_AT), Ok(()), "no flags: no clear");
        assert_eq!(dev.clears.lock().unwrap().len(), 1);
    }

    #[test]
    fn the_clear_flags_are_the_games_as_d3ds() {
        assert_eq!(clear_flags(1), 0xf0, "all colour channels");
        assert_eq!(clear_flags(1 << 1), 0x10);
        assert_eq!(clear_flags(1 << 4), 0x80);
        assert_eq!(clear_flags(1 << 5), 1, "depth");
        assert_eq!(clear_flags(1 << 6), 2, "stencil");
        assert_eq!(clear_flags(0), 0);
    }

    /// A refused device call is named, with nothing written.
    #[test]
    fn a_refused_call_is_named() {
        let dev = Device::default();
        let mut m = mem();
        dev.refuse.store(true, Ordering::Relaxed);
        assert_eq!(cut_select(&mut m, &dev, A, Z, 1), Err("device"));
        assert_eq!(m.u32(CACHE_COLOUR).unwrap(), 0, "the cache unchanged");
    }

    /// The prelude `0x00014ac0` at target 0 with the context's `+7` set:
    /// the 640 x 480 screen cleared black, the device's viewport put back,
    /// then the clear.
    #[test]
    fn the_prelude_clears_the_screen_and_puts_the_viewport_back() {
        let dev = Device::default();
        let mut m = FlatMemory::new(0x0040_0000, 0x0030_0000);
        m.set_u32(DEVICE, PRELUDE_DEVICE).unwrap();
        m.set_u32(ONE, 1f32.to_bits()).unwrap();
        let saved = [8, 16, 320, 240, 0, 1f32.to_bits()];
        for (i, w) in saved.iter().enumerate() {
            m.set_u32(PRELUDE_DEVICE + DEVICE_VIEWPORT + 4 * i as u32, *w).unwrap();
        }
        m.write(CLEAR_PRELUDE_ON, &[1]).unwrap();
        assert_eq!(cut_clear(&mut m, &dev, 1 | 1 << 5, 0x0011_2233, 0x3f80_0000, 0, SCRATCH_AT), Ok(()));
        assert_eq!(*dev.viewports.lock().unwrap(), [[0, 0, 640, 480, 0, 1f32.to_bits()], saved]);
        assert_eq!(*dev.clears.lock().unwrap(), [[0, 0xf0, 0, 0, 0], [0, 0xf1, 0x0011_2233, 0x3f80_0000, 0]]);
        m.set_u32(CLEAR_PRELUDE_TARGET, 0x12).unwrap();
        assert_eq!(cut_clear(&mut m, &dev, 1, 0, 0, 0, SCRATCH_AT), Ok(()));
        assert_eq!(dev.viewports.lock().unwrap().len(), 2, "no prelude off target 0");
    }

    const SCRATCH_AT: u32 = 0x0061_0000;
    const PRELUDE_DEVICE: u32 = 0x0062_0000;

    const DEVICE_AT: u32 = 0x0050_0000;
    const INDICES: u32 = 0x0060_0100;

    /// The finish's globals with stages 0 and 2 committed and an index
    /// buffer holding `refs`.
    fn finish_mem(refs: u32) -> FlatMemory {
        let mut m = FlatMemory::new(0x0040_0000, 0x0030_0000);
        m.set_u32(DEVICE, DEVICE_AT).unwrap();
        for (stage, t) in [(0, A), (1, 0), (2, B), (3, 0)] {
            m.set_u32(STAGE_REQUESTED + 4 * stage, t).unwrap();
            m.set_u32(STAGE_COMMITTED + 4 * stage, t).unwrap();
        }
        m.set_u32(FINISH_CLEARED, 7).unwrap();
        m.set_u32(DEVICE_AT + DEVICE_INDICES, INDICES).unwrap();
        m.set_u32(DEVICE_AT + DEVICE_BASE_VERTEX, 12).unwrap();
        m.set_u32(INDICES, refs).unwrap();
        m
    }

    /// The frame's finish makes its texture and pixel-shader resets on the
    /// device through the host, with the body's own writes, and no guest
    /// D3D entry: only the stages the engine had committed are set.
    #[test]
    fn the_frame_finish_makes_no_d3d_entry() {
        let dev = Device::default();
        let mut m = finish_mem((2 * DEVICE_REF) | 1);
        assert_eq!(cut_finish(&mut m, &dev), Ok(()));
        assert_eq!(*dev.textures.lock().unwrap(), [(0, 0), (2, 0)], "the committed stages, in order");
        assert_eq!(*dev.shaders.lock().unwrap(), [0], "the pixel shader none");
        for stage in 0..4 {
            assert_eq!(m.u32(STAGE_REQUESTED + 4 * stage).unwrap(), 0);
            assert_eq!(m.u32(STAGE_COMMITTED + 4 * stage).unwrap(), 0);
        }
        assert_eq!(m.u32(FINISH_UNSET).unwrap(), u32::MAX);
        assert_eq!(m.u32(FINISH_CLEARED).unwrap(), 0);
        assert_eq!(m.u32(INDICES).unwrap(), DEVICE_REF | 1, "one device reference dropped");
        assert_eq!((m.u32(DEVICE_AT + DEVICE_INDICES).unwrap(), m.u32(DEVICE_AT + DEVICE_BASE_VERTEX).unwrap()), (0, 0));
        assert_eq!(cut_finish(&mut m, &dev), Ok(()), "a second finish");
        assert_eq!(dev.textures.lock().unwrap().len(), 2, "nothing committed is left to set");
        assert_eq!(m.u32(INDICES).unwrap(), DEVICE_REF | 1, "no index buffer to release");
    }

    /// The index buffer's last reference (its destroy) and a refused stage
    /// are named.
    #[test]
    fn a_finish_the_cut_cannot_make_is_named() {
        let dev = Device::default();
        let mut m = finish_mem(DEVICE_REF);
        assert_eq!(cut_finish(&mut m, &dev), Err("release"));
        assert_eq!(m.u32(INDICES).unwrap(), DEVICE_REF, "the reference not dropped");
        assert_eq!(m.u32(DEVICE_AT + DEVICE_INDICES).unwrap(), INDICES);
        assert!(dev.shaders.lock().unwrap().is_empty());
        let dev = Device::default();
        dev.refuse.store(true, Ordering::Relaxed);
        let mut m = finish_mem(2 * DEVICE_REF);
        assert_eq!(cut_finish(&mut m, &dev), Err("device"));
        assert_eq!(m.u32(STAGE_COMMITTED).unwrap(), A, "the stage's cache kept");
        assert_eq!(m.u32(FINISH_CLEARED).unwrap(), 7);
    }
}
