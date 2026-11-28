//! `native-post`: the view's post passes drawn by the native renderer
//! (docs/graphics/native-post-hud.md, "Stage 5: post and HUD";
//! docs/graphics/retire-d3d-display.md, "The post and the screen
//! effects"). On with the native renderer (`impl.render.native`, on by
//! default).
//!
//! The colour pass `0x000214f0` and the screen effect `0x0002d140` are
//! Rust: no guest body runs. Each makes the guest's choices and writes in
//! Rust ([`colour_pass`], [`screen_effect`]) and hands what it draws to
//! the [`Host`], which draws it as native passes on the frame graph's
//! targets. A call the body or the host does not cover panics, named
//! `post: <cause>` ([`unhandled`]).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use engine::colour_pass::{self, Parameters};
use engine::memory::{GuestMemory, MemFault};
use engine::points;

use hooks::{BuildError, Builder, Feature, Manifest};

pub const ID: &str = "native-post";

/// The colour pass's stack words (`ret 0x1c`) and the screen effect's
/// (`ret 0x10`).
pub const COLOUR_ARGS: u32 = 7;
pub const SCREEN_ARGS: u32 = 4;

/// Whether the native post is on.
pub fn on(switches: &hooks::Switches) -> bool {
    hooks::switches::native_render_on(switches)
}

/// The caller's floating-point state at the call: the numbers are the
/// game's bit for bit only under it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fp {
    pub mxcsr: u32,
    pub x87_control: u16,
}

/// One call of `0x0002d140`: EAX and its four stack words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenEffect {
    /// The `egor` tag datum.
    pub datum: u32,
    /// The address of its four values.
    pub values: u32,
    pub columns: u32,
    pub rows: u32,
    /// The address of the corners (x, y, u, v each).
    pub corners: u32,
}

/// What the post draws through: boot's glue over the GPU and the device.
pub trait Host: Send + Sync {
    fn retire_map(&self) {}

    /// Draws the glow of the colour pass with parameters `p` at this
    /// point in the frame, as native passes on the graph's targets. `Err` names what it cannot draw.
    fn glow(&self, mem: &mut dyn GuestMemory, p: &Parameters, fp: Fp) -> Result<(), String>;

    /// Runs the screen effect `call` past its value copy: its passes
    /// drawn as native passes, the guest's writes made ([`resize`],
    /// [`end_writes`]). `Ok(true)` when it ran to its end
    /// (`0x0002dae8`), `Ok(false)` when it returned at once.
    fn screen_effect(&self, mem: &mut dyn GuestMemory, call: ScreenEffect, fp: Fp) -> Result<bool, String>;
}

static HOST: Mutex<Option<Arc<dyn Host>>> = Mutex::new(None);
static CALLS: AtomicU64 = AtomicU64::new(0);
static GLOWS: AtomicU64 = AtomicU64::new(0);
static EFFECTS: AtomicU64 = AtomicU64::new(0);
static EFFECTS_ENDED: AtomicU64 = AtomicU64::new(0);

/// Installs the host (boot). Without one every call panics.
pub fn set_host(host: Arc<dyn Host>) {
    *HOST.lock().unwrap_or_else(|e| e.into_inner()) = Some(host);
}

fn host() -> Arc<dyn Host> {
    HOST.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_else(|| unhandled("host"))
}

pub(crate) fn retire_map() {
    let host = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(host) = host {
        host.retire_map();
    }
}

/// A call the post cannot make: there is no guest body to run instead.
pub fn unhandled(cause: impl std::fmt::Display) -> ! {
    unimplemented!("post: {cause}")
}

/// The calls so far.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// Calls of the colour pass.
    pub calls: u64,
    /// Those that drew the glow.
    pub glows: u64,
    /// Calls of `0x0002d140`.
    pub effects: u64,
    /// Those that ran to the end (`0x0002dae8`).
    pub effects_ended: u64,
}

