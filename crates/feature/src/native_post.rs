//! `native-post`: the view's post passes drawn by the native renderer
//! (docs/graphics/native-renderer.md, "Stage 5: post and HUD"). Off by
//! default; `H2_FEATURES=impl.render.native_post=on` turns it on (exactly
//! that key).
//!
//! The colour pass of `render_screen_effects`, a glow (`0x000214f0`,
//! `points::COLOUR_PASS`), runs as the game's own code, so it leaves
//! every state, register and guest word as it always does; only its quads
//! are not drawn by the D3D path. One hook, a context-only `before` with
//! nothing after its body (the pipelined render runs it on its own
//! thread), reads the call's seven arguments and hands them to the
//! [`Host`]. The host plans the glow from them, and arms the device to
//! drop exactly the plan's quads and draw the native glow in place of the
//! last (`d3d::Device::swallow`); a quad the plan does not have ends that
//! and the D3D path draws the rest. The call always runs.
//!
//! A second hook, on each of the scenario's screen effects (`0x0002d140`,
//! `points::SCREEN_EFFECT`, the switch on only), hands the host EAX and
//! the four stack words ([`ScreenEffect`]); the host plans the effect
//! (`engine::screen_effect::plan`) and arms nothing until every piece it
//! needs draws natively, refusing per reason. The call always runs.
//!
//! With the switch off the hook is placed only for the frame A/B
//! ([`place_for_capture`]): the host then observes the same quads,
//! dropping none, to capture the view's colour where the native glow
//! would land.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

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

/// What the colour pass becomes: boot's glue over the GPU and the device.
pub trait Host: Send + Sync {
    /// The call of `0x000214f0` with `args` (its seven stack words) is
    /// about to run. `native`: drop its quads and draw the glow natively;
    /// otherwise only observe them (the A/B). `true`: the device is armed.
    fn colour_pass(&self, mem: &mut dyn GuestMemory, args: [u32; 7], fp: Fp, native: bool) -> bool;

    /// The call of `0x0002d140` described by `call` is about to run (the
    /// switch on). `true`: the device is armed. By default nothing is.
    fn screen_effect(&self, mem: &mut dyn GuestMemory, call: ScreenEffect, fp: Fp) -> bool {
        let _ = (mem, call, fp);
        false
    }
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
    let armed = host.colour_pass(mem, args, fp, native);
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
    let armed = host.screen_effect(mem, call, fp);
    if armed {
        EFFECTS_ARMED.fetch_add(1, Ordering::Relaxed);
    }
    armed
}

pub struct NativePost;

impl Feature for NativePost {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "The view's post passes drawn by the native GPU renderer").doc("docs/graphics/native-renderer.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
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
        if native {
            b.raw()
                .before(&points::SCREEN_EFFECT, "native_post_screen_effect", move |c| {
                    let rt = Arc::clone(c.ctx.runtime());
                    let fp = Fp {
                        mxcsr: c.cpu.mxcsr,
                        x87_control: c.cpu.x87.control,
                    };
                    let mut mem = rt.memory();
                    on_screen_effect(&mut *mem, c.cpu.eax, c.cpu.esp, fp);
                    Ok(Flow::Continue)
                })
                .context_only();
        }
        b.snapshot(crate::SnapshotPolicy::Stateless);
        Ok(())
    }
}
