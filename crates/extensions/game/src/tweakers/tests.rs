use super::*;
use engine::DatumHandle;
use engine::datum::DatumArray;
use engine::entry::{self, Regs};
use engine::testing::FakeGuest;
use variants::ext::Ext;
use variants::record::GameType;

const NAME: u32 = 0x0001_0000;
const ARRAY: u32 = 0x0002_0000;
const SCREEN: u32 = 0x0010_0000;
const HANDLER: u32 = SCREEN + SCREEN_LIST;
const STACK: u32 = 0x0003_0000;
const COUNT: u32 = 0x0003_1000;
const HANDLES: u32 = 0x0003_2000;
const SCRATCH: u32 = 0x0004_0000;
const RECORDS: u32 = 0x0005_0000;

/// A list handler at `list` whose rows hold `words`, elements of
/// `element` bytes.
fn rows_with(g: &mut FakeGuest, list: u32, words: &[i16], element: i32) -> Vec<DatumHandle> {
    g.map(NAME, 0x20).map(ARRAY, 0x1000);
    g.put_u32(list + LIST_ROWS, ARRAY);
    let rows = DatumArray::at(ARRAY);
    rows.initialize_contiguous(g, NAME, 9, element, 0, 0)
        .unwrap();
    rows.make_valid(g).unwrap();
    words
        .iter()
        .map(|&w| {
            let h = rows.new_datum(g).unwrap().unwrap();
            let elem = rows.try_get(g, h).unwrap().unwrap();
            g.set_u16(elem + ROW_WORD, w as u16).unwrap();
            h
        })
        .collect()
}

fn utf16(s: &str) -> Vec<u8> {
    s.encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect()
}

/// A Slayer variant record at `at` named `name` with extension block
/// `block`.
fn put_record(m: &mut dyn GuestMemory, at: u32, name: &str, block: [u8; ext::LEN]) {
    put_typed(m, at, name, block, GameType::Slayer);
}

/// A variant record at `at` of type `ty`, named `name`, with extension
/// block `block`.
fn put_typed(m: &mut dyn GuestMemory, at: u32, name: &str, block: [u8; ext::LEN], ty: GameType) {
    m.write(at + variants::record::NAME_AT as u32, &utf16(name))
        .unwrap();
    m.write(at + ext::AT as u32, &block).unwrap();
    m.set_u32(at + variants::record::ENGINE_AT as u32, ty.engine() as u32)
        .unwrap();
}

/// The block of the listed script named `name`.
fn block_of(name: &str) -> [u8; ext::LEN] {
    community::listed_named(name).unwrap().block()
}

#[test]
fn our_rows_are_last_after_territories() {
    let words = with_row(&GAME_TYPE_WORDS);
    assert_eq!(words, [0, 1, 3, 4, 6, 7, 8, RESEARCH_WORD, PARTY_WORD]);
    assert_eq!(words[words.len() - 3], 8, "Territories just above them");
    assert_eq!(with_row(&words), words, "added once");
    // Two retail rows are left out of some lists; ours are still last.
    assert_eq!(with_row(&[0, 1, 3, 6, 7]), [0, 1, 3, 6, 7, RESEARCH_WORD, PARTY_WORD]);
    for r in Row::ALL {
        assert!(!GAME_TYPE_WORDS.contains(&word_of_row(r)));
        assert_eq!(row_of_word(word_of_row(r)), Some(r));
    }
    assert_eq!(row_of_word(SLAYER_WORD), None);
}

#[test]
fn both_game_type_screens_get_the_row_but_not_the_create_mode() {
    assert!(wants_row(0xcf, false), "Game Setup's Change Rules");
    assert!(wants_row(0x3c, false), "Settings, Game Variants");
    assert!(!wants_row(0x3c, true), "a new variant of a game type");
    assert!(!wants_row(0xb2, false), "another screen");
}

