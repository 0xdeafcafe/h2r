//! The map cache status `0x213f10` answered from [`engine::map_status`]
//! against the lifted original, alone and inside its frame poll
//! `0x163680` (docs/runtime/cache-slots.md, "The status poll").
//!
//! Each scenario seeds the slot paths, copy requests, queued paths and a
//! path argument, then runs a call three times on one [`Cache`]: from the
//! seeded state (a miss), again with fresh garbage in the dead stack (a
//! hit), and after one change to a byte the original reads (a miss, or a
//! hit when the byte is outside every block). Each run is compared with
//! the original run from the same state: every register, and every store
//! outside the dead stack, in order. The hooked run mimics the plan's
//! chain: a hit returns at once; a miss runs the original and records
//! its answer when it returns.
//!
//! ```text
//! H2_XBE=/path/to/default.xbe cargo test -p engine --features differential \
//!     --test it map_status_differential::
//! ```
#![cfg(feature = "differential")]

use std::collections::BTreeMap;

use engine::map_status::{
    self, Answer, Cache, Lookup, QUEUE, QUEUE_SECOND, REQUESTS, SLOT_PATHS, SLOT_STRIDE,
};
use guest_layout::memory::{GuestMemory, MemFault};
use guest_layout::points;
use ir::interp::{self, Cpu};
use lift::harness::HARNESS_EXIT;
use lifted::image::{Image, Rng};

const STACK: u32 = 0x007f_f000;
/// The frame poll's out block (`ret 4`, three dwords).
const OUT: u32 = 0x0062_0000;
/// Where scenario path arguments live.
const ARGS: u32 = 0x0062_1000;
const POLL: u32 = 0x0016_3680;
/// The map path pointers the frame poll passes.
const POLL_PATHS: [u32; 4] = [0x0046_87fc, 0x0046_8800, 0x0046_8804, 0x0046_8808];
const STEPS: u64 = 5_000_000;

#[derive(Clone)]
struct Ram<'a> {
    image: &'a [u8],
    overlay: BTreeMap<u32, u8>,
    writes: Vec<(u32, Vec<u8>)>,
}

impl Ram<'_> {
    fn get(&self, a: u32, dst: &mut [u8]) -> bool {
        if a as usize + dst.len() > self.image.len() {
            return false;
        }
        for (i, b) in dst.iter_mut().enumerate() {
            let p = a + i as u32;
            *b = self
                .overlay
                .get(&p)
                .copied()
                .unwrap_or(self.image[p as usize]);
        }
        true
    }
    fn put(&mut self, a: u32, data: &[u8]) -> bool {
        if a as usize + data.len() > self.image.len() {
            return false;
        }
        self.writes.push((a, data.to_vec()));
        for (i, &b) in data.iter().enumerate() {
            self.overlay.insert(a + i as u32, b);
        }
        true
    }
    fn word(&self, a: u32) -> u32 {
        let mut b = [0; 4];
        assert!(self.get(a, &mut b));
        u32::from_le_bytes(b)
    }
    fn seed(&mut self, a: u32, data: &[u8]) {
        assert!(self.put(a, data));
    }
    /// Stores and final bytes outside the dead stack below `floor`.
    fn live(&self, floor: u32) -> (Vec<(u32, Vec<u8>)>, BTreeMap<u32, u8>) {
        let dead = |a: u32| (STACK - 0x10000..floor).contains(&a);
        let writes = self
            .writes
            .iter()
            .flat_map(|(a, d)| d.iter().enumerate().map(move |(i, &b)| (a + i as u32, b)))
            .filter(|&(a, _)| !dead(a))
            .map(|(a, b)| (a, vec![b]))
            .collect();
        let bytes = self
            .overlay
            .iter()
            .filter(|(a, _)| !dead(**a))
            .map(|(&a, &b)| (a, b))
            .collect();
        (writes, bytes)
    }
}

