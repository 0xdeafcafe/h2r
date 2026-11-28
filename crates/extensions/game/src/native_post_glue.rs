//! The `native-post` host (docs/graphics/native-post-hud.md, stage 5;
//! docs/graphics/retire-d3d-display.md, "The post and the screen
//! effects"): the glow of the colour pass `0x000214f0` and each screen
//! effect `0x0002d140`, drawn as native passes on the frame graph's
//! targets while the calls' guest bodies do not run
//! (`render_bridge::native_post`).
//!
//! Each pass is a stage at the frame's transparents slot
//! (`native_render::Stage`), drawn on the frame's colour. A target the
//! call names by its role in Halo's target table
//! (`frame_scene::TargetRole::Game`: target 1, the stand-ins, the glow's
//! levels 7 and 8) has no frame target yet and panics by name
//! ([`frame_target`]). Raster state is the plan's: the glow's blend, a
//! template pass's own states (`ScreenProgram::state`). What a call needs
//! that is not drawn natively is an `Err` the feature panics with,
//! `post: <cause>`.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use engine::colour_pass::Parameters;
use engine::glow::{self as game_glow, BlurArgs, WideArgs, WideDraw};
use engine::memory::GuestMemory;
use engine::screen_effect::{self, Convolution, Effect};
use engine::screen_grid::{self, Grid};
use frame_scene::TargetRole;
use frame_types::Slot;
use map::TagId;
use native_render::camera::GuestRead;
use native_render::glow::{self, Blend, Draw, GlowTarget, GpuGlow, Plan, Surface};
use native_render::screen_gpu::{
    self, ConvolutionDraw, ConvolutionGrid, Dest, QuadFrame, ScreenConvolution, ScreenDraw,
    PostRaster, ScreenKey, ScreenTarget, StripVertex,
};
use native_render::screen_look::ScreenStage;
use native_render::{Stage, StageWork};
use render_bridge::native_post::{self as post_feature, Fp, Host, ScreenEffect};
use render_bridge::scope_zoom_hud;

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

