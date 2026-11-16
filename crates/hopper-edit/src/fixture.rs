//! Invented test data for the hopper tools (this crate's tests, h2-editor's
//! and h2-studio's): a salted synthetic `multiplayer_hopper.dat` image and
//! made-up variants and schemas. No game data.

use live_proto::hopper::{HOPPERS, Hopper, HopperFile, HopperText, MAX_RECORDS, RECORD};
use variants::record as vrecord;
use variants::schema::Builtin;
use variants::{GameType, GameVariant, Schema};

/// A synthetic file: hoppers 0..3 with a few records each, every slot
/// and descriptor salted with marker bytes in the regions we do not
/// model, a variant per record, and nine different language texts.
pub fn sample() -> HopperFile {
    let mut f = HopperFile::blank();
    // Salt everything with a pattern first, so unknown bytes are
    // nonzero everywhere.
    for (k, b) in f.image_mut().iter_mut().enumerate() {
        *b = (k.wrapping_mul(31) % 251) as u8;
    }
    // As on retail files, the raw name reads empty (the validated table
    // after it stays salted), and so does each record's string (the
    // variant after its first byte stays salted).
    for i in 0..HOPPERS {
        f.image_mut()[i * 0x614 + 0x51b] = 0;
        for j in 0..MAX_RECORDS {
            f.image_mut()[0x6140 + i * 0x15cb8 + 0x48 + j * RECORD + 0x24c] = 0;
        }
    }
    for i in 0..HOPPERS {
        // Pattern-salted hoppers have nonsense fields; give each a
        // plausible descriptor so loads are sane. Hoppers 4.. keep the
        // salted text and name regions untouched.
        let n = if i < 3 { 2 + i as u32 } else { 1 };
        let h = f.hopper(i).unwrap();
        f.set_hopper(
            i,
            &Hopper {
                hopper_type: 4,
                status: 1,
                record_count: n,
                name: h.name.clone(),
                ranges: [1, 8, 4],
                dwords: h.dwords.clone(),
            },
        )
        .unwrap();
        f.set_record_count(i, n).unwrap();
    }
    for i in 0..3 {
        f.set_text(
            i,
            &HopperText {
                title: format!("List {i}"),
                description: format!("Playlist number {i}"),
            },
        )
        .unwrap();
        for j in 0..(2 + i) {
            put_variant(
                &mut f,
                i,
                j,
                &format!("V{i}{j}"),
                if j % 2 == 0 { 2 } else { 1 },
                j % 2 == 1,
            );
            let mut r = f.record(i, j).unwrap();
            r.dwords[0] = 1 + j as u32;
            r.id = 10 * i as u32 + j as u32;
            f.set_record(i, j, &r).unwrap();
        }
    }
    // A non-English title slot that a rewrite would clobber.
    let at = 0x0c + 3 * 0x10;
    f.image_mut()[at..at + 5].copy_from_slice(b"Liste");
    f
}

/// Writes a variant name (UTF-16 at +0x24c+4), engine and team flag.
pub fn put_variant(f: &mut HopperFile, i: usize, j: usize, name: &str, engine: u32, team: bool) {
    let slot = 0x6140 + i * 0x15cb8 + 0x48 + j * RECORD + 0x24c;
    let img = f.image_mut();
    img[slot + 4..slot + 0x44].fill(0);
    for (k, u) in name.encode_utf16().enumerate() {
        img[slot + 4 + 2 * k..][..2].copy_from_slice(&u.to_le_bytes());
    }
    img[slot + 0x44..][..4].copy_from_slice(&engine.to_le_bytes());
    let flags = u32::from_le_bytes(img[slot + 0x48..][..4].try_into().unwrap());
    img[slot + 0x48..][..4].copy_from_slice(&((flags & !1) | team as u32).to_le_bytes());
}

/// A variant the clean-up accepts, with every byte it allows set to
/// something recognisable.
pub fn good_variant(name: &str) -> GameVariant {
    let mut v = GameVariant::blank(GameType::Slayer);
    v.set_name(name).unwrap();
    v.set_builtin_flag(true);
    v.set_description(7);
    let mut b = v.to_bytes();
    b[0x48] = 1; // team play
    b[0x50] = 25; // score to win
    b[0x58] = 2; // a 0..2 field: lands on the record's second dword
    let v = GameVariant::from_bytes(&b).unwrap();
    assert!(vrecord::check(&v).is_empty(), "{:?}", vrecord::check(&v));
    v
}

/// The fallback schema with two invented built-ins (the variant picker's
/// tests).
pub fn schema() -> Schema {
    let mut s = Schema::fallback();
    s.builtins = vec![
        Builtin {
            name: "Alpha\u{e001}".into(),
            game_type: GameType::Slayer,
            description: 3,
            pairs: Vec::new(),
        },
        Builtin {
            name: "Bravo".into(),
            game_type: GameType::King,
            description: 4,
            pairs: Vec::new(),
        },
    ];
    s
}
