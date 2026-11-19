//! `native-post`: the view's post passes drawn by the native renderer
//! (docs/graphics/native-renderer.md, "Stage 5: post and HUD"). Off by
//! default; `H2_FEATURES=impl.render.native_post=on` turns it on (exactly
//! that key).
//!
//! `render_scene_passes` ends each view's scene with layer 17, `0x00020770`
//! and `render_screen_effects` (`0x00013870`, `points::RENDER_SCREEN_EFFECTS`):
//! the colour pass over the view's target, the scenario's screen effects
//! and `0x0002df00` (`native_render::post`). One hook, context-only and
//! with nothing after a body, so the pipelined render runs it on its own
//! thread (docs/graphics/pipelined-render.md, "Render-safe hooks"):
//!
//! - a `before` on `render_screen_effects`: reads what the call was asked
//!   to draw ([`PostFrame`]) and counts it; when the [`Host`] draws it,
//!   the host asks the device for an external pass and the hook answers
//!   `ret 8` in place of the call. The device issues the pass at its next
//!   entry: the first draw after the scene passes return, which is the
//!   fade (`0x00040890`) or the HUD and UI (`0x00147af1`) of
//!   `render_view_scene`, so the native post lands where the game's did,
//!   after the scene and before the HUD.
//!
//! Slice 1 has no host in boot: nothing is skipped, and the counts say
//! what each view's call asks for (the live run's census of what 03a
//! draws after the scene).
//!
//! With `render-timing` on nothing is placed: its timer on the scene's
//! callees needs the call's end, which a skipped call never reaches.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use engine::points;
use native_render::camera::GuestRead;
use native_render::post::{PostFrame, SCREEN_EFFECTS_CLEANUP};

use crate::{BuildError, Builder, Feature, Flow, Manifest};

pub const ID: &str = "native-post";
/// The switch, looked up exactly.
pub const SWITCH: &str = "impl.render.native_post";

/// Whether the native post is on (off by default).
pub fn on(switches: &crate::Switches) -> bool {
    switches.feature(SWITCH, false)
}

/// What draws the post: boot's glue over the GPU and the device.
pub trait Host: Send + Sync {
    /// Whether the host draws this call's passes natively now. It must
    /// also leave what the call's prologue leaves (the shader draw state
    /// reset `0x00016b10`, `[0x004670bc]`, `0x000245c0`; the design's
    /// item 2), so a host says yes only once that is done.
    fn accepts(&self, mem: &dyn GuestMemory, frame: &PostFrame) -> bool;
    /// Asks the device for an external pass drawing `frame`'s passes, at
    /// its next entry. `false`: nothing asked (the call runs).
    fn open(&self, frame: &PostFrame) -> bool;
}

static HOST: Mutex<Option<Arc<dyn Host>>> = Mutex::new(None);
static CALLS: AtomicU64 = AtomicU64::new(0);
static SKIPPED: AtomicU64 = AtomicU64::new(0);
static COLOUR: AtomicU64 = AtomicU64::new(0);
static LATE: AtomicU64 = AtomicU64::new(0);
static EFFECTS: AtomicU64 = AtomicU64::new(0);

/// Installs the host (boot). Without one nothing is skipped.
pub fn set_host(host: Arc<dyn Host>) {
    *HOST.lock().unwrap_or_else(|e| e.into_inner()) = Some(host);
}

/// What the calls asked for so far.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// Calls of `render_screen_effects`.
    pub calls: u64,
    /// Calls the host drew natively.
    pub skipped: u64,
    /// Calls asking for the colour pass (`0x000214f0`).
    pub colour: u64,
    /// Calls asking for `0x0002df00`.
    pub late: u64,
    /// Calls whose screen-effect loop could draw (its gates but
    /// `0x00022710` pass, with entries).
    pub screen_effects: u64,
}

pub fn counts() -> Counts {
    Counts {
        calls: CALLS.load(Ordering::Relaxed),
        skipped: SKIPPED.load(Ordering::Relaxed),
        colour: COLOUR.load(Ordering::Relaxed),
        late: LATE.load(Ordering::Relaxed),
        screen_effects: EFFECTS.load(Ordering::Relaxed),
    }
}

struct Ro<'a>(&'a dyn GuestMemory);

impl GuestRead for Ro<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }
}

/// The call's `before`, on any guest memory: `esp` at the entry (the
/// return address, then the `late` and `colour` bytes). `true` when the
/// host drew it and the call must not run.
pub fn on_screen_effects(mem: &mut dyn GuestMemory, esp: u32) -> bool {
    let n = CALLS.fetch_add(1, Ordering::Relaxed) + 1;
    let (Ok(late), Ok(colour)) = (mem.u8(esp.wrapping_add(4)), mem.u8(esp.wrapping_add(8))) else {
        return false;
    };
    let Some(frame) = PostFrame::read(&Ro(&*mem), late != 0, colour != 0) else {
        return false;
    };
    if frame.colour {
        COLOUR.fetch_add(1, Ordering::Relaxed);
    }
    if frame.late {
        LATE.fetch_add(1, Ordering::Relaxed);
    }
    if frame.screen_effects_gated && frame.screen_effects > 0 {
        EFFECTS.fetch_add(1, Ordering::Relaxed);
    }
    if n % 1800 == 1 {
        let c = counts();
        eprintln!(
            "native-post: {} calls: colour pass {}, late pass {}, screen effects possible {}; {} drawn natively",
            c.calls, c.colour, c.late, c.screen_effects, c.skipped
        );
    }
    // HOST is only held to clone the host: never across its calls.
    let Some(host) = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone() else {
        return false;
    };
    if !host.accepts(&*mem, &frame) || !host.open(&frame) {
        return false;
    }
    SKIPPED.fetch_add(1, Ordering::Relaxed);
    true
}

pub struct NativePost;

impl Feature for NativePost {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "The view's post passes drawn by the native GPU renderer").doc("docs/graphics/native-renderer.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        if !on(b.switches()) {
            return Ok(());
        }
        if b.switches().feature(crate::render_timing::ID, false) {
            eprintln!("native-post: not placed with render-timing on (its timer on the scene's callees needs their end)");
            return Ok(());
        }
        b.raw()
            .before(&points::RENDER_SCREEN_EFFECTS, "native_post_skip", |c| {
                let rt = Arc::clone(c.ctx.runtime());
                let skip = {
                    let mut mem = rt.memory();
                    on_screen_effects(&mut *mem, c.cpu.esp)
                };
                if skip {
                    c.ret(SCREEN_EFFECTS_CLEANUP).map(Flow::Return)
                } else {
                    Ok(Flow::Continue)
                }
            })
            .context_only();
        b.snapshot(crate::SnapshotPolicy::Stateless);
        Ok(())
    }
}
