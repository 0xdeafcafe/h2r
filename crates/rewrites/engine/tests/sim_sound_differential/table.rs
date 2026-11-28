//! The sound table's admission against the engine's (docs/engine/sound-instances.md,
//! slice 2). Each case builds one random sound tag, history, pool fill and
//! request twice: as guest records for `native_sound_impulse`'s body, whose
//! draws, chooser, class lookup and `datum_new` run as the lifted original,
//! and as `SoundTags` and a `Request` for `Sounds::admit`. Both must leave
//! the same seed, return the same handle and bind the same selection.

use super::*;
use engine::points::{RANDOM_SEEDS, SOUND_GLOBALS, SOUND_INSTANCES, TAG_ENTRIES};
use sim_sound::chooser::History;
use sim_sound::instances::{
    CAPACITY, Handle, Permutation, PitchRange, Playback, Rejected, Request, Rng, Scale, SoundTag,
    Sounds, TagIndex, TagTable,
};
use sim_sound::owners::{NullOwners, Reason};

const CASES: u32 = 4096;
const TAG: u32 = 0x1234_0005;
const ENTRIES: u32 = ARENA + 0x100;
const SOUND: u32 = ARENA + 0x400;
const REQUEST: u32 = ARENA + 0x800;
const GESTALT: u32 = ARENA + 0x1800;
const PLAYBACKS: u32 = ARENA + 0x1900;
const SCALES: u32 = ARENA + 0x1a00;
const BOUNDS: u32 = ARENA + 0x1b00;
const RANGES: u32 = ARENA + 0x2000;
const PERMUTATIONS: u32 = ARENA + 0x2800;
const BITS: u32 = ARENA + 0x3000;
const CHUNKS: u32 = ARENA + 0x3400;
const PROMOTIONS: u32 = ARENA + 0x3800;
const PROMOTION_ROWS: u32 = ARENA + 0x3900;
const GAME_GLOBALS: u32 = ARENA + 0x4000;
const GLOBALS_TAG: u32 = ARENA + 0x4100;
const CLASSES: u32 = ARENA + 0x4200;
const SEEDS: u32 = ARENA + 0x4400;
const SOUND_STATE: u32 = ARENA + 0x4500;
const POOL: u32 = ARENA + 0x4600;
const BITMAP: u32 = ARENA + 0x4700;
const ROWS: u32 = ARENA + 0x8000;
const ROW: u32 = 0xbc;
const RULES: usize = 4;

/// Run as the original: the draws, the chooser and everything it calls,
/// the class lookup, `datum_new` and the CRT conversion.
const LIFTED: &[u32] = &[0x125260, 0x1252f0, 0x219070, 0x221810, 0x16b840, 0x322b0c, 0x320560];

struct Rand(u64);

impl Rand {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 16) as u32
    }
    fn below(&mut self, n: u32) -> u32 {
        self.next() % n
    }
    fn one<T: Copy>(&mut self, of: &[T]) -> T {
        of[self.below(of.len() as u32) as usize]
    }
    fn chance(&mut self, in_n: u32) -> bool {
        self.below(in_n) == 0
    }
}

/// One case's world, in Rust terms.
struct Params {
    seed: u32,
    tag: SoundTag,
    tags: TagTable,
    /// Per absolute range; range `i`'s used set is at bit `32 * i`.
    history: Vec<History>,
    request: Request,
    /// Record `+0` as the engine reads it.
    record_flags: u32,
    fill: u32,
    hole: Option<u32>,
}

