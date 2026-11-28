//! The sound table's bridge (docs/engine/sound-instances.md): the outside
//! sound entries as Rust bodies over `sim_sound::instances::Sounds`
//! ([`entries`]), the tick's sound phase, and the readers that hand the
//! table its inputs while tags, units and class state live in guest
//! memory. The table runs on every run; the game's own sound pool and
//! mixer loop never run.

pub mod ambience;
pub mod clock;
pub mod entries;
pub mod gate;
pub mod guest_tags;
pub mod levels;
pub mod lifecycle;
pub mod mixer_feed;
pub mod mouth;
pub mod owner_host;
pub mod playback;
pub mod promotion;
pub mod request;
pub mod scene;
pub mod units;