impl interp::Memory for Ram<'_> {
    fn read(&mut self, a: u32, dst: &mut [u8]) -> Result<(), interp::MemFault> {
        if Ram::get(self, a, dst) {
            Ok(())
        } else {
            Err(interp::MemFault {
                addr: a,
                write: false,
            })
        }
    }
    fn write(&mut self, a: u32, data: &[u8]) -> Result<(), interp::MemFault> {
        if Ram::put(self, a, data) {
            Ok(())
        } else {
            Err(interp::MemFault {
                addr: a,
                write: true,
            })
        }
    }
}

impl GuestMemory for Ram<'_> {
    fn read(&self, a: u32, dst: &mut [u8]) -> Result<(), MemFault> {
        if self.get(a, dst) {
            Ok(())
        } else {
            Err(MemFault {
                addr: a,
                len: dst.len() as u32,
                write: false,
            })
        }
    }
    fn write(&mut self, a: u32, data: &[u8]) -> Result<(), MemFault> {
        if self.put(a, data) {
            Ok(())
        } else {
            Err(MemFault {
                addr: a,
                len: data.len() as u32,
                write: true,
            })
        }
    }
}

fn answer_of(c: &Cpu) -> Answer {
    Answer::of(c.gpr[0], c.gpr[1], c.gpr[2], c.eflags, c.flags_valid)
}

#[derive(Default, Debug)]
struct Counts {
    hits: u32,
    misses: u32,
    bypasses: u32,
}

/// Run from `cpu.eip` to `HARNESS_EXIT`. With a cache, entries to the
/// status go through it as the plan's chain does.
fn drive(
    original: &mut lifted::Original,
    mut cache: Option<&mut Cache>,
    cpu: &mut Cpu,
    mem: &mut Ram<'_>,
    counts: &mut Counts,
) {
    // Calls that missed and run the original: (entry ESP, return PC).
    let mut open: Vec<(u32, u32)> = Vec::new();
    for _ in 0..STEPS {
        if let Some(&(esp, ret)) = open.last()
            && cpu.eip == ret
            && cpu.gpr[4] == esp.wrapping_add(4)
        {
            open.pop();
            let cache = cache.as_deref_mut().expect("only a cache opens calls");
            cache.record(&*mem, esp, answer_of(cpu));
            continue;
        }
        if cpu.eip == HARNESS_EXIT {
            assert!(open.is_empty(), "returned with a status call open");
            return;
        }
        if cpu.eip == points::MAP_STATUS.base
            && let Some(cache) = cache.as_deref_mut()
        {
            let esp = cpu.gpr[4];
            match cache.lookup(&*mem, esp, cpu.gpr[0]) {
                Lookup::Hit(a) => {
                    counts.hits += 1;
                    cpu.gpr[0] = a.eax;
                    cpu.gpr[1] = a.ecx;
                    cpu.gpr[2] = a.edx;
                    (cpu.eflags, cpu.flags_valid) = a.merge_flags(cpu.eflags, cpu.flags_valid);
                    cpu.eip = mem.word(esp);
                    cpu.gpr[4] = esp.wrapping_add(4);
                    continue;
                }
                Lookup::Miss => {
                    counts.misses += 1;
                    open.push((esp, mem.word(esp)));
                }
                Lookup::Bypass(_) => counts.bypasses += 1,
            }
        }
        let pc = cpu.eip;
        lifted::code::step(original, cpu, mem).unwrap_or_else(|e| panic!("at {pc:#x}: {e}"));
    }
    panic!("step limit at {:#x}", cpu.eip);
}

/// One entry state: registers, then memory.
struct Call<'a> {
    cpu: Cpu,
    mem: Ram<'a>,
    /// Stores below this are the dead stack.
    floor: u32,
}