#[test]
fn each_row_keeps_its_own_and_slayer_the_rest() {
    let under: Vec<_> = row_variants().map(|l| l.name.as_str()).collect();
    assert!(under.contains(&"Superbounce") && under.contains(&"Zombies"), "{under:?}");
    let slayer: Vec<_> = community::listed()
        .iter()
        .flatten()
        .filter(|l| l.record.game_type() == GameType::Slayer && l.row.is_none())
        .map(|l| l.name.as_str())
        .collect();
    assert!(!slayer.contains(&"Superbounce") && !slayer.contains(&"Zombies"), "{slayer:?}");
    assert!(slayer.contains(&"SWAT"), "the rest stay: {slayer:?}");

    let none = [0; ext::LEN];
    let sb = community::row_of("Superbounce \u{e006}", &none);
    let zombies = community::row_of("Zombies \u{e006}", &none);
    let swat = community::row_of("SWAT \u{e006}", &none);
    let (research, party) = (Some(Row::Research), Some(Row::PartyGames));
    assert!(keeps(research, true, sb) && !keeps(party, true, sb) && !keeps(None, true, sb));
    assert!(keeps(party, true, zombies) && !keeps(research, true, zombies));
    assert!(keeps(None, true, swat) && !keeps(research, true, swat) && !keeps(party, true, swat));
    // A record that did not load stays retail's.
    assert!(keeps(None, false, research) && !keeps(research, false, research));
}

#[test]
fn every_row_variant_is_a_slayer_variant() {
    // A row's list is Slayer's list filtered: its new variants, copies and
    // defaults are Slayer's. Each built-in's record names its script.
    for l in row_variants() {
        assert_eq!(l.record.game_type(), GameType::Slayer, "{}", l.name);
        assert!(!l.ext().is_default(), "{}", l.name);
    }
    // Hide and Seek names Party Games but is Juggernaut's: it is no row's
    // variant, and stays on Juggernaut's own list, which is not filtered.
    let hide = community::listed_named("Hide and Seek").unwrap();
    assert_eq!(hide.row, Some(Row::PartyGames));
    assert_eq!(hide.record.game_type(), GameType::Juggernaut);
    assert!(row_variants().all(|l| l.name != "Hide and Seek"));
    assert!(filtered(SLAYER_CATEGORY));
    assert!(!filtered(2), "King of the Hill's list is retail's");
}

#[test]
fn a_record_of_another_type_is_on_no_row() {
    let mut g = FakeGuest::new();
    g.map(RECORDS, 0x1000);
    let hide = community::listed_named("Hide and Seek").unwrap();
    put_typed(&mut g, RECORDS, &hide.display_name(), hide.block(), GameType::Juggernaut);
    assert_eq!(record_row(&g, RECORDS), None, "Juggernaut's");
    put_typed(&mut g, RECORDS, "My Hide", hide.block(), GameType::Juggernaut);
    assert_eq!(record_row(&g, RECORDS), None, "its copy too");
    put_typed(&mut g, RECORDS, "My Zombies", block_of("Zombies"), GameType::Slayer);
    assert_eq!(record_row(&g, RECORDS), Some(Row::PartyGames));
    put_typed(&mut g, RECORDS, "My Zombies", block_of("Zombies"), GameType::King);
    assert_eq!(record_row(&g, RECORDS), None, "a row is for Slayer records");
}

#[test]
fn a_saved_copy_with_the_block_stays_under_research() {
    let flag = block_of("Superbounce");
    let research = Some(Row::Research);
    assert_eq!(community::row_of("My Bounce", &flag), research, "renamed copy");
    assert_eq!(
        community::row_of("Superbounce \u{e006}", &[0; ext::LEN]),
        research,
        "the built-in before its block"
    );
    assert_eq!(community::row_of("Superbounce", &[0; ext::LEN]), None, "a player's own");
    assert_eq!(community::row_of("Slayer", &[0; ext::LEN]), None);
    let gravity = Ext {
        gravity: 50,
        ..Ext::RETAIL
    };
    assert_eq!(
        community::row_of("Moon Slayer", &gravity.encode()),
        None,
        "More Options alone is not a tweak"
    );
    // A block the game would ignore names no script.
    let mut bad = flag;
    bad[ext::LEN - 1] ^= 1;
    assert_eq!(community::row_of("My Bounce", &bad), None);
}

