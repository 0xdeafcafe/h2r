//! Scripted game types: the rules language, its verified IR, the
//! capability list and the VM.
//!
//! No I/O and no guest types: Studio, the cli, Live's upload route and the
//! host runtime link the same code. Source compiles to an [`Ir`] blob whose
//! SHA-256 names the script; the verifier reads the blob alone and refuses
//! it whole; the VM runs a verified blob against a [`Game`].
//! See [the design](../../../../docs/game/rules-engine.md).
#![forbid(unsafe_code)]

pub mod builtin;
pub mod caps;
pub mod fake;
pub mod ir;
pub mod lang;
pub mod scenario;
pub mod ty;
pub mod verify;
pub mod vm;
pub mod workbench;

pub use content::Sha256;
pub use ir::{DecodeError, Event, Ir};
pub use lang::{CompileError, compile};
pub use ty::Ty;
pub use verify::{Refusal, Report, load, verify};
pub use vm::{Call, Fired, Game, Input, RulesExt, ScriptError, Vm};

/// Why source did not become a runnable blob.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum BuildError {
    #[error(transparent)]
    Compile(#[from] CompileError),
    #[error(transparent)]
    Refused(#[from] Refusal),
}

/// Compiles and verifies source: what `h2 rules build` and Studio run.
pub fn build(src: &str) -> Result<(Ir, Report), BuildError> {
    let ir = compile(src)?;
    let report = verify(&ir)?;
    Ok((ir, report))
}
