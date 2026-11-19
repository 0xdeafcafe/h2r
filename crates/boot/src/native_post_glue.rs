//! The `native-post` host (docs/graphics/native-renderer.md, stage 5):
//! the colour pass of `render_screen_effects` (`0x000214f0`) as the game
//! runs it, with its quads dropped on the device and the native glow
//! drawn in place of the last.
//!
//! At the call's entry ([`feature::native_post`]) the host plans the glow
//! from its seven words ([`plan`]: `engine::colour_pass::parameters`, the
//! three combiner blocks and the blur's steps from `engine::glow`, each
//! bit-equal to the lifted original, and `native_render::glow`'s frame
//! words), then arms `d3d::Device::swallow` with one entry per quad
//! ([`targets`]): a four-vertex fan into the view's target (slot 0, bound
//! at the call), target 7 (slot 1) or target 8 (slot 2). The device drops
//! exactly those and issues the glow ([`native_render::glow::GpuGlow`])
//! after the last. A quad the plan does not have ends the swallow: it and
//! the rest are drawn by the D3D path and the glow is not; the host then
//! logs it and keeps every later call on the D3D path. Anything it cannot
//! plan (a copy after the blur, a target it does not know) is not armed.
//!
//! The glow and the A/B's capture are colour-only passes (the composite
//! binds no depth). Nothing is dropped before the glow is known to draw:
//! the device refuses to arm a drop when the pass could not be issued (no
//! GPU backend, no colour target; counted and logged once per reason), and
//! the host first observes one call with a [`Probe`] that checks, on the
//! GPU's thread, that the glow would draw into the target it gets
//! (`native_render::glow::check`). Only then are quads dropped. A glow
//! refused after that (a target it cannot sample) keeps every later call
//! on the D3D path; that frame's glow is missing.
//!
//! The scenario's screen effects (`0x0002d140`, one call per entry that
//! draws): each call is planned (`engine::screen_effect::plan`, bit-equal
//! to the lifted original) and refused per reason ([`Missing`]) until
//! every piece it needs draws natively: the blur and `0x00021360`'s
//! convolutions through the glow machinery (`engine::screen_grid` for the
//! latter's grids), a last pass into target 1, and stand-ins for targets
//! `0x19` to `0x1b`. The layer-15 template pass is missing only for a
//! pass whose program did not load with the world
//! (`native_post_load`), that samples an engine texture the native pass
//! does not supply ([`pass_drawable`]), or in a call that reads target 1
//! before drawing it ([`call_ordered`]); it draws through
//! `native_render::screen_gpu`, into the bound target or a stand-in for
//! target 1 ([`dest`]). A call with nothing missing is armed as the glow
//! is: one swallow of its strips (every strip a triangle strip of twice
//! the columns, slot 0 the bound target and 1 target 1), a [`ScreenProbe`] first
//! (observed, nothing dropped), then the [`ScreenPass`] in place of the
//! strips; a swallow that ends early keeps every later call on the D3D
//! path. The stand-ins are missing only for a call with a drawn
//! convolution (the end of the call draws nothing: [`missing`]), so a
//! call of passes into the bound target with nothing convolved
//! (`desaturate`'s shape) is armed with the switch on; each reason the
//! rest are refused for is counted and logged once.
//!
//! The frame A/B, behind `H2_POST_AB=<dir>` ([`Ab`]): the view's colour
//! where the glow lands (after the composite, before the screen effects,
//! the fade and the HUD), from each call `H2_POST_AB_AT` names (default
//! 600; a list `601,1500,2700`, or a start and a stride `600+500` for
//! [`MAX_WINDOWS`] windows spread over the run) for `H2_POST_AB_COUNT`
//! calls each (default 1), into
//! `<dir>/<native|d3d>-<call>-<w>x<h>.png` (`.raw` for a format other than
//! 8-bit RGBA or BGRA), one per copy of the target under the render scale.
//! With the switch off the hook is placed for it and the host observes
//! the same quads, dropping none, so both sides are captured at the same
//! draw.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use engine::colour_pass;
use engine::glow::{self as game_glow, BlurArgs};
use engine::memory::GuestMemory;
use engine::screen_effect::{self, Convolution, Effect};
use feature::native_post::{Fp, Host, ScreenEffect};
use gfx::vertex::PrimitiveType;
use map::TagId;
use native_render::camera::GuestRead;
use native_render::glow::{self, Blend, Draw, GlowTarget, GpuGlow, Plan, Surface};
use native_render::screen_gpu::{
    self, ConvolutionDraw, Dest, QuadFrame, ScreenConvolution, ScreenDraw, ScreenKey, ScreenSet, ScreenTarget, StripVertex,
};
use native_render::screen_look::ScreenStage;

use crate::native_lights_glue::DeviceMemory;
use crate::native_post_load::ScreenEffects;
use crate::native_world_glue::{Load, State};

struct Ro<'a>(&'a dyn GuestMemory);

impl GuestRead for Ro<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }
}

fn err<E: std::fmt::Debug>(what: &'static str) -> impl Fn(E) -> String {
    move |e| format!("{what}: {e:?}")
}

/// The glow `0x000214f0` draws for `args`, or `None` when it draws
/// nothing; `Err` when it draws something this cannot plan.
pub fn plan(mem: &dyn GuestMemory, args: [u32; 7], fp: Fp) -> Result<Option<Plan>, String> {
    let p = colour_pass::parameters(mem, args, fp.mxcsr).map_err(err("parameters"))?;
    if !p.draws {
        return Ok(None);
    }
    let mut plan = glow::read_frame(&Ro(mem)).ok_or("the window, signs or target sizes")?;
    let pass_a = game_glow::pass_a(mem, &p, fp.x87_control).map_err(err("pass_a"))?;
    let blur_combiner =
        game_glow::blur_combiner(mem, p.args[4], p.y, fp.mxcsr, fp.x87_control).map_err(err("blur_combiner"))?;
    let blur = game_glow::blur(mem, &BlurArgs::of(&p), fp.mxcsr).map_err(err("blur"))?;
    if blur.copy.is_some() {
        return Err("the blur copies its result (0x000351a0)".into());
    }
    let pass_b = game_glow::pass_b(mem, &p, fp.x87_control).map_err(err("pass_b"))?;
    let view = mem.u32(glow::VIEW_TARGET).map_err(err("the view's target"))? as u16 as i16 as i32 as u32;
    let surface = |t: u32| Surface::of_target(t, view).ok_or(format!("target {t} in the blur"));
    // 0x000214f0: the view at stages 0..3, four taps at 0.5 into 7.
    let mut stages = [Some(Surface::View); 4];
    plan.draws.push(Draw {
        dest: Surface::Down,
        stages,
        combiner: pass_a.0,
        taps: 4,
        spread: 0.5,
        blend: None,
    });
    // 0x000211a0: each step's source at stages 0..3.
    for d in &blur.draws {
        stages = [Some(surface(d.source)?); 4];
        plan.draws.push(Draw {
            dest: surface(d.dest)?,
            stages,
            combiner: blur_combiner.0,
            taps: 4,
            spread: f32::from_bits(d.spread),
            blend: None,
        });
    }
    // The composite: the result at stage 0, one tap, 0x000222d0's blend.
    stages[0] = Some(surface(blur.result)?);
    let source = if p.args[5] & 0xff != 0 { 0x307 } else { 1 };
    let dest = u32::from(p.args[6] & 0xff == 0);
    plan.draws.push(Draw {
        dest: Surface::View,
        stages,
        combiner: pass_b.0,
        taps: 1,
        spread: 0.0,
        blend: Blend::of_states(source, dest),
    });
    Ok(Some(plan))
}

