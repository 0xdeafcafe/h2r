//! `native-fog-quad`: the planar fog's screen quad (`0x00025d50`, view
//! modes 2 and 3) drawn by the native renderer, the guest's fan cut
//! (docs/graphics/retire-d3d-fog-quad.md). On with the native renderer;
//! `impl.render.native-fog-quad=off` runs the guest's fan.
//!
//! | Guest | Cut |
//! |---|---|
//! | the setup `0x00026520`, called by the fan `0x00034420` (return `0x0003452e`) of `0x00025d50` (return `0x00025d9d`) with the corners `0x00026710` | the quad from `engine::planar_fog` drawn by [`Host::draw`] (`native_render::fog_quad`) at this point in the frame; the setup returns as its gate returns when the density is not above 0: the depth word written, AL 0 |
//!
//! So the guest makes the path it makes with no fog to draw: the fan's
//! draw-state reset `0x00016b10` and rectangle, no setup state, no fan,
//! then `0x00025d50`'s depth clip control. No later draw can depend on the
//! state the setup leaves, as that path leaves none.
//!
//! The arm: the first call, and any call whose view target or bound pair
//! is not the one the last reference left, runs the guest body as a
//! reference: its writes (the combiner
//! block, the depth word, each render state's game and D3D words) are
//! compared with the cut's ([`engine::planar_fog::differences`]); a
//! difference sends every later call to the guest body for the session,
//! counted `fog_quad_mismatch_<field>`. A reference whose target select
//! `0x00014bc0` binds another pair proves nothing (`fog_quad_select_bound`):
//! the cut draws into the bound target and needs the select to keep it.
//!
//! Once a target is proven the guest body does not run for it: a quad the
//! cut cannot draw there returns as the gate does, with no fan, and is
//! `scene_dropped_fog_quad_<cause>` (`host`, `placed`, a fan that is not
//! one quad: `fan_shape`, `fan_unfinite`), as is a quad placed but not
//! recorded on the GPU.
//!
//! Every fan the guest draws is one `scene_kept_fog_quad_<cause>`: the
//! reference's `unproven` or `target`, `fallback`, `unread`, `simd`,
//! `x87`, and `other_setup` for the fan of the setup `0x00025e50` (modes
//! 1, 4 and 5), which nothing here draws.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use engine::memory::{GuestMemory, MemFault};
use engine::planar_fog::{self as pf, FogQuad, Setup};
use hooks::{BuildError, Builder, Feature, Manifest};
pub use native_render::fog_quad::FogQuadDraw;

pub const ID: &str = "native-fog-quad";
/// The switch that turns the cut off.
pub const SWITCH: &str = "impl.render.native-fog-quad";

/// The target select's cache of the bound pair (`0x0001cd30`).
pub const BOUND_COLOUR: u32 = 0x0051_f3f4;
pub const BOUND_DEPTH: u32 = 0x0051_f3f8;

/// What draws the quad: boot's glue over the shared device.
pub trait Host: Send + Sync {
    /// Places `draw` at this point in the frame. `false`: not placed,
    /// nothing recorded.
    fn draw(&self, mem: &mut dyn GuestMemory, draw: &FogQuadDraw) -> bool;
}

static HOST: Mutex<Option<Arc<dyn Host>>> = Mutex::new(None);

/// Installs the host (boot). Without one every call runs the guest body.
pub fn set_host(host: Arc<dyn Host>) {
    *HOST.lock().unwrap_or_else(|e| e.into_inner()) = Some(host);
}

fn host() -> Option<Arc<dyn Host>> {
    HOST.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Whether the cut is placed: with the native renderer on, unless
/// `impl.render.native-fog-quad=off`.
pub fn on(switches: &hooks::Switches) -> bool {
    hooks::switches::native_render_on(switches) && switches.feature("impl.render.native-fog-quad", true)
}

/// The view's target and the pair bound when the setup is called.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    pub view: u16,
    pub colour: u32,
    pub depth: u32,
}

impl Target {
    pub fn read(mem: &dyn GuestMemory) -> Result<Self, MemFault> {
        Ok(Self { view: mem.u16(pf::global::TARGET)?, colour: mem.u32(BOUND_COLOUR)?, depth: mem.u32(BOUND_DEPTH)? })
    }
}

