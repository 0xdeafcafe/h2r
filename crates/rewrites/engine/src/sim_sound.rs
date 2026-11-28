//! The sound frame, from source selection to mixer submission. Fused bodies
//! share the real guest stack so a DirectSound callback can interrupt a voice
//! update, change its record and return to the same instruction.

use crate::sim_objects::Table;
use std::sync::LazyLock;

pub mod ambience;
pub mod clock;
pub mod gate;
pub mod levels;
pub mod mouth;
pub mod playback;
pub mod promotion;
pub mod units;

pub static TABLE: LazyLock<Table> = LazyLock::new(|| {
    let groups = [
        crate::native_sound_ambience::BODIES,
        crate::native_sound_channel::BODIES,
        crate::native_sound_environment::BODIES,
        crate::native_sound_impulse::BODIES,
        crate::native_sound_listener::BODIES,
        crate::native_sound_loops::BODIES,
        crate::native_sound_mix::BODIES,
        crate::native_sound_objects::BODIES,
        crate::native_sound_scheduling::BODIES,
        crate::native_sound_segments::BODIES,
        crate::native_sound_sources::BODIES,
        crate::native_sound_spatial::BODIES,
        crate::native_sound_submix::BODIES,
        crate::native_sound_tick::BODIES,
    ];
    Table::new(groups.into_iter().flatten())
        .expect("sound entries and continuation sites are unique")
        .require_nearest_x87()
});

/// Selectors that finish a partially native sound body in the image. The
/// sound bodies run each of their paths to its end natively, so none hands
/// control back to the generated image.
pub fn generated_sites(_entry: u32) -> &'static [u32] {
    &[]
}
