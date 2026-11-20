//! One shadow check: a port and the lifted original from the same entry
//! state, each on its own copy of guest memory, compared.
//!
//! - The port runs on a copy-on-write view of the live Guest ([`View`]).
//!   The view's write watch gives the pages it stored to; the bytes it
//!   changed are those that differ from the live Guest, which nothing has
//!   written since the view was taken.
//! - The original runs on the IR interpreter over an [`Overlay`]: reads
//!   see the live Guest under the original's own writes, and writes stay in
//!   the overlay, keyed by physical address.
//!
//! The port's transfer says where to stop the original: at the port's
//! target with the port's stack pointer, stepping into calls on the way.
//! Every pass through that point is compared (registers, the bytes each
//! side changed, the kind of transfer); the first that matches in full is
//! a match. If the original returns out of the frame it was entered in
//! with no pass that matched, it is a mismatch, reported at the first
//! pass, or as "returned elsewhere" when there was none.

use std::collections::BTreeMap;

use ir::interp::{self, Cpu, Flow, MemFault};
use runtime::guest::{PAGE_SIZE, RAM_SIZE};
use runtime::watch::CONTIGUOUS_BASE;
use runtime::{CpuAbi, Guest, GuestFault, TransferAbi, WatchId, abi};

use crate::code::{self, Code};
use crate::cpu::{self, RegisterDiff};

/// A port body: the entry state in, the transfer out, over a Guest.
pub type PortFn<'a> = dyn FnMut(&mut CpuAbi, &mut Guest) -> Result<TransferAbi, GuestFault> + 'a;

/// What one check found.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// The original reached the port's transfer with the same registers
    /// and the same changed bytes, after `steps` instructions.
    Matched {
        steps: u64,
    },
    Mismatched(Box<Diff>),
    /// Not comparable here (the reason): the port faulted on its copy, the
    /// original reached code the interpreter cannot run, the step limit.
    Skipped(String),
}

/// How a port and the original differ.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Diff {
    /// `transfer`, `registers`, `memory` (the first that differs, in that
    /// order), `returned` (the original left the frame without reaching
    /// the port's transfer) or `nondeterministic` (the port gave another
    /// answer on the live Guest than on the copy).
    pub kind: &'static str,
    pub summary: String,
    /// The transfer each side made, when they differ.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transfer: Option<TransferDiff>,
    pub registers: Vec<RegisterDiff>,
    /// Runs of bytes that differ, at most [`MAX_RUNS`].
    pub memory: Vec<ByteRun>,
    /// Every byte that differs, including runs past [`MAX_RUNS`].
    pub memory_bytes: usize,
    /// Instructions the original ran.
    pub steps: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TransferDiff {
    pub port: String,
    pub original: String,
}

/// Consecutive bytes that differ: what was there, what each side left.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ByteRun {
    /// Folded physical address (`0x80000000 | physical`) of the first byte.
    pub address: String,
    /// Virtual addresses mapping it (at most two).
    pub virtual_addresses: Vec<String>,
    pub before: String,
    pub port: String,
    pub original: String,
}

/// Runs of differing bytes a [`Diff`] lists.
pub const MAX_RUNS: usize = 16;
/// Bytes a [`ByteRun`] shows.
const MAX_RUN_BYTES: usize = 64;

/// A copy-on-write view of the live Guest, kept between checks so its
/// page table is rebuilt only when the live layout changes.
#[derive(Default)]
pub struct View {
    guest: Option<(Guest, WatchId)>,
}

impl View {
    pub const fn new() -> Self {
        View { guest: None }
    }

    /// The view, now a copy of `live` with nothing marked written.
    fn refresh(&mut self, live: &Guest) -> (&mut Guest, WatchId) {
        match &mut self.guest {
            Some((view, _)) => {
                view.cow_refresh(live);
            }
            None => {
                let (mut view, _) = live.cow_snapshot();
                let id = view.subscribe_writes("shadow");
                view.watch_writes(id, CONTIGUOUS_BASE, RAM_SIZE as u32)
                    .expect("all of RAM is a valid watch range");
                self.guest = Some((view, id));
            }
        }
        let (view, id) = self.guest.as_mut().expect("just made");
        (view, *id)
    }

