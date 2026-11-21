use super::*;
use crate::lobby_tabs::{BAND_BOTTOM, BAND_TOP, Reason};
use engine::testing::FakeGuest;

const SCREEN: u32 = 0x1000;
const AREA: u32 = 0x10000;
const HEADER_AT: u32 = AREA;
const FOOTER_AT: u32 = AREA + 0x100;
const COUNT_AT: u32 = AREA + 0x200;
const BITMAP_AT: u32 = AREA + 0x300;
/// Where the strip's widgets start.
const OURS: u32 = AREA + 0x1000;

/// Split Screen, no kit map: MATCHMAKING, FIREFIGHT and CLANKERS off.
fn split_screen(leader: bool, selected: Tab) -> Shown {
    Shown {
        place: Place::SplitScreen,
        leader,
        selected,
        row: crate::lobby_tabs::row(Present::ALL, Place::SplitScreen, Facts::default()),
    }
}

fn text(g: &mut FakeGuest, at: u32, index: u16, next: u32) {
    g.put_u32(at, TEXT_VTABLE)
        .put_u32(at + WIDGET_KIND, TEXT_KIND)
        .put_u32(at + WIDGET_NEXT, next);
    g.set_u16(at + WIDGET_INDEX, index).unwrap();
    g.put_u8(at + WIDGET_VISIBLE, 1);
}

/// A lobby screen: retail's header, footer, player count and a bitmap,
/// then a widget for each of `pieces`, `stride` apart from `base`.
fn lobby(pieces: &[Piece], base: u32, stride: u32) -> FakeGuest {
    let mut g = FakeGuest::new();
    g.map(SCREEN, 0x1000).map(AREA, 0x10000);
    g.put_u32(SCREEN + WIDGET_CHILD, HEADER_AT);
    text(&mut g, HEADER_AT, 0, FOOTER_AT);
    text(&mut g, FOOTER_AT, 1, COUNT_AT);
    let first = if pieces.is_empty() { 0 } else { base };
    text(&mut g, COUNT_AT, 2, BITMAP_AT);
    put_rect(&mut g, COUNT_AT, COUNT).unwrap();
    g.put_u32(BITMAP_AT + WIDGET_KIND, 8)
        .put_u32(BITMAP_AT + WIDGET_NEXT, first);
    for (i, p) in pieces.iter().enumerate() {
        let at = base + stride * i as u32;
        let next = if i + 1 == pieces.len() {
            0
        } else {
            at + stride
        };
        text(&mut g, at, p.index(), next);
    }
    g
}

fn visible(g: &FakeGuest, at: u32) -> bool {
    g.u8(at + WIDGET_VISIBLE).unwrap() != 0
}

#[test]
fn every_piece_has_its_own_index() {
    let all = pieces(Present::ALL);
    assert_eq!(all.len(), usize::from(WIDGETS));
    for p in &all {
        assert_eq!(Piece::of_index(p.index()), Some(*p), "{p:?}");
        assert!(is_ours(p.index()));
    }
    let mut idx: Vec<u16> = all.iter().map(|p| p.index()).collect();
    idx.sort();
    idx.dedup();
    assert_eq!(idx.len(), all.len());
    assert_eq!(Piece::of_index(BLOCK_BASE + WIDGETS), None);
    assert!(!is_ours(0) && !is_ours(9), "the tag's own blocks");
}

#[test]
fn one_look_shows_per_tab() {
    let all = pieces(Present::ALL);
    let mut g = lobby(&all, OURS, 0x300);
    let found = find(&g, SCREEN).unwrap();
    assert_eq!(found.ours.len(), all.len());
    let s = split_screen(true, Tab::Forge);
    show(&mut g, &found, &s).unwrap();
    for tab in Tab::ALL {
        let shown: Vec<Look> = found
            .ours
            .iter()
            .filter_map(|&(p, at)| match p {
                Piece::Cell(t, l) if t == tab && visible(&g, at) => Some(l),
                _ => None,
            })
            .collect();
        let want = match tab {
            Tab::Forge => Look::Selected,
            Tab::CustomGames | Tab::CoOp => Look::On,
            _ => Look::Off,
        };
        assert_eq!(shown, [want], "{tab:?}");
    }
    // One look per glyph: White bright (CUSTOM GAMES is on), Black bright.
    for dir in [Dir::Prev, Dir::Next] {
        let lit: Vec<bool> = found
            .ours
            .iter()
            .filter_map(|&(p, at)| match p {
                Piece::Glyph(d, on) if d == dir && visible(&g, at) => Some(on),
                _ => None,
            })
            .collect();
        assert_eq!(lit, [true], "{dir:?}");
    }
    // At CUSTOM GAMES, White's way has only MATCHMAKING, which is off.
    let s = split_screen(true, Tab::CustomGames);
    show(&mut g, &found, &s).unwrap();
    let white: Vec<bool> = found
        .ours
        .iter()
        .filter(|(p, at)| matches!(p, Piece::Glyph(Dir::Prev, _)) && visible(&g, *at))
        .map(|(p, _)| matches!(p, Piece::Glyph(_, true)))
        .collect();
    assert_eq!(white, [false], "White dimmed");
}

