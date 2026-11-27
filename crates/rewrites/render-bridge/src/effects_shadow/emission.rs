//! R2's emission arm (docs/graphics/retire-d3d-r2.md, "R2's emission arm"):
//! each particle's emission, made in Rust by
//! [`engine::particle_emission::predict`] at the callback `0x00224550`'s
//! entry and compared at its call of the emitter `0x0023ed30` with what
//! the guest pushed, the vectors those words point at, and the input block
//! and draw context the guest wrote.
//!
//! Sampled frames only, through the same gate as the stamps and the
//! dispatch: a difference or refusal stops the gate answering.

use std::cell::RefCell;

use engine::memory::GuestMemory;
use engine::particle_emission::{self as pe, Prediction};

use super::{count, view_update, with_gate, Region};

/// The arm's name in the gate's counters (`emission_<outcome>`).
const ARM: &str = "emission";
/// Bytes compared of the input block (the cache and its mask) and of the
/// draw context.
const STATE_BYTES: u32 = 0x48;
const DRAW_BYTES: u32 = 0x10;

struct Open {
    esp: u32,
    prediction: Prediction,
    regions: [Region; 2],
}

thread_local! {
    static OPEN: RefCell<Option<Open>> = const { RefCell::new(None) };
}

/// At the callback's entry (`esp` its return address).
pub(super) fn begin(m: &dyn GuestMemory, esp: u32, mxcsr: u32, fcw: u16) {
    if OPEN.with(|o| o.borrow_mut().take()).is_some() {
        // A callback whose emitter call was never seen.
        count("emission_unbalanced", 1);
    }
    let Ok(frame) = m.u32(view_update::FRAME) else {
        return;
    };
    if !with_gate(|g| g.frame(frame)) {
        return;
    }
    let open = pe::predict(m, esp, mxcsr, fcw).and_then(|prediction| {
        let state = m.u32(esp.wrapping_add(8))?;
        let draw = m.u32(esp.wrapping_add(0x1c))?;
        Ok(Open {
            esp,
            prediction,
            regions: [Region::read(m, state, STATE_BYTES)?, Region::read(m, draw, DRAW_BYTES)?],
        })
    });
    match open {
        Ok(open) => OPEN.with(|o| *o.borrow_mut() = Some(open)),
        Err(e) => with_gate(|g| g.refused(ARM, pe::refusal(e))),
    }
}

/// What of the guest's call differs from the prediction, first first.
fn differs(m: &dyn GuestMemory, open: &Open, esp: u32, ebx: u32) -> Option<&'static str> {
    let e = &open.prediction.emission;
    let words = (0..8u32).map(|i| m.u32(esp.wrapping_add(4 + 4 * i)).ok());
    if !words.eq(e.words.iter().map(|&w| Some(w))) || ebx != e.ebx {
        return Some("words");
    }
    let vec4 = |at: u32| -> Option<[u32; 4]> {
        let mut v = [0; 4];
        for (i, w) in v.iter_mut().enumerate() {
            *w = m.u32(at.wrapping_add(4 * i as u32)).ok()?;
        }
        Some(v)
    };
    if vec4(e.words[6]) != Some(e.window) || vec4(e.words[7]) != Some(e.next_window) {
        return Some("window");
    }
    if vec4(e.ebx) != Some(e.corners) {
        return Some("corners");
    }
    let writes = &open.prediction.writes;
    let [state, draw] = &open.regions;
    if !state.holds(m, writes) {
        return Some("inputs");
    }
    if !draw.holds(m, writes) {
        return Some("sequence");
    }
    let outside = writes
        .iter()
        .filter(|(a, _)| !open.regions.iter().any(|r| r.contains(**a)));
    for (&a, &b) in outside {
        if m.u8(a).ok() != Some(b) {
            return Some("writes");
        }
    }
    None
}

/// At the emitter's entry (`esp` its return address, `ebx` the corners).
pub(super) fn emit(m: &dyn GuestMemory, esp: u32, ebx: u32) {
    if m.u32(esp).ok() != Some(pe::EMIT_RETURN) {
        return;
    }
    let Some(open) = OPEN.with(|o| o.borrow_mut().take()) else {
        return;
    };
    if open.esp.wrapping_sub(pe::EMIT_DEPTH) != esp {
        count("emission_unbalanced", 1);
        return;
    }
    match differs(m, &open, esp, ebx) {
        None => with_gate(|g| g.matched(ARM)),
        Some(what) => with_gate(|g| g.differed(ARM, what)),
    }
}

