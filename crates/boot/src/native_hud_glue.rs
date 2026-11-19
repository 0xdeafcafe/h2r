//! The native HUD's host (docs/graphics/native-renderer.md, "Stage 5:
//! post and HUD", "The UI and HUD"): it plans each batch of screen quads
//! (`native_render::hud::Batch`: text over the glyph cache so far) and
//! says once per reason why a batch stays on D3D. It answers no quad yet:
//! every screen quad and HUD shader quad keeps its D3D draw.
//!
//! `H2_NATIVE_HUD_COMPARE=N` (`feature::native_hud::compare_spec`) pairs
//! the plan with the D3D path for the first `N` windows (one batch, or
//! one HUD quad): the device traces from the window's first hook, and two
//! hook calls after the window closes (so its pushed quads have drained)
//! the trace is taken and paired (`native_render::hud::pair`): each
//! screen quad with the one four-vertex fan whose `End` lies between its
//! push-buffer put and the next quad's (the batch's end after the last),
//! each HUD quad with the next draw through `D3DDevice_End` returning
//! into `0x0022a664`. One line per window:
//! `native-hud: compare #<n> <what>: <paired> paired, <unseen> not drained, <mismatches>`.
//! A trace someone else runs (`H2_POST_TRACE`) is left alone: that window
//! is not compared.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use feature::native_hud::{Host, HudQuadCall};
use native_render::camera::GuestRead;
use native_render::hud::{self, Batch, HudQuad, Pairing, Planned, Refused, Seen};

struct Ro<'a>(&'a dyn GuestMemory);

impl GuestRead for Ro<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }
}

/// Hook calls after a window closes before its trace is taken.
pub const COMPARE_TAIL: u32 = 2;

/// One compared window.
#[derive(Debug)]
struct Window {
    what: String,
    planned: Vec<Planned>,
    /// The push-buffer put at the batch's end (a batch's window).
    end: Option<u32>,
    sealed: bool,
    waited: u32,
}

#[derive(Debug, Default)]
struct Compare {
    opened: u64,
    window: Option<Window>,
}

/// Refusals by reason ([`Refused::ALL`]), each logged once.
static REFUSALS: [AtomicU64; Refused::ALL.len()] =
    [const { AtomicU64::new(0) }; Refused::ALL.len()];
static LOGGED: AtomicU64 = AtomicU64::new(0);
static PLANNED: AtomicU64 = AtomicU64::new(0);

/// Batches refused, by [`Refused::ALL`].
pub fn refusals() -> [u64; Refused::ALL.len()] {
    std::array::from_fn(|i| REFUSALS[i].load(Ordering::Relaxed))
}

/// Batches planned.
pub fn planned() -> u64 {
    PLANNED.load(Ordering::Relaxed)
}

fn refused(r: Refused) {
    let i = r.index();
    REFUSALS[i].fetch_add(1, Ordering::Relaxed);
    if LOGGED.fetch_or(1 << i, Ordering::Relaxed) & (1 << i) == 0 {
        eprintln!(
            "native-hud: a batch of screen quads keeps its D3D draws: {} (logged once)",
            r.describe()
        );
    }
}

/// The line a compared window prints.
pub fn compare_line(n: u64, what: &str, p: &Pairing) -> String {
    let mut line = format!(
        "native-hud: compare #{n} {what}: {} screen quads paired, {} answered and absent, {} HUD quads paired, {} not drained",
        p.screen_paired, p.screen_native, p.hud_paired, p.unseen
    );
    if p.mismatches.is_empty() {
        line.push_str(", no mismatch");
    } else {
        line.push_str(&format!(", mismatches: {}", p.mismatches.join("; ")));
    }
    line
}

/// The trace's draws as the pairing takes them: a draw through an entry
/// takes the return address of the entry call before it.
pub fn seen(events: &[d3d::Traced]) -> Vec<Seen> {
    let mut ret = None;
    let mut out = Vec::new();
    for e in events {
        match e {
            d3d::Traced::Call { ret: r, .. } => ret = Some(*r),
            d3d::Traced::Draw(d) => out.push(Seen {
                push: d.push,
                ret: if d.push.is_none() { ret } else { None },
                primitive: format!("{:?}", d.primitive),
                vertices: d.vertices,
                inline: d.inline,
                target: d.target,
            }),
            _ => {}
        }
    }
    out
}