pub fn counts() -> Counts {
    Counts {
        calls: CALLS.load(Ordering::Relaxed),
        glows: GLOWS.load(Ordering::Relaxed),
        effects: EFFECTS.load(Ordering::Relaxed),
        effects_ended: EFFECTS_ENDED.load(Ordering::Relaxed),
    }
}

/// The counters `render.scene` reports.
pub fn counters() -> Vec<(String, u64)> {
    let c = counts();
    vec![
        ("post_colour_calls".into(), c.calls),
        ("post_colour_glows".into(), c.glows),
        ("post_screen_effect_calls".into(), c.effects),
        ("post_screen_effect_ended".into(), c.effects_ended),
    ]
}

/// The view's target index (a word), the target `0x00014bc0` binds at
/// the colour pass's end instead when not -1, target 0's colour surface,
/// and the target select's cache of the bound colour.
pub const VIEW_TARGET: u32 = 0x0048_58b8;
pub const EXIT_TARGET: u32 = 0x0048_58b4;
pub const TARGET0_COLOUR: u32 = crate::frame_tail::TARGET0_COLOUR;

fn stack(mem: &dyn GuestMemory, esp: u32, n: u32) -> Result<Vec<u32>, MemFault> {
    (0..n).map(|i| mem.u32(esp.wrapping_add(4 + 4 * i))).collect()
}

/// `0x00014bc0`, the target select by index ([`crate::target_bind`]).
pub const BIND_TARGET: u32 = crate::target_bind::BIND_TARGET;

/// The two binds of a colour pass that draws, by target index: the view's
/// target `[0x004858b8]` (a word), which `0x00034a90` binds for the
/// composite (`0x00021a1b..0x00021a3f`), and the one `0x00014bc0` binds at
/// the end (`0x00021a44..0x00021a6e`): `[0x004858b4]` when not -1, else
/// the view's again.
pub fn glow_binds(mem: &dyn GuestMemory) -> Result<(u32, u32), MemFault> {
    let view = u32::from(mem.u16(VIEW_TARGET)?);
    let exit = mem.u32(EXIT_TARGET)?;
    Ok((view, if exit == u32::MAX { view } else { exit }))
}

/// `0x000214f0`'s body up to its glow, on any guest memory, `esp` at its
/// entry: the override record's pending reset (`0x00021533..0x0002156a`).
/// `Some` with the parameters when the glow draws; `None` returns at once
/// (`0x00021a73`, no bind).
pub fn colour_pass(mem: &mut dyn GuestMemory, esp: u32, fp: Fp) -> Option<Parameters> {
    CALLS.fetch_add(1, Ordering::Relaxed);
    let words = stack(&*mem, esp, COLOUR_ARGS).unwrap_or_else(|_| unhandled("colour pass args"));
    let args: [u32; 7] = std::array::from_fn(|i| words[i]);
    // The frame step's scene takes the call's arguments by value
    // (docs/graphics/framescene.md, slice 5).
    crate::scene_step::post_colour(&*mem, args);
    let p = colour_pass::parameters(&*mem, args, fp.mxcsr)
        .unwrap_or_else(|e| unhandled(format!("colour pass parameters ({e:?})")));
    p.write_reset(mem).unwrap_or_else(|e| unhandled(format!("colour pass reset ({e:?})")));
    p.draws.then_some(p)
}

/// The glow through the host, the view's target bound ([`glow_binds`]).
pub fn glow(mem: &mut dyn GuestMemory, p: &Parameters, fp: Fp) {
    host().glow(mem, p, fp).unwrap_or_else(|cause| unhandled(format!("glow: {cause}")));
    GLOWS.fetch_add(1, Ordering::Relaxed);
}

/// Where `0x0002d140` copies the call's four values (`0x0002d140..0x0002d16a`),
/// which its passes' engine constants read.
pub const SCREEN_VALUES: u32 = 0x004c_19b0;

/// The call's four stack words at `esp` with `eax`.
pub fn screen_call(mem: &dyn GuestMemory, eax: u32, esp: u32) -> Result<ScreenEffect, MemFault> {
    let w = stack(mem, esp, SCREEN_ARGS)?;
    Ok(ScreenEffect { datum: eax, values: w[0], columns: w[1], rows: w[2], corners: w[3] })
}

