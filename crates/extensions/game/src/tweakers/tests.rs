use super::*;
use guest_layout::DatumHandle;
use guest_layout::datum::DatumArray;
use guest_layout::entry::{self, Regs};
use guest_layout::testing::FakeGuest;
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
    rows_in(g, list, words, element, 11)
}

/// [`rows_with`] in an array of `max` datums.
fn rows_in(g: &mut FakeGuest, list: u32, words: &[i16], element: i32, max: i32) -> Vec<DatumHandle> {
    g.map(NAME, 0x20).map(ARRAY, 0x1000);
    g.put_u32(list + LIST_ROWS, ARRAY);
    let rows = DatumArray::at(ARRAY);
    rows.initialize_contiguous(g, NAME, max, element, 0, 0)
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
    assert_eq!(words, [0, 1, 3, 4, 6, 7, 8, PARTY_WORD, FIREFIGHT_WORD, SIEGE_WORD]);
    assert_eq!(words[words.len() - 4], 8, "Territories just above them");
    assert_eq!(with_row(&words), words, "added once");
    // Two retail rows are left out of some lists; ours are still last.
    assert_eq!(with_row(&[0, 1, 3, 6, 7]), [0, 1, 3, 6, 7, PARTY_WORD, FIREFIGHT_WORD, SIEGE_WORD]);
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
fn the_script_type_on_a_renamed_save_does_not_depend_on_the_engine() {
    let mut g = FakeGuest::new();
    g.map(RECORDS, 0x1000);
    for ty in [GameType::Script, GameType::Oddball, GameType::Slayer] {
        for (name, expected) in [
            ("Hide and Seek", Type::Party),
            ("Clint Eastwood", Type::Juggernaut),
            ("Siege", Type::Siege),
        ] {
            put_typed(&mut g, RECORDS, "Edited variant", block_of(name), ty);
            assert_eq!(record_type(&g, RECORDS), Some(expected), "{name} on {ty}");
        }
    }
    let firefight = Ext {
        script: ext::Script { mode: ext::Mode::Firefight, ..Default::default() },
        ..Default::default()
    };
    put_typed(&mut g, RECORDS, "Wave test", firefight.encode(), GameType::Slayer);
    assert_eq!(record_type(&g, RECORDS), Some(Type::Firefight));
}

#[test]
fn a_saved_copy_with_the_block_stays_under_party_games() {
    let flag = block_of("Superbounce");
    let party = Some(Row::PartyGames);
    assert_eq!(community::row_of("My Bounce", &flag), party, "renamed copy");
    assert_eq!(
        community::row_of("Superbounce \u{e006}", &[0; ext::LEN]),
        party,
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

/// The variant list's fill, as `0x2ca0d9` calls `0x215900` for Slayer's
/// category: four handles come back, a Slayer variant, the built-in
/// Superbounce, a saved copy of it under another name and the built-in
/// Zombies. A row's list asks Juggernaut's category too, which holds the
/// built-ins Hide and Seek and Clint Eastwood and a saved copy of Hide and
/// Seek.
fn run_fill(list: Option<Row>, create: bool) -> (entry::testing::Run, Vec<DatumHandle>) {
    run_fill_in(list, create, SLAYER_CATEGORY)
}

fn run_fill_in(
    list: Option<Row>,
    create: bool,
    category: u32,
) -> (entry::testing::Run, Vec<DatumHandle>) {
    const A: u32 = 0xe000_0001;
    const B: u32 = 0xe020_0002;
    const C: u32 = 0xe000_0003;
    const D: u32 = 0xe020_0004;
    const E: u32 = 0xe020_0005;
    const F: u32 = 0xe020_0006;
    const G: u32 = 0xe000_0007;
    const H: u32 = 0xe000_0008;
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
    g.put_u16(COUNT, 64);
    // [esp]: the fill's return; then player, category, count, handles, flag.
    for (i, v) in [FILL_RETURN, 0, category, COUNT, HANDLES, 1]
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
            assert_eq!(m.u16(COUNT), Ok(64), "each category gets the full buffer");
            let asked = call.args[1];
            assert_eq!(call.args, [0, asked, COUNT, HANDLES, 1]);
            let got: &[u32] = match asked {
                SLAYER_CATEGORY => &[A, B, C, D, H],
                JUGGERNAUT_CATEGORY => &[E, F, G],
                _ => &[],
            };
            m.set_u16(COUNT, got.len() as u16).unwrap();
            for (i, h) in got.iter().enumerate() {
                m.set_u32(HANDLES + 4 * i as u32, *h).unwrap();
            }
            Regs::default()
        }
        LOAD_VARIANT => {
            assert_eq!(call.eax, Some(SCRATCH));
            let (name, block, ty) = match call.edx {
                Some(A) => ("Rockets", [0; ext::LEN], GameType::Slayer),
                Some(B) => ("Superbounce \u{e006}", block_of("Superbounce"), GameType::Slayer),
                Some(D) => ("Zombies \u{e006}", [0; ext::LEN], GameType::Slayer),
                Some(E) => ("Hide and Seek \u{e006}", [0; ext::LEN], GameType::Juggernaut),
                Some(F) => ("Clint Eastwood \u{e006}", [0; ext::LEN], GameType::Juggernaut),
                Some(H) => ("Copied Clint", block_of("Clint Eastwood"), GameType::Script),
                Some(G) => ("My Hide", block_of("Hide and Seek"), GameType::Juggernaut),
                _ => ("My Bounce", block_of("Superbounce"), GameType::Slayer),
            };
            m.fill(SCRATCH, 0x130, 0).unwrap();
            put_typed(m, SCRATCH, name, block, ty);
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
    let (run, rows) = run_fill(Some(Row::PartyGames), true);
    let g = &run.guest;
    assert_eq!(
        read_handles(g, COUNT, HANDLES),
        Ok(vec![0xe020_0002, 0xe000_0003, 0xe020_0004, 0xe020_0005, 0xe000_0007]),
        "Superbounce and its renamed copy, Zombies, then Hide and Seek and its saved copy from Juggernaut's category"
    );
    let create = DatumArray::at(ARRAY).try_get(g, rows[0]).unwrap();
    assert!(
        create.is_none(),
        "a new variant here would be a plain Slayer"
    );
    // Another row's list holds none of them.
    let (run, _) = run_fill(Some(Row::Siege), true);
    let kept = read_handles(&run.guest, COUNT, HANDLES).unwrap();
    assert!(!kept.contains(&0xe020_0002) && !kept.contains(&0xe020_0004), "{kept:x?}");
    // The lobby's list has no Create New and keeps its rows.
    let (run, rows) = run_fill(Some(Row::PartyGames), false);
    assert!(
        DatumArray::at(ARRAY)
            .try_get(&run.guest, rows[0])
            .unwrap()
            .is_some()
    );
}

#[test]
fn hide_and_seek_lists_under_party_games() {
    // The row's list asks Slayer's category, then Juggernaut's.
    let (run, rows) = run_fill(Some(Row::PartyGames), true);
    let asked: Vec<u32> = run
        .calls
        .iter()
        .filter(|c| c.target == points::VARIANTS_ENUMERATE.base)
        .map(|c| c.args[1])
        .collect();
    let all: Vec<_> = GameType::ALL.into_iter().filter_map(GameType::content_type).collect();
    assert_eq!(asked, all);
    let kept = read_handles(&run.guest, COUNT, HANDLES).unwrap();
    assert!(kept.contains(&0xe020_0005), "the built-in Hide and Seek");
    assert!(kept.contains(&0xe000_0007), "a saved copy of it");
    assert!(!kept.contains(&0xe020_0006), "Clint Eastwood lists under no row");
    let create = DatumArray::at(ARRAY).try_get(&run.guest, rows[0]).unwrap();
    assert!(create.is_none(), "Create New stays dropped on a row's list");
    // Another row does not hold it.
    let (run, _) = run_fill(Some(Row::Firefight), true);
    let kept = read_handles(&run.guest, COUNT, HANDLES).unwrap();
    assert!(!kept.contains(&0xe020_0005) && !kept.contains(&0xe000_0007));
}

#[test]
fn juggernauts_own_list_drops_hide_and_seek_and_keeps_the_rest() {
    let (run, rows) = run_fill_in(None, false, JUGGERNAUT_CATEGORY);
    assert_eq!(
        run.calls
            .iter()
            .filter(|c| c.target == points::VARIANTS_ENUMERATE.base)
            .count(),
        GameType::ALL.len(),
        "a renamed script may have been saved under any category"
    );
    assert_eq!(
        read_handles(&run.guest, COUNT, HANDLES),
        Ok(vec![0xe020_0006, 0xe000_0008]),
        "Clint Eastwood and an index-6 copy saved under Slayer"
    );
    assert!(DatumArray::at(ARRAY).try_get(&run.guest, rows[0]).unwrap().is_some());
}

#[test]
fn an_unrelated_content_category_is_left_alone() {
    let mut g = FakeGuest::new();
    g.map(SCREEN, 0x5000)
        .map(STACK, 0x100)
        .map(COUNT, 4)
        .map(HANDLES, 0x100);
    for (i, v) in [FILL_RETURN, 0, 99, COUNT, HANDLES, 1]
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
fn forwarding_each_category_leaves_its_datum_alone_even_if_retail_replaces_it() {
    for ours in Row::ALL {
        let mut g = FakeGuest::new();
        g.map(SCREEN, 0x5000).map(STACK, 0x100);
        let handles = rows_with(&mut g, HANDLER, &[SLAYER_WORD, word_of_row(ours)], 4);
        let old = DatumArray::at(ARRAY).try_get(&g, handles[1]).unwrap().unwrap();
        let f = move |cx: Cx| -> EntryFuture { Box::pin(press(cx, ours, HANDLER, STACK)) };
        let run = entry::testing::run(&f, g, Regs { esp: STACK + 0x80, ..Default::default() }, |call, m| {
            assert_eq!(call.target, points::VARIANT_CATEGORY_ROW_CHOSEN.base);
            assert_eq!(call.args.as_slice(), &[STACK, STACK + 0x7c]);
            assert_eq!(m.u32(STACK + 0x7c), Ok(handles[0].0));
            assert_eq!(m.u16(old + ROW_WORD), Ok(word_of_row(ours) as u16));
            m.set_u16(old + ROW_WORD, 0xaabc).unwrap(); // allocation reused during the press
            Regs::default()
        });
        assert_eq!(run.result, Ok(Ret::new(0, 8)));
        assert_eq!(run.guest.u16(old + ROW_WORD), Ok(0xaabc));
    }
}

#[test]
fn a_filtered_empty_list_still_receives_back_without_stealing_another_screens_focus() {
    use crate::ui_kit::lifecycle::{SCREEN_SLOT, SCREEN_WINDOW};
    use crate::ui_watch::{ACTIVE, PENDING, SHARED, channel};
    let mut m = FakeGuest::new();
    let ch = channel(3, SHARED).unwrap();
    let focus = ch + 0x34;
    m.map(SCREEN, 0x5000)
        .put_u32(SCREEN + SCREEN_WINDOW, 3)
        .put_u32(SCREEN + SCREEN_SLOT, SHARED)
        .put_u32(ch + ACTIVE, SCREEN);
    rows_with(&mut m, HANDLER, &[], 4);
    for group in Row::ALL {
        m.put_u8(SCREEN + SCREEN_MARK, group.mark()).put_u32(focus, 0);
        focus_empty_list(&mut m, SCREEN).unwrap();
        assert_eq!(m.u32(focus), Ok(SCREEN));
    }
    for (active, pending, bound) in [(SCREEN, 0, HANDLER), (SCREEN, HANDLER, 0), (HANDLER, 0, 0)] {
        m.put_u32(ch + ACTIVE, active).put_u32(ch + PENDING, pending).put_u32(focus, bound);
        focus_empty_list(&mut m, SCREEN).unwrap();
        assert_eq!(m.u32(focus), Ok(bound));
    }
    DatumArray::at(ARRAY).new_datum(&mut m).unwrap().unwrap();
    m.put_u32(ch + ACTIVE, SCREEN);
    focus_empty_list(&mut m, SCREEN).unwrap();
    assert_eq!(m.u32(focus), Ok(0), "retail binds nonempty lists");
}

#[test]
fn our_pane_is_the_row_with_our_word() {
    let mut g = FakeGuest::new();
    g.map(SCREEN, 0x5000);
    rows_with(&mut g, HANDLER, &with_row(&GAME_TYPE_WORDS), 4);
    assert_eq!(our_pane(&g, SCREEN, 0xcf, 7), Some(Row::PartyGames));
    assert_eq!(our_pane(&g, SCREEN, 0x3c, 7), Some(Row::PartyGames));
    assert_eq!(our_pane(&g, SCREEN, 0xcf, 6), None, "Territories has its pane");
    assert_eq!(our_pane(&g, SCREEN, 0xd0, 7), None, "another screen");
    assert_eq!(our_pane(&g, SCREEN, 0xcf, 8), Some(Row::Firefight));
    assert_eq!(our_pane(&g, SCREEN, 0xcf, 9), Some(Row::Siege));
    assert_eq!(our_pane(&g, SCREEN, 0xcf, 11), None);
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

#[test]
fn the_category_constructor_reserves_room_for_all_our_rows() {
    let mut g = FakeGuest::new();
    g.map(SCREEN, 0x5000);
    let list = HANDLER;
    assert_eq!(list_capacity(9), 9);
    BUILDING_CATEGORY.set(true);
    let capacity = list_capacity(9);
    assert_eq!(list_capacity(16), 16);
    BUILDING_CATEGORY.set(false);
    assert_eq!(capacity, 10);
    // The list as retail builds it once the constructor reserved the room.
    rows_in(&mut g, list, &GAME_TYPE_WORDS, 4, capacity as i32);
    let n = h1_options_row::insert_row_ordered(&mut g, list, with_row).unwrap();
    assert_eq!(n, capacity as usize);
    let words: Vec<i16> = (0..capacity as i32).map(|i| word_at(&g, list, i).unwrap()).collect();
    assert_eq!(words, with_row(&GAME_TYPE_WORDS));
    // No room for more: the list is left as it was, not half-added.
    assert!(h1_options_row::insert_row_ordered(&mut g, list, |w| {
        let mut w = w.to_vec();
        w.push(13);
        w
    })
    .is_err());
    assert_eq!(word_at(&g, list, 7), Some(PARTY_WORD));
    assert_eq!(word_at(&g, list, 9), Some(SIEGE_WORD));
    assert_eq!(word_at(&g, list, 10), None);
}
