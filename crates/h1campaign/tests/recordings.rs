//! Recorded-animation event streams on the owner's Halo 1 disc
//! (`$H1_ISO`): every stream walked as each game's player reads it. Prints
//! counts and offsets only; nothing is written to disk.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, Cursor};

use h1campaign::recordings::{self, Game};

const LEVELS: [&str; 10] = [
    "a10", "a30", "a50", "b30", "b40", "c10", "c20", "c40", "d20", "d40",
];

fn disc(var: &str, name: &str) -> String {
    std::env::var(var)
        .unwrap_or_else(|_| format!("{}/../../disc/{name}", env!("CARGO_MANIFEST_DIR")))
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
    for level in LEVELS {
        let h1 = h1_level(&mut iso, level);
        let original = h1campaign::cutscenes::read_recordings(&h1).unwrap();
        let mut recs = original.clone();
        let report = recordings::for_halo2(&mut recs);
        if !report.stilled.is_empty() {
            failed.push(format!("{level}: held still {:?}", report.stilled));
        }
        for (r, o) in recs.iter().zip(&original) {
            total += 1;
            let cv = r.control_version();
            let w1 = recordings::walk(Game::Halo1, cv, &o.stream).unwrap();
            if recordings::walk(Game::Halo2, cv, &o.stream).is_err() {
                raw_bad += 1;
            }
            for e in &w1.events {
                *kinds.entry(e.kind).or_default() += 1;
                let p = &o.stream[e.at + e.head_len..e.at + e.size()];
                match e.kind {
                    4 => *flags.entry(u16::from_le_bytes([p[0], p[1]])).or_default() += 1,
                    5 => *weapons.entry(i16::from_le_bytes([p[0], p[1]])).or_default() += 1,
                    _ => {}
                }
            }
            *flags
                .entry(u16::from_le_bytes([o.stream[2], o.stream[3]]))
                .or_default() += 1;
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
            if w1.ticks + 1 != r.ticks() as u32 {
                println!(
                    "{level} {}: {} ticks walked, element says {}",
                    r.name,
                    w1.ticks,
                    r.ticks()
                );
            }
        }
        println!(
            "{level}: {} recordings converted, {} animation-state events to delays",
            report.converted, report.animation_events
        );
    }
    println!("{total} recordings; {raw_bad} of them unconverted would leave Halo 2's table");
    println!("Halo 1 event types: {kinds:?}");
    println!("weapon indices: {weapons:?}");
    for (f, n) in &flags {
        println!("control flags {f:016b} x{n}");
    }
    assert!(raw_bad > 0, "the unconverted streams should show the fault");
    assert!(failed.is_empty(), "{failed:#?}");
}
