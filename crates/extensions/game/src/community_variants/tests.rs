use super::*;
use crate::game_variants::inject;
use ::rules::builtin::LISTED;
use variants::record::SettingId;

#[test]
fn the_built_in_line_names_the_focused_entrys_origin() {
    for n in [0, 38] {
        assert_eq!(origin_text(n), "Bungie Original");
    }
    for n in [DESCRIPTION_BASE, DESCRIPTION_BASE + listed().len() as u32 - 1] {
        assert_eq!(origin_text(n), "Community Classic");
    }
    for n in [94, 99, 100] {
        assert_eq!(origin_text(n), "Downloaded Content");
    }
    // Past the listed scripts and short of the store's: nobody's.
    assert_eq!(
        origin_text(DESCRIPTION_BASE + listed().len() as u32),
        "Bungie Original"
    );
    assert_eq!(origin_text(39), "Bungie Original");
    assert_eq!(built_in_line(name_id(0)), None);
}

#[test]
fn dedicated_host_status_uses_the_retail_matchmaking_status_strings() {
    for id in [0x0e00_0761, 0x1100_0762] {
        assert_eq!(
            matchmaking_status_text(id, || Some("Joining dedicated host (lhr)".into())).as_deref(),
            Some("Joining dedicated host (lhr)"),
        );
        assert_eq!(matchmaking_status_text(id, || None), None);
    }
    for id in [0x1400_043a, 0x0007_f8a0, name_id(0)] {
        assert_eq!(
            matchmaking_status_text(id, || panic!("unrelated string")),
            None,
        );
    }
}

/// Sparse little-endian memory; unwritten bytes read as missing.
#[derive(Default)]
struct Fake(HashMap<u32, u8>);
impl Mem for Fake {
    fn rd(&self, addr: u32, out: &mut [u8]) -> bool {
        for (i, b) in out.iter_mut().enumerate() {
            match self.0.get(&(addr + i as u32)) {
                Some(v) => *b = *v,
                None => return false,
            }
        }
        true
    }
    fn wr(&mut self, addr: u32, data: &[u8]) -> bool {
        for (i, b) in data.iter().enumerate() {
            self.0.insert(addr + i as u32, *b);
        }
        true
    }
}

const GOOF: u32 = 0x8000_0000;
const OLD: u32 = 0x8000_1000;
const TAGS: u32 = 0x8001_0000;

/// A goof tag with `n` invented retail entries and setting pages that
/// offer `offer`.
fn world(n: u32, offer: &[(u32, Vec<i32>)]) -> Fake {
    let mut m = Fake::default();
    m.wr(GOOF, &[0; 0x100]);
    for i in 0..n {
        let e: Vec<u8> = [0x0100_0000 + i, i % 7, 0, 0, i]
            .iter()
            .flat_map(|w: &u32| w.to_le_bytes())
            .collect();
        m.wr(OLD + i * ENTRY_SIZE, &e);
    }
    m.set_u32(GOOF + GOOF_BUILTINS, n);
    m.set_u32(GOOF + GOOF_BUILTINS + 4, OLD);
    // One page naming every sily, plus a null reference.
    let (page, refs) = (0x8000_2000, 0x8000_3000);
    m.set_u32(GOOF + GOOF_PAGES, 1);
    m.set_u32(GOOF + GOOF_PAGES + 4, page);
    m.wr(page, &[0; PAGE_SIZE as usize]);
    m.set_u32(page + 8, offer.len() as u32 + 1);
    m.set_u32(page + 0xc, refs);
    m.set_u32(refs, 0);
    m.set_u32(refs + 4, u32::MAX);
    for (k, (setting, values)) in offer.iter().enumerate() {
        let k = k as u32;
        let sily = 0x8010_0000 + k * 0x1000;
        m.set_u32(refs + (k + 1) * 8 + 4, 0xe000_0000 | (k + 5));
        m.set_u32(TAGS + (k + 5) * 16 + 8, sily);
        m.wr(sily, &[0; 0x24]);
        m.set_u32(sily, *setting);
        m.set_u32(sily + SILY_OPTIONS, values.len() as u32);
        m.set_u32(sily + SILY_OPTIONS + 4, sily + 0x100);
        for (j, v) in values.iter().enumerate() {
            let o = sily + 0x100 + j as u32 * OPTION_SIZE;
            m.set_u32(o, 0);
            m.set_u32(o + 4, *v as u32);
            m.set_u32(o + 8, 0x0700_0000);
        }
    }
    m
}

