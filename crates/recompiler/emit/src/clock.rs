//! The block clock's costs (docs/runtime/block-clock-imprecise-faults.md).
//!
//! With the block clock (module feature `H2GEN_FEATURE_BLOCK_CLOCK`,
//! `H2_ACCESS_COUNT=block`) the scheduler's work count is not the number
//! of guest memory accesses but a cost the plan assigns to two places in
//! the IR:
//!
//! - entering a handler at its function entry: [`entry_cost`];
//! - each trip round a loop, at its header ([`crate::backedge`]):
//!   [`header_cost`], charged once per trip that runs the header, after
//!   the header's poll (a trip that leaves at the poll charges nothing,
//!   and the resumed trip charges once).
//!
//! A cost is the number of guest instructions in that block, at least one.
//! Both depend only on the lifted IR's block structure, which no IR pass
//! changes, so any backend that polls the same headers charges the same
//! amounts at the same points, however it performs memory accesses or
//! calls. That is what lets two backends, or a backend and a native port
//! charging a declared constant, share a lockstep session.

use ir::IrFunction;

/// The cost of block `start` of `f`: its guest instructions, at least 1
/// (1 for a start that is not a block).
pub fn block_cost(f: &IrFunction, start: u32) -> u32 {
    f.blocks.get(&start).map_or(1, |b| (b.insns.len() as u32).max(1))
}

/// What entering `f` at its function entry charges.
pub fn entry_cost(f: &IrFunction) -> u32 {
    block_cost(f, f.entry)
}

/// What one trip through loop header `header` of `f` charges.
pub fn header_cost(f: &IrFunction, header: u32) -> u32 {
    block_cost(f, header)
}
