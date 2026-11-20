//! The globals' player representation block (`h1convert::globals`) against
//! the retail XBE and the disc: addresses, instruction shapes, tag names and
//! counts only; no bytes are kept.
//!
//! ```sh
//! H2_XBE=... H2_DISC=... cargo test -p h1convert --test it h2_globals:: -- --ignored --nocapture
//! ```

use std::fs::File;
use std::io::{BufReader, Cursor};

use h1convert::globals::{
    PLAYER_REPRESENTATION, PLAYER_REPRESENTATION_SIZE, REPRESENTATION_UNIT,
    REPRESENTATION_UNIT_WORD,
};
use iced_x86::{Decoder, DecoderOptions, Instruction, Mnemonic, OpKind};
use map::{FourCC, Map, TagId};

/// The instructions of `0x0014f040` up to its unit read.
fn player_unit_function() -> Vec<Instruction> {
    let bytes = std::fs::read(std::env::var("H2_XBE").expect("set H2_XBE")).unwrap();
    let x = xbe::Xbe::parse(&bytes).unwrap();
    let code = x.bytes_at(0x0014_f040, 0xa4).unwrap();
    let mut d = Decoder::with_ip(32, code, 0x0014_f040, DecoderOptions::NONE);
    d.iter().take_while(|i| i.ip() < 0x0014_f0e4).collect()
}

fn at(code: &[Instruction], ip: u32) -> &Instruction {
    code.iter()
        .find(|i| i.ip() == u64::from(ip))
        .unwrap_or_else(|| panic!("no instruction at {ip:#x}"))
}

fn disp(i: &Instruction) -> u32 {
    assert!(
        (0..i.op_count()).any(|n| i.op_kind(n) == OpKind::Memory),
        "{:#x}",
        i.ip()
    );
    i.memory_displacement32()
}

#[test]
#[ignore = "needs the retail default.xbe in $H2_XBE"]
fn the_player_unit_comes_from_the_globals_player_representations() {
    let c = player_unit_function();
    // The player's representation byte (players stride 0x21c, +0x88).
    assert_eq!(disp(at(&c, 0x0014_f040)), 0x004e_8c24);
    assert_eq!(at(&c, 0x0014_f04e).immediate32(), 0x21c);
    assert_eq!(disp(at(&c, 0x0014_f05b)), 0x88);
    // The globals, the block's count, clamped to its last element.
    assert_eq!(disp(at(&c, 0x0014_f062)), 0x004e_034c);
    assert_eq!(disp(at(&c, 0x0014_f077)) as usize, PLAYER_REPRESENTATION);
    // Multiplayer (game mode 2): 0 becomes 2, 1 becomes 3.
    assert_eq!(disp(at(&c, 0x0014_f087)), 0x004e_6948);
    let cmp = at(&c, 0x0014_f08d);
    assert_eq!((disp(cmp), cmp.immediate8to32()), (8, 2));
    assert_eq!(at(&c, 0x0014_f097).immediate32(), 2);
    assert_eq!(at(&c, 0x0014_f0a3).immediate32(), 3);
    // The element: address, stride, the unit's datum and the word beside.
    assert_eq!(
        disp(at(&c, 0x0014_f0b6)) as usize,
        PLAYER_REPRESENTATION + 4
    );
    let imul = at(&c, 0x0014_f0bc);
    assert_eq!(imul.mnemonic(), Mnemonic::Imul);
    assert_eq!(imul.immediate32() as usize, PLAYER_REPRESENTATION_SIZE);
    assert_eq!(disp(at(&c, 0x0014_f0c2)) as usize, REPRESENTATION_UNIT + 4);
    assert_eq!(disp(at(&c, 0x0014_f0c9)) as usize, REPRESENTATION_UNIT_WORD);
}

fn h2_maps() -> Vec<(String, Map<Cursor<Vec<u8>>>)> {
    let path = std::env::var("H2_DISC").expect("set H2_DISC");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(path).unwrap())).unwrap();
    let mut out = Vec::new();
    for e in iso.entries().unwrap() {
        let Some(name) = e
            .path
            .rsplit(['/', '\\'])
            .next()
            .and_then(|n| n.strip_suffix(".map"))
            .map(str::to_owned)
        else {
            continue;
        };
        if ["shared", "single_player_shared", "mainmenu"].contains(&name.as_str()) {
            continue;
        }
        let m = Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap();
        out.push((name, m));
    }
    out
}

#[test]
#[ignore = "reads the Halo 2 disc ($H2_DISC)"]
fn every_disc_map_names_its_player_bipeds_there() {
    let mut multiplayer = 0;
    for (name, mut m) in h2_maps() {
        let g = m.globals().unwrap().clone();
        let base = g.location.unwrap().address;
        let b = m.read_tag_bytes(g.id).unwrap();
        let w = |at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
        assert_eq!(w(PLAYER_REPRESENTATION), 4, "{name}");
        let first = (w(PLAYER_REPRESENTATION + 4) - base) as usize;
        let units: Vec<(FourCC, Option<String>)> = (0..4)
            .map(|e| {
                let at = first + e * PLAYER_REPRESENTATION_SIZE;
                assert_eq!(w(at + REPRESENTATION_UNIT_WORD), 0, "{name} {e}");
                let id = TagId(w(at + REPRESENTATION_UNIT + 4));
                let tag = m.tags().iter().find(|t| t.id == id);
                if let Some(t) = tag {
                    assert_eq!(t.group.as_u32(), w(at + REPRESENTATION_UNIT), "{name} {e}");
                }
                (
                    FourCC(w(at + REPRESENTATION_UNIT).to_be_bytes()),
                    tag.and_then(|t| t.name.clone()),
                )
            })
            .collect();
        let last = |n: &Option<String>| {
            n.as_deref()
                .map(|n| n.rsplit('\\').next().unwrap_or(n).to_owned())
        };
        if units[2].1.is_some() {
            multiplayer += 1;
            assert_eq!(
                last(&units[2].1).as_deref(),
                Some("masterchief_mp"),
                "{name}"
            );
            assert_eq!(last(&units[3].1).as_deref(), Some("elite_mp"), "{name}");
            assert!(units[0].1.is_none() && units[1].1.is_none(), "{name}");
        } else {
            // Campaign: the Master Chief's (0) or the Arbiter's (1) levels.
            assert!(
                units[0].1.is_some() || units[1].1.is_some(),
                "{name}: no campaign player"
            );
            assert!(units[3].1.is_none(), "{name}");
        }
        eprintln!(
            "{name}: {:?}",
            units.iter().map(|(_, n)| last(n)).collect::<Vec<_>>()
        );
    }
    assert!(multiplayer >= 10, "{multiplayer} multiplayer maps");
}
