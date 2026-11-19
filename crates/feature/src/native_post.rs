//! `native-post`: the view's post passes drawn by the native renderer
//! (docs/graphics/native-renderer.md, "Stage 5: post and HUD"). Off by
//! default; `H2_FEATURES=impl.render.native_post=on` turns it on (exactly
//! that key).
//!
//! The colour pass and each screen effect run their guest code. Entry
//! hooks supply the call's arguments and stack identity. Return hooks
//! close that exact call before its caller can issue another draw.
//!
//! `H2_POST_TRACE` ([`TraceSpec`]) traces the first calls of
//! `0x0002d140`: the host keeps the device's account of what each runs
//! (every D3D entry called and its caller, the swallow armed, every draw
//! and its fate), and a third hook, on `0x00016610`
//! (`points::TEMPLATE_PASS`), adds each template pass the effect sets up.
//!
//! With the switch off the hook is placed only for the frame A/B
//! ([`place_for_capture`]): the host then observes the same quads,
//! dropping none, to capture the view's colour where the native glow
//! would land.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use engine::memory::GuestMemory;
use engine::points;

use crate::{BuildError, Builder, Feature, Flow, Manifest};

pub const ID: &str = "native-post";
/// The switch, looked up exactly.
pub const SWITCH: &str = "impl.render.native_post";

/// Whether the native post is on (off by default).
pub fn on(switches: &crate::Switches) -> bool {
    switches.feature(SWITCH, false)
}

/// The caller's floating-point state at the hook: the numbers are the
/// game's bit for bit only under it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fp {
    pub mxcsr: u32,
    pub x87_control: u16,
}

/// Which guest call owns a post transaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PostCall {
    Colour,
    Screen,
}

impl PostCall {
    fn cleanup(self) -> u32 {
        match self {
            Self::Colour => 28,
            Self::Screen => 16,
        }
    }
}

/// The guest stack at entry, before the feature chain installs its return
/// continuation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CallSite {
    pub kind: PostCall,
    pub esp: u32,
    pub ret: u32,
}

/// What the colour pass becomes: boot's glue over the GPU and the device.
pub trait Host: Send + Sync {
    /// The call of `0x000214f0` with `args` (its seven stack words) is
    /// about to run. `native`: drop its quads and draw the glow natively;
    /// otherwise only observe them (the A/B). `true`: the device is armed.
    fn colour_pass(&self, mem: &mut dyn GuestMemory, site: CallSite, args: [u32; 7], fp: Fp, native: bool) -> bool;

    /// The call of `0x0002d140` described by `call` is about to run (the
    /// switch on). `true`: the device is armed. By default nothing is.
    fn screen_effect(&self, mem: &mut dyn GuestMemory, site: CallSite, call: ScreenEffect, fp: Fp) -> bool {
        let _ = (mem, site, call, fp);
        false
    }

    /// Finishes the call whose entry frame was at `esp`.
    fn finish(&self, _mem: &mut dyn GuestMemory, _kind: PostCall, _esp: u32) {}

    /// `H2_POST_TRACE`: the call of `0x0002d140` described by `call`,
    /// returning to `ret`, is about to run; called before
    /// [`Host::screen_effect`] for it.
    fn trace_effect(&self, mem: &mut dyn GuestMemory, call: ScreenEffect, ret: u32) {
        let _ = (mem, call, ret);
    }

    /// `H2_POST_TRACE`: `0x00016610` called with `args` (its six stack
    /// words), returning to `ret`.
    fn trace_template(&self, mem: &mut dyn GuestMemory, args: [u32; 6], ret: u32) {
        let _ = (mem, args, ret);
    }

    /// `H2_POST_TRACE`: a line for the trace running, if one is.
    fn trace_note(&self, note: String) {
        let _ = note;
    }
}

/// `H2_POST_TRACE` (docs/graphics/native-renderer.md, stage 5): `N` traces
/// each egor's first `N` calls of `0x0002d140` (empty: [`TRACE_CALLS`]).
/// `N,after` also notes the return values of the template and effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TraceSpec {
    pub calls: u64,
    pub after: bool,
}

/// The calls of each egor `H2_POST_TRACE` traces when it names no count.
pub const TRACE_CALLS: u64 = 4;

