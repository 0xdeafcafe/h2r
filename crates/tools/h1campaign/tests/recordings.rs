//! Recorded-animation event streams on the owner's Halo 1 disc
//! (`$H1_ISO`): every stream walked as each game's player reads it. Prints
//! counts and offsets only; nothing is written to disk.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, Cursor};

use h1campaign::recordings::{self, Game};

// Owner's Xbox campaign disc: (level, recording rows, initial + type-4 flag words).
const LEVELS: [(&str, usize, usize); 10] = [
    ("a10", 99, 206),
    ("a30", 63, 309),
    ("a50", 34, 120),
    ("b30", 34, 182),
    ("b40", 24, 107),
    ("c10", 51, 389),
    ("c20", 1, 4),
    ("c40", 12, 30),
    ("d20", 20, 61),
    ("d40", 11, 30),
];

fn disc(var: &str, name: &str) -> String {
    std::env::var(var)
        .unwrap_or_else(|_| format!("{}/../../../disc/{name}", env!("CARGO_MANIFEST_DIR")))
}

fn h1_level(iso: &mut xiso::Xiso<BufReader<File>>, name: &str) -> h1map::H1Map {
    let d = h1map::disc::Disc::open(iso).unwrap();
    let e = d.map(name).unwrap().clone();
    h1map::H1Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap()
}

fn h1_iso() -> xiso::Xiso<BufReader<File>> {
    xiso::Xiso::open(BufReader::new(
        File::open(disc("H1_ISO", "halo1.iso")).unwrap(),
    ))
    .unwrap()
}

fn read_xbe(iso: &mut xiso::Xiso<BufReader<File>>) -> Vec<u8> {
    let e = iso.find("default.xbe").unwrap();
    iso.read(&e).unwrap()
}

fn word(x: &xbe::Xbe, va: u32) -> u32 {
    u32::from_le_bytes(x.bytes_at(va, 4).unwrap().try_into().unwrap())
}

/// The decoder facts `h1campaign::recordings` is built on, checked against
/// both games' executables: the version table picks the version-4 decoder
/// pair, the event loop dispatches `header & 3` for the delay and
/// `header >> 2` through the handler table without a bound, the table
/// holds the handlers the payload lengths come from, and the word after it
/// is not code.
#[test]
#[ignore = "reads disc/halo1.iso and disc/halo2.iso"]
fn decoder_tables_match_the_executables() {
    let h2 = read_xbe(
        &mut xiso::Xiso::open(BufReader::new(
            File::open(disc("H2_DISC", "halo2.iso")).unwrap(),
        ))
        .unwrap(),
    );
    let h1 = read_xbe(&mut h1_iso());
    for (game, bytes, versions, (pair, decoder), table, handlers) in [
        (
            Game::Halo2,
            &h2,
            recordings::HALO2_VERSIONS,
            recordings::HALO2_DECODER,
            recordings::HALO2_HANDLER_TABLE,
            &recordings::HALO2_HANDLERS[..],
        ),
        (
            Game::Halo1,
            &h1,
            recordings::HALO1_VERSIONS,
            recordings::HALO1_DECODER,
            recordings::HALO1_HANDLER_TABLE,
            &recordings::HALO1_HANDLERS[..],
        ),
    ] {
        let x = xbe::Xbe::parse(bytes).unwrap();
        assert_eq!(word(&x, versions + 3 * 4), pair, "{game:?} version 4");
        assert_eq!([word(&x, pair), word(&x, pair + 4)], decoder, "{game:?}");
        // The loop: `mov dl,[eax]; xor ecx,ecx; mov cl,dl; and ecx,3;
        // jmp [ecx*4+..]`, then `movzx ecx,byte [eax]; shr ecx,2;
        // mov reg,[ecx*4+table]`.
        let code = x.bytes_at(decoder[1], 0x80).unwrap();
        let find = |pat: &[u8]| code.windows(pat.len()).position(|w| w == pat);
        assert!(
            find(&[
                0x8a, 0x10, 0x33, 0xc9, 0x8a, 0xca, 0x83, 0xe1, 0x03, 0xff, 0x24, 0x8d
            ])
            .is_some(),
            "{game:?} delay dispatch"
        );
        let shr = find(&[0x0f, 0xb6, 0x08, 0xc1, 0xe9, 0x02]).expect("type dispatch");
        assert_eq!(
            code[shr + 6..shr + 13][3..],
            table.to_le_bytes(),
            "{game:?} handler table operand"
        );
        let words: Vec<u32> = (0..=handlers.len() as u32)
            .map(|i| word(&x, table + 4 * i))
            .collect();
        assert_eq!(&words[..handlers.len()], handlers, "{game:?} handlers");
        assert_eq!(handlers.len(), recordings::handler_count(game) as usize);
        let past = words[handlers.len()];
        assert!(
            x.bytes_at(past, 1).is_err() || past < 0x10000,
            "{game:?}: the word past the table ({past:#x}) is not an address"
        );
    }
}

