//! The frame tail `0x00013b90` in Rust with the native renderer
//! (docs/graphics/retire-d3d-display.md, "The frame tail"): the freeze
//! frame's copy and composite as overlay items, so the tail makes no
//! guest D3D entry.
//!
//! The guest's body (Code, `0x00013b90..0x00013c17`):
//!
//! 1. `0x0001fd20` (EAX 0): the fade record `[0x005093e0]`, its fade
//!    `+0xa4` brought up to the clock. The guest's call: it has no D3D
//!    entry.
//! 2. No record, or `+0xa4` at or below `[0x0045dbd8]`: return.
//! 3. `+0xa0` set: target `0x12` allocated (`0x0001dc40`, unless
//!    `[0x004b5690]`), the view's target copied into it (`0x000351a0`)
//!    and `0x0001fd00` (`+0xa0` = 0); refused, `0x0001fcd0` (the record
//!    zeroed).
//! 4. The composite `0x00022070(+0xa4, +0xa8)`: target `0x12` over the
//!    view's target, blend mode 7, the alpha [`composite_alpha`] in the
//!    final combiner's constant ([`composite_words`]).
//!
//! Here the copy is an [`OverlayKind::Hold`](frame_scene::OverlayKind::Hold)
//! of [`HOLD`], kept by the renderer across frames, and the composite a
//! quad over the window sampling it with the guest's combiner and blend.
//! Target `0x12` is never allocated, so `0x0001dc40` and the cache drain
//! under it do not run. The copy's success is `0x0001fd00`'s write, a
//! pass the host did not place `0x0001fcd0`'s.
//!
//! A tail this does not cover runs the guest's body, counted
//! `scene_kept_frame_tail_<cause>` ([`setup`]): `host`, `target` (the view's
//! target is not target 0, or target 0 is not the bound one), `size`,
//! `window`, `blend`, `unread`. Native tails count `frame_tail_native`,
//! `frame_tail_copies`, `frame_tail_composites`; a pass the host did not
//! place `frame_tail_refused_pass`, and a hold or held texture the GPU
//! refused `frame_tail_refused_hold` ([`refused`]).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use engine::memory::{GuestMemory, MemFault};
use frame_scene::{ClipRect, Overlay, OverlayBlend, OverlayItem, OverlayMaterial, OverlayTexture, OverlayVertex, Program, Sampler};
use native_render::camera::GuestRead;
use native_render::hud::Blend;

/// `0x0001fd20`: the fade record, brought up to the clock.
pub const FADE_RECORD: u32 = 0x0001_fd20;
/// The fade record's pointer (`0x0001fd00`, `0x0001fcd0`).
pub const RECORD: u32 = 0x0050_93e0;
/// The record's copy flag (a byte), fade and exponent.
pub const COPY_FLAG: u32 = 0xa0;
pub const FADE: u32 = 0xa4;
pub const EXPONENT: u32 = 0xa8;
/// The record's size zeroed by `0x0001fcd0` (`0xff` words).
pub const RECORD_WORDS: u32 = 0xff;
/// The fade's floor (`[0x0045dbd8]`), one (`[0x0045dbc0]`) and the
/// alpha's scale to the constant's byte (`[0x0045dc38]`).
pub const FLOOR: u32 = 0x0045_dbd8;
pub const ONE: u32 = 0x0045_dbc0;
pub const ALPHA_SCALE: u32 = 0x0045_dc38;
/// `0x0001fcd0`'s gate and flag.
pub const ALLOCATED: u32 = 0x004b_5690;
pub const REFUSED: u32 = 0x004b_569d;
/// The view's target index (a word) and target 0's colour surface
/// (`0x00014bc0`'s first case).
pub const VIEW_TARGET: u32 = 0x0048_58b8;
pub const TARGET0_COLOUR: u32 = 0x0050_935c;
/// Halo's render-target table (`0x0001dd60`): stride `0x98`, width
/// `+0x80`, height `+0x84`, surface `+0x88`, a byte `+0x95` that hides
/// the size; and Halo's window (shorts: top, left, bottom, right), as
/// `native_render::glow` reads them.
pub const TARGETS: u32 = 0x004b_4b58;
pub const TARGET_STRIDE: u32 = 0x98;
pub const WINDOW: u32 = 0x004b_9dd0;
/// The composite's blend mode (`0x00022070`'s second argument to
/// `0x00034a90`).
pub const COMPOSITE_BLEND: u16 = 7;
/// The renderer's held texture the copy keeps the frame in.
pub const HOLD: u32 = 0x12;
/// The composite's combiner block: no tag, the guest's words.
pub const COMPOSITE_PROGRAM: u32 = u32::MAX;

