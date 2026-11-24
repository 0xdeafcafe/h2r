//! Compare one manager segment before a callee can change its inputs or
//! outputs. A prediction belongs to the original invocation on the host
//! stack, so a different guest thread or a nested call cannot consume it.
use super::{Body, Bus, evaluate, lookup};
use runtime::{CpuAbi, Guest, TransferAbi, abi};
use std::collections::BTreeMap;
use std::sync::Mutex;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Mismatch {
    /// The entry or return selector at which the segment began.
    pub pc: u32,
    pub what: String,
    pub plan: String,
    pub guest: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Stats {
    pub matched: u64,
    pub differed: u64,
    pub declined: u64,
    /// The guest faulted, stopped, or yielded before its outgoing boundary.
    pub interrupted: u64,
    pub first: Option<Mismatch>,
}

static STATS: Mutex<BTreeMap<u32, Stats>> = Mutex::new(BTreeMap::new());

fn record(entry: u32, f: impl FnOnce(&mut Stats)) {
    f(STATS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(entry)
        .or_default());
}

/// Segment counts grouped by function, in entry address order.
pub fn stats() -> Vec<(u32, Stats)> {
    STATS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .map(|(&k, v)| (k, v.clone()))
        .collect()
}

#[must_use]
pub struct Prediction {
    body: &'static Body,
    pc: u32,
    cpu: CpuAbi,
    transfer: TransferAbi,
    writes: Vec<(u32, u8)>,
}

fn plan(body: &'static Body, cpu: &CpuAbi, ram: &mut Guest, pc: u32) -> Option<Prediction> {
    let mut bus = Bus::for_segment(ram, cpu, pc, body.entry..body.end, body.sites)?;
    let mut cpu = *cpu;
    let transfer = evaluate(body, &mut cpu, &mut bus, pc).ok()?;
    // Both generated tiers spill the outgoing target to EIP at this
    // boundary. The Rust body delegates the transfer to the dispatcher.
    cpu.eip = transfer.target_pc;
    Some(Prediction {
        body,
        pc,
        cpu,
        transfer,
        writes: bus.final_bytes(),
    })
}

/// Evaluate the Rust segment on a CPU copy and a private store log. A late
/// fault discards all predicted stores, including already prepared pushes.
pub fn predict(cpu: &CpuAbi, ram: &mut Guest, pc: u32) -> Option<Prediction> {
    let body = lookup(pc)?;
    let prediction = plan(body, cpu, ram, pc);
    if prediction.is_none() {
        record(body.entry, |s| s.declined += 1);
    }
    prediction
}

impl Prediction {
    /// The original could not run to a transfer.
    pub fn guest_error(self) {
        record(self.body.entry, |s| s.interrupted += 1);
    }

    /// Inspect the state at the generated transfer. The write set is the
    /// Rust plan's destinations; the lifted differential checks all RAM.
    pub fn compare(self, cpu: &CpuAbi, ram: &Guest, transfer: TransferAbi) {
        if matches!(transfer.kind, abi::FAULT | abi::STOP)
            || (transfer.kind == abi::JUMP
                && transfer.route == abi::RESUME
                && (self.body.entry..self.body.end).contains(&transfer.target_pc))
        {
            self.guest_error();
            return;
        }
        let difference = self.difference(cpu, ram, transfer);
        record(self.body.entry, |s| match difference {
            Some(m) => {
                s.differed += 1;
                s.first.get_or_insert(m);
            }
            None => s.matched += 1,
        });
    }

    fn difference(&self, cpu: &CpuAbi, ram: &Guest, transfer: TransferAbi) -> Option<Mismatch> {
        // Only build diagnostics on a mismatch, outside the statistics lock.
        macro_rules! check {
            ($name:expr, $want:expr, $got:expr) => {
                if $want != $got {
                    return Some(Mismatch {
                        pc: self.pc,
                        what: $name.into(),
                        plan: format!("{:x?}", $want),
                        guest: format!("{:x?}", $got),
                    });
                }
            };
        }
        check!("transfer", self.transfer, transfer);
        let want = &self.cpu;
        macro_rules! fields {
            ($($field:ident),+) => {
                $(check!(stringify!($field), want.$field, cpu.$field);)+
            };
        }
        fields!(
            abi_version,
            struct_size,
            eax,
            ecx,
            edx,
            ebx,
            esp,
            ebp,
            esi,
            edi,
            eip,
            fs_base,
            eflags_defined,
            mxcsr
        );
        // An undefined flag has no architectural value, but the set of
        // defined flags itself must agree.
        check!(
            "eflags",
            want.eflags & want.eflags_defined,
            cpu.eflags & cpu.eflags_defined
        );
        check!("x87", want.x87, cpu.x87);
        for (i, (expected, actual)) in want.xmm.iter().zip(&cpu.xmm).enumerate() {
            check!(
                format!("xmm{i}"),
                u128::from_le_bytes(*expected),
                u128::from_le_bytes(*actual)
            );
        }
        let physical = ram.physical_ram();
        for &(addr, value) in &self.writes {
            let actual = physical.get(addr as usize).copied();
            if actual != Some(value) {
                return Some(Mismatch {
                    pc: self.pc,
                    what: format!("physical RAM {addr:#010x}"),
                    plan: format!("{value:#04x}"),
                    guest: actual.map_or_else(|| "unmapped".into(), |b| format!("{b:#04x}")),
                });
            }
        }
        None
    }
}