/// Pairs a window: its screen quads by their puts, its HUD quads in order.
pub fn pair_window(planned: &[Planned], end: Option<u32>, seen: &[Seen]) -> Pairing {
    let screens: Vec<Planned> = planned
        .iter()
        .copied()
        .filter(|p| matches!(p, Planned::Screen { .. }))
        .collect();
    let huds: Vec<Planned> = planned
        .iter()
        .copied()
        .filter(|p| matches!(p, Planned::Hud))
        .collect();
    let mut p = hud::pair(&screens, seen, end);
    let h = hud::pair(&huds, seen, None);
    p.hud_paired = h.hud_paired;
    p.unseen += h.unseen;
    p.mismatches.extend(h.mismatches);
    p.mismatches.truncate(hud::MISMATCH_LINES);
    p
}

struct Glue<B: d3d::backend::Backend + Send + 'static> {
    device: d3d::abi::SharedDevice<B>,
    /// The open batch's plan (`None` between batches).
    batch: Mutex<Option<Result<Batch, Refused>>>,
    compare: Mutex<Compare>,
    limit: Option<u64>,
}

impl<B: d3d::backend::Backend + Send + 'static> Glue<B> {
    /// A hook call: the closed window's wait goes on, and its trace is
    /// taken and paired once it has waited [`COMPARE_TAIL`] calls; then a
    /// new window opens while windows are left.
    fn tick(&self) {
        let Some(limit) = self.limit else { return };
        let done = {
            let mut c = self.compare.lock().unwrap_or_else(|e| e.into_inner());
            let ripe = c.window.as_mut().is_some_and(|w| {
                w.sealed = true;
                w.waited += 1;
                w.waited >= COMPARE_TAIL
            });
            if ripe {
                c.window.take().map(|w| (c.opened, w))
            } else {
                None
            }
        };
        if let Some((n, w)) = done {
            self.finish(n, w);
        }
        let mut c = self.compare.lock().unwrap_or_else(|e| e.into_inner());
        if c.opened >= limit || c.window.is_some() {
            return;
        }
        let Ok(mut d) = self.device.lock() else {
            return;
        };
        if d.tracing() {
            static BUSY: std::sync::Once = std::sync::Once::new();
            BUSY.call_once(|| eprintln!("native-hud: the device is tracing for another diagnostic; a window is not compared"));
            return;
        }
        d.trace(true);
        c.opened += 1;
        c.window = Some(Window {
            what: String::new(),
            planned: Vec::new(),
            end: None,
            sealed: false,
            waited: 0,
        });
    }

    fn finish(&self, n: u64, w: Window) {
        let events = {
            let Ok(mut d) = self.device.lock() else {
                return;
            };
            let (events, past) = d.take_trace();
            d.trace(false);
            if past > 0 {
                eprintln!(
                    "native-hud: compare #{n}: {past} trace events past the cap were not kept"
                );
            }
            events
        };
        let p = pair_window(&w.planned, w.end, &seen(&events));
        eprintln!("{}", compare_line(n, &w.what, &p));
    }

    /// The open window, unsealed (one that is taking this call's plan).
    fn with_window(&self, f: impl FnOnce(&mut Window)) {
        if self.limit.is_none() {
            return;
        }
        let mut c = self.compare.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(w) = c.window.as_mut()
            && !w.sealed
        {
            f(w);
        }
    }
}