/// The glow `0x000214f0` draws for parameters `p` (which draw); `Err`
/// when it draws something this cannot plan.
pub fn plan(mem: &dyn GuestMemory, p: &Parameters, fp: Fp) -> Result<Plan, String> {
    let mut plan = glow::read_frame(&Ro(mem)).ok_or("the window, signs or target sizes")?;
    let pass_a = game_glow::pass_a(mem, p, fp.x87_control).map_err(err("pass_a"))?;
    let blur_combiner = game_glow::blur_combiner(mem, p.args[4], p.y, fp.mxcsr, fp.x87_control)
        .map_err(err("blur_combiner"))?;
    let blur = game_glow::blur(mem, &BlurArgs::of(p), fp.mxcsr).map_err(err("blur"))?;
    if blur.copy.is_some() {
        return Err("the blur copies its result (0x000351a0)".into());
    }
    let pass_b = game_glow::pass_b(mem, p, fp.x87_control).map_err(err("pass_b"))?;
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
    Ok(plan)
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
/// the differential stubs `0x00016610`; without this the native pass would
/// draw passes the game skips. The passes it left out, or
/// `None` when a gate could not be read ([`Missing::TemplateSkipped`]).
pub fn skip_undrawn(mem: &dyn GuestMemory, effect: &mut Effect) -> Option<usize> {
    let drawn = template_gates(mem, effect)?;
    let before = effect.passes.len();
    let mut d = drawn.into_iter();
    effect.passes.retain(|_| d.next().unwrap_or(false));
    Some(before - effect.passes.len())
}

/// Per pass of `effect`, whether `0x00016610` draws it
/// (`screen_effect::template_draws`); `None` when a gate could not be read.
pub fn template_gates(mem: &dyn GuestMemory, effect: &Effect) -> Option<Vec<bool>> {
    effect
        .passes
        .iter()
        .map(|p| {
            let implementation = i32::try_from(p.implementation).unwrap_or(-2);
            screen_effect::template_draws(mem, effect.shader, screen_effect::TEMPLATE_LAYER, implementation, p.selector).ok()
        })
        .collect()
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
/// then `engine::screen_effect::plan_with`, a converted Halo 1 scope's
/// blur taking the radius its values carry
/// (`render_bridge::scope_zoom_hud::zoom_blur`).
pub fn plan_effect(
    mem: &dyn GuestMemory,
    c: ScreenEffect,
    fp: Fp,
) -> Result<Option<(Effect, [u32; 4])>, String> {
    let (values, corners) = native_render::post::screen_inputs(
        &Ro(mem), c.values, c.corners, c.columns, c.rows,
    )?;
    let effect = screen_effect::plan_with(
        mem,
        c.datum,
        values,
        c.columns,
        c.rows,
        &corners,
        fp.mxcsr,
        fp.x87_control,
        &|args| scope_zoom_hud::zoom_blur(values, args),
    )
    .map_err(err("plan"))?;
    Ok(effect.map(|e| (e, values)))
}

impl Missing {
    /// Its name in the panic, `post: screen effect: <name>`.
    pub fn name(self) -> &'static str {
        match self {
            Missing::TemplatePass => "template_pass",
            Missing::PassTarget => "pass_target",
            Missing::ViewTarget => "view_target",
            Missing::Blur => "blur",
            Missing::Wide => "wide",
            Missing::StandIns => "stand_ins",
            Missing::TemplateSkipped => "template_skipped",
        }
    }
}

/// Screen-effect calls that drew nothing because the map's effects were
/// still loading ([`EffectsSlot`]): the call's guest writes are made, its
/// passes are not drawn.
static EFFECTS_LOADING: AtomicU64 = AtomicU64::new(0);
static LOADING_LOGGED: AtomicBool = AtomicBool::new(false);

/// The glue's counters for the frame scene's counter map.
pub(crate) fn counters() -> impl Iterator<Item = (String, u64)> {
    post_feature::counters()
        .into_iter()
        .chain([("post_screen_effect_loading".to_string(), EFFECTS_LOADING.load(Ordering::Relaxed))])
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

/// The slots of a screen call's native pass ([`screen_slots`]): 0 is
/// unused, 1 the bound target (`0x00016610` binds target 0 itself,
/// `0x00014bc0` of 0 or 1, Code, `0x0001675c..0x00016765`, so the
/// strips go to target 0 whatever the caller had bound: the UI's call at
/// `0x0022bbbb` as much as `render_screen_effects`' at `0x00013b52`),
/// 2 target 1, 3 to 5 the stand-ins `0x19` to `0x1b`.
pub const SLOT_BOUND: u8 = 1;
pub const SLOT_TARGET1: u8 = 2;
pub const SLOT_STAND_INS: u8 = 3;

/// What a post pass's slot is to the game: the view (target 0's colour,
/// `[0x0050935c]`, a surface of the device's), the bound colour (the
/// target select's cache `[0x0051f3f4]`: the glow's view, which the body
/// binds by its index first), or a role of Halo's target table, its
/// surface header at `+0x18` (`render_bridge::native_targets::role`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PostTarget {
    View,
    Bound,
    Role(TargetRole),
}

/// The surface header of `t`; `Err` for a table entry that is not made
/// (no surface, a hidden size, no levels: `0x0001dcf0` returns none) or a
/// role the table does not hold.
pub fn post_slot(mem: &dyn GuestMemory, t: PostTarget) -> Result<u32, String> {
    match t {
        PostTarget::View => mem
            .u32(post_feature::TARGET0_COLOUR)
            .ok()
            .filter(|&h| h != 0)
            .ok_or("the view's colour surface".into()),
        PostTarget::Bound => mem
            .u32(render_bridge::native_targets::CACHE_COLOUR)
            .ok()
            .filter(|&h| h != 0)
            .ok_or("the bound colour surface".into()),
        PostTarget::Role(TargetRole::Game(n)) => {
            target_level_surface(mem, u32::from(n)).ok_or(format!("target {n:#x} is not made"))
        }
        PostTarget::Role(role) => Err(format!("{role:?} is no post target")),
    }
}

/// Whether the view reads as `X8` (alpha one) where a pass samples Halo's
/// target `index`: the texture `0x00014f60` binds for it
/// (`native_post::target_texture`) is `LIN_X8R8G8B8`.
pub fn reads_opaque(mem: &dyn GuestMemory, index: u32) -> Result<bool, String> {
    let header = post_feature::target_texture(mem, index).map_err(|e| format!("the view's texture: {e}"))?;
    let format = mem.u32(header.wrapping_add(0xc)).map_err(|_| "the view's texture header")?;
    Ok(gfx::D3DFormat::from_raw((format >> 8) & 0xff) == Some(gfx::D3DFormat::LinX8R8G8B8))
}

/// The guest ranges a screen call's pass records: each pass's strips,
/// each convolution's quad or grid strips (`ScreenCommands::ranges`).
pub fn screen_ranges(call: &ScreenCall) -> usize {
    call.passes
        .iter()
        .map(|(_, _, strips, convolutions)| {
            strips.len()
                + convolutions
                    .iter()
                    .flat_map(|c| &c.draws)
                    .map(|q| q.grid.as_ref().map_or(1, |g| g.strips.len()))
                    .sum::<usize>()
        })
        .sum()
}

/// A screen call's slots by [`SLOT_BOUND`], [`SLOT_TARGET1`] and
/// [`SLOT_STAND_INS`]: the view when a pass draws into it or reads it,
/// target 1 when a pass draws into it, each stand-in a convolution draws
/// into or reads.
pub fn screen_slot_targets(call: &ScreenCall) -> [Option<(PostTarget, bool)>; 6] {
    let mut slots = [None; 6];
    slots[usize::from(SLOT_BOUND)] = Some((PostTarget::View, call.passes.iter().any(|p| p.1 == Dest::Bound)));
    if call.passes.iter().any(|p| matches!(p.1, Dest::Target1 { .. })) {
        slots[usize::from(SLOT_TARGET1)] = Some((PostTarget::Role(TargetRole::Game(1)), true));
    }
    for q in call.passes.iter().flat_map(|p| &p.3).flat_map(|c| &c.draws) {
        for (target, written) in [(q.dest, true), (q.source, false)] {
            if let Some(i) = screen_gpu::stand_in(target) {
                let slot = &mut slots[usize::from(SLOT_STAND_INS) + i];
                let was = slot.is_some_and(|(_, w)| w);
                *slot = Some((PostTarget::Role(TargetRole::Game(0x19 + i as u8)), was || written));
            }
        }
    }
    slots
}

/// The glow's slots: the bound colour (the view's target, which the body binds first) 0, target 7 1, target 8 2.
pub const GLOW_TARGETS: [PostTarget; 3] =
    [PostTarget::Bound, PostTarget::Role(TargetRole::Game(7)), PostTarget::Role(TargetRole::Game(8))];

/// The body's target-table writes (`0x0001ddd0`) in its order, for
/// `effect` as planned and `drawn` its passes' template gates: before a
/// pass's gate (`0x0002d49f`, ahead of `0x00016610` at `0x0002d4b7`),
/// target 1 at its size; past a drawn one's gate, each drawn
/// convolution's two stand-ins (`0x0002da3e`, `0x0002da47`).
pub fn resizes(effect: &Effect, drawn: &[bool]) -> Vec<(u32, (i32, i32))> {
    let mut out = Vec::new();
    for (p, &d) in effect.passes.iter().zip(drawn) {
        if let Some(size) = p.size {
            out.push((1, size));
        }
        if !d {
            continue;
        }
        for c in &p.convolutions {
            match c {
                Convolution::Kept { .. } => {}
                Convolution::Blur { size, args, .. } => out.extend([(args.dest, *size), (args.other, *size)]),
                Convolution::Wide { size, args, .. } => out.extend([(args.dest, *size), (args.other, *size)]),
            }
        }
    }
    out
}

/// One size per target in a call: the native pass draws each target at
/// one shape. `Err` names a target the call sizes twice.
pub fn one_size_each(resizes: &[(u32, (i32, i32))]) -> Result<(), String> {
    for (i, (t, s)) in resizes.iter().enumerate() {
        if resizes[..i].iter().any(|(u, z)| u == t && z != s) {
            return Err(format!("target {t:#x} sized twice in one call"));
        }
    }
    Ok(())
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

/// The frame target behind a role of Halo's target table. The frame has
/// none for the post yet.
fn frame_target(target: PostTarget) -> glow::PostSurface<'static> {
    unimplemented!("post: {target:?} from the frame's targets")
}

/// The frame's colour as the post's surface, at the guest's size.
fn frame_colour<'a>(t: &'a gfx::external::ExternalAttachments, view: &native_render::program::ViewInputs) -> glow::PostSurface<'a> {
    glow::PostSurface { view: &t.color, guest_size: view.guest_size }
}