/// A piece a screen effect needs that does not draw natively yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Missing {
    /// A layer-15 template pass (`0x00016610`) whose program did not load
    /// or that binds an engine texture the native pass does not supply
    /// ([`pass_drawable`]).
    TemplatePass,
    /// A pass into target 1 (at the window's size, a half or a quarter)
    /// that is the call's last, or has no size: the native pass is issued
    /// into the target bound at the last strip, which must be the call's.
    PassTarget,
    /// A blur convolution (`0x000211a0`) without its quads' frame
    /// ([`quad_frame`]).
    Blur,
    /// A wide convolution (`0x00021360`, `engine::screen_grid`).
    Wide,
    /// Stand-ins for targets `0x19` to `0x1b`, which a drawn convolution
    /// renders into (and a later pass samples through engine textures 24
    /// and 25): the blur's are `screen_gpu`'s, so only a wide one, or a
    /// blur without its frame, misses them.
    StandIns,
}

impl Missing {
    pub const ALL: [Missing; 5] = [
        Missing::TemplatePass,
        Missing::PassTarget,
        Missing::Blur,
        Missing::Wide,
        Missing::StandIns,
    ];
}

/// Whether the native screen pass draws `p` of egor `egor`: its program
/// loaded (`p.implementation` the layer pass, `p.selector` its
/// implementation), every stage it samples bound with what the pass
/// supplies, and strips of at least a triangle each.
pub fn pass_drawable(effects: Option<&ScreenEffects>, egor: TagId, p: &screen_effect::Pass) -> bool {
    let Ok(implementation) = usize::try_from(p.selector) else { return false };
    let Some(program) = effects.and_then(|e| e.program(egor, p.implementation as usize, implementation)) else {
        return false;
    };
    let bound = (0..4).all(|n| {
        program.dims[n].is_none()
            || match program.stages[n] {
                ScreenStage::Empty => false,
                ScreenStage::Bitmap { bitmap, image } => effects.is_some_and(|e| e.images.contains_key(&(bitmap, image))),
                ScreenStage::Engine(id) => screen_gpu::supplied(id),
            }
    });
    bound && !p.strips.is_empty() && p.strips.iter().all(|s| s.len() >= 3)
}

/// Where pass `p` draws: `kind` 0 the bound target, else target 1 at the
/// size the plan gives it.
pub fn dest(p: &screen_effect::Pass) -> Option<Dest> {
    match (p.kind, p.size) {
        (0, _) => Some(Dest::Bound),
        (_, Some((w, h))) => Some(Dest::Target1 { guest: (u32::try_from(w).ok()?, u32::try_from(h).ok()?) }),
        (_, None) => None,
    }
}

/// A convolution as the native pass orders and draws it: one kept its
/// result alone, a blur its quads (`engine::glow::BlurDraw`), a wide one
/// its draws (`engine::glow::WideDraw`; for the order only, as
/// [`Missing::Wide`] keeps it on D3D). `None` for one that copies its
/// result (`0x000351a0`; `0x0002d140` passes -1, so never) or at a
/// negative size.
pub fn screen_convolution(c: &Convolution) -> Option<ScreenConvolution> {
    let size = |(w, h): (i32, i32)| Some((u32::try_from(w).ok()?, u32::try_from(h).ok()?));
    let quad = |source, dest, spread| ConvolutionDraw { source, dest, spread };
    match c {
        Convolution::Kept { result } => Some(ScreenConvolution { size: (0, 0), draws: Vec::new(), result: *result }),
        Convolution::Blur { size: s, blur, .. } if blur.copy.is_none() => Some(ScreenConvolution {
            size: size(*s)?,
            draws: blur.draws.iter().map(|d| quad(d.source, d.dest, f32::from_bits(d.spread))).collect(),
            result: blur.result,
        }),
        Convolution::Wide { size: s, wide, .. } if wide.copy.is_none() => Some(ScreenConvolution {
            size: size(*s)?,
            draws: wide.draws.iter().map(|d| quad(d.source, d.dest, 0.0)).collect(),
            result: wide.result,
        }),
        Convolution::Blur { .. } | Convolution::Wide { .. } => None,
    }
}

/// `p`'s convolutions ([`screen_convolution`]), `None` if one is not
/// drawable.
pub fn screen_convolutions(p: &screen_effect::Pass) -> Option<Vec<ScreenConvolution>> {
    p.convolutions.iter().map(screen_convolution).collect()
}

/// Whether `effect`'s passes and convolutions read only what the call
/// drew before (`screen_gpu::check_order`: target 1, the stand-ins and
/// the results), each with its program loaded.
pub fn call_ordered(effects: Option<&ScreenEffects>, egor: TagId, effect: &Effect) -> bool {
    let mut passes = Vec::new();
    for p in &effect.passes {
        let program = usize::try_from(p.selector)
            .ok()
            .and_then(|i| effects?.program(egor, p.implementation as usize, i));
        match (program, dest(p), screen_convolutions(p)) {
            (Some(program), Some(d), Some(c)) => passes.push((program, d, c)),
            _ => return false,
        }
    }
    screen_gpu::check_order(passes.iter().map(|(p, d, c)| (*p, *d, &c[..]))).is_ok()
}

/// The frame of a call's convolution quads (`screen_gpu::QuadFrame`):
/// the window and the tap signs (`glow::read_window`) and the blur's
/// combiner as `0x0002d140` has `0x000211a0` make it (its seventh word 1:
/// `0x00020f30` with its ninth and tenth, strength and `y`, both 0).
pub fn quad_frame(mem: &dyn GuestMemory, fp: Fp) -> Option<QuadFrame> {
    let (window, signs) = glow::read_window(&Ro(mem))?;
    let combiner = game_glow::blur_combiner(mem, 0, 0, fp.mxcsr, fp.x87_control).ok()?.0;
    Some(QuadFrame { window, signs, combiner })
}

/// What `effect` needs that is not native yet, once each, in order;
/// `drawable` says whether the native template pass draws a pass
/// ([`pass_drawable`]), `blurs` whether the blur's quads can be
/// ([`quad_frame`] read).
pub fn missing(effect: &Effect, drawable: &dyn Fn(&screen_effect::Pass) -> bool, blurs: bool) -> Vec<Missing> {
    let mut m = Vec::new();
    let last = effect.passes.len().saturating_sub(1);
    for (i, p) in effect.passes.iter().enumerate() {
        if !drawable(p) {
            m.push(Missing::TemplatePass);
        }
        let sized = matches!(dest(p), Some(Dest::Target1 { guest: (w, h) }) if w > 0 && h > 0);
        if p.kind != 0 && (i == last || !sized) {
            m.push(Missing::PassTarget);
        }
        for c in &p.convolutions {
            match c {
                Convolution::Kept { .. } => {}
                Convolution::Blur { .. } if blurs => {}
                Convolution::Blur { .. } => m.extend([Missing::Blur, Missing::StandIns]),
                Convolution::Wide { .. } => m.extend([Missing::Wide, Missing::StandIns]),
            }
        }
    }
    // The end of the call (`0x0002dae8..0x0002db64`, Code) draws nothing:
    // `0x0001ddd0` writes the sizes of targets 1, 0x19 and 0x1b back (the
    // table at `0x004b4b58`: `+0x80`/`+0x84` and the surface words
    // `+0x14`, `+0x28`), from those `0x0002d26e..0x0002d344` saved on
    // entry (0 for a target without a surface); 0x19 is written twice,
    // the second time with 0x1a's saved size, and 0x1a never, as the
    // original does. `0x00016b10` applies the
    // last pass's restore states (`0x00018900`) and clears the draw-state
    // cache at `0x00485b48`, `[0x004670bc]` is set and the results
    // `0x00485a78`/`7c` cleared. The game runs all of it on both paths, so
    // an effect whose convolutions are all kept needs no stand-ins.
    m.sort();
    m.dedup();
    m
}

/// The most corners a call's plan reads.
pub const MAX_CORNERS: u32 = 64;