/// `0x0002d140`'s body, `eax` the datum and `esp` at its entry: the
/// values copied, then the host's draw. EAX at its return: -1 from the
/// end (`0x0002db4d`), else the datum.
pub fn screen_effect(mem: &mut dyn GuestMemory, eax: u32, esp: u32, fp: Fp) -> u32 {
    EFFECTS.fetch_add(1, Ordering::Relaxed);
    let call = screen_call(&*mem, eax, esp).unwrap_or_else(|_| unhandled("screen effect args"));
    // A converted Halo 1 scope-zoom profile is no egor the plan reads.
    if crate::scope_zoom_hud::hud_profile(&*mem, eax, esp).is_some() {
        unhandled("scope_zoom");
    }
    crate::scene_step::post_effect(&*mem, call.datum, call.values, call.columns, call.rows);
    let values = (0..4)
        .map(|i| mem.u32(call.values.wrapping_add(4 * i)))
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_else(|_| unhandled("screen effect values"));
    for (i, v) in values.iter().enumerate() {
        mem.set_u32(SCREEN_VALUES + 4 * i as u32, *v).unwrap_or_else(|_| unhandled("screen effect values"));
    }
    let ended = host().screen_effect(mem, call, fp).unwrap_or_else(|cause| unhandled(format!("screen effect: {cause}")));
    if ended {
        EFFECTS_ENDED.fetch_add(1, Ordering::Relaxed);
        u32::MAX
    } else {
        eax
    }
}

/// Halo's render-target table (`0x004b4b58`, stride `0x98`): width
/// `+0x80`, height `+0x84`, surface `+0x88`, the hidden-size byte
/// `+0x95`, the flag `0x0001ddd0` reads `+0x94`, and the size words of
/// the entry's embedded texture and surface headers (`+0x14`, `+0x28`).
pub const TARGETS: u32 = crate::native_targets::TARGETS;
pub const TARGET_STRIDE: u32 = crate::native_targets::TARGET_STRIDE;

fn entry(index: u32) -> u32 {
    TARGETS.wrapping_add(index.wrapping_mul(TARGET_STRIDE))
}

/// `0x0001ddd0(ecx index, esi width, edi height)`: the table's size and
/// both headers' size words (Code, `0x0001ddd0..0x0001de1e`), the words
/// zero unless `+0x94` is set.
pub fn resize(mem: &mut dyn GuestMemory, index: u32, width: u32, height: u32) -> Result<(), MemFault> {
    let e = entry(index);
    let pitch = (width.wrapping_mul(4) >> 6) << 24;
    let mut word = pitch.wrapping_sub(0x0100_0000) | (height << 12).wrapping_sub(0x1000) | width.wrapping_sub(1);
    if mem.u8(e + 0x94)? == 0 {
        word = 0;
    }
    mem.set_u32(e + 0x80, width)?;
    mem.set_u32(e + 0x84, height)?;
    mem.set_u32(e + 0x14, word)?;
    mem.set_u32(e + 0x28, word)
}

/// The targets whose sizes `0x0002d140` saves on entry
/// (`0x0002d26e..0x0002d344`) and writes back at its end.
pub const SAVED: [u32; 4] = [1, 0x19, 0x1a, 0x1b];

/// The saved sizes of [`SAVED`]: the table's, or 0 by 0 for a target with
/// no surface or a hidden size.
pub fn save_sizes(mem: &dyn GuestMemory) -> Result<[(u32, u32); 4], MemFault> {
    let mut out = [(0, 0); 4];
    for (o, t) in out.iter_mut().zip(SAVED) {
        let e = entry(t);
        if mem.u32(e + 0x88)? != 0 && mem.u8(e + 0x95)? == 0 {
            *o = (mem.u32(e + 0x80)?, mem.u32(e + 0x84)?);
        }
    }
    Ok(out)
}