/// Every listed script's entry.
fn all_listed() -> Vec<&'static Listed> {
    listed()
        .iter()
        .map(|l| l.as_ref().unwrap())
        .collect()
}

/// An editor that offers each listed record's value of every setting of
/// its type, and nothing else.
fn all_offered() -> Vec<(u32, Vec<i32>)> {
    let mut by: HashMap<u32, Vec<i32>> = HashMap::new();
    for l in all_listed() {
        let ty = l.record.game_type();
        for s in variants::record::all_settings() {
            if !variants::schema::applies(s, ty) {
                continue;
            }
            if let Some(v) = l.record.get(s) {
                let values = by.entry(s.0).or_default();
                if !values.contains(&v) {
                    values.push(v);
                }
            }
        }
    }
    let mut out: Vec<_> = by.into_iter().collect();
    out.sort();
    out
}

fn offer_map(offer: &[(u32, Vec<i32>)]) -> HashMap<u32, Vec<i32>> {
    offer.iter().cloned().collect()
}

fn position(name: &str) -> usize {
    LISTED.iter().position(|b| b.name == name).unwrap()
}

#[test]
fn entries_are_built_from_the_listed_scripts() {
    let offer = offer_map(&all_offered());
    let (ours, lines) = entries(&offer);
    assert!(lines.is_empty(), "{lines:?}");
    assert_eq!(ours.len(), listed().len());
    for (i, (e, b)) in ours.iter().zip(LISTED).enumerate() {
        let ir = ::rules::compile(b.source).unwrap();
        // The name: the script's, a space and the icon, at our id.
        assert_eq!(e.name_id, name_id(i));
        assert_eq!(
            text(e.name_id),
            Some(format!("{} \u{e006}", ir.header.name))
        );
        // The game type: the list type of the record's engine.
        let ty = GameType::from_list_index(e.list_type).unwrap();
        assert_eq!(ty, listed()[i].as_ref().unwrap().record.game_type().menu_template(), "{}", b.name);
        // The pairs: the record of the script, every setting the editor
        // offers, not only the script's.
        let record = variants::portable::record_for(&ir, &[]).unwrap();
        assert_eq!(
            e.pairs,
            inject::pairs(&record, &offer).unwrap(),
            "{}",
            b.name
        );
        for (s, v) in &e.pairs {
            assert_eq!(
                record.get(SettingId(*s)),
                Some(*v),
                "{}: setting {s}",
                b.name
            );
        }
        assert!(e.pairs.len() > ir.settings.len(), "{}", b.name);
        // The description: `about`, at its number.
        assert_eq!(e.description, DESCRIPTION_BASE + i as u32);
        assert_eq!(description_for(e.description), Some(description_id(i)));
        assert_eq!(
            text(description_id(i)).as_deref(),
            Some(ir.header.about.as_str())
        );
        assert!(!ir.header.about.is_empty(), "{}", b.name);
        assert!(ir.header.about.encode_utf16().count() < 0xff, "{}", b.name);
    }
    // The script's own values reach the entry: SWAT's no shields, team
    // play and 50 points; Hide and Seek is a Juggernaut entry.
    let swat = &ours[position("SWAT")];
    for pair in [(19, 1), (91, 1), (2, 50)] {
        assert!(swat.pairs.contains(&pair), "{pair:?} in {:?}", swat.pairs);
    }
    assert_eq!(ours[position("Hide and Seek")].list_type, 2, "Juggernaut");
    assert_eq!(ours[position("Sun King")].list_type, 3, "King of the Hill");
}

#[test]
fn listed_names_carry_our_icon_and_avoid_retails() {
    let names: Vec<&str> = all_listed().iter().map(|l| l.name.as_str()).collect();
    for l in all_listed() {
        assert!(l.display_name().ends_with(ICON));
        assert!(!l.display_name().contains('\u{e008}'), "never the padlock");
    }
    // Retail names to avoid (docs/game/community-gametypes.md).
    for clash in [
        "Rockets",
        "Swords",
        "Snipers",
        "Fiesta",
        "Dodgeball",
        "Juggernaut",
        "Ninjanaut",
    ] {
        assert!(!names.contains(&clash));
    }
}

