//! The record clean-up `0x19d650` and the lobby protocol map `0x161c90`
//! against the lifted original, for the native answers of the script
//! index (`variants::record::clean_up`, `variants::record::protocol`; the
//! hooks in `crates/extensions/game/src/rules/record.rs`; the design in
//! docs/game/game-types-without-carriers.md, "No retail engine object").
//!
//! - On invented records of every retail index the original keeps a
//!   record exactly when `variants::record::check` finds nothing, leaves
//!   the registers the native answer leaves, and resets a broken one to a
//!   Slayer record; the native clean-up leaves all of them to it.
//! - A script record is kept, has its type area cleaned, or is reset as
//!   Slayer would be, through a chain that does what the hook does; the
//!   original alone resets every one of them.
//! - The protocol map answers as the original, registers included, for
//!   every index but the script index, which plays distributed.
//!
//! ```sh
//! H2_XBE=/abs/path/default.xbe cargo test -p engine --features differential \
//!     --test it variant_record_differential::
//! ```
#![cfg(feature = "differential")]

use std::collections::BTreeMap;

use engine::points;
use ir::interp::{self, Cpu};
use lift::harness::HARNESS_EXIT;
use lifted::image::{Image, Rng};
use variants::record::{
    self, CleanUp, DISTRIBUTED, ENGINE_AT, GameType, GameVariant, RECORD_SIZE, SCRIPT_INDEX,
    SYNCHRONOUS, TYPE_AREA, check,
};

const STACK: u32 = 0x007f_f000;
/// Where the record under test lives.
const REC: u32 = 0x0062_0000;
const STEPS: u64 = 1_000_000;

/// The image's memory with the test's writes over it.
#[derive(Clone)]
struct Ram<'a> {
    image: &'a [u8],
    overlay: BTreeMap<u32, u8>,
}

