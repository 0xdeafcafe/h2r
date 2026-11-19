//! The `native-post` host (docs/graphics/native-renderer.md, stage 5):
//! the colour pass of `render_screen_effects` (`0x000214f0`) as the game
//! runs it, with its quads dropped on the device and the native glow
//! drawn in place of the last.
//!
//! Entry hooks plan the guest's glow and screen draws. The device retains
//! their original packets through the call's return, then either commits
//! a complete native recording on every live copy or replays the packets.
//! Each native pass uses the original targets' live attachments and
//! formats. The checks run on every call, including after a map reload.
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

use std::io::BufReader;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use engine::colour_pass;
use engine::glow::{self as game_glow, BlurArgs, WideArgs, WideDraw};
use engine::memory::GuestMemory;
use engine::screen_effect::{self, Convolution, Effect};
use engine::screen_grid::{self, Grid};
use feature::native_post::{self as post_feature, CallSite, Fp, Host, PostCall, ScreenEffect};
use gfx::vertex::PrimitiveType;
use map::{Map, ResourceFile, TagId};
use native_render::camera::GuestRead;
use native_render::glow::{self, Blend, Draw, GlowTarget, GpuGlow, Plan, Surface};
use native_render::screen_gpu::{
    self, ConvolutionDraw, ConvolutionGrid, Dest, QuadFrame, ScreenConvolution, ScreenDraw, ScreenKey, ScreenSet, ScreenTarget,
    StripVertex,
};
use native_render::screen_look::ScreenStage;

use crate::native_lights_glue::DeviceMemory;
use crate::native_post_load::ScreenEffects;
use crate::native_world_glue::{locate, region};

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
    /// that is the call's last, or has no size, or a convolution drawn
    /// after the last pass: the native pass is issued into the target
    /// bound at the last draw, which must be the call's.
    PassTarget,
    /// A blur convolution (`0x000211a0`) without its quads' frame
    /// ([`quad_frame`]).
    Blur,
    /// A wide convolution (`0x00021360`) without its frame
    /// ([`quad_frame`]: its combiner is the blur's), or with a grid
    /// (`engine::screen_grid`) that could not be made ([`grid_of`]).
    Wide,
    /// Stand-ins for targets `0x19` to `0x1b`, which a drawn convolution
    /// renders into (and a later pass samples through engine textures 24
    /// and 25): they are `screen_gpu`'s, so only a convolution without
    /// its frame misses them.
    StandIns,
    /// A pass whose `0x00016610` gate could not be read
    /// (`screen_effect::template_draws` failed): whether `0x0002d140`
    /// skips it is not known. (A pass the gate says is skipped is left
    /// out of the plan, [`skip_undrawn`].)
    TemplateSkipped,
}

impl Missing {
    pub const ALL: [Missing; 6] = [
        Missing::TemplatePass,
        Missing::PassTarget,
        Missing::Blur,
        Missing::Wide,
        Missing::StandIns,
        Missing::TemplateSkipped,
    ];
}

