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
//! - The wire codec (`variants::wire`, encode `0x7cc50`, decode
//!   `0x7d520`) writes and reads the bits the original does for invented
//!   records and streams of every retail index, garbage streams included;
//!   a script record goes on the wire as a Slayer record would, less
//!   Slayer's type area, and comes off it with a zero one, where the
//!   original alone refuses it.
//! - The content category (`0x216a50`) answers as the original for every
//!   retail index; the script index is filed as Slayer's.
//!
//! ```sh
//! H2_XBE=/abs/path/default.xbe cargo test -p engine --features differential \
//!     --test it variant_record_differential::
//! ```
#![cfg(feature = "differential")]

use std::collections::BTreeMap;

use guest_layout::points;
use ir::interp::{self, Cpu};
use lift::harness::HARNESS_EXIT;
use lifted::image::{Image, Rng};
use variants::record::{
    self, CleanUp, DISTRIBUTED, ENGINE_AT, GameType, GameVariant, RECORD_SIZE, SCRIPT_INDEX,
    SYNCHRONOUS, TYPE_AREA, check,
};
use variants::wire::{self, Bits, Decoded};

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

/// The content category answers as the game for every retail index; the
/// script index, where the game jumps through a zero entry, is filed as
/// Slayer's natively.
#[test]
fn the_content_category_answers_as_the_game_for_every_retail_index() {
    let image = Image::require();
    let mut original = image.original();
    let base = Ram::new(image);
    let mut rng = Rng(0x0021_6a50);
    for ty in GameType::ALL {
        let engine = ty.engine();
        let mut mem = base.clone();
        assert!(mem.put(REC + ENGINE_AT as u32, &engine.to_le_bytes()));
        let mut gpr = regs(&mut rng);
        gpr[0] = REC;
        let cpu = run(&mut original, &mut mem, points::VARIANT_CATEGORY.base, gpr);
        assert_eq!(Some(cpu.gpr[0]), record::content_category(engine), "{ty:?}");
        assert_eq!(
            cpu.gpr[1],
            engine as u32 - 1,
            "{ty:?}: ECX the index less one"
        );
        assert_eq!(
            cpu.gpr[2..4],
            gpr[2..4],
            "{ty:?}: EDX and EBX are the caller's"
        );
        assert_eq!(cpu.gpr[5..], gpr[5..], "{ty:?}");
    }
    assert_eq!(
        record::content_category(SCRIPT_INDEX),
        GameType::Slayer.content_type()
    );
}

/// Where the bit stream's header and its words live.
const STREAM: u32 = 0x0063_0000;
const WORDS: u32 = 0x0064_0000;

/// Puts `bits` in memory as the game's stream.
fn seed_bits(mem: &mut Ram<'_>, bits: &Bits) {
    let mut head = [0u8; 0x18];
    head[0..4].copy_from_slice(&WORDS.to_le_bytes());
    head[4..8].copy_from_slice(&bits.size.to_le_bytes());
    head[0x10..0x14].copy_from_slice(&bits.pos.to_le_bytes());
    head[0x14] = u8::from(bits.error);
    assert!(mem.put(STREAM, &head));
    let words: Vec<u8> = bits.words.iter().flat_map(|w| w.to_le_bytes()).collect();
    assert!(mem.put(WORDS, &words));
}

/// The stream as the game left it.
fn bits_of(mem: &Ram<'_>, like: &Bits) -> Bits {
    let mut head = [0u8; 0x18];
    assert!(mem.get(STREAM, &mut head));
    let mut words = vec![0u8; like.words.len() * 4];
    assert!(mem.get(WORDS, &mut words));
    Bits {
        words: words
            .chunks_exact(4)
            .map(|w| u32::from_le_bytes(w.try_into().unwrap()))
            .collect(),
        size: like.size,
        pos: u32::from_le_bytes(head[0x10..0x14].try_into().unwrap()),
        error: head[0x14] != 0,
    }
}

