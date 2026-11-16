//! Checks against the user's own disc and XBE (`$H2_XBE`, the ISO beside
//! it or `$H2_ISO`). Ignored: nothing of the disc is in the repository.

use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;

fn iso() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("H2_ISO") {
        return Some(p.into());
    }
    let xbe = PathBuf::from(std::env::var("H2_XBE").ok()?);
    Some(xbe.parent()?.join("halo2.iso"))
}

pub fn mainmenu() -> map::Map<BufReader<File>> {
    let path = iso().expect("set H2_XBE or H2_ISO");
    let mut x = xiso::Xiso::open(BufReader::new(File::open(&path).unwrap())).unwrap();
    let e = x.find("/maps/mainmenu.map").unwrap();
    let base = x.partition_offset() + u64::from(e.sector) * xiso::SECTOR;
    map::Map::from_region(BufReader::new(File::open(&path).unwrap()), base, e.size.into()).unwrap()
}

/// The schema from the retail tag: every definition has a place in the
/// record, the type pages agree with `fallback_owner`, every type has
/// defaults, and every built-in, built the way the game builds it, passes
/// the clean-up and offers only values its definitions list.
#[test]
#[ignore = "needs the user's disc"]
fn schema_from_the_disc() {
    use variants::record::{self, GameType};
    let mut m = mainmenu();
    let s = variants::Schema::from_map(&mut m, tags::Language::English).unwrap();
    assert!(!s.pages.is_empty());
    for (id, def) in &s.settings {
        assert!(record::field(*id).is_some(), "setting {} has no field", id.0);
        assert!(!def.choices.is_empty(), "setting {} offers nothing", id.0);
        if std::env::var("VERBOSE").is_ok() {
            println!("{:3} {:?} / {:?} ({} choices)", id.0, def.title, def.help, def.choices.len());
        }
    }
    for p in &s.pages {
        if (13..=21).contains(&p.id) {
            let ty = GameType::from_engine(p.id as i32 - 12);
            for id in &p.settings {
                assert_eq!(record::fallback_owner(*id), Some(ty), "setting {} on page {}", id.0, p.id);
            }
        }
    }
    for ty in GameType::ALL {
        assert!(!s.defaults[&ty].is_empty(), "{ty} has no defaults");
        let v = s.new_variant(ty);
        assert!(s.pages_for(ty).len() >= 5, "{ty}");
        assert!(record::check(&v).is_empty() || ty == GameType::Ctf || ty == GameType::Assault, "{ty}: {:?}", record::check(&v));
    }
    assert!(s.builtins.len() >= 30);
    for b in &s.builtins {
        let v = s.build(b).unwrap();
        let issues = s.validate(&v);
        assert!(issues.is_empty(), "{}: {:?}", b.name, issues.iter().map(|i| i.to_string()).collect::<Vec<_>>());
    }
}

