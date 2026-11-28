//! Seeded differential test: every adapter in `guest_layout::abi::OVERRIDES`
//! against the original x86, lifted by `crates/recompiler/lift` and run on the IR
//! interpreter (`ir::interp`), from identical 64 MB guest states.
//!
//! Behind the `differential` feature, which pulls in `ir`, `lift`,
//! `analyze` and `xbe`. It uses `lift::Lifter::{new, lift}`,
//! `lift::harness::load_image` and `ir::interp::{step, Cpu, FlatMemory}`.
//!
//! Needs the user's XBE, never in the repo: `H2_XBE=/abs/path/default.xbe`,
//! or `disc/default.xbe` in the checkout. Without one it prints SKIPPED and
//! passes. Run optimized, it interprets millions of instructions:
//!
//! ```sh
//! H2_XBE=/abs/path/default.xbe cargo test -p engine --release \
//!     --features differential --test it -- differential:: --skip _differential:: --nocapture
//! ```
//!
//! `H2_DIFF_SEEDS=1,7,99` and `H2_DIFF_SCALE=2` override the defaults.
//!
//! **What "agree" means** (the comparison masks):
//! - Both return normally: same return target (the planted
//!   [`HARNESS_EXIT`]) and ESP; same EAX unless the adapter's `output` is
//!   `None`; same value in every register not in the adapter's `clobbers`
//!   list (the adapters leave clobbered registers untouched, the originals
//!   don't, and no caller can depend on them). Flags, x87 and SSE state are
//!   not compared: none of these functions defines them as outputs.
//! - Both fault: same fault address and direction. In either case all of
//!   memory must match except the dead stack, the `DEAD_STACK` bytes below
//!   the ESP at entry, where only the original builds a frame.
//! - Anything else (one side faults, or a mismatch) is a disagreement and
//!   fails the test.
//!
//! **Views.** For the lookups a view answers (`datum_try_and_get`,
//! `datum_get_absolute`, `tag_get_checked`), the view is asked the same
//! question on the memory the original ran on and must give its answer
//! (`guest_layout::view`), so the views are checked against real layouts.
//!
//! **The game's path.** Every call is also made the way the game makes it
//! with `impl.engine` on: through `guest_layout::runtime_glue::invoke` (the body
//! the `engine-leaves` feature installs) on a `runtime::Guest` holding the
//! same 64 MB. That side must agree with the direct adapter exactly: same
//! outcome, every register, every byte, and the same fault address and
//! direction (the runtime's own fault, which `invoke` reports).

#![cfg(feature = "differential")]

use std::collections::BTreeMap;

use guest_layout::abi::{self, Output, Reg, Regs};
use guest_layout::memory::{GuestMemory, MemFault};
use ir::IrInsn;
use ir::interp::{self, Cpu, Exit, FlatMemory};

mod common;
use common::{IMAGE_SHA256, sha256, xbe_path};
/// Seeds used for the recorded runs (see `docs/engine/README.md`).
const DEFAULT_SEEDS: &[u64] = &[1, 7, 99, 2026, 31337];
const RAM: usize = 64 << 20;
const HARNESS_EXIT: u32 = 0xFFFF_FFFC;
const STACK_TOP: u32 = 0x03ff_0000;
const DEAD_STACK: u32 = 0x2000;
const STEP_LIMIT: u64 = 20_000_000;

// ---- the two sides -------------------------------------------------------

/// The engine's view of the interpreter's memory.
struct Eng<'a>(&'a mut FlatMemory);

fn span(len: usize, addr: u32, n: usize) -> Option<std::ops::Range<usize>> {
    let end = addr as u64 + n as u64;
    (end <= len as u64).then_some(addr as usize..end as usize)
}

impl GuestMemory for Eng<'_> {
    fn read(&self, addr: u32, buf: &mut [u8]) -> Result<(), MemFault> {
        let r = span(self.0.bytes.len(), addr, buf.len()).ok_or(MemFault {
            addr,
            len: buf.len() as u32,
            write: false,
        })?;
        buf.copy_from_slice(&self.0.bytes[r]);
        Ok(())
    }
    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), MemFault> {
        let r = span(self.0.bytes.len(), addr, data.len()).ok_or(MemFault {
            addr,
            len: data.len() as u32,
            write: true,
        })?;
        self.0.bytes[r].copy_from_slice(data);
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Outcome {
    Returned(Regs),
    /// Fault address and whether it was a write.
    Fault(u32, bool),
    /// The interpreter stopped for some other reason (unsupported, ...).
    Stopped(String),
}

struct Original<'a> {
    lifter: &'a lift::Lifter<'a>,
    cache: BTreeMap<u32, IrInsn>,
    steps: u64,
}