/// A record of a retail type for the wire: its stock record with some
/// fields drawn anew inside their bits (the encoder formats a warning for
/// a value past them, and a 16-bit store it sign-extends past 0x7fff),
/// sometimes a new name of 0 to 32 units, and, when `noisy`, noise in the
/// spare bytes the wire does not carry (docs/game/game-options.md, "Spare
/// bytes").
fn wire_record(rng: &mut Rng, ty: GameType, noisy: bool) -> [u8; RECORD_SIZE] {
    let mut b = GameVariant::stock(ty).unwrap().to_bytes();
    if noisy {
        for range in [
            0x02..0x03,
            0x5c..0x74,
            0x8c..0xa4,
            0xb0..0xb4,
            0xb8..0xcc,
            0xd8..0xf0,
            0x10c..RECORD_SIZE,
        ] {
            for x in &mut b[range] {
                *x = rng.u32() as u8;
            }
        }
    }
    if rng.below(4) == 0 {
        b[0] = rng.below(2) as u8;
    }
    if rng.below(2) == 0 {
        let len = rng.below(33) as usize;
        for k in 0..32 {
            let u = if k < len {
                1 + rng.below(0xffff) as u16
            } else {
                0
            };
            let at = record::NAME_AT + 2 * k;
            b[at..at + 2].copy_from_slice(&u.to_le_bytes());
        }
    }
    // The description goes on the wire plus one, in 7 bits.
    if rng.below(3) == 0 {
        b[record::DESCRIPTION_AT] = rng.below(0x7f) as u8;
    }
    for (at, bytes, bits) in wire::fields(ty.engine()).unwrap() {
        if rng.below(4) != 0 {
            continue;
        }
        let mut v = rng.u32() & ((1u64 << bits) - 1) as u32;
        if bytes == 2 && bits == 16 {
            v &= 0x7fff;
        }
        b[at..at + bytes].copy_from_slice(&v.to_le_bytes()[..bytes]);
    }
    b
}

/// A stream for the encoder: noise in its words, a random start, and
/// sometimes too short for the record.
fn writer(rng: &mut Rng) -> Bits {
    let size = if rng.below(4) == 0 {
        4 + rng.below(40)
    } else {
        0x80 + rng.below(0x80)
    };
    let mut bits = Bits::new(size);
    if rng.below(2) == 0 {
        for w in &mut bits.words {
            *w = rng.u32();
        }
    }
    bits.pos = rng.below(40).min(size * 8);
    bits
}

/// Runs the original encode of the record at `REC` into the stream.
fn run_encode(original: &mut lifted::Original, mem: &mut Ram<'_>, rng: &mut Rng) {
    assert!(mem.put(STACK + 4, &STREAM.to_le_bytes()));
    assert!(mem.put(STACK + 8, &REC.to_le_bytes()));
    let mut gpr = regs(rng);
    gpr[4] = STACK;
    assert!(mem.put(STACK, &HARNESS_EXIT.to_le_bytes()));
    let mut cpu = Cpu {
        gpr,
        eip: points::VARIANT_ENCODE.base,
        eflags: 0x202,
        ..Cpu::default()
    };
    cpu.flags_valid = u32::MAX;
    lifted::code::run_until(original, &mut cpu, mem, STEPS, |c| c.eip == HARNESS_EXIT)
        .unwrap_or_else(|e| panic!("encode: {e}"));
    assert_eq!(cpu.gpr[4], STACK + 12, "the encode pops its two arguments");
}

/// Runs the original decode from the stream into the record at `REC`: AL.
fn run_decode(original: &mut lifted::Original, mem: &mut Ram<'_>, rng: &mut Rng) -> u32 {
    let mut gpr = regs(rng);
    gpr[0] = REC; // eax
    gpr[1] = STREAM; // ecx
    run(original, mem, points::VARIANT_DECODE.base, gpr).gpr[0] & 0xff
}

#[test]
fn the_encode_writes_what_the_model_writes_for_every_retail_index() {
    let image = Image::require();
    let mut original = image.original();
    let base = Ram::new(image);
    let mut rng = Rng(0x0007_cc50);
    for n in 0..1500 {
        let ty = GameType::ALL[rng.below(GameType::ALL.len() as u32) as usize];
        let r = wire_record(&mut rng, ty, true);
        let start = writer(&mut rng);
        let mut want = start.clone();
        wire::encode(&r, &mut want).unwrap();

        let mut mem = base.clone();
        mem.set_record(&r);
        seed_bits(&mut mem, &start);
        run_encode(&mut original, &mut mem, &mut rng);
        assert_eq!(bits_of(&mem, &start), want, "{n} {ty:?}");
        assert_eq!(mem.record(), r, "{n}: the encode reads only");
    }
}

