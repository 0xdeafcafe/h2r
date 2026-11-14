//! Native runtime for the C11 lifter. Guest pointers remain 32-bit addresses;
//! versioned C types transport canonical state and explicit control transfers.
#![deny(unsafe_op_in_unsafe_fn)]
#![deny(unsafe_code)]

pub mod abi;
pub mod atomic;
mod continuation;
pub mod cpu;
pub mod dispatch;
pub mod events;
pub mod exec;
#[allow(unsafe_code)]
pub mod ffi;
pub mod guest;
mod handler;
pub mod kernel;
pub mod loader;
pub mod runner;
pub mod services;

pub use abi::{GuestFn, TransferAbi};
pub use cpu::CpuAbi;
pub use dispatch::Dispatch;
pub use exec::{Event, ExecCtx, Runner, Runtime};
pub use ffi::ExecCtxOpaque;
pub use guest::{Guest, GuestFault};
pub use loader::{LoadedImage, load_xbe};