#[test]
fn a_script_the_disc_does_not_offer_is_left_out_by_name() {
    // Offer everything except a battle rifle start.
    let mut offer = all_offered();
    for (s, v) in &mut offer {
        if *s == 89 {
            v.retain(|x| *x != 3);
        }
    }
    let m = world(3, &offer);
    let (ours, lines) = entries(&offered(&m, GOOF, TAGS).unwrap());
    let kept: Vec<u32> = ours.iter().map(|e| e.name_id).collect();
    for (i, l) in all_listed().into_iter().enumerate() {
        let rifle = l.record.get(SettingId(89)) == Some(3);
        assert_eq!(kept.contains(&name_id(i)), !rifle, "{}", l.name);
        let named = lines.iter().any(|line| {
            line.contains(&format!(" {} left out", l.name)) && line.contains("does not offer")
        });
        assert_eq!(named, rifle, "{}: {lines:?}", l.name);
    }
    assert!(
        lines.iter().any(|l| l.contains("SWAT left out")),
        "{lines:?}"
    );
    assert_eq!(lines.len(), listed().len() - ours.len(), "one line each");

    // With nothing to add the tag is unchanged.
    let mut m = world(3, &[]);
    assert_eq!(
        extend_builtins(&mut m, GOOF, &[], DATA_BASE, ENTRIES_AT),
        Some(false)
    );
    assert_eq!(m.u32(GOOF + GOOF_BUILTINS + 4), Some(OLD));
    assert_eq!(m.u32(GOOF + GOOF_BUILTINS), Some(3));
}

/// Bumper Cars starts unarmed, which only our choices offer
/// (`variant_options`): without them it is left out, with them it is
/// kept. Bounce Practice's magnum is a retail choice either way.
#[test]
fn bumper_cars_needs_our_none_and_bounce_practice_has_a_magnum() {
    let (bumper, bounce) = (position("Bumper Cars"), position("Bounce Practice"));
    let records = all_listed();
    assert_eq!(records[bumper].record.get(SettingId(89)), Some(1));
    assert_eq!(records[bumper].record.get(SettingId(94)), Some(1));
    assert_eq!(
        records[bounce].record.get(SettingId(89)),
        Some(4),
        "a magnum"
    );
    assert_eq!(records[bounce].record.get(SettingId(94)), Some(1));

    // Retail's lists: every value our scripts use except primary None.
    let mut retail = all_offered();
    for (s, v) in &mut retail {
        if *s == 89 {
            v.retain(|x| *x != 1);
        }
    }
    let mut m = world(3, &retail);
    let (ours, _) = entries(&offered(&m, GOOF, TAGS).unwrap());
    let kept: Vec<u32> = ours.iter().map(|e| e.name_id).collect();
    assert!(!kept.contains(&name_id(bumper)) && kept.contains(&name_id(bounce)));

    // With our choices merged into the same lists, it is kept.
    use crate::variant_options as vo;
    let done = vo::extend(&mut m, GOOF, TAGS, vo::BASE, vo::SIZE).unwrap();
    assert!(done.lists >= 1);
    let (ours, lines) = entries(&offered(&m, GOOF, TAGS).unwrap());
    assert!(lines.is_empty(), "{lines:?}");
    assert_eq!(ours.len(), listed().len());
}

#[test]
fn superbounce_is_filed_under_party_games_by_its_script() {
    let sb = listed_named("Superbounce").unwrap();
    assert_eq!(
        sb.record.game_type(),
        GameType::Script,
        "the record is independent of the Party Games row"
    );
    assert_eq!(Row::of_type(sb.game_type), Some(Row::PartyGames));
    let ext = sb.ext();
    assert!(ext.out_of_range().is_none());
    assert_eq!(Ext::decode(&sb.block()), variants::ext::Block::Ext(ext));
    // Mode 2, naming the compiled-in Superbounce script with its default
    // options.
    let ir = ::rules::compile(::rules::builtin::SUPERBOUNCE).unwrap();
    assert_eq!(ext.script.mode, variants::ext::Mode::Rules);
    assert_eq!(ext.script.prefix[..], ir.id().0[..8]);
    assert_eq!(ext.script.options, ir.default_options());
    let found = crate::rules::runtime::compiled_in(ext.script.prefix).unwrap();
    assert_eq!(found.id, ir.id());
    assert!(crate::rules::header::script_asks_superbounce(ext.script));
    // Its block files a renamed copy under Party Games through the script's
    // header, and its list name files the built-in before its block.
    let none = [0; ext::LEN];
    assert_eq!(row_of("My Bounce", &sb.block()), Some(Row::PartyGames));
    assert_eq!(row_of(&sb.display_name(), &none), Some(Row::PartyGames));
    assert_eq!(row_of("My Bounce", &none), None);
}

