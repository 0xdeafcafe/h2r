use super::*;
use engine::DatumHandle;
use engine::datum::DatumArray;
use engine::entry::{self, Regs};
use engine::testing::FakeGuest;

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

/// A variant record at `at` named `name` with extension block `block`.
fn put_record(m: &mut dyn GuestMemory, at: u32, name: &str, block: [u8; ext::LEN]) {
    m.write(at + variants::record::NAME_AT as u32, &utf16(name))
        .unwrap();
    m.write(at + ext::AT as u32, &block).unwrap();
}

#[test]
fn tweakers_is_the_last_row_after_territories() {
    let words = with_row(&GAME_TYPE_WORDS);
    assert_eq!(words, [0, 1, 3, 4, 6, 7, 8, TWEAKERS_WORD]);
    assert_eq!(words[words.len() - 2], 8, "Territories just above it");
    assert_eq!(with_row(&words), words, "added once");
    // Two retail rows are left out of some lists; ours is still last.
    assert_eq!(with_row(&[0, 1, 3, 6, 7]), [0, 1, 3, 6, 7, TWEAKERS_WORD]);
    assert!(!GAME_TYPE_WORDS.contains(&TWEAKERS_WORD));
}

#[test]
fn both_game_type_screens_get_the_row_but_not_the_create_mode() {
    assert!(wants_row(0xcf, false), "Game Setup's Change Rules");
    assert!(wants_row(0x3c, false), "Settings, Game Variants");
    assert!(!wants_row(0x3c, true), "a new variant of a game type");
    assert!(!wants_row(0xb2, false), "another screen");
}

#[test]
fn superbounce_is_under_tweakers_and_no_longer_under_slayer() {
    let tweakers: Vec<_> = tweakers_variants().map(|v| v.name).collect();
    assert_eq!(tweakers, ["Superbounce"]);
    let slayer: Vec<_> = VARIANTS
        .iter()
        .filter(|v| v.ty == community::GameType::Slayer && !is_tweakers_variant(v))
        .map(|v| v.name)
        .collect();
    assert!(!slayer.contains(&"Superbounce"), "{slayer:?}");
    assert!(slayer.contains(&"Zombies"), "the rest stay: {slayer:?}");

    let builtin = community::is_tweaker("Superbounce \u{e006}", &[0; ext::LEN]);
    let zombies = community::is_tweaker("Zombies \u{e006}", &[0; ext::LEN]);
    // Slayer's list drops it, Tweakers' keeps it; the other way for the rest.
    assert!(!keeps(false, true, builtin) && keeps(true, true, builtin));
    assert!(keeps(false, true, zombies) && !keeps(true, true, zombies));
    // A record that did not load stays retail's.
    assert!(keeps(false, false, true) && !keeps(true, false, true));
}

#[test]
fn every_tweaker_is_a_slayer_variant() {
    // The Tweakers list is Slayer's list filtered: its new variants, copies
    // and defaults are Slayer's.
    for v in tweakers_variants() {
        assert_eq!(v.ty, community::GameType::Slayer, "{}", v.name);
        assert!(v.tweak().is_some_and(|e| !e.is_default()), "{}", v.name);
    }
    assert!(filtered(SLAYER_CATEGORY));
    assert!(!filtered(2), "King of the Hill's list is retail's");
}

#[test]
fn a_saved_copy_with_the_block_stays_under_tweakers() {
    let flag = community::superbounce_ext().encode();
    assert!(community::is_tweaker("My Bounce", &flag), "renamed copy");
    assert!(
        community::is_tweaker("Superbounce", &[0; ext::LEN]),
        "by name"
    );
    assert!(!community::is_tweaker("Slayer", &[0; ext::LEN]));
    let gravity = Ext {
        gravity: 50,
        ..Ext::RETAIL
    };
    assert!(
        !community::is_tweaker("Moon Slayer", &gravity.encode()),
        "More Options alone is not a tweak"
    );
    // A block the game would ignore names no script.
    let mut bad = flag;
    bad[ext::LEN - 1] ^= 1;
    assert!(!community::is_tweaker("My Bounce", &bad));
}

#[test]
fn the_builtin_records_get_their_extension_block_once() {
    let mut g = FakeGuest::new();
    g.map(BUILTIN_LIST & !0xfff, 0x1000).map(RECORDS, 0x2000);
    g.put_u32(BUILTIN_LIST, RECORDS).put_u32(RECORDS, 2);
    let rec = |i: u32| RECORDS + 4 + i * BUILTIN_RECORD + BUILTIN_VARIANT;
    put_record(&mut g, rec(0), "Zombies \u{e006}", [0; ext::LEN]);
    put_record(&mut g, rec(1), "Superbounce \u{e006}", [0; ext::LEN]);
    assert_eq!(patch_builtins(&mut g), Ok(1));
    assert_eq!(
        Ext::decode(&record_block(&g, rec(1)).unwrap()),
        Block::Ext(community::superbounce_ext())
    );
    assert_eq!(record_block(&g, rec(0)), Some([0; ext::LEN]), "untouched");
    assert!(record_is_tweaker(&g, rec(1)) && !record_is_tweaker(&g, rec(0)));
    assert_eq!(patch_builtins(&mut g), Ok(0), "already there");
    // No list yet: nothing to do.
    g.put_u32(BUILTIN_LIST, 0);
    assert_eq!(patch_builtins(&mut g), Ok(0));
}

