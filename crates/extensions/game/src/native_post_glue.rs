//! The `native-post` host (docs/graphics/native-post-hud.md, stage 5):
//! the colour pass of `render_screen_effects` (`0x000214f0`) as the game
//! runs it, with its quads dropped on the device and the native glow
//! drawn in place of the last.
//!
//! Entry hooks plan the guest's glow and screen draws. The device retains
//! their original packets through the call's return, then either commits
//! a complete native recording on every live copy or replays the packets.
//! Each native pass uses the original targets' live attachments and
//! formats. The checks run on every call, including after a map reload.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use engine::colour_pass;
use engine::glow::{self as game_glow, BlurArgs, WideArgs, WideDraw};
use engine::memory::GuestMemory;
use engine::screen_effect::{self, Convolution, Effect};
use engine::screen_grid::{self, Grid};
use gfx::vertex::PrimitiveType;
use map::TagId;
use native_render::camera::GuestRead;
use native_render::glow::{self, Blend, Draw, GlowTarget, GpuGlow, Plan, Surface};
use native_render::screen_gpu::{
    self, ConvolutionDraw, ConvolutionGrid, Dest, QuadFrame, ScreenConvolution, ScreenDraw,
    ScreenKey, ScreenTarget, PostRaster, StripVertex,
};
use native_render::screen_look::{CombinerInputs, ScreenStage};
use render_bridge::native_post::{
    self as post_feature, CallSite, Fp, Host, PostCall, ScreenEffect,
};