fn params(case: u32) -> Params {
    let mut r = Rand(0x9e37_79b9_7f4a_7c15 ^ (u64::from(case) << 20 | u64::from(case)));
    for _ in 0..4 {
        r.next();
    }
    let first_range = r.below(3) as i16;
    let range_count = 1 + r.below(3) as i8;
    let rows = first_range as usize + range_count as usize + 1;
    let mut tags = TagTable::default();
    let mut permutations = 0i16;
    for _ in 0..rows {
        let count = 1 + r.below(6) as i16;
        let lo = r.below(2400) as i16 - 1200;
        let hi = lo + r.below(1200) as i16;
        let natural = [lo - r.below(100) as i16, hi + r.below(100) as i16];
        tags.ranges.push(PitchRange { natural, playable: [lo, hi], first_permutation: permutations, count });
        permutations += count;
    }
    for _ in 0..permutations {
        let skip = r.one(&[0, 0, 0, 0x4000, 0x8000, 0xc000, 0xffff]);
        tags.permutations.push(Permutation { skip, length_ms: Some(100) });
    }
    let lo = r.below(1200) as i16 - 600;
    tags.scales.push(Scale { pitch: [lo, lo + r.below(1200) as i16] });
    tags.promotions.push(
        (0..RULES)
            .map(|_| if r.chance(3) { 0xffff } else { r.below(range_count as u32) as u16 })
            .collect(),
    );
    let pitch_lo = r.below(2400) as i16 - 1200;
    let tag = SoundTag {
        flags: if r.chance(2) { 8 } else { 0 },
        class_flags: (if r.chance(2) { 4 } else { 0 }) | (if r.chance(4) { 1 << 5 } else { 0 }),
        playback: Playback {
            gain: r.one(&[-80.0, -2.0, 0.0, 3.5]),
            gain_range: r.one(&[0.0, 6.0, -3.0, 0.5]),
            pitch: [pitch_lo, pitch_lo + r.below(1200) as i16 - 300],
        },
        first_range,
        range_count,
        scale: 0,
        promotion: if r.chance(2) { 0xff } else { 0 },
        distance: [0.0, 0.0],
    };
    tags.sounds.insert(TagIndex(TAG), tag);
    let history = tags
        .ranges
        .iter()
        .map(|row| {
            let count = row.count as u32;
            let last = if r.chance(3) { -1 } else { r.below(count) as i8 };
            let next = if r.chance(4) { r.below(count) as i8 } else { -1 };
            let used = r.next() & ((1 << count) - 1);
            History { last, next, used: if count > 1 { used } else { r.next() & 0xff } }
        })
        .collect();
    let mut flags = 0u32;
    let mut request = Request::new(TagIndex(TAG));
    request.channel = r.next() as u8;
    request.rule = if r.chance(2) { -1 } else { r.below(RULES as u32) as i32 };
    request.word_04 = r.next() as u16;
    request.level = r.one(&[-100.0, -60.0, 0.0, 10.0, f32::NAN]);
    for word in &mut request.source.0 {
        *word = r.next();
    }
    request.source.0[1] = r.one(&[0.0f32, 0.5, 1.0, -0.25]).to_bits();
    if r.chance(2) {
        flags |= 2;
        request.flags_08 = Some(r.next());
    }
    if r.chance(2) {
        flags |= 0x40;
        request.extra_8e = Some(r.next() as u16);
    }
    if r.chance(2) {
        flags |= 0x80;
        request.gain_base = Some(r.one(&[-70.0, 0.0, 2.0]));
    }
    if r.chance(2) {
        flags |= 0x100;
        request.extra_98 = Some(r.next());
    }
    if r.chance(2) {
        flags |= 0x200;
        request.extra_90 = Some(r.next() as u8);
    }
    if r.chance(4) {
        flags |= 0x400;
        request.selection = Some((r.below(range_count as u32) as u16, r.below(4) as u16));
    }
    let fill = r.one(&[0, 0, 1, 2, 7, CAPACITY - 1, CAPACITY]);
    let hole = (fill > 1 && r.chance(3)).then(|| r.below(fill));
    Params { seed: r.next(), tag, tags, history, request, record_flags: flags, fill, hole }
}

/// The used set's bytes for a range of `count`.
fn used_bytes(count: i16) -> u32 {
    (count as u32 + 7) >> 3
}

