//! Async entries: Rust bodies for guest functions that call back into the
//! guest (docs/feature-engine.md "Calling code that is still recompiled").
//!
//! One host thread drives every guest thread, so a Rust body cannot make a
//! guest call as a plain call: the call has to unwind to the scheduler and
//! come back. An entry is therefore an `async` block. Each guest call is an
//! `.await` on [`Cx::call`]; whoever drives the future (the installer in
//! the game, [`testing::run`] in a test) sees the call, makes it, and polls
//! again with the callee's registers. The compiler writes the state machine
//! that hand-written continuation closures used to spell out.
//!
//! Guest memory is taken per closure ([`Cx::read`], [`Cx::write`]), so no
//! guard is held across an await, and the future is `Send + 'static`.
//!
//! ```
//! use engine::entry::{self, Cx, GuestCall, Ret};
//! use engine::testing::FakeGuest;
//!
//! // A thiscall body that asks the guest for a length and returns twice it.
//! let f = |cx: Cx| -> entry::EntryFuture {
//!     Box::pin(async move {
//!         let this = cx.entry().ecx;
//!         let r = cx.call(GuestCall::new(0x1000).ecx(this)).await?;
//!         Ok(Ret::new(r.eax * 2, 0))
//!     })
//! };
//! let mut g = FakeGuest::new();
//! g.map(0x8000, 0x1000);
//! let regs = entry::Regs { ecx: 7, esp: 0x8800, ..Default::default() };
//! let run = entry::testing::run(&f, g, regs, |call, _| {
//!     assert_eq!(call.ecx, Some(7));
//!     entry::Regs { eax: 21, ..Default::default() }
//! });
//! assert_eq!(run.result, Ok(Ret::new(42, 0)));
//! assert_eq!(run.calls.len(), 1);
//! ```

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use crate::memory::{GuestMemory, MemFault};

/// The integer registers at an entry, or after a guest call returns.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Regs {
    pub eax: u32,
    pub ecx: u32,
    pub edx: u32,
    pub ebx: u32,
    pub esp: u32,
    pub ebp: u32,
    pub esi: u32,
    pub edi: u32,
}

/// One call into the guest: the target, the registers to set (unset ones
/// keep their value), and the stack arguments, `args[0]` at `[esp+4]` when
/// the callee starts. The stack is put back after the call whatever the
/// callee pops, so caller- and callee-cleanup are the same here.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GuestCall {
    pub target: u32,
    pub eax: Option<u32>,
    pub ecx: Option<u32>,
    pub edx: Option<u32>,
    pub ebx: Option<u32>,
    pub esi: Option<u32>,
    pub edi: Option<u32>,
    pub args: Vec<u32>,
}

impl GuestCall {
    pub fn new(target: u32) -> Self {
        GuestCall {
            target,
            ..Default::default()
        }
    }
    pub fn eax(mut self, v: u32) -> Self {
        self.eax = Some(v);
        self
    }
    pub fn ecx(mut self, v: u32) -> Self {
        self.ecx = Some(v);
        self
    }
    pub fn edx(mut self, v: u32) -> Self {
        self.edx = Some(v);
        self
    }
    pub fn ebx(mut self, v: u32) -> Self {
        self.ebx = Some(v);
        self
    }
    pub fn esi(mut self, v: u32) -> Self {
        self.esi = Some(v);
        self
    }
    pub fn edi(mut self, v: u32) -> Self {
        self.edi = Some(v);
        self
    }
    /// Stack arguments, first argument first.
    pub fn args(mut self, args: impl IntoIterator<Item = u32>) -> Self {
        self.args = args.into_iter().collect();
        self
    }
}

/// How an entry returns: EAX, and the argument bytes it pops (`ret n`).
/// EBX, ESI, EDI, EBP and ESP are the caller's again, whatever the body
/// called.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ret {
    pub eax: u32,
    pub pop: u16,
}

impl Ret {
    pub fn new(eax: u32, pop: u16) -> Self {
        Ret { eax, pop }
    }
}

/// Why an entry did not return.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EntryError {
    /// A guest access from Rust faulted.
    Fault(MemFault),
    /// The body awaited something other than a guest call.
    Stalled,
}

impl From<MemFault> for EntryError {
    fn from(f: MemFault) -> Self {
        EntryError::Fault(f)
    }
}

impl std::fmt::Display for EntryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EntryError::Fault(m) => write!(
                f,
                "guest {} fault at {:#010x} ({} bytes)",
                if m.write { "write" } else { "read" },
                m.addr,
                m.len
            ),
            EntryError::Stalled => f.write_str("awaited something other than a guest call"),
        }
    }
}

pub type EntryResult = Result<Ret, EntryError>;
pub type EntryFuture = Pin<Box<dyn Future<Output = EntryResult> + Send>>;
/// An entry: called once per guest call with a fresh [`Cx`].
pub type EntryFn = dyn Fn(Cx) -> EntryFuture + Send + Sync;