/// Every recording on the ten levels, rewritten as the converter writes it,
/// walks through Halo 2's player: each event's type has a handler in
/// Halo 2's table (no null and nothing past it), the stream ends on the
/// end event with nothing after, and it lasts as long as Halo 1's.
#[test]
#[ignore = "reads disc/halo1.iso"]
fn every_converted_recording_walks_in_halo_2() {
    let mut iso = h1_iso();
    let mut failed = Vec::new();
    let (mut total, mut raw_bad) = (0, 0);
    let mut kinds = BTreeMap::<u8, usize>::new();
    let mut flags = BTreeMap::<u16, usize>::new();
    let mut weapons = BTreeMap::<i16, usize>::new();
    for (level, expected_recordings, expected_flag_words) in LEVELS {
        let h1 = h1_level(&mut iso, level);
        let original = h1campaign::cutscenes::read_recordings(&h1).unwrap();
        assert_eq!(original.len(), expected_recordings, "{level} source rows");
        let mut recs = original.clone();
        let report = recordings::for_halo2(&mut recs).unwrap_or_else(|e| panic!("{level}: {e}"));
        assert_eq!(report.converted, expected_recordings, "{level} encoded rows");
        let mut flag_words = 0;
        let mut flagged_recordings = 0;
        for (r, o) in recs.iter().zip(&original) {
            total += 1;
            let cv = r.control_version;
            let w1 = recordings::walk(Game::Halo1, cv, &o.stream).unwrap();
            if recordings::walk(Game::Halo2, cv, &o.stream).is_err() {
                raw_bad += 1;
            }
            let initial_flags = u16::from_le_bytes([o.stream[2], o.stream[3]]);
            *flags.entry(initial_flags).or_default() += 1;
            let mut used_flags = initial_flags;
            flag_words += 1;
            for e in &w1.events {
                *kinds.entry(e.kind).or_default() += 1;
                let p = &o.stream[e.at + e.head_len..e.at + e.size()];
                match e.kind {
                    4 => {
                        let word = u16::from_le_bytes([p[0], p[1]]);
                        *flags.entry(word).or_default() += 1;
                        used_flags |= word;
                        flag_words += 1;
                    }
                    5 => *weapons.entry(i16::from_le_bytes([p[0], p[1]])).or_default() += 1,
                    _ => {}
                }
            }
            // H1 control bits 2, 3, 14 and 15 are absent from retail recordings.
            if used_flags & 0xc00c != 0 {
                flagged_recordings += 1;
            }
            *weapons
                .entry(i16::from_le_bytes([o.stream[4], o.stream[5]]))
                .or_default() += 1;
            let w2 = match recordings::walk(Game::Halo2, cv, &r.stream) {
                Ok(w) => w,
                Err(e) => {
                    failed.push(format!("{level} {}: {e}", r.name));
                    continue;
                }
            };
            for (source, dest) in w1.events.iter().zip(&w2.events) {
                assert_eq!(
                    (source.kind, source.delay), (dest.kind, dest.delay),
                    "{level}: {}", r.name
                );
                let h1_payload = &o.stream[source.at + source.head_len..source.at + source.size()];
                let h2_payload = &r.stream[dest.at + dest.head_len..dest.at + dest.size()];
                match source.kind {
                    2 => assert_eq!(
                        map::recording::expand_mode(h2_payload[0] as i8 as i32 as u32),
                        recordings::mode(h1_payload[0])
                    ),
                    4 => assert_eq!((h1_payload.len(), h2_payload.len()), (2, 4)),
                    _ => assert_eq!(h1_payload, h2_payload, "{level}: {}", r.name),
                }
            }
            for e in &w2.events {
                let h = recordings::HALO2_HANDLERS.get(e.kind as usize).copied();
                if e.kind > recordings::END && h.unwrap_or(0) == 0 {
                    failed.push(format!(
                        "{level} {}: type {} at {:#x}",
                        r.name, e.kind, e.at
                    ));
                }
            }
            if w2.trailing != 0 || w2.ticks != w1.ticks || w2.events.len() != w1.events.len() {
                failed.push(format!(
                    "{level} {}: {} trailing, {} ticks for {}, {} events for {}",
                    r.name,
                    w2.trailing,
                    w2.ticks,
                    w1.ticks,
                    w2.events.len(),
                    w1.events.len()
                ));
            }
            assert_eq!(w1.ticks + 1, r.ticks as u32, "{level}: {}", r.name);
            // Throttle, all three vectors, version-specific scalar fields
            // and the trailer follow the wider H2 prefix, in that order.
            let h1_init = recordings::prologue_len(Game::Halo1, cv);
            let h2_init = recordings::prologue_len(Game::Halo2, cv);
            assert_eq!(&o.stream[8..h1_init], &r.stream[16..h2_init]);
            assert_eq!(
                u32::from_le_bytes(r.stream[..4].try_into().unwrap()),
                recordings::mode(o.stream[0])
            );
        }
        assert_eq!(flag_words, expected_flag_words, "{level} source flag words");
        assert_eq!(flagged_recordings, 0, "{level} recordings using mask 0xc00c");
        println!(
            "{level}: {} recordings converted, {} timed mode changes, {flag_words} source flag words, {flagged_recordings} recordings using mask 0xc00c",
            report.converted, report.animation_events
        );
    }
    println!("{total} recordings; {raw_bad} of them unconverted would leave Halo 2's table");
    println!("Halo 1 event types: {kinds:?}");
    println!("weapon indices: {weapons:?}");
    for (f, n) in &flags {
        println!("control flags {f:016b} x{n}");
    }
    assert_eq!(total, 349);
    assert_eq!(kinds.get(&2), Some(&73));
    assert_eq!(raw_bad, 189);
    assert!(failed.is_empty(), "{failed:#?}");
}