/// The plan of the call `c` and its values: its values and corners read,
/// then `engine::screen_effect::plan`.
pub fn plan_effect(mem: &dyn GuestMemory, c: ScreenEffect, fp: Fp) -> Result<Option<(Effect, [u32; 4])>, String> {
    let n = c.columns.checked_mul(c.rows).filter(|&n| n <= MAX_CORNERS).ok_or("too many corners")?;
    let word = |a: u32| mem.u32(a).map_err(err("a value or corner"));
    let mut values = [0u32; 4];
    for (i, v) in values.iter_mut().enumerate() {
        *v = word(c.values.wrapping_add(4 * i as u32))?;
    }
    let mut corners = Vec::with_capacity(n as usize);
    for i in 0..n {
        let at = c.corners.wrapping_add(16 * i);
        corners.push([word(at)?, word(at + 4)?, word(at + 8)?, word(at + 12)?]);
    }
    let effect = screen_effect::plan(mem, c.datum, values, c.columns, c.rows, &corners, fp.mxcsr, fp.x87_control)
        .map_err(err("plan"))?;
    Ok(effect.map(|e| (e, values)))
}

static EFFECT_REFUSED: [AtomicU64; Missing::ALL.len()] = [const { AtomicU64::new(0) }; Missing::ALL.len()];
static EFFECT_LOGGED: [AtomicBool; Missing::ALL.len()] = [const { AtomicBool::new(false) }; Missing::ALL.len()];
static EFFECT_PLAN_LOGGED: AtomicBool = AtomicBool::new(false);

/// The calls refused for each reason so far, in [`Missing::ALL`]'s order.
pub fn effect_refusals() -> [u64; Missing::ALL.len()] {
    std::array::from_fn(|i| EFFECT_REFUSED[i].load(Ordering::Relaxed))
}

/// A call as the native pass draws it: per pass its key, destination,
/// strips (`depth` the positions' z and w, `[0x0045dd40]`) and
/// convolutions; the call's values (what its passes' engine constants
/// read) and its quads' frame.
pub struct ScreenCall {
    pub egor: TagId,
    pub passes: Vec<(ScreenKey, Dest, Vec<Vec<StripVertex>>, Vec<ScreenConvolution>)>,
    pub values: [u32; 4],
    pub frame: Option<QuadFrame>,
}

/// `effect`'s passes for egor `egor` (each drawable: [`pass_drawable`],
/// its convolutions [`screen_convolutions`]), called with `values`.
pub fn screen_call(effect: &Effect, egor: TagId, values: [u32; 4], depth: u32, frame: Option<QuadFrame>) -> ScreenCall {
    let passes = effect
        .passes
        .iter()
        .map(|p| {
            let key = (egor.0, p.implementation as usize, p.selector.max(0) as usize);
            let strips = p
                .strips
                .iter()
                .map(|s| {
                    let words: Vec<_> = s.iter().map(|v| (v.texcoords, v.position)).collect();
                    screen_gpu::strip(&words, depth)
                })
                .collect();
            (key, dest(p).unwrap_or(Dest::Bound), strips, screen_convolutions(p).unwrap_or_default())
        })
        .collect();
    ScreenCall { egor, passes, values, frame }
}

/// The swallow of a call's draws in order: per pass its strips, each a
/// triangle strip (`SET_BEGIN_END` 6) of twice the columns, then its
/// convolutions' quads ([`QUAD`]); each with its slot, 0 the bound
/// target, 1 target 1, `2 + n` stand-in `n` (`screen_gpu::STAND_INS`).
pub fn screen_swallow(call: &ScreenCall) -> Option<(Vec<u8>, Vec<(PrimitiveType, usize)>)> {
    let mut draws = Vec::new();
    for (_, d, strips, convolutions) in &call.passes {
        for s in strips {
            draws.push((u8::from(*d != Dest::Bound), (PrimitiveType::TriangleStrip, s.len())));
        }
        for q in convolutions.iter().flat_map(|c| &c.draws) {
            draws.push((2 + screen_gpu::stand_in(q.dest)? as u8, QUAD));
        }
    }
    if draws.is_empty() {
        return None;
    }
    Some(draws.into_iter().unzip())
}

/// The native screen effect, in place of a call's strips.
struct ScreenPass {
    effects: Arc<ScreenEffects>,
    call: ScreenCall,
    screens: Arc<Mutex<(usize, ScreenSet)>>,
}

impl d3d::ExternalPass for ScreenPass {
    fn colour_only(&self) -> bool {
        true
    }

    fn encode(&mut self, mut t: d3d::ExternalTarget<'_>) {
        let mut screens = self.screens.lock().unwrap_or_else(|e| e.into_inner());
        // A new map's effects: the passes made for the last are stale.
        let epoch = Arc::as_ptr(&self.effects) as usize;
        if screens.0 != epoch {
            screens.1.clear();
            screens.0 = epoch;
        }
        let mut draws = Vec::new();
        for (key, dest, strips, convolutions) in &self.call.passes {
            let Some(program) = self.effects.program(self.call.egor, key.1, key.2) else {
                screen_refused(self.call.egor, &screen_gpu::ScreenRefused::NoProgram);
                return;
            };
            let values = self.call.values;
            draws.push(ScreenDraw { key: *key, program, strips, dest: *dest, values, convolutions });
        }
        let images = &self.effects.images;
        let mut stage_image = |s: ScreenStage| match s {
            ScreenStage::Bitmap { bitmap, image } => images.get(&(bitmap, image)).cloned(),
            _ => None,
        };
        let target = ScreenTarget {
            device: t.device,
            queue: t.queue,
            encoder: &mut *t.encoder,
            color: t.color,
            color_format: t.color_format,
            guest_size: t.guest_size,
            size: t.size,
            z_scale: t.z_scale,
        };
        if let Err(r) = screens.1.encode(target, &draws, self.call.frame.as_ref(), &mut stage_image) {
            screen_refused(self.call.egor, &r);
        }
    }
}

/// Screen effect `egor` not drawn after its strips were dropped: that
/// frame's is missing, and its later calls keep the D3D path.
fn screen_refused(egor: TagId, r: &screen_gpu::ScreenRefused) {
    if !SCREEN_REFUSAL_LOGGED.swap(true, Ordering::Relaxed) {
        eprintln!(
            "native-post: the screen effect {:#010x} was not drawn: {r:?}; this frame's is missing, its later calls keep the D3D path (logged once)",
            egor.0
        );
    }
    fall_back(egor);
}

/// Before any strip is dropped: whether each pass of the call would draw
/// into the target its strips went to (all drawn by D3D).
struct ScreenProbe {
    effects: Arc<ScreenEffects>,
    call: ScreenCall,
}

impl d3d::ExternalPass for ScreenProbe {
    fn colour_only(&self) -> bool {
        true
    }

    fn encode(&mut self, t: d3d::ExternalTarget<'_>) {
        let mut ordered = Vec::new();
        let checked = self.call.passes.iter().try_for_each(|(key, dest, _, convolutions)| {
            let program = self.effects.program(self.call.egor, key.1, key.2).ok_or(screen_gpu::ScreenRefused::NoProgram)?;
            ordered.push((program, *dest, &convolutions[..]));
            screen_gpu::check(t.color, program)
        });
        let checked = checked
            .and_then(|()| screen_gpu::check_order(ordered.iter().copied()))
            .and_then(|()| {
                let frame = self.call.frame.as_ref();
                screen_gpu::check_convolutions(t.color, frame, ordered.iter().map(|(p, _, c)| (*p, *c)))
            });
        match checked {
            Ok(()) => {
                if prove(self.call.egor) {
                    eprintln!(
                        "native-post: the screen effect {:#010x} can draw into the bound target; its strips are dropped from its next call",
                        self.call.egor.0
                    );
                }
            }
            Err(r) => {
                if fall_back(self.call.egor) {
                    eprintln!(
                        "native-post: the screen effect {:#010x} cannot draw into the bound target: {r:?}; it keeps its D3D strips",
                        self.call.egor.0
                    );
                }
            }
        }
    }
}

/// A set of egors, noted once each.
pub struct Egors(Mutex<Vec<TagId>>);

impl Egors {
    pub const fn new() -> Self {
        Egors(Mutex::new(Vec::new()))
    }

