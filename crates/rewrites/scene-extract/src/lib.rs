//! Guest memory to `FrameScene` (docs/graphics/framescene.md, section
//! 4.1): the only render code with guest offsets.
//!
//! [`layout`] holds the offsets, one file per guest record, moved
//! unchanged from `native-render`; [`extract`] builds a scene's views from
//! them on the sim thread. Until the stages that still read guest memory
//! at their hooks have moved here (slices 3 to 6), `native-render`
//! re-exports the layout modules at their old paths and [`GuestRead`] is
//! shared with them.
//!
//! ## Writes the tick reads
//!
//! Section 5.2 of the spec: each slice that replaces a guest entry adds a
//! row here naming the tick-visible writes it reproduces and the test
//! that proves each. Slice 1 replaces no entry: extraction only reads,
//! the guest's front end runs as before, and the stages that draw from
//! the scene skip guest items by the same masks as before.
//!
//! | Entry replaced | Writes reproduced | Test |
//! |---|---|---|
//! | `0x0022aff1`, a proven HUD bitmap widget's draw (`hud`, `render_bridge::native_hud::walk`) | the widget globals `0x004e6978`..`0x004e69b0` (bitmap, image, inputs, colours) | `the_guests_quads_match_rusts_bit_for_bit` (`native_hud::walk`), and the shadow arm on each widget |
//! | `0x0022aa16`, a proven HUD text widget's draw | the inputs `0x004e6980`.. and the shader colour `0x004e6950`..; not the font renderer's state `0x004e73a0`.. | `a_number_widget_is_laid_out_ending_at_its_point_with_each_glyphs_shadow_first` |
//! | `0x0013a532`, a proven HUD definition's walk | none of its own (**S**: the walk, `0x0013a050`, the gate `0x0013ac87` and the inputs `0x0013ad48` store to their frames only); its widgets' as the two rows above | `the_loop_is_rusts_once_the_selection_and_every_shown_widget_are_proven` |
//! | `0x0022b8e2`, a proven screen-effect widget's body | its frame, the bitmap, image and inputs globals | `a_proven_screen_widgets_frame_is_the_call_the_guests_body_makes` |

pub mod decals;
pub mod decorators;
pub mod extract;
pub mod flares;
mod guest;
pub mod hud;
pub mod late;
pub mod layer_sort;
pub mod layout;
pub mod light_groups;
pub mod lights;
pub mod testing;
pub mod ui;
pub mod visibility;
pub mod weather;

pub use guest::{GuestRead, GuestWrite};
