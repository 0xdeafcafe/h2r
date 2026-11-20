//! HaloScript action-test words in the local player-control globals.
//! The input gather at 0x187b30 updates them before gameplay uses the packet.
pub const PLAYER_CONTROL: u32 = 0x004e_d284;
/// Sticky inputs recorded since the action-test reset.
pub const LATCHED: u32 = 4;
/// Inputs claimed by an evaluator.
pub const CAPTURE: u32 = 8;
/// Claims carried past reset until the mapped button is released.
pub const UNTIL_RELEASE: u32 = 12;
/// The action/reload bit in each of the three words.
pub const ACTION: u32 = 1;