/// Guest memory for a [`Cx`], taken for one closure at a time.
pub trait Mem: Send + Sync {
    fn with(&self, f: &mut dyn FnMut(&mut dyn GuestMemory));
}

impl<G: GuestMemory + Send> Mem for Mutex<G> {
    fn with(&self, f: &mut dyn FnMut(&mut dyn GuestMemory)) {
        let mut g = self.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut *g)
    }
}

impl<M: Mem + ?Sized> Mem for Arc<M> {
    fn with(&self, f: &mut dyn FnMut(&mut dyn GuestMemory)) {
        (**self).with(f)
    }
}

enum Slot {
    Idle,
    Want(GuestCall),
    Made,
    Done(Regs),
}

struct Inner {
    mem: Box<dyn Mem>,
    entry: Regs,
    slot: Mutex<Slot>,
}

/// What an entry body gets: the registers it was entered with, guest
/// memory per closure, and guest calls. Cheap to clone.
#[derive(Clone)]
pub struct Cx(Arc<Inner>);

impl Cx {
    pub fn new(mem: impl Mem + 'static, entry: Regs) -> Self {
        Cx(Arc::new(Inner {
            mem: Box::new(mem),
            entry,
            slot: Mutex::new(Slot::Idle),
        }))
    }
    /// The registers at entry (`esp` points at the return address).
    pub fn entry(&self) -> Regs {
        self.0.entry
    }
    /// The `n`-th dword stack argument at entry (0-based: `[esp+4+4n]`).
    pub fn arg(&self, n: u32) -> Result<u32, MemFault> {
        let at = self.0.entry.esp.wrapping_add(4 + 4 * n);
        self.read(|m| m.u32(at))
    }
    /// Runs `f` on guest memory. Never hold what it returns across an await
    /// if it borrows memory (it cannot: the borrow ends here).
    pub fn read<T>(&self, f: impl FnOnce(&dyn GuestMemory) -> T) -> T {
        let mut f = Some(f);
        let mut out = None;
        self.0.mem.with(&mut |m| {
            if let Some(f) = f.take() {
                out = Some(f(m));
            }
        });
        out.expect("Mem::with runs its closure")
    }
    /// Runs `f` on guest memory, mutably.
    pub fn write<T>(&self, f: impl FnOnce(&mut dyn GuestMemory) -> T) -> T {
        let mut f = Some(f);
        let mut out = None;
        self.0.mem.with(&mut |m| {
            if let Some(f) = f.take() {
                out = Some(f(m));
            }
        });
        out.expect("Mem::with runs its closure")
    }
    /// Calls into the guest; resolves to the registers when it returns.
    pub fn call(&self, call: GuestCall) -> CallFuture {
        CallFuture {
            cx: self.clone(),
            call: Some(call),
        }
    }

    fn slot(&self) -> std::sync::MutexGuard<'_, Slot> {
        self.0.slot.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The future of one [`Cx::call`].
pub struct CallFuture {
    cx: Cx,
    call: Option<GuestCall>,
}

impl Future for CallFuture {
    type Output = Result<Regs, EntryError>;
    fn poll(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
        if let Some(call) = self.call.take() {
            *self.cx.slot() = Slot::Want(call);
            return Poll::Pending;
        }
        let mut slot = self.cx.slot();
        match std::mem::replace(&mut *slot, Slot::Idle) {
            Slot::Done(r) => Poll::Ready(Ok(r)),
            other => {
                *slot = other;
                Poll::Pending
            }
        }
    }
}

/// What a driver does next.
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    /// Make this call, then [`resume`] with its registers and poll again.
    Call(GuestCall),
    /// The body finished.
    Done(EntryResult),
}

/// Polls the body once. A body pending on anything but a guest call is
/// [`EntryError::Stalled`] (there is nothing else to wait for).
pub fn poll(fut: &mut EntryFuture, cx: &Cx) -> Step {
    let mut task = Context::from_waker(Waker::noop());
    match fut.as_mut().poll(&mut task) {
        Poll::Ready(r) => Step::Done(r),
        Poll::Pending => {
            let mut slot = cx.slot();
            match std::mem::replace(&mut *slot, Slot::Made) {
                Slot::Want(c) => Step::Call(c),
                _ => {
                    *slot = Slot::Idle;
                    Step::Done(Err(EntryError::Stalled))
                }
            }
        }
    }
}

/// Hands a finished call's registers to the body that asked for it.
pub fn resume(cx: &Cx, regs: Regs) {
    *cx.slot() = Slot::Done(regs);
}

/// Running entries without the runtime: fake callees on a fake guest.
pub mod testing {
    use super::*;
    use crate::testing::FakeGuest;

