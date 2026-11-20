//! `native-hud`: the HUD's and the UI's screen draws met by the native
//! renderer (docs/graphics/native-renderer.md, "Stage 5: post and HUD",
//! "The UI and HUD"). On by default (since 2026-11-20);
//! `H2_FEATURES=impl.render.native_hud=off` turns it off for the
//! comparison against the D3D path (exactly that key).
//!
//! Four hooks, all context-only with nothing after a body, so the
//! pipelined render runs them on its own thread:
//!
//! - `SCREEN_QUADS_BEGIN` (`0x00035b90`): a batch of screen quads begins
//!   with its record (text: `0x0001f070` over the glyph cache). The
//!   [`Host`] plans the batch (`native_render::hud::Batch`) or says why it
//!   stays on D3D. The begin always runs: it sets the state the game
//!   expects after it.
//! - `SCREEN_QUAD` (`0x000363a0`): one quad of the batch. When the host
//!   answers it ([`Host::screen_quad`] `true`), the hook returns `ret 4`
//!   in its place and the quad's words are never pushed. The host answers
//!   a quad of a planned text batch once a native pass holding it is
//!   reserved on the device (docs/graphics/native-renderer.md, "Native
//!   text"); every other quad keeps its D3D draw.
//! - `SCREEN_QUADS_END` (`0x00036560`): the batch ends.
//! - `HUD_SHADER_QUAD` (`0x0022a664`): one HUD shader quad (ECX the
//!   corners, EAX the coordinate rectangle, stack the bitmap datum, its
//!   index and the shader). The host notes the corners; the quad's own
//!   draw is claimed, or not, where D3D resolves it (a draw admission).
//!
//! `H2_NATIVE_HUD_COMPARE` ([`compare_spec`]): the host queues the first
//! quads' corners and pairs each with the draw the D3D path resolves for
//! it, through a draw admission on the device that claims nothing.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use engine::memory::GuestMemory;
use engine::points;

use crate::{BuildError, Builder, Feature, Flow, Manifest};

pub const ID: &str = "native-hud";
/// The switch, looked up exactly.
pub const SWITCH: &str = "impl.render.native_hud";

/// Whether the native HUD is on (on by default since 2026-11-20;
/// `impl.render.native_hud=off` turns it off for the comparison against
/// the D3D path).
pub fn on(switches: &crate::Switches) -> bool {
    crate::switches::native_stage_on(switches, SWITCH)
}

/// One call of `0x0022a664`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HudQuadCall {
    /// ECX: the four corners `(x, y)`.
    pub corners: u32,
    /// EAX: the coordinate rectangle.
    pub rect: u32,
    /// The three stack words: bitmap datum, bitmap index, shader.
    pub args: [u32; 3],
}

/// What the screen draws become: boot's glue over the GPU and the device.
pub trait Host: Send + Sync {
    /// A batch begins with the record at `record`.
    fn begin(&self, mem: &mut dyn GuestMemory, record: u32);
    /// One quad of the open batch, its four corners at `corners`. `true`:
    /// the host draws it, and the quad must not run.
    fn screen_quad(&self, mem: &mut dyn GuestMemory, corners: u32) -> bool;
    /// The batch ends.
    fn end(&self, mem: &mut dyn GuestMemory);
    /// A HUD shader quad is about to be drawn by the D3D path.
    fn hud_quad(&self, mem: &mut dyn GuestMemory, call: HudQuadCall);
}

/// `H2_NATIVE_HUD_COMPARE` (docs/graphics/native-renderer.md, stage 5):
/// `N` pairs the first `N` quads (screen or HUD) with the draws the
/// device resolves for them; empty, [`COMPARE_WINDOWS`].
pub fn compare_spec() -> Option<u64> {
    static SPEC: OnceLock<Option<u64>> = OnceLock::new();
    *SPEC.get_or_init(|| {
        let v = std::env::var("H2_NATIVE_HUD_COMPARE").ok()?;
        let n = parse_compare(&v);
        if n.is_none() {
            eprintln!(
                "native-hud: H2_NATIVE_HUD_COMPARE={v:?} is not a count; nothing is compared"
            );
        }
        n
    })
}

/// Quads compared when `H2_NATIVE_HUD_COMPARE` names no count.
pub const COMPARE_WINDOWS: u64 = 512;

/// `H2_NATIVE_HUD_COMPARE`'s value: empty is [`COMPARE_WINDOWS`], else a
/// count above 0.
pub fn parse_compare(s: &str) -> Option<u64> {
    match s.trim() {
        "" => Some(COMPARE_WINDOWS),
        n => n.parse().ok().filter(|&n| n > 0),
    }
}

static HOST: Mutex<Option<Arc<dyn Host>>> = Mutex::new(None);
static BATCHES: AtomicU64 = AtomicU64::new(0);
static QUADS: AtomicU64 = AtomicU64::new(0);
static ANSWERED: AtomicU64 = AtomicU64::new(0);
static HUD_QUADS: AtomicU64 = AtomicU64::new(0);

