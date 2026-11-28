//! R2's answer (docs/graphics/retire-d3d-r2.md, "R2's answer"): on every
//! frame the bodies Rust models do not run.
//!
//! - **Particles.** At `0x00224550`'s entry the emission is made in Rust
//!   ([`engine::particle_emission`]) and drawn through the native
//!   particle path ([`crate::native_particles::draw_emission`]); its
//!   writes (the input cache, the sequence) are made in the guest and the
//!   callback returns. The emitter `0x0023ed30` never runs, so no particle
//!   quad of the sorted dispatch reaches D3D.
//!   With a mirror host ([`mirror`]) the particle is held as a GPU
//!   particle system's birth and its system draws when `0x002243c0`
//!   returns.
//! - **Contrails.** At `0x00042cf0`'s entry the ribbon is made in Rust
//!   ([`engine::contrail_ribbon`]), written where the builder writes it,
//!   and the builder returns its AL; the polygon handler's draw of it is
//!   the native effect path's.
//!
//! - **Stamps.** At `0x00174420`'s entry the outermost call is made in
//!   Rust ([`view_update`]) over the arena with the render's stamps on it
//!   ([`stamps::View`]); its stamps stay in [`stamps`], its other writes
//!   (the particle-model singletons, the sorted entries) are made in the
//!   guest, and the call returns.
//!
//! What the answer does not take keeps its guest body, counted
//! `scene_kept_15_<kind>_<cause>`: the model's refusal (`fault`, `simd`,
//! `x87`, `unbounded`, ...) or the native draw's (`refused`, `no_host`,
//! `declaration`, ...).

use engine::contrail_ribbon as cr;
use guest_layout::memory::GuestMemory;
use engine::particle_emission as pe;

use super::{count, mirror, sse_supported, stamps, view_update, with_gate};

/// Whether this call is the answer's: every call in a frame that reads.
/// One whose frame does not read keeps its body, `scene_kept_15_<kind>_frame`.
fn ours(m: &dyn GuestMemory, kind: &str) -> bool {
    let Ok(frame) = m.u32(view_update::FRAME) else {
        kept(kind, "frame");
        return false;
    };
    with_gate(|g| g.frame(frame));
    true
}

fn kept(kind: &str, cause: &str) {
    crate::native_layers::count_kept_as("", 15, kind, cause, 1);
}

/// Makes `writes` in the guest. The model read every byte first, so a
/// store does not fault; one that does is counted and the rest go on.
fn apply<'a>(m: &mut dyn GuestMemory, writes: impl IntoIterator<Item = (&'a u32, &'a u8)>) {
    for (&a, &b) in writes {
        if m.set_u8(a, b).is_err() {
            count("answer_fault", 1);
        }
    }
}

/// At `0x00174420`'s entry (`esp` its return address), on a frame the
/// answer takes. True when the call is made and returns (`ret 0x10`);
/// false runs the guest's body.
pub(super) fn view_update(m: &mut dyn GuestMemory, esp: u32, mxcsr: u32, cw: u16) -> bool {
    if !sse_supported(mxcsr) {
        kept("stamps", "mxcsr");
        return false;
    }
    let mut args = [0; 4];
    for (i, a) in (0..).zip(args.iter_mut()) {
        match m.u32(esp.wrapping_add(4 + 4 * i)) {
            Ok(v) => *a = v,
            Err(_) => {
                kept("stamps", "fault");
                return false;
            }
        }
    }
    let rest = stamps::with(|s| {
        let p = view_update::predict(&s.view(&*m)?, args, cw)?;
        let rest = s.keep(&*m, &p.writes)?;
        Ok::<_, view_update::Refusal>((rest, p.entries.len()))
    });
    match rest {
        Ok((rest, entries)) => {
            apply(m, &rest);
            count("answer_stamps", 1);
            count("stamps_entries_queued", entries as u64);
            true
        }
        Err(r) => {
            kept("stamps", r.name());
            false
        }
    }
}

/// At `0x00224550`'s entry (`esp` its return address). True when the
/// particle is drawn (or the emitter is off) and the callback returns
/// (`ret 0x1c`); false runs the guest's body.
pub(super) fn particle(m: &mut dyn GuestMemory, esp: u32, mxcsr: u32, fcw: u16) -> bool {
    if !ours(&*m, "particle") {
        return false;
    }
    let p = match pe::predict(&*m, esp, mxcsr, fcw) {
        Ok(p) => p,
        Err(e) => {
            kept("particle", pe::refusal(e));
            return false;
        }
    };
    // The mirror holds it for the draw's batch, or draws its batch so
    // this one's direct draw keeps the guest's order.
    if mirror::take(m, esp, &p) {
        apply(m, &p.writes);
        return true;
    }
    // The emitter's frame, where the callback would push it (below ESP).
    let e = &p.emission;
    let at = esp.wrapping_sub(pe::EMIT_DEPTH);
    let mut frame: Vec<(u32, u32)> = vec![(at, pe::EMIT_RETURN)];
    frame.extend(e.words.iter().enumerate().map(|(i, &w)| (at + 4 + 4 * i as u32, w)));
    for (base, v) in [(e.words[6], e.window), (e.words[7], e.next_window), (e.ebx, e.corners)] {
        frame.extend(v.iter().enumerate().map(|(i, &w)| (base + 4 * i as u32, w)));
    }
    if frame.iter().any(|&(a, w)| m.set_u32(a, w).is_err()) {
        kept("particle", "stack");
        return false;
    }
    match crate::native_particles::draw_emission(m, at, e.ebx) {
        Ok(_) => {
            apply(m, &p.writes);
            count("answer_particles", 1);
            true
        }
        Err(cause) => {
            kept("particle", cause);
            false
        }
    }
}