    pub fn contains(&self, egor: TagId) -> bool {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).contains(&egor)
    }

    /// Notes `egor`; whether it was not noted before.
    pub fn insert(&self, egor: TagId) -> bool {
        let mut v = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let new = !v.contains(&egor);
        if new {
            v.push(egor);
        }
        new
    }
}

/// The egors whose swallow ended early or whose pass cannot draw: their
/// later calls keep the D3D path, each on its own (a refused blur leaves
/// desaturate armed).
static SCREEN_FELL_BACK: Egors = Egors::new();
/// The egors a [`ScreenProbe`] found draw into the bound target, each
/// probed on its own (one effect's passes prove nothing of another's).
static SCREEN_PROVED: Egors = Egors::new();
/// The egor of the last screen swallow armed (its datum), what a
/// mismatch after it is charged to.
static LAST_SCREEN: AtomicU32 = AtomicU32::new(u32::MAX);

/// Whether `egor` was proved.
fn proved(egor: TagId) -> bool {
    SCREEN_PROVED.contains(egor)
}

/// Notes `egor` proved; whether it was not before.
fn prove(egor: TagId) -> bool {
    SCREEN_PROVED.insert(egor)
}

/// Whether `egor` keeps the D3D path.
fn fell_back(egor: TagId) -> bool {
    SCREEN_FELL_BACK.contains(egor)
}

/// `egor`'s later calls keep the D3D path; whether they did not before.
fn fall_back(egor: TagId) -> bool {
    SCREEN_FELL_BACK.insert(egor)
}
static SCREEN_REFUSAL_LOGGED: AtomicBool = AtomicBool::new(false);
/// Which hook armed the last swallow (a mismatch is that one's).
static LAST_SWALLOW: AtomicU8 = AtomicU8::new(NONE_ARMED);
const NONE_ARMED: u8 = 0;
const GLOW_ARMED: u8 = 1;
const SCREEN_ARMED: u8 = 2;

/// The swallow's slot per quad: the view 0, target 7 1, target 8 2.
pub fn targets(plan: &Plan) -> Vec<u8> {
    plan.draws
        .iter()
        .map(|d| match d.dest {
            Surface::View => 0,
            Surface::Down => 1,
            Surface::Other => 2,
        })
        .collect()
}

/// A quad of `0x00034420`: `D3DDevice_Begin(7)`, four vertices.
pub const QUAD: (PrimitiveType, usize) = (PrimitiveType::TriangleFan, 4);

/// `H2_POST_AB`: where and which calls the A/B captures.
#[derive(Debug, PartialEq, Eq)]
pub struct Ab {
    dir: PathBuf,
    /// The first call of each window, ascending.
    starts: Vec<u64>,
    count: u64,
}

/// Windows a stride (`H2_POST_AB_AT=<start>+<stride>`) makes.
pub const MAX_WINDOWS: u64 = 10;

/// `H2_POST_AB_AT`: one call, a list (`601,1500,2700`), or a start and a
/// stride (`600+500`: [`MAX_WINDOWS`] starts). `None` when it does not
/// parse.
fn starts(at: &str) -> Option<Vec<u64>> {
    let at = at.trim();
    let mut v = if let Some((start, stride)) = at.split_once('+') {
        let (start, stride) = (start.trim().parse::<u64>().ok()?, stride.trim().parse::<u64>().ok()?);
        if stride == 0 {
            return None;
        }
        (0..MAX_WINDOWS).map(|i| start + i * stride).collect()
    } else {
        at.split(',').map(|n| n.trim().parse::<u64>().ok()).collect::<Option<Vec<_>>>()?
    };
    v.sort_unstable();
    v.dedup();
    (!v.is_empty()).then_some(v)
}

/// Calls after the last capture the host still arms for, so the copies
/// already made are mapped and written (each only once the frame that
/// made it was submitted: the next pass's encode, on the GPU's thread).
const AB_TAIL: u64 = 8;

impl Ab {
    fn from_env() -> Option<Ab> {
        let dir = PathBuf::from(std::env::var_os("H2_POST_AB")?);
        let num = |k: &str, d: u64| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
        let at = std::env::var("H2_POST_AB_AT").unwrap_or_else(|_| "600".into());
        let starts = starts(&at).unwrap_or_else(|| {
            eprintln!("native-post A/B: H2_POST_AB_AT={at:?} is not a call, a list or start+stride; using 600");
            vec![600]
        });
        Some(Ab {
            dir,
            starts,
            count: num("H2_POST_AB_COUNT", 1).max(1),
        })
    }

    fn get() -> Option<&'static Ab> {
        static AB: OnceLock<Option<Ab>> = OnceLock::new();
        AB.get_or_init(Ab::from_env).as_ref()
    }

    /// Whether `call` is in a window `extra` calls longer than asked.
    fn within(&self, call: u64, extra: u64) -> bool {
        self.starts.iter().any(|&a| (a..a + self.count + extra).contains(&call))
    }

    fn captures(&self, call: u64) -> bool {
        self.within(call, 0)
    }

    fn pumps(&self, call: u64) -> bool {
        self.within(call, AB_TAIL)
    }
}

/// Whether the A/B is asked for (boot places the hook for it).
pub fn ab_requested() -> bool {
    Ab::get().is_some()
}

const PENDING: u8 = 0;
const READY: u8 = 1;
const FAILED: u8 = 2;

/// A copy of the view's colour on its way back.
struct Readback {
    buffer: wgpu::Buffer,
    state: Arc<AtomicU8>,
    mapping: bool,
    path: PathBuf,
    size: (u32, u32),
    padded: u32,
    format: wgpu::TextureFormat,
}

static READBACKS: Mutex<Vec<Readback>> = Mutex::new(Vec::new());

/// Copies the bound colour into a buffer, to be written by a later
/// [`pump`].
fn capture(t: &mut d3d::ExternalTarget<'_>, stem: &str) {
    let Some(ab) = Ab::get() else { return };
    let texture = t.color.texture();
    let Some(bpp) = t.color_format.block_copy_size(None) else { return };
    let (w, h) = (texture.width(), texture.height());
    let padded = (w * bpp).div_ceil(256) * 256;
    let buffer = t.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("native post A/B"),
        size: u64::from(padded) * u64::from(h),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    t.encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded), rows_per_image: Some(h) },
        },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    READBACKS.lock().unwrap_or_else(|e| e.into_inner()).push(Readback {
        buffer,
        state: Arc::new(AtomicU8::new(PENDING)),
        mapping: false,
        path: ab.dir.join(format!("{stem}-{w}x{h}")),
        size: (w, h),
        padded,
        format: t.color_format,
    });
}

/// Maps the copies made before this pass (their frames were submitted:
/// passes encode in order on the GPU's thread) and writes the ones
/// mapped. Polls the device for the mappings; never waits.
fn pump(device: &wgpu::Device) {
    let _ = device.poll(wgpu::PollType::Poll);
    let mut list = READBACKS.lock().unwrap_or_else(|e| e.into_inner());
    list.retain_mut(|r| {
        if !r.mapping {
            r.mapping = true;
            let state = Arc::clone(&r.state);
            r.buffer.slice(..).map_async(wgpu::MapMode::Read, move |res| {
                state.store(if res.is_ok() { READY } else { FAILED }, Ordering::Release);
            });
            return true;
        }
        match r.state.load(Ordering::Acquire) {
            READY => {
                if let Err(e) = write(r) {
                    eprintln!("native-post A/B: {}: {e}", r.path.display());
                }
                r.buffer.unmap();
                false
            }
            FAILED => false,
            _ => true,
        }
    });
}

