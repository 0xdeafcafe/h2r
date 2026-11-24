//! Decide which DirectSound mix-bin operation preserves the requested sends.
//! The driver can submit a shorter descriptor of unique bins if the routing
//! stays the same. A duplicate whose gain differs can require a full rebind.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BinChanges {
    pub seen: u32,
    pub changed: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinAction {
    Rebind,
    Keep,
    Append,
}

impl BinChanges {
    /// `previous_gain` is the comparison scratch indexed by the **row**
    /// (22165a). The first occurrence is stored by bin number (221689).
    pub fn visit(&mut self, bin: u32, gain_changed: bool, duplicate_gain_agrees: bool) -> BinAction {
        let bit = 1u32.wrapping_shl(bin);
        let duplicate = self.seen & bit != 0;
        if gain_changed {
            if duplicate && (!duplicate_gain_agrees || self.changed & bit != 0) {
                return BinAction::Rebind;
            }
            self.changed |= bit;
        }
        self.seen |= bit;
        if duplicate { BinAction::Keep } else { BinAction::Append }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changing_one_duplicate_can_rebind_the_entire_stream() {
        let mut bins = BinChanges::default();
        assert_eq!(bins.visit(4, false, false), BinAction::Append);
        assert_eq!(bins.visit(4, true, false), BinAction::Rebind);
        assert_eq!(bins.visit(7, true, false), BinAction::Append);
        assert_eq!(bins.visit(7, true, true), BinAction::Rebind);
    }

    #[test]
    fn a_duplicate_can_change_a_single_gain_once() {
        let mut bins = BinChanges::default();
        assert_eq!(bins.visit(4, false, false), BinAction::Append);
        assert_eq!(bins.visit(4, true, true), BinAction::Keep);
        assert_eq!(bins.visit(4, false, false), BinAction::Keep);
        assert_eq!(bins.changed, 1 << 4);
    }
}