fn set(g: &mut Guest, words: &[(u32, u32)]) {
    for &(at, v) in words {
        g.set_u32(at, v).unwrap();
    }
}

fn guest(image: &Image, p: &Params) -> (Guest, Cpu) {
    let mut g = Guest::new();
    g.map_zeroed(0, RAM as u32, Permissions::RW).unwrap();
    let len = image.ram.bytes.len().min(RAM);
    g.write_bytes(0, &image.ram.bytes[..len], 1).unwrap();
    g.write_bytes(ARENA, &vec![0; 0x20000], 1).unwrap();
    g.write_bytes(STACK_BOTTOM, &[0xa5; 65536], 1).unwrap();
    let t = &p.tag;
    set(
        &mut g,
        &[
            (TAG_ENTRIES.base, ENTRIES),
            (ENTRIES + (TAG & 0xffff) * 16 + 8, SOUND),
            (ENTRIES + 16 + 8, GLOBALS_TAG),
            (GLOBALS_TAG + 4, CLASSES),
            (0x4e034c, GAME_GLOBALS),
            (GAME_GLOBALS + 0xc0, 1),
            (GAME_GLOBALS + 0xc4, GAME_GLOBALS + 0x100),
            (GAME_GLOBALS + 0x104, 1),
            (0x51ebd4, GESTALT),
            (GESTALT + 4, PLAYBACKS),
            (GESTALT + 12, SCALES),
            (GESTALT + 0x1c, BOUNDS),
            (GESTALT + 0x24, RANGES),
            (GESTALT + 0x2c, PERMUTATIONS),
            (GESTALT + 0x3c, BITS),
            (GESTALT + 0x44, CHUNKS),
            (GESTALT + 0x4c, PROMOTIONS),
            (PROMOTIONS + 4, PROMOTION_ROWS),
            (PLAYBACKS + 0x10, t.playback.gain.to_bits()),
            (PLAYBACKS + 0x14, t.playback.gain_range.to_bits()),
            (RANDOM_SEEDS.base, SEEDS),
            (SEEDS + 4, p.seed),
            (SOUND_GLOBALS.base, SOUND_STATE),
            (SOUND_STATE + 0x84, 1000),
            (SOUND_INSTANCES.base, POOL),
        ],
    );
    g.set_u8(SOUND, t.flags).unwrap();
    g.set_u8(SOUND + 2, 0).unwrap();
    g.set_u16(SOUND + 6, 0).unwrap();
    g.set_u16(SOUND + 8, t.first_range as u16).unwrap();
    g.set_u8(SOUND + 10, t.range_count as u8).unwrap();
    g.set_u8(SOUND + 11, t.scale as u8).unwrap();
    g.set_u8(SOUND + 12, t.promotion).unwrap();
    g.set_u8(CLASSES + 0xa, t.class_flags).unwrap();
    g.set_u16(PLAYBACKS + 0x18, t.playback.pitch[0] as u16).unwrap();
    g.set_u16(PLAYBACKS + 0x1a, t.playback.pitch[1] as u16).unwrap();
    let scale = p.tags.scales[0];
    g.set_u16(SCALES + 8, scale.pitch[0] as u16).unwrap();
    g.set_u16(SCALES + 10, scale.pitch[1] as u16).unwrap();
    for (i, row) in p.tags.ranges.iter().enumerate() {
        let i = i as u32;
        let (at, bounds) = (RANGES + i * 12, BOUNDS + i * 10);
        let h = p.history[i as usize];
        g.set_u16(at + 2, i as u16).unwrap();
        g.set_u8(at + 4, h.last as u8).unwrap();
        g.set_u8(at + 5, h.next as u8).unwrap();
        g.set_u16(at + 6, (i * 32) as u16).unwrap();
        g.set_u16(at + 8, row.first_permutation as u16).unwrap();
        g.set_u16(at + 10, row.count as u16).unwrap();
        for (off, v) in [(2, row.natural[0]), (4, row.natural[1]), (6, row.playable[0]), (8, row.playable[1])] {
            g.set_u16(bounds + off, v as u16).unwrap();
        }
        let bytes = h.used.to_le_bytes();
        g.write_bytes(BITS + i * 4, &bytes[..used_bytes(row.count) as usize], 1).unwrap();
    }
    for (i, perm) in p.tags.permutations.iter().enumerate() {
        g.set_u16(PERMUTATIONS + i as u32 * 16 + 2, perm.skip).unwrap();
    }
    for (rule, &range) in p.tags.promotions[0].iter().enumerate() {
        g.set_u16(PROMOTION_ROWS + rule as u32 * 16, range).unwrap();
    }
    // The pool: a datum array of 384 rows with `fill` taken in order and
    // one optional hole, as the table's pool holds after the same frees.
    let salt = |i: u32| 0xef73 + i as u16;
    let mut bitmap = [0u32; 12];
    for i in 0..p.fill {
        g.set_u16(ROWS + i * ROW, salt(i)).unwrap();
        bitmap[(i >> 5) as usize] |= 1 << (i & 31);
    }
    let (mut high, mut scan, mut active) = (p.fill, p.fill, p.fill);
    if let Some(j) = p.hole {
        g.set_u16(ROWS + j * ROW, 0).unwrap();
        bitmap[(j >> 5) as usize] &= !(1 << (j & 31));
        scan = scan.min(j);
        active -= 1;
        if j + 1 == high {
            high = j;
        }
    }
    for (i, w) in bitmap.iter().enumerate() {
        g.set_u32(BITMAP + i as u32 * 4, *w).unwrap();
    }
    g.write_bytes(POOL, b"sound instances\0", 1).unwrap();
    set(
        &mut g,
        &[
            (POOL + 0x20, CAPACITY),
            (POOL + 0x24, ROW),
            (POOL + 0x2c, 0x6440_7440),
            (POOL + 0x34, scan),
            (POOL + 0x38, high),
            (POOL + 0x3c, active),
            (POOL + 0x44, ROWS),
            (POOL + 0x48, BITMAP),
        ],
    );
    g.set_u8(POOL + 0x29, 1).unwrap();
    g.set_u8(POOL + 0x2a, 3).unwrap();
    g.set_u16(POOL + 0x40, salt(p.fill)).unwrap();
    // The request record and the entry's arguments.
    let q = &p.request;
    g.set_u32(REQUEST, p.record_flags).unwrap();
    g.set_u16(REQUEST + 4, q.word_04).unwrap();
    g.set_u32(REQUEST + 8, q.flags_08.unwrap_or(0)).unwrap();
    for (i, w) in q.source.0.iter().enumerate() {
        g.set_u32(REQUEST + 12 + i as u32 * 4, *w).unwrap();
    }
    g.set_u16(REQUEST + 0x8e, q.extra_8e.unwrap_or(0)).unwrap();
    g.set_u8(REQUEST + 0x90, q.extra_90.unwrap_or(0)).unwrap();
    g.set_u32(REQUEST + 0x94, q.gain_base.unwrap_or(0.0).to_bits()).unwrap();
    g.set_u32(REQUEST + 0x98, q.extra_98.unwrap_or(0)).unwrap();
    let (range, permutation) = q.selection.unwrap_or((0, 0));
    g.set_u16(REQUEST + 0x9c, range).unwrap();
    g.set_u16(REQUEST + 0x9e, permutation).unwrap();
    g.set_u32(STACK, EXIT).unwrap();
    for (i, w) in [TAG, u32::from(q.channel) | 0x1200, REQUEST, q.rule as u32].into_iter().enumerate() {
        g.set_u32(STACK + 4 + i as u32 * 4, w).unwrap();
    }
    let mut cpu = Cpu {
        eip: 0x126000,
        gpr: [0x19, 0x321, 0x54, 0x765, STACK, 0x98, 0xba, 0xdc],
        eflags: 0x893,
        ..Cpu::default()
    };
    cpu.x87.control = sim_sound::instances::FPU_CONTROL;
    (g, cpu)
}