/// What answers the device's side: boot's glue.
pub trait Host: Send + Sync {
    /// Places `overlay` at this point in the frame over the bound colour
    /// target, unfitted: whether the device took it.
    fn place(&self, mem: &mut dyn GuestMemory, overlay: Overlay) -> bool;
}

static HOST: Mutex<Option<Arc<dyn Host>>> = Mutex::new(None);

/// Installs the host (boot). Without one every tail runs the guest body.
pub fn set_host(host: Arc<dyn Host>) {
    *HOST.lock().unwrap_or_else(|e| e.into_inner()) = Some(host);
}

fn host() -> Option<Arc<dyn Host>> {
    HOST.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

#[derive(Default)]
struct Counts {
    calls: AtomicU64,
    native: AtomicU64,
    copies: AtomicU64,
    composites: AtomicU64,
    kept: Mutex<BTreeMap<&'static str, u64>>,
    refused: Mutex<BTreeMap<&'static str, u64>>,
}

static COUNTS: std::sync::LazyLock<Counts> = std::sync::LazyLock::new(Counts::default);

fn bump(map: &Mutex<BTreeMap<&'static str, u64>>, cause: &'static str) {
    *map.lock().unwrap_or_else(|e| e.into_inner()).entry(cause).or_default() += 1;
}

/// A native tail's part refused after it was placed: `pass` (the host did
/// not place it), `hold` (the GPU had no held texture to copy into or
/// sample).
pub fn refused(cause: &'static str) {
    bump(&COUNTS.refused, cause);
}

/// The counters `render.scene` reports.
pub fn counters() -> Vec<(String, u64)> {
    let c = &*COUNTS;
    let mut out = vec![
        ("frame_tail_calls".to_string(), c.calls.load(Ordering::Relaxed)),
        ("frame_tail_native".to_string(), c.native.load(Ordering::Relaxed)),
        ("frame_tail_copies".to_string(), c.copies.load(Ordering::Relaxed)),
        ("frame_tail_composites".to_string(), c.composites.load(Ordering::Relaxed)),
    ];
    let map = |m: &Mutex<BTreeMap<&'static str, u64>>, prefix: &str| -> Vec<(String, u64)> {
        m.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|(k, n)| (format!("{prefix}{k}"), *n)).collect()
    };
    out.extend(map(&c.kept, "scene_kept_frame_tail_"));
    out.extend(map(&c.refused, "frame_tail_refused_"));
    out
}

struct Ro<'a>(&'a dyn GuestMemory);

impl GuestRead for Ro<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }
}

fn f32_at(mem: &dyn GuestMemory, addr: u32) -> Result<f32, MemFault> {
    mem.u32(addr).map(f32::from_bits)
}

/// What the tail draws into, read before the record: the bound target's
/// size in guest pixels, Halo's window `(left, top, right, bottom)` in
/// them, the composite's blend.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Setup {
    pub size: [u32; 2],
    pub window: [f32; 4],
    pub blend: OverlayBlend,
}