impl Original<'_> {
    fn run(&mut self, mem: &mut FlatMemory, r: Regs, entry: u32) -> Outcome {
        let mut cpu = Cpu {
            gpr: [r.eax, r.ecx, r.edx, r.ebx, r.esp, r.ebp, r.esi, r.edi],
            fs_base: RAM as u32,
            eip: entry,
            ..Cpu::default()
        };
        let mut pc = entry;
        let mut steps = 0u64;
        while pc != HARNESS_EXIT {
            if steps == STEP_LIMIT {
                return Outcome::Stopped("step limit".into());
            }
            if !self.cache.contains_key(&pc) {
                if let Some(f) = self.lifter.lift(pc) {
                    for insn in f.blocks.into_values().flat_map(|b| b.insns) {
                        self.cache.entry(insn.pc).or_insert(insn);
                    }
                }
                if !self.cache.contains_key(&pc) {
                    return Outcome::Stopped(format!("not lifted: {pc:#x}"));
                }
            }
            steps += 1;
            match interp::step(&mut cpu, mem, &self.cache[&pc]) {
                Ok(flow) => pc = flow.target(),
                Err(Exit::Memory { fault, .. }) => {
                    self.steps += steps;
                    return Outcome::Fault(fault.addr, fault.write);
                }
                Err(e) => return Outcome::Stopped(format!("{e:?}")),
            }
        }
        self.steps += steps;
        let g = cpu.gpr;
        Outcome::Returned(Regs {
            eax: g[0],
            ecx: g[1],
            edx: g[2],
            ebx: g[3],
            esp: g[4],
            ebp: g[5],
            esi: g[6],
            edi: g[7],
        })
    }
}

// ---- comparison -----------------------------------------------------------

#[derive(Default, Debug, Clone, Copy)]
struct Tally {
    returned: u64,
    faulted: u64,
    disagreed: u64,
    /// Calls whose game path (runtime `invoke`) disagreed with the adapter.
    game_disagreed: u64,
    /// Calls also answered by a view (`guest_layout::view`), and disagreements.
    views: u64,
    view_disagreed: u64,
}

guest_layout::record! {
    /// Any element: the views' lookups use the array's own element size.
    struct Element: 0x4 {}
}

struct Harness<'a> {
    original: Original<'a>,
    /// Memory the original runs on.
    a: FlatMemory,
    /// Memory the Rust runs on. Kept equal to `a` between calls.
    b: FlatMemory,
    /// The runtime's guest, for the game's path; kept equal to `b`. RAM is
    /// mapped at 0 in one piece starting at physical `c_phys`.
    c: runtime::Guest,
    c_phys: usize,
    tally: BTreeMap<&'static str, Tally>,
    failures: Vec<String>,
}

fn reg(r: &Regs, x: Reg) -> u32 {
    match x {
        Reg::Eax => r.eax,
        Reg::Ecx => r.ecx,
        Reg::Edx => r.edx,
        Reg::Ebx => r.ebx,
        Reg::Ebp => r.ebp,
        Reg::Esi => r.esi,
        Reg::Edi => r.edi,
    }
}

fn runtime_guest(image: &FlatMemory) -> (runtime::Guest, usize) {
    let mut g = runtime::Guest::new();
    g.map_zeroed(0, RAM as u32, runtime::guest::Permissions::RW)
        .expect("64 MB at 0 in the runtime guest");
    let phys = g.mappings()[0].physical_start as usize;
    g.write_bytes(0, &image.bytes, 1)
        .expect("image into the runtime guest");
    (g, phys)
}