use crate::native_lights_glue::DeviceMemory;
use crate::native_map;
use crate::native_post_load::ScreenEffects;

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
    let blur_combiner = game_glow::blur_combiner(mem, p.args[4], p.y, fp.mxcsr, fp.x87_control)
        .map_err(err("blur_combiner"))?;
    let blur = game_glow::blur(mem, &BlurArgs::of(&p), fp.mxcsr).map_err(err("blur"))?;
    if blur.copy.is_some() {
        return Err("the blur copies its result (0x000351a0)".into());
    }
    let pass_b = game_glow::pass_b(mem, &p, fp.x87_control).map_err(err("pass_b"))?;
    let view = mem
        .u32(glow::VIEW_TARGET)
        .map_err(err("the view's target"))? as u16 as i16 as i32 as u32;
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
    /// A pass into target 1 without a usable guest size.
    PassTarget,
    /// A pass needs the view, but no strip pins it.
    ViewTarget,
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
    pub const ALL: [Missing; 7] = [
        Missing::TemplatePass,
        Missing::PassTarget,
        Missing::ViewTarget,
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
        drawn.push(
            screen_effect::template_draws(
                mem,
                shader,
                screen_effect::TEMPLATE_LAYER,
                implementation,
                p.selector,
            )
            .ok()?,
        );
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
pub fn pass_drawable(
    effects: Option<&ScreenEffects>,
    egor: TagId,
    p: &screen_effect::Pass,
) -> bool {
    let Ok(implementation) = usize::try_from(p.selector) else {
        return false;
    };
    let Some(program) =
        effects.and_then(|e| e.program(egor, p.implementation as usize, implementation))
    else {
        return false;
    };
    let bound = (0..4).all(|n| {
        program.dims[n].is_none()
            || match program.stages[n] {
                ScreenStage::Empty => false,
                ScreenStage::Bitmap { bitmap, image } => {
                    effects.is_some_and(|e| e.images.contains_key(&(bitmap, image)))
                }
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
        (_, Some((w, h))) => Some(Dest::Target1 {
            guest: (u32::try_from(w).ok()?, u32::try_from(h).ok()?),
        }),
        (_, None) => None,
    }
}

/// Makes the grid of a wide convolution's draw: the pass, the call's
/// arguments, the stand-ins' size and the draw ([`wide_grid`],
/// [`grid_of`]).
pub type MakeGrid<'a> =
    &'a dyn Fn(&screen_effect::Pass, &WideArgs, (i32, i32), &WideDraw) -> Option<ConvolutionGrid>;

/// For the order alone ([`call_ordered`]): an empty grid, which
/// `screen_gpu` never draws.
pub fn unmade_grid(
    _: &screen_effect::Pass,
    _: &WideArgs,
    _: (i32, i32),
    _: &WideDraw,
) -> Option<ConvolutionGrid> {
    Some(ConvolutionGrid::default())
}

/// A convolution of pass `p` as the native pass orders and draws it: one
/// kept its result alone, a blur its quads (`engine::glow::BlurDraw`), a
/// wide one its grids (`engine::glow::WideDraw`, each made by `grid`).
/// `None` for one that copies its result (`0x000351a0`; `0x0002d140`
/// passes -1, so never), at a negative size, or with a grid not made.
pub fn screen_convolution(
    p: &screen_effect::Pass,
    c: &Convolution,
    grid: MakeGrid<'_>,
) -> Option<ScreenConvolution> {
    let size = |(w, h): (i32, i32)| Some((u32::try_from(w).ok()?, u32::try_from(h).ok()?));
    match c {
        Convolution::Kept { result } => Some(ScreenConvolution {
            size: (0, 0),
            draws: Vec::new(),
            result: *result,
        }),
        Convolution::Blur { size: s, blur, .. } if blur.copy.is_none() => Some(ScreenConvolution {
            size: size(*s)?,
            draws: blur
                .draws
                .iter()
                .map(|d| ConvolutionDraw {
                    source: d.source,
                    dest: d.dest,
                    spread: f32::from_bits(d.spread),
                    grid: None,
                })
                .collect(),
            result: blur.result,
        }),
        Convolution::Wide {
            size: s,
            args,
            wide,
        } if wide.copy.is_none() => Some(ScreenConvolution {
            size: size(*s)?,
            draws: wide
                .draws
                .iter()
                .map(|d| {
                    Some(ConvolutionDraw {
                        source: d.source,
                        dest: d.dest,
                        spread: 0.0,
                        grid: Some(grid(p, args, *s, d)?),
                    })
                })
                .collect::<Option<_>>()?,
            result: wide.result,
        }),
        Convolution::Blur { .. } | Convolution::Wide { .. } => None,
    }
}

/// `p`'s convolutions ([`screen_convolution`]), `None` if one is not
/// drawable.
pub fn screen_convolutions(
    p: &screen_effect::Pass,
    grid: MakeGrid<'_>,
) -> Option<Vec<ScreenConvolution>> {
    p.convolutions
        .iter()
        .map(|c| screen_convolution(p, c, grid))
        .collect()
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
    let bound = if d.source == args.source {
        source
    } else {
        size
    };
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
    let bound = (
        u32::try_from(w).ok().filter(|&w| w > 0)?,
        u32::try_from(h).ok().filter(|&h| h > 0)?,
    );
    let strips = screen_grid::strips(mem, g, fp.mxcsr, fp.x87_control).ok()?;
    let f = f32::from_bits;
    let strips = strips
        .iter()
        .map(|s| {
            s.iter()
                .map(|v| StripVertex {
                    position: v.position.map(f),
                    texcoords: v.taps.map(|t| t.map(f)),
                })
                .collect()
        })
        .collect();
    Some(ConvolutionGrid { bound, strips })
}

/// The size of the source of a wide convolution of pass `p`
/// (`0x0001dd60` as `0x0002d140` leaves it): target 1 the pass's, the
/// bound target the table's.
pub fn wide_source(
    mem: &dyn GuestMemory,
    p: &screen_effect::Pass,
    source: u32,
) -> Option<(i32, i32)> {
    let set: Vec<_> = p.size.map(|s| (1, s)).into_iter().collect();
    screen_effect::target_size(mem, source, &set).ok()
}

/// Whether `effect`'s passes and convolutions read only what the call
/// drew before (`screen_gpu::check_order`: target 1, the stand-ins and
/// the results), each with its program loaded.
pub fn call_ordered(effects: Option<&ScreenEffects>, egor: TagId, effect: &Effect) -> Result<(), Missing> {
    let mut passes = Vec::new();
    for p in &effect.passes {
        let program = usize::try_from(p.selector)
            .ok()
            .and_then(|i| effects?.program(egor, p.implementation as usize, i));
        match (program, dest(p), screen_convolutions(p, &unmade_grid)) {
            (Some(program), Some(d), Some(c)) => passes.push((program, d, c)),
            _ => return Err(Missing::TemplatePass),
        }
    }
    screen_gpu::check_order(passes.iter().map(|(p, d, c)| (*p, *d, &c[..]))).map_err(|reason| match reason {
        screen_gpu::ScreenRefused::Attachment(0) => Missing::ViewTarget,
        _ => Missing::TemplatePass,
    })
}

/// The frame of a call's convolution quads (`screen_gpu::QuadFrame`):
/// the window and the tap signs (`glow::read_window`) and the blur's
/// combiner as `0x0002d140` has `0x000211a0` make it (its seventh word 1:
/// `0x00020f30` with its ninth and tenth, strength and `y`, both 0).
pub fn quad_frame(mem: &dyn GuestMemory, fp: Fp) -> Option<QuadFrame> {
    let (window, signs) = glow::read_window(&Ro(mem))?;
    let combiner = game_glow::blur_combiner(mem, 0, 0, fp.mxcsr, fp.x87_control)
        .ok()?
        .0;
    Some(QuadFrame {
        window,
        signs,
        combiner,
    })
}

/// What `effect` needs that is not native yet, once each, in order;
/// `drawable` says whether the native template pass draws a pass
/// ([`pass_drawable`]), `blurs` whether the convolutions can be drawn
/// ([`quad_frame`] read: the blur's quads, and the combiner of both).
pub fn missing(
    effect: &Effect,
    drawable: &dyn Fn(&screen_effect::Pass) -> bool,
    blurs: bool,
) -> Vec<Missing> {
    let mut m = Vec::new();
    for p in &effect.passes {
        if !drawable(p) {
            m.push(Missing::TemplatePass);
        }
        let sized = matches!(dest(p), Some(Dest::Target1 { guest: (w, h) }) if w > 0 && h > 0);
        if p.kind != 0 && !sized {
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
pub use native_render::post::MAX_CORNERS;

/// The plan of the call `c` and its values: its values and corners read,
/// then `engine::screen_effect::plan`.
pub fn plan_effect(
    mem: &dyn GuestMemory,
    c: ScreenEffect,
    fp: Fp,
) -> Result<Option<(Effect, [u32; 4])>, String> {
    let (values, corners) = native_render::post::screen_inputs(
        &Ro(mem), c.values, c.corners, c.columns, c.rows,
    )?;
    let effect = screen_effect::plan(
        mem,
        c.datum,
        values,
        c.columns,
        c.rows,
        &corners,
        fp.mxcsr,
        fp.x87_control,
    )
    .map_err(err("plan"))?;
    Ok(effect.map(|e| (e, values)))
}

static EFFECT_REFUSED: [AtomicU64; Missing::ALL.len()] =
    [const { AtomicU64::new(0) }; Missing::ALL.len()];
static EFFECT_LOGGED: [AtomicBool; Missing::ALL.len()] =
    [const { AtomicBool::new(false) }; Missing::ALL.len()];
static EFFECT_PLAN_LOGGED: AtomicBool = AtomicBool::new(false);

/// Screen-effect preflight refusals for the frame scene's counter map.
pub(crate) fn refusal_counters() -> impl Iterator<Item = (String, u64)> {
    Missing::ALL.into_iter().zip(&EFFECT_REFUSED).map(|(cause, count)| {
        (format!("post_effect_refused_{}", missing_name(cause)), count.load(Ordering::Relaxed))
    })
}

/// A refusal's counter name.
fn missing_name(m: Missing) -> &'static str {
    match m {
        Missing::TemplatePass => "template_pass",
        Missing::PassTarget => "pass_target",
        Missing::ViewTarget => "view_target",
        Missing::Blur => "blur",
        Missing::Wide => "wide",
        Missing::StandIns => "stand_ins",
        Missing::TemplateSkipped => "template_skipped",
    }
}

/// A call refused for `m`: counted, and logged once per reason. The call
/// keeps it as its cause (`scene_kept_post_screen_effect_<cause>`, or a
/// HUD widget's `scene_kept_overlay_screen_effect_<cause>`).
fn refused(m: Missing) {
    render_bridge::native_hud::kept::refuse(missing_name(m));
    let i = Missing::ALL.iter().position(|a| *a == m).unwrap_or(0);
    EFFECT_REFUSED[i].fetch_add(1, Ordering::Relaxed);
    if !EFFECT_LOGGED[i].swap(true, Ordering::Relaxed) {
        eprintln!(
            "native-post: screen effects keep their D3D draws: {m:?} does not draw natively yet (logged once per reason)"
        );
    }
}

/// A call as the native pass draws it: per pass its key, destination,
/// strips (`depth` the positions' z and w, `[0x0045dd40]`) and
/// convolutions; the call's values (what its passes' engine constants
/// read) and its quads' frame.
pub struct ScreenCall {
    pub egor: TagId,
    pub passes: Vec<(
        ScreenKey,
        Dest,
        Vec<Vec<StripVertex>>,
        Vec<ScreenConvolution>,
    )>,
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
            let key = (
                egor.0,
                p.implementation as usize,
                p.selector.max(0) as usize,
            );
            let strips = p
                .strips
                .iter()
                .map(|s| {
                    let words: Vec<_> = s.iter().map(|v| (v.texcoords, v.position)).collect();
                    screen_gpu::strip(&words, depth)
                })
                .collect();
            Some((
                key,
                dest(p)?,
                strips,
                screen_convolutions(p, grid)?,
            ))
        })
        .collect::<Option<_>>()?;
    Some(ScreenCall {
        egor,
        passes,
        values,
        frame,
    })
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
        let slot = if *d == Dest::Bound {
            SLOT_BOUND
        } else {
            SLOT_TARGET1
        };
        for s in strips {
            draws.push((slot, (PrimitiveType::TriangleStrip, s.len())));
        }
        for q in convolutions.iter().flat_map(|c| &c.draws) {
            let slot = SLOT_STAND_INS + screen_gpu::stand_in(q.dest)? as u8;
            match &q.grid {
                None => draws.push((slot, QUAD)),
                Some(g) => draws.extend(
                    g.strips
                        .iter()
                        .map(|s| (slot, (PrimitiveType::TriangleStrip, s.len()))),
                ),
            }
        }
    }
    if draws.is_empty() {
        return None;
    }
    Some(draws.into_iter().unzip())
}

/// The direct D3DDevice_End at the end of a layer-15 strip.
const SCREEN_STRIP_END: u32 = post_feature::SCREEN_STRIP_TAIL;

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

/// The embedded level-zero surface returned by the guest's
/// `0x0001dcf0` for a live render-target descriptor.
fn target_level_surface(mem: &dyn GuestMemory, index: u32) -> Option<u32> {
    let descriptor = glow::TARGETS.checked_add(index.checked_mul(glow::TARGET_STRIDE)?)?;
    if mem.u32(descriptor.checked_add(0x88)?).ok()? == 0
        || mem.u8(descriptor.checked_add(0x95)?).ok()? != 0
        || mem.i32(descriptor.checked_add(0x78)?).ok()? <= 0
    {
        return None;
    }
    descriptor.checked_add(0x18)
}

fn slot(slots: &[Option<d3d::ExternalColor>], slot: usize) -> Option<&d3d::ExternalColor> {
    slots.get(slot)?.as_ref()
}

/// What a post pass prepares against on one target copy: the copy's
/// device, queue and arena, its slots and stages, and the request's depth
/// scale. Read from an `ExternalTarget` (`record_ranges`) or an
/// `ExternalAttachments` (`record_encoded`) alike.
struct PostCopy<'a> {
    device: &'a wgpu::Device,
    queue: &'a wgpu::Queue,
    uploads: &'a gfx::ring::SubmissionUploads,
    slots: &'a [Option<d3d::ExternalColor>],
    stages: [Option<&'a wgpu::TextureView>; 4],
    z_scale: f32,
}

impl<'a> PostCopy<'a> {
    fn of_target(t: &d3d::ExternalTarget<'a>) -> Self {
        Self {
            device: t.device,
            queue: t.queue,
            uploads: t.uploads,
            slots: t.slots,
            stages: t.stages,
            z_scale: t.z_scale,
        }
    }

    fn of_attachments(
        t: &'a d3d::backend::ExternalAttachments,
        request: d3d::ExternalPassRequest,
    ) -> Self {
        Self {
            device: &t.device,
            queue: &t.queue,
            uploads: &t.uploads,
            slots: &t.slots,
            stages: t.stages.each_ref().map(Option::as_ref),
            z_scale: request.z_scale,
        }
    }
}

fn call_entry(kind: PostCall) -> u32 {
    match kind {
        PostCall::Colour => 0x0002_14f0,
        PostCall::Screen => 0x0002_d140,
    }
}

fn swallow_call(site: CallSite) -> d3d::SwallowCall {
    d3d::SwallowCall {
        entry: call_entry(site.kind),
        esp: site.esp,
        ret: site.ret,
    }
}

struct ScreenPass {
    effects: Arc<ScreenEffects>,
    call: ScreenCall,
    raster: OnceLock<Vec<ScreenRaster>>,
}

struct ScreenRaster {
    strip: PostRaster,
    convolutions: Vec<PostRaster>,
}

impl ScreenPass {
    fn effect(&self, key: ScreenKey) -> Option<&native_render::screen_look::ScreenProgram> {
        self.effects.program(self.call.egor, key.1, key.2)
    }

    /// The call on one copy, prepared: the cache's lock is released when
    /// this returns; the commands hold their own handles.
    fn prepare(
        &self,
        t: PostCopy<'_>,
    ) -> Result<screen_gpu::ScreenCommands, screen_gpu::ScreenRefused> {
        let mut screens = self.effects.screens.lock().unwrap_or_else(|e| e.into_inner());
        let mut draws = Vec::new();
        let raster = self
            .raster
            .get()
            .ok_or(screen_gpu::ScreenRefused::NoProgram)?;
        for ((key, dest, strips, convolutions), raster) in self.call.passes.iter().zip(raster) {
            let program = self
                .effect(*key)
                .ok_or(screen_gpu::ScreenRefused::NoProgram)?;
            draws.push(ScreenDraw {
                key: *key,
                program,
                strips,
                raster: raster.strip,
                dest: *dest,
                values: self.call.values,
                convolutions,
                convolution_rasters: &raster.convolutions,
            });
        }
        let images = &self.effects.images;
        let mut stage_image = |stage| match stage {
            ScreenStage::Bitmap { bitmap, image } => images.get(&(bitmap, image)).cloned(),
            _ => None,
        };
        let bound = slot(t.slots, SLOT_BOUND as usize).map(glow::PostSurface::from);
        let target1 = slot(t.slots, SLOT_TARGET1 as usize).map(glow::PostSurface::from);
        let convolutions = std::array::from_fn(|i| {
            slot(t.slots, usize::from(SLOT_STAND_INS) + i).map(glow::PostSurface::from)
        });
        let target = ScreenTarget {
            device: t.device,
            queue: t.queue,
            uploads: t.uploads,
            color: bound,
            target1,
            convolutions,
            z_scale: t.z_scale,
        };
        screens
            .prepare(target, &draws, self.call.frame.as_ref(), &mut stage_image)
    }

    fn refused(&self, error: screen_gpu::ScreenRefused) {
        log_rejection(|| {
            format!(
                "screen effect {:#010x} on live target: {error:?}",
                self.call.egor.0
            )
        });
    }
}

impl d3d::ExternalPass for ScreenPass {
    fn colour_only(&self) -> bool {
        true
    }

    // Frozen by `validate`: the call, its values and its raster are the
    // pass's own, and every source is a slot or the view's copy made in
    // the recording. So a scene replay can record it again on its
    // attachments (docs/graphics/framescene.md, slice 5).
    fn replays_scene(&self) -> bool {
        self.raster.get().is_some()
    }

    // Only prepared recordings can replace a screen call.
    fn encode(&mut self, _: d3d::ExternalTarget<'_>) {}

    fn record_ranges(&mut self, t: d3d::ExternalTarget<'_>) -> usize {
        match self.prepare(PostCopy::of_target(&t)) {
            Ok(commands) => {
                commands.record(t.encoder);
                commands.ranges()
            }
            Err(error) => {
                self.refused(error);
                0
            }
        }
    }

    // Target 1, the stand-ins and the view's feedback copy are targets
    // of the call's own: its passes go in the frame's open encoder.
    fn record_encoded(
        &mut self,
        t: &d3d::backend::ExternalAttachments,
        request: d3d::ExternalPassRequest,
    ) -> Option<Result<d3d::backend::ExternalEncoded, &'static str>> {
        Some(match self.prepare(PostCopy::of_attachments(t, request)) {
            Ok(commands) => Ok(d3d::backend::ExternalEncoded {
                ranges: commands.ranges(),
                draws: commands.draws(),
                passes: commands.passes(),
                commands: Box::new(move |encoder: &mut wgpu::CommandEncoder| {
                    commands.record(encoder)
                }),
            }),
            Err(error) => {
                self.refused(error);
                Err("screen effect refused on the live target")
            }
        })
    }
}

impl d3d::SwallowPass for ScreenPass {
    fn validate(
        &self,
        originals: &[d3d::SwallowOriginal<'_>],
        slots: &[Option<d3d::SwallowTarget>],
    ) -> Result<(), String> {
        let mut originals = originals.iter();
        let mut drawn = screen_gpu::Drawn::default();
        let mut raster = Vec::with_capacity(self.call.passes.len());
        for (key, dest, strips, convolutions) in &self.call.passes {
            let program = self.effect(*key).ok_or("missing screen program")?;
            let reads = drawn.reads(program, *dest).map_err(|e| format!("{e:?}"))?;
            let mut pass_raster = None;
            for strip in strips {
                let original = originals.next().ok_or("missing screen strip")?;
                if !matches_strip(original.draw, strip) {
                    return Err(
                        "the resolved screen strip differs from the planned vertices".into(),
                    );
                }
                let mut live = post_raster(original.draw, Some(program.combiner_inputs))
                .map_err(|why| format!("screen effect {:#010x}: {why}", self.call.egor.0))?;
                for (stage, source) in reads.iter().enumerate() {
                    if let Some(index) = source {
                        let is_view = *index == 0;
                        check_bound_source(original, stage, screen_slot(*index), slots, is_view)?;
                        if is_view {
                            live.opaque_view[stage] = original.draw.texture_info[stage]
                                .is_some_and(|info| info.format == gfx::D3DFormat::LinX8R8G8B8);
                        }
                    }
                }
                if pass_raster.replace(live).is_some_and(|first| first != live) {
                    return Err("screen strip raster state changed within the pass".into());
                }
            }
            let strip = pass_raster.ok_or("the screen pass has no strips")?;
            let mut convolution_rasters = Vec::new();
            for q in convolutions.iter().flat_map(|c| &c.draws) {
                let frame = self.call.frame.as_ref().ok_or("missing convolution frame")?;
                let inputs = CombinerInputs::read(&shaders::PixelShaderDef::from_words(&frame.combiner));
                let mut grid_raster = None;
                for _ in 0..q.grid.as_ref().map_or(1, |g| g.strips.len()) {
                    let original = originals.next().ok_or("missing convolution draw")?;
                    let live = post_raster(original.draw, Some(inputs))
                        .map_err(|why| format!("screen convolution into {}: {why}", q.dest))?;
                    if grid_raster.replace(live).is_some_and(|first| first != live) {
                        return Err("screen convolution raster state changed within the grid".into());
                    }
                    for stage in 0..4 {
                        if original.sampled[stage] {
                            check_source(original, stage, screen_slot(q.source), slots)?;
                        }
                    }
                }
                convolution_rasters.push(grid_raster.ok_or("the convolution has no strips")?);
            }
            raster.push(ScreenRaster { strip, convolutions: convolution_rasters });
            drawn
                .pass(program, *dest, convolutions)
                .map_err(|e| format!("{e:?}"))?;
        }
        if originals.next().is_some() {
            return Err("unexpected post packet".into());
        }
        self.raster
            .set(raster)
            .map_err(|_| "screen raster was already captured".to_owned())
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
    check_bound_source(original, stage, slot, slots, false)
}

pub(crate) fn check_bound_source(
    original: &d3d::SwallowOriginal<'_>,
    stage: usize,
    slot: Option<u8>,
    slots: &[Option<d3d::SwallowTarget>],
    opaque_entry: bool,
) -> Result<(), String> {
    let target = slot
        .and_then(|i| slots.get(usize::from(i)))
        .copied()
        .flatten();
    let info = original.draw.texture_info[stage];
    let extras = original.draw.extras[stage];
    if let (Some(target), Some(info)) = (target, info)
        && original.source_data[stage] == Some(target.data)
        && target.size == (info.width, info.height)
        && info.levels == 1
        && extras.color_key_op == 0
        && extras.color_sign == 0
        && extras.alpha_kill == 0
        && info.kind == gfx::texture::TextureKind::D2
        && (matches!(
            info.format,
            gfx::D3DFormat::LinA8R8G8B8 | gfx::D3DFormat::A8R8G8B8
        ) || (opaque_entry && info.format == gfx::D3DFormat::LinX8R8G8B8))
    {
        return Ok(());
    }
    Err(format!(
        "live source for stage {stage} differs from post slot {slot:?} ({info:?})"
    ))
}

fn matches_strip(draw: &d3d::ResolvedDraw, strip: &[StripVertex]) -> bool {
    use gfx::vertex::VertexDataType::Float4;
    let ([layout], [stream]) = (draw.layout.streams.as_slice(), draw.streams.as_slice()) else {
        return false;
    };
    if layout.stream != 0
        || layout.stride != 256
        || stream.stride != 256
        || draw.layout.pretransformed
        || draw.indices.is_some()
        || draw.vertex_count as usize != strip.len()
        || stream.data.len() != strip.len() * 256
        || draw.shader.provided_inputs != 0x1f
        || layout.attributes.len() != 5
    {
        return false;
    }
    for register in 0..5u8 {
        if !layout.attributes.iter().any(|a| {
            a.register.0 == register
                && a.data_type == Float4
                && a.offset == 16 * u32::from(register)
        }) {
            return false;
        }
    }
    // Compare the issued attribute words, including signed zero and NaNs.
    strip
        .iter()
        .zip(stream.data.chunks_exact(256))
        .all(|(vertex, issued)| {
            let mut expected = std::iter::once(vertex.position)
                .chain(vertex.texcoords)
                .flatten()
                .flat_map(f32::to_le_bytes);
            issued[..80]
                .iter()
                .copied()
                .all(|byte| expected.next() == Some(byte))
        })
}

// A colour-only recording can omit a bound depth attachment if all fragments
// pass and neither depth nor stencil can be changed.
fn post_needs_depth(state: &gfx::RenderState, format: Option<wgpu::TextureFormat>) -> bool {
    use gfx::state::{CompareFunc, DepthBufferMode, StencilOp};

    let Some(format) = format else { return false };
    let depth_active = state.depth_mode != DepthBufferMode::Disabled
        && (state.depth_write || state.depth_func != CompareFunc::Always);
    let stencil = &state.stencil;
    let stencil_active = format.has_stencil_aspect()
        && stencil.enable
        && (stencil.func != CompareFunc::Always
            || (stencil.write_mask != 0 && stencil.pass != StencilOp::Keep));
    depth_active || stencil_active
}

// The ordinary programmable path consumes these same decoded defaults when
// a render word fails to decode. Only sampled-stage sampler defaults can
// differ from the native template's static state.
fn post_sampler_default_is_uncertain(draw: &d3d::ResolvedDraw) -> bool {
    use d3d::state::BadState;
    use gfx::state::TextureStageStateType as S;
    let sampled = draw.sampled_texture_stages();
    draw.state.bad.iter().any(|bad| match *bad {
        BadState::Render { .. } => false,
        BadState::Stage { stage, state, .. } => {
            sampled.get(stage as usize).copied().unwrap_or(false)
                && matches!(S::from_index(state),
                    S::AddressU | S::AddressV | S::AddressW | S::MagFilter
                    | S::MinFilter | S::MipFilter | S::MipMapLodBias
                    | S::MaxMipLevel | S::MaxAnisotropy | S::BorderColor)
        }
    })
}

fn check_combiner_raster(
    state: &gfx::RenderState,
    two_sided: bool,
    inputs: CombinerInputs,
) -> Result<(), &'static str> {
    if state.fog_enable && inputs.fog {
        return Err("the post combiner reads live fog");
    }
    if inputs.colours {
        if state.shade_mode == gfx::state::ShadeMode::Flat {
            return Err("the post combiner reads flat-shaded vertex colours");
        }
        if two_sided {
            return Err("the post combiner reads two-sided vertex colours");
        }
    }
    Ok(())
}

fn live_combiner_inputs(draw: &d3d::ResolvedDraw) -> CombinerInputs {
    let Some(def) = draw.shader.pixel_shader.as_deref() else {
        return CombinerInputs { colours: true, fog: true };
    };
    CombinerInputs::read(&d3d::nv2a::guest_def(
        def,
        &draw.shader.combiner_state,
        draw.raw.render[d3d::state::slot::PS_TEXTURE_MODES as usize],
        draw.shader.final_combiner_latch,
    ))
}

/// The template supplies shader code. Colour state can be inherited from
/// the previous draw or applied by the template wrapper outside that code.
fn post_raster(
    draw: &d3d::ResolvedDraw,
    planned_inputs: Option<CombinerInputs>,
) -> Result<PostRaster, &'static str> {
    use gfx::state::{CompareFunc, FillMode};
    let state = &draw.state.render;
    let alpha_test = (state.alpha_test_enable && state.alpha_func != CompareFunc::Always)
        .then(|| (d3d::nv2a::alpha_func(state.alpha_func), state.alpha_ref));
    if post_needs_depth(state, draw.target.depth_format.and_then(gfx::format::depth_format_to_wgpu)) {
        return Err("the post draw requires its live depth/stencil attachment");
    }
    // Build 5849's STIPPLEENABLE and TWOSIDEDLIGHTING slots.
    const STIPPLE_ENABLE: usize = 83;
    const TWO_SIDED_LIGHTING: usize = 141;
    let stipple = state.fill_mode == FillMode::Solid
        && draw.hw.stipple_control.unwrap_or(draw.raw.render[STIPPLE_ENABLE]) != 0;
    if planned_inputs.is_none() && stipple {
        return Err("the colour quad requires the polygon stipple mask");
    }
    let two_sided = draw.raw.render[TWO_SIDED_LIGHTING] != 0;
    if let Some(planned) = planned_inputs {
        // A planned combiner's fog register is the live draw's, fed into
        // its uniforms (`PostRaster::fog`): live fog refuses nothing.
        let fed = |inputs: CombinerInputs| CombinerInputs { fog: false, ..inputs };
        check_combiner_raster(state, two_sided, fed(planned))?;
        if two_sided || state.shade_mode == gfx::state::ShadeMode::Flat {
            check_combiner_raster(state, two_sided, fed(live_combiner_inputs(draw)))?;
        }
        if post_sampler_default_is_uncertain(draw) {
            return Err("the post draw has an invalid sampled-stage sampler state");
        }
    } else {
        check_combiner_raster(state, two_sided, CombinerInputs { colours: true, fog: true })?;
        if !draw.state.bad.is_empty() {
            return Err("the colour quad requires a rejected guest state");
        }
    }
    // The post pipelines use the host translator's front fill. The other
    // colour-quad clients build fixed solid pipelines.
    if planned_inputs.is_none()
        && (state.fill_mode != FillMode::Solid || state.back_fill_mode != FillMode::Solid)
    {
        return Err("the colour quad requires solid front and back fill");
    }
    if draw.ui_fit.is_some() || draw.retail_constants.is_some() {
        return Err("the post draw requires copy-specific projection");
    }
    let clip = draw
        .clip
        .filter(|_| draw.clip_rects.is_empty())
        .ok_or("the screen clip is disjoint or empty")?;
    let primitive = gfx::translate::primitive_state(
        state,
        wgpu::PrimitiveTopology::TriangleStrip,
        gfx::translate::Capabilities::from_features(wgpu::Features::empty()),
    );
    let z_scale = gfx::format::d3dz_max(draw.target.depth_format);
    Ok(PostRaster {
        alpha_test,
        stipple: stipple.then_some(draw.hw.stipple_pattern),
        z_scale: Some(z_scale),
        w_clip: d3d::hw::w_buffered(draw).then(|| d3d::hw::w_clip_range(&draw.hw, z_scale)),
        blend: gfx::translate::blend_state(state),
        blend_constant: gfx::translate::blend_constant(state),
        writes: gfx::translate::color_writes(state.color_write),
        front_face: primitive.front_face,
        cull: primitive.cull_mode,
        depth_clamp: d3d::hw::draw_clamps_depth(draw),
        fill: state.fill_mode,
        opaque_view: [false; 4],
        clip: (!clip.is_whole(draw.target.width, draw.target.height)).then_some([
            clip.x,
            clip.y,
            clip.width,
            clip.height,
        ]),
        fog: planned_inputs.map(|_| d3d::nv2a::draw_fog(draw)),
    })
}

pub(crate) fn matches_colour_state(
    draw: &d3d::ResolvedDraw,
    blend: Option<wgpu::BlendState>,
    writes: wgpu::ColorWrites,
    alpha: Option<(wgpu::CompareFunction, u8)>,
) -> bool {
    let state = &draw.state.render;
    let live_alpha = (state.alpha_test_enable && state.alpha_func != gfx::state::CompareFunc::Always)
        .then(|| (gfx::translate::compare_function(state.alpha_func), state.alpha_ref));
    live_alpha == alpha && post_raster(draw, None).is_ok_and(|r| {
        r.blend == blend && r.writes == writes && r.cull.is_none() && r.clip.is_none()
    })
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

/// The swallow's slot per quad: the view 0, target 7 1, target 8 2.
pub fn targets(plan: &Plan) -> Vec<u8> {
    plan.draws.iter().map(|d| glow_slot(d.dest)).collect()
}

fn glow_slot(surface: Surface) -> u8 {
    match surface {
        Surface::View => 0,
        Surface::Down => 1,
        Surface::Other => 2,
    }
}

fn glow_headers(mem: &dyn GuestMemory) -> Vec<Option<u32>> {
    vec![
        None, // The swallow pins the view from the bound target.
        target_level_surface(mem, glow::TARGET_DOWN),
        target_level_surface(mem, glow::TARGET_OTHER),
    ]
}

/// A quad of `0x00034420`: four vertices between pushed Begin(7) and End.
pub const QUAD: (PrimitiveType, usize) = (PrimitiveType::TriangleFan, 4);

/// The native glow, recorded against the call's view and blur levels.
struct GlowPass {
    raster: OnceLock<Vec<PostRaster>>,
    /// Per stage, whether the down-sample's entry reads the view as `X8`
    /// (alpha one), captured with the raster.
    opaque_entry: OnceLock<[bool; 4]>,
    plan: Plan,
    gpu: Arc<Mutex<GpuGlow>>,
    entry: Arc<Mutex<OpaqueEntry>>,
}

impl d3d::ExternalPass for GlowPass {
    fn colour_only(&self) -> bool {
        true
    }

    // Frozen by `validate`: the plan, the raster and the entry's format
    // are the call's own, and a copy given no entry textures reads the
    // view from its own slot. So a scene replay can record it again on
    // its attachments (docs/graphics/framescene.md, slice 5).
    fn replays_scene(&self) -> bool {
        self.raster.get().is_some() && self.opaque_entry.get().is_some()
    }

    fn encode(&mut self, _: d3d::ExternalTarget<'_>) {}

    fn record_ranges(&mut self, t: d3d::ExternalTarget<'_>) -> usize {
        match self.prepare(PostCopy::of_target(&t)) {
            Some(recording) => {
                recording.record(t.encoder);
                self.plan.draws.len()
            }
            None => 0,
        }
    }

    // The blur levels are targets of the glow's own: its quads go in the
    // frame's open encoder.
    fn record_encoded(
        &mut self,
        t: &d3d::backend::ExternalAttachments,
        request: d3d::ExternalPassRequest,
    ) -> Option<Result<d3d::backend::ExternalEncoded, &'static str>> {
        Some(match self.prepare(PostCopy::of_attachments(t, request)) {
            Some(recording) => Ok(d3d::backend::ExternalEncoded {
                ranges: self.plan.draws.len(),
                draws: recording.passes(),
                passes: recording.passes(),
                commands: Box::new(move |encoder: &mut wgpu::CommandEncoder| {
                    recording.record(encoder)
                }),
            }),
            None => Err("glow refused on the live target"),
        })
    }
}

impl GlowPass {
    /// The plan's quads on one copy, prepared; `None` (logged) when
    /// refused. The cache's lock is released when this returns.
    ///
    /// The down-sample's entry reads the view: through the copy's stage
    /// textures when it has them (the device's resolved inputs, with
    /// the `X8` alpha already one), else from the view's own slot, made
    /// opaque first where the guest read it as `X8`. A carried scene
    /// stage and its replay have no stage textures.
    fn prepare(&self, t: PostCopy<'_>) -> Option<GlowRecording> {
        let view = slot(t.slots, 0)?;
        let raster = self.raster.get()?;
        let opaque = *self.opaque_entry.get()?;
        let intermediates = [1, 2].map(|i| slot(t.slots, i).map(glow::PostSurface::from));
        let given = t.stages.iter().any(Option::is_some);
        let entry = (!given && opaque.contains(&true))
            .then(|| self.entry.lock().unwrap_or_else(|e| e.into_inner()).copy(t.device, &view.attachment));
        let entry_sources = if given {
            t.stages
        } else {
            std::array::from_fn(|n| {
                Some(match (&entry, opaque[n]) {
                    (Some(copy), true) => &copy.view,
                    _ => &view.attachment,
                })
            })
        };
        let result = self.gpu.lock().unwrap_or_else(|e| e.into_inner()).prepare_raster(
            GlowTarget {
                device: t.device,
                queue: t.queue,
                uploads: t.uploads,
                color: view.into(),
                entry_sources,
                intermediates,
            },
            &self.plan,
            Some(raster),
        );
        result
            .map(|commands| GlowRecording { entry, commands })
            .map_err(|error| log_rejection(|| format!("glow on live target: {error:?}")))
            .ok()
    }
}

/// One copy's glow: the opaque entry it makes first, when it makes one,
/// then the quads.
struct GlowRecording {
    entry: Option<EntryCopy>,
    commands: glow::GlowCommands,
}

impl GlowRecording {
    fn passes(&self) -> usize {
        self.commands.passes() + usize::from(self.entry.is_some())
    }

    fn record(&self, encoder: &mut wgpu::CommandEncoder) {
        if let Some(entry) = &self.entry {
            entry.record(encoder);
        }
        self.commands.record(encoder);
    }
}

/// The view as an `X8` read sees it, for a copy with no stage textures:
/// the view copied, then the copy's alpha set to one, as the device's
/// framebuffer snapshot of an `X8` level does.
struct EntryCopy {
    source: wgpu::Texture,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    pipeline: wgpu::RenderPipeline,
}

impl EntryCopy {
    fn record(&self, encoder: &mut wgpu::CommandEncoder) {
        encoder.copy_texture_to_texture(
            self.source.as_image_copy(),
            self.texture.as_image_copy(),
            self.source.size(),
        );
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("native glow X8 entry"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.draw(0..3, 0..1);
    }
}

/// The opaque entry's textures and its alpha pipeline, kept by device,
/// size and format. Every recording writes its copy before its quads
/// read it, in queue order, so recordings of one size share a texture.
#[derive(Default)]
struct OpaqueEntry {
    pipelines: Vec<(wgpu::Device, wgpu::TextureFormat, wgpu::RenderPipeline)>,
    textures: Vec<(wgpu::Device, wgpu::Texture, wgpu::TextureView)>,
}

impl OpaqueEntry {
    /// The kept textures: one per live copy of the view (the reference
    /// and the display), with room for a resize.
    const KEPT: usize = 4;

    fn copy(&mut self, device: &wgpu::Device, view: &wgpu::TextureView) -> EntryCopy {
        let source = view.texture().clone();
        let (size, format) = (source.size(), source.format());
        let at = self.textures.iter().position(|(d, t, _)| {
            d == device && t.size() == size && t.format() == format
        });
        let (texture, view) = match at {
            Some(at) => {
                let (_, t, v) = &self.textures[at];
                (t.clone(), v.clone())
            }
            None => {
                let texture = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("native glow X8 entry"),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::COPY_DST
                        | wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                });
                let view = texture.create_view(&Default::default());
                if self.textures.len() == Self::KEPT {
                    self.textures.remove(0);
                }
                self.textures.push((device.clone(), texture.clone(), view.clone()));
                (texture, view)
            }
        };
        EntryCopy {
            source,
            texture,
            view,
            pipeline: self.pipeline(device, format),
        }
    }

    fn pipeline(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat) -> wgpu::RenderPipeline {
        if let Some((_, _, p)) = self.pipelines.iter().find(|(d, f, _)| d == device && *f == format) {
            return p.clone();
        }
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("native glow X8 entry"),
            source: wgpu::ShaderSource::Wgsl(
                r#"
                @vertex fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
                    let x = f32((i << 1u) & 2u);
                    let y = f32(i & 2u);
                    return vec4f(2.0 * x - 1.0, 2.0 * y - 1.0, 0.0, 1.0);
                }
                @fragment fn fs_main() -> @location(0) vec4f {
                    return vec4f(0.0, 0.0, 0.0, 1.0);
                }
            "#
                .into(),
            ),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("native glow X8 entry"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALPHA,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        self.pipelines.push((device.clone(), format, pipeline.clone()));
        pipeline
    }
}

impl d3d::SwallowPass for GlowPass {
    fn entry_samples(&self) -> bool {
        true
    }

    fn validate(
        &self,
        originals: &[d3d::SwallowOriginal<'_>],
        slots: &[Option<d3d::SwallowTarget>],
    ) -> Result<(), String> {
        if originals.len() != self.plan.draws.len() {
            return Err("glow quad count differs from plan".into());
        }
        let mut rasters = Vec::with_capacity(originals.len());
        let mut opaque_entry = [false; 4];
        for (i, (original, want)) in originals.iter().zip(&self.plan.draws).enumerate() {
            let inputs = CombinerInputs::read(&shaders::PixelShaderDef::from_words(&want.combiner));
            let raster = post_raster(original.draw, Some(inputs))?;
            rasters.push(raster);
            for stage in 0..4 {
                if original.sampled[stage] {
                    let source = want.stages[stage];
                    let entry = i == 0 && source == Some(Surface::View);
                    check_bound_source(original, stage, source.map(glow_slot), slots, entry)?;
                    opaque_entry[stage] = entry
                        && original.draw.texture_info[stage]
                            .is_some_and(|info| info.format == gfx::D3DFormat::LinX8R8G8B8);
                }
            }
        }
        self.opaque_entry
            .set(opaque_entry)
            .map_err(|_| "glow entry already captured".to_owned())?;
        self.raster.set(rasters).map_err(|_| "glow raster already captured".to_owned())
    }
}

struct Glue<B: d3d::backend::Backend + Send + 'static> {
    device: d3d::abi::SharedDevice<B>,
    gpu: Arc<Mutex<GpuGlow>>,
    /// The glow's opaque entry copies ([`OpaqueEntry`]).
    entry: Arc<Mutex<OpaqueEntry>>,
    /// The current map's screen effects, loaded here whatever the native
    /// world does ([`EffectsSlot`]).
    effects: Arc<Mutex<EffectsSlot>>,
}

/// Read the live pass and both rows after the template sets its state.
fn live_strip(mem: &dyn GuestMemory, esp: u32, mxcsr: u32) -> Option<Vec<[[f32; 4]; 5]>> {
    let word = |offset| mem.u32(esp.checked_add(offset)?).ok();
    let columns = word(0xb0)?;
    if !(2..=MAX_CORNERS / 2).contains(&columns) {
        return None;
    }
    let row = word(0x18)?;
    let row_bytes = word(0x34)?;
    if row == 0 || row_bytes != columns * 16 {
        return None;
    }
    let pass = word(0x20)?;
    let bounds = [word(0x58)?, word(0x5c)?, word(0x60)?, word(0x64)?];
    let corner = |a: u32| {
        Some([
            mem.u32(a).ok()?,
            mem.u32(a.checked_add(4)?).ok()?,
            mem.u32(a.checked_add(8)?).ok()?,
            mem.u32(a.checked_add(12)?).ok()?,
        ])
    };
    let rows = (0..columns)
        .map(|i| {
            let a = row.checked_add(i * 16)?;
            Some([corner(a)?, corner(a.checked_add(row_bytes)?)?])
        })
        .collect::<Option<Vec<_>>>()?;
    let depth = f32::from_bits(mem.u32(screen_effect::constant::DEPTH).ok()?);
    let vertices = screen_effect::strip_vertices(mem, pass, bounds, &rows, mxcsr).ok()?;
    Some(
        vertices
            .into_iter()
            .map(|v| {
                let [x, y] = v.position.map(f32::from_bits);
                let [t0, t1, t2, t3] = v.texcoords.map(|t| t.map(f32::from_bits));
                [[x, y, depth, depth], t0, t1, t2, t3]
            })
            .collect(),
    )
}

impl<B: d3d::backend::Backend + Send + 'static> Host for Glue<B> {
    fn retire_map(&self) {
        let retired = self.effects.lock().unwrap_or_else(|e| e.into_inner())
            .take_stale(render_bridge::native_map_work::generation());
        if let Some(retired) = retired {
            crate::native_map::retire_owned(timeline::setup::Stage::World, retired);
        }
    }

    fn draw_strip(
        &self,
        mem: &mut dyn GuestMemory,
        esp: u32,
        call_esp: u32,
        mxcsr: u32,
    ) -> Result<bool, runtime::GuestFault> {
        use timeline::native::{Metric, Stage};
        let Ok(mut device) = self.device.lock() else {
            return Ok(false);
        };
        let entry = call_entry(PostCall::Screen);
        let end = post_feature::SCREEN_STRIP_TAIL;
        Stage::Post.count(Metric::WalkExamined, 1);
        if !device.owns_post_strip(&DeviceMemory(mem), entry, call_esp, end) {
            Stage::Post.count(Metric::WalkRefusedReservation, 1);
            return Ok(false);
        }
        let Some(vertices) = live_strip(mem, esp, mxcsr) else {
            Stage::Post.count(Metric::WalkRefusedInputs, 1);
            return Ok(false);
        };
        match device.try_post_strip(&mut DeviceMemory(mem), entry, call_esp, end, &vertices) {
            Ok(drawn) => {
                if !drawn {
                    Stage::Post.count(Metric::WalkRefusedReservation, 1);
                }
                Ok(drawn)
            }
            Err(error) => {
                eprintln!("native-post: issuing screen strip: {error}");
                Err(runtime::GuestFault {
                    addr: post_feature::SCREEN_STRIP_TAIL,
                    len: 0,
                    access: runtime::guest::Access::Execute,
                    reason: runtime::guest::FaultReason::InvalidOperation,
                })
            }
        }
    }

    fn colour_pass(
        &self,
        mem: &mut dyn GuestMemory,
        site: CallSite,
        args: [u32; 7],
        fp: Fp,
    ) -> bool {
        use render_bridge::native_hud::kept;
        self.interrupt();
        let plan = match plan(&*mem, args, fp) {
            Ok(Some(p)) => p,
            Ok(None) => {
                // The glow is off: the call draws nothing.
                kept::empty();
                return false;
            }
            Err(error) => {
                kept::refuse("plan");
                log_rejection(|| format!("glow plan: {error}"));
                return false;
            }
        };
        let targets = targets(&plan);
        let pass: Box<dyn d3d::SwallowPass> = Box::new(GlowPass {
            raster: OnceLock::new(),
            opaque_entry: OnceLock::new(),
            plan,
            gpu: Arc::clone(&self.gpu),
            entry: Arc::clone(&self.entry),
        });
        let headers = glow_headers(mem);
        let armed = self.arm(
            mem,
            d3d::Swallow {
                call: Some(swallow_call(site)),
                end_returns: Vec::new(),
                headers,
                targets,
                primitive: QUAD.0,
                vertices: QUAD.1,
                shapes: Vec::new(),
                drop: true,
                pass,
            },
        );
        if !armed {
            kept::refuse("arm");
        }
        armed
    }

    fn screen_effect(
        &self,
        mem: &mut dyn GuestMemory,
        site: CallSite,
        call: ScreenEffect,
        fp: Fp,
    ) -> bool {
        self.interrupt();
        self.effect(mem, site, call, fp)
    }

    fn finish(&self, mem: &mut dyn GuestMemory, kind: PostCall, esp: u32) {
        if let Ok(mut device) = self.device.lock() {
            let before = device.stats();
            if let Err(error) = device.finish_swallow(&mut DeviceMemory(mem), call_entry(kind), esp)
            {
                eprintln!("native-post: finishing {kind:?}: {error}");
            }
            let after = device.stats();
            if after.swallow_refusals > before.swallow_refusals
                && let Some(detail) = device.swallow_mismatch()
            {
                log_rejection(|| format!("{kind:?}: {detail}"));
            }
        }
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

    /// [`Host::screen_effect`]'s body.
    fn effect(
        &self,
        mem: &mut dyn GuestMemory,
        site: CallSite,
        call: ScreenEffect,
        fp: Fp,
    ) -> bool {
        use render_bridge::native_hud::kept::refuse;
        let (mut effect, values) = match plan_effect(&*mem, call, fp) {
            Ok(Some(e)) => e,
            Ok(None) => {
                refuse("unplanned");
                return false;
            }
            Err(e) => {
                refuse("plan");
                if !EFFECT_PLAN_LOGGED.swap(true, Ordering::Relaxed) {
                    eprintln!(
                        "native-post: a screen effect keeps its D3D draws: {e} (logged once)"
                    );
                }
                return false;
            }
        };
        let effects = self.screen_effects(&*mem);
        let egor = TagId(call.datum);
        let skipped = skip_undrawn(&*mem, &mut effect);
        let ordered = call_ordered(effects.as_deref(), egor, &effect);
        if effects.is_none() {
            // The first call on a map starts their load: nothing is
            // missing yet, they are not in.
            refuse("loading");
            return false;
        }
        let frame = quad_frame(&*mem, fp);
        let mut m = missing(
            &effect,
            &|p| pass_drawable(effects.as_deref(), egor, p),
            frame.is_some(),
        );
        if let Err(reason) = ordered && !m.contains(&reason) {
            m.push(reason);
        }
        if skipped.is_none() {
            m.push(Missing::TemplateSkipped);
        }
        for m in &m {
            refused(*m);
        }
        // Armed only once nothing is missing (never while the stand-ins
        // are); until then the D3D path draws every effect.
        if !m.is_empty() {
            return false;
        }
        let Some(effects) = effects else { return false };
        let Ok(depth) = mem.u32(screen_effect::constant::DEPTH) else {
            refuse("effect_depth");
            return false;
        };
        let grid = |p: &screen_effect::Pass, a: &WideArgs, size: (i32, i32), d: &WideDraw| {
            grid_of(
                &*mem,
                fp,
                &wide_grid(a, size, d, wide_source(&*mem, p, a.source)?),
            )
        };
        let Some(call) = screen_call(&effect, egor, values, depth, frame, &grid) else {
            refused(Missing::Wide);
            return false;
        };
        let Some(plan) = screen_swallow(&call) else {
            refuse("swallow");
            return false;
        };
        let (targets, shapes) = plan;
        let (primitive, vertices) = shapes[0];
        let end_returns = screen_end_returns(&call);
        // Target 0 comes from a device surface, outside the level table.
        // Its first strip pins SLOT_BOUND after the guest binds it.
        let headers = [None, None, Some(1), Some(0x19), Some(0x1a), Some(0x1b)]
            .map(|index| index.and_then(|index| target_level_surface(mem, index)))
            .to_vec();
        let pass = Box::new(ScreenPass {
            effects,
            call,
            raster: OnceLock::new(),
        });
        let armed = self.arm(
            mem,
            d3d::Swallow {
                call: Some(swallow_call(site)),
                end_returns,
                headers,
                targets,
                primitive,
                vertices,
                shapes,
                drop: true,
                pass,
            },
        );
        if !armed {
            refuse("arm");
        }
        armed
    }
}

impl<B: d3d::backend::Backend + Send + 'static> Glue<B> {
    /// The loaded map's screen effects: `None` until they load, the first
    /// call on a map starting their load ([`spawn_load`]).
    fn screen_effects(&self, mem: &dyn GuestMemory) -> Option<Arc<ScreenEffects>> {
        map_effects(&self.effects, mem, &mut |name, generation| {
            spawn_load(name, generation, Arc::clone(&self.effects))
        })
    }

    fn arm(&self, mem: &mut dyn GuestMemory, swallow: d3d::Swallow) -> bool {
        let Ok(mut device) = self.device.lock() else {
            return false;
        };
        device
            .swallow(&mut DeviceMemory(mem), swallow)
            .unwrap_or_else(|error| {
                eprintln!("native-post: arming the call: {error}");
                false
            })
    }
}

/// The screen effects of the map the game has loaded, which the post
/// glue loads itself: the hook runs with the native post on, whether or
/// not the native world (whose load used to carry them) is. The slot is
/// keyed by the map's name and its load generation
/// (`native_map_work::generation`), so a map opened again, as an edit
/// that reloads it does, loads its effects again from the file.
#[derive(Default)]
pub struct EffectsSlot {
    map: Option<(String, u64)>,
    effects: Option<Arc<ScreenEffects>>,
}

impl EffectsSlot {
    fn take_stale(&mut self, generation: u64) -> Option<Arc<ScreenEffects>> {
        if self.map.as_ref().is_some_and(|(_, old)| *old != generation) {
            self.map = None;
            return self.effects.take();
        }
        None
    }

    /// Map `name`'s effects at load `generation` once loaded, and whether
    /// a load of them must start now: at the first ask on a map load,
    /// which forgets the last one's.
    pub fn ask(&mut self, name: &str, generation: u64) -> (Option<Arc<ScreenEffects>>, bool) {
        if self
            .map
            .as_ref()
            .is_some_and(|(n, g)| n == name && *g == generation)
        {
            return (self.effects.clone(), false);
        }
        self.map = Some((name.to_string(), generation));
        self.effects = None;
        (None, true)
    }

    /// The load of map `name` at `generation` finished: kept while it is
    /// still the map load; whether it was.
    pub fn loaded(&mut self, name: &str, generation: u64, effects: ScreenEffects) -> bool {
        if !self
            .map
            .as_ref()
            .is_some_and(|(n, g)| n == name && *g == generation)
        {
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
    load: &mut dyn FnMut(String, u64),
) -> Option<Arc<ScreenEffects>> {
    let name = crate::health::loaded_map_name(mem)
        .ok()
        .filter(|n| !n.is_empty())?;
    let generation = render_bridge::native_map_work::generation();
    let (effects, start) = slot
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .ask(&name, generation);
    if start {
        load(name, generation);
    }
    effects
}

/// Loads map `name`'s screen effects on a thread of their own into
/// `slot`; a map that does not load keeps every effect on D3D.
fn spawn_load(name: String, generation: u64, slot: Arc<Mutex<EffectsSlot>>) {
    let spawned = background::spawn("native-post load", move || {
        let current = || generation == render_bridge::native_map_work::generation();
        let Some(_load) = native_map::begin_load(timeline::setup::Stage::World, current) else { return };
        let _scope = mem_watch::scope("native.post.load");
        match load_effects(&name) {
            Ok(e) if current() => {
                slot.lock().unwrap_or_else(|e| e.into_inner()).loaded(&name, generation, e);
            }
            Ok(_) => {}
            Err(e) => eprintln!("native-post: {name}: the screen effects did not load ({e}); every screen effect keeps its D3D draws"),
        }
    });
    if let Err(e) = spawned {
        eprintln!(
            "native-post: {e}: the screen effects did not load; every screen effect keeps its D3D draws"
        );
    }
}

/// Loads the map's screen effects with the raw resources served by the VFS.
fn load_effects(name: &str) -> Result<ScreenEffects, native_render::load::Error> {
    let (mut map, mut files) = native_map::open(name)?;
    Ok(crate::native_post_load::load(&mut map, &mut files))
}

/// Installs the host over `device`. It loads each map's screen effects
/// itself ([`EffectsSlot`]).
pub fn install<B: d3d::backend::Backend + Send + 'static>(device: &d3d::abi::SharedDevice<B>) {
    crate::h1_cinematics::install_device(device);
    render_bridge::native_post::set_host(Arc::new(Glue {
        device: Arc::clone(device),
        gpu: Arc::new(Mutex::new(GpuGlow::new())),
        entry: Arc::new(Mutex::new(OpaqueEntry::default())),
        effects: Arc::new(Mutex::new(EffectsSlot::default())),
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The colour pass's stand-in now that the A/B capture is gone: it
    /// draws nothing and binds no depth.
    struct CapturePass(Option<String>);

    impl d3d::ExternalPass for CapturePass {
        fn colour_only(&self) -> bool {
            true
        }

        fn encode(&mut self, t: d3d::ExternalTarget<'_>) {
            let _ = (&self.0, t.device.poll(wgpu::PollType::Poll));
        }
    }

    impl d3d::SwallowPass for CapturePass {}

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
    fn a_live_strip_reads_paired_rows_and_refuses_an_inconsistent_stride() {
        use engine::memory::FlatMemory;
        let mut mem = FlatMemory::new(0x0040_0000, 0x0010_0000);
        let (stack, pass, corners) = (0x0048_0000, 0x0049_0000, 0x004a_0000);
        for (offset, word) in [(0xb0, 2), (0x18, corners), (0x34, 32), (0x20, pass)] {
            mem.set_u32(stack + offset, word).unwrap();
        }
        mem.set_u32(screen_effect::constant::ONE, 1.0f32.to_bits())
            .unwrap();
        mem.set_u32(screen_effect::constant::DEPTH, 0.5f32.to_bits())
            .unwrap();
        for (i, corner) in [
            [1.0f32, 2.0, 0.25, 0.5],
            [3.0, 2.0, 0.75, 0.5],
            [1.0, 4.0, 0.25, 1.0],
            [3.0, 4.0, 0.75, 1.0],
        ]
        .into_iter()
        .enumerate()
        {
            for (n, value) in corner.into_iter().enumerate() {
                mem.set_u32(corners + 16 * i as u32 + 4 * n as u32, value.to_bits())
                    .unwrap();
            }
        }
        let strip = live_strip(&mem, stack, 0x1f80).unwrap();
        assert_eq!(
            strip
                .iter()
                .map(|registers| registers[0][..2].to_vec())
                .collect::<Vec<_>>(),
            [
                vec![1.0, 2.0],
                vec![1.0, 4.0],
                vec![3.0, 2.0],
                vec![3.0, 4.0],
            ]
        );
        assert_eq!(strip[1][0], [1.0, 4.0, 0.5, 0.5]);
        assert_eq!(strip[1][4], [0.25, 1.0, 0.0, 1.0]);
        mem.set_u32(stack + 0x34, 16).unwrap();
        assert!(live_strip(&mem, stack, 0x1f80).is_none());
    }

    #[test]
    fn the_colour_swallow_matches_the_embedded_surfaces_in_pass_order() {
        use d3d::GuestMemory as _;
        use d3d::pushbuf::method;
        use d3d::resource::{ResourceType, common};
        use engine::memory::FlatMemory;
        use gfx::vertex::fvf;

        let plan = Plan {
            window: [0.0; 4],
            signs: [0.0; 2],
            view: (32, 32),
            down: (16, 16),
            other: (16, 16),
            draws: [
                Surface::Down,
                Surface::Other,
                Surface::Down,
                Surface::Other,
                Surface::Down,
                Surface::View,
            ]
            .map(draw)
            .to_vec(),
        };
        let call = d3d::SwallowCall {
            entry: call_entry(PostCall::Colour),
            esp: 0x0043_0100,
            ret: 0x2000,
        };
        let config = d3d::DeviceConfig::halo2(
            d3d::GuestRange {
                addr: 0x0041_0000,
                len: 0x1_0000,
            },
            d3d::GuestRange {
                addr: 0x0042_0000,
                len: 0x1000,
            },
        );
        let down = glow::TARGETS + glow::TARGET_DOWN * glow::TARGET_STRIDE;
        let other = glow::TARGETS + glow::TARGET_OTHER * glow::TARGET_STRIDE;

        for wrong_target in [false, true] {
            let mut mem = FlatMemory::new(0x0040_0000, 0x10_0000);
            for slot in 0..d3d::pushbuf::SIMPLE_ENCODE_SLOTS as u32 {
                let header = (1 << 18) | (4 * (slot + 1));
                mem.set_u32(config.layout.simple_encode + 4 * slot, header)
                    .unwrap();
            }
            for (descriptor, data, (width, height)) in [
                (down, 0x0010_0000, plan.down),
                (other, 0x0011_0000, plan.other),
            ] {
                mem.set_u32(descriptor + 0x88, data).unwrap();
                mem.set_u32(descriptor + 0x78, 1).unwrap();
                let surface = descriptor + 0x18;
                let resource = (ResourceType::Surface.raw() << common::TYPE_SHIFT) | 1;
                let format = (2 << 4) | (gfx::D3DFormat::LinA8R8G8B8.raw() << 8) | (1 << 16);
                let size = (width - 1) | ((height - 1) << 12);
                for (i, word) in [resource, data, 0, format, size, 0].into_iter().enumerate() {
                    mem.set_u32(surface + 4 * i as u32, word).unwrap();
                }
            }
            let headers = glow_headers(&mem);
            let mut dm = DeviceMemory(&mut mem);
            let mut backend = d3d::RecordingBackend::new();
            backend.external_passes = true;
            let mut device = d3d::Device::new(config, backend);
            let params = 0x0043_0000;
            d3d::PresentParameters {
                width: plan.view.0,
                height: plan.view.1,
                back_buffer_format: gfx::D3DFormat::A8R8G8B8.raw(),
                back_buffer_count: 1,
                multisample_type: 0,
                swap_effect: 0,
                enable_auto_depth_stencil: false,
                auto_depth_stencil_format: 0,
                flags: 0,
                refresh_rate: 0,
                presentation_interval: 0,
                buffer_surface: 0,
                depth_stencil_surface: 0,
            }
            .write(&mut dm, params)
            .unwrap();
            device.create_device(&mut dm, 0, params, 0).unwrap();
            device.set_vertex_shader(&mut dm, fvf::XYZRHW).unwrap();
            let view = device.bound_target()[0];
            let swallow = d3d::Swallow {
                call: Some(call),
                end_returns: Vec::new(),
                headers,
                targets: targets(&plan),
                primitive: QUAD.0,
                vertices: QUAD.1,
                shapes: Vec::new(),
                drop: true,
                pass: Box::new(CapturePass(None)),
            };
            assert!(device.swallow(&mut dm, swallow).unwrap());

            // Downsample, four ping-pong blur draws, then the composite.
            // The guest binds the embedded surfaces and pushes a fan with
            // the position register last for each corner.
            let (down, other) = (down + 0x18, other + 0x18);
            let first_blur = if wrong_target { down } else { other };
            for target in [down, first_blur, down, other, down, view] {
                device.set_render_target(&mut dm, target, 0).unwrap();
                let mut put = dm.read_u32(config.layout.device).unwrap();
                let mut push = |words: &[u32]| {
                    for &word in words {
                        dm.write_u32(put, word).unwrap();
                        put += 4;
                    }
                };
                push(&[(1 << 18) | method::BEGIN_END, 7]);
                for (x, y) in [(0.0_f32, 0.0_f32), (0.0, 8.0), (8.0, 8.0), (8.0, 0.0)] {
                    push(&[
                        (4 << 18) | (method::VERTEX_DATA4F + 16),
                        x.to_bits(),
                        y.to_bits(),
                        0,
                        0,
                    ]);
                    push(&[
                        (4 << 18) | method::VERTEX4F,
                        x.to_bits(),
                        y.to_bits(),
                        0,
                        1.0_f32.to_bits(),
                    ]);
                }
                push(&[(1 << 18) | method::BEGIN_END, 0]);
                dm.write_u32(config.layout.device, put).unwrap();
            }
            assert_eq!(device.drain_until_callback(&mut dm).unwrap(), None);
            assert_eq!(device.stats().draws, if wrong_target { 6 } else { 0 });
            device
                .finish_swallow(&mut dm, call.entry, call.esp)
                .unwrap();
            let (matched, reason) = if wrong_target {
                (1, d3d::SwallowRefusal::Shape)
            } else {
                // The recorder cannot encode a replacement. It reaches
                // the encode boundary with all six originals still held.
                (6, d3d::SwallowRefusal::Encode)
            };
            let stats = device.stats();
            assert_eq!(stats.swallowed_draws, matched);
            assert_eq!(stats.swallow_draws_replayed, matched);
            assert_eq!(
                stats.swallow_reasons[reason.index()],
                1,
                "{:?}",
                device.swallow_mismatch()
            );
            assert_eq!(stats.draws, 6);
        }
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
    /// before.
    #[test]
    fn a_screen_swallow_takes_the_target_its_pass_binds() {
        let effect = Effect {
            shader: 0,
            implementations: 2,
            passes: vec![drawn_pass(2, 3)],
        };
        let call = screen_call(&effect, EGOR, [0; 4], 0, None, &unmade_grid).unwrap();
        let plan = screen_swallow(&call).unwrap();
        assert!(!plan.0.contains(&0), "{:?}", plan.0);
        assert_eq!(plan.0, [1, 1]);
        assert_eq!(
            plan.1,
            [
                (PrimitiveType::TriangleStrip, 6),
                (PrimitiveType::TriangleStrip, 6)
            ]
        );
    }

    /// A call is armed only when `0x00016610` draws every pass the plan
    /// has (the gate itself: engine's
    /// `the_template_pass_draws_only_an_implementation_with_a_vertex_shader`);
    /// a template that does not read keeps it on D3D.
    #[test]
    fn a_pass_the_template_skips_is_left_out_and_an_unread_gate_refuses() {
        use engine::memory::FlatMemory;
        let m = FlatMemory::new(0x0010_0000, 0x100);
        let mut one = Effect {
            shader: 0xe001_0005,
            implementations: 1,
            passes: vec![pass(0, vec![])],
        };
        assert_eq!(skip_undrawn(&m, &mut one), None, "unread: refused");
        assert_eq!(one.passes.len(), 1, "untouched");
        let mut none = Effect {
            passes: vec![],
            ..one.clone()
        };
        assert_eq!(skip_undrawn(&m, &mut none), Some(0), "nothing to skip");
        // A shader of -1: 0x00016610 returns 0 at once, every pass skipped.
        let mut unbound = Effect {
            shader: u32::MAX,
            passes: vec![pass(0, vec![]), pass(2, vec![])],
            ..one
        };
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
        assert_eq!(
            missing(&effect(vec![]), no, false),
            vec![],
            "the restores at the end draw nothing"
        );
        let e = effect(vec![
            pass(0, vec![Convolution::Kept { result: 1 }]),
            pass(2, vec![]),
        ]);
        assert_eq!(
            missing(&e, no, false),
            vec![Missing::TemplatePass, Missing::PassTarget]
        );
        assert_eq!(
            missing(&e, yes, false),
            vec![Missing::PassTarget],
            "the template pass draws"
        );
        // desaturate's shape: one pass into the bound target, nothing
        // convolved.
        assert_eq!(
            missing(&effect(vec![pass(0, vec![])]), yes, false),
            vec![],
            "armed"
        );
        assert_eq!(
            missing(
                &effect(vec![pass(0, vec![Convolution::Kept { result: 0 }])]),
                yes,
                false
            ),
            vec![],
            "a kept one draws nothing"
        );
        // Both destinations are pinned independently of their order.
        let into_1 = screen_effect::Pass {
            size: Some((320, 240)),
            ..pass(2, vec![])
        };
        assert_eq!(
            missing(&effect(vec![into_1.clone(), pass(0, vec![])]), yes, false),
            vec![]
        );
        assert_eq!(
            missing(&effect(vec![pass(0, vec![]), into_1.clone()]), yes, false),
            vec![],
            "target 1 last"
        );
        assert_eq!(
            missing(&effect(vec![into_1.clone()]), yes, false),
            vec![],
            "the program determines whether the view is needed"
        );
        let unsized_1 = screen_effect::Pass {
            size: Some((0, 240)),
            ..into_1
        };
        assert_eq!(
            missing(&effect(vec![unsized_1, pass(0, vec![])]), yes, false),
            vec![Missing::PassTarget],
            "no size"
        );
        // The blur: its quads draw once their frame is read; so do a wide
        // one's grids (its combiner is the frame's).
        let args = BlurArgs {
            source: 1,
            steps: 0,
            scale: 0,
            growth: 0,
            dest: 0x19,
            other: 0x1a,
            last: u32::MAX,
        };
        let blur = game_glow::Blur {
            draws: vec![],
            result: 1,
            copy: None,
        };
        let into = |c: Convolution| screen_effect::Pass {
            size: Some((320, 240)),
            ..pass(2, vec![c])
        };
        let blurred = || {
            effect(vec![
                into(Convolution::Blur {
                    size: (320, 240),
                    args,
                    blur: blur.clone(),
                }),
                pass(0, vec![]),
            ])
        };
        assert_eq!(
            missing(&blurred(), yes, false),
            vec![Missing::Blur, Missing::StandIns],
            "no frame"
        );
        assert_eq!(
            missing(&blurred(), yes, true),
            vec![],
            "drawn into the stand-ins"
        );
        let wide = game_glow::Wide {
            draws: vec![],
            result: 1,
            copy: None,
        };
        let wide_args = game_glow::WideArgs {
            source: 1,
            taps: 0,
            scale: 0,
            words: [0; 2],
            dest: 0x19,
            other: 0x1a,
            last: u32::MAX,
        };
        let widened = effect(vec![
            into(Convolution::Wide {
                size: (320, 240),
                args: wide_args,
                wide,
            }),
            pass(0, vec![]),
        ]);
        assert_eq!(
            missing(&widened, yes, false),
            vec![Missing::Wide, Missing::StandIns],
            "no frame"
        );
        assert_eq!(
            missing(&widened, yes, true),
            vec![],
            "drawn into the stand-ins"
        );
        // A terminal convolution writes its own slot after the view strip.
        let first = || screen_effect::Pass {
            size: Some((320, 240)),
            ..pass(2, vec![])
        };
        let last = effect(vec![
            first(),
            pass(
                0,
                vec![Convolution::Blur {
                    size: (320, 240),
                    args,
                    blur: game_glow::Blur {
                        draws: vec![game_glow::BlurDraw { source: 0, dest: 0x19, spread: 0 }],
                        result: 0x19,
                        copy: None,
                    },
                }],
            ),
        ]);
        assert_eq!(missing(&last, yes, true), vec![]);
        let kept_last = effect(vec![
            first(),
            pass(0, vec![Convolution::Kept { result: 0 }]),
        ]);
        assert_eq!(
            missing(&kept_last, yes, true),
            vec![],
            "a kept one draws nothing"
        );
    }

    /// The blur's convolution as the native pass draws it: its quads'
    /// sources, stand-ins and spreads, its result; one kept names the
    /// pass's target.
    #[test]
    fn a_convolution_is_its_quads_and_its_result() {
        let args = BlurArgs {
            source: 1,
            steps: 0,
            scale: 0,
            growth: 0,
            dest: 0x19,
            other: 0x1a,
            last: u32::MAX,
        };
        let draws = vec![
            game_glow::BlurDraw {
                source: 1,
                dest: 0x19,
                spread: 1.5f32.to_bits(),
            },
            game_glow::BlurDraw {
                source: 0x19,
                dest: 0x1a,
                spread: 2.0f32.to_bits(),
            },
        ];
        let blur = game_glow::Blur {
            draws,
            result: 0x1a,
            copy: None,
        };
        let p = pass(1, vec![]);
        let c = screen_convolution(
            &p,
            &Convolution::Blur {
                size: (320, 240),
                args,
                blur: blur.clone(),
            },
            &unmade_grid,
        )
        .unwrap();
        assert_eq!((c.size, c.result), ((320, 240), 0x1a));
        assert_eq!(
            c.draws,
            vec![
                ConvolutionDraw {
                    source: 1,
                    dest: 0x19,
                    spread: 1.5,
                    grid: None
                },
                ConvolutionDraw {
                    source: 0x19,
                    dest: 0x1a,
                    spread: 2.0,
                    grid: None
                },
            ]
        );
        let kept = screen_convolution(&p, &Convolution::Kept { result: 1 }, &unmade_grid);
        assert_eq!(kept.map(|c| (c.draws.len(), c.result)), Some((0, 1)));
        let copied = game_glow::Blur {
            copy: Some((0x1a, 0x1b)),
            ..blur.clone()
        };
        assert_eq!(
            screen_convolution(
                &p,
                &Convolution::Blur {
                    size: (320, 240),
                    args,
                    blur: copied
                },
                &unmade_grid
            ),
            None,
            "0x000351a0"
        );
        assert_eq!(
            screen_convolution(
                &p,
                &Convolution::Blur {
                    size: (-1, 240),
                    args,
                    blur
                },
                &unmade_grid
            ),
            None
        );
    }

    /// A wide convolution as the native pass draws it: each draw its grid,
    /// made for its own step; none when a grid is not made.
    #[test]
    fn a_wide_convolution_is_its_grids() {
        let args = WideArgs {
            source: 1,
            taps: 2,
            scale: 0,
            words: [5, 6],
            dest: 0x19,
            other: 0x1a,
            last: u32::MAX,
        };
        let draws = vec![
            WideDraw {
                source: 1,
                dest: 0x19,
                offsets: [1, 2, 3, 4],
            },
            WideDraw {
                source: 0x19,
                dest: 0x1a,
                offsets: [5, 6, 7, 8],
            },
        ];
        let wide = game_glow::Wide {
            draws,
            result: 0x1a,
            copy: None,
        };
        let c = Convolution::Wide {
            size: (320, 240),
            args,
            wide: wide.clone(),
        };
        let p = screen_effect::Pass {
            size: Some((640, 480)),
            ..pass(1, vec![])
        };
        // A grid naming its step: one strip whose first tap is the offsets'.
        let named = |_: &screen_effect::Pass, a: &WideArgs, size: (i32, i32), d: &WideDraw| {
            let g = wide_grid(a, size, d, (640, 480));
            let v = StripVertex {
                texcoords: [[f32::from_bits(d.offsets[0]); 4]; 4],
                ..Default::default()
            };
            Some(ConvolutionGrid {
                bound: (g.bound[0].0 as u32, g.bound[0].1 as u32),
                strips: vec![vec![v; 3]],
            })
        };
        let s = screen_convolution(&p, &c, &named).unwrap();
        assert_eq!((s.size, s.result), ((320, 240), 0x1a));
        assert_eq!(
            s.draws
                .iter()
                .map(|d| (d.source, d.dest))
                .collect::<Vec<_>>(),
            vec![(1, 0x19), (0x19, 0x1a)]
        );
        let grids: Vec<_> = s.draws.iter().map(|d| d.grid.clone().unwrap()).collect();
        assert_eq!(
            (grids[0].bound, grids[1].bound),
            ((640, 480), (320, 240)),
            "the source's, then a stand-in's"
        );
        assert_eq!(grids[1].strips[0][0].texcoords[0][0], f32::from_bits(5));
        assert_eq!(
            screen_convolution(&p, &c, &|_, _, _, _| None),
            None,
            "a grid not made"
        );
        let copied = Convolution::Wide {
            size: (320, 240),
            args,
            wide: game_glow::Wide {
                copy: Some((0x1a, 0x1b)),
                ..wide
            },
        };
        assert_eq!(screen_convolution(&p, &copied, &named), None, "0x000351a0");
    }

    /// `0x00034d90`'s inputs as `0x00021360` pushes them.
    #[test]
    fn a_wide_draw_is_its_grid() {
        let radii = [0.25f32.to_bits(), 0.75f32.to_bits()];
        let args = WideArgs {
            source: 0,
            taps: 1,
            scale: 0,
            words: radii,
            dest: 0x1b,
            other: 0x19,
            last: u32::MAX,
        };
        let first = WideDraw {
            source: 0,
            dest: 0x1b,
            offsets: [1, 2, 3, 4],
        };
        let g = wide_grid(&args, (160, 120), &first, (640, 480));
        assert_eq!(
            g,
            Grid {
                size: (160, 120),
                bound: [(640, 480); 4],
                taps: [1, 2, 3, 4],
                depth: 1.0f32.to_bits(),
                radii,
                full: false,
            }
        );
        let next = WideDraw {
            source: 0x1b,
            dest: 0x19,
            offsets: [0; 4],
        };
        assert_eq!(
            wide_grid(&args, (160, 120), &next, (640, 480)).bound,
            [(160, 120); 4],
            "a stand-in, at the pair's size"
        );
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
        let fp = Fp {
            mxcsr: 0x1f80,
            x87_control: 0x027f,
        };
        let args = WideArgs {
            source: 1,
            taps: 1,
            scale: 0,
            words: [0; 2],
            dest: 0x19,
            other: 0x1a,
            last: u32::MAX,
        };
        let d = WideDraw {
            source: 1,
            dest: 0x19,
            offsets: [0.5f32.to_bits(), 0, 0, 0],
        };
        let g = wide_grid(&args, (320, 240), &d, (640, 480));
        let grid = grid_of(&m, fp, &g).expect("made");
        assert_eq!(grid.bound, (640, 480));
        assert_eq!(grid.strips.len(), screen_grid::ROWS as usize);
        assert!(
            grid.strips
                .iter()
                .all(|s| s.len() == 2 * (screen_grid::COLUMNS as usize + 1))
        );
        let exact = screen_grid::vertex(&m, &g, 3, 5, fp.mxcsr, fp.x87_control).unwrap();
        // Row 5's strip, column 3, the upper vertex.
        let v = grid.strips[5][6];
        assert_eq!(v.position.map(f32::to_bits), exact.position);
        assert_eq!(v.texcoords.map(|t| t.map(f32::to_bits)), exact.taps);
        assert_eq!(
            grid.strips[0][0].position[..2],
            [0.0, 0.0],
            "the window's corner"
        );
        let unbound = Grid {
            bound: [(0, 480); 4],
            ..g
        };
        assert_eq!(grid_of(&m, fp, &unbound), None);
    }

    /// The source of a wide convolution: target 1 the pass's size, the
    /// bound target the table's (`0x0001dd60`'s default without a surface).
    #[test]
    fn a_wide_source_is_its_targets_size() {
        use engine::memory::FlatMemory;
        use screen_effect::{TARGET_STRIDE, TARGETS};
        let mut m = FlatMemory::new(TARGETS, 0x1000);
        m.set_u32(TARGETS + TARGET_STRIDE + 0x88, 1).unwrap();
        let p = screen_effect::Pass {
            size: Some((320, 240)),
            ..pass(2, vec![])
        };
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
            m.write(crate::health::CACHE_MAP_NAME, name.as_bytes())
                .unwrap();
            m
        };
        let slot = Mutex::new(EffectsSlot::default());
        let mut started = Vec::new();
        let mainmenu = named("mainmenu");
        assert!(
            map_effects(&slot, &mainmenu, &mut |n, _| started.push(n)).is_none(),
            "loading"
        );
        assert_eq!(
            started,
            vec!["mainmenu".to_string()],
            "started by the post itself"
        );
        assert!(map_effects(&slot, &mainmenu, &mut |n, _| started.push(n)).is_none());
        assert_eq!(started.len(), 1, "one load per map");
        assert!(slot.lock().unwrap().loaded(
            "mainmenu",
            render_bridge::native_map_work::generation(),
            ScreenEffects::default()
        ));
        assert!(
            map_effects(&slot, &mainmenu, &mut |n, _| started.push(n)).is_some(),
            "loaded"
        );
        let other = named("01a_tutorial");
        assert!(map_effects(&slot, &other, &mut |n, _| started.push(n)).is_none());
        assert_eq!(started.last().map(String::as_str), Some("01a_tutorial"));
        assert!(
            !slot.lock().unwrap().loaded(
                "mainmenu",
                render_bridge::native_map_work::generation(),
                ScreenEffects::default()
            ),
            "a late load of the last map is dropped"
        );
        assert!(map_effects(&slot, &other, &mut |n, _| started.push(n)).is_none());
        assert!(
            map_effects(&slot, &named(""), &mut |n, _| started.push(n)).is_none(),
            "no map loaded"
        );
        assert_eq!(started.len(), 2);
    }

    /// A map opened again (its load generation moved, as the reload an
    /// edit asks for does) loads its effects again; a load of the
    /// earlier generation that finishes late is dropped.
    #[test]
    fn the_screen_effects_slot_is_keyed_by_map_generation() {
        let mut slot = EffectsSlot::default();
        let (effects, start) = slot.ask("mainmenu", 1);
        assert!(effects.is_none() && start);
        assert!(slot.loaded("mainmenu", 1, ScreenEffects::default()));
        let (effects, start) = slot.ask("mainmenu", 1);
        assert!(effects.is_some() && !start, "same map, same load");
        let (effects, start) = slot.ask("mainmenu", 2);
        assert!(
            effects.is_none() && start,
            "the same map opened again loads again"
        );
        assert!(
            !slot.loaded("mainmenu", 1, ScreenEffects::default()),
            "the earlier load finishes late"
        );
        assert!(slot.loaded("mainmenu", 2, ScreenEffects::default()));
        assert!(slot.ask("mainmenu", 2).0.is_some());
    }

    #[test]
    fn activation_forgets_screen_effects_without_waiting_for_another_screen_draw() {
        let mut slot = EffectsSlot::default();
        slot.ask("synthetic", 1);
        assert!(slot.loaded("synthetic", 1, ScreenEffects::default()));
        let in_flight = slot.ask("synthetic", 1).0.unwrap();
        let old = Arc::downgrade(&in_flight);

        assert!(slot.take_stale(1).is_none());
        drop(slot.take_stale(2).unwrap());
        assert!(!slot.loaded("synthetic", 1, ScreenEffects::default()));
        assert!(slot.ask("synthetic", 2).1);
        assert!(old.upgrade().is_some());
        drop(in_flight);
        assert!(old.upgrade().is_none());
    }

    #[test]
    fn a_pass_draws_where_its_kind_says() {
        assert_eq!(dest(&pass(0, vec![])), Some(Dest::Bound));
        let half = screen_effect::Pass {
            size: Some((320, 240)),
            ..pass(2, vec![])
        };
        assert_eq!(dest(&half), Some(Dest::Target1 { guest: (320, 240) }));
        assert_eq!(dest(&pass(1, vec![])), None, "target 1 without a size");
        assert_eq!(
            dest(&screen_effect::Pass {
                size: Some((-1, 2)),
                ..half
            }),
            None
        );
    }

    /// blur's order: the view into target 1, then target 1 back.
    #[test]
    fn a_call_reads_target_1_only_after_drawing_it() {
        let mut e = effects([
            ScreenStage::Engine(3),
            ScreenStage::Empty,
            ScreenStage::Empty,
            ScreenStage::Empty,
        ]);
        let copy = program([
            ScreenStage::Engine(5),
            ScreenStage::Empty,
            ScreenStage::Empty,
            ScreenStage::Empty,
        ]);
        e.effects
            .get_mut(&EGOR)
            .unwrap()
            .programs
            .insert((0, 0), copy);
        let back = drawn_pass(2, 4);
        let into_1 = screen_effect::Pass {
            implementation: 0,
            kind: 1,
            size: Some((640, 480)),
            ..drawn_pass(2, 4)
        };
        let call = |passes| Effect {
            shader: 0,
            implementations: 2,
            passes,
        };
        assert!(call_ordered(
            Some(&e),
            EGOR,
            &call(vec![into_1.clone(), back.clone()])
        ).is_ok());
        assert_eq!(call_ordered(Some(&e), EGOR, &call(vec![into_1.clone()])), Err(Missing::ViewTarget));
        assert!(
            call_ordered(Some(&e), EGOR, &call(vec![back.clone()])).is_err(),
            "target 1 as D3D left it"
        );
        assert!(
            call_ordered(Some(&e), EGOR, &call(vec![back, into_1])).is_err(),
            "read before drawn"
        );
        assert!(
            call_ordered(None, EGOR, &call(vec![drawn_pass(1, 4)])).is_err(),
            "no world"
        );
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
            combiner_inputs: Default::default(),
            raster_shader: None,
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
                stencil: None,
                depth_offset: None,
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
        e.images
            .insert((BITMAP, 0), native_render::gpu::Image::solid([0; 4]));
        e
    }

    /// A pass of layer pass 1, implementation 0, with `rows` strips of
    /// `columns` corner pairs.
    fn drawn_pass(rows: usize, columns: usize) -> screen_effect::Pass {
        let v = screen_effect::Vertex {
            texcoords: [[0; 4]; 4],
            position: [0; 2],
        };
        screen_effect::Pass {
            implementation: 1,
            selector: 0,
            strips: vec![vec![v; 2 * columns]; rows],
            ..pass(0, vec![])
        }
    }

    #[test]
    fn a_template_pass_draws_with_its_program_and_what_it_binds() {
        let bitmap = ScreenStage::Bitmap {
            bitmap: BITMAP,
            image: 0,
        };
        let view = effects([
            ScreenStage::Engine(5),
            bitmap,
            ScreenStage::Empty,
            ScreenStage::Empty,
        ]);
        let p = drawn_pass(3, 4);
        assert!(
            pass_drawable(Some(&view), EGOR, &p),
            "the view's copy and a decoded bitmap"
        );
        assert!(!pass_drawable(None, EGOR, &p), "no world loaded");
        assert!(
            !pass_drawable(Some(&view), TagId(0xe000_0009), &p),
            "another egor"
        );
        assert!(
            !pass_drawable(
                Some(&view),
                EGOR,
                &screen_effect::Pass {
                    selector: 1,
                    ..p.clone()
                }
            ),
            "no such implementation"
        );
        assert!(
            !pass_drawable(Some(&view), EGOR, &drawn_pass(0, 4)),
            "no strips"
        );
        assert!(
            !pass_drawable(Some(&view), EGOR, &drawn_pass(2, 1)),
            "two-vertex strips"
        );
        let target = effects([
            ScreenStage::Engine(13),
            ScreenStage::Empty,
            ScreenStage::Empty,
            ScreenStage::Empty,
        ]);
        assert!(
            !pass_drawable(Some(&target), EGOR, &p),
            "target 9 has no stand-in"
        );
        let target1 = effects([
            ScreenStage::Engine(3),
            ScreenStage::Empty,
            ScreenStage::Empty,
            ScreenStage::Empty,
        ]);
        assert!(
            pass_drawable(Some(&target1), EGOR, &p),
            "target 1's stand-in (the call's order is checked apart)"
        );
        let undecoded = effects([
            ScreenStage::Bitmap {
                bitmap: BITMAP,
                image: 1,
            },
            ScreenStage::Empty,
            ScreenStage::Empty,
            ScreenStage::Empty,
        ]);
        assert!(
            !pass_drawable(Some(&undecoded), EGOR, &p),
            "a bitmap that did not decode"
        );
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
        assert_eq!(
            call.values, values,
            "what the passes' engine constants read"
        );
        assert_eq!(
            call.passes[0].0,
            (EGOR.0, 1, 0),
            "(egor, layer pass, implementation)"
        );
        assert_eq!(call.passes[0].1, Dest::Bound);
        assert_eq!(call.passes[0].2[0][0].position[2].to_bits(), 0x4b7f_ffff);
        let strip = (PrimitiveType::from_xbox(6).unwrap(), 8);
        assert_eq!(
            screen_swallow(&call),
            Some((vec![SLOT_BOUND; 6], vec![strip; 6])),
            "SET_BEGIN_END 6"
        );
        let uneven = Effect {
            passes: vec![drawn_pass(1, 4), drawn_pass(1, 3)],
            ..effect.clone()
        };
        let shapes =
            screen_swallow(&screen_call(&uneven, EGOR, [0; 4], 0, None, &unmade_grid).unwrap())
                .map(|s| s.1);
        assert_eq!(
            shapes,
            Some(vec![strip, (PrimitiveType::TriangleStrip, 6)]),
            "each strip its own count"
        );
        let empty = Effect {
            passes: vec![],
            ..effect.clone()
        };
        assert_eq!(
            screen_swallow(&screen_call(&empty, EGOR, [0; 4], 0, None, &unmade_grid).unwrap()),
            None
        );
        // Target 1's strips go to slot 2, the target the first finds; no
        // draw is slot 0's (the target bound when armed).
        let into_1 = screen_effect::Pass {
            kind: 1,
            size: Some((640, 480)),
            ..drawn_pass(2, 4)
        };
        let both = Effect {
            passes: vec![into_1, drawn_pass(3, 4)],
            ..effect
        };
        let call = screen_call(&both, EGOR, [0; 4], 0, None, &unmade_grid).unwrap();
        assert_eq!(call.passes[0].1, Dest::Target1 { guest: (640, 480) });
        assert_eq!(
            screen_swallow(&call).map(|s| s.0),
            Some(vec![2, 2, 1, 1, 1])
        );
        // A blur after the first pass: its quads into 0x19 (slot 3) and
        // 0x1a (slot 4), between the passes' strips.
        let args = BlurArgs {
            source: 1,
            steps: 0,
            scale: 0,
            growth: 0,
            dest: 0x19,
            other: 0x1a,
            last: u32::MAX,
        };
        let draws = vec![
            game_glow::BlurDraw {
                source: 1,
                dest: 0x19,
                spread: 0,
            },
            game_glow::BlurDraw {
                source: 0x19,
                dest: 0x1a,
                spread: 0,
            },
        ];
        let blur = game_glow::Blur {
            draws,
            result: 0x1a,
            copy: None,
        };
        let into_1 = screen_effect::Pass {
            convolutions: vec![Convolution::Blur {
                size: (320, 240),
                args,
                blur,
            }],
            ..both.passes[0].clone()
        };
        let blurred = Effect {
            passes: vec![into_1, both.passes[1].clone()],
            ..both.clone()
        };
        let (slots, shapes) =
            screen_swallow(&screen_call(&blurred, EGOR, [0; 4], 0, None, &unmade_grid).unwrap())
                .unwrap();
        assert_eq!(slots, vec![2, 2, 3, 4, 1, 1, 1]);
        assert_eq!(shapes[2..4], [QUAD, QUAD]);
        // A wide one: each draw its grid's strips, into its stand-in.
        let args = WideArgs {
            source: 1,
            taps: 2,
            scale: 0,
            words: [0; 2],
            dest: 0x19,
            other: 0x1a,
            last: u32::MAX,
        };
        let draws = vec![
            WideDraw {
                source: 1,
                dest: 0x19,
                offsets: [0; 4],
            },
            WideDraw {
                source: 0x19,
                dest: 0x1a,
                offsets: [0; 4],
            },
        ];
        let wide = game_glow::Wide {
            draws,
            result: 0x1a,
            copy: None,
        };
        let into_1 = screen_effect::Pass {
            convolutions: vec![Convolution::Wide {
                size: (320, 240),
                args,
                wide,
            }],
            ..both.passes[0].clone()
        };
        let widened = Effect {
            passes: vec![into_1, both.passes[1].clone()],
            ..both
        };
        let grid = |_: &screen_effect::Pass, _: &WideArgs, _: (i32, i32), _: &WideDraw| {
            Some(ConvolutionGrid {
                bound: (640, 480),
                strips: vec![vec![StripVertex::default(); 42]; 2],
            })
        };
        let (slots, shapes) =
            screen_swallow(&screen_call(&widened, EGOR, [0; 4], 0, None, &grid).unwrap()).unwrap();
        assert_eq!(slots, vec![2, 2, 3, 3, 4, 4, 1, 1, 1]);
        assert_eq!(
            shapes[2..6],
            [(PrimitiveType::TriangleStrip, 42); 4],
            "0x00034770's SET_BEGIN_END 6"
        );
        assert!(
            screen_call(&widened, EGOR, [0; 4], 0, None, &|_, _, _, _| None).is_none(),
            "a grid not made"
        );
    }

    #[test]
    fn a_screen_effects_plan_reads_its_words() {
        use engine::memory::FlatMemory;
        let m = FlatMemory::new(0x0060_0000, 0x1000);
        let fp = Fp {
            mxcsr: 0x1f80,
            x87_control: 0x027f,
        };
        let call = |columns, rows| ScreenEffect {
            datum: 0xe000_0001,
            values: 0x0060_0000,
            columns,
            rows,
            corners: 0x0060_0100,
        };
        assert_eq!(
            plan_effect(&m, call(9, 9), fp),
            Err("too many corners".to_string())
        );
        assert!(
            plan_effect(&m, call(2, 2), fp).is_err(),
            "the window is not in this memory"
        );
        let unreadable = ScreenEffect {
            values: 0x0010_0000,
            ..call(2, 2)
        };
        assert!(
            plan_effect(&m, unreadable, fp)
                .unwrap_err()
                .starts_with("unreadable screen values")
        );
    }

    #[test]
    fn colour_only_quads_keep_effective_depth_and_stencil_operations() {
        use gfx::state::{CompareFunc, StencilOp};
        let mut state = gfx::RenderState::default();
        let depth = Some(wgpu::TextureFormat::Depth24PlusStencil8);
        assert!(!post_needs_depth(&state, None));
        assert!(post_needs_depth(&state, depth));
        state.depth_write = false;
        assert!(post_needs_depth(&state, depth));
        state.depth_func = CompareFunc::Always;
        state.stencil.enable = true;
        assert!(!post_needs_depth(&state, depth));
        state.stencil.pass = StencilOp::Replace;
        assert!(post_needs_depth(&state, depth));
        state.stencil.write_mask = 0;
        assert!(!post_needs_depth(&state, depth));
        state.stencil.func = CompareFunc::Equal;
        assert!(post_needs_depth(&state, depth));
        assert!(!post_needs_depth(&state, Some(wgpu::TextureFormat::Depth16Unorm)));
    }

    #[test]
    fn inherited_shader_raster_state_only_matters_for_inputs_the_combiner_reads() {
        use gfx::state::ShadeMode;
        use shaders::PixelShaderDef;
        use shaders::psh::bits::{REG_FOG, REG_T0, REG_V1R0_SUM, final_efg, inputs};

        let state = gfx::RenderState {
            fog_enable: true,
            shade_mode: ShadeMode::Flat,
            ..Default::default()
        };
        let combiner = |source| CombinerInputs::read(&PixelShaderDef {
            final_inputs_abcd: inputs(0, 0, 0, source),
            final_inputs_efg: final_efg(0, 0, REG_T0, 0),
            ..Default::default()
        });
        assert_eq!(check_combiner_raster(&state, true, combiner(REG_T0)), Ok(()));
        assert_eq!(check_combiner_raster(&state, true, combiner(REG_FOG)), Err("the post combiner reads live fog"));
        assert_eq!(check_combiner_raster(&state, true, combiner(REG_V1R0_SUM)), Err("the post combiner reads flat-shaded vertex colours"));
        let smooth = gfx::RenderState { shade_mode: ShadeMode::Gouraud, ..state };
        assert_eq!(check_combiner_raster(&smooth, true, combiner(REG_V1R0_SUM)), Err("the post combiner reads two-sided vertex colours"));
        assert_eq!(check_combiner_raster(&state, true, CombinerInputs::read(&PixelShaderDef::default())), Err("the post combiner reads live fog"));
    }

    #[test]
    fn every_pass_is_colour_only() {
        use d3d::ExternalPass;
        let plan = Plan {
            window: [0.0; 4],
            signs: [0.0; 2],
            view: (640, 480),
            down: (160, 120),
            other: (160, 120),
            draws: vec![],
        };
        let glow = GlowPass {
            raster: OnceLock::new(),
            opaque_entry: OnceLock::new(),
            plan: plan.clone(),
            gpu: Arc::new(Mutex::new(GpuGlow::new())),
            entry: Arc::default(),
        };
        assert!(glow.colour_only(), "the composite binds no depth");
        assert!(CapturePass(None).colour_only());
        let effects = Arc::new(crate::native_post_load::ScreenEffects::default());
        let call = || ScreenCall {
            egor: EGOR,
            passes: Vec::new(),
            values: [0; 4],
            frame: None,
        };
        assert!(
            ScreenPass {
                effects,
                call: call(),
                raster: OnceLock::new()
            }
            .colour_only(),
            "the screen passes bind no depth"
        );
    }

    /// A post pass is a scene stage a replay can record again only once
    /// its call's raster is frozen.
    #[test]
    fn post_passes_record_again_only_once_validated() {
        use d3d::{ExternalPass, SwallowPass};
        let glow = GlowPass {
            raster: OnceLock::new(),
            opaque_entry: OnceLock::new(),
            plan: Plan {
                window: [0.0; 4],
                signs: [0.0; 2],
                view: (640, 480),
                down: (160, 120),
                other: (160, 120),
                draws: vec![],
            },
            gpu: Arc::new(Mutex::new(GpuGlow::new())),
            entry: Arc::default(),
        };
        assert!(!glow.replays_scene());
        glow.validate(&[], &[]).unwrap();
        assert!(glow.replays_scene());
        assert_eq!(glow.opaque_entry.get(), Some(&[false; 4]));

        let screen = ScreenPass {
            effects: Arc::new(crate::native_post_load::ScreenEffects::default()),
            call: ScreenCall { egor: EGOR, passes: Vec::new(), values: [0; 4], frame: None },
            raster: OnceLock::new(),
        };
        assert!(!screen.replays_scene());
        screen.validate(&[], &[]).unwrap();
        assert!(screen.replays_scene());
    }
}