    /// Drop the copy (and its hold on live pages).
    pub fn clear(&mut self) {
        self.guest = None;
    }
}

/// The original's memory: the live Guest, read-only, under its own writes.
pub struct Overlay<'a> {
    live: &'a Guest,
    /// Folded physical address to byte.
    pub writes: BTreeMap<u32, u8>,
}

impl<'a> Overlay<'a> {
    pub fn new(live: &'a Guest) -> Self {
        Overlay {
            live,
            writes: BTreeMap::new(),
        }
    }

    /// Calls `each(offset, folded)` for each page-contiguous piece of
    /// `[addr, addr + len)`; the first unmapped address is the error.
    fn pieces(
        &self,
        addr: u32,
        len: usize,
        mut each: impl FnMut(usize, u32, usize),
    ) -> Result<(), u32> {
        let mut done = 0;
        while done < len {
            let va = addr.wrapping_add(done as u32);
            let folded = self.live.fold_address(va).ok_or(va)?;
            let n = (len - done).min((PAGE_SIZE - (va & (PAGE_SIZE - 1))) as usize);
            each(done, folded, n);
            done += n;
        }
        Ok(())
    }

    /// The bytes the original changed: written, and not what live holds.
    pub fn changed(&self) -> BTreeMap<u32, u8> {
        self.writes
            .iter()
            .filter(|&(&folded, &v)| live_byte(self.live, folded) != Some(v))
            .map(|(&a, &v)| (a, v))
            .collect()
    }
}

impl interp::Memory for Overlay<'_> {
    fn read(&mut self, addr: u32, buf: &mut [u8]) -> Result<(), MemFault> {
        self.live.read_bytes(addr, buf, 1).map_err(|f| MemFault {
            addr: f.addr,
            write: false,
        })?;
        if self.writes.is_empty() {
            return Ok(());
        }
        let writes = &self.writes;
        self.pieces(addr, buf.len(), |at, folded, n| {
            for (a, &v) in writes.range(folded..folded + n as u32) {
                buf[at + (a - folded) as usize] = v;
            }
        })
        .map_err(|va| MemFault {
            addr: va,
            write: false,
        })
    }

    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), MemFault> {
        let mut spans = Vec::with_capacity(2);
        self.pieces(addr, data.len(), |at, folded, n| {
            spans.push((at, folded, n))
        })
        .map_err(|va| MemFault {
            addr: va,
            write: true,
        })?;
        for (at, folded, n) in spans {
            for i in 0..n {
                self.writes.insert(folded + i as u32, data[at + i]);
            }
        }
        Ok(())
    }
}

fn live_byte(live: &Guest, folded: u32) -> Option<u8> {
    let physical = folded & !CONTIGUOUS_BASE;
    live.physical_page(physical)
        .map(|page| page[(physical & (PAGE_SIZE - 1)) as usize])
}

/// The bytes the port changed on `view`: on a page it stored to, and not
/// what live holds.
fn port_changes(view: &mut Guest, id: WatchId, live: &Guest) -> BTreeMap<u32, u8> {
    let mut out = BTreeMap::new();
    for r in view.take_dirty(id).unwrap_or_default() {
        for page in (r.addr..r.addr + r.len).step_by(PAGE_SIZE as usize) {
            let physical = page & !CONTIGUOUS_BASE;
            let (Some(after), Some(before)) =
                (view.physical_page(physical), live.physical_page(physical))
            else {
                continue;
            };
            for (i, (&a, &b)) in after.iter().zip(before).enumerate() {
                if a != b {
                    out.insert(page + i as u32, a);
                }
            }
        }
    }
    out
}