/// The template's draw-state cache `0x00016b10` clears at the end, its
/// restore pointer `+0x24`, the float it stores (`[0x0045dbd4]`), the
/// flag set and the two results cleared.
pub const TEMPLATE_CACHE: u32 = 0x0048_5b48;
pub const TEMPLATE_ONE: u32 = 0x0045_dbd4;
pub const END_FLAG: u32 = 0x0046_70bc;
pub const RESULTS: [u32; 2] = [0x0048_5a78, 0x0048_5a7c];

/// The end of `0x0002d140` (Code, `0x0002dae8..0x0002db5a`), `saved`
/// from [`save_sizes`]: target 1, `0x19` and `0x1b` given their saved
/// sizes back, `0x19` written a second time with `0x1a`'s (the
/// original's order; `0x1a` keeps what the call set), `[0x004670bc]`
/// set, `0x00016b10` on the template cache, the results -1. `Err` names
/// what it cannot make: a cache with restore states to apply
/// (`0x00018900`, a D3D call) is `template_restore`.
pub fn end_writes(mem: &mut dyn GuestMemory, saved: &[(u32, u32); 4]) -> Result<(), String> {
    let fault = |e: MemFault| format!("end writes ({e:?})");
    for (t, (w, h)) in [(1, saved[0]), (0x19, saved[1]), (0x19, saved[2]), (0x1b, saved[3])] {
        resize(mem, t, w, h).map_err(fault)?;
    }
    mem.write(END_FLAG, &[1]).map_err(fault)?;
    let c = TEMPLATE_CACHE;
    if mem.u32(c + 0x24).map_err(fault)? != 0 {
        return Err("template_restore".into());
    }
    let one = mem.u32(TEMPLATE_ONE).map_err(fault)?;
    for off in (0xc..=0x24).step_by(4) {
        mem.set_u32(c + off, 0).map_err(fault)?;
    }
    for off in [0, 0x40, 0x1320, 0x1360, 0x13a0, 0x13e0] {
        mem.set_u32(c + off, u32::MAX).map_err(fault)?;
    }
    for off in [0x1330, 0x1370, 0x13b0, 0x13f0] {
        mem.set_u32(c + off, one).map_err(fault)?;
    }
    for r in RESULTS {
        mem.set_u32(r, u32::MAX).map_err(fault)?;
    }
    Ok(())
}

/// `0x00014f60`'s texture for Halo's target `index` (Code,
/// `0x00014f88..0x00014fa0`): its case byte `[0x00015154 + index]`, the
/// case's code `[0x00015114 + 4 * case]`. The two buffer cases pick
/// `[0x00509354 + 4 * n]`, `n` the frame count `[0x00485aa0]` (64 bits)
/// less one, or not, modulo 2 (`_allrem`, `0x00373150`); four cases a
/// fixed header. Another case is `Err`.
pub const TEXTURE_CASE: u32 = 0x0001_5154;
pub const TEXTURE_JUMP: u32 = 0x0001_5114;
pub const FRAME_COUNT: u32 = 0x0048_5aa0;
pub const BUFFER_TEXTURES: u32 = 0x0050_9354;

pub fn target_texture(mem: &dyn GuestMemory, index: u32) -> Result<u32, &'static str> {
    let unread = |_| "unread";
    let case = mem.u8(TEXTURE_CASE.wrapping_add(index)).map_err(unread)?;
    let code = mem.u32(TEXTURE_JUMP + 4 * u32::from(case)).map_err(unread)?;
    let count = || -> Result<i64, &'static str> {
        let lo = mem.u32(FRAME_COUNT).map_err(unread)?;
        let hi = mem.u32(FRAME_COUNT + 4).map_err(unread)?;
        Ok(((u64::from(hi) << 32) | u64::from(lo)) as i64)
    };
    let buffer = |n: i64| mem.u32(BUFFER_TEXTURES.wrapping_add((n as u32).wrapping_mul(4))).map_err(unread);
    match code {
        0x0001_4fa7 => buffer(count()?.wrapping_sub(1) % 2),
        0x0001_4fd4 => buffer(count()? % 2),
        0x0001_4ffc => mem.u32(0x0050_9374).map_err(unread),
        0x0001_500e => mem.u32(0x0050_937c).map_err(unread),
        0x0001_5020 => mem.u32(0x0050_9368).map_err(unread),
        0x0001_5032 => mem.u32(0x0050_936c).map_err(unread),
        _ => Err("texture case"),
    }
}