/// At `0x00042cf0`'s entry (`esp` its return address). `Some(al)` when
/// the ribbon is written and the builder returns (`ret 0xc`); `None` runs
/// the guest's body.
pub(super) fn contrail(m: &mut dyn GuestMemory, esp: u32, mxcsr: u32, fcw: u16) -> Option<u8> {
    if !ours(&*m, "contrail") {
        return None;
    }
    let p = match cr::predict(&*m, esp, mxcsr, fcw) {
        Ok(p) => p,
        Err(e) => {
            kept("contrail", cr::refusal(e));
            return None;
        }
    };
    apply(m, &p.writes);
    if m.write(p.out, &p.vertices).is_err() {
        count("answer_fault", 1);
    }
    count("answer_contrails", 1);
    Some(p.ret)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::native_particles::{self as np, Call, Host, Producer};
    use super::super::fixtures::{ESP, world};
    use guest_layout::memory::FlatMemory;

    fn counter(name: &str) -> u64 {
        crate::native_layers::counters()
            .into_iter()
            .find(|(k, _)| *k == name)
            .map_or(0, |(_, v)| v)
    }

    /// A host that takes every direct particle and records it.
    #[derive(Default)]
    struct Takes(Mutex<Vec<Call>>, bool);

    impl Host for Takes {
        fn try_emit(&self, _: &mut dyn GuestMemory, call: &Call) -> bool {
            self.0.lock().unwrap().push(call.clone());
            self.1
        }
        fn arm(&self, _: &mut dyn GuestMemory, _: &Call) -> bool {
            false
        }
        fn arm_dynamic(&self, _: &mut dyn GuestMemory, _: &native_render::effect_geometry::DynamicCall) -> bool {
            false
        }
        fn arm_effect(&self, _: &mut dyn GuestMemory, _: native_render::effect_immediate::Site) -> bool {
            false
        }
        fn arm_flare(&self, _: &mut dyn GuestMemory, _: native_render::flares::Site) {}
        fn publish(&self, _: &mut dyn GuestMemory, _: Producer, _: u32) {}
        fn finish(&self, _: &mut dyn GuestMemory, _: Producer) {}
    }

    /// The world with the emitter on and its register table the retail
    /// one.
    fn answering_world() -> FlatMemory {
        let mut m = world();
        m.set_u8(native_render::particles::EMIT_ENABLED, 1).unwrap();
        for (i, r) in [1u32, 2, 3, 4, 5, 6, 7, 8].into_iter().enumerate() {
            m.set_u32(native_render::particles::ATTRIBUTE_REGISTERS + 4 * i as u32, r).unwrap();
        }
        m
    }

    #[test]
    fn an_answered_particle_draws_from_the_rust_emission_and_a_refused_one_keeps_its_guest_body() {
        let mut m = answering_world();
        let host = Arc::new(Takes(Mutex::default(), true));
        np::set_host(host.clone());
        let want = pe::predict(&m, ESP, 0x1f80, 0x027f).unwrap();
        assert!(particle(&mut m, ESP, 0x1f80, 0x027f), "the callback returns");
        let calls = host.0.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].esp, ESP - pe::EMIT_DEPTH);
        assert_eq!(calls[0].return_address, pe::EMIT_RETURN);
        // The emitter never ran: what drew is what Rust made.
        let colour = calls[0].emission.attributes[8].unwrap();
        assert_eq!(colour[3], (f32::from((want.emission.words[4] >> 24) as u8) / 255.0).to_bits());
        for (&a, &b) in &want.writes {
            assert_eq!(m.u8(a).unwrap(), b, "the guest holds the model's writes");
        }
        drop(calls);

        // A particle the host refuses keeps its guest body, by name.
        let mut m = answering_world();
        np::set_host(Arc::new(Takes(Mutex::default(), false)));
        let kept = counter("scene_kept_15_particle_refused");
        let before = m.u32(0x004c_2000 + 0x44).unwrap();
        assert!(!particle(&mut m, ESP, 0x1f80, 0x027f));
        assert_eq!(counter("scene_kept_15_particle_refused"), kept + 1);
        assert_eq!(m.u32(0x004c_2000 + 0x44).unwrap(), before, "the guest makes the writes itself");
    }

    #[test]
    fn every_frame_is_the_answers_and_an_unread_frame_is_kept_by_name() {
        let mut m = answering_world();
        for frame in [64, 65, 128] {
            m.set_u32(view_update::FRAME, frame).unwrap();
            assert!(ours(&m, "particle"), "frame {frame}");
        }
        let unread = FlatMemory::new(0x0010_0000, 0x10);
        let before = counter("scene_kept_15_particle_frame");
        assert!(!ours(&unread, "particle"));
        assert_eq!(counter("scene_kept_15_particle_frame"), before + 1);
    }
}