impl Harness<'_> {
    fn poke(&mut self, addr: u32, data: &[u8]) {
        let r = addr as usize..addr as usize + data.len();
        self.a.bytes[r.clone()].copy_from_slice(data);
        self.b.bytes[r].copy_from_slice(data);
        self.c
            .write_bytes(addr, data, 1)
            .expect("runtime guest poke");
    }
    /// The views' answer to the same question as a lookup the original
    /// just answered, from the memory it ran on: `None` when the function
    /// has no view, `Some(None)` when they agree, `Some(Some(why))` when not.
    fn view_check(&mut self, name: &str, r: &Regs, theirs: &Outcome) -> Option<Option<String>> {
        use guest_layout::view::{Datums, Tags};
        let m = Eng(&mut self.a);
        let ours = match name {
            "datum_try_and_get" => Datums::<Element>::at(r.esi)
                .get(&m, guest_layout::DatumHandle(r.eax))
                .map(|e| e.map(|e| e.addr)),
            "datum_get_absolute" => Datums::<Element>::at(r.esi)
                .get_absolute(&m, r.edx as i32)
                .map(|e| e.map(|e| e.addr)),
            "tag_get_checked" => Tags.get(
                &m,
                guest_layout::DatumHandle(r.ecx),
                guest_layout::tags::GroupTag(r.ebx),
            ),
            _ => return None,
        };
        Some(match (theirs, ours) {
            (Outcome::Returned(x), Ok(v)) if x.eax == v.unwrap_or(0) => None,
            (Outcome::Fault(..), Err(_)) => None,
            (t, o) => Some(format!("original {t:?}, view {o:?}")),
        })
    }

    fn c_ram(&self) -> &[u8] {
        &self.c.physical_ram()[self.c_phys..self.c_phys + RAM]
    }
    fn poke32(&mut self, addr: u32, v: u32) {
        self.poke(addr, &v.to_le_bytes());
    }
    fn peek32(&self, addr: u32) -> u32 {
        u32::from_le_bytes(
            self.a.bytes[addr as usize..addr as usize + 4]
                .try_into()
                .unwrap(),
        )
    }

    /// Call `addr` both ways from the same state and compare. Returns the
    /// original's EAX, or `None` if it did not return.
    fn call(&mut self, addr: u32, r: Regs, args: &[u32], label: &str) -> Option<u32> {
        self.call_below(STACK_TOP, addr, r, args, label)
    }

    /// [`call`](Self::call) with `args` pushed just below `top`. With `top`
    /// at the end of RAM, every stack argument after `args` is unmapped.
    fn call_below(
        &mut self,
        top: u32,
        addr: u32,
        mut r: Regs,
        args: &[u32],
        label: &str,
    ) -> Option<u32> {
        let o = abi::find(addr).expect("no adapter at that address");
        let mut sp = top;
        for a in args.iter().rev() {
            sp -= 4;
            self.poke32(sp, *a);
        }
        sp -= 4;
        self.poke32(sp, HARNESS_EXIT);
        r.esp = sp;

        let ours = {
            let mut rb = r;
            match (o.run)(&mut rb, &mut Eng(&mut self.b)) {
                Ok(t) if t == HARNESS_EXIT => Outcome::Returned(rb),
                Ok(t) => Outcome::Stopped(format!("returned to {t:#x}")),
                Err(f) => Outcome::Fault(f.addr, f.write),
            }
        };
        let game = {
            let mut cpu = runtime::CpuAbi {
                eax: r.eax,
                ecx: r.ecx,
                edx: r.edx,
                ebx: r.ebx,
                esp: r.esp,
                ebp: r.ebp,
                esi: r.esi,
                edi: r.edi,
                ..Default::default()
            };
            match guest_layout::runtime_glue::invoke(o, &mut cpu, &mut self.c) {
                Ok(t) if t == runtime::TransferAbi::ret(HARNESS_EXIT) => Outcome::Returned(Regs {
                    eax: cpu.eax,
                    ecx: cpu.ecx,
                    edx: cpu.edx,
                    ebx: cpu.ebx,
                    esp: cpu.esp,
                    ebp: cpu.ebp,
                    esi: cpu.esi,
                    edi: cpu.edi,
                }),
                Ok(t) => Outcome::Stopped(format!("transfer {t:?}")),
                Err(f) => Outcome::Fault(f.addr, f.access == runtime::guest::Access::Write),
            }
        };
        let mut game_bad = vec![];
        if game != ours {
            game_bad.push(format!("outcome {game:?} vs {ours:?}"));
        }
        if self.c_ram() != &self.b.bytes[..] {
            game_bad.push("memory differs".to_string());
        }
        let theirs = self.original.run(&mut self.a, r, addr);
        let view_bad = self.view_check(o.name, &r, &theirs);

        let mut bad = vec![];
        match (&theirs, &ours) {
            (Outcome::Returned(x), Outcome::Returned(y)) => {
                if x.esp != y.esp {
                    bad.push(format!("esp {:#x} vs {:#x}", x.esp, y.esp));
                }
                for (rg, name) in [
                    (Reg::Eax, "eax"),
                    (Reg::Ecx, "ecx"),
                    (Reg::Edx, "edx"),
                    (Reg::Ebx, "ebx"),
                    (Reg::Ebp, "ebp"),
                    (Reg::Esi, "esi"),
                    (Reg::Edi, "edi"),
                ] {
                    let masked =
                        o.clobbers.contains(&rg) || (rg == Reg::Eax && o.output == Output::None);
                    if !masked && reg(x, rg) != reg(y, rg) {
                        bad.push(format!("{name} {:#x} vs {:#x}", reg(x, rg), reg(y, rg)));
                    }
                }
            }
            (Outcome::Fault(fa, wa), Outcome::Fault(fb, wb)) => {
                if (fa, wa) != (fb, wb) {
                    bad.push(format!("fault {fa:#x}/{wa} vs {fb:#x}/{wb}"));
                }
            }
            _ => bad.push(format!("outcome {theirs:?} vs {ours:?}")),
        }
        // Memory, except the original's dead frame below the entry ESP.
        let dead = (sp - DEAD_STACK) as usize..sp as usize;
        let (x, y) = (&self.a.bytes, &self.b.bytes);
        if x[..dead.start] != y[..dead.start] || x[dead.end..] != y[dead.end..] {
            let diff: Vec<usize> = (0..x.len())
                .filter(|i| !dead.contains(i) && x[*i] != y[*i])
                .collect();
            bad.push(format!(
                "{} bytes differ, first {:#x} ({:#04x} vs {:#04x})",
                diff.len(),
                diff[0],
                x[diff[0]],
                y[diff[0]]
            ));
        }
        let frame = self.a.bytes[dead.clone()].to_vec();
        self.b.bytes[dead.clone()].copy_from_slice(&frame);
        self.c
            .write_bytes(dead.start as u32, &frame, 1)
            .expect("dead frame");

        let t = self.tally.entry(o.name).or_default();
        if let Some(agreed) = view_bad.as_ref().map(|b| b.is_none()) {
            t.views += 1;
            if !agreed {
                t.view_disagreed += 1;
                self.failures.push(format!(
                    "{} {label}: view: {}",
                    o.name,
                    view_bad.flatten().unwrap_or_default()
                ));
            }
        }
        if !game_bad.is_empty() {
            t.game_disagreed += 1;
            self.failures.push(format!(
                "{} {label}: game path: {}",
                o.name,
                game_bad.join("; ")
            ));
            self.c.write_bytes(0, &self.b.bytes, 1).expect("resync");
        }
        if !bad.is_empty() {
            t.disagreed += 1;
            self.failures
                .push(format!("{} {label}: {}", o.name, bad.join("; ")));
            self.b = self.a.clone();
            self.c.write_bytes(0, &self.a.bytes, 1).expect("resync");
        } else if matches!(theirs, Outcome::Fault(..)) {
            t.faulted += 1;
        } else {
            t.returned += 1;
        }
        match theirs {
            Outcome::Returned(x) => Some(x.eax),
            _ => None,
        }
    }
}

