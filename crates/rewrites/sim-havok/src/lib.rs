//! Havok kernels. The engine adapters own guest layout and callback boundaries.
#![deny(clippy::disallowed_methods)]

pub mod array;
pub mod broadphase;
pub mod convex;
pub mod deactivation;
pub mod fp;
pub mod integrate_phase;
pub mod island_integrate;
pub mod island_solve;
pub mod jacobian;
pub mod joint;
pub mod layout;
pub mod mem;
pub mod motion;
pub mod motion_solve;
pub mod solver;
pub mod structure;
pub mod support;
pub mod view;
pub mod weld;
