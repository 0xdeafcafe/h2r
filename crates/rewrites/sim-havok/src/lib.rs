//! Havok kernels. The engine adapters own guest layout and callback boundaries.
#![deny(clippy::disallowed_methods)]

pub mod array;
pub mod broadphase;
pub mod fp;
pub mod jacobian;
pub mod layout;
pub mod mem;
pub mod motion;
pub mod solver;
pub mod structure;
pub mod support;
pub mod view;
pub mod weld;
