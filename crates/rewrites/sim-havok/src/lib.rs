//! Havok kernels. The engine adapters own guest layout and callback boundaries.
#![deny(clippy::disallowed_methods)]

pub mod array;
pub mod broadphase;
pub mod closest;
pub mod commit;
pub mod contact_point;
pub mod convex;
pub mod deactivation;
pub mod fp;
pub mod gjk;
pub mod heap;
pub mod integrate_phase;
pub mod island_integrate;
pub mod island_solve;
pub mod jacobian;
pub mod joint;
pub mod layout;
pub mod manifold;
pub mod mem;
pub mod motion;
pub mod motion_solve;
pub mod penetration;
pub mod shape_support;
pub mod solver;
pub mod structure;
pub mod support;
pub mod view;
pub mod weld;