/// Leaves out of `effect` the passes `0x00016610` does not draw (Code,
/// `0x0002d4b7..0x0002d4be`: such a pass is skipped whole, its strips
/// and its convolutions, `0x0002d4be` jumping to the next pass at
/// `0x0002dabf`). `engine::screen_effect::plan` has every pass drawn, as
/// the differential stubs `0x00016610`; without this the swallow would
/// wait for strips the game never draws. The passes it left out, or
/// `None` when a gate could not be read ([`Missing::TemplateSkipped`]).
pub fn skip_undrawn(mem: &dyn GuestMemory, effect: &mut Effect) -> Option<usize> {
    let shader = effect.shader;
    let mut drawn = Vec::with_capacity(effect.passes.len());
    for p in &effect.passes {
        let implementation = i32::try_from(p.implementation).unwrap_or(-2);
        drawn.push(screen_effect::template_draws(mem, shader, screen_effect::TEMPLATE_LAYER, implementation, p.selector).ok()?);
    }
    let before = effect.passes.len();
    let mut d = drawn.into_iter();
    effect.passes.retain(|_| d.next().unwrap_or(false));
    Some(before - effect.passes.len())
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

/// Makes the grid of a wide convolution's draw: the pass, the call's
/// arguments, the stand-ins' size and the draw ([`wide_grid`],
/// [`grid_of`]).
pub type MakeGrid<'a> = &'a dyn Fn(&screen_effect::Pass, &WideArgs, (i32, i32), &WideDraw) -> Option<ConvolutionGrid>;

/// For the order alone ([`call_ordered`]): an empty grid, which
/// `screen_gpu` never draws.
pub fn unmade_grid(_: &screen_effect::Pass, _: &WideArgs, _: (i32, i32), _: &WideDraw) -> Option<ConvolutionGrid> {
    Some(ConvolutionGrid::default())
}

/// A convolution of pass `p` as the native pass orders and draws it: one
/// kept its result alone, a blur its quads (`engine::glow::BlurDraw`), a
/// wide one its grids (`engine::glow::WideDraw`, each made by `grid`).
/// `None` for one that copies its result (`0x000351a0`; `0x0002d140`
/// passes -1, so never), at a negative size, or with a grid not made.
pub fn screen_convolution(p: &screen_effect::Pass, c: &Convolution, grid: MakeGrid<'_>) -> Option<ScreenConvolution> {
    let size = |(w, h): (i32, i32)| Some((u32::try_from(w).ok()?, u32::try_from(h).ok()?));
    match c {
        Convolution::Kept { result } => Some(ScreenConvolution { size: (0, 0), draws: Vec::new(), result: *result }),
        Convolution::Blur { size: s, blur, .. } if blur.copy.is_none() => Some(ScreenConvolution {
            size: size(*s)?,
            draws: blur
                .draws
                .iter()
                .map(|d| ConvolutionDraw { source: d.source, dest: d.dest, spread: f32::from_bits(d.spread), grid: None })
                .collect(),
            result: blur.result,
        }),
        Convolution::Wide { size: s, args, wide } if wide.copy.is_none() => Some(ScreenConvolution {
            size: size(*s)?,
            draws: wide
                .draws
                .iter()
                .map(|d| Some(ConvolutionDraw { source: d.source, dest: d.dest, spread: 0.0, grid: Some(grid(p, args, *s, d)?) }))
                .collect::<Option<_>>()?,
            result: wide.result,
        }),
        Convolution::Blur { .. } | Convolution::Wide { .. } => None,
    }
}

/// `p`'s convolutions ([`screen_convolution`]), `None` if one is not
/// drawable.
pub fn screen_convolutions(p: &screen_effect::Pass, grid: MakeGrid<'_>) -> Option<Vec<ScreenConvolution>> {
    p.convolutions.iter().map(|c| screen_convolution(p, c, grid)).collect()
}

/// `0x00034d90`'s inputs for draw `d` of a wide convolution into
/// stand-ins `size` (`args` the call's; Code, `0x00021455..0x00021485`):
/// the target the stand-in, each stage bound (`0x00014f60`, which stores
/// `0x0001dd60`'s size of what it binds at `0x00485af4`/`0x00485b04`) at
/// the draw's source's size, `source` for the convolution's own source
/// and `size` for a stand-in (both of the pair are sized so), EDI the
/// draw's offsets, stack word 4 the `1.0` pushed at `0x00021469`, words 5
/// and 6 the call's two words, word 7's low byte (`0x00021360`'s word 6,
/// 0 from `0x0002d140`) clear: the window's rectangle.
pub fn wide_grid(args: &WideArgs, size: (i32, i32), d: &WideDraw, source: (i32, i32)) -> Grid {
    let bound = if d.source == args.source { source } else { size };
    Grid {
        size,
        bound: [bound; 4],
        taps: d.offsets,
        depth: 1.0f32.to_bits(),
        radii: args.words,
        full: false,
    }
}

/// `g`'s strips (`engine::screen_grid::strips`, under the call's `fp`)
/// for the native grid: each vertex its position and its four taps.
/// `None` when they cannot be made, or the source is bound at no size.
pub fn grid_of(mem: &dyn GuestMemory, fp: Fp, g: &Grid) -> Option<ConvolutionGrid> {
    let (w, h) = g.bound[0];
    let bound = (u32::try_from(w).ok().filter(|&w| w > 0)?, u32::try_from(h).ok().filter(|&h| h > 0)?);
    let strips = screen_grid::strips(mem, g, fp.mxcsr, fp.x87_control).ok()?;
    let f = f32::from_bits;
    let strips = strips
        .iter()
        .map(|s| s.iter().map(|v| StripVertex { position: v.position.map(f), texcoords: v.taps.map(|t| t.map(f)) }).collect())
        .collect();
    Some(ConvolutionGrid { bound, strips })
}

/// The size of the source of a wide convolution of pass `p`
/// (`0x0001dd60` as `0x0002d140` leaves it): target 1 the pass's, the
/// bound target the table's.
pub fn wide_source(mem: &dyn GuestMemory, p: &screen_effect::Pass, source: u32) -> Option<(i32, i32)> {
    let set: Vec<_> = p.size.map(|s| (1, s)).into_iter().collect();
    screen_effect::target_size(mem, source, &set).ok()
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
        match (program, dest(p), screen_convolutions(p, &unmade_grid)) {
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
/// ([`pass_drawable`]), `blurs` whether the convolutions can be drawn
/// ([`quad_frame`] read: the blur's quads, and the combiner of both).
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
        // The native pass is issued at the call's last draw, into the
        // target bound then: a convolution drawn after the last pass's
        // strips would leave a stand-in bound.
        let drawn = |c: &Convolution| !matches!(c, Convolution::Kept { .. });
        if i == last && p.convolutions.iter().any(drawn) {
            m.push(Missing::PassTarget);
        }
        for c in &p.convolutions {
            match c {
                Convolution::Kept { .. } => {}
                Convolution::Blur { .. } | Convolution::Wide { .. } if blurs => {}
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

/// A call refused for `m`: counted, and logged once per reason.
fn refused(m: Missing) {
    let i = Missing::ALL.iter().position(|a| *a == m).unwrap_or(0);
    EFFECT_REFUSED[i].fetch_add(1, Ordering::Relaxed);
    if !EFFECT_LOGGED[i].swap(true, Ordering::Relaxed) {
        eprintln!("native-post: screen effects keep their D3D draws: {m:?} does not draw natively yet (logged once per reason)");
    }
}

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
/// its convolutions [`screen_convolutions`], the wide ones' grids made by
/// `grid`), called with `values`; `None` when a convolution is not
/// drawable.
pub fn screen_call(
    effect: &Effect,
    egor: TagId,
    values: [u32; 4],
    depth: u32,
    frame: Option<QuadFrame>,
    grid: MakeGrid<'_>,
) -> Option<ScreenCall> {
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
            Some((key, dest(p).unwrap_or(Dest::Bound), strips, screen_convolutions(p, grid)?))
        })
        .collect::<Option<_>>()?;
    Some(ScreenCall { egor, passes, values, frame })
}

/// A swallow's plan: each draw's slot, and its shape.
pub type SwallowPlan = (Vec<u8>, Vec<(PrimitiveType, usize)>);

/// The slot of a call's passes into the bound target. Not 0, the target
/// bound when the swallow is armed: `0x00016610` binds the pass's target
/// itself (`0x00014bc0` of 0 or 1, Code, `0x0001675c..0x00016765`), so
/// the strips go to target 0 whatever the caller had bound (the UI's call
/// at `0x0022bbbb` as much as `render_screen_effects`' at `0x00013b52`).
pub const SLOT_BOUND: u8 = 1;
/// Target 1's slot, and the first stand-in's (`0x19`; `0x1a` and `0x1b`
/// after it).
pub const SLOT_TARGET1: u8 = 2;
pub const SLOT_STAND_INS: u8 = 3;

/// The swallow of a call's draws in order: per pass its strips, each a
/// triangle strip (`SET_BEGIN_END` 6) of twice the columns, then its
/// convolutions' draws, a blur's each a quad ([`QUAD`]), a wide one's
/// each its grid's strips (`0x00034770`: per row a triangle strip of
/// twice the columns and one); each with its slot ([`SLOT_BOUND`],
/// [`SLOT_TARGET1`], [`SLOT_STAND_INS`] plus `n` for stand-in `n` of
/// `screen_gpu::STAND_INS`), none of them 0.
pub fn screen_swallow(call: &ScreenCall) -> Option<SwallowPlan> {
    let mut draws = Vec::new();
    for (_, d, strips, convolutions) in &call.passes {
        let slot = if *d == Dest::Bound { SLOT_BOUND } else { SLOT_TARGET1 };
        for s in strips {
            draws.push((slot, (PrimitiveType::TriangleStrip, s.len())));
        }
        for q in convolutions.iter().flat_map(|c| &c.draws) {
            let slot = SLOT_STAND_INS + screen_gpu::stand_in(q.dest)? as u8;
            match &q.grid {
                None => draws.push((slot, QUAD)),
                Some(g) => draws.extend(g.strips.iter().map(|s| (slot, (PrimitiveType::TriangleStrip, s.len())))),
            }
        }
    }
    if draws.is_empty() {
        return None;
    }
    Some(draws.into_iter().unzip())
}

/// The direct D3DDevice_End at the end of a layer-15 strip.
const SCREEN_STRIP_END: u32 = 0x0002_d8b2;

fn screen_end_returns(call: &ScreenCall) -> Vec<Option<u32>> {
    let mut returns = Vec::new();
    for (_, _, strips, convolutions) in &call.passes {
        returns.extend(std::iter::repeat_n(Some(SCREEN_STRIP_END), strips.len()));
        for draw in convolutions.iter().flat_map(|c| &c.draws) {
            let count = draw.grid.as_ref().map_or(1, |g| g.strips.len());
            returns.extend(std::iter::repeat_n(None, count));
        }
    }
    returns
}

fn target_header(mem: &dyn GuestMemory, index: u32) -> Option<u32> {
    let addr = glow::TARGETS.checked_add(index.checked_mul(glow::TARGET_STRIDE)?)?;
    mem.u32(addr + 0x88).ok().filter(|&h| h != 0)
}

fn slot<'a>(target: &d3d::ExternalTarget<'a>, slot: usize) -> Option<&'a d3d::ExternalColor> {
    target.slots.get(slot)?.as_ref()
}

fn call_entry(kind: PostCall) -> u32 {
    match kind {
        PostCall::Colour => 0x0002_14f0,
        PostCall::Screen => 0x0002_d140,
    }
}

fn swallow_call(site: CallSite) -> d3d::SwallowCall {
    d3d::SwallowCall { entry: call_entry(site.kind), esp: site.esp, ret: site.ret }
}

type ScreenCache = Arc<Mutex<(Weak<ScreenEffects>, ScreenSet)>>;

struct ScreenPass {
    effects: Arc<ScreenEffects>,
    call: ScreenCall,
    screens: ScreenCache,
}

impl ScreenPass {
    fn effect(&self, key: ScreenKey) -> Option<&native_render::screen_look::ScreenProgram> {
        self.effects.program(self.call.egor, key.1, key.2)
    }

    fn record(&self, t: d3d::ExternalTarget<'_>) -> Result<usize, screen_gpu::ScreenRefused> {
        let mut screens = self.screens.lock().unwrap_or_else(|e| e.into_inner());
        if !screens.0.ptr_eq(&Arc::downgrade(&self.effects)) {
            screens.1.clear();
            screens.0 = Arc::downgrade(&self.effects);
        }
        let mut draws = Vec::new();
        for (key, dest, strips, convolutions) in &self.call.passes {
            let program = self.effect(*key).ok_or(screen_gpu::ScreenRefused::NoProgram)?;
            draws.push(ScreenDraw { key: *key, program, strips, dest: *dest, values: self.call.values, convolutions });
        }
        let images = &self.effects.images;
        let mut stage_image = |stage| match stage {
            ScreenStage::Bitmap { bitmap, image } => images.get(&(bitmap, image)).cloned(),
            _ => None,
        };
        let bound = slot(&t, SLOT_BOUND as usize).ok_or(screen_gpu::ScreenRefused::Attachment(0))?;
        let target1 = slot(&t, SLOT_TARGET1 as usize).map(|s| &s.attachment);
        let convolutions = std::array::from_fn(|i| slot(&t, usize::from(SLOT_STAND_INS) + i).map(|s| &s.attachment));
        let target = ScreenTarget {
            device: t.device,
            queue: t.queue,
            encoder: t.encoder,
            color: &bound.attachment,
            target1,
            convolutions,
            guest_size: bound.guest_size,
            size: bound.size,
            z_scale: t.z_scale,
        };
        screens.1.encode(target, &draws, self.call.frame.as_ref(), &mut stage_image)
    }
}

impl d3d::ExternalPass for ScreenPass {
    fn colour_only(&self) -> bool { true }

    // Only prepared recordings can replace a screen call.
    fn encode(&mut self, _: d3d::ExternalTarget<'_>) {}

    fn record_ranges(&mut self, t: d3d::ExternalTarget<'_>) -> usize {
        match self.record(t) {
            Ok(count) => count,
            Err(error) => {
                log_rejection(|| format!("screen effect {:#010x} on live target: {error:?}", self.call.egor.0));
                0
            }
        }
    }
}

impl d3d::SwallowPass for ScreenPass {
    fn validate(&self, originals: &[d3d::SwallowOriginal<'_>], slots: &[Option<d3d::SwallowTarget>]) -> Result<(), String> {
        let mut originals = originals.iter();
        let mut drawn = screen_gpu::Drawn::default();
        for (key, dest, strips, convolutions) in &self.call.passes {
            let program = self.effect(*key).ok_or("missing screen program")?;
            let reads = drawn.reads(program, *dest).map_err(|e| format!("{e:?}"))?;
            for _ in strips {
                let original = originals.next().ok_or("missing screen strip")?;
                let draw = original.draw;
                let state = &draw.state.render;
                if !matches_colour_state(draw, program.state.blend, program.state.color_writes, program.state.alpha_test)
                    || (state.blend_enable && gfx::translate::blend_constant(state) != program.state.blend_constant)
                {
                    return Err(format!("screen effect {:#010x}: live strip colour state differs from its program", self.call.egor.0));
                }
                for (stage, source) in reads.iter().enumerate() {
                    if let Some(index) = source {
                        check_source(original, stage, screen_slot(*index), slots)?;
                    }
                }
            }
            for q in convolutions.iter().flat_map(|c| &c.draws) {
                for _ in 0..q.grid.as_ref().map_or(1, |g| g.strips.len()) {
                    let original = originals.next().ok_or("missing convolution draw")?;
                    if !matches_colour_state(original.draw, None, wgpu::ColorWrites::ALL, None) {
                        return Err(format!("screen effect {:#010x}: live convolution colour state differs", self.call.egor.0));
                    }
                    for stage in 0..4 {
                        if original.sampled[stage] {
                            check_source(original, stage, screen_slot(q.source), slots)?;
                        }
                    }
                }
            }
            drawn.pass(program, *dest, convolutions).map_err(|e| format!("{e:?}"))?;
        }
        Ok(())
    }
}

fn screen_slot(target: u32) -> Option<u8> {
    match target {
        0 => Some(SLOT_BOUND),
        1 => Some(SLOT_TARGET1),
        _ => screen_gpu::stand_in(target).map(|i| SLOT_STAND_INS + i as u8),
    }
}

fn check_source(
    original: &d3d::SwallowOriginal<'_>,
    stage: usize,
    slot: Option<u8>,
    slots: &[Option<d3d::SwallowTarget>],
) -> Result<(), String> {
    let target = slot.and_then(|i| slots.get(usize::from(i))).copied().flatten();
    let info = original.draw.texture_info[stage];
    let extras = original.draw.extras[stage];
    if let (Some(target), Some(info)) = (target, info)
        && original.source_data[stage] == Some(target.data)
        && target.size == (info.width, info.height)
        && info.levels == 1
        && extras.color_key_op == 0
        && extras.color_sign == 0
        && extras.alpha_kill == 0
        && matches!(info.format, gfx::D3DFormat::LinA8R8G8B8 | gfx::D3DFormat::A8R8G8B8)
    {
        return Ok(());
    }
    Err(format!("live source for stage {stage} differs from post slot {slot:?} ({info:?})"))
}

fn matches_colour_state(
    draw: &d3d::ResolvedDraw,
    blend: Option<wgpu::BlendState>,
    writes: wgpu::ColorWrites,
    alpha: Option<(wgpu::CompareFunction, u8)>,
) -> bool {
    use gfx::state::{CompareFunc, CullMode, FillMode};
    let state = &draw.state.render;
    let live_alpha = (state.alpha_test_enable && state.alpha_func != CompareFunc::Always)
        .then(|| (gfx::translate::compare_function(state.alpha_func), state.alpha_ref));
    gfx::translate::blend_state(state) == blend
        && gfx::translate::color_writes(state.color_write) == writes
        && live_alpha == alpha
        && state.cull_mode == CullMode::None
        && state.fill_mode == FillMode::Solid
        && state.back_fill_mode == FillMode::Solid
        && !state.fog_enable
        && draw.clip_rects.is_empty()
        && draw.clip.is_some_and(|c| c.is_whole(draw.target.width, draw.target.height))
}

fn log_rejection(detail: impl FnOnce() -> String) {
    static LOGGED: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let mut logged = LOGGED.lock().unwrap_or_else(|e| e.into_inner());
    let detail = detail();
    if logged.len() < 32 && !logged.contains(&detail) {
        eprintln!("native-post: {detail}. Using the D3D draws.");
        logged.push(detail);
    }
}

/// `H2_POST_SHAPES` set: each screen-effect call's swallow plan on stderr
/// (its egor, probe or drop, each draw's slot, primitive and vertex
/// count), the first [`DUMPED_PLANS`] calls, for an A/B to set beside
/// the device's account of a mismatch.
fn dump_plan(egor: TagId, drop: bool, plan: &SwallowPlan) {
    static DUMPED: AtomicU64 = AtomicU64::new(0);
    static ON: OnceLock<bool> = OnceLock::new();
    if !*ON.get_or_init(|| std::env::var_os("H2_POST_SHAPES").is_some()) {
        return;
    }
    if DUMPED.fetch_add(1, Ordering::Relaxed) >= DUMPED_PLANS {
        return;
    }
    eprintln!("native-post: H2_POST_SHAPES {}", plan_line(egor, drop, plan));
}

/// How many calls `H2_POST_SHAPES` dumps.
pub const DUMPED_PLANS: u64 = 64;

/// One call's plan as `H2_POST_SHAPES` writes it.
pub fn plan_line(egor: TagId, drop: bool, plan: &SwallowPlan) -> String {
    let draws: Vec<String> = plan.0.iter().zip(&plan.1).map(|(s, (p, n))| format!("{s}:{p:?}x{n}")).collect();
    format!("{:#010x} {} [{}]", egor.0, if drop { "drop" } else { "probe" }, draws.join(" "))
}

/// `H2_POST_TRACE` ([`post_feature::TraceSpec`]): the calls of
/// `0x0002d140` traced so far, and whether one is running (the device
/// keeping its account, [`d3d::Traced`]). A trace runs from its call's
/// hook to the next hook of either kind (the next effect, or the next
/// frame's colour pass), then is written, one line per event.
static TRACE: Mutex<Traces> = Mutex::new(Traces::new());

/// The traces so far: their count, each egor's, and whether one runs.
pub struct Traces {
    traced: u64,
    egors: Vec<(u32, u64)>,
    running: bool,
}

impl Traces {
    pub const fn new() -> Self {
        Traces { traced: 0, egors: Vec::new(), running: false }
    }

    /// Starts a trace of a call of `egor` while it has had fewer than
    /// `calls`: its number (from 1), or `None`.
    pub fn start(&mut self, egor: u32, calls: u64) -> Option<u64> {
        let i = match self.egors.iter().position(|(e, _)| *e == egor) {
            Some(i) => i,
            None => {
                self.egors.push((egor, 0));
                self.egors.len() - 1
            }
        };
        if self.egors[i].1 >= calls {
            return None;
        }
        self.egors[i].1 += 1;
        self.traced += 1;
        self.running = true;
        Some(self.traced)
    }

    /// Ends the trace running: its number, or `None`.
    pub fn end(&mut self) -> Option<u64> {
        std::mem::replace(&mut self.running, false).then_some(self.traced)
    }
}

/// One event of a trace as `H2_POST_TRACE` writes it.
pub fn trace_line(e: &d3d::Traced) -> String {
    match e {
        d3d::Traced::Call { entry, ret } => format!("call {entry} from {ret:#010x}"),
        d3d::Traced::Arming => "swallow: arming, draining what was queued first".into(),
        d3d::Traced::Armed { draws } => format!("swallow: armed for {draws} draws"),
        d3d::Traced::Draw(d) => {
            let inline = if d.inline > 0 { format!(", an inline array of {} words", d.inline) } else { String::new() };
            let push = d.push.map(|p| format!(", its End pushed at {p:#010x}")).unwrap_or_default();
            let fate = d.fate.map(|f| format!(": {f}")).unwrap_or_default();
            format!(
                "draw {} {:?} x{}{inline}{push}, into {:#010x}, after {} draws{fate}",
                d.entry, d.primitive, d.vertices, d.target, d.draws
            )
        }
        d3d::Traced::Note(n) => n.clone(),
    }
}

/// The line a traced call of `0x00016610` adds: its six words, its
/// caller, and whether `engine::screen_effect::template_draws` says the
/// pass draws (`port`).
pub fn template_line(args: [u32; 6], ret: u32, port: Result<bool, String>) -> String {
    let port = match port {
        Ok(d) => format!("the port says it {}", if d { "draws" } else { "skips" }),
        Err(e) => format!("the port fails: {e}"),
    };
    format!(
        "0x00016610(shader {:#010x}, layer {}, implementation {}, selector {}, target {}, {}) from {ret:#010x}; {port}",
        args[0], args[1], args[2] as i32, args[3] as i32, args[4], args[5]
    )
}

/// The line that opens a trace.
pub fn effect_line(call: ScreenEffect, ret: u32) -> String {
    format!(
        "0x0002d140(egor {:#010x}, values {:#010x}, columns {}, rows {}, corners {:#010x}) from {ret:#010x}",
        call.datum, call.values, call.columns, call.rows, call.corners
    )
}

/// The swallow's slot per quad: the view 0, target 7 1, target 8 2.
pub fn targets(plan: &Plan) -> Vec<u8> {
    plan.draws
        .iter()
        .map(|d| glow_slot(d.dest))
        .collect()
}

fn glow_slot(surface: Surface) -> u8 {
    match surface {
        Surface::View => 0,
        Surface::Down => 1,
        Surface::Other => 2,
    }
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
    if !texture.usage().contains(wgpu::TextureUsages::COPY_SRC) {
        return;
    }
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

/// The native glow, recorded against the call's view and blur levels.
struct GlowPass {
    plan: Plan,
    gpu: Arc<Mutex<GpuGlow>>,
    capture: Option<CapturePass>,
}

impl d3d::ExternalPass for GlowPass {
    fn colour_only(&self) -> bool { true }

    fn encode(&mut self, _: d3d::ExternalTarget<'_>) {}

    fn record_ranges(&mut self, t: d3d::ExternalTarget<'_>) -> usize {
        let intermediates = [1, 2].map(|i| slot(&t, i).map(|s| &s.attachment));
        let result = self.gpu.lock().unwrap_or_else(|e| e.into_inner()).encode(
            GlowTarget {
                device: t.device,
                queue: t.queue,
                encoder: t.encoder,
                color: t.color,
                intermediates,
                guest_size: t.guest_size,
                size: t.size,
            },
            &self.plan,
        );
        match result {
            Ok(()) => self.plan.draws.len(),
            Err(error) => {
                log_rejection(|| format!("glow on live target: {error:?}"));
                0
            }
        }
    }
}

impl d3d::SwallowPass for GlowPass {
    fn validate(&self, originals: &[d3d::SwallowOriginal<'_>], slots: &[Option<d3d::SwallowTarget>]) -> Result<(), String> {
        for (original, want) in originals.iter().zip(&self.plan.draws) {
            let blend = want.blend.map(Blend::state);
            if !matches_colour_state(original.draw, blend, wgpu::ColorWrites::ALL, None) {
                return Err(format!("glow into {:?}: live quad colour state differs", want.dest));
            }
            for stage in 0..4 {
                if original.sampled[stage] {
                    check_source(original, stage, want.stages[stage].map(glow_slot), slots)?;
                }
            }
        }
        Ok(())
    }

    fn after_commit(&mut self) -> Option<Box<dyn d3d::ExternalPass>> {
        self.capture.take().map(|pass| Box::new(pass) as Box<dyn d3d::ExternalPass>)
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

impl d3d::SwallowPass for CapturePass {}

static GLOW_PLAN_REFUSED: AtomicU64 = AtomicU64::new(0);
static SCREEN_PLAN_REFUSED: AtomicU64 = AtomicU64::new(0);
const SUMMARY_PERIOD: Duration = Duration::from_secs(5);

struct Glue<B: d3d::backend::Backend + Send + 'static> {
    device: d3d::abi::SharedDevice<B>,
    gpu: Arc<Mutex<GpuGlow>>,
    /// The current map's screen effects, loaded here whatever the native
    /// world does ([`EffectsSlot`]).
    effects: Arc<Mutex<EffectsSlot>>,
    screens: ScreenCache,
    calls: AtomicU64,
    summary: Mutex<Option<Instant>>,
    glow_commits: AtomicU64,
    screen_commits: AtomicU64,
}

impl<B: d3d::backend::Backend + Send + 'static> Host for Glue<B> {
    fn colour_pass(&self, mem: &mut dyn GuestMemory, site: CallSite, args: [u32; 7], fp: Fp, native: bool) -> bool {
        self.interrupt();
        self.trace_end();
        let call = self.calls.fetch_add(1, Ordering::Relaxed) + 1;
        let ab = Ab::get();
        let pumping = ab.is_some_and(|a| a.pumps(call));
        if !native && !pumping {
            return false;
        }
        let plan = match plan(&*mem, args, fp) {
            Ok(Some(p)) => p,
            Ok(None) => return false,
            Err(error) => {
                GLOW_PLAN_REFUSED.fetch_add(1, Ordering::Relaxed);
                log_rejection(|| format!("glow plan: {error}"));
                return false;
            }
        };
        let capturing = ab.is_some_and(|a| a.captures(call));
        let capture = |side: &str| CapturePass(capturing.then(|| format!("{side}-{call:06}")));
        let targets = targets(&plan);
        let pass: Box<dyn d3d::SwallowPass> = if native {
            Box::new(GlowPass { plan, gpu: Arc::clone(&self.gpu), capture: pumping.then(|| capture("native")) })
        } else {
            Box::new(capture("d3d"))
        };
        let headers = vec![None, target_header(mem, glow::TARGET_DOWN), target_header(mem, glow::TARGET_OTHER)];
        self.arm(mem, d3d::Swallow {
            call: swallow_call(site),
            end_returns: Vec::new(),
            headers,
            targets,
            primitive: QUAD.0,
            vertices: QUAD.1,
            shapes: Vec::new(),
            drop: native,
            pass,
        })
    }

    fn screen_effect(&self, mem: &mut dyn GuestMemory, site: CallSite, call: ScreenEffect, fp: Fp) -> bool {
        self.interrupt();
        let armed = self.effect(mem, site, call, fp);
        self.note(|| format!("hook: armed {armed}"));
        armed
    }

    fn finish(&self, mem: &mut dyn GuestMemory, kind: PostCall, esp: u32) {
        if let Ok(mut device) = self.device.lock() {
            let before = device.stats();
            if let Err(error) = device.finish_swallow(&mut DeviceMemory(mem), call_entry(kind), esp) {
                eprintln!("native-post: finishing {kind:?}: {error}");
            }
            let after = device.stats();
            if after.swallow_refusals > before.swallow_refusals
                && let Some(detail) = device.swallow_mismatch()
            {
                log_rejection(|| format!("{kind:?}: {detail}"));
            }
            let counter = match kind {
                PostCall::Colour => &self.glow_commits,
                PostCall::Screen => &self.screen_commits,
            };
            counter.fetch_add(after.swallow_commits - before.swallow_commits, Ordering::Relaxed);
            self.trace_summary(after);
        }
        if kind == PostCall::Screen {
            self.trace_end();
        }
    }

    fn trace_effect(&self, _mem: &mut dyn GuestMemory, call: ScreenEffect, ret: u32) {
        self.trace_end();
        let Some(spec) = post_feature::trace_spec() else { return };
        let mut t = TRACE.lock().unwrap_or_else(|e| e.into_inner());
        let Ok(mut d) = self.device.lock() else { return };
        if t.start(call.datum, spec.calls).is_none() {
            return;
        }
        d.trace(true);
        d.trace_note(|| effect_line(call, ret));
    }

    fn trace_template(&self, mem: &mut dyn GuestMemory, args: [u32; 6], ret: u32) {
        self.note(|| {
            let port = screen_effect::template_draws(&*mem, args[0], args[1], args[2] as i32, args[3] as i32)
                .map_err(|e| format!("{e:?}"));
            template_line(args, ret, port)
        });
    }

    fn trace_note(&self, note: String) {
        self.note(|| note);
    }
}

impl<B: d3d::backend::Backend + Send + 'static> Glue<B> {
    fn interrupt(&self) {
        if let Ok(mut device) = self.device.lock()
            && let Err(error) = device.interrupt_swallow()
        {
            eprintln!("native-post: replaying interrupted call: {error}");
        }
    }

    fn trace_summary(&self, stats: d3d::Stats) {
        if post_feature::trace_spec().is_none() {
            return;
        }
        let now = Instant::now();
        let mut last = self.summary.lock().unwrap_or_else(|e| e.into_inner());
        if last.is_some_and(|last| now.duration_since(last) < SUMMARY_PERIOD) {
            return;
        }
        *last = Some(now);
        let device_reasons = d3d::SwallowRefusal::ALL
            .map(|reason| format!("{reason:?}={}", stats.swallow_reasons[reason.index()]))
            .join(" ");
        let missing = Missing::ALL.into_iter().zip(effect_refusals())
            .map(|(reason, count)| format!("{reason:?}={count}"))
            .collect::<Vec<_>>().join(" ");
        eprintln!(
            "native-post: H2_POST_TRACE summary swallowed_calls={} dropped={} replayed={} committed={} glow={} screen={} refused={} {device_reasons} glow_plan={} screen_plan={} {missing}",
            stats.swallow_calls, stats.swallow_draws_dropped, stats.swallow_draws_replayed,
            stats.swallow_commits, self.glow_commits.load(Ordering::Relaxed),
            self.screen_commits.load(Ordering::Relaxed), stats.swallow_refusals,
            GLOW_PLAN_REFUSED.load(Ordering::Relaxed), SCREEN_PLAN_REFUSED.load(Ordering::Relaxed),
        );
    }

    /// Adds `note` to the trace running, if one is.
    fn note(&self, note: impl FnOnce() -> String) {
        if !TRACE.lock().unwrap_or_else(|e| e.into_inner()).running {
            return;
        }
        if let Ok(mut d) = self.device.lock() {
            d.trace_note(note);
        }
    }

    /// Ends the trace running, if one is, and writes it.
    fn trace_end(&self) {
        let Some(call) = TRACE.lock().unwrap_or_else(|e| e.into_inner()).end() else { return };
        let Ok(mut d) = self.device.lock() else { return };
        let (events, past) = d.take_trace();
        d.trace(false);
        let end = format!(
            "end: {} swallow mismatches, the last: {}; a swallow {}",
            d.stats().swallow_mismatches,
            d.swallow_mismatch().unwrap_or("none"),
            if d.swallowing() { "still armed" } else { "not armed" }
        );
        std::mem::drop(d);
        for e in &events {
            eprintln!("native-post: H2_POST_TRACE #{call} {}", trace_line(e));
        }
        if past > 0 {
            eprintln!("native-post: H2_POST_TRACE #{call} {past} more not kept");
        }
        eprintln!("native-post: H2_POST_TRACE #{call} {end}");
    }

    /// [`Host::screen_effect`]'s body.
    fn effect(&self, mem: &mut dyn GuestMemory, site: CallSite, call: ScreenEffect, fp: Fp) -> bool {
        let (mut effect, values) = match plan_effect(&*mem, call, fp) {
            Ok(Some(e)) => e,
            Ok(None) => {
                self.note(|| "hook: no plan (no egor, or nothing to draw)".into());
                return false;
            }
            Err(e) => {
                SCREEN_PLAN_REFUSED.fetch_add(1, Ordering::Relaxed);
                if !EFFECT_PLAN_LOGGED.swap(true, Ordering::Relaxed) {
                    eprintln!("native-post: a screen effect keeps its D3D draws: {e} (logged once)");
                }
                return false;
            }
        };
        let effects = self.screen_effects(&*mem);
        let egor = TagId(call.datum);
        let skipped = skip_undrawn(&*mem, &mut effect);
        if let Some(n) = skipped.filter(|&n| n > 0) {
            self.note(|| format!("hook: {n} passes 0x00016610 skips left out"));
        }
        let ordered = call_ordered(effects.as_deref(), egor, &effect);
        if effects.is_none() {
            // The first call on a map starts their load: nothing is
            // missing yet, they are not in.
            self.note(|| "hook: the map's screen effects are still loading".into());
            return false;
        }
        let frame = quad_frame(&*mem, fp);
        let mut m = missing(&effect, &|p| ordered && pass_drawable(effects.as_deref(), egor, p), frame.is_some());
        if skipped.is_none() {
            m.push(Missing::TemplateSkipped);
        }
        for m in &m {
            refused(*m);
        }
        if !m.is_empty() {
            self.note(|| format!("hook: refused, missing {m:?}"));
        }
        // Armed only once nothing is missing (never while the stand-ins
        // are); until then the D3D path draws every effect.
        if !m.is_empty() {
            return false;
        }
        let Some(effects) = effects else { return false };
        let Ok(depth) = mem.u32(screen_effect::constant::DEPTH) else {
            SCREEN_PLAN_REFUSED.fetch_add(1, Ordering::Relaxed);
            self.note(|| "hook: the depth constant is unreadable".into());
            return false;
        };
        let grid = |p: &screen_effect::Pass, a: &WideArgs, size: (i32, i32), d: &WideDraw| {
            grid_of(&*mem, fp, &wide_grid(a, size, d, wide_source(&*mem, p, a.source)?))
        };
        let Some(call) = screen_call(&effect, egor, values, depth, frame, &grid) else {
            refused(Missing::Wide);
            self.note(|| "hook: refused, a wide convolution's grid was not made".into());
            return false;
        };
        let Some(plan) = screen_swallow(&call) else {
            self.note(|| "hook: nothing to swallow (no strips, or a convolution into no stand-in)".into());
            return false;
        };
        dump_plan(egor, true, &plan);
        self.note(|| format!("hook: plan {}", plan_line(egor, true, &plan)));
        let (targets, shapes) = plan;
        let (primitive, vertices) = shapes[0];
        let end_returns = screen_end_returns(&call);
        let headers = [None, Some(0), Some(1), Some(0x19), Some(0x1a), Some(0x1b)]
            .map(|index| index.and_then(|index| target_header(mem, index))).to_vec();
        let pass = Box::new(ScreenPass { effects, call, screens: Arc::clone(&self.screens) });
        self.arm(mem, d3d::Swallow {
            call: swallow_call(site), end_returns, headers, targets, primitive, vertices, shapes, drop: true, pass,
        })

    }
}

impl<B: d3d::backend::Backend + Send + 'static> Glue<B> {
    /// The loaded map's screen effects: `None` until they load, the first
    /// call on a map starting their load ([`spawn_load`]).
    fn screen_effects(&self, mem: &dyn GuestMemory) -> Option<Arc<ScreenEffects>> {
        map_effects(&self.effects, mem, &mut |name| spawn_load(name, Arc::clone(&self.effects)))
    }

    fn arm(&self, mem: &mut dyn GuestMemory, swallow: d3d::Swallow) -> bool {
        let Ok(mut device) = self.device.lock() else { return false };
        device.swallow(&mut DeviceMemory(mem), swallow).unwrap_or_else(|error| {
            eprintln!("native-post: arming the call: {error}");
            false
        })
    }
}

/// The screen effects of the map the game has loaded, which the post
/// glue loads itself: the hook runs with the native post on, whether or
/// not the native world (whose load used to carry them) is.
#[derive(Default)]
pub struct EffectsSlot {
    map: Option<String>,
    effects: Option<Arc<ScreenEffects>>,
}

impl EffectsSlot {
    /// Map `name`'s effects once loaded, and whether a load of them must
    /// start now: at the first ask on a map, which forgets the last
    /// map's.
    pub fn ask(&mut self, name: &str) -> (Option<Arc<ScreenEffects>>, bool) {
        if self.map.as_deref() == Some(name) {
            return (self.effects.clone(), false);
        }
        self.map = Some(name.to_string());
        self.effects = None;
        (None, true)
    }

    /// The load of map `name` finished: kept while it is still the map;
    /// whether it was.
    pub fn loaded(&mut self, name: &str, effects: ScreenEffects) -> bool {
        if self.map.as_deref() != Some(name) {
            return false;
        }
        self.effects = Some(Arc::new(effects));
        true
    }
}

/// The screen effects of the map `mem` has loaded (its cache header's
/// name) as `slot` holds them, `load` started at the first ask on a map.
/// Nothing but the map's name is read: the native world plays no part.
pub fn map_effects(
    slot: &Mutex<EffectsSlot>,
    mem: &dyn GuestMemory,
    load: &mut dyn FnMut(String),
) -> Option<Arc<ScreenEffects>> {
    let name = crate::health::loaded_map_name(mem).ok().filter(|n| !n.is_empty())?;
    let (effects, start) = slot.lock().unwrap_or_else(|e| e.into_inner()).ask(&name);
    if start {
        load(name);
    }
    effects
}

/// Loads map `name`'s screen effects on a thread of their own into
/// `slot`; a map that does not load keeps every effect on D3D.
fn spawn_load(name: String, slot: Arc<Mutex<EffectsSlot>>) {
    let spawned = std::thread::Builder::new().name("native-post load".into()).spawn(move || match load_effects(&name) {
        Ok(e) => {
            slot.lock().unwrap_or_else(|e| e.into_inner()).loaded(&name, e);
        }
        Err(e) => eprintln!("native-post: {name}: the screen effects did not load ({e}); every screen effect keeps its D3D draws"),
    });
    if let Err(e) = spawned {
        eprintln!("native-post: {e}: the screen effects did not load; every screen effect keeps its D3D draws");
    }
}

/// Map `name` opened as the native world opens it (its file through the
/// locator, the shared maps as resources), and its egors loaded
/// (`native_post_load::load`).
fn load_effects(name: &str) -> Result<ScreenEffects, native_render::load::Error> {
    let file = locate(name).ok_or("the map's file is not found")?;
    let mut map = Map::from_region(BufReader::new(std::fs::File::open(&file.path)?), file.offset, file.len)?;
    let mut files = assets::ResourceFiles::new();
    files.set(ResourceFile::Local, region(&file))?;
    for (which, short) in [
        (ResourceFile::MainMenu, "mainmenu"),
        (ResourceFile::Shared, "shared"),
        (ResourceFile::SinglePlayerShared, "single_player_shared"),
    ] {
        if let Some(f) = locate(short) {
            files.set(which, region(&f))?;
        }
    }
    Ok(crate::native_post_load::load(&mut map, &mut files))
}

/// Installs the host over `device`. It loads each map's screen effects
/// itself ([`EffectsSlot`]).
pub(crate) fn install<B: d3d::backend::Backend + Send + 'static>(device: &d3d::abi::SharedDevice<B>) {
    feature::native_post::set_host(Arc::new(Glue {
        device: Arc::clone(device),
        gpu: Arc::new(Mutex::new(GpuGlow::new())),
        effects: Arc::new(Mutex::new(EffectsSlot::default())),
        screens: Arc::new(Mutex::new((Weak::new(), ScreenSet::new()))),
        calls: AtomicU64::new(0),
        summary: Mutex::new(None),
        glow_commits: AtomicU64::new(0),
        screen_commits: AtomicU64::new(0),
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

    /// What arms a screen call's swallow: no draw is slot 0's (the target
    /// bound when armed), since `0x00016610` binds the pass's target
    /// itself; the mainmenu A/B's call mismatched at its first strip
    /// before. `H2_POST_SHAPES` writes the plan.
    #[test]
    fn a_screen_swallow_takes_the_target_its_pass_binds() {
        let effect = Effect { shader: 0, implementations: 2, passes: vec![drawn_pass(2, 3)] };
        let call = screen_call(&effect, EGOR, [0; 4], 0, None, &unmade_grid).unwrap();
        let plan = screen_swallow(&call).unwrap();
        assert!(!plan.0.contains(&0), "{:?}", plan.0);
        assert_eq!(plan_line(EGOR, false, &plan), "0xe0000001 probe [1:TriangleStripx6 1:TriangleStripx6]");
        assert!(plan_line(EGOR, true, &plan).contains(" drop "));
    }

    /// A call is armed only when `0x00016610` draws every pass the plan
    /// has (the gate itself: engine's
    /// `the_template_pass_draws_only_an_implementation_with_a_vertex_shader`);
    /// a template that does not read keeps it on D3D.
    #[test]
    fn a_pass_the_template_skips_is_left_out_and_an_unread_gate_refuses() {
        use engine::memory::FlatMemory;
        let m = FlatMemory::new(0x0010_0000, 0x100);
        let mut one = Effect { shader: 0xe001_0005, implementations: 1, passes: vec![pass(0, vec![])] };
        assert_eq!(skip_undrawn(&m, &mut one), None, "unread: refused");
        assert_eq!(one.passes.len(), 1, "untouched");
        let mut none = Effect { passes: vec![], ..one.clone() };
        assert_eq!(skip_undrawn(&m, &mut none), Some(0), "nothing to skip");
        // A shader of -1: 0x00016610 returns 0 at once, every pass skipped.
        let mut unbound = Effect { shader: u32::MAX, passes: vec![pass(0, vec![]), pass(2, vec![])], ..one };
        assert_eq!(skip_undrawn(&m, &mut unbound), Some(2));
        assert!(unbound.passes.is_empty());
        assert!(Missing::ALL.contains(&Missing::TemplateSkipped));
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
        // The blur: its quads draw once their frame is read; so do a wide
        // one's grids (its combiner is the frame's).
        let args = BlurArgs { source: 1, steps: 0, scale: 0, growth: 0, dest: 0x19, other: 0x1a, last: u32::MAX };
        let blur = game_glow::Blur { draws: vec![], result: 1, copy: None };
        let into = |c: Convolution| screen_effect::Pass { size: Some((320, 240)), ..pass(2, vec![c]) };
        let blurred = || effect(vec![into(Convolution::Blur { size: (320, 240), args, blur: blur.clone() }), pass(0, vec![])]);
        assert_eq!(missing(&blurred(), yes, false), vec![Missing::Blur, Missing::StandIns], "no frame");
        assert_eq!(missing(&blurred(), yes, true), vec![], "drawn into the stand-ins");
        let wide = game_glow::Wide { draws: vec![], result: 1, copy: None };
        let wide_args = game_glow::WideArgs { source: 1, taps: 0, scale: 0, words: [0; 2], dest: 0x19, other: 0x1a, last: u32::MAX };
        let widened = effect(vec![into(Convolution::Wide { size: (320, 240), args: wide_args, wide }), pass(0, vec![])]);
        assert_eq!(missing(&widened, yes, false), vec![Missing::Wide, Missing::StandIns], "no frame");
        assert_eq!(missing(&widened, yes, true), vec![], "drawn into the stand-ins");
        // A convolution drawn after the last pass would leave a stand-in
        // bound for the native pass.
        let first = || screen_effect::Pass { size: Some((320, 240)), ..pass(2, vec![]) };
        let last = effect(vec![first(), pass(0, vec![Convolution::Blur { size: (320, 240), args, blur: blur.clone() }])]);
        assert_eq!(missing(&last, yes, true), vec![Missing::PassTarget]);
        let kept_last = effect(vec![first(), pass(0, vec![Convolution::Kept { result: 0 }])]);
        assert_eq!(missing(&kept_last, yes, true), vec![], "a kept one draws nothing");
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
        let p = pass(1, vec![]);
        let c = screen_convolution(&p, &Convolution::Blur { size: (320, 240), args, blur: blur.clone() }, &unmade_grid).unwrap();
        assert_eq!((c.size, c.result), ((320, 240), 0x1a));
        assert_eq!(c.draws, vec![
            ConvolutionDraw { source: 1, dest: 0x19, spread: 1.5, grid: None },
            ConvolutionDraw { source: 0x19, dest: 0x1a, spread: 2.0, grid: None },
        ]);
        let kept = screen_convolution(&p, &Convolution::Kept { result: 1 }, &unmade_grid);
        assert_eq!(kept.map(|c| (c.draws.len(), c.result)), Some((0, 1)));
        let copied = game_glow::Blur { copy: Some((0x1a, 0x1b)), ..blur.clone() };
        assert_eq!(screen_convolution(&p, &Convolution::Blur { size: (320, 240), args, blur: copied }, &unmade_grid), None, "0x000351a0");
        assert_eq!(screen_convolution(&p, &Convolution::Blur { size: (-1, 240), args, blur }, &unmade_grid), None);
    }

    /// A wide convolution as the native pass draws it: each draw its grid,
    /// made for its own step; none when a grid is not made.
    #[test]
    fn a_wide_convolution_is_its_grids() {
        let args = WideArgs { source: 1, taps: 2, scale: 0, words: [5, 6], dest: 0x19, other: 0x1a, last: u32::MAX };
        let draws = vec![
            WideDraw { source: 1, dest: 0x19, offsets: [1, 2, 3, 4] },
            WideDraw { source: 0x19, dest: 0x1a, offsets: [5, 6, 7, 8] },
        ];
        let wide = game_glow::Wide { draws, result: 0x1a, copy: None };
        let c = Convolution::Wide { size: (320, 240), args, wide: wide.clone() };
        let p = screen_effect::Pass { size: Some((640, 480)), ..pass(1, vec![]) };
        // A grid naming its step: one strip whose first tap is the offsets'.
        let named = |_: &screen_effect::Pass, a: &WideArgs, size: (i32, i32), d: &WideDraw| {
            let g = wide_grid(a, size, d, (640, 480));
            let v = StripVertex { texcoords: [[f32::from_bits(d.offsets[0]); 4]; 4], ..Default::default() };
            Some(ConvolutionGrid { bound: (g.bound[0].0 as u32, g.bound[0].1 as u32), strips: vec![vec![v; 3]] })
        };
        let s = screen_convolution(&p, &c, &named).unwrap();
        assert_eq!((s.size, s.result), ((320, 240), 0x1a));
        assert_eq!(s.draws.iter().map(|d| (d.source, d.dest)).collect::<Vec<_>>(), vec![(1, 0x19), (0x19, 0x1a)]);
        let grids: Vec<_> = s.draws.iter().map(|d| d.grid.clone().unwrap()).collect();
        assert_eq!((grids[0].bound, grids[1].bound), ((640, 480), (320, 240)), "the source's, then a stand-in's");
        assert_eq!(grids[1].strips[0][0].texcoords[0][0], f32::from_bits(5));
        assert_eq!(screen_convolution(&p, &c, &|_, _, _, _| None), None, "a grid not made");
        let copied = Convolution::Wide { size: (320, 240), args, wide: game_glow::Wide { copy: Some((0x1a, 0x1b)), ..wide } };
        assert_eq!(screen_convolution(&p, &copied, &named), None, "0x000351a0");
    }

    /// `0x00034d90`'s inputs as `0x00021360` pushes them.
    #[test]
    fn a_wide_draw_is_its_grid() {
        let radii = [0.25f32.to_bits(), 0.75f32.to_bits()];
        let args = WideArgs { source: 0, taps: 1, scale: 0, words: radii, dest: 0x1b, other: 0x19, last: u32::MAX };
        let first = WideDraw { source: 0, dest: 0x1b, offsets: [1, 2, 3, 4] };
        let g = wide_grid(&args, (160, 120), &first, (640, 480));
        assert_eq!(g, Grid {
            size: (160, 120),
            bound: [(640, 480); 4],
            taps: [1, 2, 3, 4],
            depth: 1.0f32.to_bits(),
            radii,
            full: false,
        });
        let next = WideDraw { source: 0x1b, dest: 0x19, offsets: [0; 4] };
        assert_eq!(wide_grid(&args, (160, 120), &next, (640, 480)).bound, [(160, 120); 4], "a stand-in, at the pair's size");
    }

    /// The grid's strips as `engine::screen_grid` makes them, each vertex
    /// its position and taps; a source bound at no size makes none.
    #[test]
    fn a_grid_is_screen_grids_strips() {
        use engine::memory::FlatMemory;
        use engine::screen_grid::constant as k;
        let mut m = FlatMemory::new(0x0040_0000, 0x0020_0000);
        for (a, v) in [
            (k::ONE, 1.0f32),
            (k::HALF, 0.5),
            (k::SIGN_CLEAR, -0.5),
            (k::DOUBLE, 2.0),
            (k::DEPTH, 1.0),
            (k::NEAR, 0.1),
            (k::FAR, 100.0),
        ] {
            m.set_u32(a, v.to_bits()).unwrap();
        }
        // Top 0, left 0, bottom 480, right 640.
        for (o, v) in [(0u32, 0u16), (2, 0), (4, 480), (6, 640)] {
            m.set_u16(screen_grid::WINDOW + o, v).unwrap();
        }
        let fp = Fp { mxcsr: 0x1f80, x87_control: 0x027f };
        let args = WideArgs { source: 1, taps: 1, scale: 0, words: [0; 2], dest: 0x19, other: 0x1a, last: u32::MAX };
        let d = WideDraw { source: 1, dest: 0x19, offsets: [0.5f32.to_bits(), 0, 0, 0] };
        let g = wide_grid(&args, (320, 240), &d, (640, 480));
        let grid = grid_of(&m, fp, &g).expect("made");
        assert_eq!(grid.bound, (640, 480));
        assert_eq!(grid.strips.len(), screen_grid::ROWS as usize);
        assert!(grid.strips.iter().all(|s| s.len() == 2 * (screen_grid::COLUMNS as usize + 1)));
        let exact = screen_grid::vertex(&m, &g, 3, 5, fp.mxcsr, fp.x87_control).unwrap();
        // Row 5's strip, column 3, the upper vertex.
        let v = grid.strips[5][6];
        assert_eq!(v.position.map(f32::to_bits), exact.position);
        assert_eq!(v.texcoords.map(|t| t.map(f32::to_bits)), exact.taps);
        assert_eq!(grid.strips[0][0].position[..2], [0.0, 0.0], "the window's corner");
        let unbound = Grid { bound: [(0, 480); 4], ..g };
        assert_eq!(grid_of(&m, fp, &unbound), None);
    }

    /// The source of a wide convolution: target 1 the pass's size, the
    /// bound target the table's (`0x0001dd60`'s default without a surface).
    #[test]
    fn a_wide_source_is_its_targets_size() {
        use engine::memory::FlatMemory;
        use screen_effect::{TARGETS, TARGET_STRIDE};
        let mut m = FlatMemory::new(TARGETS, 0x1000);
        m.set_u32(TARGETS + TARGET_STRIDE + 0x88, 1).unwrap();
        let p = screen_effect::Pass { size: Some((320, 240)), ..pass(2, vec![]) };
        assert_eq!(wide_source(&m, &p, 1), Some((320, 240)));
        assert_eq!(wide_source(&m, &p, 0), Some((640, 480)));
    }

    /// The screen effects load with the native post alone: the first call
    /// on a map starts their load from its name (no native world), later
    /// calls wait for it, and a new map forgets the last one's.
    #[test]
    fn the_screen_effects_load_per_map_without_the_world() {
        use engine::memory::FlatMemory;
        let named = |name: &str| {
            let mut m = FlatMemory::new(crate::health::CACHE_MAP_NAME, 0x40);
            m.write(crate::health::CACHE_MAP_NAME, name.as_bytes()).unwrap();
            m
        };
        let slot = Mutex::new(EffectsSlot::default());
        let mut started = Vec::new();
        let mainmenu = named("mainmenu");
        assert!(map_effects(&slot, &mainmenu, &mut |n| started.push(n)).is_none(), "loading");
        assert_eq!(started, vec!["mainmenu".to_string()], "started by the post itself");
        assert!(map_effects(&slot, &mainmenu, &mut |n| started.push(n)).is_none());
        assert_eq!(started.len(), 1, "one load per map");
        assert!(slot.lock().unwrap().loaded("mainmenu", ScreenEffects::default()));
        assert!(map_effects(&slot, &mainmenu, &mut |n| started.push(n)).is_some(), "loaded");
        let other = named("01a_tutorial");
        assert!(map_effects(&slot, &other, &mut |n| started.push(n)).is_none());
        assert_eq!(started.last().map(String::as_str), Some("01a_tutorial"));
        assert!(!slot.lock().unwrap().loaded("mainmenu", ScreenEffects::default()), "a late load of the last map is dropped");
        assert!(map_effects(&slot, &other, &mut |n| started.push(n)).is_none());
        assert!(map_effects(&slot, &named(""), &mut |n| started.push(n)).is_none(), "no map loaded");
        assert_eq!(started.len(), 2);
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
        let call = screen_call(&effect, EGOR, values, 0x4b7f_ffff, None, &unmade_grid).unwrap();
        assert_eq!(call.passes.len(), 2);
        assert_eq!(call.values, values, "what the passes' engine constants read");
        assert_eq!(call.passes[0].0, (EGOR.0, 1, 0), "(egor, layer pass, implementation)");
        assert_eq!(call.passes[0].1, Dest::Bound);
        assert_eq!(call.passes[0].2[0][0].position[2].to_bits(), 0x4b7f_ffff);
        let strip = (PrimitiveType::from_xbox(6).unwrap(), 8);
        assert_eq!(screen_swallow(&call), Some((vec![SLOT_BOUND; 6], vec![strip; 6])), "SET_BEGIN_END 6");
        let uneven = Effect { passes: vec![drawn_pass(1, 4), drawn_pass(1, 3)], ..effect.clone() };
        let shapes = screen_swallow(&screen_call(&uneven, EGOR, [0; 4], 0, None, &unmade_grid).unwrap()).map(|s| s.1);
        assert_eq!(shapes, Some(vec![strip, (PrimitiveType::TriangleStrip, 6)]), "each strip its own count");
        let empty = Effect { passes: vec![], ..effect.clone() };
        assert_eq!(screen_swallow(&screen_call(&empty, EGOR, [0; 4], 0, None, &unmade_grid).unwrap()), None);
        // Target 1's strips go to slot 2, the target the first finds; no
        // draw is slot 0's (the target bound when armed).
        let into_1 = screen_effect::Pass { kind: 1, size: Some((640, 480)), ..drawn_pass(2, 4) };
        let both = Effect { passes: vec![into_1, drawn_pass(3, 4)], ..effect };
        let call = screen_call(&both, EGOR, [0; 4], 0, None, &unmade_grid).unwrap();
        assert_eq!(call.passes[0].1, Dest::Target1 { guest: (640, 480) });
        assert_eq!(screen_swallow(&call).map(|s| s.0), Some(vec![2, 2, 1, 1, 1]));
        // A blur after the first pass: its quads into 0x19 (slot 3) and
        // 0x1a (slot 4), between the passes' strips.
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
        let blurred = Effect { passes: vec![into_1, both.passes[1].clone()], ..both.clone() };
        let (slots, shapes) = screen_swallow(&screen_call(&blurred, EGOR, [0; 4], 0, None, &unmade_grid).unwrap()).unwrap();
        assert_eq!(slots, vec![2, 2, 3, 4, 1, 1, 1]);
        assert_eq!(shapes[2..4], [QUAD, QUAD]);
        // A wide one: each draw its grid's strips, into its stand-in.
        let args = WideArgs { source: 1, taps: 2, scale: 0, words: [0; 2], dest: 0x19, other: 0x1a, last: u32::MAX };
        let draws = vec![
            WideDraw { source: 1, dest: 0x19, offsets: [0; 4] },
            WideDraw { source: 0x19, dest: 0x1a, offsets: [0; 4] },
        ];
        let wide = game_glow::Wide { draws, result: 0x1a, copy: None };
        let into_1 = screen_effect::Pass {
            convolutions: vec![Convolution::Wide { size: (320, 240), args, wide }],
            ..both.passes[0].clone()
        };
        let widened = Effect { passes: vec![into_1, both.passes[1].clone()], ..both };
        let grid = |_: &screen_effect::Pass, _: &WideArgs, _: (i32, i32), _: &WideDraw| {
            Some(ConvolutionGrid { bound: (640, 480), strips: vec![vec![StripVertex::default(); 42]; 2] })
        };
        let (slots, shapes) = screen_swallow(&screen_call(&widened, EGOR, [0; 4], 0, None, &grid).unwrap()).unwrap();
        assert_eq!(slots, vec![2, 2, 3, 3, 4, 4, 1, 1, 1]);
        assert_eq!(shapes[2..6], [(PrimitiveType::TriangleStrip, 42); 4], "0x00034770's SET_BEGIN_END 6");
        assert!(screen_call(&widened, EGOR, [0; 4], 0, None, &|_, _, _, _| None).is_none(), "a grid not made");
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
        assert!(CapturePass(None).colour_only());
        let effects = Arc::new(crate::native_post_load::ScreenEffects::default());
        let call = || ScreenCall { egor: EGOR, passes: Vec::new(), values: [0; 4], frame: None };
        let screens = Arc::new(Mutex::new((Weak::new(), ScreenSet::new())));
        assert!(ScreenPass { effects, call: call(), screens }.colour_only(), "the screen passes bind no depth");
    }

    /// `H2_POST_TRACE`'s lines: each event the device kept, the call that
    /// opens a trace and each template pass, as the A/B reads them.
    #[test]
    fn a_trace_writes_what_the_device_saw() {
        let draw = |inline, push, fate| {
            d3d::Traced::Draw(d3d::TracedDraw {
                entry: "inline Begin/End",
                primitive: PrimitiveType::QuadList,
                vertices: 0,
                inline,
                push,
                target: 0x8007_c000,
                draws: 12,
                fate,
            })
        };
        let lines: Vec<String> = [
            d3d::Traced::Call { entry: "D3D_MakeRequestedSpace", ret: 0x0002_d5f0 },
            d3d::Traced::Arming,
            d3d::Traced::Armed { draws: 1 },
            draw(24, Some(0x8010_0040), Some("mismatch")),
            d3d::Traced::Draw(d3d::TracedDraw {
                entry: "D3DDevice_DrawVertices",
                primitive: PrimitiveType::TriangleStrip,
                vertices: 4,
                inline: 0,
                push: None,
                target: 0x8007_c000,
                draws: 11,
                fate: None,
            }),
            d3d::Traced::Note("hook: armed true".into()),
        ]
        .iter()
        .map(trace_line)
        .collect();
        assert_eq!(
            lines,
            [
                "call D3D_MakeRequestedSpace from 0x0002d5f0",
                "swallow: arming, draining what was queued first",
                "swallow: armed for 1 draws",
                "draw inline Begin/End QuadList x0, an inline array of 24 words, its End pushed at 0x80100040, into 0x8007c000, after 12 draws: mismatch",
                "draw D3DDevice_DrawVertices TriangleStrip x4, into 0x8007c000, after 11 draws",
                "hook: armed true",
            ]
        );
        let call = ScreenEffect { datum: EGOR.0, values: 0x0050_0000, columns: 2, rows: 2, corners: 0x0050_0010 };
        assert_eq!(
            effect_line(call, 0x0001_3b57),
            "0x0002d140(egor 0xe0000001, values 0x00500000, columns 2, rows 2, corners 0x00500010) from 0x00013b57"
        );
        let args = [0x0012_3450, 15, 0, u32::MAX, 1, 0];
        assert_eq!(
            template_line(args, 0x0002_d4bc, Ok(true)),
            "0x00016610(shader 0x00123450, layer 15, implementation 0, selector -1, target 1, 0) from 0x0002d4bc; the port says it draws"
        );
        assert!(template_line(args, 0x0002_d4bc, Ok(false)).ends_with("; the port says it skips"));
        assert!(template_line(args, 0x0002_d4bc, Err("Data".into())).ends_with("; the port fails: Data"));
        assert_eq!(post_feature::return_line("0x00016610", 0x1234_5600), "0x00016610 returned, AL 0");
    }

    /// `H2_POST_TRACE=N` traces each egor's first `N` calls, numbered
    /// across them, one at a time.
    #[test]
    fn each_egor_gets_its_own_traces() {
        let mut t = Traces::new();
        assert_eq!(t.end(), None, "none running");
        assert_eq!(t.start(EGOR.0, 2), Some(1));
        assert_eq!(t.end(), Some(1));
        assert_eq!(t.end(), None, "ended once");
        assert_eq!(t.start(EGOR.0, 2), Some(2));
        assert_eq!(t.start(EGOR.0, 2), None, "its two");
        assert_eq!(t.end(), Some(2));
        assert_eq!(t.start(0xe0ff_0001, 2), Some(3), "another egor's first");
    }
}