#[derive(Default)]
struct Counts {
    calls: u64,
    cut: u64,
    referenced: u64,
    matched: u64,
    undrawn: u64,
    select_bound: u64,
    kept: BTreeMap<&'static str, u64>,
    dropped: BTreeMap<&'static str, u64>,
    mismatched: BTreeMap<String, u64>,
}

static COUNTS: Mutex<Option<Counts>> = Mutex::new(None);
static FALLBACK: AtomicBool = AtomicBool::new(false);
/// The target the last clean reference was called with and left.
static PROOF: Mutex<Option<Target>> = Mutex::new(None);

fn counts<R>(f: impl FnOnce(&mut Counts) -> R) -> R {
    f(COUNTS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Counts::default))
}

fn keep(cause: &'static str) {
    counts(|c| *c.kept.entry(cause).or_default() += 1);
}

/// A quad placed but not recorded on the GPU.
pub fn dropped(cause: &'static str) {
    counts(|c| *c.dropped.entry(cause).or_default() += 1);
}

pub fn fallen_back() -> bool {
    FALLBACK.load(Ordering::Relaxed)
}

fn proof() -> Option<Target> {
    *PROOF.lock().unwrap_or_else(|e| e.into_inner())
}

/// What the setup call does.
#[derive(Debug, PartialEq, Eq)]
pub enum Turn {
    /// Not the quad's call, or the guest draws it: run the body.
    Guest,
    /// Run the body, then compare its writes ([`referenced`]).
    Reference(Reference),
    /// The quad is placed and the gate's writes are made: return AL 0.
    Return,
}

/// A reference call: the cut's quad and the target it was called with.
#[derive(Debug, PartialEq, Eq)]
pub struct Reference {
    pub expected: FogQuad,
    pub before: Target,
}

/// Whether the setup whose stack is at `esp` is the fan's of `0x00025d50`
/// with its corners: the fan's frame above it holds the return to
/// `0x00025d50`, then the rectangle's address and the callback. The
/// rectangle's address, if so.
pub fn quad_call(mem: &dyn GuestMemory, esp: u32) -> Option<u32> {
    let fan = esp.wrapping_add(pf::SETUP_FRAME);
    let word = |a: u32| mem.u32(a).ok();
    (word(esp)? == pf::SETUP_RETURN && word(fan)? == pf::QUAD_RETURN && word(fan.wrapping_add(8))? == pf::CORNERS)
        .then(|| word(fan.wrapping_add(4)))
        .flatten()
}

fn cause(e: engine::light_constants::Error) -> &'static str {
    match e {
        engine::light_constants::Error::Simd => "simd",
        engine::light_constants::Error::X87(_) => "x87",
        _ => "unread",
    }
}

fn fan(q: &FogQuad) -> [[f32; 4]; 4] {
    q.positions.map(|p| p.map(f32::from_bits))
}

/// The setup call at `esp` under the call's MXCSR and x87 control word,
/// `host` the one drawing.
pub fn turn(mem: &mut dyn GuestMemory, esp: u32, mxcsr: u32, x87_control: u16, host: Option<&dyn Host>) -> Turn {
    let Some(rect_at) = quad_call(&*mem, esp) else {
        return Turn::Guest;
    };
    counts(|c| c.calls += 1);
    match pf::draws(&*mem, mxcsr) {
        Ok(true) => {}
        Ok(false) => {
            // The body stops at its gate: no state, no fan.
            counts(|c| c.undrawn += 1);
            return Turn::Guest;
        }
        Err(e) => {
            keep(cause(e));
            return Turn::Guest;
        }
    }
    if fallen_back() {
        keep("fallback");
        return Turn::Guest;
    }
    let expected = match pf::quad(&*mem, mxcsr, x87_control, rect_at) {
        Ok(q) => q,
        Err(e) => {
            keep(cause(e));
            return Turn::Guest;
        }
    };
    let Ok(before) = Target::read(&*mem) else {
        keep("unread");
        return Turn::Guest;
    };
    let proven = proof();
    if proven != Some(before) {
        keep(if proven.is_none() { "unproven" } else { "target" });
        return Turn::Reference(Reference { expected, before });
    }
    // Proven: the body does not run. A quad not drawn is dropped, counted.
    let placed = match host {
        None => Err("host"),
        Some(host) => match FogQuadDraw::from_fan(expected.combiner, fan(&expected)) {
            Err(shape) => Err(shape.cause()),
            Ok(draw) if !host.draw(mem, &draw) => Err("placed"),
            Ok(_) => Ok(()),
        },
    };
    if let Err(why) = placed {
        dropped(why);
    }
    if pf::gate_writes(mem).is_err() {
        // The quad is placed and cannot be taken back; the depth word is
        // the one write the gate makes.
        mismatch(&["depth_write".to_owned()]);
    }
    if placed.is_ok() {
        counts(|c| c.cut += 1);
    }
    Turn::Return
}

