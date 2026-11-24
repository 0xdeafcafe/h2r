//! `rules::settings::TABLE` held to the record: the rules crate does not
//! depend on `variants`, so this is where each row's store is checked
//! against `record::field`, its carriers against `schema::applies` and its
//! range against the record clean-up's (docs/game/rules-variants.md, "The
//! settings block"). The disc test checks the stock record against the
//! retail editor's offers.

use rules::caps::CARRIERS;
use rules::settings::{ExtField, Kind, Row, Store, TABLE};
use variants::ext::{self, Ext};
use variants::record::{self, GameType, GameVariant, SettingId};
use variants::schema;

fn ty_of(carrier: u8) -> GameType {
    GameType::from_engine(i32::from(carrier))
}

/// Whether the record clean-up keeps `value` in setting `id` of the stock
/// record of `ty`: the setter takes it and the clean-up finds nothing at
/// the setting's offset.
fn kept(ty: GameType, id: SettingId, value: i32) -> bool {
    let mut v = GameVariant::stock(ty).unwrap();
    if v.set(id, value).is_err() {
        return false;
    }
    let at = record::field(id).unwrap().offset;
    record::check(&v).iter().all(|p| p.offset != at)
}

/// Whether the block takes `value` in an `hx` field.
fn ext_takes(field: ExtField, value: i32) -> bool {
    let Ok(n) = u16::try_from(value) else {
        return false;
    };
    let mut e = Ext::default();
    match field {
        ExtField::Gravity => e.gravity = n,
        ExtField::Damage => e.damage = n,
        ExtField::Shield => e.shield = n,
        ExtField::Health => e.health = n,
        ExtField::UnlimitedGrenades => {
            if n > 1 {
                return false;
            }
            e.unlimited_grenades = n == 1;
        }
    }
    let mut v = GameVariant::stock(GameType::Slayer).unwrap();
    v.set_ext(&e).is_ok()
}

/// The carriers that apply, by engine index.
fn carriers_where(f: impl Fn(u8) -> bool) -> Vec<u8> {
    CARRIERS.iter().map(|&(_, e)| e).filter(|&e| f(e)).collect()
}

/// The record's carrier rules on team play: CTF and Assault are always
/// team play and Juggernaut never. They are `record::refused`'s, not the
/// row's range (`record_for_refuses_what_the_game_would_not_keep`).
fn team_rule(row: &Row, ty: GameType) -> bool {
    row.name == "team_play"
        && matches!(ty, GameType::Ctf | GameType::Assault | GameType::Juggernaut)
}

#[test]
fn the_table_matches_the_record() {
    for row in TABLE {
        let (lo, hi) = row.kind.range();
        match row.store {
            Store::Ext(field) => {
                assert!(
                    row.carriers.is_empty(),
                    "{}: the block is every carrier's",
                    row.name
                );
                assert!(!row.is_record());
                let want = match field {
                    ExtField::Gravity => Some(ext::GRAVITY),
                    ExtField::Damage | ExtField::Shield | ExtField::Health => Some(ext::PERCENT),
                    ExtField::UnlimitedGrenades => None,
                };
                match want {
                    Some(r) => assert_eq!(
                        row.kind,
                        Kind::Percent {
                            lo: i32::from(*r.start()),
                            hi: i32::from(*r.end())
                        },
                        "{}",
                        row.name
                    ),
                    None => assert_eq!(row.kind, Kind::Bool, "{}", row.name),
                }
                assert!(ext_takes(field, lo) && ext_takes(field, hi), "{}", row.name);
                assert!(
                    !ext_takes(field, lo - 1) && !ext_takes(field, hi + 1),
                    "{}",
                    row.name
                );
            }
            Store::Setting(_) | Store::Score | Store::Paired { .. } => {
                assert!(row.is_record());
                for &(name, c) in CARRIERS {
                    let ty = ty_of(c);
                    let Some(s) = row.setting(c) else {
                        assert!(!row.applies(c), "{} on {name} has no setting", row.name);
                        continue;
                    };
                    assert!(row.applies(c), "{} on {name}", row.name);
                    let id = SettingId(s);
                    assert!(
                        record::field(id).is_some(),
                        "{}: setting {s} has no field",
                        row.name
                    );
                    assert!(
                        record::symbolic_name(id).is_some(),
                        "{}: setting {s} has no name of ours",
                        row.name
                    );
                    assert!(
                        schema::applies(id, ty),
                        "{}: setting {s} is not {ty}'s",
                        row.name
                    );
                    if team_rule(row, ty) {
                        continue;
                    }
                    assert!(
                        kept(ty, id, lo),
                        "{} on {name}: {lo} is the clean-up's",
                        row.name
                    );
                    assert!(
                        kept(ty, id, hi),
                        "{} on {name}: {hi} is the clean-up's",
                        row.name
                    );
                    assert!(
                        !kept(ty, id, lo - 1),
                        "{} on {name}: {} is kept",
                        row.name,
                        lo - 1
                    );
                    assert!(
                        !kept(ty, id, hi + 1),
                        "{} on {name}: {} is kept",
                        row.name,
                        hi + 1
                    );
                }
                // The carriers the row names are the ones its settings
                // apply to.
                let all: Vec<u8> = carriers_where(|_| true);
                match row.store {
                    Store::Setting(s) => assert_eq!(
                        carriers_where(|c| row.applies(c)),
                        carriers_where(|c| schema::applies(SettingId(s), ty_of(c))),
                        "{}",
                        row.name
                    ),
                    Store::Score => {
                        assert!(row.carriers.is_empty(), "{}", row.name);
                        assert_eq!(carriers_where(|c| row.applies(c)), all);
                    }
                    Store::Paired { ctf, assault } => {
                        assert_eq!(carriers_where(|c| row.applies(c)), [1, 9], "{}", row.name);
                        assert_eq!(
                            carriers_where(|c| schema::applies(SettingId(ctf), ty_of(c))),
                            [1]
                        );
                        assert_eq!(
                            carriers_where(|c| schema::applies(SettingId(assault), ty_of(c))),
                            [9]
                        );
                    }
                    Store::Ext(_) => unreachable!(),
                }
            }
        }
        // A named choice is in the range, and `none` is a kept value.
        if let Some(none) = row.none_value() {
            assert!(row.in_range(none), "{}", row.name);
        }
    }
}

