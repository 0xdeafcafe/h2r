//! `native-fog-fans`: the second fog's screen fans (`0x00025dc0`)
//! (docs/graphics/retire-d3d-fog-fans.md). On with the native renderer.
//!
//! The native renderer has no stage for the fans yet, so a call that
//! would draw one is `unimplemented!("fog fans: <cause>")`, the cause
//! naming the fans ([`engine::fog_fans::Fans::cause`]), or `simd` and
//! `unread` for a gate the SSE model refuses or that does not read. A call
//! that draws no fan runs as the game's: its tail (the depth clip control
//! and two words) makes no draw.

use engine::fog_fans::{self as ff, Fans};
use engine::memory::GuestMemory;
use hooks::{BuildError, Builder, Feature, Manifest};

pub const ID: &str = "native-fog-fans";

/// Whether the cut is placed: with the native renderer.
pub fn on(switches: &hooks::Switches) -> bool {
    hooks::switches::native_render_on(switches)
}

/// A fan the native renderer cannot draw, by cause.
pub fn unported(cause: &str) -> ! {
    unimplemented!("fog fans: {cause}")
}

/// `0x00025dc0`'s entry with `ecx` and `edx` under the call's MXCSR: a
/// call that draws a fan panics, named.
pub fn before_fans(mem: &dyn GuestMemory, mxcsr: u32, ecx: u32, edx: u32) {
    match ff::fans(mem, mxcsr, ecx as u8, edx as u8) {
        Ok(Fans::None) => {}
        Ok(fans) => unported(fans.cause()),
        Err(engine::light_constants::Error::Simd) => unported("simd"),
        Err(_) => unported("unread"),
    }
}

pub struct NativeFogFans;

impl Feature for NativeFogFans {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "The second fog's screen fans, a named panic until the native renderer draws them")
            .doc("docs/graphics/retire-d3d-fog-fans.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        if !on(b.switches()) {
            return Ok(());
        }
        b.raw()
            .observe_before(&engine::points::FOG_FANS, "native_fog_fans", |o| {
                before_fans(&*o.guest(), o.cpu.mxcsr, o.cpu.ecx, o.cpu.edx)
            })
            .context_only();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::fog_fans::global;
    use engine::memory::FlatMemory;

    const MXCSR: u32 = 0x1f80;

    fn mem(amount: f32) -> FlatMemory {
        let mut m = FlatMemory::new(0x0040_0000, 0x0012_0000);
        m.set_u32(global::DATUM, 0xe1a3_0012).unwrap();
        m.set_f32(global::AMOUNT, amount).unwrap();
        m.set_f32(global::ZERO, 0.0).unwrap();
        m
    }

    #[test]
    fn a_call_with_no_fan_runs() {
        before_fans(&mem(0.0), MXCSR, 0x01, 0x01);
        before_fans(&mem(0.5), MXCSR, 0xff00, 0xff00);
    }

    #[test]
    #[should_panic(expected = "fog fans: layers")]
    fn the_layers_panic() {
        before_fans(&mem(0.5), MXCSR, 0x01, 0);
    }

    #[test]
    #[should_panic(expected = "fog fans: composite")]
    fn the_composite_panics() {
        before_fans(&mem(0.5), MXCSR, 0, 0x01);
    }

    #[test]
    #[should_panic(expected = "fog fans: unread")]
    fn an_unread_gate_panics() {
        let m = FlatMemory::new(0x0010_0000, 0x10);
        before_fans(&m, MXCSR, 1, 0);
    }
}
