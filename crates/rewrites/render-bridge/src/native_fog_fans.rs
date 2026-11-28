//! `native-fog-fans`: the second fog's screen fans (`0x00025dc0`) drawn
//! by the native renderer (docs/graphics/retire-d3d-fog-fans.md). On with
//! the native renderer.
//!
//! Each fan's setup, which the fan `0x00034420` calls through EDI (return
//! `0x0003452e`), is cut ([`turn`]) when its frame holds `0x00025dc0`'s
//! fan call and the fan's corners callback:
//!
//! | Setup (corners, fan's return) | Here |
//! |---|---|
//! | layers `0x00026880` (`0x00026ca0`, `0x00025e04`) | `engine::fog_fans::layers` into target `0x13`, the fog patch's bitmap at stages 0..3 |
//! | layers' composite `0x00026e50` (`0x00027520`, `0x00025e2d`) | the branch byte clear: the weather plate `0x0003a5c0` (`engine::fog_fans::weather_plate_placement`), true its block with target 3 at stage 0 and `0x13` at stage 1 (`engine::fog_fans::plate_fan`); false or the byte set: the set branch, `engine::fog_fans::layers_composite` |
//! | composite `0x000276d0` (`0x00027960`, `0x00025e2d`) | `engine::fog_fans::composite` |
//!
//! [`Host::fan`] (`native_render::fog_fans`) draws the fan at this point in
//! the frame on the frame graph's surfaces by role (the targets' surface
//! headers); the setup makes the writes a later read sees (the depth
//! word) and returns AL 0, so no fan reaches D3D. The guest makes the path
//! it makes with the gate shut, but for the fan's draw-state reset
//! `0x00016b10` and rectangle. The setups' bodies never run.
//!
//! What the cut cannot draw is `unimplemented!("fog fans: <cause>")`: no
//! host (`host`), a pass the device does not place (`placed_<why>`), the
//! host's (a texture it cannot decode, a stage or copy it cannot make), a
//! bitmap that binds nothing (`texture_unbound`), a view
//! target other than 0 (`composite into target <n>`), a target the guest
//! has not made (`target 3 unmade`, `target <n> unmade`), a weather plate
//! call other than the composite's (`weather plate <cause>`: its kind,
//! flags or result), a fan not finite
//! (`fan_unfinite`), an operand the SSE or x87 model refuses (`simd`,
//! `x87`), an unreadable input (`unread`), and a fan placed but not
//! recorded ([`unported`], the GPU's cause).

use std::sync::{Arc, Mutex};

use engine::fog_fans::{self as ff, LayersDest};
use engine::memory::GuestMemory;
use frame_types::TargetRole;
use hooks::{BuildError, Builder, Feature, Manifest};
pub use native_render::fog_fans::{Coords, FanDraw, FanRaster, FanStage};

pub const ID: &str = "native-fog-fans";

/// The rasterizer data element `[0x00485a80]`, and the bitmap datum the
/// layers' composite binds at stage 2 (`[[0x00485a80] + 0x14]`, image 0).
pub const RASTERIZER: u32 = 0x0048_5a80;
pub const RASTERIZER_BITMAP: u32 = 0x14;

/// What a stage samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The surface at a header: a game target's (by role) or the view's.
    Surface(u32),
    /// A bitmap image's texture header as `0x000143c0` binds it
    /// ([`stage_texture`]), decoded from guest memory by the host.
    Texture(u32),
}

/// One fan to place: the draw, the surface header it draws into, and what
/// each of its stages samples.
#[derive(Clone, Debug, PartialEq)]
pub struct Fan {
    pub draw: FanDraw,
    pub dest: u32,
    pub sources: [Option<Source>; 4],
}

/// What draws the fans: boot's glue over the shared device.
pub trait Host: Send + Sync {
    /// Places `fan` at this point in the frame. `Err` names why it was not
    /// placed (nothing recorded).
    fn fan(&self, mem: &mut dyn GuestMemory, fan: &Fan) -> Result<(), &'static str>;
}

static HOST: Mutex<Option<Arc<dyn Host>>> = Mutex::new(None);

/// Installs the host (boot).
pub fn set_host(host: Arc<dyn Host>) {
    *HOST.lock().unwrap_or_else(|e| e.into_inner()) = Some(host);
}

fn host() -> Option<Arc<dyn Host>> {
    HOST.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Whether the cut is placed: with the native renderer.
pub fn on(switches: &hooks::Switches) -> bool {
    hooks::switches::native_render_on(switches)
}

/// A fan the native renderer cannot draw, by cause.
pub fn unported(cause: &str) -> ! {
    unimplemented!("fog fans: {cause}")
}

fn cause(e: engine::light_constants::Error) -> &'static str {
    match e {
        engine::light_constants::Error::Simd => "simd",
        engine::light_constants::Error::X87(_) => "x87",
        _ => "unread",
    }
}

