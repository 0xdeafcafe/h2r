//! Native UI and HUD hooks, plus the optional screen-quad comparison.
//! Producers may skip emission after a complete native reservation. The UI
//! walk keeps the game's order, state setup and layout callbacks.

mod backdrop;
mod model;
mod outline;
mod ui;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use engine::memory::GuestMemory;
use engine::points;
use runtime::abi::TransferAbi;
use timeline::native::{Metric, Stage};

use crate::{BuildError, Builder, Feature, Flow, Manifest};

pub const ID: &str = "native-hud";
/// The switch, looked up exactly.
pub const SWITCH: &str = "impl.render.native_hud";
pub const UI_SWITCH: &str = "impl.render.native_ui";

pub(crate) use ui::instruction_entries;
pub use outline::{PUSH_BYTES as OUTLINE_PUSH_BYTES, Vertices as OutlineVertices};
pub use ui::UiScope;

/// Both names select the same UI renderer. The old HUD key remains an
/// exact opt-out and cannot override a disabled native renderer.
pub fn on(switches: &crate::Switches) -> bool {
    crate::switches::native_render_default(switches)
        && switches.feature(UI_SWITCH, true)
        && switches.feature(SWITCH, true)
}

pub const WALK_SWITCH: &str = "impl.render.native_hud_walk";

pub fn walk_on(switches: &crate::Switches) -> bool {
    on(switches) && switches.feature(WALK_SWITCH, true)
}

pub(crate) const HUD_QUAD_READY: points::Point = points::Point::new(
    "HUD_QUAD_READY", 0x0022_a7f6, points::Sig::Site, "docs/graphics/native-renderer.md",
);
const HUD_QUAD_TAIL: u32 = 0x0022_a854;
const HUD_QUAD_LOCAL_BYTES: u32 = 0x70;
const HUD_QUAD_VERTEX_BYTES: u32 = 0x1c;

/// The four vertices after the guest resolves the engine bitmap and
/// multiplies the texture coordinates by its live dimensions.
pub type HudVertices = [[(u8, [f32; 4]); 3]; 4];

fn vertices(mem: &dyn GuestMemory, frame: u32, colour: u32) -> Option<HudVertices> {
    // The helper writes white to register 9 before the vertex loop.
    if colour != u32::MAX {
        return None;
    }
    let base = frame.checked_sub(HUD_QUAD_LOCAL_BYTES)?;
    let mut vertices = [[(0, [0.0; 4]); 3]; 4];
    for (i, vertex) in vertices.iter_mut().enumerate() {
        let at = base.checked_add(i as u32 * HUD_QUAD_VERTEX_BYTES)?;
        let read = |o| {
            let v = f32::from_bits(mem.u32(at.checked_add(o)?).ok()?);
            (v == 0.0 || v.is_normal()).then_some(v)
        };
        *vertex = [
            (9, [1.0; 4]),
            (3, [read(16)?, read(20)?, 0.0, 1.0]),
            (0, [read(0)?, read(4)?, read(8)?, read(12)?]),
        ];
    }
    Some(vertices)
}