struct ScreenPass {
    effects: Arc<ScreenEffects>,
    call: ScreenCall,
    raster: Vec<ScreenRaster>,
}

struct ScreenRaster {
    strip: PostRaster,
    convolutions: Vec<PostRaster>,
}

impl ScreenPass {
    /// The call's pass, its raster made from the plan: each template
    /// pass's own states (`ScreenProgram::state`), the view read as `X8`
    /// where `view_opaque` says so, each convolution quad or grid with no
    /// blend and every channel written (`0x000211a0`, `0x00021360`). `Err`
    /// for a pass without its program or out of order
    /// (`screen_gpu::Drawn`).
    fn new(effects: Arc<ScreenEffects>, call: ScreenCall, view_opaque: bool) -> Result<Self, String> {
        let mut drawn = screen_gpu::Drawn::default();
        let mut raster = Vec::with_capacity(call.passes.len());
        for (key, dest, _, convolutions) in &call.passes {
            let program = effects.program(call.egor, key.1, key.2).ok_or("missing screen program")?;
            let reads = drawn.reads(program, *dest).map_err(|e| format!("{e:?}"))?;
            let mut strip = PostRaster::from(program.state);
            for (stage, source) in reads.iter().enumerate() {
                if *source == Some(0) {
                    strip.opaque_view[stage] = view_opaque;
                }
            }
            let convolutions_raster = convolutions.iter().flat_map(|c| &c.draws).map(|_| PostRaster::default()).collect();
            raster.push(ScreenRaster { strip, convolutions: convolutions_raster });
            drawn.pass(program, *dest, convolutions).map_err(|e| format!("{e:?}"))?;
        }
        Ok(Self { effects, call, raster })
    }

