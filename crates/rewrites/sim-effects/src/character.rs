//! A `char` part (`0x17a8f1`): an AI character effect on the effect's
//! object.
//!
//! 1. The part's tag `-1` returns, `EAX` = the group.
//! 2. The location's node word (`[ECX+2]`): `0xffff` is node `-1`; another
//!    value with its top bit set returns, `EAX` = the group's top half and
//!    the word; else the node is the word.
//! 3. `0x1469f0([effect+0x64])`, seconds to ticks (`ret 4`).
//! 4. `0x1ca290(ticks, [effect+0x48], node, [part+0x14], [part+0x18],
//!    location+8)` with `ESI` = the tag (`ret 0x18`); its `EAX` returns.
//!
//! No draw in the arm. The audit compares step 4's call ([`Call`]).


/// The leaves.
pub const TICKS: u32 = 0x0014_69f0;
pub const START: u32 = 0x001c_a290;
/// Where the switch calls `0x1ca290` from.
pub const START_RETURN: u32 = 0x0017_a942;

/// Everything the arm reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Inputs {
    /// The switch's entry `ESP`.
    pub entry: u32,
    pub effect: u32,
    pub group: u32,
    pub tag: u32,
    /// `[effect+0x48]` and `[effect+0x64]`.
    pub object: u32,
    pub seconds: u32,
    /// `ECX` at entry and its `+2` word (read only when the tag is not -1).
    pub location: u32,
    pub node: u16,
    /// The part's `+0x14` and `+0x18`.
    pub a: u32,
    pub b: u32,
    /// `EAX` at entry (`EDI` through the arm).
    pub source: u32,
}

/// What the arm does with a part.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plan {
    /// Return at once with this `EAX`.
    Return(u32),
    /// Make the two calls with this node.
    Start { node: u32 },
}

impl Inputs {
    pub fn plan(&self) -> Plan {
        if self.tag == u32::MAX {
            return Plan::Return(self.group);
        }
        if self.node == 0xffff {
            return Plan::Start { node: u32::MAX };
        }
        if self.node & 0x8000 != 0 {
            return Plan::Return((self.group & 0xffff_0000) | u32::from(self.node));
        }
        Plan::Start { node: u32::from(self.node) }
    }

    /// Step 3's registers (`EAX`, `ECX`, `EDX`): `[effect+0x48]`,
    /// `[effect+0x64]`, the part's `+0x14`.
    pub fn ticks_regs(&self) -> [u32; 3] {
        [self.object, self.seconds, self.a]
    }

    /// Step 4's arguments after the ticks.
    pub fn start_args(&self, node: u32) -> [u32; 5] {
        [self.object, node, self.a, self.b, self.location.wrapping_add(8)]
    }
}

/// The arm's prediction: the plan, and for a start the call the guest's
/// arm makes (`ESI`, `EBX`, `EDI` and the arguments after the ticks).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Call {
    pub entry: u32,
    pub plan: Plan,
    pub esi: u32,
    pub ebx: u32,
    pub edi: u32,
    pub args: [u32; 5],
}

impl Call {
    pub fn predict(i: &Inputs) -> Call {
        let plan = i.plan();
        let node = match plan {
            Plan::Start { node } => node,
            Plan::Return(_) => 0,
        };
        Call { entry: i.entry, plan, esi: i.tag, ebx: i.effect, edi: i.source, args: i.start_args(node) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs() -> Inputs {
        Inputs {
            entry: 0x0030_0000,
            effect: 0x0061_0000,
            group: 0x6368_6172,
            tag: 0xe100_0003,
            object: 0x0004_0007,
            seconds: 2.0f32.to_bits(),
            location: 0x0060_0000,
            node: 5,
            a: 0x14,
            b: 0x18,
            source: 0x0062_0000,
        }
    }

    /// The node word's three cases and the missing tag, as `0x17a8f1` to
    /// `0x17a91f` branch.
    #[test]
    fn the_plan_follows_retails_branches() {
        let i = inputs();
        assert_eq!(i.plan(), Plan::Start { node: 5 });
        assert_eq!(Inputs { node: 0xffff, ..i }.plan(), Plan::Start { node: u32::MAX });
        assert_eq!(Inputs { node: 0x8002, ..i }.plan(), Plan::Return(0x6368_8002));
        assert_eq!(Inputs { tag: u32::MAX, node: 0x8002, ..i }.plan(), Plan::Return(0x6368_6172));
        assert_eq!(i.start_args(5), [0x0004_0007, 5, 0x14, 0x18, 0x0060_0008]);
        assert_eq!(i.ticks_regs(), [0x0004_0007, 2.0f32.to_bits(), 0x14]);
    }
}