/// After a reference body returned with `al`: its writes compared with the
/// cut's, and the proof the next calls cut against.
pub fn referenced(mem: &dyn GuestMemory, al: u8, r: Reference) {
    counts(|c| c.referenced += 1);
    let mut fields = match pf::differences(mem, &r.expected) {
        Ok(f) => f,
        Err(_) => vec!["unread".to_owned()],
    };
    if al != 1 {
        fields.push("al".to_owned());
    }
    if !fields.is_empty() {
        mismatch(&fields);
        return;
    }
    counts(|c| c.matched += 1);
    let after = Target::read(mem).ok();
    let mut p = PROOF.lock().unwrap_or_else(|e| e.into_inner());
    if after == Some(r.before) {
        *p = Some(r.before);
    } else {
        *p = None;
        drop(p);
        counts(|c| c.select_bound += 1);
    }
}

/// A reference that differs: every later call runs the guest body.
pub fn mismatch(fields: &[String]) {
    counts(|c| {
        for f in fields {
            *c.mismatched.entry(f.clone()).or_default() += 1;
        }
    });
    *PROOF.lock().unwrap_or_else(|e| e.into_inner()) = None;
    if !FALLBACK.swap(true, Ordering::Relaxed) {
        eprintln!("native-fog-quad: the setup's writes differ from the cut's ({fields:?}); the guest body draws the planar fog quad from here");
    }
}

/// `0x00025d50`'s entry: the fan of the setup `0x00025e50`, which nothing
/// here draws, is counted kept.
pub fn before_fog(mem: &dyn GuestMemory) {
    match pf::setup(mem) {
        Ok(Setup::Other) => keep("other_setup"),
        Ok(_) => {}
        Err(_) => keep("unread_setup"),
    }
}

/// `render.scene` rows: `fog_quad_{calls,cut,referenced,matched,undrawn,
/// select_bound,proven,fallback}`, the fans the guest draws by cause
/// (`scene_kept_fog_quad_<cause>`), the quads placed but not recorded
/// (`scene_dropped_fog_quad_<cause>`) and the references' differences
/// (`fog_quad_mismatch_<field>`).
pub fn counters() -> Vec<(String, u64)> {
    counts(|c| {
        let mut out = vec![
            ("fog_quad_calls".to_owned(), c.calls),
            ("fog_quad_cut".to_owned(), c.cut),
            ("fog_quad_referenced".to_owned(), c.referenced),
            ("fog_quad_matched".to_owned(), c.matched),
            ("fog_quad_undrawn".to_owned(), c.undrawn),
            ("fog_quad_select_bound".to_owned(), c.select_bound),
            ("fog_quad_proven".to_owned(), u64::from(proof().is_some())),
            ("fog_quad_fallback".to_owned(), u64::from(fallen_back())),
        ];
        out.extend(c.kept.iter().map(|(k, n)| (format!("scene_kept_fog_quad_{k}"), *n)));
        out.extend(c.dropped.iter().map(|(k, n)| (format!("scene_dropped_fog_quad_{k}"), *n)));
        out.extend(c.mismatched.iter().map(|(k, n)| (format!("fog_quad_mismatch_{k}"), *n)));
        out
    })
}

