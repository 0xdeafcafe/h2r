//! R2's ribbon arm (docs/graphics/retire-d3d-r2.md, "R2's ribbon arm"): each
//! contrail's vertices, made in Rust by
//! [`engine::contrail_ribbon::predict`] at the builder `0x00042cf0`'s
//! entry and compared at its return with the vertices the guest wrote,
//! its AL, every other byte it wrote and the draw context it read.
//!
//! Sampled frames only, through the same gate as the stamps, the dispatch
//! and the emission: a difference or refusal stops the gate answering.

use std::cell::RefCell;

use engine::contrail_ribbon::{self as cr, Prediction};
use engine::memory::GuestMemory;

use super::{count, view_update, with_gate, Region};

/// The arm's name in the gate's counters (`ribbon_<outcome>`).
const ARM: &str = "ribbon";
/// Bytes compared of the draw context (the builder only reads it).
const CONTEXT_BYTES: u32 = 0x18;

struct Open {
    esp: u32,
    prediction: Prediction,
    context: Region,
}

thread_local! {
    static OPEN: RefCell<Option<Open>> = const { RefCell::new(None) };
}

/// At the builder's entry (`esp` its return address).
pub(super) fn begin(m: &dyn GuestMemory, esp: u32, mxcsr: u32, fcw: u16) {
    if OPEN.with(|o| o.borrow_mut().take()).is_some() {
        // A builder whose return was never seen.
        count("ribbon_unbalanced", 1);
    }
    let Ok(frame) = m.u32(view_update::FRAME) else {
        return;
    };
    if !with_gate(|g| g.frame(frame)) {
        return;
    }
    let open = cr::predict(m, esp, mxcsr, fcw).and_then(|prediction| {
        let context = m.u32(esp.wrapping_add(0xc))?;
        Ok(Open {
            esp,
            prediction,
            context: Region::read(m, context, CONTEXT_BYTES)?,
        })
    });
    match open {
        Ok(open) => OPEN.with(|o| *o.borrow_mut() = Some(open)),
        Err(e) => with_gate(|g| g.refused(ARM, cr::refusal(e))),
    }
}

/// What of the guest's run differs from the prediction, first first.
fn differs(m: &dyn GuestMemory, open: &Open, al: u8) -> Option<&'static str> {
    let p = &open.prediction;
    if !m.bytes(p.out, p.vertices.len()).is_ok_and(|v| v == p.vertices) {
        return Some("vertices");
    }
    if al != p.ret {
        return Some("return");
    }
    for (&a, &b) in p.writes.iter().filter(|(a, _)| !p.in_vertices(**a)) {
        if m.u8(a).ok() != Some(b) {
            return Some("writes");
        }
    }
    if !open.context.holds(m, &p.writes) {
        return Some("context");
    }
    None
}

/// At the builder's return (`esp` past its arguments, `eax` its result).
pub(super) fn end(m: &dyn GuestMemory, esp: u32, eax: u32) {
    let Some(open) = OPEN.with(|o| o.borrow_mut().take()) else {
        return;
    };
    if open.esp.wrapping_add(cr::POP) != esp {
        count("ribbon_unbalanced", 1);
        return;
    }
    match differs(m, &open, eax as u8) {
        None => with_gate(|g| g.matched(ARM)),
        Some(what) => with_gate(|g| g.differed(ARM, what)),
    }
}