// ---- inputs ------------------------------------------------------------------

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next() % n }
    }
    fn range(&mut self, lo: i64, hi: i64) -> i64 {
        lo + self.below((hi - lo + 1) as u64) as i64
    }
    fn chance(&mut self, pct: u64) -> bool {
        self.below(100) < pct
    }
    fn pick<T: Copy>(&mut self, v: &[T]) -> T {
        v[self.below(v.len() as u64) as usize]
    }
}

const NAME: u32 = 0x01f0_0000;
const ITER: u32 = 0x01f0_0100;
const ARRAYS: u32 = 0x0200_0000;

fn datum_fuzz(h: &mut Harness, rng: &mut Rng, arrays: usize, ops: usize) {
    for n in 0..arrays {
        let hdr = ARRAYS + (n as u32 % 8) * 4 + rng.below(4) as u32;
        let cap = rng.range(1, 70) as u32;
        let size = rng.pick(&[2u32, 3, 4, 6, 8, 12, 0x14, 0x15, 24, 64]);
        let align = rng.pick(&[0u32, 0, 2, 4, 5]);
        // Names of 0-40 bytes (longer than the 32-byte field too), at
        // unaligned addresses, so every strncpy path runs.
        let name_at = NAME + rng.below(4) as u32;
        let mut name: Vec<u8> = (0..rng.pick(&[0u64, 1, 2, 3, 5, 31, 32, 40]))
            .map(|_| rng.range(1, 255) as u8)
            .collect();
        name.push(0);
        name.extend_from_slice(&[0x55; 8]);
        h.poke(name_at, &name);
        // Dirty memory, so every zeroing is observable.
        let junk: Vec<u8> = (0..0x2000).map(|_| rng.next() as u8).collect();
        h.poke(hdr, &junk);
        h.call(
            0x16b5f0,
            Regs {
                ecx: align,
                edx: size,
                ebx: cap,
                ..Default::default()
            },
            &[hdr, name_at, 0x0dea_dbee],
            "init_contig",
        );
        if rng.chance(20) {
            let bitmap = h.peek32(hdr + 0x48);
            h.call(
                0x16b650,
                Regs {
                    esi: hdr,
                    ..Default::default()
                },
                &[name_at, cap, size, align, 0, bitmap],
                "init",
            );
            let data = (hdr + 0x4c + 63) & !63;
            h.poke32(hdr + 0x24, size);
            for _ in 0..rng.below(cap as u64 + 1) {
                let i = rng.below(cap as u64) as u32;
                h.poke(
                    data + i * size,
                    &(rng.range(1, 0xffff) as u16).to_le_bytes(),
                );
            }
            h.call(
                0x16b6b0,
                Regs {
                    esi: hdr,
                    eax: cap,
                    ecx: data,
                    ..Default::default()
                },
                &[],
                "attach",
            );
        } else {
            h.call(
                0x16b790,
                Regs {
                    eax: hdr,
                    ..Default::default()
                },
                &[],
                "make_valid",
            );
        }
        if rng.chance(15) {
            let f = h.a.bytes[hdr as usize + 0x2a] | 8;
            h.poke(hdr + 0x2a, &[f]);
        }
        if rng.chance(25) {
            h.poke(
                hdr + 0x40,
                &(rng.range(0xfff0, 0xffff) as u16).to_le_bytes(),
            );
        }
        let mut handles: Vec<u32> = vec![];
        for _ in 0..ops {
            let hw = h.peek32(hdr + 0x38) as i32;
            let some = |rng: &mut Rng, hs: &Vec<u32>| -> u32 {
                match rng.below(5) {
                    0 | 1 if !hs.is_empty() => rng.pick(hs),
                    2 if !hs.is_empty() => rng.pick(hs) ^ 0x0001_0000,
                    3 => u32::MAX,
                    _ => {
                        ((rng.range(0x8000, 0xffff) as u32) << 16)
                            | rng.below(cap as u64 + 2) as u32
                    }
                }
            };
            let idx = rng.range(-2, cap as i64 + 1) as i32 as u32;
            match rng.below(18) {
                0..=3 => {
                    let r = Regs {
                        edx: hdr,
                        ..Default::default()
                    };
                    if let Some(v) = h.call(0x16b840, r, &[], "new")
                        && v != u32::MAX
                    {
                        handles.push(v);
                    }
                }
                4 => {
                    let w = some(rng, &handles);
                    if w & 0xffff < cap + 2 {
                        h.call(
                            0x16b910,
                            Regs {
                                eax: w,
                                edx: hdr,
                                ..Default::default()
                            },
                            &[],
                            "new_at",
                        );
                    }
                }
                5 => {
                    h.call(
                        0x16b990,
                        Regs {
                            edi: idx,
                            edx: hdr,
                            ..Default::default()
                        },
                        &[],
                        "new_at_abs",
                    );
                }
                6 | 7 => {
                    let w = some(rng, &handles);
                    if w != u32::MAX && (w & 0xffff) < cap {
                        h.call(
                            0x16ba40,
                            Regs {
                                eax: w,
                                edx: hdr,
                                ..Default::default()
                            },
                            &[],
                            "delete",
                        );
                    }
                }
                8 | 9 => {
                    let w = some(rng, &handles);
                    h.call(
                        0x16bae0,
                        Regs {
                            eax: w,
                            esi: hdr,
                            ..Default::default()
                        },
                        &[],
                        "try_get",
                    );
                }
                10 => {
                    h.call(
                        0x16bb20,
                        Regs {
                            edx: idx,
                            esi: hdr,
                            ..Default::default()
                        },
                        &[],
                        "get_abs",
                    );
                    if (idx as i32) >= -1 && (idx as i32) < cap as i32 {
                        h.call(
                            0x16bb50,
                            Regs {
                                ecx: idx,
                                edx: hdr,
                                ..Default::default()
                            },
                            &[],
                            "abs_to_index",
                        );
                    }
                }
                11 => {
                    let start = rng.range(-2, hw as i64 + 2) as i32 as u32;
                    h.call(
                        0x16bc00,
                        Regs {
                            ecx: hdr,
                            edx: start,
                            ..Default::default()
                        },
                        &[],
                        "next_index",
                    );
                    let w = some(rng, &handles);
                    h.call(
                        0x16bbc0,
                        Regs {
                            eax: w,
                            esi: hdr,
                            ..Default::default()
                        },
                        &[],
                        "next_datum",
                    );
                }
                12 => {
                    let w = some(rng, &handles);
                    if w == u32::MAX || (w & 0xffff) <= cap {
                        h.call(
                            0x16bcc0,
                            Regs {
                                eax: hdr,
                                edx: w,
                                ..Default::default()
                            },
                            &[],
                            "previous",
                        );
                    }
                    h.call(0x16bc40, Regs::default(), &[hdr], "last");
                }
                13 => {
                    h.poke32(ITER, hdr);
                    h.poke32(ITER + 4, u32::MAX);
                    h.poke32(ITER + 8, u32::MAX);
                    for _ in 0..cap + 1 {
                        if h.call(
                            0x16bb70,
                            Regs {
                                esi: ITER,
                                ..Default::default()
                            },
                            &[],
                            "iter",
                        ) == Some(0)
                        {
                            break;
                        }
                    }
                }
                14 => {
                    if rng.chance(30) {
                        h.call(
                            0x16b7a0,
                            Regs {
                                esi: hdr,
                                ..Default::default()
                            },
                            &[],
                            "delete_all",
                        );
                    }
                }
                15 => {
                    if rng.chance(10) {
                        h.call(
                            0x16b790,
                            Regs {
                                eax: hdr,
                                ..Default::default()
                            },
                            &[],
                            "make_valid",
                        );
                    }
                }
                16 => {
                    // datum_initialize on an arbitrary element of this array.
                    let data = h.peek32(hdr + 0x44);
                    let elem = data + rng.below(cap as u64) as u32 * size;
                    h.call(
                        0x16ba00,
                        Regs {
                            edx: hdr,
                            esi: elem,
                            ..Default::default()
                        },
                        &[],
                        "initialize_datum",
                    );
                }
                _ => {
                    for _ in 0..cap + 1 {
                        match h.call(
                            0x16b840,
                            Regs {
                                edx: hdr,
                                ..Default::default()
                            },
                            &[],
                            "new(fill)",
                        ) {
                            Some(u32::MAX) | None => break,
                            Some(v) => handles.push(v),
                        }
                    }
                }
            }
        }
    }
}