/// `0x00025dc0`'s entry with `ecx` and `edx` under the call's MXCSR: a
/// gate the SSE model refuses or that does not read panics; the fans are
/// cut at their setups ([`turn`]).
pub fn before_fans(mem: &dyn GuestMemory, mxcsr: u32, ecx: u32, edx: u32) {
    if let Err(e) = ff::fans(mem, mxcsr, ecx as u8, edx as u8) {
        unported(cause(e));
    }
}

/// The select's target 3 (`0x00014bc0`, case `0x00014c21`): the colour
/// surface over the depth buffer's memory, `[0x00509370]` (`0x00014600`),
/// role `Slot(0x00509370)`. Table entry 3 is never made (`0x0001dd60`
/// sizes it 640 by 480 from nothing). Its stage bind `0x00014f60` binds
/// the texture over the same memory, `[0x00509368]`; the host samples the
/// surface.
pub const TARGET3: u32 = crate::native_targets::TARGET_SLOTS[0];

/// Target 3's surface header ([`TARGET3`]); `target 3 unmade` while the
/// global is 0.
fn target3(mem: &dyn GuestMemory) -> u32 {
    match mem.u32(TARGET3) {
        Ok(0) => unported("target 3 unmade"),
        Ok(h) => h,
        Err(_) => unported("unread"),
    }
}

/// Table target `n`'s first surface header, role `Game(n)`, as
/// `0x0001dcf0` returns it: live (`+0x88` set, the hiding byte `+0x95`
/// clear, a level at `+0x78`), else `target <n> unmade`.
fn game(mem: &dyn GuestMemory, n: u8) -> u32 {
    use crate::native_targets::{TARGET_STRIDE, TARGETS};
    let entry = TARGETS + TARGET_STRIDE * u32::from(n);
    let live = (|| Ok::<_, engine::MemFault>(mem.u32(entry + 0x88)? != 0 && mem.u8(entry + 0x95)? == 0 && (mem.u32(entry + 0x78)? as i32) > 0))()
        .unwrap_or_else(|_| unported("unread"));
    if !live {
        unported(&format!("target {n:#x} unmade"));
    }
    crate::native_targets::surface(TargetRole::Game(n)).unwrap_or_else(|| unported("target"))
}

/// The surface header of the view's target `n`: target 0's colour is the
/// view's own (`0x00014bc0`'s first case).
fn view_surface(mem: &dyn GuestMemory, n: i16) -> u32 {
    if n != 0 {
        unported(&format!("composite into target {n}"));
    }
    mem.u32(crate::frame_tail::TARGET0_COLOUR).unwrap_or_else(|_| unported("unread"))
}

/// What a setup call does.
#[derive(Debug, PartialEq, Eq)]
pub enum Turn {
    /// Not `0x00025dc0`'s fan: run the setup.
    Guest,
    /// The fan is placed and the setup's writes made: return AL 0.
    Return,
    /// A stage's image is not resident: ask the guest's texture cache
    /// `0x00012310` for it (eax the image, xmm2 the LOD 0), as
    /// `0x000143c0` does, then [`turn_with`] its header. Nothing is
    /// written before.
    Stream(u32),
}

/// The texture cache's get of an image's header (`eax` the image record,
/// `xmm2` the LOD), which `0x000143c0` calls: the header when resident,
/// else what the cache has after asking for the load.
pub const TEXTURE_CACHE: u32 = 0x0001_2310;

/// What [`fan`] makes of a setup call.
#[derive(Debug, PartialEq)]
pub enum Made {
    /// The fan to place; the setup's writes are made.
    Draw(Fan),
    /// [`Turn::Stream`]: nothing written.
    Stream(u32),
    /// The cache returned no header for a stage: the writes are made and
    /// nothing is drawn that frame.
    NoTexture,
}

/// The header `0x000143c0` binds for `bitmap`'s `image`: the resident one
/// (`native_render::effect_material::texture_cached`), else the one the
/// guest's cache returned for that image record in `streamed`, else
/// `Err` the record to ask about. A bitmap that binds nothing panics
/// (`texture_unbound`: the stage would keep the last texture).
pub fn stage_texture(mem: &dyn GuestMemory, bitmap: u32, image: i32, streamed: &[(u32, u32)]) -> Result<u32, u32> {
    use native_render::effect_material::{Cached, texture_cached};
    match texture_cached(&|a| mem.u32(a).ok(), bitmap, image).unwrap_or_else(|why| unported(why)) {
        Cached::Header(h) => Ok(h),
        Cached::Streaming { image } => streamed.iter().find(|(i, _)| *i == image).map(|&(_, h)| h).ok_or(image),
    }
}

/// Which of the three setups a cut is at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Setup {
    Layers,
    LayersComposite,
    Composite,
}