fn original(cpu: &mut runtime::CpuAbi, ctx: &mut runtime::ExecCtx, addr: u32) -> Result<runtime::TransferAbi, runtime::GuestFault> {
    if let Some(original) = ctx.runtime().resolve_original(addr) {
        return Ok(original.invoke(cpu, ctx, addr));
    }
    let event = ctx.emit(runtime::Event::UnsupportedTarget { pc: cpu.eip, target: addr, kind: runtime::abi::JUMP, route: runtime::abi::ENTRY });
    Ok(runtime::TransferAbi::fault(event))
}

fn setup_call(cpu: &mut runtime::CpuAbi, ctx: &mut runtime::ExecCtx, addr: u32) -> Result<runtime::TransferAbi, runtime::GuestFault> {
    if addr != engine::points::PLANAR_FOG_SETUP.base {
        return original(cpu, ctx, addr);
    }
    let rt = Arc::clone(ctx.runtime());
    let turn = {
        let mut mem = rt.memory();
        turn(&mut *mem, cpu.esp, cpu.mxcsr, cpu.x87.control, host().as_deref())
    };
    match turn {
        Turn::Guest => original(cpu, ctx, addr),
        Turn::Reference(r) => {
            let out = original(cpu, ctx, addr)?;
            let mem = rt.memory();
            referenced(&*mem, cpu.eax as u8, r);
            Ok(out)
        }
        Turn::Return => {
            cpu.eax &= !0xff;
            rt.memory().ret(cpu, 4)
        }
    }
}

pub struct NativeFogQuad;