/// The variant list's fill, as `0x2ca0d9` calls `0x215900`: four handles
/// come back, a Slayer variant, the built-in Superbounce, a saved copy of
/// it under another name and the built-in Zombies.
fn run_fill(list: Option<Row>, create: bool) -> (entry::testing::Run, Vec<DatumHandle>) {
    const A: u32 = 0xe000_0001;
    const B: u32 = 0xe020_0002;
    const C: u32 = 0xe000_0003;
    const D: u32 = 0xe020_0004;
    let mut g = FakeGuest::new();
    g.map(SCREEN, 0x5000)
        .map(STACK, 0x100)
        .map(COUNT, 4)
        .map(HANDLES, 0x100);
    g.map(SCRATCH, 0x200).map(BUILTIN_LIST & !0xfff, 0x1000);
    g.put_u8(SCREEN + SCREEN_MARK, list.map_or(0, Row::mark));
    g.put_u8(HANDLER + HANDLER_CREATE, u8::from(create));
    let rows = rows_with(&mut g, HANDLER, &[0], 0xc);
    if create {
        // Create New: handle -1.
        let elem = DatumArray::at(ARRAY).try_get(&g, rows[0]).unwrap().unwrap();
        g.set_u32(elem + 4, u32::MAX).unwrap();
    }
    // [esp]: the fill's return; then player, category, count, handles, flag.
    for (i, v) in [FILL_RETURN, 0, SLAYER_CATEGORY, COUNT, HANDLES, 1]
        .into_iter()
        .enumerate()
    {
        g.put_u32(STACK + 4 * i as u32, v);
    }
    let f = |cx: Cx| -> EntryFuture { Box::pin(fill(cx, SCRATCH)) };
    let regs = Regs {
        esp: STACK,
        ebx: HANDLER,
        ..Default::default()
    };
    let run = entry::testing::run(&f, g, regs, |call, m| match call.target {
        t if t == points::VARIANTS_ENUMERATE.base => {
            assert_eq!(call.args, [0, SLAYER_CATEGORY, COUNT, HANDLES, 1]);
            m.set_u16(COUNT, 4).unwrap();
            for (i, h) in [A, B, C, D].into_iter().enumerate() {
                m.set_u32(HANDLES + 4 * i as u32, h).unwrap();
            }
            Regs::default()
        }
        LOAD_VARIANT => {
            assert_eq!(call.eax, Some(SCRATCH));
            let (name, block) = match call.edx {
                Some(A) => ("Rockets", [0; ext::LEN]),
                Some(B) => ("Superbounce \u{e006}", block_of("Superbounce")),
                Some(D) => ("Zombies \u{e006}", [0; ext::LEN]),
                _ => ("My Bounce", block_of("Superbounce")),
            };
            m.fill(SCRATCH, 0x130, 0).unwrap();
            put_record(m, SCRATCH, name, block);
            Regs {
                eax: 1,
                ..Default::default()
            }
        }
        other => panic!("unexpected call {other:#x}"),
    });
    assert_eq!(run.result, Ok(Ret::new(0, 20)));
    (run, rows)
}

#[test]
fn slayers_list_keeps_everything_but_our_rows() {
    let (run, rows) = run_fill(None, true);
    let g = &run.guest;
    assert_eq!(read_handles(g, COUNT, HANDLES), Ok(vec![0xe000_0001]));
    let create = DatumArray::at(ARRAY).try_get(g, rows[0]).unwrap();
    assert!(create.is_some(), "Slayer keeps Create New");
}

#[test]
fn a_rows_list_keeps_its_own_and_drops_create_new() {
    let (run, rows) = run_fill(Some(Row::Research), true);
    let g = &run.guest;
    assert_eq!(
        read_handles(g, COUNT, HANDLES),
        Ok(vec![0xe020_0002, 0xe000_0003]),
        "the built-in and its renamed copy"
    );
    let create = DatumArray::at(ARRAY).try_get(g, rows[0]).unwrap();
    assert!(
        create.is_none(),
        "a new variant here would be a plain Slayer"
    );
    let (run, _) = run_fill(Some(Row::PartyGames), true);
    assert_eq!(read_handles(&run.guest, COUNT, HANDLES), Ok(vec![0xe020_0004]), "Zombies");
    // The lobby's list has no Create New and keeps its rows.
    let (run, rows) = run_fill(Some(Row::Research), false);
    assert!(
        DatumArray::at(ARRAY)
            .try_get(&run.guest, rows[0])
            .unwrap()
            .is_some()
    );
}

