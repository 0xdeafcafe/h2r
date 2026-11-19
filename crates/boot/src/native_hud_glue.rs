//! The native HUD's host (docs/graphics/native-renderer.md, "Stage 5:
//! post and HUD", "The UI and HUD"): it plans each batch of screen quads
//! (`native_render::hud::Batch`: text over the glyph cache so far) and
//! says once per reason why a batch stays on D3D. It answers no quad yet:
//! every screen quad and HUD shader quad keeps its D3D draw.
//!
//! `H2_NATIVE_HUD_COMPARE=N` (`feature::native_hud::compare_spec`) pairs
//! the first `N` hooked quads with the draws the D3D path resolves for
//! them. The hooks queue each quad's corners as the call pushes them
//! (`hud::Expect`); a draw admission on the device (`d3d::DrawAdmission`,
//! the handoff's seam: it sees every resolved draw in issue order, with
//! the guest's state applied) reads each four-vertex fan's positions and
//! pairs it with the first waiting quad of the same corners
//! (`hud::Pairing`). It claims no draw. For the first paired quad of each
//! kind it prints what D3D resolved (entry, target, stage 0's texture,
//! blend) beside the batch's plan; every 64 pairings, and when the last
//! queued quad is paired, one line of totals:
//! `native-hud: compare: <s> screen quads and <h> HUD quads paired with
//! their D3D draws; <ls> screen and <lh> HUD lost; <o> other fans; <w> waiting`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use feature::native_hud::{Host, HudQuadCall};
use gfx::vertex::{PrimitiveType, VertexDataType, VertexLayout, VertexRegister};
use native_render::camera::GuestRead;
use native_render::hud::{self, Batch, Expect, HudQuad, Kind, Pairing, Refused};

struct Ro<'a>(&'a dyn GuestMemory);

impl GuestRead for Ro<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }
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

/// The positions of a four-vertex fan as the D3D path resolved it
/// (`ResolvedDraw`'s primitive, count, index presence, layout and
/// streams): attribute 0 of each vertex, its x and y. `None` for any
/// other draw, or a position that is not floats.
pub fn fan_corners(
    primitive: PrimitiveType,
    vertex_count: u32,
    indexed: bool,
    layout: &VertexLayout,
    streams: &[d3d::StreamBytes],
) -> Option<[[f32; 2]; 4]> {
    if primitive != PrimitiveType::TriangleFan || vertex_count != 4 || indexed {
        return None;
    }
    let (n, a) = layout.streams.iter().enumerate().find_map(|(n, s)| {
        s.attributes
            .iter()
            .find(|a| a.register == VertexRegister::POSITION)
            .map(|a| (n, a))
    })?;
    if !matches!(
        a.data_type,
        VertexDataType::Float2 | VertexDataType::Float3 | VertexDataType::Float4
    ) {
        return None;
    }
    let stream = streams.get(n)?;
    let bytes: &[u8] = &stream.data;
    let f = |at: usize| -> Option<f32> {
        let b = bytes.get(at..at + 4)?;
        Some(f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let mut out = [[0.0f32; 2]; 4];
    for (i, c) in out.iter_mut().enumerate() {
        let at = i * stream.stride as usize + a.offset as usize;
        *c = [f(at)?, f(at + 4)?];
    }
    Some(out)
}

/// What the compare shares between the hooks and the admission.
struct Compare {
    pairing: Mutex<Pairing>,
    /// Quads still to queue.
    left: AtomicU64,
    /// The plan of the last batch begun, said beside the first resolved
    /// screen quad.
    plan: Mutex<String>,
    said: Mutex<[bool; 2]>,
}

impl Compare {
    fn expect(&self, e: Expect) {
        if self
            .left
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1))
            .is_ok()
        {
            self.pairing
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .expect(e);
        }
    }
}

impl d3d::DrawAdmission for Compare {
    fn admit(
        &self,
        snapshot: &d3d::DrawSnapshot<'_>,
    ) -> Option<(d3d::ExternalPlan, Box<dyn d3d::ExternalPass>)> {
        let d = snapshot.draw;
        let corners = fan_corners(
            d.primitive,
            d.vertex_count,
            d.indices.is_some(),
            &d.layout,
            &d.streams,
        )?;
        let mut p = self.pairing.lock().unwrap_or_else(|e| e.into_inner());
        if p.waiting() == 0 {
            return None;
        }
        let paired = p.draw(&corners)?;
        let total = p.paired[0] + p.paired[1];
        let (line, done) = (p.line(), p.waiting() == 0);
        std::mem::drop(p);
        let k = match paired.kind {
            Kind::Screen => 0,
            Kind::Hud => 1,
        };
        let first = {
            let mut said = self.said.lock().unwrap_or_else(|e| e.into_inner());
            !std::mem::replace(&mut said[k], true)
        };
        if first {
            let r = &d.state.render;
            eprintln!(
                "native-hud: compare: the first {:?} quad resolved as {} into {:#010x}, stage 0 {:?}, blend {} {:?} {:?} {:?}; plan: {}",
                paired.kind,
                d.entry,
                snapshot.color_header,
                d.texture_info[0],
                r.blend_enable,
                r.src_blend,
                r.dst_blend,
                r.blend_op,
                self.plan.lock().unwrap_or_else(|e| e.into_inner())
            );
        }
        if total % 64 == 0 || (done && self.left.load(Ordering::Relaxed) == 0) {
            eprintln!("native-hud: compare: {line}");
        }
        None
    }
}