impl Setup {
    pub fn of(entry: u32) -> Option<Setup> {
        Some(match entry {
            ff::setup::LAYERS => Setup::Layers,
            ff::setup::LAYERS_COMPOSITE => Setup::LayersComposite,
            ff::setup::COMPOSITE => Setup::Composite,
            _ => return None,
        })
    }

    /// The fan's return and corners callback that make a call this
    /// setup's of `0x00025dc0`.
    fn frame(self) -> (u32, u32) {
        match self {
            Setup::Layers => (ff::LAYERS_RETURN, ff::setup::LAYERS_CORNERS),
            Setup::LayersComposite => (ff::SECOND_RETURN, ff::setup::LAYERS_COMPOSITE_CORNERS),
            Setup::Composite => (ff::SECOND_RETURN, ff::setup::COMPOSITE_CORNERS),
        }
    }
}

fn floats<const N: usize>(v: [[u32; 4]; N]) -> [[f32; 4]; N] {
    v.map(|p| p.map(f32::from_bits))
}

fn refused(r: native_render::fog_fans::Refusal) -> ! {
    unported(r.cause())
}

const PIXELS: Option<FanStage> = Some(FanStage { coords: Coords::Pixels, wrap: false });

/// The fan of `setup` whose rectangle is at `rect_at`; `mem` takes the
/// setup's and its callback's writes. `streamed` holds the headers the
/// guest's texture cache returned so far, by image record.
pub fn fan(mem: &mut dyn GuestMemory, setup: Setup, rect_at: u32, mxcsr: u32, x87_control: u16, streamed: &[(u32, u32)]) -> Made {
    Made::Draw(match setup {
        Setup::Composite => {
            let c = ff::composite(&*mem, mxcsr, x87_control, rect_at).unwrap_or_else(|e| unported(cause(e)));
            let draw = FanDraw::composite(c.combiner, floats(c.positions), floats(c.coordinates)).unwrap_or_else(|r| refused(r));
            if ff::composite_writes(mem).is_err() {
                unported("depth_write");
            }
            Fan { draw, dest: view_surface(&*mem, c.target), sources: [Some(Source::Surface(target3(&*mem))), None, None, None] }
        }
        Setup::Layers => {
            // `0x000268cf`, per stage, before any write.
            let (bitmap, images) = ff::layers_bitmap(&*mem).unwrap_or_else(|_| unported("unread"));
            let mut headers = [0; 4];
            for (h, image) in headers.iter_mut().zip(images) {
                match stage_texture(&*mem, bitmap, i32::from(image as i16), streamed) {
                    Ok(t) => *h = t,
                    Err(record) => return Made::Stream(record),
                }
            }
            let l = ff::layers(mem, mxcsr, x87_control, rect_at).unwrap_or_else(|e| unported(cause(e)));
            if headers.contains(&0) {
                return Made::NoTexture;
            }
            let stage = Some(FanStage { coords: Coords::Unit, wrap: true });
            let draw = FanDraw::from_fan(l.combiner, floats(l.positions), l.coordinates.map(floats), [stage; 4], FanRaster::Opaque)
                .unwrap_or_else(|r| refused(r));
            let sources = headers.map(|h| Some(Source::Texture(h)));
            Fan { draw, dest: game(&*mem, ff::LAYERS_TARGET), sources }
        }
        Setup::LayersComposite => {
            if ff::weather_plate(&*mem).unwrap_or_else(|_| unported("unread")) {
                let args = ff::PlateArgs::fog(&*mem).unwrap_or_else(|_| unported("unread"));
                if let Some(why) = args.unported() {
                    unported(&format!("weather plate {why}"));
                }
                if let Some(plate) = ff::weather_plate_placement(mem, mxcsr, x87_control, &args).unwrap_or_else(|e| unported(cause(e))) {
                    return Made::Draw(plate_fan(mem, plate, rect_at, mxcsr));
                }
            }
            // Stage 2 before any write (a false plate writes nothing).
            let bitmap = mem
                .u32(RASTERIZER)
                .and_then(|e| mem.u32(e.wrapping_add(RASTERIZER_BITMAP)))
                .unwrap_or_else(|_| unported("unread"));
            let header = match stage_texture(&*mem, bitmap, 0, streamed) {
                Ok(h) => h,
                Err(record) => return Made::Stream(record),
            };
            let c = ff::layers_composite(mem, mxcsr, x87_control, rect_at).unwrap_or_else(|e| unported(cause(e)));
            if header == 0 {
                return Made::NoTexture;
            }
            let unit = Some(FanStage { coords: Coords::Unit, wrap: false });
            let raster = if c.blue_only() { FanRaster::BlueOnly } else { FanRaster::Constant };
            let draw = FanDraw::from_fan(c.combiner, floats(c.positions), c.coordinates.map(floats), [PIXELS, None, unit, PIXELS], raster)
                .unwrap_or_else(|r| refused(r));
            let dest = match c.dest {
                LayersDest::View(n) => view_surface(&*mem, n),
                LayersDest::Target3 => target3(&*mem),
            };
            let sources = [
                Some(Source::Surface(target3(&*mem))),
                None,
                Some(Source::Texture(header)),
                Some(Source::Surface(game(&*mem, ff::LAYERS_TARGET))),
            ];
            Fan { draw, dest, sources }
        }
    })
}