#[test]
fn a_copy_lists_under_its_scripts_row() {
    let none = [0; ext::LEN];
    for l in all_listed() {
        // A copy Edit makes keeps the block and takes a new name.
        assert_eq!(row_of("My Copy", &l.block()), Row::of_type(l.game_type), "{}", l.name);
        // The built-in before its block is written: by its list name.
        assert_eq!(row_of(&l.display_name(), &none), Row::of_type(l.game_type), "{}", l.name);
        // Its bare name and no block is a player's variant of its own.
        assert_eq!(row_of(&l.name, &none), None, "{}", l.name);
    }
    let zombies = listed_named("Zombies").unwrap();
    assert_eq!(
        row_of("My Zombies", &zombies.block()),
        Some(Row::PartyGames)
    );
    let swat = listed_named("SWAT").unwrap();
    assert_eq!(
        row_of("My SWAT", &swat.block()),
        None,
        "SWAT's stay under Slayer"
    );
    // A block naming a script decides, whatever the name.
    assert_eq!(row_of(&zombies.display_name(), &swat.block()), None);
    // More Options alone is no script.
    let gravity = Ext {
        gravity: 50,
        ..Ext::RETAIL
    };
    assert_eq!(row_of("Moon Slayer", &gravity.encode()), None);
    // A block the game would ignore names no script; the name still does
    // for a built-in.
    let mut bad = zombies.block();
    bad[ext::LEN - 1] ^= 1;
    assert_eq!(row_of("My Zombies", &bad), None);
    assert_eq!(row_of(&zombies.display_name(), &bad), Some(Row::PartyGames));
}

#[test]
fn the_party_games_row_holds_the_party_and_practice_scripts() {
    let under = |r: Row| -> Vec<&str> {
        all_listed()
            .into_iter()
            .filter(|l| Row::of_type(l.game_type) == Some(r))
            .map(|l| l.name.as_str())
            .collect()
    };
    assert_eq!(
        under(Row::PartyGames),
        [
            "Zombies",
            "Hide and Seek",
            "Jousting",
            "Cops and Robbers",
            "Bounce Practice",
            "Glitch Hunt",
            "Bumper Cars",
            "Superbounce",
            "Gun Game",
            "Race",
            "VIP"
        ]
    );
    assert_eq!(under(Row::Siege), ["Siege"]);
}

/// A built-in variant record at `at` named `name`, of type `ty`, with no
/// block: as `0x149f88` files it.
fn put_builtin(m: &mut dyn guest_layout::memory::GuestMemory, at: u32, name: &str, ty: GameType) {
    let mut v = GameVariant::blank(ty);
    v.set_name(name).unwrap();
    m.write(at, v.bytes()).unwrap();
    m.write(at + ext::AT as u32, &[0; ext::LEN]).unwrap();
}