/// The tail's target and state, or why the guest body runs. The copy
/// reads and the composite writes the view's target `[0x004858b8]`
/// (`0x00014bc0`): covered when that is target 0 and target 0's colour
/// `[0x0050935c]` is the bound one (the target select's cache
/// `[0x0051f3f4]`), so the overlay draws where the guest's quads would.
pub fn setup(mem: &dyn GuestMemory) -> Result<Setup, &'static str> {
    let unread = |_| "unread";
    if mem.u32(VIEW_TARGET).map_err(unread)? as u16 != 0
        || mem.u32(crate::native_targets::CACHE_COLOUR).map_err(unread)? != mem.u32(TARGET0_COLOUR).map_err(unread)?
    {
        return Err("target");
    }
    let size = target0_size(mem).map_err(unread)?;
    if size.0 == 0 || size.1 == 0 {
        return Err("size");
    }
    let window = read_window(mem).map_err(unread)?;
    let [x0, y0, x1, y1] = window;
    let ro = Ro(mem);
    if !(0.0 <= x0 && x0 < x1 && x1 <= size.0 as f32 && 0.0 <= y0 && y0 < y1 && y1 <= size.1 as f32) {
        return Err("window");
    }
    let blend = match Blend::read(&ro, COMPOSITE_BLEND).ok_or("blend")? {
        Blend::Off => OverlayBlend::Off,
        // A constant factor names the device's blend colour, which the
        // overlay does not read.
        Blend::On { source, dest, .. } if [source, dest].iter().any(|f| (0x8001..=0x8004).contains(f)) => {
            return Err("blend");
        }
        Blend::On { source, dest, equation } => OverlayBlend::On { source, dest, equation },
    };
    Ok(Setup { size: [size.0, size.1], window, blend })
}

/// Target 0's size as `0x0001dd60` gives it: the table's, or 640 by 480
/// with no surface or a hidden size.
pub(crate) fn target0_size(mem: &dyn GuestMemory) -> Result<(u32, u32), MemFault> {
    if mem.u32(TARGETS + 0x88)? != 0 && mem.u8(TARGETS + 0x95)? == 0 {
        return Ok((mem.u32(TARGETS + 0x80)?, mem.u32(TARGETS + 0x84)?));
    }
    Ok((640, 480))
}

/// Halo's window `(left, top, right, bottom)`.
fn read_window(mem: &dyn GuestMemory) -> Result<[f32; 4], MemFault> {
    let short = |at: u32| -> Result<f32, MemFault> { Ok(f32::from(mem.u16(at)? as i16)) };
    Ok([short(WINDOW + 2)?, short(WINDOW)?, short(WINDOW + 6)?, short(WINDOW + 4)?])
}

/// `0x00022070`'s alpha for fade `fade` and exponent `exponent` (Code,
/// `0x00022070..0x0002218d`): `min(fade, 1) ^ max(exponent, 0)`, 1 at
/// most; `None` where it draws nothing (a fade at or below 0, or an alpha
/// below `floor`, or at or below it after the clamp).
pub fn composite_alpha(fade: f32, exponent: f32, one: f32, floor: f32) -> Option<f32> {
    if fade.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
        return None;
    }
    let base = if fade > one { one } else { fade };
    let exponent = if exponent > 0.0 { exponent } else { 0.0 };
    let p = f64::from(base).powf(f64::from(exponent));
    if f64::from(floor) > p {
        return None;
    }
    if p > f64::from(one) {
        return Some(one);
    }
    let alpha = p as f32;
    (alpha > floor).then_some(alpha)
}

/// Words of the combiner block (`D3DPIXELSHADERDEF`).
pub const COMBINER_WORDS: usize = 60;

/// The composite's combiner block (Code, `0x000221e2..0x00022265`): the
/// block at `0x00484f68` zeroed, one stage, stage 0 a 2D texture, the
/// final combiner `t0.rgb * c0.a` and alpha `c0.a`, its constant 0 the
/// alpha `alpha * [0x0045dc38]` rounded in its top byte.
pub fn composite_words(alpha: f32, scale: f32) -> [u32; COMBINER_WORDS] {
    let mut words = [0u32; COMBINER_WORDS];
    // PSFinalCombinerInputsABCD, ...EFG: A t0, B c0's alpha; G c0's alpha.
    words[0x20 / 4] = 0x0811_0000;
    words[0x24 / 4] = 0x0000_1100;
    // PSFinalCombinerConstant0: `fistp` of the product, `<< 24`.
    words[0xac / 4] = ((f64::from(alpha) * f64::from(scale)).round_ties_even() as i32 as u32) << 24;
    // PSCombinerCount, PSTextureModes.
    words[0xd4 / 4] = 0x0001_1001;
    words[0xd8 / 4] = 1;
    words
}