const TINDEX: u32 = 0x0230_0000;
const TENTRIES: u32 = 0x0231_0000;
const TAG_INDEX_GLOBAL: u32 = 0x0054_7f00;
const TAG_ENTRIES_GLOBAL: u32 = 0x004e_3b44;

fn fourcc(rng: &mut Rng) -> u32 {
    let mut b = [0u8; 4];
    for x in &mut b {
        *x = rng.range(0x21, 0x7e) as u8;
    }
    if rng.chance(5) {
        b[0] |= 0x80; // exercises the wrapping comparator
    }
    u32::from_be_bytes(b)
}

fn tag_fuzz(h: &mut Harness, rng: &mut Rng, scenarios: usize, queries: usize) {
    for s in 0..scenarios {
        let mut groups: Vec<u32> = (0..rng.range(0, 24)).map(|_| fourcc(rng)).collect();
        if !rng.chance(10) {
            groups.sort();
        }
        groups.dedup();
        let recs: Vec<(u32, u32, u32)> = groups
            .iter()
            .map(|&g| {
                let p = if rng.chance(50) {
                    u32::MAX
                } else {
                    rng.pick(&groups)
                };
                let gp = if p == u32::MAX || rng.chance(50) {
                    u32::MAX
                } else {
                    rng.pick(&groups)
                };
                (g, p, gp)
            })
            .collect();
        let groups_at = TINDEX + 0x20;
        for (i, (g, p, gp)) in recs.iter().enumerate() {
            let r = groups_at + i as u32 * 12;
            h.poke32(r, *g);
            h.poke32(r + 4, *p);
            h.poke32(r + 8, *gp);
        }
        let ntags = rng.range(0, 40) as u32;
        let mut datums = vec![];
        for i in 0..ntags {
            let e = TENTRIES + i * 16;
            if rng.chance(15) {
                h.poke32(e, u32::MAX);
                h.poke32(e + 4, u32::MAX);
                h.poke32(e + 8, 0);
            } else {
                let g = if recs.is_empty() || rng.chance(5) {
                    fourcc(rng)
                } else {
                    rng.pick(&recs).0
                };
                let d = ((rng.range(0xe000, 0xffff) as u32) << 16) | i;
                h.poke32(e, g);
                h.poke32(e + 4, d);
                h.poke32(e + 8, 0x1000_0000 + i * 0x100);
                datums.push(d);
            }
        }
        h.poke32(TINDEX, if rng.chance(3) { 0 } else { groups_at });
        h.poke32(TINDEX + 4, recs.len() as u32);
        h.poke32(TINDEX + 8, TENTRIES);
        h.poke32(TINDEX + 0x18, ntags);
        h.poke32(TAG_INDEX_GLOBAL, TINDEX);
        h.poke32(TAG_ENTRIES_GLOBAL, TENTRIES);
        let null_groups = h.peek32(TINDEX) == 0;
        for q in 0..queries {
            let datum = match rng.below(6) {
                0..=2 if !datums.is_empty() => rng.pick(&datums),
                3 if !datums.is_empty() => rng.pick(&datums) ^ 0x0004_0000,
                4 => u32::MAX,
                _ => {
                    ((rng.below(0x10000) as u32) << 16)
                        | rng.pick(&[0u32, 1, ntags, ntags + 1, 0x7fff, 0x8000, 0xffff])
                }
            };
            let want = match rng.below(5) {
                0 => u32::MAX,
                1 => fourcc(rng),
                _ if !recs.is_empty() => {
                    let r = rng.pick(&recs);
                    rng.pick(&[r.0, r.1, r.2])
                }
                _ => fourcc(rng),
            };
            let label = format!("s{s}q{q}");
            if !null_groups {
                h.call(
                    0x122c10,
                    Regs {
                        ecx: datum,
                        ebx: want,
                        ..Default::default()
                    },
                    &[],
                    &label,
                );
            }
            h.call(
                0x122d00,
                Regs {
                    ecx: want,
                    ..Default::default()
                },
                &[],
                &label,
            );
        }
    }
}