#[test]
fn every_listed_record_gets_its_block_at_load() {
    use crate::tweakers::{
        BUILTIN_LIST, BUILTIN_RECORD, BUILTIN_VARIANT, record_block, record_type,
    };
    const RECORDS: u32 = 0x0060_0000;
    let all = all_listed();
    let mut g = guest_layout::testing::FakeGuest::new();
    g.map(BUILTIN_LIST & !0xfff, 0x1000).map(RECORDS, 0x4000);
    assert_eq!(write_blocks(&mut g), Ok(0), "no list yet");
    // A retail built-in, every listed one, then a listed name on another
    // engine (not the built-in).
    let n = all.len() as u32 + 2;
    g.put_u32(BUILTIN_LIST, RECORDS).put_u32(RECORDS, n);
    let rec = |i: u32| RECORDS + 4 + i * BUILTIN_RECORD + BUILTIN_VARIANT;
    put_builtin(&mut g, rec(0), "Slayer \u{e008}", GameType::Slayer);
    for (k, l) in all.iter().enumerate() {
        put_builtin(
            &mut g,
            rec(1 + k as u32),
            &l.display_name(),
            GameType::from_list_index(l.list_type).unwrap(),
        );
    }
    put_builtin(&mut g, rec(n - 1), &all[0].display_name(), GameType::Ctf);
    assert_eq!(write_blocks(&mut g), Ok(all.len()));
    assert_eq!(
        record_block(&g, rec(0)),
        Some([0; ext::LEN]),
        "retail's untouched"
    );
    assert_eq!(
        record_block(&g, rec(n - 1)),
        Some([0; ext::LEN]),
        "another engine's"
    );
    for (k, l) in all.iter().enumerate() {
        let r = rec(1 + k as u32);
        let block = record_block(&g, r).unwrap();
        assert_eq!(block, l.block(), "{}", l.name);
        assert_eq!(crate::tweakers::record_engine(&g, r), Some(l.record.game_type().engine()), "{}", l.name);
        // A game on it finds its script.
        let script = Ext::in_effect(&block).script;
        if l.game_type == Type::Firefight {
            assert_eq!(script.mode, variants::ext::Mode::Firefight);
            if script.prefix == ::firefight::builtin::MAP_WAVES {
                assert_eq!(l.name, ::firefight::builtin::MAP_WAVES_NAME);
            } else {
                assert_eq!(::firefight::builtin::by_prefix(script.prefix).unwrap().name, l.name);
            }
            assert_eq!(record_type(&g, r), Some(Type::Firefight));
            continue;
        }
        assert_eq!(script.mode, variants::ext::Mode::Rules, "{}", l.name);
        let found = crate::rules::runtime::compiled_in(script.prefix).expect("compiled in");
        assert_eq!(found.id.0[..8], script.prefix, "{}", l.name);
        assert_eq!(record_type(&g, r), Some(l.game_type), "{}", l.name);
    }
    assert_eq!(write_blocks(&mut g), Ok(0), "already there");
}

#[test]
fn the_goof_template_becomes_a_script_record_before_selection_or_editing() {
    use guest_layout::memory::GuestMemory;
    use variants::record::{RECORD_SIZE, TYPE_AREA};
    let template = GameType::from_list_index(goof_type(GameType::Script).unwrap()).unwrap();
    assert_eq!(template, GameType::Slayer);
    let siege = listed_named("Siege").unwrap();
    let mut g = guest_layout::testing::FakeGuest::new();
    g.map(GOOF, RECORD_SIZE as u32);
    put_builtin(&mut g, GOOF, &siege.display_name(), template);
    g.set_u8(GOOF + TYPE_AREA.start as u32, 1).unwrap();
    assert_eq!(restore_script_record(&mut g, GOOF, GameType::Script, &siege.block()), Ok(true));
    let mut raw = [0; RECORD_SIZE];
    g.read(GOOF, &mut raw).unwrap();
    let restored = GameVariant::from_bytes(&raw).unwrap();
    assert_eq!(restored.game_type(), GameType::Script);
    assert!(raw[TYPE_AREA].iter().all(|v| *v == 0));
    assert_eq!(restored.name(), siege.display_name());
    assert_eq!(crate::tweakers::record_type(&g, GOOF), Some(Type::Siege));
    assert_eq!(restore_script_record(&mut g, GOOF, GameType::Script, &siege.block()), Ok(false));
}

#[test]
fn the_rows_texts_are_ours_and_apart_from_the_scripts() {
    for r in Row::ALL {
        assert_eq!(text(r.label_id()).as_deref(), Some(r.label()));
        assert_eq!(text(r.title_id()).as_deref(), Some(r.title()));
        assert_eq!(text(r.description_id()).as_deref(), Some(r.description()));
        assert_eq!(r.title(), r.label().to_uppercase(), "as SLAYER is Slayer's");
        assert!(r.description().ends_with('.') && r.description().len() < 120);
        assert_eq!(Row::from_mark(r.mark()), Some(r));
    }
    assert_eq!(Row::from_mark(0), None);
    let ids: Vec<u32> = row_ids().collect();
    assert_eq!(ids.len(), 3 * Row::ALL.len());
    for (k, id) in ids.iter().enumerate() {
        assert!(is_ours(*id));
        assert!(!ids[..k].contains(id), "{id:#x} twice");
        assert!((0..listed().len()).all(|i| *id != name_id(i) && *id != description_id(i)));
    }
}