/// The tail once `0x0001fd20` returned: whether it copies, and the
/// composite's alpha when it draws. `None`: no record, or a fade at or
/// below the floor; the tail returns.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tail {
    pub copy: bool,
    pub alpha: Option<f32>,
}

/// Steps 2 to 4's reads of `record` (EAX of `0x0001fd20`).
pub fn read(mem: &dyn GuestMemory, record: u32) -> Result<Option<Tail>, MemFault> {
    if record == 0 {
        return Ok(None);
    }
    let floor = f32_at(mem, FLOOR)?;
    let fade = f32_at(mem, record.wrapping_add(FADE))?;
    // `comiss; jbe`: at or below the floor, or unordered.
    if fade.partial_cmp(&floor) != Some(std::cmp::Ordering::Greater) {
        return Ok(None);
    }
    let copy = mem.u8(record.wrapping_add(COPY_FLAG))? != 0;
    let exponent = f32_at(mem, record.wrapping_add(EXPONENT))?;
    let alpha = composite_alpha(fade, exponent, f32_at(mem, ONE)?, floor);
    Ok(Some(Tail { copy, alpha }))
}

/// The tail's overlay over `setup`'s target: the hold when it copies,
/// then the composite's quad over the window, each corner's coordinate
/// its own pixel of the held texture (`0x00034a90` with one tap and no
/// spread), every channel written (`0x01010101`).
pub fn overlay(setup: &Setup, tail: &Tail, scale: f32) -> Option<Overlay> {
    let mut overlay = Overlay::new(setup.size);
    if tail.copy {
        overlay.push_hold(HOLD).ok()?;
    }
    if let Some(alpha) = tail.alpha {
        let [x0, y0, x1, y1] = setup.window;
        let (w, h) = (setup.size[0] as f32, setup.size[1] as f32);
        let corner = |x: f32, y: f32| OverlayVertex { position: [x, y], uv: [x / w, y / h], color: [1.0; 4] };
        let material = OverlayMaterial {
            program: Program::Shader { tag: COMPOSITE_PROGRAM },
            texture: OverlayTexture::Held { hold: HOLD },
            // Clamp and point (`[0x00404dd0..]` 3 and 1).
            sampler: Sampler { wrap: false, point: true },
            blend: setup.blend,
        };
        let corners = [corner(x0, y0), corner(x1, y0), corner(x1, y1), corner(x0, y1)];
        let n = overlay.push_quad(material, ClipRect::full(setup.size), corners).ok()? as usize;
        let item = &mut overlay.items[n];
        item.shader_words = Some(composite_words(alpha, scale).to_vec().into_boxed_slice());
        item.shader_stages = [Some(OverlayTexture::Held { hold: HOLD }), None, None, None];
        item.color_write = OverlayItem::RGB_WRITE | (1 << 24);
    }
    (!overlay.is_empty()).then_some(overlay)
}

/// `0x0001fd00`: the copy made.
fn copied(mem: &mut dyn GuestMemory) -> Result<(), MemFault> {
    let record = mem.u32(RECORD)?;
    if record != 0 {
        mem.write(record.wrapping_add(COPY_FLAG), &[0])?;
    }
    Ok(())
}

/// `0x0001fcd0`: the copy refused, the record zeroed.
fn copy_refused(mem: &mut dyn GuestMemory) -> Result<(), MemFault> {
    let record = mem.u32(RECORD)?;
    if record == 0 {
        return Ok(());
    }
    for n in 0..RECORD_WORDS {
        mem.set_u32(record.wrapping_add(4 * n), 0)?;
    }
    if mem.u32(ALLOCATED)? != 0 && mem.u8(REFUSED)? == 0 {
        mem.write(REFUSED, &[1])?;
    }
    Ok(())
}

/// What the native tail did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Made {
    pub copied: bool,
    pub composited: bool,
}