/// Answer the call the native body left at: run it as the original, or
/// stand in for the device and runtime services the table does not model.
fn callout(code: &mut lifted::Original, c: &mut Cpu, g: &mut Guest, level: f32, context: &str) {
    let sp = c.gpr[4];
    let site = g.u32(sp).unwrap();
    if LIFTED.contains(&c.eip) {
        for _ in 0..200_000 {
            if let Flow::Return { target } = lifted::code::step(code, c, &mut Memory(g)).unwrap()
                && target == site
            {
                return;
            }
        }
        panic!("{context}: lifted {:#x} did not return", c.eip);
    }
    let pop = match c.eip {
        // The level for the request's scale: the request carries it.
        0x1251e0 => {
            c.gpr[0] = level.to_bits();
            8
        }
        // Source position fix-ups: nothing the table reads.
        0x2226b0 => 0,
        // Listener distance: the host's propagation delay, never the sim's.
        0x127e20 => {
            c.xmm[0] = 0;
            0
        }
        // The chunk cache: nothing cached, nothing pinned.
        0x218850 => {
            c.gpr[0] = 0;
            12
        }
        other => panic!("{context}: unexpected call {other:#x}"),
    };
    c.gpr[4] = sp + 4 + pop;
    c.eip = site;
}

struct Engine {
    handle: u32,
    seed: u32,
    row: Option<[u8; ROW as usize]>,
}