#[test]
fn another_categorys_fill_is_left_alone() {
    let mut g = FakeGuest::new();
    g.map(SCREEN, 0x5000)
        .map(STACK, 0x100)
        .map(COUNT, 4)
        .map(HANDLES, 0x100);
    for (i, v) in [FILL_RETURN, 0, 2, COUNT, HANDLES, 1]
        .into_iter()
        .enumerate()
    {
        g.put_u32(STACK + 4 * i as u32, v);
    }
    let f = |cx: Cx| -> EntryFuture { Box::pin(fill(cx, SCRATCH)) };
    let regs = Regs {
        esp: STACK,
        ebx: HANDLER,
        ..Default::default()
    };
    let run = entry::testing::run(&f, g, regs, |call, m| {
        assert_eq!(
            call.target,
            points::VARIANTS_ENUMERATE.base,
            "no record loads"
        );
        m.set_u16(COUNT, 1).unwrap();
        m.set_u32(HANDLES, 7).unwrap();
        Regs::default()
    });
    assert_eq!(run.calls.len(), 1);
    assert_eq!(read_handles(&run.guest, COUNT, HANDLES), Ok(vec![7]));
}

#[test]
fn handles_are_written_back_with_their_count() {
    let mut g = FakeGuest::new();
    g.map(COUNT, 4).map(HANDLES, 0x100);
    write_handles(&mut g, COUNT, HANDLES, &[5, 6]).unwrap();
    assert_eq!(read_handles(&g, COUNT, HANDLES), Ok(vec![5, 6]));
    write_handles(&mut g, COUNT, HANDLES, &[]).unwrap();
    assert_eq!(read_handles(&g, COUNT, HANDLES), Ok(vec![]));
}

#[test]
fn our_pane_is_the_row_with_our_word() {
    let mut g = FakeGuest::new();
    g.map(SCREEN, 0x5000);
    rows_with(&mut g, HANDLER, &with_row(&GAME_TYPE_WORDS), 4);
    assert_eq!(our_pane(&g, SCREEN, 0xcf, 7), Some(Row::Research));
    assert_eq!(our_pane(&g, SCREEN, 0x3c, 7), Some(Row::Research));
    assert_eq!(our_pane(&g, SCREEN, 0xcf, 8), Some(Row::PartyGames));
    assert_eq!(our_pane(&g, SCREEN, 0xcf, 6), None, "Territories has its pane");
    assert_eq!(our_pane(&g, SCREEN, 0xd0, 7), None, "another screen");
    assert_eq!(our_pane(&g, SCREEN, 0xcf, 9), None, "no row there");
}

#[test]
fn a_pane_text_is_found_by_its_block_not_the_header() {
    let mut g = FakeGuest::new();
    g.map(SCREEN, 0x5000).map(0x0020_0000, 0x1000);
    let (desc, title, bitmap) = (0x0020_0000, 0x0020_0100, 0x0020_0200);
    let header = SCREEN + 0x80;
    // Children: the header (a text, block index 0 too), a bitmap, then the
    // pane's two texts.
    g.put_u32(SCREEN + 0x14, header);
    g.put_u32(header + 4, 6)
        .put_u16(header + 0xa, 0)
        .put_u32(header + 0x18, bitmap);
    g.put_u32(bitmap + 4, 8)
        .put_u16(bitmap + 0xa, 0)
        .put_u32(bitmap + 0x18, desc);
    g.put_u32(desc + 4, 6)
        .put_u16(desc + 0xa, 0)
        .put_u32(desc + 0x18, title);
    g.put_u32(title + 4, 6)
        .put_u16(title + 0xa, 1)
        .put_u32(title + 0x18, 0);
    assert_eq!(pane_text(&g, SCREEN, PANE_DESCRIPTION_BLOCK), Some(desc));
    assert_eq!(pane_text(&g, SCREEN, PANE_TITLE_BLOCK), Some(title));
    assert_eq!(pane_text(&g, SCREEN, 2), None);
}