fn walk_quad(mem: &mut dyn GuestMemory, frame: u32, colour: u32) -> bool {
    Stage::Ui.count(Metric::WalkExamined, 1);
    let Some(vertices) = vertices(mem, frame, colour) else {
        Stage::Ui.count(Metric::WalkRefusedInputs, 1);
        return false;
    };
    let drawn = host().is_some_and(|host| host.draw_quad(mem, &vertices));
    ui::record(drawn);
    drawn
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
    fn draw_quad(&self, _mem: &mut dyn GuestMemory, _vertices: &HudVertices) -> bool {
        false
    }

    fn screen_fan(&self, _mem: &mut dyn GuestMemory, _vertices: &HudVertices) -> bool {
        false
    }

    fn fade_fan(&self, _mem: &mut dyn GuestMemory, _vertices: &[[(u8, [f32; 4]); 1]; 4]) -> bool {
        false
    }

    fn loading_bitmap_fan(&self, _mem: &mut dyn GuestMemory, _vertices: &[[(u8, [f32; 4]); 2]; 4]) -> bool {
        false
    }

    fn backdrop_wash(&self, _mem: &mut dyn GuestMemory, _vertices: &HudVertices) -> bool {
        false
    }

    fn backdrop_composite(
        &self,
        _mem: &mut dyn GuestMemory,
        _vertices: &engine::ui_backdrop::Fan<2>,
    ) -> bool {
        false
    }

    fn model_ranges(&self, _mem: &mut dyn GuestMemory, _ranges: &[engine::ui_model::Range]) -> bool {
        false
    }

    fn outline(&self, _mem: &mut dyn GuestMemory, _vertices: &OutlineVertices) -> bool {
        false
    }

    fn inline_ui_quad(&self, _mem: &mut dyn GuestMemory, _primitive: u32, _bytes: Vec<u8>) -> bool {
        false
    }

    fn enter_ui(&self, _mem: &mut dyn GuestMemory, _scope: UiScope, _stack: u32, _eligible: bool) {}

    fn leave_ui(&self, _mem: &mut dyn GuestMemory, _scope: UiScope, _stack: u32) {}

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
    if crate::switches::native_render_default(switches) {
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
        let compare = compare_on(b.switches()).is_some();
        if !on(b.switches()) && !compare {
            return Ok(());
        }
        if compare {
            b.raw()
                .before(&points::SCREEN_QUADS_BEGIN, "native_hud_begin", |c| {
                    let rt = Arc::clone(c.ctx.runtime());
                    let mut mem = rt.memory();
                    on_begin(&mut *mem, c.cpu.esp);
                    Ok(Flow::Continue)
                })
                .context_only()
                .optional();
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
                .context_only()
                .optional();
            b.raw()
                .before(&points::SCREEN_QUADS_END, "native_hud_end", |c| {
                    let rt = Arc::clone(c.ctx.runtime());
                    let mut mem = rt.memory();
                    on_end(&mut *mem);
                    Ok(Flow::Continue)
                })
                .context_only()
                .optional();
            b.raw()
                .before(&points::HUD_SHADER_QUAD, "native_hud_shader_quad", |c| {
                    let rt = Arc::clone(c.ctx.runtime());
                    let mut mem = rt.memory();
                    on_hud_quad(&mut *mem, c.cpu.ecx, c.cpu.eax, c.cpu.esp);
                    Ok(Flow::Continue)
                })
                .context_only()
                .optional();
        } else {
            ui::place(b);
        }
        if walk_on(b.switches()) && !compare {
            b.raw().before(&HUD_QUAD_READY, "native_hud_walk_quad", |c| {
                let rt = Arc::clone(c.ctx.runtime());
                if !rt.read_dispatch(|dispatch| dispatch.has_resume(HUD_QUAD_TAIL)) {
                    return Ok(Flow::Continue);
                }
                let Some(esp) = c.cpu.esp.checked_add(4) else { return Ok(Flow::Continue) };
                let mut mem = rt.memory();
                if mem.u32(c.cpu.esp).ok() != Some(7)
                    || !walk_quad(&mut *mem, c.cpu.ebp, c.cpu.ebx)
                {
                    return Ok(Flow::Continue);
                }
                // The fan primitive word is already on the stack.
                c.cpu.esp = esp;
                Ok(Flow::Return(TransferAbi::resume(HUD_QUAD_TAIL)))
            }).context_only().optional();
        }
        b.snapshot(crate::SnapshotPolicy::Stateless);
        Ok(())
    }
}

#[cfg(test)]
mod walk_tests {
    use super::*;
    use engine::memory::FlatMemory;

    #[test]
    fn the_hud_walk_uses_the_live_corner_order_and_separate_uv_register() {
        let mut mem = FlatMemory::new(0x1000, 0x1000);
        let frame = 0x1800;
        for i in 0..4 {
            let at = frame - HUD_QUAD_LOCAL_BYTES + i * HUD_QUAD_VERTEX_BYTES;
            for (j, word) in [i as f32, -0.0, 0.5, 2.0, 10.0 + i as f32, 20.0].into_iter().enumerate() {
                mem.set_u32(at + 4 * j as u32, word.to_bits()).unwrap();
            }
        }
        let quad = vertices(&mem, frame, u32::MAX).unwrap();
        assert_eq!(quad[2][0], (9, [1.0; 4]));
        assert_eq!(quad[2][1], (3, [12.0, 20.0, 0.0, 1.0]));
        assert_eq!(quad[2][2].0, 0);
        assert_eq!(quad[2][2].1.map(f32::to_bits), [2.0, -0.0, 0.5, 2.0].map(f32::to_bits));
        assert!(vertices(&mem, frame, 0).is_none());
        mem.set_u32(frame - HUD_QUAD_LOCAL_BYTES, f32::NAN.to_bits()).unwrap();
        assert!(vertices(&mem, frame, u32::MAX).is_none());
    }
}