struct Glue {
    compare: Option<Arc<Compare>>,
}

impl Host for Glue {
    fn begin(&self, mem: &mut dyn GuestMemory, record: u32) {
        let plan = Batch::read(&Ro(mem), record);
        match &plan {
            Ok(_) => {
                PLANNED.fetch_add(1, Ordering::Relaxed);
            }
            Err(r) => refused(*r),
        }
        if let Some(c) = &self.compare
            && c.left.load(Ordering::Relaxed) > 0
        {
            *c.plan.lock().unwrap_or_else(|e| e.into_inner()) = match &plan {
                Ok(b) => format!(
                    "batch at {record:#010x} over {:#010x} ({}x{}, linear {}), {:?}",
                    b.bitmap.at, b.bitmap.width, b.bitmap.height, b.bitmap.linear, b.blend
                ),
                Err(r) => format!("batch at {record:#010x} on D3D: {}", r.describe()),
            };
        }
    }

    fn screen_quad(&self, mem: &mut dyn GuestMemory, corners: u32) -> bool {
        let Some(c) = &self.compare else { return false };
        let m = Ro(mem);
        // `0x000363a0` draws nothing while this word is set.
        if m.read_u32(hud::QUADS_OFF & !3)
            .is_none_or(|w| (w >> 16) as u16 != 0)
        {
            return false;
        }
        if let Some(q) = hud::read_quad(&m, corners) {
            c.expect(Expect {
                kind: Kind::Screen,
                corners: q.map(|v| [v.x, v.y]),
            });
        }
        false
    }

    fn end(&self, _mem: &mut dyn GuestMemory) {}

    fn hud_quad(&self, mem: &mut dyn GuestMemory, call: HudQuadCall) {
        let Some(c) = &self.compare else { return };
        if let Some(q) = HudQuad::read(&Ro(mem), call.corners, call.rect, call.args) {
            c.expect(Expect {
                kind: Kind::Hud,
                corners: q.corners,
            });
        }
    }
}

/// Installs the host over `device`, and with `H2_NATIVE_HUD_COMPARE` the
/// compare's admission on it.
pub(crate) fn install<B: d3d::backend::Backend + Send + 'static>(
    device: &d3d::abi::SharedDevice<B>,
) {
    let compare = feature::native_hud::compare_spec().map(|n| {
        Arc::new(Compare {
            pairing: Mutex::new(Pairing::default()),
            left: AtomicU64::new(n),
            plan: Mutex::new(String::new()),
            said: Mutex::new([false; 2]),
        })
    });
    if let Some(c) = &compare
        && let Ok(mut d) = device.lock()
    {
        d.add_draw_admission(Arc::clone(c) as Arc<dyn d3d::DrawAdmission>);
    }
    feature::native_hud::set_host(Arc::new(Glue { compare }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use gfx::vertex::{StreamLayout, VertexAttribute};

    fn fan(
        stride: u32,
        offset: u32,
        data_type: VertexDataType,
    ) -> (VertexLayout, Vec<d3d::StreamBytes>) {
        let mut bytes = vec![0u8; 4 * stride as usize];
        for i in 0..4usize {
            let at = i * stride as usize + offset as usize;
            bytes[at..at + 4].copy_from_slice(&(i as f32).to_le_bytes());
            bytes[at + 4..at + 8].copy_from_slice(&(10.0 + i as f32).to_le_bytes());
        }
        let layout = VertexLayout {
            streams: vec![StreamLayout {
                stream: 0,
                attributes: vec![VertexAttribute {
                    register: VertexRegister::POSITION,
                    data_type,
                    offset,
                }],
                stride,
            }],
            pretransformed: false,
        };
        (
            layout,
            vec![d3d::StreamBytes {
                data: bytes.into(),
                stride,
            }],
        )
    }

    #[test]
    fn a_fan_reads_its_positions_at_their_offset() {
        let fan4 = PrimitiveType::TriangleFan;
        let (l, s) = fan(24, 8, VertexDataType::Float4);
        assert_eq!(
            fan_corners(fan4, 4, false, &l, &s),
            Some([[0.0, 10.0], [1.0, 11.0], [2.0, 12.0], [3.0, 13.0]])
        );
        assert_eq!(
            fan_corners(PrimitiveType::TriangleStrip, 4, false, &l, &s),
            None
        );
        assert_eq!(fan_corners(fan4, 3, false, &l, &s), None);
        assert_eq!(fan_corners(fan4, 4, true, &l, &s), None, "indexed");
        let (l, s) = fan(24, 8, VertexDataType::Short2);
        assert_eq!(fan_corners(fan4, 4, false, &l, &s), None, "not floats");
        let (l, mut s) = fan(16, 0, VertexDataType::Float2);
        s[0].data = vec![0u8; 20].into();
        assert_eq!(
            fan_corners(fan4, 4, false, &l, &s),
            None,
            "a stream that ends early"
        );
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
