//! The decal decorators' host: `render_bridge::native_decal_decorators::Host`.
//!
//! The stage drew each placement from a template taken from the guest
//! body's own resolved draw. There is no guest draw to take it from, so
//! the body and the cut are named gaps until the template is built from
//! the decorator's tags.

use std::sync::Arc;

use engine::memory::GuestMemory;
use render_bridge::native_decal_decorators::{self as stage, Body, Call};

struct Glue;

impl stage::Host for Glue {
    fn frame(&self) -> u64 {
        render_bridge::frame_wait::presents()
    }

    fn begin_body(&self, _mem: &mut dyn GuestMemory, _call: &Call, _sampled: bool) {
        unimplemented!("decal decorators: template from the guest draw")
    }

    fn end_body(&self, _mem: &mut dyn GuestMemory, _call: &Call) -> Body {
        unimplemented!("decal decorators: template from the guest draw")
    }

    fn holds(&self, _mem: &dyn GuestMemory, _call: &Call) -> Result<(), &'static str> {
        unimplemented!("decal decorators: template from the guest draw")
    }

    fn draw(&self, _mem: &mut dyn GuestMemory, _call: &Call) -> Result<(), &'static str> {
        unimplemented!("decal decorators: template from the guest draw")
    }
}

/// Installs the host, unless `impl.render.native-decal-decorators=off`
/// (or the native renderer is off).
pub fn install() {
    let Ok(switches) = hooks::Switches::from_env() else {
        return;
    };
    if !stage::on(&switches) {
        return;
    }
    stage::set_host(Arc::new(Glue));
}
