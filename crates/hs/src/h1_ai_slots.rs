//! Converted Halo 1 AI calls on the retail Halo 2 interpreter.
//!
//! Only a named slot and a marked function-name atom opt a call into this
//! ABI. Halo 2 never evaluates that atom. Its data word selects the
//! operation, which supplies the real argument types to the native
//! fixed-signature evaluator. The retail descriptor still supplies the
//! result type. See docs/halo1/ai-control.md.

use crate::casts::types::{AI, OBJECT, OBJECT_LIST, VOID};

pub const CONTROL: u16 = 300;
pub const SEE_OBJECT: u16 = 305;
pub const CONTROL_TOKEN: &str = "h1_ai_control";
pub const DISCARD_EVALUATOR: u32 = 0x002a_b6c0;
pub const SEE_OBJECT_EVALUATOR: u32 = 0x002a_53b0;

/// Every retail builtin slot taken over conditionally by the AI lane.
/// Keeping these together lets the converter and the runtime check the
/// same index, evaluator and result type.
pub const CLAIMS: &[Claim] = &[
    Claim {
        slot: CONTROL,
        evaluator: DISCARD_EVALUATOR,
        result: VOID,
        retail_parameters: &[AI],
    },
    Claim {
        slot: SEE_OBJECT,
        evaluator: SEE_OBJECT_EVALUATOR,
        result: VOID,
        retail_parameters: &[AI, OBJECT],
    },
];

pub struct Claim {
    pub slot: u16,
    pub evaluator: u32,
    pub result: u16,
    pub retail_parameters: &'static [u16],
}

impl Claim {
    pub fn matches(&self, builtin: &crate::Builtin) -> bool {
        builtin.evaluate() == self.evaluator
            && builtin.return_type() == self.result
            && builtin.parameters() == self.retail_parameters
            && builtin.flags() & 2 == 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum Operation {
    SeePlayers = 1,
    Free = 2,
    FreeUnits = 3,
    MigrateUnits = 4,
    Teleport = 5,
    TeleportUnsupported = 6,
}

impl Operation {
    const MARKER: u32 = 0x4831_0000; // H1 in the high word

    pub const fn marker(self) -> u32 {
        Self::MARKER | self as u32
    }

    pub fn from_marker(word: u32) -> Option<Self> {
        Some(match word.checked_sub(Self::MARKER)? {
            1 => Self::SeePlayers,
            2 => Self::Free,
            3 => Self::FreeUnits,
            4 => Self::MigrateUnits,
            5 => Self::Teleport,
            6 => Self::TeleportUnsupported,
            _ => return None,
        })
    }

    pub const fn parameters(self) -> &'static [u16] {
        match self {
            Self::SeePlayers | Self::Free | Self::Teleport | Self::TeleportUnsupported => &[AI],
            Self::FreeUnits => &[OBJECT_LIST],
            Self::MigrateUnits => &[OBJECT_LIST, AI],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_claims_are_distinct_and_markers_are_exact() {
        assert!(CLAIMS.windows(2).all(|pair| pair[0].slot < pair[1].slot));
        assert_eq!(
            Operation::from_marker(Operation::Free.marker()),
            Some(Operation::Free)
        );
        assert_eq!(Operation::from_marker(0), None);
        assert_eq!(
            Operation::from_marker(Operation::Free.marker() | 0x100),
            None
        );
    }
}