/// The values slice 1 read from the record, checked where they live:
/// rounds is setting 0, the weapon values follow `0x19eda0` (0 the map's
/// own weapon, 1 none, 2 random; docs/game/game-options.md, "Starting
/// weapon None") and the three penalty and damage rows name the settings
/// `record::symbolic_name` gives those names.
#[test]
fn the_named_rows_are_the_records_settings() {
    let row = |name: &str| rules::settings::named(name).unwrap().1;
    let setting = |name: &str| match row(name).store {
        Store::Setting(s) => s,
        other => panic!("{name}: {other:?}"),
    };
    for (name, s, ours) in [
        ("rounds", 0, "Rounds"),
        ("suicide_penalty", 18, "Suicide penalty (seconds)"),
        ("extra_damage", 104, "Extra damage"),
        ("damage_resistance", 105, "Damage resistance"),
        ("starting_weapon", 89, "Starting weapon"),
        ("secondary_weapon", 94, "Secondary weapon"),
        ("team_count", 93, "Team count"),
    ] {
        assert_eq!(setting(name), s, "{name}");
        assert_eq!(record::symbolic_name(SettingId(s)), Some(ours), "{name}");
    }
    for name in ["starting_weapon", "secondary_weapon"] {
        let r = row(name);
        assert_eq!(r.choice("map_default"), Some(0), "{name}");
        assert_eq!(r.choice("none"), Some(1), "{name}");
        assert_eq!(r.choice("random"), Some(2), "{name}");
        assert_eq!(r.kind.range(), (0, 0x14), "{name}");
    }
    // The starting weapon's none is one of our added choices.
    assert!(
        variants::extra::OPTIONS
            .iter()
            .any(|e| e.setting == 89 && e.value == 1)
    );
    // The stock record starts with the map's weapons.
    let stock = GameVariant::stock(GameType::Slayer).unwrap();
    assert_eq!(stock.get(SettingId(89)), Some(0));
    assert_eq!(stock.get(SettingId(94)), Some(0));
}

/// The record of a script of the retail type `ty` whose settings block is
/// `block`.
fn record_of_block(ty: &str, block: &str) -> GameVariant {
    let src = format!("rules \"T\" 1\ntype {ty}\nuses {ty}\n\nsettings {{\n{block}\n}}\n");
    let ir = rules::compile(&src).unwrap_or_else(|e| panic!("{block}: {e}"));
    variants::portable::record_for(&ir, &[]).unwrap_or_else(|e| panic!("{block}: {e}"))
}