const LREC: u32 = 0x0210_0000;
const LIDX: u32 = 0x0210_0100;
const LBLOB: u32 = 0x0220_0000;

fn locale_fuzz(h: &mut Harness, rng: &mut Rng, scenarios: usize, queries: usize) {
    for _ in 0..scenarios {
        let n = rng.range(0, 60) as u32;
        let mut ids = vec![];
        // Four extra entries past the count, so an over-read would show.
        for i in 0..n + 4 {
            let id = if !ids.is_empty() && rng.chance(10) {
                rng.pick(&ids) ^ 0x0100_0000 // same index, other length byte
            } else {
                ((rng.range(0, 255) as u32) << 24) | rng.below(0x100) as u32
            };
            h.poke32(LIDX + i * 8, id);
            h.poke32(LIDX + i * 8 + 4, rng.below(0x10000) as u32);
            if i < n {
                ids.push(id);
            }
        }
        h.poke32(LREC, LIDX);
        h.poke32(LREC + 4, LBLOB);
        h.poke32(LREC + 8, n);
        h.poke(LREC + 0x18, &[rng.chance(80) as u8]);
        for q in 0..queries {
            let key = if !ids.is_empty() && rng.chance(60) {
                rng.pick(&ids)
            } else {
                ((rng.range(0, 255) as u32) << 24) | rng.below(0x100) as u32
            };
            let start = rng.range(-5, n as i64 + 3) as i32 as u32;
            let count = rng.range(-2, n as i64 + 6) as i32 as u32;
            let label = format!("q{q}");
            h.call(
                0x1a02d0,
                Regs {
                    eax: start,
                    ecx: count,
                    edx: LREC,
                    edi: key,
                    ..Default::default()
                },
                &[],
                &label,
            );
            h.call(
                0x1a0310,
                Regs {
                    eax: rng.next() as u32,
                    esi: LREC,
                    ecx: start,
                    ebx: key,
                    ..Default::default()
                },
                &[count],
                &label,
            );
        }
    }
}