#[test]
fn the_header_is_hidden_and_put_back_with_the_strip_off() {
    let all = pieces(Present::ALL);
    let mut g = lobby(&all, OURS, 0x300);
    let found = find(&g, SCREEN).unwrap();
    assert_eq!(found.header, Some(HEADER_AT));
    assert_eq!(found.count, Some(COUNT_AT));
    show(&mut g, &found, &split_screen(true, Tab::CustomGames)).unwrap();
    assert!(!visible(&g, HEADER_AT), "the selected tab is the title");
    assert!(visible(&g, FOOTER_AT), "the footer stays");
    assert_eq!(rect_at(&g, COUNT_AT), Some(COUNT_NARROW));
    // Found again with the count narrowed.
    let again = find(&g, SCREEN).unwrap();
    assert_eq!(again.count, Some(COUNT_AT));
    put_back(&mut g, &again).unwrap();
    assert!(visible(&g, HEADER_AT));
    assert_eq!(rect_at(&g, COUNT_AT), Some(COUNT));
    assert!(again.ours.iter().all(|(_, at)| !visible(&g, *at)));
    // A lobby the strip never touched is left as it is.
    let mut fresh = lobby(&[], OURS, 0x300);
    fresh.put_u8(HEADER_AT + WIDGET_VISIBLE, 0);
    let f = find(&fresh, SCREEN).unwrap();
    put_back(&mut fresh, &f).unwrap();
    assert!(!visible(&fresh, HEADER_AT), "retail's own visibility");
}

#[test]
fn the_strip_is_found_again_by_its_ids() {
    let mut all = pieces(Present::ALL);
    let g = lobby(&all, OURS, 0x300);
    let first = find(&g, SCREEN).unwrap();
    assert!(missing(&first, Present::ALL).is_empty());
    // The pane rebuilt: the same pieces at other addresses, in another
    // order. Each is found by its block index, not its address.
    all.reverse();
    let rebuilt = lobby(&all, OURS + 0x8000, 0x200);
    let again = find(&rebuilt, SCREEN).unwrap();
    for (p, at) in &again.ours {
        let i = all.iter().position(|q| q == p).unwrap() as u32;
        assert_eq!(*at, OURS + 0x8000 + 0x200 * i, "{p:?}");
    }
    assert_eq!(
        again.header,
        Some(HEADER_AT),
        "ours never read as the header"
    );
    // A pane with none of ours: every piece is missing, so the update
    // adds them all.
    let bare = lobby(&[], OURS, 0x300);
    let f = find(&bare, SCREEN).unwrap();
    assert!(f.ours.is_empty());
    assert_eq!(missing(&f, Present::ALL), pieces(Present::ALL));
    // Only the tabs present are wanted.
    let no_forge = Present {
        forge: false,
        ..Present::ALL
    };
    assert!(
        !missing(&f, no_forge)
            .iter()
            .any(|p| matches!(p, Piece::Cell(Tab::Forge, _)))
    );
    // Ours placed before retail's texts: the header is still retail's.
    let mut g = lobby(&[], OURS, 0x300);
    let ours = OURS;
    text(
        &mut g,
        ours,
        Piece::Cell(Tab::Forge, Look::On).index(),
        HEADER_AT,
    );
    g.put_u32(SCREEN + WIDGET_CHILD, ours);
    g.put_u32(BITMAP_AT + WIDGET_NEXT, 0);
    let f = find(&g, SCREEN).unwrap();
    assert_eq!(f.header, Some(HEADER_AT));
    assert_eq!(f.ours, [(Piece::Cell(Tab::Forge, Look::On), ours)]);
}