    fn effect(&self, key: ScreenKey) -> Option<&native_render::screen_look::ScreenProgram> {
        self.effects.program(self.call.egor, key.1, key.2)
    }

    /// The call on the frame's colour, prepared: the cache's lock is
    /// released when this returns; the commands hold their own handles.
    fn prepare(
        &self,
        t: &gfx::external::ExternalAttachments,
        view: &native_render::program::ViewInputs,
    ) -> Result<screen_gpu::ScreenCommands, screen_gpu::ScreenRefused> {
        let mut screens = self.effects.screens.lock().unwrap_or_else(|e| e.into_inner());
        let mut draws = Vec::new();
        for ((key, dest, strips, convolutions), raster) in self.call.passes.iter().zip(&self.raster) {
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
        let slots = screen_slot_targets(&self.call);
        let role = |i: u8| slots[usize::from(i)].map(|(t, _)| frame_target(t));
        let target = ScreenTarget {
            device: &t.device,
            queue: &t.queue,
            uploads: &t.uploads,
            color: Some(frame_colour(t, view)),
            target1: role(SLOT_TARGET1),
            convolutions: std::array::from_fn(|i| role(SLOT_STAND_INS + i as u8)),
            z_scale: view.z_scale,
        };
        screens
            .prepare(target, &draws, self.call.frame.as_ref(), &mut stage_image)
    }

    /// A copy the GPU could not record: there is no guest draw behind it.
    fn refused(&self, error: screen_gpu::ScreenRefused) -> ! {
        post_feature::unhandled(format!("screen effect {:#010x} on the graph's targets: {error:?}", self.call.egor.0))
    }
}

impl Stage for ScreenPass {
    fn name(&self) -> &'static str {
        "post: screen effect"
    }

    fn record(&mut self, t: &gfx::external::ExternalAttachments, view: &native_render::program::ViewInputs) -> StageWork {
        match self.prepare(t, view) {
            Ok(commands) => StageWork::Encoded(gfx::external::ExternalEncoded {
                ranges: commands.ranges(),
                draws: commands.draws(),
                passes: commands.passes(),
                commands: Box::new(move |encoder: &mut wgpu::CommandEncoder| commands.record(encoder)),
            }),
            Err(error) => self.refused(error),
        }
    }
}