/// Steps 2 to 4 for `record` over `setup`, through `host`: no guest call.
/// A pass the host does not place is a refused copy (`0x0001fcd0`'s
/// writes, so the composite reads no fade) and no composite, counted
/// `pass`.
pub fn finish(mem: &mut dyn GuestMemory, host: &dyn Host, setup: &Setup, record: u32) -> Result<Made, MemFault> {
    let Some(tail) = read(&*mem, record)? else {
        return Ok(Made::default());
    };
    let scale = f32_at(&*mem, ALPHA_SCALE)?;
    let Some(overlay) = overlay(setup, &tail, scale) else {
        return Ok(Made::default());
    };
    let placed = host.place(mem, overlay);
    if !placed {
        refused("pass");
    }
    if tail.copy {
        if placed { copied(mem)? } else { copy_refused(mem)? }
    }
    Ok(Made { copied: tail.copy && placed, composited: tail.alpha.is_some() && placed })
}

fn fault(f: MemFault) -> runtime::GuestFault {
    runtime::GuestFault {
        addr: f.addr,
        len: f.len as usize,
        access: if f.write { runtime::guest::Access::Write } else { runtime::guest::Access::Read },
        reason: runtime::guest::FaultReason::Unmapped,
    }
}

fn original(cpu: &mut runtime::CpuAbi, ctx: &mut runtime::ExecCtx, addr: u32) -> Result<runtime::TransferAbi, runtime::GuestFault> {
    if let Some(original) = ctx.runtime().resolve_original(addr) {
        return Ok(original.invoke(cpu, ctx, addr));
    }
    let event = ctx.emit(runtime::Event::UnsupportedTarget { pc: cpu.eip, target: addr, kind: runtime::abi::JUMP, route: runtime::abi::ENTRY });
    Ok(runtime::TransferAbi::fault(event))
}

/// `0x00013b90`'s entry: the setup, then `0x0001fd20` as the guest's call
/// and [`finish`] at its return, which returns to the tail's caller with
/// its callee-saved registers as they came in.
fn tail_call(cpu: &mut runtime::CpuAbi, ctx: &mut runtime::ExecCtx, addr: u32) -> Result<runtime::TransferAbi, runtime::GuestFault> {
    COUNTS.calls.fetch_add(1, Ordering::Relaxed);
    let Some(host) = host() else {
        bump(&COUNTS.kept, "host");
        return original(cpu, ctx, addr);
    };
    let rt = Arc::clone(ctx.runtime());
    let mut mem = rt.memory();
    let target = match setup(&*mem) {
        Ok(s) => s,
        Err(cause) => {
            drop(mem);
            bump(&COUNTS.kept, cause);
            return original(cpu, ctx, addr);
        }
    };
    COUNTS.native.fetch_add(1, Ordering::Relaxed);
    let (esp, ret) = (cpu.esp, mem.u32(cpu.esp)?);
    let saved = (cpu.ebx, cpu.esi, cpu.edi, cpu.ebp);
    cpu.eax = 0;
    ctx.call_guest_named(cpu, &mut mem, FADE_RECORD, "render: the frame tail", move |cpu, ctx| {
        let rt = Arc::clone(ctx.runtime());
        let made = {
            let mut mem = rt.memory();
            finish(&mut *mem, &*host, &target, cpu.eax).map_err(fault)?
        };
        if made.copied {
            COUNTS.copies.fetch_add(1, Ordering::Relaxed);
        }
        if made.composited {
            COUNTS.composites.fetch_add(1, Ordering::Relaxed);
        }
        (cpu.ebx, cpu.esi, cpu.edi, cpu.ebp) = saved;
        cpu.esp = esp.wrapping_add(4);
        Ok(runtime::TransferAbi::ret(ret))
    })
}