fn write(r: &Readback) -> Result<(), String> {
    let data = r.buffer.slice(..).get_mapped_range().map_err(|e| format!("{e:?}"))?;
    let (w, h) = r.size;
    let row = (w * r.format.block_copy_size(None).unwrap_or(4)) as usize;
    let mut px = Vec::with_capacity(row * h as usize);
    for line in data.chunks(r.padded as usize).take(h as usize) {
        px.extend_from_slice(&line[..row]);
    }
    drop(data);
    use wgpu::TextureFormat as F;
    let rgba = match r.format {
        F::Rgba8Unorm | F::Rgba8UnormSrgb => true,
        F::Bgra8Unorm | F::Bgra8UnormSrgb => {
            for p in px.chunks_exact_mut(4) {
                p.swap(0, 2);
            }
            true
        }
        _ => false,
    };
    if let Some(dir) = r.path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    if !rgba {
        let path = r.path.with_extension(format!("{:?}.raw", r.format));
        return std::fs::write(path, px).map_err(|e| e.to_string());
    }
    let path = r.path.with_extension("png");
    let file = std::fs::File::create(&path).map_err(|e| e.to_string())?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut out = enc.write_header().map_err(|e| e.to_string())?;
    out.write_image_data(&px).map_err(|e| e.to_string())?;
    eprintln!("native-post A/B: wrote {}", path.display());
    Ok(())
}

/// The native glow, in place of the pass's last quad.
struct GlowPass {
    plan: Plan,
    gpu: Arc<Mutex<GpuGlow>>,
    /// `Some(stem)`: capture after drawing.
    capture: Option<String>,
}

impl d3d::ExternalPass for GlowPass {
    fn colour_only(&self) -> bool {
        true
    }

    fn encode(&mut self, mut t: d3d::ExternalTarget<'_>) {
        pump(t.device);
        let drawn = self.gpu.lock().unwrap_or_else(|e| e.into_inner()).encode(
            GlowTarget {
                device: t.device,
                queue: t.queue,
                encoder: &mut *t.encoder,
                color: t.color,
                color_format: t.color_format,
                guest_size: t.guest_size,
                size: t.size,
            },
            &self.plan,
        );
        if let Err(r) = drawn {
            GLOW_REFUSED.fetch_add(1, Ordering::Relaxed);
            if !GLOW_REFUSAL_LOGGED.swap(true, Ordering::Relaxed) {
                eprintln!("native-post: the glow was not drawn: {r:?}; this frame's glow is missing, later calls keep the D3D path (logged once)");
            }
            FELL_BACK.store(true, Ordering::Relaxed);
        }
        if let Some(stem) = &self.capture {
            capture(&mut t, stem);
        }
    }
}

/// The A/B's D3D side: a capture where the glow would land.
struct CapturePass(Option<String>);

impl d3d::ExternalPass for CapturePass {
    fn colour_only(&self) -> bool {
        true
    }

    fn encode(&mut self, mut t: d3d::ExternalTarget<'_>) {
        pump(t.device);
        if let Some(stem) = &self.0 {
            capture(&mut t, stem);
        }
    }
}

/// Before any quad is dropped: whether the glow would draw into the
/// target this call's quads went to (observed, all drawn by D3D). Proved,
/// later calls drop their quads; refused, they keep them.
struct Probe {
    plan: Plan,
    capture: Option<String>,
}

impl d3d::ExternalPass for Probe {
    fn colour_only(&self) -> bool {
        true
    }

    fn encode(&mut self, mut t: d3d::ExternalTarget<'_>) {
        pump(t.device);
        match glow::check(t.color, t.guest_size, &self.plan) {
            Ok(()) => {
                if !PROVED.swap(true, Ordering::Relaxed) {
                    eprintln!("native-post: the glow can draw into the view's target; its quads are dropped from the next call");
                }
            }
            Err(r) => {
                if !FELL_BACK.swap(true, Ordering::Relaxed) {
                    eprintln!("native-post: the glow cannot draw into the view's target: {r:?}; the colour pass keeps its D3D quads");
                }
            }
        }
        if let Some(stem) = &self.capture {
            capture(&mut t, stem);
        }
    }
}

static GLOW_REFUSED: AtomicU64 = AtomicU64::new(0);
static GLOW_REFUSAL_LOGGED: AtomicBool = AtomicBool::new(false);
static PLAN_LOGGED: AtomicBool = AtomicBool::new(false);
/// A swallow ended early, or the glow cannot draw: later calls keep the
/// D3D path.
static FELL_BACK: AtomicBool = AtomicBool::new(false);
/// A [`Probe`] found the glow draws into the view's target.
static PROVED: AtomicBool = AtomicBool::new(false);
static NO_GPU_LOGGED: AtomicBool = AtomicBool::new(false);

struct Glue<B: d3d::backend::Backend + Send + 'static> {
    device: d3d::abi::SharedDevice<B>,
    gpu: Arc<Mutex<GpuGlow>>,
    /// The world, for its screen effects.
    state: State,
    /// The screen passes on the GPU, with the address of the effects they
    /// were made for.
    screens: Arc<Mutex<(usize, ScreenSet)>>,
    calls: AtomicU64,
    mismatches: AtomicU64,
}

impl<B: d3d::backend::Backend + Send + 'static> Host for Glue<B> {
    fn colour_pass(&self, mem: &mut dyn GuestMemory, args: [u32; 7], fp: Fp, native: bool) -> bool {
        let call = self.calls.fetch_add(1, Ordering::Relaxed) + 1;
        let ab = Ab::get();
        let capturing = ab.is_some_and(|a| a.captures(call));
        let stem = |side: &str| capturing.then(|| format!("{side}-{call:06}"));
        let Ok((mismatches, gpu)) = self.device.lock().map(|d| (d.stats().swallow_mismatches, d.can_external_pass()))
        else {
            return false;
        };
        if !gpu && !NO_GPU_LOGGED.swap(true, Ordering::Relaxed) {
            eprintln!(
                "native-post: no external passes in this run (no GPU backend: headless without H2_BOOT_GFX), so the glow stays D3D's and the A/B writes nothing; H2_BOOT_GFX=<dir> H2_BOOT_GFX_EVERY=never renders on the GPU"
            );
        }
        self.note_mismatches(mismatches);
        let native = native && !FELL_BACK.load(Ordering::Relaxed);
        if !native && !ab.is_some_and(|a| a.pumps(call)) {
            return false;
        }
        let plan = match plan(&*mem, args, fp) {
            Ok(Some(p)) => p,
            Ok(None) => return false,
            Err(e) => {
                if !PLAN_LOGGED.swap(true, Ordering::Relaxed) {
                    eprintln!("native-post: the colour pass keeps its D3D quads: {e} (logged once)");
                }
                return false;
            }
        };
        let targets = targets(&plan);
        let (drop, pass) = arm(native, PROVED.load(Ordering::Relaxed));
        let pass: Box<dyn d3d::ExternalPass> = match pass {
            Pass::Glow => Box::new(GlowPass {
                plan,
                gpu: Arc::clone(&self.gpu),
                capture: stem("native"),
            }),
            Pass::Probe => Box::new(Probe {
                plan,
                capture: stem("d3d"),
            }),
            Pass::Capture => Box::new(CapturePass(stem("d3d"))),
        };
        let swallow = d3d::Swallow {
            targets,
            primitive: QUAD.0,
            vertices: QUAD.1,
            shapes: Vec::new(),
            drop,
            pass,
        };
        let Ok(mut d) = self.device.lock() else { return false };
        let armed = d.swallow(&mut DeviceMemory(mem), swallow).unwrap_or(false);
        if armed {
            LAST_SWALLOW.store(GLOW_ARMED, Ordering::Relaxed);
        }
        armed
    }

    fn screen_effect(&self, mem: &mut dyn GuestMemory, call: ScreenEffect, fp: Fp) -> bool {
        let (effect, values) = match plan_effect(&*mem, call, fp) {
            Ok(Some(e)) => e,
            Ok(None) => return false,
            Err(e) => {
                if !EFFECT_PLAN_LOGGED.swap(true, Ordering::Relaxed) {
                    eprintln!("native-post: a screen effect keeps its D3D draws: {e} (logged once)");
                }
                return false;
            }
        };
        let effects = self.screen_effects();
        let egor = TagId(call.datum);
        let ordered = call_ordered(effects.as_deref(), egor, &effect);
        let frame = quad_frame(&*mem, fp);
        let m = missing(&effect, &|p| ordered && pass_drawable(effects.as_deref(), egor, p), frame.is_some());
        for m in &m {
            let i = Missing::ALL.iter().position(|a| a == m).unwrap_or(0);
            EFFECT_REFUSED[i].fetch_add(1, Ordering::Relaxed);
            if !EFFECT_LOGGED[i].swap(true, Ordering::Relaxed) {
                eprintln!(
                    "native-post: screen effects keep their D3D draws: {m:?} does not draw natively yet (logged once per reason)"
                );
            }
        }
        // Armed only once nothing is missing (never while the stand-ins
        // are); until then the D3D path draws every effect.
        if !m.is_empty() {
            return false;
        }
        let Some(effects) = effects else { return false };
        let Ok(mismatches) = self.device.lock().map(|d| d.stats().swallow_mismatches) else { return false };
        self.note_mismatches(mismatches);
        if fell_back(egor) {
            return false;
        }
        let Ok(depth) = mem.u32(screen_effect::constant::DEPTH) else { return false };
        let call = screen_call(&effect, egor, values, depth, frame);
        let Some((targets, shapes)) = screen_swallow(&call) else { return false };
        let (primitive, vertices) = shapes[0];
        let (drop, pass): (bool, Box<dyn d3d::ExternalPass>) = if proved(egor) {
            let pass = ScreenPass { effects, call, screens: Arc::clone(&self.screens) };
            (true, Box::new(pass))
        } else {
            (false, Box::new(ScreenProbe { effects, call }))
        };
        let swallow = d3d::Swallow { targets, primitive, vertices, shapes, drop, pass };
        let Ok(mut d) = self.device.lock() else { return false };
        let armed = d.swallow(&mut DeviceMemory(mem), swallow).unwrap_or(false);
        if armed {
            LAST_SWALLOW.store(SCREEN_ARMED, Ordering::Relaxed);
            LAST_SCREEN.store(egor.0, Ordering::Relaxed);
        }
        armed
    }
}