#[test]
fn the_decode_reads_what_the_model_reads_for_every_retail_index() {
    let image = Image::require();
    let mut original = image.original();
    let base = Ram::new(image);
    let mut rng = Rng(0x0007_d520);
    let (mut kept, mut refused, mut garbage) = (0, 0, 0);
    for n in 0..1500 {
        // A stream the encoder made, or noise.
        let stream = if rng.below(3) == 0 {
            garbage += 1;
            let mut bits = Bits::new(0x20 + rng.below(0xe0));
            for w in &mut bits.words {
                *w = rng.u32();
            }
            bits
        } else {
            let ty = GameType::ALL[rng.below(GameType::ALL.len() as u32) as usize];
            let mut bits = Bits::new(0x100);
            wire::encode(&wire_record(&mut rng, ty, false), &mut bits).unwrap();
            if rng.below(8) == 0 {
                // Cut short.
                bits.size = rng.below(bits.pos / 8 + 1);
                bits.words.truncate(bits.size.div_ceil(4) as usize);
            }
            bits.pos = 0;
            bits
        };
        if wire::peek_engine(&stream) == SCRIPT_INDEX as u32 {
            continue;
        }
        // The record decoded into: zero, or noise the decode leaves where
        // it writes nothing (and the clean-up then mostly refuses).
        let mut out = [0u8; RECORD_SIZE];
        if rng.below(4) == 0 {
            for x in &mut out {
                *x = rng.u32() as u8;
            }
        }
        let mut want_bits = stream.clone();
        let mut want = out;
        let answer = wire::decode(&mut want, &mut want_bits);

        let mut mem = base.clone();
        mem.set_record(&out);
        seed_bits(&mut mem, &stream);
        let al = run_decode(&mut original, &mut mem, &mut rng);
        let what = format!("{n} engine {}", wire::peek_engine(&stream));
        assert_eq!(al == 1, answer == Decoded::Kept, "{what}");
        assert_eq!(mem.record(), want, "{what}: the record");
        assert_eq!(bits_of(&mem, &stream), want_bits, "{what}: the stream");
        match answer {
            Decoded::Kept => kept += 1,
            Decoded::Refused => refused += 1,
        }
    }
    assert!(
        kept > 100 && refused > 200 && garbage > 300,
        "{kept} {refused} {garbage}"
    );
}

#[test]
fn a_script_record_crosses_the_wire_as_slayer_does_less_its_type_area() {
    let image = Image::require();
    let mut original = image.original();
    let base = Ram::new(image);
    let mut rng = Rng(0x0006_0006);
    for n in 0..200 {
        let ty = GameType::ALL[rng.below(GameType::ALL.len() as u32) as usize];
        let mut r = wire_record(&mut rng, ty, false);
        r[ENGINE_AT..ENGINE_AT + 4].copy_from_slice(&SCRIPT_INDEX.to_le_bytes());
        r[TYPE_AREA].fill(0);
        let mut slayer = r;
        slayer[ENGINE_AT..ENGINE_AT + 4].copy_from_slice(&2i32.to_le_bytes());

        // The encode: ours for the script record, the game's for Slayer's.
        let start = Bits::new(0x100);
        let mut ours = start.clone();
        wire::encode(&r, &mut ours).unwrap();
        let mut mem = base.clone();
        mem.set_record(&slayer);
        seed_bits(&mut mem, &start);
        run_encode(&mut original, &mut mem, &mut rng);
        let game = bits_of(&mem, &start);
        assert_eq!(game.pos, ours.pos + 3, "{n}: Slayer's 3 type-area bits");
        let mut as_slayer = ours.clone();
        as_slayer.words[0] = (as_slayer.words[0] & !0xf) | 2;
        as_slayer.pos += 3;
        assert_eq!(game, as_slayer, "{n}: the same bits but the index");

        // The decode: ours of our stream, the game's of Slayer's.
        let mut stream = ours.clone();
        stream.pos = 0;
        let mut got = [0u8; RECORD_SIZE];
        let answer = wire::decode(&mut got, &mut stream.clone());
        let mut mem = base.clone();
        mem.set_record(&[0; RECORD_SIZE]);
        let mut game_stream = game.clone();
        game_stream.pos = 0;
        seed_bits(&mut mem, &game_stream);
        let al = run_decode(&mut original, &mut mem, &mut rng);
        let mut want = mem.record();
        assert_eq!(engine_of(&want), 2);
        want[ENGINE_AT..ENGINE_AT + 4].copy_from_slice(&SCRIPT_INDEX.to_le_bytes());
        assert_eq!(got, want, "{n}: Slayer's record but the index");
        assert_eq!(got[TYPE_AREA], [0; 0x1c]);
        let v = GameVariant::from_bytes(&got).unwrap();
        assert_eq!(answer == Decoded::Kept, check(&v).is_empty(), "{n}");
        assert_eq!(
            al == 1,
            answer == Decoded::Kept,
            "{n}: as the game keeps Slayer's"
        );

        // The game alone refuses the script stream.
        let mut mem = base.clone();
        seed_bits(&mut mem, &stream);
        assert_eq!(run_decode(&mut original, &mut mem, &mut rng), 0, "{n}");
    }
}
