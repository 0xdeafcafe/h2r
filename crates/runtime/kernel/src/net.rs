//! XNet's PHY boundary for a console with no network device.
//!
//! The NIC is not modelled. What the title observes of it is two kernel
//! calls, decoded from the retail image (see README.md "PHY / link state"):
//!
//! * `PhyInitialize(BOOLEAN forceReset, PVOID reserved)`, ordinal 253,
//!   returns an NTSTATUS. Both observed call sites (0x36c4a1 and 0x3d6a7d)
//!   push two zeros, and 0x3d6a85 branches on a negative result to abort
//!   adapter bring-up; 0x36c4a1 ignores the result. Initializing the PHY
//!   does not need a cable, so it succeeds.
//! * `PhyGetLinkState(BOOLEAN update)`, ordinal 252, returns a bit mask.
//!   Bit 0 is link-up (0x36c4b8..0x36c4c0 returns the mask only when bit 0
//!   is set, 0x3dabcc tests it directly), bit 2 selects the 100 Mbps speed
//!   at 0x3d5f88 and bit 3 selects full duplex at 0x3d5f96. 0 is
//!   "cable unplugged", which XNet treats as no network.
use crate::{Action, Core, abi::Args};

/// Link mask bits the title tests.
pub const LINK_UP: u32 = 0x01;
pub const LINK_100_MBPS: u32 = 0x04;
pub const LINK_FULL_DUPLEX: u32 = 0x08;
/// Every bit the title interprets; others are never reported.
pub const LINK_MASK: u32 = LINK_UP | LINK_100_MBPS | LINK_FULL_DUPLEX;

impl Core {
    pub(crate) fn phy_call(&mut self, a: &Args) -> Action {
        if a.export.ordinal == crate::ord::PHY_INITIALIZE {
            // The second parameter is zero at every observed site; a value
            // would be an unverified contract.
            if a.get(1) != 0 {
                return Action::Stop("PhyInitialize with a nonzero second parameter unverified");
            }
            self.phy_ready = true;
            return Action::status(crate::status::SUCCESS);
        }
        // Before PhyInitialize the PHY has not been probed: no link.
        let state = if self.phy_ready {
            self.console.phy_link & LINK_MASK
        } else {
            0
        };
        // Without a cable, speed and duplex are meaningless.
        Action::value(if state & LINK_UP != 0 { state } else { 0 })
    }
}