/// A snapshot load: an open callback never reaches its emitter.
pub(super) fn forget() {
    OPEN.with(|o| o.borrow_mut().take());
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use engine::memory::FlatMemory;
    use engine::particle_emission::constant;

    pub const ESP: u32 = 0x004f_0000;
    const DEF: u32 = 0x004c_0000;
    const STATE: u32 = 0x004c_2000;
    const DRAW: u32 = 0x004c_4000;
    const TINT: u32 = 0x004c_5000;
    const BLOB: u32 = 0x004c_6000;
    const ONE: u32 = 0x0045_dbc0;

    /// A particle whose definition takes its window and corners from
    /// itself and every property 0, at a sampled frame, entered at [`ESP`].
    pub fn world() -> FlatMemory {
        let mut m = FlatMemory::new(0x0041_0000, 0x000f_0000);
        m.set_u32(view_update::FRAME, super::super::SAMPLE * 7).unwrap();
        m.set_u32(ONE, 1.0f32.to_bits()).unwrap();
        m.set_u32(constant::PACK, 255.0f32.to_bits()).unwrap();
        m.set_u32(0x0046_86cc, BLOB).unwrap();
        m.set_u32(DEF, 0x1_0000).unwrap();
        for p in [0x1c, 0x2c, 0x3c, 0x4c] {
            m.set_u32(DEF + p + 8, 1).unwrap();
            m.set_u32(DEF + p + 0xc, BLOB).unwrap();
            m.set_u16(DEF + p + 6, u16::MAX).unwrap();
        }
        m.set_u32(DEF + 0x98, 1).unwrap();
        m.set_u32(DRAW, DEF).unwrap();
        let args = [DEF + 0x200, STATE, 0x004d_0000, 0x004d_0010, 0, TINT, DRAW];
        for (i, a) in args.into_iter().enumerate() {
            m.set_u32(ESP + 4 + 4 * i as u32, a).unwrap();
        }
        m
    }

    /// The guest's run as the model predicts it, up to the emitter.
    pub fn guest_runs(m: &mut FlatMemory) -> u32 {
        let p = pe::predict(&*m, ESP, 0x1f80, 0x027f).unwrap();
        for (&a, &b) in &p.writes {
            m.set_u8(a, b).unwrap();
        }
        let at = ESP - pe::EMIT_DEPTH;
        m.set_u32(at, pe::EMIT_RETURN).unwrap();
        for (i, w) in p.emission.words.iter().enumerate() {
            m.set_u32(at + 4 + 4 * i as u32, *w).unwrap();
        }
        for (base, v) in [
            (p.emission.words[6], p.emission.window),
            (p.emission.words[7], p.emission.next_window),
            (p.emission.ebx, p.emission.corners),
        ] {
            for (i, w) in v.iter().enumerate() {
                m.set_u32(base + 4 * i as u32, *w).unwrap();
            }
        }
        p.emission.ebx
    }

    fn counter(name: &str) -> u64 {
        super::super::tested(name)
    }

    #[test]
    fn an_emission_the_guest_makes_as_the_model_does_matches() {
        let mut m = world();
        let matched = counter("emission_matched");
        begin(&m, ESP, 0x1f80, 0x027f);
        let ebx = guest_runs(&mut m);
        emit(&m, ESP - pe::EMIT_DEPTH, ebx);
        assert_eq!(counter("emission_matched"), matched + 1);
        assert_eq!(m.u32(STATE + 0x44).unwrap(), 1, "the guest cached input 0");
    }

    #[test]
    fn a_word_or_a_cached_input_the_model_did_not_make_differs() {
        let mut m = world();
        let differed = counter("emission_mismatched_words");
        begin(&m, ESP, 0x1f80, 0x027f);
        let ebx = guest_runs(&mut m);
        m.set_u32(ESP - pe::EMIT_DEPTH + 4 + 4 * 3, 7).unwrap();
        emit(&m, ESP - pe::EMIT_DEPTH, ebx);
        assert_eq!(counter("emission_mismatched_words"), differed + 1);

        let mut m = world();
        let differed = counter("emission_mismatched_inputs");
        begin(&m, ESP, 0x1f80, 0x027f);
        let ebx = guest_runs(&mut m);
        m.set_u32(STATE + 4, 9).unwrap();
        emit(&m, ESP - pe::EMIT_DEPTH, ebx);
        assert_eq!(counter("emission_mismatched_inputs"), differed + 1);
    }

    #[test]
    fn another_callers_emitter_call_is_not_compared_and_a_refusal_is_named() {
        let mut m = world();
        let matched = counter("emission_matched");
        begin(&m, ESP, 0x1f80, 0x027f);
        let ebx = guest_runs(&mut m);
        m.set_u32(ESP - pe::EMIT_DEPTH, 0x0015_7442).unwrap();
        emit(&m, ESP - pe::EMIT_DEPTH, ebx);
        assert_eq!(counter("emission_matched"), matched);

        let mut m = world();
        m.set_u32(DRAW, 0x0100_0000).unwrap();
        let refused = counter("emission_refused_fault");
        begin(&m, ESP, 0x1f80, 0x027f);
        assert_eq!(counter("emission_refused_fault"), refused + 1);
    }
}