impl<B: d3d::backend::Backend + Send + 'static> Host for Glue<B> {
    fn begin(&self, mem: &mut dyn GuestMemory, record: u32) {
        self.tick();
        let m = Ro(mem);
        let plan = Batch::read(&m, record);
        match &plan {
            Ok(_) => {
                PLANNED.fetch_add(1, Ordering::Relaxed);
            }
            Err(r) => refused(*r),
        }
        let what = match &plan {
            _ if self.limit.is_none() => String::new(),
            Ok(b) => format!(
                "batch at {record:#010x} over {:#010x} ({:?})",
                b.bitmap.at, b.blend
            ),
            Err(r) => format!("batch at {record:#010x} (D3D: {})", r.describe()),
        };
        self.with_window(|w| w.what = what);
        *self.batch.lock().unwrap_or_else(|e| e.into_inner()) = Some(plan);
    }

    fn screen_quad(&self, mem: &mut dyn GuestMemory, _corners: u32) -> bool {
        if self.limit.is_none() {
            return false;
        }
        let m = Ro(mem);
        // `0x000363a0` draws nothing while this word is set.
        if m.read_u32(hud::QUADS_OFF & !3)
            .is_none_or(|w| (w >> 16) as u16 != 0)
        {
            return false;
        }
        if let Some(put) = hud::push_put(&m) {
            self.with_window(|w| w.planned.push(Planned::Screen { put, native: false }));
        }
        false
    }

    fn end(&self, mem: &mut dyn GuestMemory) {
        let end = hud::push_put(&Ro(mem));
        self.with_window(|w| {
            w.end = end;
            w.sealed = true;
        });
        self.batch.lock().unwrap_or_else(|e| e.into_inner()).take();
    }

    fn hud_quad(&self, mem: &mut dyn GuestMemory, call: HudQuadCall) {
        self.tick();
        let q = HudQuad::read(&Ro(mem), call.corners, call.rect, call.args);
        self.with_window(|w| {
            w.what = match q {
                Some(q) => format!(
                    "HUD quad of bitmap {:#010x}[{}], shader {:#010x}, corners {:?}",
                    q.bitmap, q.index, q.shader, q.corners
                ),
                None => "HUD quad (unreadable)".into(),
            };
            w.planned.push(Planned::Hud);
            w.sealed = true;
        });
    }
}

/// Installs the host over `device`.
pub(crate) fn install<B: d3d::backend::Backend + Send + 'static>(
    device: &d3d::abi::SharedDevice<B>,
) {
    feature::native_hud::set_host(Arc::new(Glue {
        device: Arc::clone(device),
        batch: Mutex::new(None),
        compare: Mutex::new(Compare::default()),
        limit: feature::native_hud::compare_spec(),
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use gfx::vertex::PrimitiveType;

    fn draw(push: Option<u32>, vertices: usize) -> d3d::Traced {
        d3d::Traced::Draw(d3d::TracedDraw {
            entry: if push.is_some() {
                "inline Begin/End"
            } else {
                "D3DDevice_End"
            },
            primitive: PrimitiveType::TriangleFan,
            vertices,
            inline: 0,
            push,
            target: 0x10,
            draws: 0,
            fate: None,
        })
    }

    #[test]
    fn an_entry_draw_takes_the_return_of_the_call_before_it() {
        let events = [
            d3d::Traced::Call {
                entry: "D3DDevice_End",
                ret: hud::HUD_QUAD_END_RET,
            },
            draw(None, 4),
            draw(Some(0x200), 4),
        ];
        let s = seen(&events);
        assert_eq!(s[0].ret, Some(hud::HUD_QUAD_END_RET));
        assert_eq!(s[0].primitive, "TriangleFan");
        assert_eq!(s[1].ret, None, "a pushed draw has no entry");
    }

    #[test]
    fn a_window_pairs_its_screen_quads_and_its_hud_quads_apart() {
        let events = [
            draw(Some(0x110), 4),
            d3d::Traced::Call {
                entry: "D3DDevice_End",
                ret: hud::HUD_QUAD_END_RET,
            },
            draw(None, 4),
            draw(Some(0x190), 4),
        ];
        let planned = [
            Planned::Screen {
                put: 0x100,
                native: false,
            },
            Planned::Hud,
            Planned::Screen {
                put: 0x180,
                native: false,
            },
        ];
        let p = pair_window(&planned, Some(0x200), &seen(&events));
        assert_eq!(
            (p.screen_paired, p.hud_paired, p.unseen),
            (2, 1, 0),
            "{p:?}"
        );
        let line = compare_line(3, "a batch", &p);
        assert!(
            line.starts_with("native-hud: compare #3 a batch: 2 screen quads paired"),
            "{line}"
        );
        assert!(line.ends_with("no mismatch"), "{line}");
    }

    #[test]
    fn every_refusal_has_its_own_slot() {
        let mut slots = std::collections::HashSet::new();
        for r in Refused::ALL {
            assert!(slots.insert(r.index()), "{r:?}");
        }
        assert!(Refused::ALL.len() <= 64, "the logged-once bits fit a word");
    }
}