#[test]
fn string_ids_are_private_and_round_trip() {
    for (i, l) in all_listed().into_iter().enumerate() {
        assert!(is_ours(name_id(i)) && is_ours(description_id(i)));
        assert_eq!(text(name_id(i)).unwrap(), l.display_name());
        assert_eq!(text(description_id(i)).unwrap(), l.about);
        assert_eq!(
            description_for(DESCRIPTION_BASE + i as u32),
            Some(description_id(i))
        );
    }
    // Retail-shaped ids (length byte, small index) are never ours.
    for id in [
        0x0700_0406,
        0x0100_030c,
        0x0200_031d,
        0x1c00_164d,
        0,
        u32::MAX,
    ] {
        assert!(!is_ours(id) && text(id).is_none());
    }
    // Retail description numbers, and numbers past ours, pass through.
    for n in [
        0,
        2,
        38,
        39,
        DESCRIPTION_BASE + listed().len() as u32,
        93,
        100,
    ] {
        assert_eq!(description_for(n), None, "{n}");
    }
    assert!(
        DESCRIPTION_BASE + listed().len() as u32 <= inject::DESCRIPTION_BASE,
        "the store's numbers start at 94"
    );
}

#[test]
fn utf16_buffer_is_nul_terminated_and_bounded() {
    let b = utf16_buffer("Zombies \u{e006}");
    assert_eq!(b.len(), 10 * 2);
    assert_eq!(&b[16..20], &[0x06, 0xe0, 0, 0]);
    let long = "x".repeat(400);
    assert_eq!(utf16_buffer(&long).len(), 0x100 * 2);
}

#[test]
fn extend_appends_after_retail_entries_unchanged() {
    let offer = all_offered();
    let mut m = world(39, &offer);
    let retail_before: Vec<u8> = {
        let mut b = vec![0; 39 * ENTRY_SIZE as usize];
        assert!(m.rd(OLD, &mut b));
        b
    };
    let (ours, lines) = entries(&offered(&m, GOOF, TAGS).unwrap());
    assert!(lines.is_empty(), "{lines:?}");
    assert_eq!(
        extend_builtins(&mut m, GOOF, &ours, DATA_BASE, ENTRIES_AT),
        Some(true)
    );

    let n = m.u32(GOOF + GOOF_BUILTINS).unwrap();
    assert_eq!(n as usize, 39 + listed().len());
    assert_eq!(m.u32(GOOF + GOOF_BUILTINS + 4), Some(ENTRIES_AT));
    // Retail entries: byte for byte, in place; the old block untouched.
    let mut b = vec![0; retail_before.len()];
    assert!(m.rd(ENTRIES_AT, &mut b));
    assert_eq!(b, retail_before);
    assert!(m.rd(OLD, &mut b));
    assert_eq!(b, retail_before);
    // Ours decode back to their entries.
    for (k, want) in ours.iter().enumerate() {
        let e = ENTRIES_AT + (39 + k as u32) * ENTRY_SIZE;
        assert_eq!(m.u32(e), Some(name_id(k)));
        assert_eq!(m.u32(e + 4), Some(want.list_type));
        let display = text(m.u32(e).unwrap()).unwrap();
        let template = GameType::from_list_index(m.u32(e + 4).unwrap()).unwrap();
        let l = listed()[k].as_ref().unwrap();
        let ty = l.game_type;
        assert_eq!(filed_type(&display, &[0; ext::LEN], template), Some(ty), "{display}");
        assert_eq!(filed_type("Renamed", &l.block(), GameType::Script), Some(ty), "{}", l.name);
        let (count, at) = (m.u32(e + 8).unwrap(), m.u32(e + 12).unwrap());
        let got: Vec<(u32, i32)> = (0..count)
            .map(|j| {
                (
                    m.u32(at + j * 8).unwrap(),
                    m.u32(at + j * 8 + 4).unwrap() as i32,
                )
            })
            .collect();
        assert_eq!(got, want.pairs);
        assert_eq!(m.u32(e + 16), Some(DESCRIPTION_BASE + k as u32));
    }
    // Again: already ours, nothing changes.
    assert_eq!(
        extend_builtins(&mut m, GOOF, &ours, DATA_BASE, ENTRIES_AT),
        Some(false)
    );
    assert_eq!(m.u32(GOOF + GOOF_BUILTINS), Some(n));
}

