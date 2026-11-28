//! The sky's cut: the host of `render_bridge::native_sky::before_sky`.
//!
//! The cut replayed strips kept from a guest sky pass. There is no guest
//! pass to keep them from, so each entry is a named gap until the sky is
//! built from the BSP's own sections.

use std::sync::Arc;

use guest_layout::memory::GuestMemory;

struct Glue;

impl render_bridge::native_sky::CutHost for Glue {
    fn begin_guest(&self, _mem: &dyn GuestMemory) {
        unimplemented!("sky: view block from the device")
    }

    fn end_guest(&self) {
        unimplemented!("sky: strips from the guest pass")
    }

    fn draw(&self, _mem: &mut dyn GuestMemory) -> Result<usize, &'static str> {
        unimplemented!("sky: strips from the guest pass")
    }
}

/// Installs the cut's host.
pub fn install() {
    render_bridge::native_sky::set_cut_host(Arc::new(Glue));
}