/// `record::field` against the setter's descriptors in the XBE: each of
/// the 0x70 table entries at `0x470828` is a small function that stores a
/// kind, an offset and a bit or a float scale through its three pointer
/// arguments.
#[test]
#[ignore = "needs the user's XBE"]
fn descriptors_match_the_xbe() {
    use iced_x86::{Decoder, DecoderOptions, Mnemonic, OpKind, Register};
    use variants::record::{Kind, SettingId, field};
    let data = std::fs::read(std::env::var("H2_XBE").expect("set H2_XBE")).unwrap();
    let x = xbe::Xbe::parse(&data).unwrap();
    const TABLE: u32 = 0x0047_0828;
    for k in 0..0x70u32 {
        let fp = x.read_u32(TABLE + 4 * k).unwrap();
        let want = field(SettingId(k));
        if fp == 0 {
            assert_eq!(want, None, "setting {k}");
            continue;
        }
        let code = x.bytes_at(fp, 64).unwrap();
        let (mut arg, mut eax_one) = (0u32, false);
        let mut out = [None::<u32>; 3];
        let mut scale_one = false;
        for i in Decoder::with_ip(32, code, fp.into(), DecoderOptions::NONE).iter().take(16) {
            match i.mnemonic() {
                Mnemonic::Ret => break,
                Mnemonic::Mov if i.op1_kind() == OpKind::Memory && i.memory_base() == Register::ESP => {
                    arg = i.memory_displacement32() / 4 - 1;
                }
                Mnemonic::Xor if i.op0_register() == i.op1_register() => eax_one = false,
                Mnemonic::Inc => eax_one = true,
                Mnemonic::Movss if i.op0_kind() == OpKind::Register => {
                    let v = x.read_u32(i.memory_displacement32()).unwrap();
                    scale_one = f32::from_bits(v) == 1.0;
                }
                Mnemonic::Movss => out[arg as usize] = Some(if scale_one { 1 } else { u32::MAX }),
                Mnemonic::Mov if i.op1_kind() == OpKind::Immediate32 => out[arg as usize] = Some(i.immediate32()),
                Mnemonic::Mov if i.op0_kind() == OpKind::Memory && eax_one => out[arg as usize] = Some(1),
                Mnemonic::And if i.op1_kind() == OpKind::Immediate8to32 => out[arg as usize] = Some(0),
                _ => {}
            }
        }
        let [Some(kind), Some(offset), Some(param)] = out else { panic!("setting {k}: {out:?}") };
        let got = match kind {
            0 => Kind::Bit16(param as u8),
            1 => Kind::Bit32(param as u8),
            2 => Kind::Bool8,
            3 | 4 | 5 => {
                assert_eq!(param, 1, "setting {k} has a scale");
                [Kind::U8, Kind::I16, Kind::I32][kind as usize - 3]
            }
            n => panic!("setting {k}: kind {n}"),
        };
        let want = want.unwrap_or_else(|| panic!("setting {k} is missing"));
        assert_eq!((want.offset, want.kind), (offset as usize, got), "setting {k}");
    }
}

/// The content types' file names and labels read from the XBE: every
/// variant type has a distinct ASCII file name and a label in each
/// language, and the listing can strip the label it builds.
#[test]
#[ignore = "needs the user's XBE"]
fn names_from_the_xbe() {
    use variants::save::{XbeNames, save_name};
    let data = std::fs::read(std::env::var("H2_XBE").expect("set H2_XBE")).unwrap();
    let x = xbe::Xbe::parse(&data).unwrap();
    let en = XbeNames::read(&x, tags::Language::English).unwrap();
    let mut files: Vec<&str> = Vec::new();
    for ty in variants::GameType::ALL {
        let f = en.file_name(ty).unwrap();
        assert!(!f.is_empty() && f.is_ascii(), "{ty}");
        assert!(!files.contains(&f));
        files.push(f);
        assert_eq!(en.game_type_of(f), Some(ty));
        let label = en.label(ty).unwrap();
        assert!(!label.is_empty(), "{ty}");
        assert_eq!(en.strip_label(&save_name(label, "Mine")), "Mine");
    }
    let mut differs = false;
    for l in tags::Language::ALL {
        let n = XbeNames::read(&x, l).unwrap();
        assert_eq!(n.file_names, en.file_names, "file names do not depend on the language");
        assert!(n.labels.iter().all(|s| !s.is_empty()), "{l:?}");
        differs |= n.labels != en.labels;
    }
    assert!(differs, "some language has its own labels");
}

/// Checks a saved variant the game wrote (`H2_SAVED_VARIANT` = the payload
/// file inside its save directory) against the signing key we assume for
/// our runtime. This is the in-game check docs/variants.md asks for.
#[test]
#[ignore = "needs a variant saved by the game"]
fn saved_variant_verifies() {
    let Ok(path) = std::env::var("H2_SAVED_VARIANT") else {
        eprintln!("H2_SAVED_VARIANT is not set; nothing to check");
        return;
    };
    let bytes = std::fs::read(path).unwrap();
    let f = variants::save::decode_file(&bytes, &variants::save::RUNTIME_KEY).unwrap();
    assert!(f.signed, "the signature does not match HMAC-SHA1 with a zero key");
    assert!(variants::record::check(&f.variant).is_empty());
}