/// Fixed cases whose outcome is a fault or a sentinel, so fault agreement
/// (address, direction and the bytes written before it) is always tested.
fn fault_fixtures(h: &mut Harness) {
    let top = RAM as u32; // first unmapped address
    // datum_new whose element runs off the end of RAM: rep stosd zeroes
    // the 8 mapped bytes, then faults at `top`.
    let hdr = 0x0200_8000;
    h.poke(hdr, &[0; 0x4c]);
    h.poke32(hdr + 0x20, 1);
    h.poke32(hdr + 0x24, 16);
    h.poke32(hdr + 0x44, top - 8);
    h.poke32(hdr + 0x48, 0x0200_8100);
    h.poke32(hdr + 0x40, 0x8000);
    h.poke32(0x0200_8100, 0);
    h.poke(top - 8, &[0xcc; 8]);
    h.call(
        0x16b840,
        Regs {
            edx: hdr,
            ..Default::default()
        },
        &[],
        "fault: element past RAM",
    );
    // datum_initialize with an odd element size straddling the end:
    // dwords, then a faulting byte tail.
    h.poke32(hdr + 0x24, 7);
    h.poke(top - 7, &[0xcc; 7]);
    h.call(
        0x16ba00,
        Regs {
            edx: hdr,
            esi: top - 6,
            ..Default::default()
        },
        &[],
        "fault: byte tail past RAM",
    );
    // data_initialize with a name running off the end of RAM (strncpy's
    // aligned dword read faults after the bytes before it were copied).
    h.poke(top - 6, b"abcdef");
    h.call(
        0x16b650,
        Regs {
            esi: hdr,
            ..Default::default()
        },
        &[top - 6, 4, 4, 0, 0, 0],
        "fault: name past RAM",
    );
    // previous_datum's read of element -1 just below RAM... is mapped
    // here (address 0 region), so instead put data at 0: element -1 wraps
    // to the top of the address space and faults.
    h.poke(hdr, &[0; 0x4c]);
    h.poke32(hdr + 0x24, 4);
    h.poke32(hdr + 0x38, 1);
    h.poke32(hdr + 0x44, 0);
    h.call(
        0x16bcc0,
        Regs {
            eax: hdr,
            edx: u32::MAX,
            ..Default::default()
        },
        &[],
        "fault: element -1 wraps",
    );

    // tag_get_checked: an entry at address 0 with a matching datum is
    // rejected by the null-entry test.
    h.poke32(TAG_INDEX_GLOBAL, TINDEX);
    h.poke32(TINDEX, TINDEX + 0x20);
    h.poke32(TINDEX + 4, 1);
    h.poke32(TINDEX + 0x18, 1);
    h.poke32(TINDEX + 0x20, u32::from_be_bytes(*b"scnr"));
    h.poke32(TINDEX + 0x24, u32::MAX);
    h.poke32(TINDEX + 0x28, u32::MAX);
    h.poke32(TAG_ENTRIES_GLOBAL, 0);
    h.poke32(0, u32::from_be_bytes(*b"scnr"));
    h.poke32(4, 0x8000_0000);
    h.poke32(8, 0x1234_5678);
    let r = h.call(
        0x122c10,
        Regs {
            ecx: 0x8000_0000,
            ebx: u32::from_be_bytes(*b"scnr"),
            ..Default::default()
        },
        &[],
        "zero entry",
    );
    assert_eq!(r, Some(0), "the original rejects an entry at address 0");
    h.poke(0, &[0; 12]);
    // A group table past the end of RAM: the bsearch probe faults.
    h.poke32(TINDEX, top - 4);
    h.poke32(TINDEX + 4, 3);
    h.call(
        0x122d00,
        Regs {
            ecx: 1,
            ..Default::default()
        },
        &[],
        "fault: group table past RAM",
    );

    unmapped_argument_fixtures(h);
}