fn fp(cpu: &runtime::CpuAbi) -> Fp {
    Fp { mxcsr: cpu.mxcsr, x87_control: cpu.x87.control }
}

/// Returns from the call at `esp` (its return address at `[esp]`) past
/// `args` stack words.
fn ret(cpu: &mut runtime::CpuAbi, mem: &dyn GuestMemory, esp: u32, args: u32) -> Result<runtime::TransferAbi, runtime::GuestFault> {
    let to = mem.u32(esp).map_err(fault)?;
    cpu.esp = esp.wrapping_add(4 + 4 * args);
    Ok(runtime::TransferAbi::ret(to))
}

fn fault(f: MemFault) -> runtime::GuestFault {
    runtime::GuestFault {
        addr: f.addr,
        len: f.len as usize,
        access: if f.write { runtime::guest::Access::Write } else { runtime::guest::Access::Read },
        reason: runtime::guest::FaultReason::Unmapped,
    }
}

/// `0x00014bc0(target, 0)` with AL 1 ([`crate::target_bind::bind`]),
/// the viewport staged below the body's `esp`.
fn bind(mem: &mut dyn GuestMemory, esp: u32, target: u32) {
    let host = crate::native_targets::installed("post");
    let index = i32::from(target as u16 as i16);
    crate::target_bind::bind(mem, &*host, index, 0, true, esp.wrapping_sub(crate::view_setup::SCRATCH))
        .unwrap_or_else(|cause| unhandled(format!("bind: {cause}")));
}

/// The colour pass: its body to the glow, then, when it draws, the view's
/// target bound, the glow on it and the end's bind ([`glow_binds`]).
fn colour_call(cpu: &mut runtime::CpuAbi, ctx: &mut runtime::ExecCtx, _addr: u32) -> Result<runtime::TransferAbi, runtime::GuestFault> {
    let rt = Arc::clone(ctx.runtime());
    let esp = cpu.esp;
    let fp = fp(cpu);
    let mut mem = rt.memory();
    let Some(p) = colour_pass(&mut *mem, esp, fp) else {
        return ret(cpu, &*mem, esp, COLOUR_ARGS);
    };
    let (view, end) = glow_binds(&*mem).map_err(fault)?;
    bind(&mut *mem, esp, view);
    glow(&mut *mem, &p, fp);
    bind(&mut *mem, esp, end);
    ret(cpu, &*mem, esp, COLOUR_ARGS)
}

fn screen_call_body(cpu: &mut runtime::CpuAbi, ctx: &mut runtime::ExecCtx, _addr: u32) -> Result<runtime::TransferAbi, runtime::GuestFault> {
    let rt = Arc::clone(ctx.runtime());
    let mut mem = rt.memory();
    let esp = cpu.esp;
    // A shadowed HUD widget's call is checked against Rust's.
    crate::native_hud::walk::observe_screen(&*mem, cpu.eax, esp);
    cpu.eax = screen_effect(&mut *mem, cpu.eax, esp, fp(cpu));
    ret(cpu, &*mem, esp, SCREEN_ARGS)
}

pub struct NativePost;

impl Feature for NativePost {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "The view's post passes drawn by the native GPU renderer").doc("docs/graphics/native-renderer.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        // Stateless, on or off: no history carried between frames (no
        // temporal AA, no exposure adaptation); every call is drawn from
        // what the guest asks for at its entry. The host's screen effects
        // are keyed by the map name it reads from guest RAM.
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        if !on(b.switches()) {
            return Ok(());
        }
        b.raw().implement(&points::COLOUR_PASS, "native_post_colour", colour_call).context_only();
        b.raw().implement(&points::SCREEN_EFFECT, "native_post_screen_effect", screen_call_body).context_only();
        Ok(())
    }
}