/// The layers' composite's fan when the weather plate returned true:
/// the plate's block and destination, target 3 at stage 0 (clamp, point)
/// and target `0x13` at stage 1 (`0x00026efd`: clamp, linear), both in
/// their pixels.
fn plate_fan(mem: &mut dyn GuestMemory, plate: ff::WeatherPlate, rect_at: u32, mxcsr: u32) -> Fan {
    let c = ff::plate_fan(mem, mxcsr, rect_at, plate).unwrap_or_else(|e| unported(cause(e)));
    let raster = if c.blue_only() { FanRaster::BlueOnly } else { FanRaster::Constant };
    let draw = FanDraw::from_fan(c.combiner, floats(c.positions), c.coordinates.map(floats), [PIXELS, PIXELS, None, None], raster)
        .unwrap_or_else(|r| refused(r));
    let dest = match c.dest {
        LayersDest::View(n) => view_surface(&*mem, n),
        LayersDest::Target3 => target3(&*mem),
    };
    let sources = [Some(Source::Surface(target3(&*mem))), Some(Source::Surface(game(&*mem, ff::LAYERS_TARGET))), None, None];
    Fan { draw, dest, sources }
}

/// The setup `setup`'s call at `esp` under the call's MXCSR and x87
/// control word, `host` the one drawing.
pub fn turn(mem: &mut dyn GuestMemory, setup: Setup, esp: u32, mxcsr: u32, x87_control: u16, host: Option<&dyn Host>) -> Turn {
    turn_with(mem, setup, esp, mxcsr, x87_control, host, &[])
}

/// [`turn`] with the headers the guest's texture cache returned so far.
pub fn turn_with(
    mem: &mut dyn GuestMemory,
    setup: Setup,
    esp: u32,
    mxcsr: u32,
    x87_control: u16,
    host: Option<&dyn Host>,
    streamed: &[(u32, u32)],
) -> Turn {
    let (fan_return, corners) = setup.frame();
    let Some(rect_at) = ff::fan_call(&*mem, esp, fan_return, corners) else {
        return Turn::Guest;
    };
    let fan = match fan(mem, setup, rect_at, mxcsr, x87_control, streamed) {
        Made::Draw(fan) => fan,
        Made::Stream(image) => return Turn::Stream(image),
        Made::NoTexture => return Turn::Return,
    };
    let host = host.unwrap_or_else(|| unported("host"));
    if let Err(why) = host.fan(mem, &fan) {
        unported(why);
    }
    Turn::Return
}

fn original(cpu: &mut runtime::CpuAbi, ctx: &mut runtime::ExecCtx, addr: u32) -> Result<runtime::TransferAbi, runtime::GuestFault> {
    if let Some(original) = ctx.runtime().resolve_original(addr) {
        return Ok(original.invoke(cpu, ctx, addr));
    }
    let event = ctx.emit(runtime::Event::UnsupportedTarget { pc: cpu.eip, target: addr, kind: runtime::abi::JUMP, route: runtime::abi::ENTRY });
    Ok(runtime::TransferAbi::fault(event))
}

fn setup_call(cpu: &mut runtime::CpuAbi, ctx: &mut runtime::ExecCtx, addr: u32) -> Result<runtime::TransferAbi, runtime::GuestFault> {
    let Some(setup) = Setup::of(addr) else {
        return original(cpu, ctx, addr);
    };
    let rt = Arc::clone(ctx.runtime());
    let turn = {
        let mut mem = rt.memory();
        turn(&mut *mem, setup, cpu.esp, cpu.mxcsr, cpu.x87.control, host().as_deref())
    };
    match turn {
        Turn::Guest => original(cpu, ctx, addr),
        Turn::Return => {
            cpu.eax &= !0xff;
            rt.memory().ret(cpu, 4)
        }
        Turn::Stream(image) => stream(cpu, ctx, setup, Vec::new(), image),
    }
}