impl TraceSpec {
    /// The variable's value; `None` when it is not one.
    pub fn parse(s: &str) -> Option<TraceSpec> {
        let mut parts = s.split(',').map(str::trim);
        let calls = match parts.next() {
            None | Some("") => TRACE_CALLS,
            Some(n) => n.parse().ok().filter(|&n| n > 0)?,
        };
        let mut after = false;
        for p in parts {
            match p {
                "after" => after = true,
                _ => return None,
            }
        }
        Some(TraceSpec { calls, after })
    }
}

/// `H2_POST_TRACE`, read once; a value that is not one is said once and
/// traces nothing.
pub fn trace_spec() -> Option<TraceSpec> {
    static SPEC: OnceLock<Option<TraceSpec>> = OnceLock::new();
    *SPEC.get_or_init(|| {
        let v = std::env::var("H2_POST_TRACE").ok()?;
        let spec = TraceSpec::parse(&v);
        if spec.is_none() {
            eprintln!("native-post: H2_POST_TRACE={v:?} is not N or N,after; nothing is traced");
        }
        spec
    })
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

static HOST: Mutex<Option<Arc<dyn Host>>> = Mutex::new(None);
static CAPTURE: AtomicBool = AtomicBool::new(false);
static CALLS: AtomicU64 = AtomicU64::new(0);
static ARMED: AtomicU64 = AtomicU64::new(0);
static EFFECTS: AtomicU64 = AtomicU64::new(0);
static EFFECTS_ARMED: AtomicU64 = AtomicU64::new(0);

/// Installs the host (boot). Without one nothing is armed.
pub fn set_host(host: Arc<dyn Host>) {
    *HOST.lock().unwrap_or_else(|e| e.into_inner()) = Some(host);
}

/// Places the hook with the switch off too, observing (boot, for the
/// frame A/B's D3D side). Before the plan is built.
pub fn place_for_capture(on: bool) {
    CAPTURE.store(on, Ordering::Relaxed);
}

/// The calls so far.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// Calls of the colour pass.
    pub calls: u64,
    /// Calls the device was armed for.
    pub armed: u64,
    /// Calls of `0x0002d140` (the switch on).
    pub effects: u64,
    /// Those the device was armed for.
    pub effects_armed: u64,
}

pub fn counts() -> Counts {
    Counts {
        calls: CALLS.load(Ordering::Relaxed),
        armed: ARMED.load(Ordering::Relaxed),
        effects: EFFECTS.load(Ordering::Relaxed),
        effects_armed: EFFECTS_ARMED.load(Ordering::Relaxed),
    }
}

/// The hook's body on any guest memory: `esp` at the entry (the return
/// address, then the seven words). `true` when the host armed the device.
pub fn on_colour_pass(mem: &mut dyn GuestMemory, esp: u32, fp: Fp, native: bool) -> bool {
    CALLS.fetch_add(1, Ordering::Relaxed);
    let mut args = [0u32; 7];
    for (i, a) in args.iter_mut().enumerate() {
        let Ok(v) = mem.u32(esp.wrapping_add(4 + 4 * i as u32)) else {
            return false;
        };
        *a = v;
    }
    // HOST is only held to clone the host: never across its calls.
    let Some(host) = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone() else {
        return false;
    };
    let Ok(ret) = mem.u32(esp) else { return false };
    let site = CallSite { kind: PostCall::Colour, esp, ret };
    let armed = host.colour_pass(mem, site, args, fp, native);
    if armed {
        ARMED.fetch_add(1, Ordering::Relaxed);
    }
    armed
}

/// The screen-effect hook's body: `eax` the datum, `esp` at the entry
/// (the return address, then the four words). `true` when the host armed
/// the device.
pub fn on_screen_effect(mem: &mut dyn GuestMemory, eax: u32, esp: u32, fp: Fp) -> bool {
    EFFECTS.fetch_add(1, Ordering::Relaxed);
    let mut w = [0u32; 4];
    for (i, a) in w.iter_mut().enumerate() {
        let Ok(v) = mem.u32(esp.wrapping_add(4 + 4 * i as u32)) else {
            return false;
        };
        *a = v;
    }
    let call = ScreenEffect {
        datum: eax,
        values: w[0],
        columns: w[1],
        rows: w[2],
        corners: w[3],
    };
    let Some(host) = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone() else {
        return false;
    };
    let Ok(ret) = mem.u32(esp) else { return false };
    let site = CallSite { kind: PostCall::Screen, esp, ret };
    if trace_spec().is_some() {
        host.trace_effect(mem, call, ret);
    }
    let armed = host.screen_effect(mem, site, call, fp);
    if armed {
        EFFECTS_ARMED.fetch_add(1, Ordering::Relaxed);
    }
    armed
}