fn run_engine(image: &Image, code: &mut lifted::Original, table: &Table, p: &Params, context: &str) -> (Engine, Guest) {
    let (mut g, mut cpu) = guest(image, p);
    let mut pc = cpu.eip;
    for _ in 0..256 {
        let mut abi_cpu = from_interp(&cpu);
        let t = sim_objects::drive(&mut abi_cpu, &mut g, table, pc).expect(context);
        cpu = to_interp(&abi_cpu);
        cpu.eip = t.target_pc;
        match t.kind {
            abi::RETURN => {
                assert_eq!(t.target_pc, EXIT, "{context}");
                let handle = cpu.gpr[0];
                let row = (handle != u32::MAX).then(|| {
                    let mut row = [0u8; ROW as usize];
                    g.read_bytes(ROWS + (handle & 0xffff) * ROW, &mut row, 1).unwrap();
                    row
                });
                let seed = g.u32(SEEDS + 4).unwrap();
                return (Engine { handle, seed, row }, g);
            }
            abi::CALL => {
                callout(code, &mut cpu, &mut g, p.request.level, context);
                pc = cpu.eip;
            }
            _ => panic!("{context}: unexpected transfer {t:?}"),
        }
    }
    panic!("{context}: admission did not return");
}

fn run_table(p: &Params) -> (Result<Handle, Rejected>, Rng, Sounds) {
    let mut s = Sounds::new();
    let mut spare = Rng::new(1);
    let filler = Request { selection: Some((0, 0)), level: 1000.0, ..Request::new(TagIndex(TAG)) };
    let mut handles = Vec::new();
    for _ in 0..p.fill {
        handles.push(s.admit(&mut spare, &p.tags, filler, 0).expect("fill"));
    }
    if let Some(j) = p.hole {
        s.finish(handles[j as usize], Reason::Superseded, 0, &mut NullOwners).expect("hole");
    }
    for (i, h) in p.history.iter().enumerate() {
        s.chooser_mut().set_history(i as u16, *h);
    }
    let mut rng = Rng::new(p.seed);
    let result = s.admit(&mut rng, &p.tags, p.request, 0);
    (result, rng, s)
}