#[test]
fn our_pane_is_slayers_with_our_texts() {
    let mut g = FakeGuest::new();
    g.map(SCREEN, 0x5000)
        .map(0x0020_0000, 0x1000)
        .map(STACK, 0x100);
    let (desc, title) = (0x0020_0000, 0x0020_0100);
    const WORD: u32 = 0x0020_0800;
    const ORIGINAL: u32 = 0x00a0_0000;
    let f = |cx: Cx| -> EntryFuture { Box::pin(pane(cx, Row::PartyGames, ORIGINAL, SCREEN, 0, WORD)) };
    let regs = Regs {
        esp: STACK,
        ecx: SCREEN,
        ..Default::default()
    };
    let mut set = Vec::new();
    let run = entry::testing::run(&f, g, regs, |call, m| {
        if call.target == ORIGINAL {
            assert_eq!(
                (call.ecx, call.args.as_slice()),
                (Some(SCREEN), [WORD].as_slice())
            );
            assert_eq!(m.u16(WORD), Ok(SLAYER_PANE as u16));
            // The pane's widgets, as the pane builder makes them.
            m.set_u32(SCREEN + 0x14, desc).unwrap();
            m.set_u32(desc + 4, 6).unwrap();
            m.set_u32(desc + 0x18, title).unwrap();
            m.set_u32(title + 4, 6).unwrap();
            m.set_u16(title + 0xa, 1).unwrap();
        } else {
            assert_eq!(call.target, points::TEXT_SET.base);
            set.push((call.edi.unwrap(), call.args[0]));
        }
        Regs::default()
    });
    assert_eq!(run.result, Ok(Ret::new(0, 4)));
    assert_eq!(
        set,
        [
            (desc, Row::PartyGames.description_id()),
            (title, Row::PartyGames.title_id())
        ]
    );
}

#[test]
fn a_rows_list_reopens_through_slayers_loader_for_its_mode() {
    assert_eq!(slayer_loader(true, false), SLAYER_SETTINGS_LOADER);
    assert_eq!(slayer_loader(false, true), SLAYER_PICK_LOADER);
    assert_eq!(slayer_loader(false, false), SLAYER_LOBBY_LOADER);
}

#[test]
fn the_mark_is_the_screens_spare_byte() {
    // After the mode bytes, inside the 0x4fe0-byte screen.
    assert!(SCREEN_MARK > SCREEN_PICK && SCREEN_MARK < 0x4fe0);
    assert_eq!(
        SCREEN_LIST + HANDLER_CREATE,
        0x4fbc,
        "the screen's settings byte, copied"
    );
    let mut g = FakeGuest::new();
    g.map(SCREEN, 0x5000);
    assert_eq!(screen_row(&g, SCREEN), None);
    for r in Row::ALL {
        g.put_u8(SCREEN + SCREEN_MARK, r.mark());
        assert_eq!(screen_row(&g, SCREEN), Some(r));
    }
}

#[test]
fn the_rows_read_in_the_games_words() {
    assert_eq!(
        community::text(Row::Research.label_id()).as_deref(),
        Some("Research and Devolution")
    );
    assert_eq!(
        community::text(Row::PartyGames.label_id()).as_deref(),
        Some("Party Games")
    );
}

#[test]
fn the_hooks_are_on_the_plan() {
    let mut r = hooks::Registry::new(hooks::Switches::defaults());
    r.add(&Tweakers).unwrap();
    let text = r.build(&hooks::Base).unwrap().text();
    for (point, label) in [
        (
            "VARIANT_CATEGORY_SCREEN_CTOR",
            "after [tweakers:add our game type rows]",
        ),
        (
            "VARIANT_CATEGORY_ROW_TEXT",
            "before [tweakers:our rows' text]",
        ),
        (
            "VARIANT_CATEGORY_ROW_CHOSEN",
            "before [tweakers:our rows' press]",
        ),
        (
            "VARIANT_LIST_SCREEN_CTOR",
            "after [tweakers:mark a row's list]",
        ),
        (
            "VARIANTS_ENUMERATE",
            "before [tweakers:keep each row's variants apart]",
        ),
        (
            "VARIANT_LIST_UPDATE",
            "before [tweakers:a row's list's title and description]",
        ),
        (
            "VARIANT_LIST_LOADER",
            "before [tweakers:a row's list reopens as one]",
        ),
    ] {
        let line = text
            .lines()
            .find(|l| l.contains(&format!(" {point}: ")))
            .unwrap_or_else(|| panic!("{point} not on the plan:\n{text}"));
        assert!(line.contains(label), "{line}");
    }
}