/// The return continuation runs before the guest caller resumes.
pub fn finish(mem: &mut dyn GuestMemory, kind: PostCall, esp: u32) {
    if let Some(host) = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone() {
        host.finish(mem, kind, esp.wrapping_sub(4 + kind.cleanup()));
    }
}

/// `H2_POST_TRACE`'s hook on `0x00016610`: `esp` at the entry (the
/// return address, then the six words).
pub fn on_template_pass(mem: &mut dyn GuestMemory, esp: u32) {
    let mut args = [0u32; 6];
    for (i, a) in args.iter_mut().enumerate() {
        let Ok(v) = mem.u32(esp.wrapping_add(4 + 4 * i as u32)) else {
            return;
        };
        *a = v;
    }
    let ret = mem.u32(esp).unwrap_or(0);
    if let Some(host) = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone() {
        host.trace_template(mem, args, ret);
    }
}

/// `H2_POST_TRACE=N,after`: `what` returned with `eax`.
pub fn on_return(what: &str, eax: u32) {
    if let Some(host) = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone() {
        host.trace_note(return_line(what, eax));
    }
}

/// The line [`on_return`] adds.
pub fn return_line(what: &str, eax: u32) -> String {
    format!("{what} returned, AL {}", eax & 0xff)
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
        // are keyed by the map name it reads from guest RAM, and its proof
        // and fallback sets (by effect tag) only ever send an effect back
        // to the D3D path, which draws the same picture.
        b.snapshot(crate::SnapshotPolicy::Stateless);
        let native = on(b.switches());
        if !native && !CAPTURE.load(Ordering::Relaxed) {
            return Ok(());
        }
        b.raw()
            .before(&points::COLOUR_PASS, "native_post_colour", move |c| {
                let rt = Arc::clone(c.ctx.runtime());
                let fp = Fp {
                    mxcsr: c.cpu.mxcsr,
                    x87_control: c.cpu.x87.control,
                };
                let mut mem = rt.memory();
                on_colour_pass(&mut *mem, c.cpu.esp, fp, native);
                Ok(Flow::Continue)
            })
            .context_only();
        b.raw().after(&points::COLOUR_PASS, "native_post_colour_exit", |c| {
            let rt = Arc::clone(c.ctx.runtime());
            finish(&mut *rt.memory(), PostCall::Colour, c.cpu.esp);
            Ok(())
        });
        if native {
            b.raw()
                .before(&points::SCREEN_EFFECT, "native_post_screen_effect", move |c| {
                    let rt = Arc::clone(c.ctx.runtime());
                    let fp = Fp {
                        mxcsr: c.cpu.mxcsr,
                        x87_control: c.cpu.x87.control,
                    };
                    let mut mem = rt.memory();
                    if super::h1_scope_zoom::hud_profile(&*mem, c.cpu.eax, c.cpu.esp).is_none() {
                        on_screen_effect(&mut *mem, c.cpu.eax, c.cpu.esp, fp);
                    }
                    Ok(Flow::Continue)
                })
                .context_only();
            b.raw().after(&points::SCREEN_EFFECT, "native_post_screen_exit", |c| {
                let rt = Arc::clone(c.ctx.runtime());
                if trace_spec().is_some_and(|trace| trace.after) {
                    on_return("0x0002d140", c.cpu.eax);
                }
                finish(&mut *rt.memory(), PostCall::Screen, c.cpu.esp);
                Ok(())
            });
            if let Some(trace) = trace_spec() {
                b.raw()
                    .before(&points::TEMPLATE_PASS, "native_post_trace_template", |c| {
                        let rt = Arc::clone(c.ctx.runtime());
                        let mut mem = rt.memory();
                        on_template_pass(&mut *mem, c.cpu.esp);
                        Ok(Flow::Continue)
                    })
                    .context_only()
                    .optional();
                if trace.after {
                    b.raw()
                        .observe_after(&points::TEMPLATE_PASS, "native_post_trace_template_return", |o| {
                            on_return("0x00016610", o.cpu.eax)
                        })
                        .optional();
                }
            }
        }
        Ok(())
    }
}