/// The native glow, recorded against the call's view and blur levels.
struct GlowPass {
    /// Per stage, whether the down-sample's entry reads the view as `X8`
    /// (alpha one): [`reads_opaque`] for the stages it binds the view at.
    opaque_entry: [bool; 4],
    plan: Plan,
    gpu: Arc<Mutex<GpuGlow>>,
    entry: Arc<Mutex<OpaqueEntry>>,
}

impl Stage for GlowPass {
    fn name(&self) -> &'static str {
        "post: glow"
    }

    // The blur levels are targets of the glow's own: its quads go in the
    // frame's encoder.
    fn record(&mut self, t: &gfx::external::ExternalAttachments, view: &native_render::program::ViewInputs) -> StageWork {
        let recording = self.prepare(t, view);
        StageWork::Encoded(gfx::external::ExternalEncoded {
            ranges: self.plan.draws.len(),
            draws: recording.passes(),
            passes: recording.passes(),
            commands: Box::new(move |encoder: &mut wgpu::CommandEncoder| recording.record(encoder)),
        })
    }
}

impl GlowPass {
    /// The plan's quads on the frame's colour, prepared, with the plan's
    /// raster (each quad's blend, every channel written: `0x00034a90`'s
    /// `0x01010101`). A recording the GPU cannot make panics: there is no
    /// guest draw behind it. The cache's lock is released when this
    /// returns.
    ///
    /// The down-sample's entry reads the view from the frame's colour,
    /// made opaque first where the guest reads it as `X8`.
    fn prepare(&self, t: &gfx::external::ExternalAttachments, view: &native_render::program::ViewInputs) -> GlowRecording {
        let colour = frame_colour(t, view);
        let opaque = self.opaque_entry;
        let intermediates = [GLOW_TARGETS[1], GLOW_TARGETS[2]].map(|r| Some(frame_target(r)));
        let entry = opaque
            .contains(&true)
            .then(|| self.entry.lock().unwrap_or_else(|e| e.into_inner()).copy(&t.device, colour.view));
        let entry_sources = std::array::from_fn(|n| {
            Some(match (&entry, opaque[n]) {
                (Some(copy), true) => &copy.view,
                _ => colour.view,
            })
        });
        let result = self.gpu.lock().unwrap_or_else(|e| e.into_inner()).prepare(
            GlowTarget {
                device: &t.device,
                queue: &t.queue,
                uploads: &t.uploads,
                color: colour,
                entry_sources,
                intermediates,
            },
            &self.plan,
        );
        match result {
            Ok(commands) => GlowRecording { entry, commands },
            Err(error) => post_feature::unhandled(format!("glow on the frame's targets: {error:?}")),
        }
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

/// The view as an `X8` read sees it: the view copied, then the copy's
/// alpha set to one.
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

struct Glue {
    gpu: Arc<Mutex<GpuGlow>>,
    /// The glow's opaque entry copies ([`OpaqueEntry`]).
    entry: Arc<Mutex<OpaqueEntry>>,
    /// The current map's screen effects, loaded here whatever the native
    /// world does ([`EffectsSlot`]).
    effects: Arc<Mutex<EffectsSlot>>,
}

impl Host for Glue {
    fn retire_map(&self) {
        let retired = self.effects.lock().unwrap_or_else(|e| e.into_inner())
            .take_stale(render_bridge::native_map_work::generation());
        if let Some(retired) = retired {
            crate::native_map::retire_owned(timeline::setup::Stage::World, retired);
        }
    }

    fn glow(&self, mem: &mut dyn GuestMemory, p: &Parameters, fp: Fp) -> Result<(), String> {
        let plan = plan(&*mem, p, fp)?;
        let view = u32::from(mem.u32(glow::VIEW_TARGET).map_err(|_| "the view's target")? as u16);
        let opaque = reads_opaque(&*mem, view)?;
        let entry = plan.draws.first().ok_or("the glow has no down-sample")?;
        let opaque_entry = std::array::from_fn(|n| opaque && entry.stages[n] == Some(Surface::View));
        let pass = GlowPass {
            opaque_entry,
            plan,
            gpu: Arc::clone(&self.gpu),
            entry: Arc::clone(&self.entry),
        };
        crate::native_targets_glue::push(&*mem, Slot::Transparents, pass);
        Ok(())
    }

    fn screen_effect(&self, mem: &mut dyn GuestMemory, call: ScreenEffect, fp: Fp) -> Result<bool, String> {
        let Some((mut effect, values)) = plan_effect(&*mem, call, fp).map_err(|e| format!("plan: {e}"))? else {
            // `0x0002d140` returns at once (no egor, an empty window, or
            // another view's target).
            return Ok(false);
        };
        let saved = post_feature::save_sizes(&*mem).map_err(|e| format!("saved sizes ({e:?})"))?;
        let drawn = template_gates(&*mem, &effect).ok_or(Missing::TemplateSkipped.name())?;
        let sizes = resizes(&effect, &drawn);
        one_size_each(&sizes)?;
        for (t, (w, h)) in &sizes {
            post_feature::resize(mem, *t, *w as u32, *h as u32).map_err(|e| format!("resize ({e:?})"))?;
        }
        let mut gate = drawn.into_iter();
        effect.passes.retain(|_| gate.next().unwrap_or(false));
        if !effect.passes.is_empty() {
            self.draw_effect(mem, call, effect, values, fp)?;
        }
        post_feature::end_writes(mem, &saved)?;
        Ok(true)
    }
}

impl Glue {
    /// The drawn passes of `effect`, called with `values`, as one native
    /// pass. While the map's effects load nothing is drawn (counted
    /// `post_screen_effect_loading`).
    fn draw_effect(
        &self,
        mem: &mut dyn GuestMemory,
        call: ScreenEffect,
        effect: Effect,
        values: [u32; 4],
        fp: Fp,
    ) -> Result<(), String> {
        let egor = TagId(call.datum);
        let Some(effects) = self.screen_effects(&*mem) else {
            EFFECTS_LOADING.fetch_add(1, Ordering::Relaxed);
            if !LOADING_LOGGED.swap(true, Ordering::Relaxed) {
                eprintln!("native-post: a screen effect waits for its map's effects to load (logged once)");
            }
            return Ok(());
        };
        let frame = quad_frame(&*mem, fp);
        let mut m = missing(&effect, &|p| pass_drawable(Some(&*effects), egor, p), frame.is_some());
        if let Err(reason) = call_ordered(Some(&*effects), egor, &effect) {
            m.push(reason);
        }
        if let Some(first) = m.first() {
            return Err(first.name().into());
        }
        let depth = mem.u32(screen_effect::constant::DEPTH).map_err(|_| "the effect's depth")?;
        let grid = |p: &screen_effect::Pass, a: &WideArgs, size: (i32, i32), d: &WideDraw| {
            grid_of(&*mem, fp, &wide_grid(a, size, d, wide_source(&*mem, p, a.source)?))
        };
        let call = screen_call(&effect, egor, values, depth, frame, &grid).ok_or(Missing::Wide.name())?;
        let opaque = reads_opaque(&*mem, 0)?;
        let pass = ScreenPass::new(effects, call, opaque)?;
        crate::native_targets_glue::push(&*mem, Slot::Transparents, pass);
        Ok(())
    }

    /// The loaded map's screen effects: `None` until they load, the first
    /// call on a map starting their load ([`spawn_load`]).
    fn screen_effects(&self, mem: &dyn GuestMemory) -> Option<Arc<ScreenEffects>> {
        map_effects(&self.effects, mem, &mut |name, generation| {
            spawn_load(name, generation, Arc::clone(&self.effects))
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
/// `slot`; a map that does not load draws no screen effect, which the feature names.
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
            Err(e) => eprintln!("native-post: {name}: the screen effects did not load ({e}); no screen effect draws"),
        }
    });
    if let Err(e) = spawned {
        eprintln!(
            "native-post: {e}: the screen effects did not load; no screen effect draws"
        );
    }
}

/// Loads the map's screen effects with the raw resources served by the VFS.
fn load_effects(name: &str) -> Result<ScreenEffects, native_render::load::Error> {
    let (mut map, mut files) = native_map::open(name)?;
    Ok(crate::native_post_load::load(&mut map, &mut files))
}

/// Installs the host. It loads each map's screen effects itself
/// ([`EffectsSlot`]).
pub fn install() {
    render_bridge::native_post::set_host(Arc::new(Glue {
        gpu: Arc::new(Mutex::new(GpuGlow::new())),
        entry: Arc::new(Mutex::new(OpaqueEntry::default())),
        effects: Arc::new(Mutex::new(EffectsSlot::default())),
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The glow's and a screen call's slots are the view and Halo's table
    /// entries by role: each entry's surface header, and only while the
    /// entry is made (`0x0001dcf0`).
    #[test]
    fn post_slots_name_the_view_and_table_roles() {
        use engine::memory::FlatMemory;
        let mut m = FlatMemory::new(0x0040_0000, 0x0020_0000);
        m.set_u32(post_feature::TARGET0_COLOUR, 0x0051_0000).unwrap();
        assert_eq!(post_slot(&m, PostTarget::View), Ok(0x0051_0000));
        let role = PostTarget::Role(TargetRole::Game(7));
        assert!(post_slot(&m, role).is_err(), "target 7 not made");
        let e = glow::TARGETS + 7 * glow::TARGET_STRIDE;
        m.set_u32(e + 0x88, 0x0052_0000).unwrap();
        m.set_u32(e + 0x78, 1).unwrap();
        let header = post_slot(&m, role).unwrap();
        assert_eq!(header, e + 0x18);
        assert_eq!(render_bridge::native_targets::role(header, &|_| None), Some(TargetRole::Game(7)), "the role it names");
        m.write(e + 0x95, &[1]).unwrap();
        assert!(post_slot(&m, role).is_err(), "a hidden size");
        assert!(post_slot(&m, PostTarget::Role(TargetRole::DisplayDepth)).is_err());
        // The glow's view is the bound colour, whichever target the view's is.
        assert_eq!(GLOW_TARGETS[0], PostTarget::Bound);
        m.set_u32(render_bridge::native_targets::CACHE_COLOUR, 0x0053_0000).unwrap();
        assert_eq!(post_slot(&m, PostTarget::Bound).unwrap(), 0x0053_0000);
        assert_eq!(GLOW_TARGETS[1], PostTarget::Role(TargetRole::Game(glow::TARGET_DOWN as u8)));
        assert_eq!(GLOW_TARGETS[2], PostTarget::Role(TargetRole::Game(glow::TARGET_OTHER as u8)));
        m.set_u32(post_feature::TARGET0_COLOUR, 0).unwrap();
        assert!(post_slot(&m, PostTarget::View).is_err(), "no view surface");
    }

    /// The view reads as `X8` where the texture `0x00014f60` binds for it
    /// is `LIN_X8R8G8B8`.
    #[test]
    fn the_view_reads_opaque_through_an_x8_texture() {
        use engine::memory::FlatMemory;
        let mut m = FlatMemory::new(0x0001_0000, 0x0060_0000);
        m.write(post_feature::TEXTURE_CASE, &[2]).unwrap();
        m.set_u32(post_feature::TEXTURE_JUMP + 8, 0x0001_4ffc).unwrap();
        m.set_u32(0x0050_9374, 0x0051_0000).unwrap();
        let format = |f: gfx::D3DFormat| (2 << 4) | (f.raw() << 8) | (1 << 16);
        m.set_u32(0x0051_000c, format(gfx::D3DFormat::LinX8R8G8B8)).unwrap();
        assert_eq!(reads_opaque(&m, 0), Ok(true));
        m.set_u32(0x0051_000c, format(gfx::D3DFormat::LinA8R8G8B8)).unwrap();
        assert_eq!(reads_opaque(&m, 0), Ok(false));
        m.set_u32(post_feature::TEXTURE_JUMP + 8, 0x0001_5050).unwrap();
        assert!(reads_opaque(&m, 0).is_err(), "a case not read");
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

    /// A pass `0x00016610` does not draw is left out of the native pass
    /// (the gate itself: engine's
    /// `the_template_pass_draws_only_an_implementation_with_a_vertex_shader`);
    /// a template that does not read is `template_skipped`.
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
            "drawn natively"
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
            "target 1 as the call came in"
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
    fn a_calls_ranges_and_slots_are_its_strips_and_targets() {
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
        assert_eq!(screen_ranges(&call), 6, "each pass's strips");
        let view = Some((PostTarget::View, true));
        assert_eq!(screen_slot_targets(&call), [None, view, None, None, None, None]);
        let empty = Effect { passes: vec![], ..effect.clone() };
        let call = screen_call(&empty, EGOR, [0; 4], 0, None, &unmade_grid).unwrap();
        assert_eq!(screen_ranges(&call), 0);
        // Target 1's strips go to its slot; the view's is read-only.
        let into_1 = screen_effect::Pass { kind: 1, size: Some((640, 480)), ..drawn_pass(2, 4) };
        let both = Effect { passes: vec![into_1, drawn_pass(3, 4)], ..effect };
        let call = screen_call(&both, EGOR, [0; 4], 0, None, &unmade_grid).unwrap();
        assert_eq!(call.passes[0].1, Dest::Target1 { guest: (640, 480) });
        assert_eq!(screen_ranges(&call), 5);
        let target1 = Some((PostTarget::Role(TargetRole::Game(1)), true));
        assert_eq!(screen_slot_targets(&call)[..3], [None, view, target1]);
        let only_1 = Effect { passes: vec![both.passes[0].clone()], ..both.clone() };
        let call = screen_call(&only_1, EGOR, [0; 4], 0, None, &unmade_grid).unwrap();
        assert_eq!(screen_slot_targets(&call)[1], Some((PostTarget::View, false)), "read, not drawn into");
        // A blur after the first pass: its quads into 0x19 and 0x1a.
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
        let call = screen_call(&blurred, EGOR, [0; 4], 0, None, &unmade_grid).unwrap();
        assert_eq!(screen_ranges(&call), 7, "two strips, two quads, three strips");
        let stand_in = |n: u8| Some((PostTarget::Role(TargetRole::Game(n)), true));
        assert_eq!(screen_slot_targets(&call)[3..], [stand_in(0x19), stand_in(0x1a), None]);
        // The body's resizes: target 1 before the pass's gate, the blur's
        // pair after it; a pass the gate skips resizes target 1 alone.
        let sizes = resizes(&blurred, &[true, true]);
        assert_eq!(sizes, [(1, (640, 480)), (0x19, (320, 240)), (0x1a, (320, 240))]);
        assert_eq!(resizes(&blurred, &[false, true]), [(1, (640, 480))]);
        assert!(one_size_each(&sizes).is_ok());
        assert!(one_size_each(&[(0x19, (320, 240)), (0x19, (160, 120))]).is_err());
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
        let call = screen_call(&widened, EGOR, [0; 4], 0, None, &grid).unwrap();
        assert_eq!(screen_ranges(&call), 9, "two strips, two grids of two, three strips");
        assert!(screen_call(&widened, EGOR, [0; 4], 0, None, &|_, _, _, _| None).is_none(), "a grid not made");
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

    /// A screen pass's raster is its template's states, the view read as
    /// `X8` at the stages that sample it when the view is.
    #[test]
    fn a_screen_pass_rasters_from_its_template() {
        let mut stages = effects([ScreenStage::Engine(5), ScreenStage::Empty, ScreenStage::Empty, ScreenStage::Empty]);
        let blend = Some(wgpu::BlendState::ALPHA_BLENDING);
        for p in stages.effects.get_mut(&EGOR).unwrap().programs.values_mut() {
            p.state.blend = blend;
            p.state.color_writes = wgpu::ColorWrites::COLOR;
        }
        let effects = Arc::new(stages);
        let effect = Effect { shader: 0, implementations: 2, passes: vec![drawn_pass(2, 4)] };
        let call = || screen_call(&effect, EGOR, [0; 4], 0, None, &unmade_grid).unwrap();
        let opaque = ScreenPass::new(Arc::clone(&effects), call(), true).unwrap();
        let strip = opaque.raster[0].strip;
        assert_eq!((strip.blend, strip.writes), (blend, wgpu::ColorWrites::COLOR));
        assert_eq!(strip.opaque_view, [true, false, false, false]);
        assert!(opaque.raster[0].convolutions.is_empty());
        let plain = ScreenPass::new(Arc::clone(&effects), call(), false).unwrap();
        assert_eq!(plain.raster[0].strip.opaque_view, [false; 4]);
        let other = ScreenCall { egor: TagId(0xe000_0009), ..call() };
        assert!(ScreenPass::new(effects, other, false).is_err(), "no program");
    }
}