/// The rows the default matchmaking variants need
/// (docs/live/live-matchmaking.md, "6.8 The default set"): raw settings 12, 13, 14
/// and 25 (the hopper defaults writer clears 12 to 14), the second heavy
/// vehicle, and the CTF and Assault settings that sit in the type area:
/// CTF's bits `0xc` (flag at home, touch return) and Assault's `+0x14`
/// (the flag type). Each is written into the record's own bytes, reads
/// back through the table, and writing it leaves the rest of the stock
/// record as it was.
#[test]
fn the_matchmaking_rows_round_trip_through_a_script_record() {
    let stock = |ty| GameVariant::stock(ty).unwrap();
    // (row, carrier, script value, value read back, where it lands)
    #[allow(clippy::type_complexity)]
    let cases: [(&str, &str, &str, i32, Box<dyn Fn(&GameVariant) -> i64>); 7] = [
        ("setting_twelve", "slayer", "false", 0, Box::new(|v| i64::from(v.bytes()[0x48] >> 4 & 1))),
        ("setting_thirteen", "slayer", "true", 1, Box::new(|v| i64::from(v.bytes()[0x48] >> 5 & 1))),
        ("join_in_progress", "slayer", "2", 2, Box::new(|v| i64::from(v.bytes()[0x58]))),
        ("respawn_modifier", "oddball", "cycling", 1, Box::new(|v| i64::from(v.bytes()[0xa8]))),
        ("second_heavy_vehicle", "slayer", "none", 4, Box::new(|v| i64::from(v.bytes()[0xd1]))),
        ("ctf_flag_at_home", "ctf", "true", 1, Box::new(|v| i64::from(v.bytes()[0xf0] >> 2 & 1))),
        ("ctf_flag_type", "assault", "1", 1, Box::new(|v| i64::from(v.bytes()[0x104]))),
    ];
    for (name, carrier, text, value, at) in cases {
        let ty = ty_of(rules::caps::CARRIERS.iter().find(|c| c.0 == carrier).unwrap().1);
        let v = record_of_block(carrier, &format!("    {name} = {text}"));
        let row = rules::settings::named(name).unwrap().1;
        let engine = rules::caps::CARRIERS.iter().find(|c| c.0 == carrier).unwrap().1;
        assert_eq!(variants::portable::setting_value(&v, row, engine), Some(value), "{name}");
        assert_eq!(at(&v), i64::from(value), "{name}: its own bytes");
        // Nothing else changed but the name, the script reference and
        // the one setting (setting 12 starts on in the stock record).
        let mut was = stock(ty);
        let id = SettingId(row.setting(engine).unwrap());
        was.set(id, value).unwrap();
        was.set_name("T").unwrap();
        let mut now = v.clone();
        let mut e = now.ext();
        e.script = ext::Script::default();
        now.set_ext(&e).unwrap();
        assert_eq!(now.bytes()[..], was.bytes()[..], "{name}");
    }
    // CTF's two type-area bits, both at once: the stock 0x2 and 0xc.
    let v = record_of_block("ctf", "    ctf_flag_at_home = true\n    ctf_touch_return = true");
    assert_eq!(u32::from_le_bytes(v.bytes()[0xf0..0xf4].try_into().unwrap()), 0xe);
    // The values are the clean-up's: past the range is refused by name.
    for block in [
        "join_in_progress = 3",
        "respawn_modifier = 3",
        "second_heavy_vehicle = 5",
    ] {
        let src = format!("rules \"T\" 1\ntype slayer\nuses slayer\n\nsettings {{\n    {block}\n}}\n");
        assert!(rules::compile(&src).is_err(), "{block}");
    }
}

/// The names of the new rows are ours and unique, and each has a label.
#[test]
fn the_matchmaking_rows_have_names_and_record_labels() {
    for (name, setting, label) in [
        ("setting_twelve", 12, "Setting 12"),
        ("setting_thirteen", 13, "Setting 13"),
        ("join_in_progress", 14, "Join in progress"),
        ("respawn_modifier", 25, "Respawn modifier"),
        ("second_heavy_vehicle", 80, "Second heavy vehicle"),
    ] {
        let row = rules::settings::named(name).unwrap().1;
        assert_eq!(row.setting(2), Some(setting), "{name}");
        assert_eq!(record::symbolic_name(SettingId(setting)), Some(label), "{name}");
    }
    let r = rules::settings::named("respawn_modifier").unwrap().1;
    assert_eq!(
        [r.choice("inheritance"), r.choice("cycling"), r.choice("none")],
        [Some(0), Some(1), Some(2)]
    );
}

// ---- the disc ----

fn iso() -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("H2_ISO") {
        return Some(p.into());
    }
    let xbe = std::path::PathBuf::from(std::env::var("H2_XBE").ok()?);
    Some(xbe.parent()?.join("halo2.iso"))
}

fn mainmenu() -> map::Map<std::io::BufReader<std::fs::File>> {
    use std::fs::File;
    use std::io::BufReader;
    let path = iso().expect("set H2_XBE or H2_ISO");
    let mut x = xiso::Xiso::open(BufReader::new(File::open(&path).unwrap())).unwrap();
    let e = x.find("/maps/mainmenu.map").unwrap();
    let base = x.partition_offset() + u64::from(e.sector) * xiso::SECTOR;
    map::Map::from_region(
        BufReader::new(File::open(&path).unwrap()),
        base,
        e.size.into(),
    )
    .unwrap()
}

/// Each carrier's stock values are values its `sily` definitions offer,
/// with our added choices: else every script variant of the carrier is
/// left out of the list (docs/game/rules-variants.md, "Not verified").
#[test]
#[ignore = "needs the user's disc"]
fn the_stock_record_is_offered() {
    let mut m = mainmenu();
    let s = variants::Schema::from_map(&mut m, tags::Language::English).unwrap();
    let mut out = Vec::new();
    for ty in GameType::ALL {
        let v = GameVariant::stock(ty).unwrap();
        for issue in s.validate(&v) {
            if let schema::Issue::NotOffered { id, value } = issue {
                out.push(format!(
                    "{ty}: setting {} ({}) holds {value}",
                    id.0,
                    s.title(id)
                ));
            }
        }
    }
    assert!(
        out.is_empty(),
        "stock values the editor does not offer:\n{}",
        out.join("\n")
    );
}
