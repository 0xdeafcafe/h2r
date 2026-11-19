//! Experiments exposed only to the opt-in integration benchmark.
#[path = "pack_floor/arithmetic.rs"]
mod arithmetic;
#[path = "pack_floor/native.rs"]
mod native;
pub use native::speed::{compare, profile, verify};