/// Asks the guest's texture cache for `image`, as `0x000143c0` does, then
/// runs the cut again with its header; another stage not resident asks
/// again.
fn stream(
    cpu: &mut runtime::CpuAbi,
    ctx: &mut runtime::ExecCtx,
    setup: Setup,
    streamed: Vec<(u32, u32)>,
    image: u32,
) -> Result<runtime::TransferAbi, runtime::GuestFault> {
    if streamed.len() >= 4 {
        unported("texture_stream");
    }
    let rt = Arc::clone(ctx.runtime());
    let (esp, mxcsr, control) = (cpu.esp, cpu.mxcsr, cpu.x87.control);
    cpu.eax = image;
    cpu.xmm[2] = [0; 16];
    let mut mem = rt.memory();
    ctx.call_guest_named(cpu, &mut mem, TEXTURE_CACHE, "native_fog_fans_texture", move |cpu, ctx| {
        if cpu.esp != esp {
            unported("texture_stack");
        }
        let mut streamed = streamed;
        streamed.push((image, cpu.eax));
        let rt = Arc::clone(ctx.runtime());
        let turn = {
            let mut mem = rt.memory();
            turn_with(&mut *mem, setup, esp, mxcsr, control, host().as_deref(), &streamed)
        };
        match turn {
            Turn::Return => {
                cpu.eax &= !0xff;
                rt.memory().ret(cpu, 4)
            }
            Turn::Stream(next) => stream(cpu, ctx, setup, streamed, next),
            Turn::Guest => unported("texture_stream"),
        }
    })
}

pub struct NativeFogFans;