impl Call<'_> {
    /// The original and the hooked run from this state, compared.
    fn check(&self, original: &mut lifted::Original, cache: &mut Cache, what: &str) -> Counts {
        let (mut want, mut want_mem) = (self.cpu.clone(), self.mem.clone());
        drive(
            original,
            None,
            &mut want,
            &mut want_mem,
            &mut Counts::default(),
        );
        let (mut got, mut got_mem) = (self.cpu.clone(), self.mem.clone());
        let mut counts = Counts::default();
        drive(original, Some(cache), &mut got, &mut got_mem, &mut counts);
        assert_eq!(got, want, "registers: {what}");
        let (got_writes, got_bytes) = got_mem.live(self.floor);
        let (want_writes, want_bytes) = want_mem.live(self.floor);
        assert_eq!(got_writes, want_writes, "stores: {what}");
        assert_eq!(got_bytes, want_bytes, "memory: {what}");
        counts
    }
}

fn cpu(rng: &mut Rng, entry: u32, esp: u32, eax: u32) -> Cpu {
    let mut c = Cpu {
        gpr: [
            eax,
            rng.u32(),
            rng.u32(),
            rng.u32(),
            esp,
            rng.u32(),
            rng.u32(),
            rng.u32(),
        ],
        eip: entry,
        eflags: 0x202 | (rng.u32() & 0x8d5),
        ..Cpu::default()
    };
    c.flags_valid = u32::MAX;
    c
}

fn cstr(mem: &Ram<'_>, ptr: u32) -> Vec<u8> {
    let mut out = Vec::new();
    for i in 0..0x100 {
        let mut b = [0];
        assert!(mem.get(ptr + i, &mut b));
        if b[0] == 0 {
            return out;
        }
        out.push(b[0]);
    }
    panic!("no NUL at {ptr:#x}");
}

/// The names the scenarios draw from: the frame poll's own maps and a few more.
fn names(mem: &Ram<'_>) -> Vec<Vec<u8>> {
    let mut names: Vec<Vec<u8>> = POLL_PATHS
        .iter()
        .map(|&p| cstr(mem, mem.word(p)))
        .map(|path| {
            let name = path.rsplit(|&b| b == b'\\').next().unwrap().to_vec();
            name.split(|&b| b == b'.').next().unwrap().to_vec()
        })
        .collect();
    for extra in [
        &b"lockout"[..],
        b"ascension",
        b"03a_oldmombasa",
        b"Zanzibar",
    ] {
        names.push(extra.to_vec());
    }
    names
}

fn pick<'a, T>(rng: &mut Rng, of: &'a [T]) -> &'a T {
    &of[rng.below(of.len() as u32) as usize]
}

/// A path for `name` in one of the spellings the status normalises.
fn spelling(rng: &mut Rng, name: &[u8]) -> Vec<u8> {
    let mut p = match rng.below(5) {
        0 => [&b"d:\\maps\\"[..], name, b".map"].concat(),
        1 => [&b"D:\\MAPS\\"[..], &name.to_ascii_uppercase(), b".MAP"].concat(),
        2 => [&b"t:\\$c\\offer\\"[..], name, b".map"].concat(),
        3 => [&b"w:\\"[..], name].concat(),
        _ => name.to_vec(),
    };
    p.push(0);
    p
}

fn normalised(name: &[u8]) -> Vec<u8> {
    let mut p = [&b"d:\\maps\\"[..], &name.to_ascii_lowercase(), b".map\0"].concat();
    p.resize(0x100, 0);
    p
}