fn word(row: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(row[at..at + 4].try_into().unwrap())
}

fn half(row: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(row[at..at + 2].try_into().unwrap())
}

#[test]
fn the_table_draws_what_the_engines_admission_draws() {
    let Some(image) = Image::get() else { return };
    let mut code = image.original();
    let table = Table::new(
        [0x126000, 0x218f10, 0x218f50, 0x1286b0, 0x125e60, 0x1268e0, 0x126960]
            .iter()
            .map(|entry| engine::sim_sound::TABLE.at(*entry).unwrap().0),
    )
    .unwrap();
    let mut outcomes = [0u32; 4];
    for case in 0..CASES {
        let p = params(case);
        let context = format!("table admission case {case}");
        let (engine, g) = run_engine(image, &mut code, &table, &p, &context);
        let (result, rng, sounds) = run_table(&p);
        assert_eq!(rng.state(), engine.seed, "{context}: seed after");
        match result {
            Ok(handle) => {
                outcomes[0] += 1;
                assert_eq!(handle.raw(), engine.handle, "{context}: handle");
                let row = engine.row.expect("admitted");
                let i = sounds.get(handle).unwrap();
                assert_eq!(
                    (i.selection.range as u8, i.selection.permutation as u8),
                    (row[0x9c], row[0x9d]),
                    "{context}: selection"
                );
                assert_eq!(i.pitch.to_bits(), word(&row, 0x90), "{context}: pitch");
                assert_eq!(i.gain.to_bits(), word(&row, 0x94), "{context}: gain");
                assert_eq!(i.seed, word(&row, 0x98), "{context}: seed word");
                for (n, w) in i.source.0.iter().enumerate() {
                    assert_eq!(*w, word(&row, 0x18 + n * 4), "{context}: source word {n}");
                }
                let o = i.options;
                assert_eq!(o.flags, half(&row, 4), "{context}: +4");
                assert_eq!(o.word_06, half(&row, 6), "{context}: +6");
                assert_eq!(o.word_08, word(&row, 8), "{context}: +8");
                assert_eq!([o.rule, o.channel], [row[0xa0], row[0xa1]], "{context}: rule, channel");
                assert_eq!(o.word_a2, half(&row, 0xa2), "{context}: +0xa2");
                assert_eq!(o.byte_a4, row[0xa4], "{context}: +0xa4");
            }
            Err(Rejected::Silent) => {
                outcomes[1] += 1;
                assert_eq!(engine.handle, u32::MAX, "{context}");
            }
            Err(Rejected::Full) => {
                outcomes[2] += 1;
                assert_eq!(engine.handle, u32::MAX, "{context}");
            }
            Err(other) => panic!("{context}: the table refused {other:?}"),
        }
        // The chooser's history: every range's last, next and used set.
        for (n, range) in p.tags.ranges.iter().enumerate() {
            let at = RANGES + n as u32 * 12;
            let h = sounds.chooser().history(n as u16);
            assert_eq!(h.last as u8, g.u8(at + 4).unwrap(), "{context}: range {n} last");
            assert_eq!(h.next as u8, g.u8(at + 5).unwrap(), "{context}: range {n} next");
            let mut bytes = [0u8; 4];
            let len = used_bytes(range.count) as usize;
            g.read_bytes(BITS + n as u32 * 4, &mut bytes[..len], 1).unwrap();
            let mask = if len >= 4 { u32::MAX } else { (1u32 << (len * 8)) - 1 };
            assert_eq!(h.used & mask, u32::from_le_bytes(bytes), "{context}: range {n} used");
        }
        if p.request.selection.is_none() && result.is_ok() {
            outcomes[3] += 1;
        }
    }
    // Every outcome is exercised.
    let [admitted, silent, full, chosen] = outcomes;
    assert!(admitted > 1000 && silent > 100 && full > 100 && chosen > 500, "{outcomes:?}");
}