/// A snapshot load: an open builder never returns.
pub(super) fn forget() {
    OPEN.with(|o| o.borrow_mut().take());
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::memory::FlatMemory;
    use engine::contrail_ribbon::constant;

    const ESP: u32 = 0x004f_0000;
    const OUT: u32 = 0x004d_0000;
    const CTX: u32 = 0x004c_0000;
    const C: u32 = 0x004c_1000;
    const DEF: u32 = 0x004c_2000;
    const MODE: u32 = 0x004c_3000;
    const STAGES: u32 = 0x004c_4000;
    const ARRAY: u32 = 0x004c_5000;
    const DATA: u32 = 0x004c_6000;
    const ONE: u32 = 0x0045_dbc0;
    const SMALL: u32 = 0x0045_dbdc;

    /// A two-point camera-facing ribbon at a sampled frame, entered at
    /// [`ESP`].
    fn world() -> FlatMemory {
        let mut m = FlatMemory::new(0x0041_0000, 0x0010_0000);
        m.set_u32(view_update::FRAME, super::super::SAMPLE * 7).unwrap();
        for (at, v) in [(ONE, 1.0f32), (SMALL, 1e-4), (constant::HALF, 0.5), (constant::PACK, 255.0)] {
            m.set_u32(at, v.to_bits()).unwrap();
        }
        m.set_u32(constant::POINTS, ARRAY).unwrap();
        m.set_u32(ARRAY + 0x44, DATA).unwrap();
        m.set_u32(constant::CAMERA + 8, 10.0f32.to_bits()).unwrap();
        m.set_u32(CTX, C).unwrap();
        m.set_u32(CTX + 4, DEF).unwrap();
        m.set_u32(CTX + 0xc, MODE).unwrap();
        m.set_u32(CTX + 0x14, 4).unwrap();
        m.set_u32(C + 8, u32::MAX).unwrap();
        m.set_u32(C + 0x38, 0).unwrap();
        m.set_u16(DEF + 0x18, 4).unwrap();
        m.set_u32(DEF + 0xe8, 1).unwrap();
        m.set_u32(DEF + 0xec, STAGES).unwrap();
        for i in 0..9 {
            m.set_u32(STAGES + 0x18 + 4 * i, 1.0f32.to_bits()).unwrap();
        }
        m.set_u32(DATA + 0x34, 1).unwrap();
        m.set_u32(DATA + 0x38 + 0x1c, 1.0f32.to_bits()).unwrap();
        m.set_u32(DATA + 0x38 + 0x34, u32::MAX).unwrap();
        for (i, a) in [OUT, 96, CTX].into_iter().enumerate() {
            m.set_u32(ESP + 4 + 4 * i as u32, a).unwrap();
        }
        m
    }

    /// The guest's run as the model predicts it: its writes and its AL.
    fn guest_runs(m: &mut FlatMemory) -> u32 {
        let p = cr::predict(&*m, ESP, 0x1f80, 0x027f).unwrap();
        for (&a, &b) in &p.writes {
            m.set_u8(a, b).unwrap();
        }
        0x1234_5600 | u32::from(p.ret)
    }

    fn counter(name: &str) -> u64 {
        let key = format!("layer_plan_15_effects_shadow_{name}");
        crate::native_layers::counters()
            .into_iter()
            .find(|(k, _)| *k == key)
            .map_or(0, |(_, v)| v)
    }

    #[test]
    fn a_ribbon_the_guest_builds_as_the_model_does_matches() {
        let mut m = world();
        let matched = counter("ribbon_matched");
        begin(&m, ESP, 0x1f80, 0x027f);
        let eax = guest_runs(&mut m);
        end(&m, ESP + cr::POP, eax);
        assert_eq!(counter("ribbon_matched"), matched + 1);
        assert_eq!(m.u32(OUT + 0x18 * 3).unwrap(), 1.0f32.to_bits(), "the guest wrote the second point");
    }

    #[test]
    fn a_vertex_a_result_or_a_stray_write_the_model_did_not_make_differs() {
        let mut m = world();
        let differed = counter("ribbon_mismatched_vertices");
        begin(&m, ESP, 0x1f80, 0x027f);
        let eax = guest_runs(&mut m);
        m.set_u32(OUT + 4, 7).unwrap();
        end(&m, ESP + cr::POP, eax);
        assert_eq!(counter("ribbon_mismatched_vertices"), differed + 1);

        let mut m = world();
        let differed = counter("ribbon_mismatched_return");
        begin(&m, ESP, 0x1f80, 0x027f);
        let eax = guest_runs(&mut m);
        end(&m, ESP + cr::POP, eax & !0xff);
        assert_eq!(counter("ribbon_mismatched_return"), differed + 1);

        let mut m = world();
        let differed = counter("ribbon_mismatched_context");
        begin(&m, ESP, 0x1f80, 0x027f);
        let eax = guest_runs(&mut m);
        m.set_u32(CTX + 0x10, 1).unwrap();
        end(&m, ESP + cr::POP, eax);
        assert_eq!(counter("ribbon_mismatched_context"), differed + 1);
    }

    #[test]
    fn an_unseen_return_is_unbalanced_and_a_refusal_is_named() {
        let mut m = world();
        let unbalanced = counter("ribbon_unbalanced");
        begin(&m, ESP, 0x1f80, 0x027f);
        let eax = guest_runs(&mut m);
        end(&m, ESP + 4, eax);
        assert_eq!(counter("ribbon_unbalanced"), unbalanced + 1);

        let mut m = world();
        m.set_u32(DATA + 0x38 + 0x34, 0).unwrap();
        let refused = counter("ribbon_refused_unbounded");
        begin(&m, ESP, 0x1f80, 0x027f);
        assert_eq!(counter("ribbon_refused_unbounded"), refused + 1);
    }
}