/// Slot paths, copy requests and queued paths, drawn from `names`.
fn seed_state(rng: &mut Rng, mem: &mut Ram<'_>, names: &[Vec<u8>]) {
    for slot in 0..map_status::SLOTS {
        let path = if rng.below(3) == 0 {
            vec![0; 0x100]
        } else {
            let mut p = normalised(pick(rng, names));
            if rng.below(2) == 0 {
                p.make_ascii_uppercase();
            }
            p
        };
        mem.seed(SLOT_PATHS + slot * SLOT_STRIDE, &path);
    }
    for i in 0..8u32 {
        let at = REQUESTS + i * 0x108;
        let path = if rng.below(2) == 0 {
            vec![0; 0x100]
        } else {
            normalised(pick(rng, names))
        };
        mem.seed(at, &path);
        mem.seed(at + 0x104, &rng.below(6).to_le_bytes());
    }
    for (at, value) in [
        (QUEUE, 0x0055_bf24),
        (QUEUE + QUEUE_SECOND as u32, 0x0055_c028),
    ] {
        let mut path = if rng.below(2) == 0 {
            vec![0]
        } else {
            let name = pick(rng, names).clone();
            spelling(rng, &name)
        };
        path.resize(0x100, 0);
        mem.seed(at, &path);
        let v = *pick(rng, &[u32::MAX, 0, 1, 5]);
        mem.seed(value, &v.to_le_bytes());
    }
}

/// Fresh garbage in the dead stack below `esp`.
fn scribble(rng: &mut Rng, mem: &mut Ram<'_>, esp: u32) {
    let junk: Vec<u8> = (0..0x2000).map(|_| rng.u32() as u8).collect();
    mem.seed(esp - 0x2000, &junk);
}

/// One byte the original may read, changed.
fn mutate(rng: &mut Rng, mem: &mut Ram<'_>) -> u32 {
    let at = match rng.below(4) {
        0 => SLOT_PATHS + rng.below(map_status::SLOTS) * SLOT_STRIDE + rng.below(0x18),
        1 => REQUESTS + rng.below(8) * 0x108 + *pick(rng, &[0, 9, 0x104]),
        2 => *pick(
            rng,
            &[
                QUEUE,
                QUEUE + 3,
                0x0055_bf24,
                QUEUE + QUEUE_SECOND as u32,
                0x0055_c028,
            ],
        ),
        _ => 0x0055_c030, // past the queue block: not read
    };
    let mut b = [0];
    assert!(mem.get(at, &mut b));
    mem.seed(at, &[b[0] ^ (1 + rng.below(0x7f) as u8)]);
    at
}

/// Three runs on one cache: seeded, dead stack changed, one byte changed.
fn scenario(
    original: &mut lifted::Original,
    cache: &mut Cache,
    mut call: Call<'_>,
    rng: &mut Rng,
    what: &str,
) -> [Counts; 3] {
    let esp = call.cpu.gpr[4];
    let first = call.check(original, cache, &format!("{what}, seeded"));
    scribble(rng, &mut call.mem, esp);
    let second = call.check(original, cache, &format!("{what}, dead stack changed"));
    let at = mutate(rng, &mut call.mem);
    let third = call.check(original, cache, &format!("{what}, {at:#x} changed"));
    [first, second, third]
}

#[test]
fn the_status_matches_the_original() {
    let image = Image::require();
    let mut original = image.original();
    let base = Ram {
        image: &image.ram.bytes,
        overlay: BTreeMap::new(),
        writes: Vec::new(),
    };
    let names = names(&base);
    let mut rng = Rng(0x2131_3f10);
    let mut cache = Cache::new();
    let (mut hits, mut cached_misses) = (0, 0);
    for n in 0..200 {
        let mut mem = base.clone();
        seed_state(&mut rng, &mut mem, &names);
        let name = pick(&mut rng, &names).clone();
        let path = spelling(&mut rng, &name);
        mem.seed(ARGS, &path);
        let esp = STACK - 4 * rng.below(8);
        mem.seed(esp, &HARNESS_EXIT.to_le_bytes());
        mem.writes.clear();
        let call = Call {
            cpu: cpu(&mut rng, points::MAP_STATUS.base, esp, ARGS),
            mem,
            floor: esp,
        };
        let what = format!("case {n} {:?}", String::from_utf8_lossy(&path));
        let [first, second, third] = scenario(&mut original, &mut cache, call, &mut rng, &what);
        assert_eq!(
            (first.hits, first.misses),
            (0, 1),
            "{what}: a new state misses"
        );
        assert_eq!(
            (second.hits, second.misses),
            (1, 0),
            "{what}: only the dead stack changed"
        );
        hits += second.hits + third.hits;
        cached_misses += third.misses;
    }
    // Most single-byte changes are read; some are not.
    assert!(cached_misses > 100, "{cached_misses} misses after a change");
    assert!(hits > 200, "{hits} hits");
}