/// Every adapter with stack arguments, called with its stack ending at the
/// end of RAM so that only the first `k` arguments are mapped, for every
/// `k`. The original interleaves argument reads with its stores; these
/// pin which stores land before the faulting read.
fn unmapped_argument_fixtures(h: &mut Harness) {
    let top = RAM as u32;
    let hdr = 0x0200_8000;
    let name = 0x0200_8200;
    h.poke(name, b"fixture\0");

    // data_initialize: name, capacity, size, alignment byte, allocator,
    // bitmap. The header is cleared before the first argument read.
    let args = [name, 5, 12, 3, 0x0200_9000, 0x0200_9100];
    for k in 0..args.len() {
        h.poke(hdr, &[0xcc; 0x4c]);
        h.call_below(
            top,
            0x16b650,
            Regs {
                esi: hdr,
                ..Default::default()
            },
            &args[..k],
            &format!("unmapped args: data_initialize, {k} mapped"),
        );
    }

    // data_initialize_contiguous: buffer, name, allocator (read in the
    // order buffer, allocator, name).
    let args = [hdr, name, 0x0200_9000];
    for k in 0..args.len() {
        h.poke(hdr, &[0xcc; 0x4c]);
        h.call_below(
            top,
            0x16b5f0,
            Regs {
                ebx: 4,
                edx: 8,
                ecx: 2,
                ..Default::default()
            },
            &args[..k],
            &format!("unmapped args: data_initialize_contiguous, {k} mapped"),
        );
    }

    // locale_string_id_exists: the count is read only when the table is
    // loaded, so the unloaded call returns and the loaded one faults.
    let rec = 0x0200_a000;
    h.poke(rec, &[0; 0x1c]);
    for loaded in [0u8, 1] {
        h.poke(rec + 0x18, &[loaded]);
        h.call_below(
            top,
            0x1a0310,
            Regs {
                esi: rec,
                ebx: 0x0300_0002,
                ..Default::default()
            },
            &[],
            &format!("unmapped args: locale_string_id_exists, loaded {loaded}"),
        );
    }

    // data_last_datum: header argument unmapped.
    h.call_below(
        top,
        0x16bc40,
        Regs::default(),
        &[],
        "unmapped args: data_last_datum",
    );
    // data_last_datum's walk faults on its second element: its argument
    // slot already holds the first handle (stored at 0x16bc4e).
    h.poke(hdr, &[0; 0x4c]);
    h.poke32(hdr + 0x24, 4); // element size
    h.poke32(hdr + 0x38, 2); // high-water mark
    h.poke32(hdr + 0x44, top - 4); // element 1 is past RAM
    h.poke32(hdr + 0x48, 0x0200_8100); // bitmap
    h.poke32(0x0200_8100, 0b11);
    h.poke32(top - 4, 0x8001);
    h.call(
        0x16bc40,
        Regs::default(),
        &[hdr],
        "fault: last_datum mid-walk",
    );
}

// ---- image identity (tests/common) --------------------------------------

fn env_list(name: &str) -> Option<Vec<u64>> {
    std::env::var(name).ok().map(|s| {
        s.split(',')
            .map(|x| x.trim().parse().expect("number"))
            .collect()
    })
}

#[test]
fn sha256_known_answer() {
    assert_eq!(
        sha256(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn overrides_agree_with_the_lifted_original() {
    use std::io::Write;
    let Some(path) = xbe_path() else {
        let _ = writeln!(
            std::io::stderr(),
            "SKIPPED overrides_agree_with_the_lifted_original: no XBE (set H2_XBE)"
        );
        return;
    };
    let data = std::fs::read(&path).unwrap();
    assert_eq!(
        sha256(&data),
        IMAGE_SHA256,
        "contracts were recovered from the retail XBE only"
    );
    let x = xbe::Xbe::parse(&data).unwrap();
    let analysis = analyze::analyze(&x).unwrap();
    let lifter = lift::Lifter::new(&analysis, &x).unwrap();
    let image = lift::harness::load_image(&x).unwrap();
    assert_eq!(
        image.bytes[guest_layout::locale::EMPTY_STRING as usize],
        0,
        "EMPTY_STRING is a NUL byte"
    );

    let seeds = env_list("H2_DIFF_SEEDS").unwrap_or_else(|| DEFAULT_SEEDS[..1].to_vec());
    let scale = env_list("H2_DIFF_SCALE").map_or(1, |v| v[0] as usize);
    let c = runtime_guest(&image);
    let mut h = Harness {
        original: Original {
            lifter: &lifter,
            cache: BTreeMap::new(),
            steps: 0,
        },
        a: image.clone(),
        c: c.0,
        c_phys: c.1,
        b: image,
        tally: BTreeMap::new(),
        failures: vec![],
    };
    fault_fixtures(&mut h);
    for &seed in &seeds {
        let mut rng = Rng(seed | 1);
        datum_fuzz(&mut h, &mut rng, 40 * scale, 120);
        tag_fuzz(&mut h, &mut rng, 30 * scale, 40);
        locale_fuzz(&mut h, &mut rng, 30 * scale, 30);
    }

    println!(
        "image sha256 {IMAGE_SHA256}; seeds {seeds:?}, scale {scale}; {} original instructions",
        h.original.steps
    );
    println!(
        "{:<32} {:>9} {:>8} {:>9} {:>9} {:>6} {:>9}",
        "function", "returned", "faulted", "disagreed", "game-path", "views", "view-bad"
    );
    for (name, t) in &h.tally {
        println!(
            "{name:<32} {:>9} {:>8} {:>9} {:>9} {:>6} {:>9}",
            t.returned, t.faulted, t.disagreed, t.game_disagreed, t.views, t.view_disagreed
        );
    }
    for f in h.failures.iter().take(30) {
        println!("FAIL {f}");
    }
    let untested: Vec<_> = abi::OVERRIDES
        .iter()
        .filter(|o| !h.tally.contains_key(o.name))
        .map(|o| o.name)
        .collect();
    assert!(
        untested.is_empty(),
        "adapters never exercised: {untested:?}"
    );
    assert!(h.failures.is_empty(), "{} disagreements", h.failures.len());
}