/// Installs the host (boot). Without one nothing is planned or answered.
pub fn set_host(host: Arc<dyn Host>) {
    *HOST.lock().unwrap_or_else(|e| e.into_inner()) = Some(host);
}

fn host() -> Option<Arc<dyn Host>> {
    // HOST is only held to clone the host: never across its calls.
    HOST.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The calls so far.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub batches: u64,
    pub quads: u64,
    /// Quads the host drew in place of the D3D path.
    pub answered: u64,
    pub hud_quads: u64,
}

pub fn counts() -> Counts {
    Counts {
        batches: BATCHES.load(Ordering::Relaxed),
        quads: QUADS.load(Ordering::Relaxed),
        answered: ANSWERED.load(Ordering::Relaxed),
        hud_quads: HUD_QUADS.load(Ordering::Relaxed),
    }
}

/// The begin's body: `esp` at the entry (the return address, then the
/// record's address).
pub fn on_begin(mem: &mut dyn GuestMemory, esp: u32) {
    BATCHES.fetch_add(1, Ordering::Relaxed);
    let Ok(record) = mem.u32(esp.wrapping_add(4)) else {
        return;
    };
    if let Some(host) = host() {
        host.begin(mem, record);
    }
}

/// The quad's body: `esp` at the entry (the return address, then the
/// corners' address). `true`: the host drew it, answer `ret 4`.
pub fn on_screen_quad(mem: &mut dyn GuestMemory, esp: u32) -> bool {
    QUADS.fetch_add(1, Ordering::Relaxed);
    let Ok(corners) = mem.u32(esp.wrapping_add(4)) else {
        return false;
    };
    let Some(host) = host() else { return false };
    let answered = host.screen_quad(mem, corners);
    if answered {
        ANSWERED.fetch_add(1, Ordering::Relaxed);
    }
    answered
}

/// The end's body.
pub fn on_end(mem: &mut dyn GuestMemory) {
    if let Some(host) = host() {
        host.end(mem);
    }
}

/// The HUD quad's body: ECX, EAX, and `esp` at the entry (the return
/// address, then the three words).
pub fn on_hud_quad(mem: &mut dyn GuestMemory, ecx: u32, eax: u32, esp: u32) {
    HUD_QUADS.fetch_add(1, Ordering::Relaxed);
    let mut args = [0u32; 3];
    for (i, a) in args.iter_mut().enumerate() {
        let Ok(v) = mem.u32(esp.wrapping_add(4 + 4 * i as u32)) else {
            return;
        };
        *a = v;
    }
    if let Some(host) = host() {
        host.hud_quad(
            mem,
            HudQuadCall {
                corners: ecx,
                rect: eax,
                args,
            },
        );
    }
}

pub fn compare_on(switches: &crate::Switches) -> Option<u64> {
    if on(switches) || crate::switches::native_render_default(switches) {
        compare_spec()
    } else {
        None
    }
}

pub struct NativeHud;

impl Feature for NativeHud {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            ID,
            "The HUD's and the UI's screen draws met by the native GPU renderer",
        )
        .doc("docs/graphics/native-renderer.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        if !on(b.switches()) && compare_on(b.switches()).is_none() {
            return Ok(());
        }
        b.raw()
            .before(&points::SCREEN_QUADS_BEGIN, "native_hud_begin", |c| {
                let rt = Arc::clone(c.ctx.runtime());
                let mut mem = rt.memory();
                on_begin(&mut *mem, c.cpu.esp);
                Ok(Flow::Continue)
            })
            .context_only();
        b.raw()
            .before(&points::SCREEN_QUAD, "native_hud_quad", |c| {
                let rt = Arc::clone(c.ctx.runtime());
                let answered = {
                    let mut mem = rt.memory();
                    on_screen_quad(&mut *mem, c.cpu.esp)
                };
                if answered {
                    c.ret(4).map(Flow::Return)
                } else {
                    Ok(Flow::Continue)
                }
            })
            .context_only();
        b.raw()
            .before(&points::SCREEN_QUADS_END, "native_hud_end", |c| {
                let rt = Arc::clone(c.ctx.runtime());
                let mut mem = rt.memory();
                on_end(&mut *mem);
                Ok(Flow::Continue)
            })
            .context_only();
        b.raw()
            .before(&points::HUD_SHADER_QUAD, "native_hud_shader_quad", |c| {
                let rt = Arc::clone(c.ctx.runtime());
                let mut mem = rt.memory();
                on_hud_quad(&mut *mem, c.cpu.ecx, c.cpu.eax, c.cpu.esp);
                Ok(Flow::Continue)
            })
            .context_only();
        b.snapshot(crate::SnapshotPolicy::Stateless);
        Ok(())
    }
}