#[test]
fn a_failed_read_leaves_the_tag_alone() {
    let entry = |i: usize| NewEntry {
        name_id: name_id(i),
        list_type: 0,
        pairs: vec![(2, 25)],
        description: DESCRIPTION_BASE + i as u32,
    };
    let mut m = world(5, &[]);
    // The retail block claims more entries than exist.
    m.set_u32(GOOF + GOOF_BUILTINS, 6);
    assert_eq!(
        extend_builtins(&mut m, GOOF, &[entry(0), entry(1)], DATA_BASE, ENTRIES_AT),
        None
    );
    assert_eq!(m.u32(GOOF + GOOF_BUILTINS + 4), Some(OLD));
    assert_eq!(m.u32(GOOF + GOOF_BUILTINS), Some(6));
    // A list that would not fit our page is refused the same way.
    m.set_u32(GOOF + GOOF_BUILTINS, MAX_SLOTS);
    assert_eq!(
        extend_builtins(&mut m, GOOF, &[entry(0)], DATA_BASE, ENTRIES_AT),
        None
    );
    assert_eq!(m.u32(GOOF + GOOF_BUILTINS + 4), Some(OLD));
}

/// Against your disc: the retail editor (with our added choices) offers
/// every value of every listed script's record, so the list gets all of
/// them (docs/game/rules-variants.md, "Built-in list entries").
#[test]
#[ignore = "reads $H2_DISC (the retail disc image)"]
fn every_listed_script_is_offered_on_the_retail_disc() {
    let disc = std::env::var("H2_DISC").expect("H2_DISC");
    let mut iso =
        xiso::Xiso::open(std::io::BufReader::new(std::fs::File::open(&disc).unwrap())).unwrap();
    let e = iso.find("maps/mainmenu.map").unwrap();
    let bytes = iso.read(&e).unwrap();
    let mut map = map::Map::from_reader(std::io::Cursor::new(bytes)).unwrap();
    let schema = variants::Schema::from_map(&mut map, tags::Language::English).unwrap();
    let offer: HashMap<u32, Vec<i32>> = schema
        .settings
        .iter()
        .map(|(id, d)| (id.0, d.choices.iter().map(|c| c.value).collect()))
        .collect();
    let (ours, lines) = entries(&offer);
    assert!(
        lines.is_empty(),
        "left out on this disc:\n{}",
        lines.join("\n")
    );
    assert_eq!(ours.len(), listed().len());
}

/// The listed Firefight entries are the built-in sets in menu order and
/// then Map Waves, which names the map-waves prefix and no definition
/// (docs/game/forge-firefight.md, "Playing it").
#[test]
fn map_waves_is_listed_after_the_built_ins() {
    let fights: Vec<&Listed> = all_listed()
        .into_iter()
        .filter(|l| l.game_type == Type::Firefight)
        .collect();
    let sets = ::firefight::builtin::variants().len();
    assert_eq!(fights.len(), sets + 1, "every set, then Map Waves");
    let own = fights[sets];
    assert_eq!(own.name, ::firefight::builtin::MAP_WAVES_NAME);
    let script = Ext::in_effect(&own.block()).script;
    assert_eq!(script.mode, variants::ext::Mode::Firefight);
    assert_eq!(script.prefix, ::firefight::builtin::MAP_WAVES);
    assert!(::firefight::builtin::by_prefix(script.prefix).is_none());
    for (set, listed) in ::firefight::builtin::variants().iter().zip(&fights) {
        assert_eq!(listed.name, set.def.name, "the sets keep their order");
    }
    assert_eq!(
        all_listed().last().map(|l| l.name.as_str()),
        Some(own.name.as_str()),
        "last in the list"
    );
}