/// Places the tail's implementation (`native_targets`' feature).
pub fn place(b: &mut hooks::Builder<'_>) {
    b.raw().implement(&engine::points::RENDER_FRAME_TAIL, "native_frame_tail", tail_call).context_only();
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::memory::FlatMemory;
    use frame_scene::OverlayKind;

    const AT: u32 = 0x0050_0000;
    const SURFACE: u32 = 0x0060_0000;

    #[derive(Default)]
    struct Device {
        placed: Mutex<Vec<Overlay>>,
        refuse: bool,
    }

    impl Host for Device {
        fn place(&self, _: &mut dyn GuestMemory, overlay: Overlay) -> bool {
            if self.refuse {
                return false;
            }
            self.placed.lock().unwrap().push(overlay);
            true
        }
    }

    /// A screen effect's fade at 0.5, exponent 2, copy pending, over target
    /// 0 bound at 640 by 480 with the full window.
    fn world() -> FlatMemory {
        let mut m = FlatMemory::new(0x0040_0000, 0x0030_0000);
        m.set_u32(RECORD, AT).unwrap();
        m.write(AT + COPY_FLAG, &[1]).unwrap();
        m.set_u32(AT + FADE, 0.5f32.to_bits()).unwrap();
        m.set_u32(AT + EXPONENT, 2.0f32.to_bits()).unwrap();
        m.set_u32(FLOOR, 0.0f32.to_bits()).unwrap();
        m.set_u32(ONE, 1.0f32.to_bits()).unwrap();
        m.set_u32(ALPHA_SCALE, 255.0f32.to_bits()).unwrap();
        m.set_u32(TARGET0_COLOUR, SURFACE).unwrap();
        m.set_u32(crate::native_targets::CACHE_COLOUR, SURFACE).unwrap();
        let target0 = TARGETS;
        m.set_u32(target0 + 0x80, 640).unwrap();
        m.set_u32(target0 + 0x84, 480).unwrap();
        m.set_u32(target0 + 0x88, SURFACE).unwrap();
        // The window: top 0, left 0, bottom 480, right 640.
        let w = WINDOW;
        m.set_u32(w, 0).unwrap();
        m.set_u32(w + 4, 480 | 640 << 16).unwrap();
        let mode = 4 * u32::from(COMPOSITE_BLEND);
        m.set_u32(native_render::hud::BLEND_SOURCE + mode, 0x302).unwrap();
        m.set_u32(native_render::hud::BLEND_DEST + mode, 0x303).unwrap();
        m.set_u32(native_render::hud::BLEND_EQUATION + mode, 0x8006).unwrap();
        m
    }

    /// A frame with a screen effect: the copy and the composite are the
    /// host's one overlay, a hold then a quad over the held texture, and
    /// the tail's only guest call is `0x0001fd20` ([`finish`] takes no
    /// guest call at all), so no D3D entry is made. The copy's success is
    /// `0x0001fd00`'s write.
    #[test]
    fn a_frame_with_a_screen_effect_makes_no_d3d_entry() {
        let mut m = world();
        let dev = Device::default();
        let setup = setup(&m).unwrap();
        assert_eq!(setup, Setup { size: [640, 480], window: [0.0, 0.0, 640.0, 480.0], blend: OverlayBlend::ALPHA });
        assert_eq!(finish(&mut m, &dev, &setup, AT), Ok(Made { copied: true, composited: true }));
        let placed = dev.placed.lock().unwrap();
        assert_eq!(placed.len(), 1);
        let o = &placed[0];
        assert_eq!(o.items.iter().map(|i| i.kind).collect::<Vec<_>>(), [OverlayKind::Hold, OverlayKind::Quad]);
        assert_eq!(o.hold_of(&o.items[0]), Some(HOLD));
        let quad = &o.items[1];
        assert_eq!(quad.material.texture, OverlayTexture::Held { hold: HOLD });
        assert_eq!(quad.material.blend, OverlayBlend::ALPHA);
        assert_eq!(quad.color_write, 0x0101_0101);
        let words = quad.shader_words.as_deref().unwrap();
        assert_eq!(words.len(), COMBINER_WORDS);
        assert_eq!(words[0xac / 4], 64 << 24, "0.5 squared, times 255, rounded");
        let v = o.vertices_of(quad);
        assert_eq!((v[0].position, v[2].position, v[2].uv), ([0.0, 0.0], [640.0, 480.0], [1.0, 1.0]));
        assert_eq!(o.check(), Ok(()));
        assert_eq!(m.u8(AT + COPY_FLAG).unwrap(), 0, "0x0001fd00: the copy made");
        assert_eq!(m.u32(AT + FADE).unwrap(), 0.5f32.to_bits(), "the record kept");
    }

    /// The next frame composites from the kept texture with no copy; a
    /// fade at the floor draws nothing and places nothing.
    #[test]
    fn a_later_frame_samples_the_kept_texture_without_a_copy() {
        let mut m = world();
        m.write(AT + COPY_FLAG, &[0]).unwrap();
        let dev = Device::default();
        let setup = setup(&m).unwrap();
        assert_eq!(finish(&mut m, &dev, &setup, AT), Ok(Made { copied: false, composited: true }));
        {
            // Scoped: `&guard[0]` keeps the guard to the block's end, and
            // the lock below would then wait on itself.
            let placed = dev.placed.lock().unwrap();
            assert_eq!(placed[0].items.len(), 1);
            assert_eq!(placed[0].items[0].shader_stages[0], Some(OverlayTexture::Held { hold: HOLD }));
        }
        m.set_u32(AT + FADE, 0.0f32.to_bits()).unwrap();
        assert_eq!(finish(&mut m, &dev, &setup, AT), Ok(Made::default()));
        assert_eq!(finish(&mut m, &dev, &setup, 0), Ok(Made::default()), "no record");
        assert_eq!(dev.placed.lock().unwrap().len(), 1);
    }

    /// A pass the host does not place is the guest's refused copy: the
    /// record zeroed and the flag set, nothing composited.
    #[test]
    fn an_unplaced_pass_is_a_refused_copy() {
        let mut m = world();
        m.set_u32(ALLOCATED, 1).unwrap();
        let dev = Device { refuse: true, ..Device::default() };
        let setup = setup(&m).unwrap();
        assert_eq!(finish(&mut m, &dev, &setup, AT), Ok(Made::default()));
        assert_eq!((m.u32(AT + FADE).unwrap(), m.u8(AT + COPY_FLAG).unwrap()), (0, 0));
        assert_eq!(m.u8(REFUSED).unwrap(), 1);
        assert!(counters().iter().any(|(k, n)| k == "frame_tail_refused_pass" && *n >= 1));
    }

    /// Each case the tail does not cover is named; the guest body runs it.
    #[test]
    fn uncovered_tails_are_refused_by_name() {
        let refused = |f: &dyn Fn(&mut FlatMemory)| {
            let mut m = world();
            f(&mut m);
            setup(&m).err()
        };
        assert_eq!(refused(&|_| {}), None);
        assert_eq!(refused(&|m| m.set_u32(VIEW_TARGET, 2).unwrap()), Some("target"), "another view target");
        assert_eq!(refused(&|m| m.set_u32(crate::native_targets::CACHE_COLOUR, 0x0061_0000).unwrap()), Some("target"), "target 0 not bound");
        assert_eq!(refused(&|m| m.set_u32(WINDOW + 4, 480 | 700 << 16).unwrap()), Some("window"), "past the target");
        assert_eq!(refused(&|m| m.set_u32(WINDOW, 640 << 16).unwrap()), Some("window"), "empty");
        let mode = 4 * u32::from(COMPOSITE_BLEND);
        assert_eq!(refused(&|m| m.set_u32(native_render::hud::BLEND_SOURCE + mode, 0x8001).unwrap()), Some("blend"), "a constant factor");
    }

    /// `0x00022070`'s alpha, as its x87 code clamps and floors it.
    #[test]
    fn the_composites_alpha_is_the_guests() {
        assert_eq!(composite_alpha(0.5, 2.0, 1.0, 0.0), Some(0.25));
        assert_eq!(composite_alpha(2.0, 3.0, 1.0, 0.0), Some(1.0), "the fade at most 1");
        assert_eq!(composite_alpha(0.5, -1.0, 1.0, 0.0), Some(1.0), "the exponent at least 0");
        assert_eq!(composite_alpha(0.0, 1.0, 1.0, 0.0), None);
        assert_eq!(composite_alpha(f32::NAN, 1.0, 1.0, 0.0), None);
        assert_eq!(composite_alpha(0.1, 2.0, 1.0, 0.05), None, "below the floor");
        assert_eq!(composite_words(1.0, 255.0)[0xac / 4], 0xff00_0000);
        assert_eq!(composite_words(0.5, 255.0)[0xac / 4], 128 << 24, "127.5 rounds to even");
    }
}