#[test]
fn a_member_has_no_glyphs() {
    let all = pieces(Present::ALL);
    let mut g = lobby(&all, OURS, 0x300);
    let found = find(&g, SCREEN).unwrap();
    show(&mut g, &found, &split_screen(false, Tab::CustomGames)).unwrap();
    for (p, at) in &found.ours {
        if matches!(p, Piece::Glyph(..)) {
            assert!(!visible(&g, *at), "{p:?}");
        }
    }
    // The tabs still show: a member's strip is a read-out.
    assert!(
        found
            .ours
            .iter()
            .any(|(p, at)| *p == Piece::Cell(Tab::CustomGames, Look::Selected) && visible(&g, *at))
    );
}

#[test]
fn the_strip_takes_the_lobby_texts_animation() {
    let present = Present::ALL;
    for p in pieces(present) {
        let b = block(p, present).unwrap();
        let u16_at = |o: usize| u16::from_le_bytes([b[o], b[o + 1]]);
        let i16_at = |o: usize| i16::from_le_bytes([b[o], b[o + 1]]);
        assert_eq!(u16_at(4), TEXT_ANIMATION, "{p:?}");
        assert_eq!(i16_at(0x28), TEXT_BIAS, "{p:?}");
        assert_eq!(i16_at(0xa), FONT, "{p:?}");
        assert_eq!(
            u32::from_le_bytes(b[0..4].try_into().unwrap()),
            0,
            "{p:?}: centred"
        );
        assert_eq!(
            u32::from_le_bytes(b[0x24..0x28].try_into().unwrap()),
            p.id(),
            "{p:?}: its string"
        );
        let rect = [0x1c, 0x1e, 0x20, 0x22].map(i16_at);
        assert_eq!(Some(rect), p.rect(present), "{p:?}");
    }
    assert_eq!(Piece::Cell(Tab::Forge, Look::Selected).colour(), WHITE);
    assert_eq!(Piece::Cell(Tab::Forge, Look::On).colour(), PALE_BLUE);
    let off = Piece::Cell(Tab::Forge, Look::Off).colour();
    assert!((off[2] - PALE_BLUE[2] * DIM).abs() < 1e-6);
    // An absent tab has no cell, so no block.
    let no_forge = Present {
        forge: false,
        ..Present::ALL
    };
    assert_eq!(block(Piece::Cell(Tab::Forge, Look::On), no_forge), None);
}

#[test]
fn the_place_and_the_retail_mode_come_from_the_session() {
    assert_eq!(place(false, None), Some(Place::SplitScreen));
    assert_eq!(place(false, Some(2)), Some(Place::SplitScreen));
    assert_eq!(place(true, Some(2)), Some(Place::XboxLive));
    assert_eq!(place(true, Some(1)), Some(Place::SystemLink));
    assert_eq!(place(true, None), None, "a networked lobby with no session");
    assert_eq!(retail_mode(false, 0, false), RetailMode::Custom);
    assert_eq!(retail_mode(true, 6, false), RetailMode::Matchmaking);
    assert_eq!(retail_mode(true, 7, true), RetailMode::Matchmaking);
    assert_eq!(retail_mode(true, 1, true), RetailMode::CoOp);
    assert_eq!(
        retail_mode(false, 6, false),
        RetailMode::Custom,
        "not networked"
    );
}

#[test]
fn a_member_reads_its_tab_from_the_lobby() {
    let reading = Reading {
        place: Place::SystemLink,
        leader: false,
        retail: RetailMode::Custom,
        kit_map: true,
        facts: Facts {
            kit_maps: true,
            ..Facts::default()
        },
    };
    let s = shown(&reading, Present::ALL, Mode::Multiplayer, false);
    assert_eq!(s.selected, Tab::Firefight, "a kit map in a custom lobby");
    let leader = Reading {
        leader: true,
        ..reading
    };
    assert_eq!(
        shown(&leader, Present::ALL, Mode::Forge, false).selected,
        Tab::Forge
    );
}