fn show(t: &TransferAbi, esp: u32) -> String {
    let kind = match t.kind {
        abi::CALL => "call",
        abi::JUMP => "jump",
        abi::RETURN => "return",
        abi::FAULT => "fault",
        abi::STOP => "stop",
        _ => "?",
    };
    match t.kind {
        abi::CALL => format!(
            "call {:#010x} returning to {:#010x}, esp {esp:#010x}",
            t.target_pc, t.resume_pc
        ),
        _ => format!("{kind} to {:#010x}, esp {esp:#010x}", t.target_pc),
    }
}

fn show_flow(flow: Option<&Flow>, cpu: &Cpu) -> String {
    let esp = cpu.gpr[4];
    match flow {
        Some(Flow::Call {
            target,
            continuation,
        }) => {
            format!("call {target:#010x} returning to {continuation:#010x}, esp {esp:#010x}")
        }
        Some(Flow::Return { target }) => format!("return to {target:#010x}, esp {esp:#010x}"),
        _ => format!("jump to {:#010x}, esp {esp:#010x}", cpu.eip),
    }
}

/// Whether the original's last step is the kind of transfer the port made.
fn same_kind(t: &TransferAbi, flow: Option<&Flow>) -> bool {
    match t.kind {
        abi::CALL => {
            matches!(flow, Some(Flow::Call { continuation, .. }) if *continuation == t.resume_pc)
        }
        abi::RETURN => matches!(flow, Some(Flow::Return { .. })),
        _ => true,
    }
}

fn byte_runs(
    live: &Guest,
    port: &BTreeMap<u32, u8>,
    original: &BTreeMap<u32, u8>,
) -> (Vec<ByteRun>, usize) {
    let mut addresses: Vec<u32> = port
        .keys()
        .chain(original.keys())
        .copied()
        .filter(|a| port.get(a) != original.get(a))
        .collect();
    addresses.sort_unstable();
    addresses.dedup();
    let total = addresses.len();
    let mut runs: Vec<Vec<u32>> = Vec::new();
    for a in addresses {
        let full = runs.len() == MAX_RUNS;
        match runs.last_mut() {
            Some(run) if *run.last().unwrap() + 1 == a && run.len() < MAX_RUN_BYTES => run.push(a),
            _ if full => break,
            _ => runs.push(vec![a]),
        }
    }
    let hex = |bytes: Vec<u8>| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
    let runs = runs
        .into_iter()
        .map(|run| {
            let before: Vec<u8> = run
                .iter()
                .map(|&a| live_byte(live, a).unwrap_or(0))
                .collect();
            let side = |m: &BTreeMap<u32, u8>| -> Vec<u8> {
                run.iter()
                    .zip(&before)
                    .map(|(a, &b)| m.get(a).copied().unwrap_or(b))
                    .collect()
            };
            ByteRun {
                address: format!("{:#010x}", run[0]),
                virtual_addresses: live
                    .virtual_of(run[0] & !CONTIGUOUS_BASE)
                    .into_iter()
                    .take(2)
                    .map(|v| format!("{v:#010x}"))
                    .collect(),
                port: hex(side(port)),
                original: hex(side(original)),
                before: hex(before),
            }
        })
        .collect();
    (runs, total)
}

/// Compare at one pass of the original through the port's transfer
/// point: `None` when everything matches.
#[allow(clippy::too_many_arguments)]
fn compare(
    live: &Guest,
    t: &TransferAbi,
    port_cpu: &Cpu,
    port_bytes: &BTreeMap<u32, u8>,
    original: &Cpu,
    last: Option<&Flow>,
    overlay: &Overlay<'_>,
    steps: u64,
) -> Option<Diff> {
    let transfer = (!same_kind(t, last)).then(|| TransferDiff {
        port: show(t, port_cpu.gpr[4]),
        original: show_flow(last, original),
    });
    let registers = cpu::diff(port_cpu, original);
    let (memory, memory_bytes) = byte_runs(live, port_bytes, &overlay.changed());
    let (kind, summary) = if let Some(t) = &transfer {
        (
            "transfer",
            format!("port: {}; original: {}", t.port, t.original),
        )
    } else if let Some(r) = registers.first() {
        let more = registers.len() - 1;
        (
            "registers",
            format!(
                "{} port {} original {} (+{more} more)",
                r.name, r.port, r.original
            ),
        )
    } else if let Some(r) = memory.first() {
        (
            "memory",
            format!("{memory_bytes} bytes differ, first at {}", r.address),
        )
    } else {
        return None;
    };
    Some(Diff {
        kind,
        summary,
        transfer,
        registers,
        memory,
        memory_bytes,
        steps,
    })
}

