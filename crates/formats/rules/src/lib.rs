//! Scripted game types: the rules language, its verified IR, the
//! capability list and the VM.
//!
//! No I/O and no guest types: Studio, the cli, Live's upload route and the
//! host runtime link the same code. Source compiles to an [`Ir`] blob whose
//! SHA-256 names the script; the verifier reads the blob alone and refuses
//! it whole; the VM runs a verified blob against a [`Game`].
//! See [the design](../../../../docs/game/rules-engine.md).
#![forbid(unsafe_code)]

pub mod bodies;
pub mod builtin;
pub mod caps;
pub mod fake;
pub mod ide;
pub mod ir;
pub mod labels;
pub mod lang;
pub mod policy;
pub mod reload;
pub mod scenario;
pub mod settings;
pub mod show;
pub mod sim;
pub mod ty;
pub mod types;
pub mod vehicles;
pub mod verify;
pub mod vm;
pub mod weapons;
pub mod workbench;

pub use content::Sha256;
pub use ir::{DecodeError, Event, Ir};
pub use lang::{CompileError, Compiled, Diagnostic, Severity, compile, diagnose};
pub use ty::Ty;
pub use verify::{Refusal, Report, load, verify};
pub use vm::{Call, Fired, Game, Input, RulesExt, ScriptError, Vm};

/// The game-engine index (record `+0x44`) of a script game with no retail
/// engine object under it: 6, the slot of the deprecated Headhunter
/// engine. It is inside every retail range check (the clean-up clamps to
/// 1..9, the protocol map's table covers 1..9), it fits the codec's 4-bit
/// field, its engine table entry is never written by the game, and every
/// retail path left unanswered fails safe on it: the clean-up resets the
/// record to Slayer, the codec refuses it, the protocol map answers
/// synchronous. The record clean-up and the protocol map answer it
/// natively (`variants::record::clean_up`, `variants::record::protocol`).
/// See docs/game/game-types-without-carriers.md, "No retail engine object".
pub const SCRIPT_INDEX: u8 = 6;

/// Why source did not become a runnable blob.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum BuildError {
    #[error(transparent)]
    Compile(#[from] CompileError),
    #[error(transparent)]
    Refused(#[from] Refusal),
    /// The worst-tick estimate is over the VM's per-tick budget.
    #[error("the worst tick costs {worst} ops, over the budget of {budget}")]
    OverBudget { worst: u64, budget: u32 },
    /// The encoded blob is over the IR cap.
    #[error("the IR is {bytes} bytes, over the {max} byte cap")]
    TooLarge { bytes: usize, max: usize },
}

/// Compiles and verifies source. The blob may still be over the budget;
/// [`ship`] refuses that.
pub fn build(src: &str) -> Result<(Ir, Report), BuildError> {
    let ir = compile(src)?;
    let report = verify(&ir)?;
    Ok((ir, report))
}

/// What a blob must hold to before it is written out or published: the
/// worst-tick estimate fits the budget and the encoding fits the IR cap.
pub fn fits(ir: &Ir, report: &Report) -> Result<(), BuildError> {
    if !report.fits() {
        return Err(BuildError::OverBudget {
            worst: report.worst_tick,
            budget: vm::TICK_BUDGET,
        });
    }
    let bytes = ir.encode().len();
    if bytes > ir::MAX_IR_BYTES {
        return Err(BuildError::TooLarge {
            bytes,
            max: ir::MAX_IR_BYTES,
        });
    }
    Ok(())
}

/// Compiles, verifies and refuses a blob that does not [`fits`]: what
/// `h2 rules build`, Live's save and publish routes and a variant file
/// made from source run.
pub fn ship(src: &str) -> Result<(Ir, Report), BuildError> {
    let (ir, report) = build(src)?;
    fits(&ir, &report)?;
    Ok((ir, report))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ship_refuses_a_script_over_the_budget() {
        // 16 players times 16 players times the body, once per kill of 16.
        let src = "rules \"Heavy\" 1\ntype slayer\nglobal var n: int = 0\n\
                   on kill(v, k) { for each players a { for each players b { for each players c { n = n + 1 } } } }\n";
        let (ir, report) = build(src).unwrap();
        assert!(!report.fits(), "worst tick {}", report.worst_tick);
        assert!(matches!(ship(src), Err(BuildError::OverBudget { .. })));
        assert!(fits(&ir, &report).is_err());
        assert!(ship(builtin::SLAYER).is_ok());
    }

    #[test]
    fn options_pack_by_name_in_declaration_order() {
        use ir::{OptionError, OptionValue};
        let src = "rules \"O\" 1\ntype slayer\nenum Mode { calm, wild }\n\
                   option points: int 1..5 = 1\n\
                   option costs: bool = true\n\
                   option mode: Mode = Mode.calm\n\
                   on tick { }\n";
        let ir = compile(src).unwrap();
        assert_eq!(ir.pack_options(&[]).unwrap(), ir.default_options());
        let given = |list: &[(&str, &str)]| -> Vec<(String, OptionValue)> {
            list.iter()
                .map(|(k, v)| (k.to_string(), OptionValue::parse(v)))
                .collect()
        };
        // points takes 3 bits (value - 1), costs 1 (false is 0), mode 1.
        let bits = ir
            .pack_options(&given(&[("points", "4"), ("costs", "false"), ("mode", "wild")]))
            .unwrap();
        assert_eq!(bits, 3 | (1 << 4));
        // A host reads them back by name, as the VM does.
        assert_eq!(ir.option_value(bits, "points"), Some(4));
        assert_eq!(ir.option_value(bits, "costs"), Some(0));
        assert_eq!(ir.option_value(bits, "mode"), Some(1));
        assert_eq!(ir.option_value(ir.default_options(), "points"), Some(1));
        assert_eq!(ir.option_value(ir.default_options(), "costs"), Some(1));
        assert_eq!(ir.option_value(ir.default_options(), "mode"), Some(0));
        assert_eq!(ir.option_value(bits, "nope"), None);
        assert_eq!(
            ir.pack_options(&given(&[("nope", "1")])),
            Err(OptionError::Unknown("nope".into()))
        );
        assert_eq!(
            ir.pack_options(&given(&[("points", "9")])),
            Err(OptionError::Range { name: "points".into(), lo: 1, hi: 5 })
        );
        assert!(matches!(
            ir.pack_options(&given(&[("mode", "loud")])),
            Err(OptionError::NoValue { .. })
        ));
        assert!(matches!(
            ir.pack_options(&given(&[("costs", "true"), ("costs", "false")])),
            Err(OptionError::Twice(_))
        ));
    }
}
