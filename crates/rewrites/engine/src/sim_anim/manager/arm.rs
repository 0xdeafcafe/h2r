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

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::guest::Permissions;

    // Synthetic graph view just after its graph-element callback: the
    // guest saves its argument and return PC, then enters the resource
    // lookup with the callback's answer still in ESI.
    const ENTRY: u32 = 0x1ddb40;
    const SELECTOR: u32 = 0x1ddb60;
    const RESOURCE: u32 = 0x1dd7c0;
    const RETURN_TO: u32 = 0x1ddb6a;
    const STACK: u32 = 0x10800;

    fn fixture() -> (Guest, CpuAbi) {
        let mut ram = Guest::new();
        ram.map(0x10000, 0x1000, 0, Permissions::RW).unwrap();
        ram.map(0x20000, 0x1000, 0, Permissions::RW).unwrap();
        ram.map_zeroed(ENTRY, lookup(ENTRY).unwrap().end - ENTRY, Permissions::RX)
            .unwrap();
        let cpu = CpuAbi {
            eip: SELECTOR,
            esp: STACK,
            eax: 0x123456,
            ebx: 0xabcdef01,
            ebp: 0x35,
            ..CpuAbi::default()
        };
        (ram, cpu)
    }

    #[test]
    fn callback_frame_is_predicted_without_touching_live_state() {
        let (mut ram, before) = fixture();
        let p = plan(lookup(ENTRY).unwrap(), &before, &mut ram, SELECTOR).unwrap();
        assert_eq!(ram.u32(STACK - 8).unwrap(), 0);
        assert_eq!(ram.u32(STACK - 4).unwrap(), 0);

        let after = CpuAbi {
            eip: RESOURCE,
            esp: STACK - 8,
            ecx: before.ebp,
            esi: before.eax,
            ..before
        };
        let transfer = TransferAbi::call(RESOURCE, RETURN_TO);
        // Write the original's frame through its second mapping.
        ram.set_u32(STACK + 0x10000 - 4, before.ebx).unwrap();
        ram.set_u32(STACK + 0x10000 - 8, RETURN_TO).unwrap();
        assert_eq!(p.difference(&after, &ram, transfer), None);
        let mut clobbered = after;
        clobbered.xmm[5][4] = 1;
        assert_eq!(
            p.difference(&clobbered, &ram, transfer).unwrap().what,
            "xmm5"
        );
        assert_eq!(
            p.difference(&after, &ram, TransferAbi::jump(RESOURCE))
                .unwrap()
                .what,
            "transfer"
        );
        ram.set_u8(STACK - 2, 0x76).unwrap();
        let m = p.difference(&after, &ram, transfer).unwrap();
        assert_eq!(
            (m.pc, m.what.as_str(), m.plan.as_str(), m.guest.as_str()),
            (SELECTOR, "physical RAM 0x000007fe", "0xcd", "0x76")
        );
    }

    #[test]
    fn fault_stop_and_yield_do_not_count_as_parity() {
        let (mut ram, cpu) = fixture();
        let counts = || {
            stats()
                .into_iter()
                .find(|(entry, _)| *entry == ENTRY)
                .map(|(_, s)| s)
                .unwrap_or_default()
        };
        let initial = counts();
        for transfer in [
            TransferAbi::fault(9),
            TransferAbi::stop(12),
            TransferAbi::resume(SELECTOR),
        ] {
            predict(&cpu, &mut ram, SELECTOR)
                .unwrap()
                .compare(&cpu, &ram, transfer);
        }
        predict(&cpu, &mut ram, SELECTOR).unwrap().guest_error();
        // The first push is mapped, the second falls off its mapping.
        let short = CpuAbi {
            esp: 0x10004,
            ..cpu
        };
        assert!(predict(&short, &mut ram, SELECTOR).is_none());
        assert_eq!(ram.u32(0x10000).unwrap(), 0);
        let after = counts();
        assert_eq!(after.interrupted, initial.interrupted + 4);
        assert_eq!(after.declined, initial.declined + 1);
        assert_eq!(after.matched, initial.matched);
        assert_eq!(after.differed, initial.differed);
    }
}
