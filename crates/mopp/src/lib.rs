//! Havok MOPP code for Halo 2 structure BSPs (docs/formats/mopp.md).
//!
//! - [`code`]: the `sbsp+0x1e0` block (header and bytecode).
//! - [`vm`]: the engine's interpreter semantics as a query model.
//! - [`boxvm`]: the engine's box query, which keeps bodies' contacts.
//! - [`check`]: static validation by the engine's rules.
//! - [`build`]: the compiler.
//! - [`bounds`]: the box a program guarantees for each key.

pub mod bounds;
pub mod boxvm;
pub mod build;
pub mod check;
pub mod code;
pub mod geom;
pub mod vm;

pub use boxvm::query_box;
pub use build::{build, build_structure};
pub use code::Code;
pub use geom::Surface;
pub use vm::{Aabb, query_aabb, query_sphere};