/// Run `port` on a copy of `live` and the lifted original from the same
/// `entry`, and compare. `pc` is where the call entered (the function or a
/// resume); the original starts there whatever `entry.eip` holds. Also returns the port's state and transfer from
/// the copy, for comparing with the same port on the live Guest.
pub fn check(
    code: &mut (impl Code + ?Sized),
    view: &mut View,
    live: &Guest,
    entry: &CpuAbi,
    pc: u32,
    limit: u64,
    port: &mut PortFn<'_>,
) -> (Outcome, Option<(CpuAbi, TransferAbi)>) {
    let (copy, id) = view.refresh(live);
    let mut port_abi = *entry;
    let t = match port(&mut port_abi, copy) {
        Ok(t) => t,
        Err(f) => {
            return (
                Outcome::Skipped(format!("the port faulted on the copy at {:#010x}", f.addr)),
                None,
            );
        }
    };
    let ran = Some((port_abi, t));
    if !matches!(t.kind, abi::CALL | abi::JUMP | abi::RETURN) {
        return (
            Outcome::Skipped(format!("the port ended with transfer kind {}", t.kind)),
            ran,
        );
    }
    let port_bytes = port_changes(copy, id, live);
    let mut port_cpu = cpu::to_interp(&port_abi);
    port_cpu.eip = t.target_pc;

    let mut original = cpu::to_interp(entry);
    original.eip = pc;
    let mut overlay = Overlay::new(live);
    let mut last: Option<Flow> = None;
    let mut first: Option<Diff> = None;
    for steps in 0..=limit {
        if original.eip == t.target_pc && original.gpr[4] == port_cpu.gpr[4] {
            match compare(
                live,
                &t,
                &port_cpu,
                &port_bytes,
                &original,
                last.as_ref(),
                &overlay,
                steps,
            ) {
                None => return (Outcome::Matched { steps }, ran),
                Some(d) => {
                    first.get_or_insert(d);
                }
            }
        }
        if matches!(last, Some(Flow::Return { .. })) && original.gpr[4] > entry.esp {
            let diff = first.unwrap_or_else(|| Diff {
                kind: "returned",
                summary: format!(
                    "the original left the frame ({}) without reaching the port's {}",
                    show_flow(last.as_ref(), &original),
                    show(&t, port_cpu.gpr[4]),
                ),
                transfer: Some(TransferDiff {
                    port: show(&t, port_cpu.gpr[4]),
                    original: show_flow(last.as_ref(), &original),
                }),
                registers: Vec::new(),
                memory: Vec::new(),
                memory_bytes: 0,
                steps,
            });
            return (Outcome::Mismatched(Box::new(diff)), ran);
        }
        if steps == limit {
            break;
        }
        match code::step(code, &mut original, &mut overlay) {
            Ok(flow) => last = Some(flow),
            Err(e) => {
                return match first {
                    Some(d) => (Outcome::Mismatched(Box::new(d)), ran),
                    None => (Outcome::Skipped(format!("original: {e}")), ran),
                };
            }
        }
    }
    match first {
        Some(d) => (Outcome::Mismatched(Box::new(d)), ran),
        None => (
            Outcome::Skipped(format!(
                "original: step limit ({limit}) at {:#010x}",
                original.eip
            )),
            ran,
        ),
    }
}