impl Feature for NativeFogQuad {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "The planar fog's screen quad drawn by the native renderer").doc("docs/graphics/retire-d3d-fog-quad.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        // Each call is drawn from the guest's memory of that call; the
        // proof is checked against the bound target at each.
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        if !on(b.switches()) {
            return Ok(());
        }
        b.raw()
            .observe_before(&engine::points::PLANAR_FOG, "native_fog_quad_branch", |o| before_fog(&*o.guest()))
            .context_only();
        b.raw().implement(&engine::points::PLANAR_FOG_SETUP, "native_fog_quad", setup_call).context_only();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::memory::FlatMemory;
    use engine::planar_fog::global;

    /// The counters, the proof and the fallback are shared.
    static SERIAL: Mutex<()> = Mutex::new(());

    fn reset() -> std::sync::MutexGuard<'static, ()> {
        let g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        *COUNTS.lock().unwrap() = None;
        *PROOF.lock().unwrap() = None;
        FALLBACK.store(false, Ordering::Relaxed);
        g
    }

    const ESP: u32 = 0x0050_0000;
    const RECT: u32 = 0x0050_1000;
    const MXCSR: u32 = 0x1f80;
    const CONTROL: u16 = 0x027f;

    /// A setup call from the fan of `0x00025d50` over 640x480, density
    /// 0.5, the view's target 0 bound.
    fn mem() -> FlatMemory {
        let mut m = FlatMemory::new(0x0040_0000, 0x0012_0000);
        let f = |m: &mut FlatMemory, a: u32, v: f32| m.set_f32(a, v).unwrap();
        f(&mut m, engine::screen_grid::constant::ONE, 1.0);
        f(&mut m, engine::screen_grid::constant::DEPTH, 16_777_215.0);
        f(&mut m, engine::screen_grid::constant::NEAR, 0.0625);
        f(&mut m, engine::screen_grid::constant::FAR, 1024.0);
        f(&mut m, global::SCALE, 255.0);
        f(&mut m, global::CLIP_FAR, 1024.0);
        f(&mut m, global::DENSITY, 0.5);
        for (i, v) in [0.0f32, 640.0, 0.0, 480.0].into_iter().enumerate() {
            f(&mut m, RECT + 4 * i as u32, v);
        }
        m.set_u32(ESP, pf::SETUP_RETURN).unwrap();
        m.set_u32(ESP + pf::SETUP_FRAME, pf::QUAD_RETURN).unwrap();
        m.set_u32(ESP + pf::SETUP_FRAME + 4, RECT).unwrap();
        m.set_u32(ESP + pf::SETUP_FRAME + 8, pf::CORNERS).unwrap();
        m.set_u32(BOUND_COLOUR, 0x00aa_0000).unwrap();
        m.set_u32(BOUND_DEPTH, 0x00bb_0000).unwrap();
        m
    }

    /// The body's writes as `0x00026520` makes them.
    fn body(m: &mut FlatMemory, q: &FogQuad) {
        for (i, w) in q.combiner.iter().enumerate() {
            m.set_u32(global::BLOCK + 4 * i as u32, *w).unwrap();
        }
        pf::gate_writes(m).unwrap();
        for (_, game, d3d, value) in pf::STATES {
            m.set_u32(game, value).unwrap();
            if d3d != 0 {
                m.set_u32(d3d, value).unwrap();
            }
        }
    }

    #[derive(Default)]
    struct Draws(Mutex<Vec<FogQuadDraw>>, bool);

    impl Host for Draws {
        fn draw(&self, _: &mut dyn GuestMemory, draw: &FogQuadDraw) -> bool {
            self.0.lock().unwrap().push(draw.clone());
            !self.1
        }
    }

    fn count(name: &str) -> u64 {
        counters().into_iter().find(|(k, _)| k == name).map_or(0, |(_, n)| n)
    }

    fn reference(m: &mut FlatMemory, host: &Draws) {
        let Turn::Reference(r) = turn(m, ESP, MXCSR, CONTROL, Some(host)) else { panic!("a reference") };
        let q = r.expected.clone();
        body(m, &q);
        referenced(m, 1, r);
    }

    #[test]
    fn the_first_call_is_a_reference_and_the_next_is_cut_without_the_body() {
        let _g = reset();
        let mut m = mem();
        let host = Draws::default();
        reference(&mut m, &host);
        assert_eq!(count("fog_quad_matched"), 1);
        assert_eq!(count("fog_quad_proven"), 1);
        assert_eq!(count("scene_kept_fog_quad_unproven"), 1);
        assert!(host.0.lock().unwrap().is_empty(), "the reference draws on the guest");

        m.set_u32(global::DEPTH, 0).unwrap();
        let before = m.bytes(0x0040_0000, 0x0012_0000).unwrap();
        assert_eq!(turn(&mut m, ESP, MXCSR, CONTROL, Some(&host)), Turn::Return);
        let drawn = host.0.lock().unwrap().clone();
        assert_eq!(drawn.len(), 1);
        assert_eq!(drawn[0].rect, [0.0, 0.0, 640.0, 480.0]);
        assert_eq!(drawn[0].w, 16_777_215.0);
        // The only write is the gate's: the depth word.
        let after = m.bytes(0x0040_0000, 0x0012_0000).unwrap();
        let changed: Vec<u32> = (0..before.len()).filter(|&i| before[i] != after[i]).map(|i| 0x0040_0000 + i as u32).collect();
        assert!(!changed.is_empty());
        assert!(changed.iter().all(|&a| (global::DEPTH..global::DEPTH + 4).contains(&a)));
        assert_eq!(m.u32(global::DEPTH).unwrap(), 1024.0f32.to_bits());
        assert_eq!(count("fog_quad_cut"), 1);
    }

    #[test]
    fn a_new_target_is_a_reference_and_a_proven_one_never_runs_the_body() {
        let _g = reset();
        let mut m = mem();
        let host = Draws::default();
        reference(&mut m, &host);
        for _ in 0..256 {
            assert_eq!(turn(&mut m, ESP, MXCSR, CONTROL, Some(&host)), Turn::Return);
        }
        assert_eq!(count("scene_kept_fog_quad_check"), 0);
        m.set_u32(BOUND_COLOUR, 0x00cc_0000).unwrap();
        reference(&mut m, &host);
        assert_eq!(count("scene_kept_fog_quad_target"), 1);
        assert_eq!(turn(&mut m, ESP, MXCSR, CONTROL, Some(&host)), Turn::Return);
    }

    #[test]
    fn a_reference_that_differs_sends_the_session_to_the_body() {
        let _g = reset();
        let mut m = mem();
        let host = Draws::default();
        let Turn::Reference(r) = turn(&mut m, ESP, MXCSR, CONTROL, Some(&host)) else { panic!() };
        let q = r.expected.clone();
        body(&mut m, &q);
        m.set_u32(global::BLOCK + 4 * 43, 0).unwrap();
        referenced(&m, 1, r);
        assert_eq!(count("fog_quad_mismatch_block_43"), 1);
        assert!(fallen_back());
        assert_eq!(turn(&mut m, ESP, MXCSR, CONTROL, Some(&host)), Turn::Guest);
        assert_eq!(count("scene_kept_fog_quad_fallback"), 1);
        assert!(host.0.lock().unwrap().is_empty());
    }

    #[test]
    fn a_reference_whose_select_binds_another_pair_proves_nothing() {
        let _g = reset();
        let mut m = mem();
        let host = Draws::default();
        let Turn::Reference(r) = turn(&mut m, ESP, MXCSR, CONTROL, Some(&host)) else { panic!() };
        let q = r.expected.clone();
        body(&mut m, &q);
        m.set_u32(BOUND_COLOUR, 0x00dd_0000).unwrap();
        referenced(&m, 1, r);
        assert_eq!(count("fog_quad_select_bound"), 1);
        assert!(!fallen_back());
        assert!(matches!(turn(&mut m, ESP, MXCSR, CONTROL, Some(&host)), Turn::Reference(_)));
    }

    #[test]
    fn a_proven_call_the_cut_does_not_draw_is_dropped_counted_by_cause() {
        let _g = reset();
        let mut m = mem();
        let refusing = Draws(Mutex::new(Vec::new()), true);
        reference(&mut m, &refusing);
        let cut = count("fog_quad_cut");
        assert_eq!(turn(&mut m, ESP, MXCSR, CONTROL, Some(&refusing)), Turn::Return);
        assert_eq!(count("scene_dropped_fog_quad_placed"), 1);
        assert_eq!(turn(&mut m, ESP, MXCSR, CONTROL, None), Turn::Return);
        assert_eq!(count("scene_dropped_fog_quad_host"), 1);
        assert_eq!(count("fog_quad_cut"), cut, "a drop is not a cut");
        assert_eq!(count("scene_kept_fog_quad_placed") + count("scene_kept_fog_quad_host"), 0);
        // No fog to draw: the body stops at its gate, nothing is kept.
        m.set_f32(global::DENSITY, 0.0).unwrap();
        assert_eq!(turn(&mut m, ESP, MXCSR, CONTROL, Some(&refusing)), Turn::Guest);
        assert_eq!(count("fog_quad_undrawn"), 1);
        // Another caller is not the quad's.
        m.set_u32(ESP + pf::SETUP_FRAME + 8, 0x0002_54c0).unwrap();
        assert_eq!(quad_call(&m, ESP), None);
        let calls = count("fog_quad_calls");
        assert_eq!(turn(&mut m, ESP, MXCSR, CONTROL, None), Turn::Guest);
        assert_eq!(count("fog_quad_calls"), calls);
    }

    #[test]
    fn the_other_setups_fan_is_counted_kept() {
        let _g = reset();
        let mut m = mem();
        m.set_u32(global::MODE, 4).unwrap();
        before_fog(&m);
        m.set_u32(global::MODE, 3).unwrap();
        before_fog(&m);
        assert_eq!(count("scene_kept_fog_quad_other_setup"), 1);
    }

    /// The pass draws with the state the setup sets.
    #[test]
    fn the_native_raster_is_the_setups_state() {
        use native_render::fog_quad as nq;
        let state = |name: &str| pf::STATES.iter().find(|s| s.0 == name).map(|s| s.3).unwrap();
        assert_eq!(state("blend_source"), nq::BLEND_SOURCE);
        assert_eq!(state("blend_dest"), nq::BLEND_DEST);
        assert_eq!(state("blend_equation"), nq::BLEND_EQUATION);
        assert_eq!(state("blend_colour"), nq::BLEND_COLOUR);
        assert_eq!(state("z_enable"), nq::Z_ENABLE);
        assert_eq!(state("z_func"), nq::Z_FUNC);
        assert_eq!(pf::WORDS, nq::WORDS);
    }
}