#[test]
fn ui_lobby_tabs_answers_each_tab_with_its_state_and_reason() {
    let s = split_screen(true, Tab::CustomGames);
    let v = json_of(&s, Some(Reason::MatchmakingNeedsLive));
    assert_eq!(v["place"], "split_screen");
    assert_eq!(v["leader"], true);
    assert_eq!(v["selected"], "custom_games");
    assert_eq!(v["notice"], "Matchmaking needs Xbox Live.");
    let tabs = v["tabs"].as_array().unwrap();
    assert_eq!(tabs.len(), 6);
    assert_eq!(tabs[0]["tab"], "matchmaking");
    assert_eq!(tabs[0]["state"], "off");
    assert_eq!(tabs[0]["reason"], "Matchmaking needs Xbox Live.");
    assert_eq!(tabs[1]["state"], "selected");
    assert_eq!(tabs[1]["label"], "CUSTOM GAMES");
    assert!(tabs[1]["reason"].is_null());
    assert_eq!(tabs[2]["state"], "on");
    assert_eq!(tabs[5]["reason"], "Clankermatch needs Xbox Live.");
    assert!(json_of(&s, None)["notice"].is_null());
}

// ---- against the disc ----

fn disc() -> crate::ui_kit::preview::Disc {
    let iso = std::path::PathBuf::from(std::env::var("H2_DISC").expect("H2_DISC"));
    crate::ui_kit::preview::Disc::open(&iso).unwrap()
}

/// The lobby's pane 0 text block at `rect`.
fn lobby_block(d: &crate::ui_kit::preview::Disc, rect: [i16; 4]) -> tags::ui::TextBlock {
    let lobby = d
        .screen(crate::mods_screen::page_layout::LOBBY_SCREEN_ID as i16)
        .expect("pregame_lobby");
    lobby.panes[0]
        .text_blocks
        .iter()
        .find(|b| [b.rect.top, b.rect.left, b.rect.bottom, b.rect.right] == rect)
        .unwrap_or_else(|| panic!("no lobby text block at {rect:?}"))
        .clone()
}

#[test]
#[ignore = "reads the disc named by H2_DISC"]
fn every_tab_label_fits_its_cell() {
    let d = disc();
    for (tab, [top, left, bottom, right]) in cells(&Tab::ALL) {
        let w = d.fonts.width(FONT, tab.label());
        assert!(
            w <= i32::from(right - left),
            "{}: {w} of {}",
            tab.label(),
            right - left
        );
        assert!(d.fonts.line_height(FONT) <= i32::from(top - bottom));
    }
    for (glyph, [_, left, _, right]) in [
        (crate::lobby_tabs::PREV_TEXT, PREV),
        (crate::lobby_tabs::NEXT_TEXT, NEXT),
    ] {
        let w = d.fonts.width(FONT, glyph);
        assert!(w <= i32::from(right - left), "{glyph:?}: {w}");
    }
}

/// The count's widest text, "16 player(s) in party" in the lobby's own
/// form, fits the 80 units the strip leaves it, right-justified so the
/// narrowing keeps its right edge.
#[test]
#[ignore = "reads the disc named by H2_DISC"]
fn the_player_count_fits_its_narrowed_rectangle() {
    let d = disc();
    let b = lobby_block(&d, COUNT);
    assert_eq!(b.flags & 3, 2, "the player count is right-justified");
    let room = i32::from(COUNT_NARROW[3] - COUNT_NARROW[1]);
    let w = d.fonts.width(b.font, "16 player(s) in party");
    assert!(w <= room, "{w} of {room}");
}

#[test]
#[ignore = "reads the disc named by H2_DISC"]
fn the_strip_cells_are_inside_the_header_band() {
    let d = disc();
    let header = lobby_block(&d, HEADER);
    assert_eq!(
        [header.rect.top, header.rect.bottom],
        [BAND_TOP, BAND_BOTTOM]
    );
    let lobby = d
        .screen(crate::mods_screen::page_layout::LOBBY_SCREEN_ID as i16)
        .unwrap();
    // Nothing of the tag's besides the header and the count is in the band.
    for b in &lobby.panes[0].text_blocks {
        let r = [b.rect.top, b.rect.left, b.rect.bottom, b.rect.right];
        if r == HEADER || r == COUNT {
            continue;
        }
        assert!(b.rect.top <= BAND_BOTTOM, "{r:?} is in the header band");
    }
    let mut pieces = vec![PREV, NEXT];
    pieces.extend(cells(&Tab::ALL).into_iter().map(|(_, c)| c));
    for [top, left, bottom, right] in pieces {
        assert!(top <= header.rect.top && bottom >= header.rect.bottom);
        assert!(left >= header.rect.left && right < COUNT_NARROW[1]);
    }
}