    /// What [`run`] saw.
    pub struct Run {
        pub result: EntryResult,
        /// Every guest call, in order.
        pub calls: Vec<GuestCall>,
        pub guest: FakeGuest,
    }

    /// Runs `f` entered with `regs` on `guest`, answering each guest call
    /// with `callee` (which may change memory as the real callee would).
    pub fn run(
        f: &EntryFn,
        guest: FakeGuest,
        regs: Regs,
        mut callee: impl FnMut(&GuestCall, &mut dyn GuestMemory) -> Regs,
    ) -> Run {
        let mem = Arc::new(Mutex::new(guest));
        let cx = Cx::new(Arc::clone(&mem), regs);
        let mut fut = f(cx.clone());
        let mut calls = Vec::new();
        let result = loop {
            match poll(&mut fut, &cx) {
                Step::Done(r) => break r,
                Step::Call(c) => {
                    let out = {
                        let mut g = mem.lock().unwrap();
                        callee(&c, &mut *g)
                    };
                    calls.push(c);
                    resume(&cx, out);
                }
            }
        };
        drop(fut);
        drop(cx);
        let guest = Arc::try_unwrap(mem)
            .expect("the entry kept no memory handle")
            .into_inner()
            .unwrap();
        Run {
            result,
            calls,
            guest,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FakeGuest;

    fn boxed(
        f: impl Fn(Cx) -> Pin<Box<dyn Future<Output = EntryResult> + Send>> + Send + Sync,
    ) -> impl Fn(Cx) -> EntryFuture + Send + Sync {
        f
    }

    #[test]
    fn calls_are_made_in_order_with_their_results_and_memory_between() {
        let f = boxed(|cx| {
            Box::pin(async move {
                let a = cx.call(GuestCall::new(0x100).args([1, 2])).await?;
                cx.write(|m| m.set_u32(0x5000, a.eax))?;
                let b = cx.call(GuestCall::new(0x200).edi(a.eax)).await?;
                Ok(Ret::new(b.eax + cx.read(|m| m.u32(0x5000))?, 8))
            })
        });
        let mut g = FakeGuest::new();
        g.map(0x5000, 4);
        let run = testing::run(&f, g, Regs::default(), |c, m| match c.target {
            0x100 => Regs {
                eax: c.args[0] + c.args[1],
                ..Default::default()
            },
            0x200 => {
                // The callee sees what the body wrote before calling it.
                assert_eq!(m.u32(0x5000), Ok(3));
                Regs {
                    eax: c.edi.unwrap() * 10,
                    ..Default::default()
                }
            }
            _ => panic!("unexpected call"),
        });
        assert_eq!(run.result, Ok(Ret::new(33, 8)));
        assert_eq!(
            run.calls,
            [
                GuestCall::new(0x100).args([1, 2]),
                GuestCall::new(0x200).edi(3)
            ]
        );
    }

    #[test]
    fn a_fault_ends_the_body_and_no_further_call_is_made() {
        let f = boxed(|cx| {
            Box::pin(async move {
                cx.read(|m| m.u32(0x9000))?;
                cx.call(GuestCall::new(0x100)).await?;
                Ok(Ret::new(0, 0))
            })
        });
        let run = testing::run(&f, FakeGuest::new(), Regs::default(), |_, _| {
            panic!("no call after a fault")
        });
        assert!(matches!(
            run.result,
            Err(EntryError::Fault(MemFault { addr: 0x9000, .. }))
        ));
    }

    #[test]
    fn arguments_are_read_from_the_entry_stack() {
        let f = boxed(|cx| Box::pin(async move { Ok(Ret::new(cx.arg(1)?, 8)) }));
        let mut g = FakeGuest::new();
        g.put_u32(0x7000, 0xdead)
            .put_u32(0x7004, 1)
            .put_u32(0x7008, 2);
        let regs = Regs {
            esp: 0x7000,
            ..Default::default()
        };
        let run = testing::run(&f, g, regs, |_, _| unreachable!());
        assert_eq!(run.result, Ok(Ret::new(2, 8)));
    }

    #[test]
    fn awaiting_anything_else_is_a_stall_not_a_hang() {
        let f = boxed(|_| {
            Box::pin(async move {
                std::future::pending::<()>().await;
                Ok(Ret::new(0, 0))
            })
        });
        let run = testing::run(&f, FakeGuest::new(), Regs::default(), |_, _| unreachable!());
        assert_eq!(run.result, Err(EntryError::Stalled));
    }

    #[test]
    fn the_future_is_send_and_static() {
        fn is_send<T: Send + 'static>(_: &T) {}
        let cx = Cx::new(Mutex::new(FakeGuest::new()), Regs::default());
        let fut: EntryFuture = Box::pin(async move {
            cx.call(GuestCall::new(1)).await?;
            Ok(Ret::new(0, 0))
        });
        is_send(&fut);
    }
}