#[test]
fn uncacheable_states_run_the_original() {
    let image = Image::require();
    let mut original = image.original();
    let base = Ram {
        image: &image.ram.bytes,
        overlay: BTreeMap::new(),
        writes: Vec::new(),
    };
    let names = names(&base);
    let mut rng = Rng(0x0048_0ce8);
    let mut cache = Cache::new();
    for (n, case) in ["long path", "unterminated queue"].into_iter().enumerate() {
        let mut mem = base.clone();
        seed_state(&mut rng, &mut mem, &names);
        let mut path = vec![b'a'; map_status::PATH_MAX + 1];
        if case == "unterminated queue" {
            path = spelling(&mut rng, &names[0]);
            let tail = vec![b'q'; map_status::QUEUE_BYTES - QUEUE_SECOND];
            mem.seed(QUEUE + QUEUE_SECOND as u32, &tail);
            mem.seed(QUEUE, &[b'q'; QUEUE_SECOND]);
            // Terminated just past the block, so the original stays in bounds.
            mem.seed(0x0055_c02c, &[0]);
        } else {
            path.push(0);
        }
        mem.seed(ARGS, &path);
        mem.seed(STACK, &HARNESS_EXIT.to_le_bytes());
        mem.writes.clear();
        let call = Call {
            cpu: cpu(&mut rng, points::MAP_STATUS.base, STACK, ARGS),
            mem,
            floor: STACK,
        };
        for _ in 0..2 {
            let c = call.check(&mut original, &mut cache, &format!("{n} {case}"));
            assert_eq!((c.hits, c.misses, c.bypasses), (0, 0, 1), "{case}");
        }
    }
}

#[test]
fn the_frame_poll_matches_the_original() {
    let image = Image::require();
    let mut original = image.original();
    let base = Ram {
        image: &image.ram.bytes,
        overlay: BTreeMap::new(),
        writes: Vec::new(),
    };
    let names = names(&base);
    let mut rng = Rng(0x0016_3680);
    let mut cache = Cache::new();
    let mut hits = 0;
    for n in 0..100 {
        let mut mem = base.clone();
        seed_state(&mut rng, &mut mem, &names);
        // Some polls find their maps resident: the poll passes scenario
        // paths, which the status normalises to `d:\maps\<name>.map`.
        if rng.below(2) == 0 {
            for (slot, name) in names[..POLL_PATHS.len()].iter().enumerate() {
                if rng.below(2) == 0 {
                    mem.seed(SLOT_PATHS + slot as u32 * SLOT_STRIDE, &normalised(name));
                }
            }
        }
        let esp = STACK - 4 * rng.below(8);
        mem.seed(esp, &HARNESS_EXIT.to_le_bytes());
        mem.seed(esp + 4, &OUT.to_le_bytes());
        mem.writes.clear();
        // The frame's locals sit above the status calls' dead stack.
        let eax = rng.u32();
        let call = Call {
            cpu: cpu(&mut rng, POLL, esp, eax),
            mem,
            floor: esp - 0x34,
        };
        let [first, second, third] = scenario(
            &mut original,
            &mut cache,
            call,
            &mut rng,
            &format!("poll {n}"),
        );
        // Two to five status calls, each with its own path.
        assert!(first.misses >= 2 && first.hits == 0, "poll {n}: {first:?}");
        assert!(second.hits >= 2, "poll {n}: {second:?}");
        assert_eq!(second.misses, 0, "poll {n}: {second:?}");
        hits += second.hits + third.hits;
    }
    assert!(hits > 200, "{hits} hits");
}