impl<'a> Ram<'a> {
    fn new(image: &'a Image) -> Self {
        Self {
            image: &image.ram.bytes,
            overlay: BTreeMap::new(),
        }
    }
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
        for (i, &b) in data.iter().enumerate() {
            self.overlay.insert(a + i as u32, b);
        }
        true
    }
    fn record(&self) -> [u8; RECORD_SIZE] {
        let mut b = [0; RECORD_SIZE];
        assert!(self.get(REC, &mut b));
        b
    }
    fn set_record(&mut self, b: &[u8; RECORD_SIZE]) {
        assert!(self.put(REC, b));
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

/// Registers for a call: EBX and ECX the record, the rest noise.
fn regs(rng: &mut Rng) -> [u32; 8] {
    let mut r = [0; 8];
    for x in &mut r {
        *x = rng.u32();
    }
    r[1] = REC; // ecx
    r[3] = REC; // ebx
    r[4] = STACK; // esp
    r
}

/// Runs the original from `entry` to its return.
fn run(original: &mut lifted::Original, mem: &mut Ram<'_>, entry: u32, gpr: [u32; 8]) -> Cpu {
    assert!(mem.put(STACK, &HARNESS_EXIT.to_le_bytes()));
    let mut cpu = Cpu {
        gpr,
        eip: entry,
        eflags: 0x202,
        ..Cpu::default()
    };
    cpu.flags_valid = u32::MAX;
    lifted::code::run_until(original, &mut cpu, mem, STEPS, |c| c.eip == HARNESS_EXIT)
        .unwrap_or_else(|e| panic!("{entry:#x}: {e}"));
    assert_eq!(cpu.gpr[4], STACK + 4, "{entry:#x} pops nothing");
    cpu
}

/// The clean-up as the hook makes it: the native answer, or the original.
fn hooked_clean_up(original: &mut lifted::Original, mem: &mut Ram<'_>, gpr: [u32; 8]) -> Cpu {
    let mut raw = mem.record();
    let verdict = record::clean_up(&mut raw);
    mem.set_record(&raw);
    let al = match verdict {
        CleanUp::Retail | CleanUp::ResetAsSlayer => {
            return run(original, mem, points::VARIANT_CLEAN_UP.base, gpr);
        }
        CleanUp::Clean => 1,
        CleanUp::TypeAreaCleared => 0,
    };
    let [_, ecx, edx] = record::CLEAN_UP_EXIT;
    let mut cpu = Cpu {
        gpr,
        eip: HARNESS_EXIT,
        ..Cpu::default()
    };
    cpu.gpr[0] = al;
    cpu.gpr[1] = ecx;
    cpu.gpr[2] = edx;
    cpu.gpr[4] = STACK + 4;
    cpu
}

/// Offsets the clean-up has a rule for, to aim mutations at.
fn ruled_offsets() -> Vec<usize> {
    let mut at = vec![0x00, 0x01, 0x42, 0x43, 0x44, 0x45, 0x48, 0x49, 0x4b];
    for base in [
        0x4c, 0x50, 0x54, 0x58, 0x74, 0x78, 0x7c, 0x80, 0x84, 0x88, 0xa4, 0xa8, 0xac, 0xb0, 0xb4,
    ] {
        at.extend([base, base + 1, base + 3]);
    }
    at.extend(0xcc..=0xd7);
    at.extend(TYPE_AREA);
    at
}

/// A record of `ty` with up to three bytes changed: valid or not by chance.
fn invented(rng: &mut Rng, ty: GameType, ruled: &[usize]) -> [u8; RECORD_SIZE] {
    let mut b = GameVariant::stock(ty).unwrap().to_bytes();
    for _ in 0..rng.below(4) {
        let at = if rng.below(2) == 0 {
            ruled[rng.below(ruled.len() as u32) as usize]
        } else {
            rng.below(RECORD_SIZE as u32) as usize
        };
        b[at] = match rng.below(4) {
            0 | 1 => rng.below(0x21) as u8,
            2 => 0xff,
            _ => rng.u32() as u8,
        };
    }
    b
}

fn engine_of(b: &[u8; RECORD_SIZE]) -> i32 {
    i32::from_le_bytes(b[ENGINE_AT..ENGINE_AT + 4].try_into().unwrap())
}

#[test]
fn the_clean_up_keeps_what_check_keeps_on_every_retail_index() {
    let image = Image::require();
    let mut original = image.original();
    let base = Ram::new(image);
    let ruled = ruled_offsets();
    let mut rng = Rng(0x0019_d650);
    let (mut kept, mut reset) = (0, 0);
    for n in 0..3000 {
        let ty = GameType::ALL[rng.below(GameType::ALL.len() as u32) as usize];
        let b = invented(&mut rng, ty, &ruled);
        if engine_of(&b) == SCRIPT_INDEX {
            continue;
        }
        let v = GameVariant::from_bytes(&b).unwrap();
        let problems = check(&v);
        let mut native = b;
        assert_eq!(record::clean_up(&mut native), CleanUp::Retail, "{n}");
        assert_eq!(
            native, b,
            "{n}: the native clean-up writes no retail record"
        );

        let mut mem = base.clone();
        mem.set_record(&b);
        let cpu = run(
            &mut original,
            &mut mem,
            points::VARIANT_CLEAN_UP.base,
            regs(&mut rng),
        );
        let al = cpu.gpr[0] & 0xff;
        let what = format!("{n} {ty:?} {problems:?}");
        assert_eq!(al == 1, problems.is_empty(), "{what}");
        if problems.is_empty() {
            kept += 1;
            assert_eq!(mem.record(), b, "{what}: a kept record is not changed");
            assert_eq!(
                [cpu.gpr[0], cpu.gpr[1], cpu.gpr[2]],
                record::CLEAN_UP_EXIT,
                "{what}: the clean exit's registers"
            );
        } else {
            reset += 1;
            assert_eq!(engine_of(&mem.record()), 2, "{what}: reset to Slayer");
        }
    }
    assert!(kept > 300 && reset > 300, "{kept} kept, {reset} reset");
}

/// A script record over the stock common settings of a random type.
fn script_record(rng: &mut Rng) -> [u8; RECORD_SIZE] {
    let ty = GameType::ALL[rng.below(GameType::ALL.len() as u32) as usize];
    let mut b = GameVariant::stock(ty).unwrap().to_bytes();
    b[ENGINE_AT..ENGINE_AT + 4].copy_from_slice(&SCRIPT_INDEX.to_le_bytes());
    b[TYPE_AREA].fill(0);
    // The `hx` block's script reference (mode, hash prefix, options).
    b[0x11f] = 2;
    for x in &mut b[0x120..0x12c] {
        *x = rng.u32() as u8;
    }
    b
}

#[test]
fn a_script_record_is_kept_cleaned_or_reset_as_slayer() {
    let image = Image::require();
    let mut original = image.original();
    let base = Ram::new(image);
    let mut rng = Rng(0x0000_0006);
    // Common rule breaks: rounds, score, shields, the starting weapon, the
    // header word, the name's last unit, flag bit 15.
    let breaks: [(usize, u8); 7] = [
        (0x4c, 7),
        (0x53, 0x80),
        (0x88, 3),
        (0xd6, 0x15),
        (0x01, 1),
        (0x42, 0x41),
        (0x49, 0x80),
    ];
    for n in 0..300 {
        let b = script_record(&mut rng);
        let noise = rng.below(2) == 1;
        let broken = rng.below(3) == 0;
        let mut input = b;
        if noise {
            let at = TYPE_AREA.start + rng.below(TYPE_AREA.len() as u32) as usize;
            input[at] = 1 + rng.below(0xff) as u8;
        }
        if broken {
            let (at, x) = breaks[rng.below(breaks.len() as u32) as usize];
            input[at] = x;
        }
        let what = format!("{n} noise {noise} broken {broken}");

        // The original alone knows no script index and resets the record.
        let mut alone = base.clone();
        alone.set_record(&input);
        let cpu = run(
            &mut original,
            &mut alone,
            points::VARIANT_CLEAN_UP.base,
            regs(&mut rng),
        );
        assert_eq!(cpu.gpr[0] & 0xff, 0, "{what}: the game alone refuses it");
        assert_eq!(engine_of(&alone.record()), 2, "{what}");

        let mut mem = base.clone();
        mem.set_record(&input);
        let cpu = hooked_clean_up(&mut original, &mut mem, regs(&mut rng));
        let al = cpu.gpr[0] & 0xff;
        let got = mem.record();
        if broken {
            // As the game resets a broken Slayer record.
            let mut as_slayer = input;
            as_slayer[ENGINE_AT..ENGINE_AT + 4].copy_from_slice(&2i32.to_le_bytes());
            let mut want = base.clone();
            want.set_record(&as_slayer);
            let want_cpu = run(
                &mut original,
                &mut want,
                points::VARIANT_CLEAN_UP.base,
                regs(&mut rng),
            );
            assert_eq!(want_cpu.gpr[0] & 0xff, 0, "{what}");
            assert_eq!(al, 0, "{what}");
            assert_eq!(got, want.record(), "{what}: the default Slayer record");
        } else if noise {
            assert_eq!(al, 0, "{what}: changed");
            assert_eq!(got, b, "{what}: only the type area, back to zero");
        } else {
            assert_eq!(al, 1, "{what}: kept");
            assert_eq!(got, input, "{what}: not written");
            assert_eq!([cpu.gpr[0], cpu.gpr[1], cpu.gpr[2]], record::CLEAN_UP_EXIT);
        }
        if !broken {
            assert_eq!(engine_of(&got), SCRIPT_INDEX, "{what}: the index stays");
            assert_eq!(
                got[0x11f..0x12c],
                b[0x11f..0x12c],
                "{what}: the reference stays"
            );
        }
    }
}

#[test]
fn the_protocol_map_answers_as_the_game_and_the_script_index_plays_distributed() {
    let image = Image::require();
    let mut original = image.original();
    let base = Ram::new(image);
    let mut rng = Rng(0x0016_1c90);
    let mut engines: Vec<i32> = (-3..=20).collect();
    engines.extend([i32::MIN, i32::MAX, 0x100 + 2]);
    for _ in 0..64 {
        engines.push(rng.u32() as i32);
    }
    for engine in engines {
        let mut mem = base.clone();
        assert!(mem.put(REC + ENGINE_AT as u32, &engine.to_le_bytes()));
        let gpr = regs(&mut rng);
        let cpu = run(&mut original, &mut mem, points::VARIANT_PROTOCOL.base, gpr);
        let (eax, ecx, edx) = record::protocol(engine);
        let edx = edx.unwrap_or(gpr[2]);
        if engine == SCRIPT_INDEX {
            assert_eq!(cpu.gpr[0], SYNCHRONOUS, "the game's answer for index 6");
            assert_eq!(eax, DISTRIBUTED);
            assert_eq!([cpu.gpr[1], cpu.gpr[2]], [ecx, 1], "{engine}");
            assert_eq!(edx, 0);
        } else {
            assert_eq!(
                [cpu.gpr[0], cpu.gpr[1], cpu.gpr[2]],
                [eax, ecx, edx],
                "{engine}"
            );
        }
        assert_eq!(cpu.gpr[3], gpr[3], "EBX is the caller's");
        assert_eq!(cpu.gpr[6..], gpr[6..], "ESI and EDI are the caller's");
    }
}