impl<B: d3d::backend::Backend + Send + 'static> Glue<B> {
    /// The loaded world's screen effects (`None` before it loads, or with
    /// the switch off at load).
    fn screen_effects(&self) -> Option<Arc<ScreenEffects>> {
        match &self.state.lock().unwrap_or_else(|e| e.into_inner()).1 {
            Load::Ready(r) => r.screen_effects.clone(),
            Load::Idle | Load::Loading | Load::Failed => None,
        }
    }

    /// The device's swallow mismatches went up to `mismatches`: the last
    /// swallow armed ended early, so its hook's later calls keep the D3D
    /// path.
    fn note_mismatches(&self, mismatches: u64) {
        if mismatches <= self.mismatches.swap(mismatches, Ordering::Relaxed) {
            return;
        }
        if LAST_SWALLOW.load(Ordering::Relaxed) == SCREEN_ARMED {
            let egor = TagId(LAST_SCREEN.load(Ordering::Relaxed));
            if fall_back(egor) {
                eprintln!(
                    "native-post: the screen effect {:#010x} drew a strip its plan did not have; that call fell back to D3D, and its later calls keep it",
                    egor.0
                );
            }
        } else if !FELL_BACK.swap(true, Ordering::Relaxed) {
            eprintln!(
                "native-post: the colour pass drew a quad its plan did not have; that call's glow fell back to D3D, and every later call keeps it"
            );
        }
    }
}

/// The pass a call arms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pass {
    /// The native glow, its quads dropped.
    Glow,
    /// The check before any drop, its quads drawn.
    Probe,
    /// The A/B's D3D side, its quads drawn.
    Capture,
}

/// Whether a call drops its quads, and its pass: `native` (the switch,
/// no fall-back), once the glow is `proved` to draw.
fn arm(native: bool, proved: bool) -> (bool, Pass) {
    match (native, proved) {
        (true, true) => (true, Pass::Glow),
        (true, false) => (false, Pass::Probe),
        (false, _) => (false, Pass::Capture),
    }
}