/// The variant list's fill, as `0x2ca0d9` calls `0x215900`: three handles
/// come back, a Slayer variant, the built-in Superbounce and a saved copy
/// of it under another name.
fn run_fill(tweakers_list: bool, create: bool) -> (entry::testing::Run, Vec<DatumHandle>) {
    const A: u32 = 0xe000_0001;
    const B: u32 = 0xe020_0002;
    const C: u32 = 0xe000_0003;
    let mut g = FakeGuest::new();
    g.map(SCREEN, 0x5000)
        .map(STACK, 0x100)
        .map(COUNT, 4)
        .map(HANDLES, 0x100);
    g.map(SCRATCH, 0x200).map(BUILTIN_LIST & !0xfff, 0x1000);
    g.put_u8(SCREEN + SCREEN_MARK, u8::from(tweakers_list));
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
            m.set_u16(COUNT, 3).unwrap();
            for (i, h) in [A, B, C].into_iter().enumerate() {
                m.set_u32(HANDLES + 4 * i as u32, h).unwrap();
            }
            Regs::default()
        }
        LOAD_VARIANT => {
            assert_eq!(call.eax, Some(SCRATCH));
            let (name, block) = match call.edx {
                Some(A) => ("Rockets", [0; ext::LEN]),
                Some(B) => ("Superbounce \u{e006}", community::superbounce_ext().encode()),
                _ => ("My Bounce", community::superbounce_ext().encode()),
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
fn slayers_list_keeps_everything_but_the_tweakers() {
    let (run, rows) = run_fill(false, true);
    let g = &run.guest;
    assert_eq!(read_handles(g, COUNT, HANDLES), Ok(vec![0xe000_0001]));
    let create = DatumArray::at(ARRAY).try_get(g, rows[0]).unwrap();
    assert!(create.is_some(), "Slayer keeps Create New");
}

#[test]
fn the_tweakers_list_keeps_the_tweakers_and_drops_create_new() {
    let (run, rows) = run_fill(true, true);
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
    // The lobby's list has no Create New and keeps its rows.
    let (run, rows) = run_fill(true, false);
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
    assert!(is_our_pane(&g, SCREEN, 0xcf, 7));
    assert!(is_our_pane(&g, SCREEN, 0x3c, 7));
    assert!(
        !is_our_pane(&g, SCREEN, 0xcf, 6),
        "Territories has its pane"
    );
    assert!(!is_our_pane(&g, SCREEN, 0xd0, 7), "another screen");
    assert!(!is_our_pane(&g, SCREEN, 0xcf, 8), "no row there");
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
    let f = |cx: Cx| -> EntryFuture { Box::pin(pane(cx, ORIGINAL, SCREEN, 0, WORD)) };
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
            (desc, community::TWEAKERS_DESCRIPTION_ID),
            (title, community::TWEAKERS_TITLE_ID)
        ]
    );
}

#[test]
fn a_tweakers_list_reopens_through_slayers_loader_for_its_mode() {
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
    assert!(!is_tweakers_screen(&g, SCREEN));
    g.put_u8(SCREEN + SCREEN_MARK, 1);
    assert!(is_tweakers_screen(&g, SCREEN));
}

#[test]
fn the_row_reads_in_the_games_words() {
    use crate::community_variants::{TWEAKERS_DESCRIPTION, TWEAKERS_LABEL, TWEAKERS_TITLE};
    assert_eq!(
        community::text(community::TWEAKERS_LABEL_ID).as_deref(),
        Some("Tweakers")
    );
    assert_eq!(
        TWEAKERS_TITLE,
        TWEAKERS_LABEL.to_uppercase(),
        "as SLAYER is Slayer's"
    );
    assert!(TWEAKERS_DESCRIPTION.ends_with('.') && TWEAKERS_DESCRIPTION.len() < 120);
    for id in [
        community::TWEAKERS_LABEL_ID,
        community::TWEAKERS_TITLE_ID,
        community::TWEAKERS_DESCRIPTION_ID,
    ] {
        assert!(community::is_ours(id));
        for i in 0..VARIANTS.len() {
            assert_ne!(id, community::name_id(i));
            assert_ne!(id, community::description_id(i));
        }
    }
}

#[test]
fn the_hooks_are_on_the_plan() {
    let mut r = hooks::Registry::new(hooks::Switches::defaults());
    r.add(&Tweakers).unwrap();
    let text = r.build(&hooks::Base).unwrap().text();
    for (point, label) in [
        (
            "VARIANT_CATEGORY_SCREEN_CTOR",
            "after [tweakers:add the Tweakers row]",
        ),
        (
            "VARIANT_CATEGORY_ROW_TEXT",
            "before [tweakers:the Tweakers row's text]",
        ),
        (
            "VARIANT_CATEGORY_ROW_CHOSEN",
            "before [tweakers:the Tweakers row's press]",
        ),
        (
            "VARIANT_LIST_SCREEN_CTOR",
            "after [tweakers:mark a Tweakers list]",
        ),
        (
            "VARIANTS_ENUMERATE",
            "before [tweakers:keep a list's tweakers apart]",
        ),
        (
            "VARIANT_LIST_UPDATE",
            "before [tweakers:a Tweakers list's title and description]",
        ),
        (
            "VARIANT_LIST_LOADER",
            "before [tweakers:a Tweakers list reopens as one]",
        ),
    ] {
        let line = text
            .lines()
            .find(|l| l.contains(&format!(" {point}: ")))
            .unwrap_or_else(|| panic!("{point} not on the plan:\n{text}"));
        assert!(line.contains(label), "{line}");
    }
}