impl Feature for NativeFogFans {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "The second fog's screen fans drawn by the native renderer").doc("docs/graphics/retire-d3d-fog-fans.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        // Each call is drawn from the guest's memory of that call.
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        if !on(b.switches()) {
            return Ok(());
        }
        b.raw()
            .observe_before(&engine::points::FOG_FANS, "native_fog_fans", |o| {
                before_fans(&*o.guest(), o.cpu.mxcsr, o.cpu.ecx, o.cpu.edx)
            })
            .context_only();
        for (point, name) in [
            (&engine::points::FOG_LAYERS_SETUP, "native_fog_fans_layers"),
            (&engine::points::FOG_LAYERS_COMPOSITE_SETUP, "native_fog_fans_layers_composite"),
            (&engine::points::FOG_COMPOSITE_SETUP, "native_fog_fans_composite"),
        ] {
            b.raw().implement(point, name, setup_call).context_only();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::fog_fans::{composite_global as cg, global, layers_global as lg};
    use engine::memory::FlatMemory;

    const MXCSR: u32 = 0x1f80;
    const CONTROL: u16 = 0x027f;
    const ESP: u32 = 0x0050_0000;
    const RECT: u32 = 0x0050_1000;
    const VIEW: u32 = 0x0050_2000;

    fn mem(amount: f32) -> FlatMemory {
        let mut m = FlatMemory::new(0x0040_0000, 0x0012_0000);
        m.set_u32(global::DATUM, 0xe1a3_0012).unwrap();
        m.set_f32(global::AMOUNT, amount).unwrap();
        m.set_f32(global::ZERO, 0.0).unwrap();
        m
    }

    /// A setup call of `setup` from the fan of `0x00025dc0` over 640x480
    /// into target 0, with a fog patch whose bitmap is `0xabcd0001`.
    fn setup_mem(setup: Setup) -> FlatMemory {
        let mut m = mem(0.5);
        let f = |m: &mut FlatMemory, a: u32, v: f32| m.set_f32(a, v).unwrap();
        f(&mut m, engine::screen_grid::constant::ONE, 1.0);
        f(&mut m, engine::screen_grid::constant::DEPTH, 16_777_215.0);
        f(&mut m, engine::screen_grid::constant::NEAR, 0.0625);
        f(&mut m, engine::screen_grid::constant::FAR, 1024.0);
        f(&mut m, cg::SCALE, 255.0);
        f(&mut m, cg::CLIP_FAR, 1024.0);
        f(&mut m, cg::COLOUR, 1.0);
        f(&mut m, lg::TWO, 2.0);
        f(&mut m, lg::SPAN_FAR, 1024.0);
        f(&mut m, lg::SPAN_MIN, 1.0);
        for (i, v) in [0.0f32, 640.0, 0.0, 480.0].into_iter().enumerate() {
            f(&mut m, RECT + 4 * i as u32, v);
        }
        m.set_u32(lg::TAG_TABLE, 0x0050_3000).unwrap();
        m.set_u32(0x0050_3000 + (0x12 << 4) + 8, 0x0050_4000).unwrap();
        m.set_u32(0x0050_4000 + lg::BITMAP_DATUM, 0xabcd_0001).unwrap();
        m.set_u32(RASTERIZER, 0x0050_5000).unwrap();
        m.set_u32(0x0050_5000 + RASTERIZER_BITMAP, 0xdcba_0002).unwrap();
        m.set_u32(0x0050_5000 + 0x64, u32::MAX).unwrap();
        // The bitmaps' images, resident: the patch's (datum 1) sixteen,
        // the rasterizer's (datum 2) one.
        m.set_u32(native_render::effect_material::RESIDENT_STAMP, 4).unwrap();
        for (datum, data, records, count, header) in [(1u32, 0x0050_6000u32, PATCH_IMAGES, 16u32, PATCH_HEADER), (2, 0x0050_8000, RASTER_IMAGE, 1, RASTER_HEADER)] {
            m.set_u32(0x0050_3000 + (datum << 4) + 8, data).unwrap();
            m.set_u32(data + 0x44, count).unwrap();
            m.set_u32(data + 0x48, records).unwrap();
            for n in 0..count {
                let r = records + 0x74 * n;
                m.set_u32(r + 0x50, header + 0x100 * n).unwrap();
                m.set_u32(r + 0x70, 5).unwrap();
            }
        }
        m.set_u8(lg::BRANCH, 1).unwrap();
        m.set_u8(global::FLAG_DL, 1).unwrap();
        m.set_u32(crate::frame_tail::TARGET0_COLOUR, VIEW).unwrap();
        // Target 3's surface and a live table entry 0x13.
        m.set_u32(TARGET3, T3).unwrap();
        let entry = crate::native_targets::TARGETS + crate::native_targets::TARGET_STRIDE * 0x13;
        m.set_u32(entry + 0x88, 0x0051_c100).unwrap();
        m.set_u32(entry + 0x78, 1).unwrap();
        let (fan_return, corners) = setup.frame();
        m.set_u32(ESP, ff::SETUP_RETURN).unwrap();
        m.set_u32(ESP + ff::SETUP_FRAME, fan_return).unwrap();
        m.set_u32(ESP + ff::SETUP_FRAME + 4, RECT).unwrap();
        m.set_u32(ESP + ff::SETUP_FRAME + 8, corners).unwrap();
        m
    }

    const T3: u32 = 0x0051_c000;

    /// Table target `n`'s first surface header.
    fn table(n: u8) -> u32 {
        crate::native_targets::surface(TargetRole::Game(n)).unwrap()
    }

    const PATCH_IMAGES: u32 = 0x0050_a000;
    const PATCH_HEADER: u32 = 0x0051_0000;
    const RASTER_IMAGE: u32 = 0x0050_c000;
    const RASTER_HEADER: u32 = 0x0051_8000;

    #[derive(Default)]
    struct Drawn(Mutex<Vec<Fan>>, Option<&'static str>);

    impl Host for Drawn {
        fn fan(&self, _: &mut dyn GuestMemory, fan: &Fan) -> Result<(), &'static str> {
            self.0.lock().unwrap().push(fan.clone());
            self.1.map_or(Ok(()), Err)
        }
    }

    fn drawn(setup: Setup, m: &mut FlatMemory) -> Fan {
        let host = Drawn::default();
        assert_eq!(turn(m, setup, ESP, MXCSR, CONTROL, Some(&host)), Turn::Return);
        let mut fans = host.0.lock().unwrap().clone();
        assert_eq!(fans.len(), 1);
        fans.remove(0)
    }

    #[test]
    fn every_gate_and_flag_runs_to_the_setups() {
        for (cl, dl) in [(1, 1), (1, 0), (0, 1), (0, 0)] {
            before_fans(&mem(0.5), MXCSR, cl, dl);
            before_fans(&mem(0.0), MXCSR, cl, dl);
        }
    }

    #[test]
    #[should_panic(expected = "fog fans: unread")]
    fn an_unread_gate_panics() {
        let m = FlatMemory::new(0x0010_0000, 0x10);
        before_fans(&m, MXCSR, 1, 0);
    }

    #[test]
    fn the_setups_are_named_by_their_entries() {
        assert_eq!(Setup::of(0x0002_6880), Some(Setup::Layers));
        assert_eq!(Setup::of(0x0002_6e50), Some(Setup::LayersComposite));
        assert_eq!(Setup::of(0x0002_76d0), Some(Setup::Composite));
        assert_eq!(Setup::of(0x0002_6520), None);
        assert_eq!(engine::points::FOG_LAYERS_SETUP.base, ff::setup::LAYERS);
        assert_eq!(engine::points::FOG_LAYERS_COMPOSITE_SETUP.base, ff::setup::LAYERS_COMPOSITE);
        assert_eq!(engine::points::FOG_COMPOSITE_SETUP.base, ff::setup::COMPOSITE);
    }

    #[test]
    fn the_composite_is_cut_into_the_view_sampling_target_3() {
        let mut m = setup_mem(Setup::Composite);
        let fan = drawn(Setup::Composite, &mut m);
        assert_eq!(fan.dest, VIEW);
        assert_eq!(fan.sources[0], Some(Source::Surface(T3)));
        assert_eq!(crate::native_targets::role(T3, &|g| (g == TARGET3).then_some(T3)), Some(TargetRole::Slot(TARGET3)));
        assert_eq!(fan.draw.strip[3].0, [640.0, 480.0]);
        assert_eq!(fan.draw.strip[3].1[0], [640.0, 480.0, 0.0, 1.0]);
        assert_eq!(fan.draw.combiner[43], 0x00ff_0000);
        assert_eq!(fan.draw.raster, FanRaster::Constant);
        assert_eq!(m.u32(cg::DEPTH).unwrap(), 1024.0f32.to_bits());
    }

    #[test]
    fn the_layers_are_cut_into_target_0x13_from_the_patchs_bitmap() {
        let mut m = setup_mem(Setup::Layers);
        for n in 0..4u32 {
            m.set_u16(lg::LAYERS + lg::LAYER_STRIDE * n, 7 + n as u16).unwrap();
        }
        let fan = drawn(Setup::Layers, &mut m);
        assert_eq!(fan.dest, table(0x13));
        assert_eq!(crate::native_targets::role(fan.dest, &|_| None), Some(TargetRole::Game(0x13)));
        assert_eq!(fan.sources[2], Some(Source::Texture(PATCH_HEADER + 0x100 * 9)));
        assert_eq!(fan.draw.raster, FanRaster::Opaque);
        assert!(fan.draw.stages.iter().all(|s| *s == Some(FanStage { coords: Coords::Unit, wrap: true })));
        assert_eq!(fan.draw.combiner[54], 0x8421);
    }

    #[test]
    #[should_panic(expected = "fog fans: target 0x13 unmade")]
    fn an_unmade_table_target_is_named() {
        let mut m = setup_mem(Setup::Layers);
        let entry = crate::native_targets::TARGETS + crate::native_targets::TARGET_STRIDE * 0x13;
        m.set_u8(entry + 0x95, 1).unwrap();
        drawn(Setup::Layers, &mut m);
    }

    #[test]
    #[should_panic(expected = "fog fans: target 3 unmade")]
    fn an_unmade_target_3_is_named() {
        let mut m = setup_mem(Setup::Composite);
        m.set_u32(TARGET3, 0).unwrap();
        drawn(Setup::Composite, &mut m);
    }

    #[test]
    fn the_layers_composite_samples_target_3_the_rasterizer_bitmap_and_0x13() {
        let mut m = setup_mem(Setup::LayersComposite);
        let fan = drawn(Setup::LayersComposite, &mut m);
        assert_eq!(fan.dest, VIEW);
        assert_eq!(
            fan.sources,
            [
                Some(Source::Surface(T3)),
                None,
                Some(Source::Texture(RASTER_HEADER)),
                Some(Source::Surface(table(0x13))),
            ]
        );
        assert_eq!(fan.draw.raster, FanRaster::Constant);
        assert_eq!(fan.draw.stages[1], None);
        // `dl` clear: into target 3, blue alone.
        m.set_u8(global::FLAG_DL, 0).unwrap();
        let fan = drawn(Setup::LayersComposite, &mut m);
        assert_eq!(fan.dest, T3);
        assert_eq!(fan.draw.raster, FanRaster::BlueOnly);
    }

    /// [`setup_mem`] with the branch byte clear and the plate's constants:
    /// the fog from depth `near` to `far` over a scale of 1024.
    fn plate_mem(near: f32, far: f32) -> FlatMemory {
        use engine::fog_fans::plate_global as p;
        let mut m = setup_mem(Setup::LayersComposite);
        m.set_u8(lg::BRANCH, 0).unwrap();
        for (a, v) in [
            (p::ONE, 1.0f32),
            (p::SCALE, 1024.0),
            (p::CAP, 128.0),
            (p::RESCALE, 256.0),
            (p::UNRESCALE, 1.0 / 256.0),
            (p::FIXED, 65536.0),
            (p::UNFIXED, 1.0 / 65536.0),
            (p::HALF, 0.5),
            (p::LIMIT, 10.0),
            (lg::SPAN_NEAR, near),
            (lg::SPAN_FAR, far),
            (lg::TARGET_SCALE, 0.5),
        ] {
            m.set_f32(a, v).unwrap();
        }
        m
    }

    #[test]
    fn the_weather_plate_draws_target_3_and_0x13_at_its_depth() {
        let mut m = plate_mem(24.0, 1048.0);
        let fan = drawn(Setup::LayersComposite, &mut m);
        assert_eq!(fan.dest, VIEW);
        assert_eq!(fan.sources, [Some(Source::Surface(T3)), Some(Source::Surface(table(0x13))), None, None]);
        assert_eq!(fan.draw.stages, [PIXELS, PIXELS, None, None]);
        assert_eq!(fan.draw.raster, FanRaster::Constant);
        assert_eq!(fan.draw.combiner[54], 0x21);
        // Stage 1 at the lerp times the target scale (corner (1, 1)).
        assert_eq!(fan.draw.strip[3].1[1], [320.0, 240.0, 0.0, 1.0]);
        // The plate's result, not the clip far.
        assert_eq!(m.u32(cg::DEPTH).unwrap(), 24.0f32.to_bits());
        // `dl` clear: into target 3, blue alone.
        m.set_u8(global::FLAG_DL, 0).unwrap();
        let fan = drawn(Setup::LayersComposite, &mut m);
        assert_eq!(fan.dest, T3);
        assert_eq!(fan.draw.raster, FanRaster::BlueOnly);
        assert_eq!(fan.draw.combiner[43], 0);
    }

    #[test]
    fn a_refused_plate_takes_the_set_branch_with_the_scaled_lerp_at_stage_1() {
        let mut m = plate_mem(24.0, 24.0);
        let fan = drawn(Setup::LayersComposite, &mut m);
        assert_eq!(fan.sources[2], Some(Source::Texture(RASTER_HEADER)));
        assert_eq!(fan.draw.combiner[54], 0xa621);
        // The branch byte clear: attribute 2 is the scaled lerp, not the
        // dot vector.
        assert_eq!(fan.draw.strip[3].1[1], [320.0, 240.0, 0.0, 1.0]);
        assert_eq!(m.u32(cg::DEPTH).unwrap(), 1024.0f32.to_bits());
    }

    #[test]
    fn a_layer_out_of_range_binds_the_rasterizer_default() {
        let mut m = setup_mem(Setup::Layers);
        m.set_u32(0x0050_5000 + 0x64, 0xdcba_0002).unwrap();
        m.set_u16(lg::LAYERS + lg::LAYER_STRIDE, 40).unwrap();
        let fan = drawn(Setup::Layers, &mut m);
        assert_eq!(fan.sources[1], Some(Source::Texture(RASTER_HEADER)));
        assert_eq!(fan.sources[0], Some(Source::Texture(PATCH_HEADER)));
    }

    #[test]
    fn a_streaming_image_asks_the_cache_and_draws_what_it_returns() {
        let mut m = setup_mem(Setup::Layers);
        // Image 2 streamed (`+0x0f` bit 1) past its stamp.
        let record = PATCH_IMAGES + 0x74 * 2;
        m.set_u32(record + 0x70, 4).unwrap();
        m.set_u32(record + 0xc, 0x0200_0000).unwrap();
        m.set_u16(lg::LAYERS + 2 * lg::LAYER_STRIDE, 2).unwrap();
        let host = Drawn::default();
        let before = m.bytes(0x0040_0000, 0x0012_0000).unwrap();
        assert_eq!(turn(&mut m, Setup::Layers, ESP, MXCSR, CONTROL, Some(&host)), Turn::Stream(record));
        // Nothing is written or drawn before the cache is asked.
        assert_eq!(m.bytes(0x0040_0000, 0x0012_0000).unwrap(), before);
        assert!(host.0.lock().unwrap().is_empty());
        assert_eq!(turn_with(&mut m, Setup::Layers, ESP, MXCSR, CONTROL, Some(&host), &[(record, 0x0051_7000)]), Turn::Return);
        assert_eq!(host.0.lock().unwrap()[0].sources[2], Some(Source::Texture(0x0051_7000)));
        // No header yet: nothing drawn, the callback's depth write made.
        let host = Drawn::default();
        m.set_u32(cg::DEPTH, 0).unwrap();
        assert_eq!(turn_with(&mut m, Setup::Layers, ESP, MXCSR, CONTROL, Some(&host), &[(record, 0)]), Turn::Return);
        assert!(host.0.lock().unwrap().is_empty());
        assert_eq!(m.u32(cg::DEPTH).unwrap(), 1024.0f32.to_bits());
    }

    #[test]
    fn another_caller_runs_the_setup() {
        let mut m = setup_mem(Setup::Composite);
        m.set_u32(ESP + ff::SETUP_FRAME + 8, ff::setup::LAYERS_COMPOSITE_CORNERS).unwrap();
        assert_eq!(turn(&mut m, Setup::Composite, ESP, MXCSR, CONTROL, None), Turn::Guest);
        let mut m = setup_mem(Setup::Layers);
        m.set_u32(ESP + ff::SETUP_FRAME, ff::SECOND_RETURN).unwrap();
        assert_eq!(turn(&mut m, Setup::Layers, ESP, MXCSR, CONTROL, None), Turn::Guest);
    }

    #[test]
    #[should_panic(expected = "fog fans: bitmap")]
    fn a_fan_the_host_refuses_panics_with_its_cause() {
        let mut m = setup_mem(Setup::Layers);
        turn(&mut m, Setup::Layers, ESP, MXCSR, CONTROL, Some(&Drawn(Mutex::new(Vec::new()), Some("bitmap"))));
    }

    #[test]
    #[should_panic(expected = "fog fans: host")]
    fn no_host_panics() {
        let mut m = setup_mem(Setup::Composite);
        turn(&mut m, Setup::Composite, ESP, MXCSR, CONTROL, None);
    }

    #[test]
    #[should_panic(expected = "fog fans: composite into target 2")]
    fn another_view_target_panics() {
        let mut m = setup_mem(Setup::Composite);
        m.set_u16(cg::VIEW_TARGET, 2).unwrap();
        turn(&mut m, Setup::Composite, ESP, MXCSR, CONTROL, Some(&Drawn::default()));
    }
}
