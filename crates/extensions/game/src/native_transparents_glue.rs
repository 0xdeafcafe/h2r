//! Stage 3's walk host. The guest runs its sorted handlers; the sorted
//! pools come from the world's ready state. A handler's indexed range was
//! the guest's own resolved draw, so it has no typed drawer: a range the
//! walk hands here panics by name.

use std::sync::Arc;

use render_bridge::native_transparents::{self, IndexRange};
use timeline::native::{ReservationCause, Stage};

struct Walk {
    state: crate::native_world_glue::State,
}

impl native_transparents::WalkHost for Walk {
    fn sorted_pools(&self, mem: &dyn engine::memory::GuestMemory) -> Option<Arc<std::collections::BTreeSet<u32>>> {
        let state = self.state.lock().ok()?;
        let ready = crate::native_world_glue::current_ready(&state, mem)?;
        Some(Arc::clone(&ready.water.get()?.0.pools))
    }

    fn draw(&self, _: &mut dyn engine::memory::GuestMemory, stage: Stage, range: IndexRange) -> Result<(), ReservationCause> {
        unimplemented!("transparents: {stage:?} indexed range of {} indices from the device", range.count)
    }
}

pub fn install(state: crate::native_world_glue::State) {
    native_transparents::set_walk_host(Arc::new(Walk { state }));
}
