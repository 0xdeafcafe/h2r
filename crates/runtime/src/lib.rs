//! Native runtime for the C11 lifter. Guest pointers remain 32-bit addresses;
//! versioned C types transport canonical state and explicit control transfers.
#![deny(unsafe_op_in_unsafe_fn)]
#![deny(unsafe_code)]

pub mod abi;
pub mod atomic;
mod continuation;
#[allow(unsafe_code)]
mod cow;
pub mod cpu;
#[allow(unsafe_code)]
pub mod direct;
pub mod dispatch;
mod entry_stats;
mod coverage;
pub mod events;
pub mod exec;
pub mod executor;
#[allow(unsafe_code)]
pub mod ffi;
mod frames;
#[allow(unsafe_code)]
pub mod generated;
pub mod guest;
mod handler;
pub mod interrupt;
pub mod kernel;
pub mod loader;
pub mod perf;
pub mod preempt;
pub mod runner;
pub mod services;
pub mod trap;
pub mod tsc;
pub mod watch;

pub use abi::{GuestFn, TransferAbi};
pub use continuation::{
    Continuation, ContinuationInfo, ContinuationLayout, LayoutError, LiveRecord, SlotRecord,
};
pub use cow::CopyKind;
pub use coverage::GuestCoverage;
pub use cpu::CpuAbi;
pub use dispatch::Dispatch;
pub use exec::{Event, ExecCtx, FaultReport, GuestFrame, Runner, Runtime};
pub use ffi::ExecCtxOpaque;
pub use generated::{H2GenEntry, H2GenModule, H2GenResume, RegisteredModule};
pub use guest::{Guest, GuestFault};
pub use interrupt::{CallConvention, GuestInterrupt, Raise};
pub use loader::{LoadedImage, load_xbe, load_xbe_with};
pub use trap::{HleCall, LineRec, TrapHit, TrapLog};
pub use watch::{DirtyRange, WatchError, WatchId};