/// Check the compiler mapping and the native calls implementing each outcome.
#[test]
#[ignore = "reads disc/halo1.iso and disc/halo2.iso"]
fn script_calls_reach_the_native_recording_player() {
    let h1 = read_xbe(&mut h1_iso());
    let mut h2_iso = xiso::Xiso::open(BufReader::new(
        File::open(disc("H2_DISC", "halo2.iso")).unwrap(),
    ))
    .unwrap();
    let h2 = read_xbe(&mut h2_iso);
    let s = h1campaign::Scripting::new(&h1, &h2, &[]).unwrap();
    let x = xbe::Xbe::parse(&h2).unwrap();
    let catalog = hs::Catalog::from_xbe(&x).unwrap();
    let call_target = |at: u32| {
        assert_eq!(x.bytes_at(at, 1).unwrap()[0], 0xe8);
        (at + 5).wrapping_add(word(&x, at + 1))
    };
    for (name, index, call, target) in [
        ("recording_play", 138, 0x2a2d1c, 0x1fb360),
        ("recording_play_and_delete", 139, 0x2a2d7c, 0x1fb360),
        ("recording_play_and_hover", 140, 0x2a2ddc, 0x1fb360),
        ("recording_kill", 141, 0x2a2e30, 0x1fb760),
        ("recording_time", 142, 0x2a2e84, 0x1fb4c0),
        ("vehicle_hover", 621, 0x2aa4ae, 0x0f0fd0),
    ] {
        let source = s.catalog.index_of(name).unwrap();
        let tr = s.table.builtin(source).unwrap();
        assert_eq!(tr.kind, h1campaign::scripts::translate::Kind::Direct, "{name}");
        assert_eq!(tr.target.as_ref().unwrap().1.index, index, "{name}");
        let evaluate = catalog.function(index).unwrap().evaluate();
        assert!(evaluate <= call && call - evaluate < 0x60, "{name}");
        assert_eq!(call_target(call), target, "{name}");
    }
    for (push, flags) in [(0x2a2d1a, 0), (0x2a2d7a, 8), (0x2a2dda, 0x10)] {
        assert_eq!(x.bytes_at(push, 2).unwrap(), &[0x6a, flags]);
    }
    assert_eq!(call_target(0x1fb6d2), call_target(0x2aa4ae));
    assert_eq!(call_target(0x1fb630), 0x0c6de0);
}
