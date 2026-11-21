//! Built-in scripts, compiled into every build and found by engine index
//! for a record with no mode. See `docs/game/rules-engine.md`, "A record
//! with no mode".

/// Slayer (engine index 2): kill, suicide and betrayal scoring.
pub const SLAYER: &str = include_str!("slayer.rules");

/// The built-in script source for a retail engine index, for the engines
/// whose script exists.
pub fn builtin(engine_index: u8) -> Option<&'static str> {
    match engine_index {
        2 => Some(SLAYER),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_compiles_verifies_and_rides_its_own_engine() {
        for index in 0..=u8::MAX {
            let Some(src) = builtin(index) else { continue };
            let ir = crate::lang::compile(src).unwrap();
            assert_eq!(ir.header.carrier, index);
            let report = crate::verify::verify(&ir).unwrap();
            assert!(report.fits());
            assert_eq!(crate::verify::unhostable(&ir), None);
            assert!(ir.header.requires.is_empty());
        }
        assert!(builtin(2).is_some());
        assert!(builtin(1).is_none());
    }

    #[test]
    fn compiling_a_builtin_is_deterministic() {
        let a = crate::lang::compile(SLAYER).unwrap();
        let b = crate::lang::compile(SLAYER).unwrap();
        assert_eq!(a.encode(), b.encode());
        assert_eq!(a.id(), b.id());
    }
}