/// Installs the host over `device`, the screen effects read from the
/// world `state` loads.
pub(crate) fn install<B: d3d::backend::Backend + Send + 'static>(device: &d3d::abi::SharedDevice<B>, state: State) {
    feature::native_post::set_host(Arc::new(Glue {
        device: Arc::clone(device),
        gpu: Arc::new(Mutex::new(GpuGlow::new())),
        state,
        screens: Arc::new(Mutex::new((0, ScreenSet::new()))),
        calls: AtomicU64::new(0),
        mismatches: AtomicU64::new(0),
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draw(dest: Surface) -> Draw {
        Draw {
            dest,
            stages: [None; 4],
            combiner: [0; glow::COMBINER_WORDS],
            taps: 4,
            spread: 0.0,
            blend: None,
        }
    }

    #[test]
    fn the_swallow_has_a_slot_per_quad() {
        let plan = Plan {
            window: [0.0; 4],
            signs: [0.0; 2],
            view: (640, 480),
            small: (160, 120),
            draws: [Surface::Down, Surface::Other, Surface::Down, Surface::Other, Surface::Down, Surface::View]
                .map(draw)
                .to_vec(),
        };
        assert_eq!(targets(&plan), vec![1, 2, 1, 2, 1, 0]);
        assert_eq!(QUAD, (PrimitiveType::from_xbox(7).unwrap(), 4), "0x00034420's Begin(7)");
    }

    #[test]
    fn the_ab_takes_a_call_a_list_or_a_stride() {
        assert_eq!(starts("600"), Some(vec![600]));
        assert_eq!(starts(" 2700, 601 ,1500,601"), Some(vec![601, 1500, 2700]), "sorted, once each");
        assert_eq!(starts("600+500"), Some((0..MAX_WINDOWS).map(|i| 600 + 500 * i).collect()));
        for bad in ["", "x", "600+0", "600+", "1,,2", "-5"] {
            assert_eq!(starts(bad), None, "{bad:?}");
        }
        let ab = Ab {
            dir: PathBuf::new(),
            starts: vec![601, 1500],
            count: 3,
        };
        let captured: Vec<u64> = (0..2000).filter(|&c| ab.captures(c)).collect();
        assert_eq!(captured, vec![601, 602, 603, 1500, 1501, 1502]);
        assert!(ab.pumps(603 + AB_TAIL) && !ab.pumps(604 + AB_TAIL), "each window's tail");
        assert!(!ab.pumps(600) && !ab.pumps(1499));
    }

    #[test]
    fn nothing_is_dropped_before_the_glow_is_proved() {
        assert_eq!(arm(true, false), (false, Pass::Probe), "observe and check first");
        assert_eq!(arm(true, true), (true, Pass::Glow));
        assert_eq!(arm(false, false), (false, Pass::Capture), "the A/B's D3D side");
        assert_eq!(arm(false, true), (false, Pass::Capture));
    }

    /// desaturate proved says nothing of the blur: each egor's first call
    /// is a probe.
    #[test]
    fn each_screen_effect_is_probed_on_its_own() {
        let (a, b) = (TagId(0xe0ff_0001), TagId(0xe0ff_0002));
        assert!(!proved(a));
        assert!(prove(a) && !prove(a), "noted once");
        assert!(proved(a) && !proved(b));
    }

    /// A refused or mismatched blur keeps only the blur on D3D:
    /// desaturate stays armed.
    #[test]
    fn a_fallback_is_its_effects_alone() {
        let (blur, desaturate) = (TagId(0xe0ff_0011), TagId(0xe0ff_0012));
        assert!(!fell_back(blur) && !fell_back(desaturate));
        screen_refused(blur, &screen_gpu::ScreenRefused::ConvolutionStates);
        assert!(fell_back(blur) && !fell_back(desaturate), "a refused pass");
        assert!(!fall_back(blur), "noted once");
        let mismatched = TagId(0xe0ff_0013);
        assert!(fall_back(mismatched));
        assert!(fell_back(mismatched) && !fell_back(desaturate), "a mismatch is charged to its egor");
        let set = Egors::new();
        assert!(set.insert(desaturate) && set.contains(desaturate) && !set.contains(blur));
    }

    fn pass(kind: u16, convolutions: Vec<Convolution>) -> screen_effect::Pass {
        screen_effect::Pass {
            index: 0,
            implementation: 0,
            selector: 0,
            kind,
            size: None,
            strips: Vec::new(),
            convolutions,
        }
    }

    #[test]
    fn a_screen_effect_is_refused_for_each_missing_piece() {
        let effect = |passes| Effect {
            shader: 0,
            implementations: 1,
            passes,
        };
        let no: &dyn Fn(&screen_effect::Pass) -> bool = &|_| false;
        let yes: &dyn Fn(&screen_effect::Pass) -> bool = &|_| true;
        assert_eq!(missing(&effect(vec![]), no, false), vec![], "the restores at the end draw nothing");
        let e = effect(vec![pass(0, vec![Convolution::Kept { result: 1 }]), pass(2, vec![])]);
        assert_eq!(missing(&e, no, false), vec![Missing::TemplatePass, Missing::PassTarget]);
        assert_eq!(missing(&e, yes, false), vec![Missing::PassTarget], "the template pass draws");
        // desaturate's shape: one pass into the bound target, nothing
        // convolved.
        assert_eq!(missing(&effect(vec![pass(0, vec![])]), yes, false), vec![], "armed");
        assert_eq!(missing(&effect(vec![pass(0, vec![Convolution::Kept { result: 0 }])]), yes, false), vec![], "a kept one draws nothing");
        // Target 1, then back into the bound target: the native pass is
        // issued at the last strip, into the call's target.
        let into_1 = screen_effect::Pass { size: Some((320, 240)), ..pass(2, vec![]) };
        assert_eq!(missing(&effect(vec![into_1.clone(), pass(0, vec![])]), yes, false), vec![]);
        assert_eq!(missing(&effect(vec![pass(0, vec![]), into_1.clone()]), yes, false), vec![Missing::PassTarget], "target 1 last");
        let unsized_1 = screen_effect::Pass { size: Some((0, 240)), ..into_1 };
        assert_eq!(missing(&effect(vec![unsized_1, pass(0, vec![])]), yes, false), vec![Missing::PassTarget], "no size");
        // The blur: its quads draw once their frame is read; a wide one
        // stays on D3D.
        let args = BlurArgs { source: 1, steps: 0, scale: 0, growth: 0, dest: 0x19, other: 0x1a, last: u32::MAX };
        let blur = game_glow::Blur { draws: vec![], result: 1, copy: None };
        let blurred = || effect(vec![pass(0, vec![Convolution::Blur { size: (320, 240), args, blur: blur.clone() }])]);
        assert_eq!(missing(&blurred(), yes, false), vec![Missing::Blur, Missing::StandIns], "no frame");
        assert_eq!(missing(&blurred(), yes, true), vec![], "drawn into the stand-ins");
        let wide = game_glow::Wide { draws: vec![], result: 1, copy: None };
        let wide_args = game_glow::WideArgs { source: 1, taps: 0, scale: 0, words: [0; 2], dest: 0x19, other: 0x1a, last: u32::MAX };
        let widened = effect(vec![pass(0, vec![Convolution::Wide { size: (320, 240), args: wide_args, wide }])]);
        assert_eq!(missing(&widened, yes, true), vec![Missing::Wide, Missing::StandIns]);
    }

    /// The blur's convolution as the native pass draws it: its quads'
    /// sources, stand-ins and spreads, its result; one kept names the
    /// pass's target.
    #[test]
    fn a_convolution_is_its_quads_and_its_result() {
        let args = BlurArgs { source: 1, steps: 0, scale: 0, growth: 0, dest: 0x19, other: 0x1a, last: u32::MAX };
        let draws = vec![
            game_glow::BlurDraw { source: 1, dest: 0x19, spread: 1.5f32.to_bits() },
            game_glow::BlurDraw { source: 0x19, dest: 0x1a, spread: 2.0f32.to_bits() },
        ];
        let blur = game_glow::Blur { draws, result: 0x1a, copy: None };
        let c = screen_convolution(&Convolution::Blur { size: (320, 240), args, blur: blur.clone() }).unwrap();
        assert_eq!((c.size, c.result), ((320, 240), 0x1a));
        assert_eq!(c.draws, vec![
            ConvolutionDraw { source: 1, dest: 0x19, spread: 1.5 },
            ConvolutionDraw { source: 0x19, dest: 0x1a, spread: 2.0 },
        ]);
        assert_eq!(screen_convolution(&Convolution::Kept { result: 1 }).map(|c| (c.draws.len(), c.result)), Some((0, 1)));
        let copied = game_glow::Blur { copy: Some((0x1a, 0x1b)), ..blur.clone() };
        assert_eq!(screen_convolution(&Convolution::Blur { size: (320, 240), args, blur: copied }), None, "0x000351a0");
        assert_eq!(screen_convolution(&Convolution::Blur { size: (-1, 240), args, blur }), None);
    }

    #[test]
    fn a_pass_draws_where_its_kind_says() {
        assert_eq!(dest(&pass(0, vec![])), Some(Dest::Bound));
        let half = screen_effect::Pass { size: Some((320, 240)), ..pass(2, vec![]) };
        assert_eq!(dest(&half), Some(Dest::Target1 { guest: (320, 240) }));
        assert_eq!(dest(&pass(1, vec![])), None, "target 1 without a size");
        assert_eq!(dest(&screen_effect::Pass { size: Some((-1, 2)), ..half }), None);
    }

    /// blur's order: the view into target 1, then target 1 back.
    #[test]
    fn a_call_reads_target_1_only_after_drawing_it() {
        let mut e = effects([ScreenStage::Engine(3), ScreenStage::Empty, ScreenStage::Empty, ScreenStage::Empty]);
        let copy = program([ScreenStage::Engine(5), ScreenStage::Empty, ScreenStage::Empty, ScreenStage::Empty]);
        e.effects.get_mut(&EGOR).unwrap().programs.insert((0, 0), copy);
        let back = drawn_pass(2, 4);
        let into_1 = screen_effect::Pass { implementation: 0, kind: 1, size: Some((640, 480)), ..drawn_pass(2, 4) };
        let call = |passes| Effect { shader: 0, implementations: 2, passes };
        assert!(call_ordered(Some(&e), EGOR, &call(vec![into_1.clone(), back.clone()])));
        assert!(!call_ordered(Some(&e), EGOR, &call(vec![back.clone()])), "target 1 as D3D left it");
        assert!(!call_ordered(Some(&e), EGOR, &call(vec![back, into_1])), "read before drawn");
        assert!(!call_ordered(None, EGOR, &call(vec![drawn_pass(1, 4)])), "no world");
    }

    const EGOR: TagId = TagId(0xe000_0001);
    const BITMAP: TagId = TagId(0xe000_0002);

    fn program(stages: [ScreenStage; 4]) -> native_render::screen_look::ScreenProgram {
        native_render::screen_look::ScreenProgram {
            wgsl: String::new(),
            stages,
            samplers: [None; 4],
            dims: stages.map(native_render::screen_look::stage_dim),
            signed: [[false; 4]; 4],
            vertex: Default::default(),
            pixel: Default::default(),
            inputs: 0x1f,
            constants: Default::default(),
            state: native_render::pass_state::PassState {
                blend: None,
                blend_constant: wgpu::Color::TRANSPARENT,
                color_writes: wgpu::ColorWrites::ALL,
                depth_write: false,
                depth_compare: wgpu::CompareFunction::Always,
                primitive: wgpu::PrimitiveState::default(),
                alpha_test: None,
            },
        }
    }

    /// One egor whose layer pass 1, implementation 0 binds `stages`, its
    /// bitmap decoded.
    fn effects(stages: [ScreenStage; 4]) -> crate::native_post_load::ScreenEffects {
        let mut e = crate::native_post_load::ScreenEffects::default();
        let mut programs = std::collections::HashMap::new();
        programs.insert((1, 0), program(stages));
        e.effects.insert(
            EGOR,
            crate::native_post_load::EffectPrograms {
                shader: TagId(0xe000_0003),
                programs,
                refused: Default::default(),
            },
        );
        e.images.insert((BITMAP, 0), native_render::gpu::Image::solid([0; 4]));
        e
    }

    /// A pass of layer pass 1, implementation 0, with `rows` strips of
    /// `columns` corner pairs.
    fn drawn_pass(rows: usize, columns: usize) -> screen_effect::Pass {
        let v = screen_effect::Vertex { texcoords: [[0; 4]; 4], position: [0; 2] };
        screen_effect::Pass {
            implementation: 1,
            selector: 0,
            strips: vec![vec![v; 2 * columns]; rows],
            ..pass(0, vec![])
        }
    }

    #[test]
    fn a_template_pass_draws_with_its_program_and_what_it_binds() {
        let bitmap = ScreenStage::Bitmap { bitmap: BITMAP, image: 0 };
        let view = effects([ScreenStage::Engine(5), bitmap, ScreenStage::Empty, ScreenStage::Empty]);
        let p = drawn_pass(3, 4);
        assert!(pass_drawable(Some(&view), EGOR, &p), "the view's copy and a decoded bitmap");
        assert!(!pass_drawable(None, EGOR, &p), "no world loaded");
        assert!(!pass_drawable(Some(&view), TagId(0xe000_0009), &p), "another egor");
        assert!(!pass_drawable(Some(&view), EGOR, &screen_effect::Pass { selector: 1, ..p.clone() }), "no such implementation");
        assert!(!pass_drawable(Some(&view), EGOR, &drawn_pass(0, 4)), "no strips");
        assert!(!pass_drawable(Some(&view), EGOR, &drawn_pass(2, 1)), "two-vertex strips");
        let target = effects([ScreenStage::Engine(13), ScreenStage::Empty, ScreenStage::Empty, ScreenStage::Empty]);
        assert!(!pass_drawable(Some(&target), EGOR, &p), "target 9 has no stand-in");
        let target1 = effects([ScreenStage::Engine(3), ScreenStage::Empty, ScreenStage::Empty, ScreenStage::Empty]);
        assert!(pass_drawable(Some(&target1), EGOR, &p), "target 1's stand-in (the call's order is checked apart)");
        let undecoded = effects([ScreenStage::Bitmap { bitmap: BITMAP, image: 1 }, ScreenStage::Empty, ScreenStage::Empty, ScreenStage::Empty]);
        assert!(!pass_drawable(Some(&undecoded), EGOR, &p), "a bitmap that did not decode");
    }

    #[test]
    fn a_calls_strips_are_one_swallow_into_the_bound_target() {
        let effect = Effect {
            shader: 0,
            implementations: 2,
            passes: vec![drawn_pass(3, 4), drawn_pass(3, 4)],
        };
        let values = [0.5f32.to_bits(), 0, 0, 0];
        let call = screen_call(&effect, EGOR, values, 0x4b7f_ffff, None);
        assert_eq!(call.passes.len(), 2);
        assert_eq!(call.values, values, "what the passes' engine constants read");
        assert_eq!(call.passes[0].0, (EGOR.0, 1, 0), "(egor, layer pass, implementation)");
        assert_eq!(call.passes[0].1, Dest::Bound);
        assert_eq!(call.passes[0].2[0][0].position[2].to_bits(), 0x4b7f_ffff);
        let strip = (PrimitiveType::from_xbox(6).unwrap(), 8);
        assert_eq!(screen_swallow(&call), Some((vec![0; 6], vec![strip; 6])), "SET_BEGIN_END 6");
        let uneven = Effect { passes: vec![drawn_pass(1, 4), drawn_pass(1, 3)], ..effect.clone() };
        let shapes = screen_swallow(&screen_call(&uneven, EGOR, [0; 4], 0, None)).map(|s| s.1);
        assert_eq!(shapes, Some(vec![strip, (PrimitiveType::TriangleStrip, 6)]), "each strip its own count");
        let empty = Effect { passes: vec![], ..effect.clone() };
        assert_eq!(screen_swallow(&screen_call(&empty, EGOR, [0; 4], 0, None)), None);
        // Target 1's strips go to slot 1, the target the first finds.
        let into_1 = screen_effect::Pass { kind: 1, size: Some((640, 480)), ..drawn_pass(2, 4) };
        let both = Effect { passes: vec![into_1, drawn_pass(3, 4)], ..effect };
        let call = screen_call(&both, EGOR, [0; 4], 0, None);
        assert_eq!(call.passes[0].1, Dest::Target1 { guest: (640, 480) });
        assert_eq!(screen_swallow(&call).map(|s| s.0), Some(vec![1, 1, 0, 0, 0]));
        // A blur after the first pass: its quads into 0x19 (slot 2) and
        // 0x1a (slot 3), between the passes' strips.
        let args = BlurArgs { source: 1, steps: 0, scale: 0, growth: 0, dest: 0x19, other: 0x1a, last: u32::MAX };
        let draws = vec![
            game_glow::BlurDraw { source: 1, dest: 0x19, spread: 0 },
            game_glow::BlurDraw { source: 0x19, dest: 0x1a, spread: 0 },
        ];
        let blur = game_glow::Blur { draws, result: 0x1a, copy: None };
        let into_1 = screen_effect::Pass {
            convolutions: vec![Convolution::Blur { size: (320, 240), args, blur }],
            ..both.passes[0].clone()
        };
        let blurred = Effect { passes: vec![into_1, both.passes[1].clone()], ..both };
        let (slots, shapes) = screen_swallow(&screen_call(&blurred, EGOR, [0; 4], 0, None)).unwrap();
        assert_eq!(slots, vec![1, 1, 2, 3, 0, 0, 0]);
        assert_eq!(shapes[2..4], [QUAD, QUAD]);
    }

    #[test]
    fn a_screen_effects_plan_reads_its_words() {
        use engine::memory::FlatMemory;
        let m = FlatMemory::new(0x0060_0000, 0x1000);
        let fp = Fp { mxcsr: 0x1f80, x87_control: 0x027f };
        let call = |columns, rows| ScreenEffect {
            datum: 0xe000_0001,
            values: 0x0060_0000,
            columns,
            rows,
            corners: 0x0060_0100,
        };
        assert_eq!(plan_effect(&m, call(9, 9), fp), Err("too many corners".to_string()));
        assert!(plan_effect(&m, call(2, 2), fp).is_err(), "the window is not in this memory");
        let unreadable = ScreenEffect { values: 0x0010_0000, ..call(2, 2) };
        assert!(plan_effect(&m, unreadable, fp).unwrap_err().starts_with("a value or corner"));
    }

    #[test]
    fn every_pass_is_colour_only() {
        use d3d::ExternalPass;
        let plan = Plan {
            window: [0.0; 4],
            signs: [0.0; 2],
            view: (640, 480),
            small: (160, 120),
            draws: vec![],
        };
        let glow = GlowPass {
            plan: plan.clone(),
            gpu: Arc::new(Mutex::new(GpuGlow::new())),
            capture: None,
        };
        assert!(glow.colour_only(), "the composite binds no depth");
        assert!(Probe { plan, capture: None }.colour_only());
        assert!(CapturePass(None).colour_only());
        let effects = Arc::new(crate::native_post_load::ScreenEffects::default());
        let call = || ScreenCall { egor: EGOR, passes: Vec::new(), values: [0; 4], frame: None };
        assert!(ScreenProbe { effects: Arc::clone(&effects), call: call() }.colour_only());
        let screens = Arc::new(Mutex::new((0, ScreenSet::new())));
        assert!(ScreenPass { effects, call: call(), screens }.colour_only(), "the screen passes bind no depth");
    }
}
