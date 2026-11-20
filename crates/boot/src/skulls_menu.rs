//! The in-game Skulls page (docs/game/skulls.md "Menu"): after the campaign
//! difficulty is chosen, a page of the pressing player's skulls, when that
//! profile has collected any, before the level starts.
//!
//! **The flow** is `crate::skulls_screen`'s: a press on a difficulty opens
//! this page instead of starting, and A, X or Start on the page runs the
//! difficulty's own press, which starts the level.
//!
//! **The screen** is our own list class (`crate::list_class`, as the Video
//! screen is), feature `skulls-screen`, showing a [`SkullsScreen`]: every
//! skull, gold first, each its name and the profile's On,
//! Off or Locked in one label; a description text widget of ours with the
//! focused skull's effect and where it is found. Left or right on a
//! collected skull flips it in the store at once ([`skulls::toggle`]), and
//! its row reads "< On >" or "< Off >" as a value to step; the next
//! campaign start turns on exactly the On ones ([`skulls::selection`]). A
//! goes on to the level. B returns to the difficulty list. (The
//! multiplayer instance keeps A as its toggle.)
//!
//! **The layout** is the campaign level select's
//! (`ui\screens\game_shell\load_level\load_level`, screen 11): its list
//! (anchor, default skin) on the left, its picture box on the right with
//! the skull in it, and its description rectangle under the picture. Our
//! screens are Controls screens, so the list comes from a copy of the
//! Controls tag with that list block ([`LIST_LAYOUT`],
//! `list_class::ListLayout`); the picture and text are widgets of ours.
//!
//! **The multiplayer screen** is a second instance
//! ([`SkullsScreen::multiplayer`]) on a class of its own (`skulls-mp-screen`),
//! opened from the variant editor's Skulls row (`crate::skulls_mp_row`):
//! only the skulls that work in multiplayer, each the profile's multiplayer
//! On or Off, or Locked until the profile has collected it.
//!
//! **The picture.** Both screens show the difficulty screen's Legendary
//! skull ([`PICTURE`], the game's own bitmap) right of the list, tinted
//! gold or silver for the focused skull's tier and dimmed while it is
//! locked ([`tint`]), with the description under it.

use std::ops::Range;
use std::sync::{Arc, LazyLock, Mutex};

use settings::Store;

use crate::h1_options::OptionRow;
use crate::layout_glue::Gm;
#[cfg(test)]
use crate::layout_glue::ID_BASE;
use crate::list_class::{ListClass, ListLayout, PictureSpec, Rows, ScrollArrows, TextBlock};
use crate::skulls::{self, ABOUT, COUNT, SKULLS, State, TOTAL};
use crate::ui::{Chosen, LABEL_SLOT};

/// The difficulty list's array name (`0x45d6e0`, "difficulty list").
pub const DIFFICULTY_LIST_NAME: u32 = 0x0045_d6e0;
/// The difficulty screen's id (`screen + 0x70`).
pub const DIFFICULTY_SCREEN_ID: i16 = 12;
/// The difficulty list's row chosen (`ret 8`).
pub const DIFFICULTY_ROW_CHOSEN: u32 = 0x002c_9d38;
/// The screen's list class (`features.skulls-screen` switches it).
pub const FEATURE: &str = "skulls-screen";

/// Our strings: a label per skull, the title (also the row's label), our
/// "Locked" and a description per skull. `layout_glue::is_ours` looks at
/// the middle bytes only, so a base one higher in the top byte than
/// [`ID_BASE`] is ours too and clear of every id the model and the other
/// rows take (`ID_BASE + 0x00..0xff`).
pub const SKULLS_ID_BASE: u32 = crate::ui_kit::ids::family("skulls").base;
pub const TITLE_ID: u32 = SKULLS_ID_BASE + 0x10;
pub const LOCKED_ID: u32 = SKULLS_ID_BASE + 0x11;
/// The multiplayer screen's title.
pub const MP_TITLE_ID: u32 = SKULLS_ID_BASE + 0x12;
/// One description per multiplayer row (`skulls_mp::MP`).
const MP_DESCRIPTIONS: Range<u32> = SKULLS_ID_BASE + 0x30..SKULLS_ID_BASE + 0x38;
/// A label and a description per skull, ours included (room for 64).
const LABELS: Range<u32> = SKULLS_ID_BASE + 0x40..SKULLS_ID_BASE + 0x80;
const DESCRIPTIONS: Range<u32> = SKULLS_ID_BASE + 0x80..SKULLS_ID_BASE + 0xc0;
/// The HUD message when one of our skulls is picked up
/// (`crate::skulls_custom`), by skull index.
const FOUND: Range<u32> = SKULLS_ID_BASE + 0xc0..SKULLS_ID_BASE + 0x100;
/// Each row's label, one per skull and state: the list skin has one text
/// per row, so the state goes in the label ("Mythic: On"). A base of its
/// own (another top byte; `is_ours` reads the middle bytes), three ids per
/// skull, then the campaign page's stepped forms ([`ARROW_LABELS`]). The
/// top byte is clear of the campaign picker's (`campaign_menu::ID_BASE`,
/// `ID_BASE + 0x0200_0000`).
const ROW_LABEL_BASE: u32 = crate::ui_kit::ids::family("skull row labels").base;
/// The campaign page's On and Off labels, two per skull: "Mythic: < On >",
/// a value left and right step.
const ARROW_LABELS: u32 = ROW_LABEL_BASE + 0x80;
pub const TITLE: &str = "Skulls";
/// The campaign page's title: the level's options and its skulls.
pub const PAGE_TITLE: &str = "Game Options";
pub const PAGE_TITLE_ID: u32 = SKULLS_ID_BASE + 0x14;
/// The campaign page's first row, and what it says.
pub const START: &str = "Start Level";
pub const START_ID: u32 = SKULLS_ID_BASE + 0x15;
pub const START_DESCRIPTION: &str =
    "Start the level with the options and skulls below. Change one with the D-pad or A.";
pub const START_DESCRIPTION_ID: u32 = SKULLS_ID_BASE + 0x16;
pub const MP_TITLE: &str = "Multiplayer Skulls";
pub const LOCKED: &str = "Locked";
/// The campaign page's buttons, in the footer band where the game's own
/// screens put theirs (`list_class::FOOTER`; "(A) SELECT  (B) BACK" is
/// retail's `a_select_b_back`): the font's button glyphs (U+E10A and
/// U+E10B the d-pad's left and right, U+E100 A, U+E101 B; each icon set
/// draws its own), the words in capitals as retail's. Left or right
/// changes the focused row; A selects it, as on the game's own lists: on
/// the Start Level row (the first) it starts the level
/// (`skulls_screen::action`), on any other row it changes it. X and Start
/// do nothing here (X going on to the level read as a mystery). Under the
/// descriptions the legend ran 10 of the 21 past the description's five
/// lines (`ui_kit` preview, 2026-11-17).
pub const PROMPT: &str = "\u{e10a}  \u{e10b} CHANGE  \u{e100} SELECT  \u{e101} BACK";
/// The footer legend's id.
pub const FOOTER_ID: u32 = SKULLS_ID_BASE + 0x13;

/// The reference layout: the campaign level select
/// (`ui\screens\game_shell\load_level\load_level`, screen 11, mainmenu.map,
/// read with `h2 ui`): list anchor (-215, 130) on skin 0
/// (`ui\list_skins\default\default`, rows 200 wide, one text), picture box
/// `[177 39 -30 259]` (top, left, bottom, right; Y up, screen centre
/// origin), description text `[-30 39 -133 260]`, font 1. The Controls
/// frame art right of the centre (`br_06`, `[-30 150 -286 406]`) is left out:
/// it sits where the description goes. `H2_SKULLS_LIST_AT=x,y` and
/// `H2_SKULLS_LIST_SKIN=n` change the list without a rebuild.
pub const LIST_LAYOUT: ListLayout = ListLayout {
    skin: 0,
    anchor: [-215, 130],
    bitmaps_left_of: i16::MIN,
    visible: VISIBLE_ROWS,
};

/// The level select's own screen id.
pub const LEVEL_SELECT_SCREEN_ID: i16 = 11;

/// The list's frame: the level select's left bracket (`ul_02`, its pane 0
/// bitmap 1, `[177 -259 -79 -131]`, 128 by 256), at its own place, made
/// for this list at this anchor. The Controls tag's bracket (`ul_03`) was
/// made for the Controls list, 39 units further left and on another skin:
/// kept under this list it framed an empty box and the first row's end cap
/// cut its arm off (docs/game/ui-builder.md, slice 3), so none of the
/// Controls frame art is kept ([`LIST_LAYOUT`]`.bitmaps_left_of`).
pub const FRAME: PictureSpec = PictureSpec {
    screen_id: LEVEL_SELECT_SCREEN_ID as u32,
    pane: 0,
    bitmap: 1,
    image: 0,
    at: [-259, 177],
    animation: FRAME_ANIMATION,
};
/// [`FRAME`]'s animation: the bracket's own in the level select (read off
/// the disc), so it slides with the page as there.
pub const FRAME_ANIMATION: u16 = 2;
/// [`FRAME`]'s image size.
pub const FRAME_SIZE: [u16; 2] = [128, 256];

/// Rows the page's list lays out before it scrolls: from the anchor down to
/// the footer band, one skin-0 item (22 units) each. The Controls tag's six
/// stopped the list a third of the way down the page (the skulls page's
/// "doesn't use the full height"); the level select's own list block says
/// 16, which runs into the footer at our anchor. 13 rows end at y -156,
/// clear of the footer band (the Controls tag's header layout, `wigl`
/// header layout 0, puts its second rectangle at top -187; a 14th row would
/// end at -178, against it).
pub const VISIBLE_ROWS: i16 = 13;
/// A skin-0 item's height (docs/game/ui-screens.md "Item spacing").
pub const ITEM_HEIGHT: i16 = 22;
/// Level select's skin 0 uses the same arrow art as this page, but its
/// 15 retail levels fit without scrolling. Its arrow offsets (-30, 24)
/// and (-30, -44) put our up arrow over the bracket and our down arrow
/// beside the upper rows. Use the gutter before the picture column and
/// place the arrows above the first and below the last visible row.
pub(crate) const SCROLL_ARROWS: ScrollArrows = ScrollArrows {
    up: [224, 46],
    down: [224, -(VISIBLE_ROWS - 1) * ITEM_HEIGHT - 8],
};

/// The level select's frame and list, with room for the active arrows on
/// lists long enough to scroll. Skin overrides keep their own arrows.
pub(crate) fn level_select_list(class: ListClass, layout: ListLayout) -> ListClass {
    let class = class.layout(layout).frame(FRAME);
    if layout.skin == LIST_LAYOUT.skin {
        class.scroll_arrows(SCROLL_ARROWS)
    } else {
        class
    }
}

/// The level select's picture box: `top, left, bottom, right`.
pub const PICTURE_BOX: [i16; 4] = [177, 39, -30, 259];

/// The description text: the level select's (rectangle, font and colour,
/// a pale blue: 99, 148, 193), under its picture.
/// `H2_SKULLS_TEXT_RECT=top,left,bottom,right` moves it without a rebuild.
pub const DESCRIPTION_TEXT: TextBlock = TextBlock {
    rect: [-30, 39, -133, 260],
    font: 1,
    colour: [0.388235, 0.580392, 0.756863],
    animation: DESCRIPTION_ANIMATION,
};

/// The level select's description and picture animations, as its tag
/// stores them (1-based, `tags::ui::AnimationRef`; its text block's and its
/// picture bitmap's, read off the disc): our two widgets slide in and out
/// with the page as the level select's do. With none (0), B left the skull
/// drawn over the difficulty screen's own while the page animated out.
pub const DESCRIPTION_ANIMATION: u16 = 11;
pub const PICTURE_ANIMATION: u16 = 13;

/// The visible part of the skull art inside its 256 by 256 image:
/// `left, top, right, bottom` in image pixels from its top-left (measured
/// from the image's alpha, read off the disc; no art is kept here).
pub const SKULL_ART: [i16; 4] = [17, 3, 209, 208];

/// The picture: the difficulty screen's (12) Legendary pane (3), its first
/// bitmap, `difficulty_options` image 3 (a 256 by 256 skull, drawn at its
/// size: a bitmap widget has no scale), placed so its visible part sits
/// centred in the level select's picture box with its top on the box's
/// top: top-left (36, 180). `H2_SKULLS_PICTURE_AT=x,y` moves it.
pub const PICTURE: PictureSpec = PictureSpec {
    screen_id: DIFFICULTY_SCREEN_ID as u32,
    pane: 3,
    bitmap: 0,
    image: 3,
    at: picture_at(PICTURE_BOX, SKULL_ART),
    animation: PICTURE_ANIMATION,
};

/// Where the skull image's top-left goes so its visible part `art` is
/// centred across `bx` and starts at its top.
pub const fn picture_at(bx: [i16; 4], art: [i16; 4]) -> [i16; 2] {
    let (box_w, art_w) = (bx[3] - bx[1], art[2] - art[0]);
    [bx[1] + (box_w - art_w) / 2 - art[0], bx[0] + art[1]]
}

/// The picture's tint for a skull of tier `tier`: gold or silver, and
/// dimmed while `state` is Locked (to 60%: a third read as a faint
/// watermark in game). The art is a pale silver, so silver keeps it nearly
/// as drawn.
pub fn tint(tier: skulls::SkullTier, state: State) -> [f32; 3] {
    let c = match tier {
        skulls::SkullTier::Gold => [1.0, 0.78, 0.3],
        skulls::SkullTier::Silver => [0.86, 0.9, 1.0],
    };
    if state == State::Locked {
        c.map(|v| v * 0.6)
    } else {
        c
    }
}

/// The state's index in a skull's three row labels, and its word.
fn state_index(state: State) -> u32 {
    match state {
        State::On => 0,
        State::Off => 1,
        State::Locked => 2,
    }
}
const STATE_WORDS: [&str; 3] = ["On", "Off", LOCKED];

/// The label id of skull `i`'s row in `state`.
pub fn row_label_id(i: usize, state: State) -> u32 {
    ROW_LABEL_BASE + 3 * i as u32 + state_index(state)
}

/// The campaign page's label id of skull `i`'s row in `state`: On and Off
/// as a stepped value ("Mythic: < On >"), Locked as on every screen.
pub fn arrow_label_id(i: usize, state: State) -> u32 {
    match state {
        State::Locked => row_label_id(i, state),
        State::On | State::Off => ARROW_LABELS + 2 * i as u32 + state_index(state),
    }
}

static ROW_LABELS: LazyLock<Vec<(u32, String)>> = LazyLock::new(|| {
    let word = |st: State| STATE_WORDS[state_index(st) as usize];
    (0..TOTAL)
        .flat_map(|i| {
            let plain = [State::On, State::Off, State::Locked].map(|st| {
                (
                    row_label_id(i, st),
                    format!("{}: {}", SKULLS[i].label, word(st)),
                )
            });
            let stepped = [State::On, State::Off].map(|st| {
                (
                    arrow_label_id(i, st),
                    format!("{}: < {} >", SKULLS[i].label, word(st)),
                )
            });
            plain.into_iter().chain(stepped)
        })
        .collect()
});

/// The label id of skull `i`.
pub fn label_id(i: usize) -> u32 {
    LABELS.start + i as u32
}

/// The description id of skull `i`.
pub fn description_id(i: usize) -> u32 {
    DESCRIPTIONS.start + i as u32
}

/// The id of the HUD message shown when skull `i` (one of ours) is found.
pub fn found_id(i: usize) -> u32 {
    FOUND.start + i as u32
}

/// The HUD message when skull `i` is found: our own words (retail shows
/// the skull's name alone).
pub fn found_text(i: usize) -> String {
    format!("Skull found: {}", SKULLS[i].label)
}

/// Skull `i`'s description: the game's full name when the row shows a
/// short one, what it does, where it is and whether only on Legendary.
pub fn description(i: usize) -> String {
    let (d, a) = (&SKULLS[i], &ABOUT[i]);
    let name = if d.label == a.name {
        String::new()
    } else {
        format!("{}: ", a.name)
    };
    let needs = if a.legendary {
        "Legendary only"
    } else {
        "any difficulty"
    };
    let note = a.note.map_or(String::new(), |n| format!(" ({n})"));
    format!(
        "{} skull: {name}{} Found on {}, {needs}{note}.",
        a.tier.name(),
        a.effect,
        a.level
    )
}

/// The campaign screen's rows: skull indices, gold first.
static ORDER: LazyLock<Vec<usize>> = LazyLock::new(skulls::by_tier);

/// The campaign page's row for skull `i`.
pub fn row_of(i: usize) -> Option<i16> {
    ORDER.iter().position(|s| *s == i).map(|r| r as i16)
}

static DESCRIPTION_TEXTS: LazyLock<Vec<String>> =
    LazyLock::new(|| (0..TOTAL).map(description).collect());

static FOUND_TEXTS: LazyLock<Vec<(usize, String)>> =
    LazyLock::new(|| (COUNT..TOTAL).map(|i| (i, found_text(i))).collect());

/// The description id of multiplayer row `row`.
pub fn mp_description_id(row: usize) -> u32 {
    MP_DESCRIPTIONS.start + row as u32
}

/// Multiplayer row `row`'s description: what it does in a multiplayer game,
/// that the host's choice is everyone's, and where it is collected.
pub fn mp_description(row: usize) -> String {
    let m = &crate::skulls_mp::MP[row];
    let a = &ABOUT[m.index];
    format!(
        "{} The host's choice is everyone's. Found on {}{}.",
        m.effect,
        a.level,
        if a.legendary { ", Legendary only" } else { "" }
    )
}

static MP_DESCRIPTION_TEXTS: LazyLock<Vec<String>> = LazyLock::new(|| {
    (0..crate::skulls_mp::MP.len())
        .map(mp_description)
        .collect()
});

/// Every string we serve.
pub fn strings() -> Vec<(u32, &'static str)> {
    let mut v: Vec<(u32, &'static str)> = SKULLS
        .iter()
        .enumerate()
        .map(|(i, d)| (label_id(i), d.label))
        .collect();
    v.push((TITLE_ID, TITLE));
    v.push((PAGE_TITLE_ID, PAGE_TITLE));
    v.push((START_ID, START));
    v.push((START_DESCRIPTION_ID, START_DESCRIPTION));
    v.push((LOCKED_ID, LOCKED));
    v.push((MP_TITLE_ID, MP_TITLE));
    v.push((FOOTER_ID, PROMPT));
    v.extend(ROW_LABELS.iter().map(|(i, t)| (*i, t.as_str())));
    v.extend(
        DESCRIPTION_TEXTS
            .iter()
            .enumerate()
            .map(|(i, t)| (description_id(i), t.as_str())),
    );
    v.extend(
        MP_DESCRIPTION_TEXTS
            .iter()
            .enumerate()
            .map(|(i, t)| (mp_description_id(i), t.as_str())),
    );
    v.extend(FOUND_TEXTS.iter().map(|(i, t)| (found_id(*i), t.as_str())));
    v
}

/// The description block, moved by `H2_SKULLS_TEXT_RECT` (`env`) when it
/// holds four numbers; the line to log when it is set.
pub fn description_block(env: Option<&str>) -> (TextBlock, Option<String>) {
    let Some(v) = env else {
        return (DESCRIPTION_TEXT, None);
    };
    let n: Vec<i16> = v.split(',').filter_map(|s| s.trim().parse().ok()).collect();
    match <[i16; 4]>::try_from(n) {
        Ok(rect) => (
            TextBlock {
                rect,
                ..DESCRIPTION_TEXT
            },
            Some(format!(
                "skulls menu: description text at {rect:?} (H2_SKULLS_TEXT_RECT)"
            )),
        ),
        Err(_) => (
            DESCRIPTION_TEXT,
            Some(format!(
                "skulls menu: H2_SKULLS_TEXT_RECT={v:?} is not top,left,bottom,right; description text at {:?}",
                DESCRIPTION_TEXT.rect
            )),
        ),
    }
}

/// `n` comma-separated numbers, or `None`.
fn numbers<const N: usize>(v: &str) -> Option<[i16; N]> {
    let n: Vec<i16> = v.split(',').filter_map(|s| s.trim().parse().ok()).collect();
    <[i16; N]>::try_from(n).ok()
}

/// The list layout and picture, moved by `H2_SKULLS_LIST_AT=x,y`,
/// `H2_SKULLS_LIST_SKIN=n` and `H2_SKULLS_PICTURE_AT=x,y` (read by `env`);
/// the lines to log for those set.
pub fn placement(env: impl Fn(&str) -> Option<String>) -> (ListLayout, PictureSpec, Vec<String>) {
    let (mut layout, mut picture, mut lines) = (LIST_LAYOUT, PICTURE, Vec::new());
    let mut take = |key: &str, ok: &mut dyn FnMut(&str) -> bool| {
        if let Some(v) = env(key) {
            lines.push(if ok(&v) {
                format!("skulls menu: {key}={v}")
            } else {
                format!("skulls menu: {key}={v:?} not understood; the default stays")
            });
        }
    };
    take("H2_SKULLS_LIST_AT", &mut |v| {
        numbers::<2>(v).map(|a| layout.anchor = a).is_some()
    });
    take("H2_SKULLS_LIST_SKIN", &mut |v| {
        numbers::<1>(v).map(|[n]| layout.skin = n).is_some()
    });
    take("H2_SKULLS_PICTURE_AT", &mut |v| {
        numbers::<2>(v).map(|a| picture.at = a).is_some()
    });
    (layout, picture, lines)
}

/// Whether the menu is on: for a store that has the skull settings.
pub fn enabled(store: &Store) -> bool {
    store.def(SKULLS[0].key).is_some()
}

/// The local player index of a list callback (`[[esp+4]]+4`), as
/// `video_screen::player_mask` reads it.
pub fn player_index<M: Gm>(m: &M, esp: u32) -> Option<u32> {
    let p = m.u32(esp.wrapping_add(4))?;
    let record = m.u32(p)?;
    let index = m.u32(record.wrapping_add(4))?;
    (index < skulls::LOCAL_PLAYERS).then_some(index)
}

/// The profile whose skulls a press by local player `index` shows: the
/// one signed in on that player's slot, else [`skulls::DEFAULT_PROFILE`]
/// (where a pickup with no named profile is kept).
pub fn profile_of<M: Gm>(m: &M, index: u32) -> Option<String> {
    (index < skulls::LOCAL_PLAYERS).then(|| skulls::profile_or_default(m, index))
}

/// A press on skull `i` of `profile`'s screen: a collected skull flips; a
/// locked one stays locked. Returns what to log.
pub fn press(store: &Store, profile: &str, i: usize) -> Option<String> {
    let (def, about) = (SKULLS.get(i)?, &ABOUT[i]);
    Some(match skulls::state(store, profile, i)? {
        State::Locked => format!(
            "skulls menu: {} is locked for {profile}: not collected yet (found on {}{})",
            def.label,
            about.level,
            if about.legendary {
                ", Legendary only"
            } else {
                ""
            }
        ),
        State::On | State::Off => skulls::toggle(store, profile, def.key).unwrap_or_else(|e| e),
    })
}

/// Which skulls the screen lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Every skull, for the campaign about to start
    /// (the page after the difficulty).
    Campaign,
    /// Only those that work in multiplayer (`skulls_mp::MP`), for the games
    /// the profile hosts (the multiplayer instance, from the variant
    /// editor's row).
    Multiplayer,
}

/// The Skulls screen: every skull, for the profile it was last opened for;
/// or, the multiplayer instance, the multiplayer ones.
pub struct SkullsScreen {
    store: Arc<Store>,
    open: Mutex<Option<(String, Mode)>>,
    /// The campaign page's options above the skulls: Halo 1's for a Halo 1
    /// level (`h1_options::CAMPAIGN`), none for Halo 2's.
    options: Mutex<&'static [&'static OptionRow]>,
    /// What this instance lists, whoever opens it.
    fixed: Mode,
}

/// What a campaign page row is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageRow {
    /// The first: A starts the level.
    Start,
    /// A Halo 1 option.
    Option(&'static OptionRow),
    /// A skull, by index.
    Skull(usize),
}

impl SkullsScreen {
    /// The campaign screen.
    pub fn new(store: Arc<Store>) -> Self {
        SkullsScreen {
            store,
            open: Mutex::new(None),
            options: Mutex::new(&[]),
            fixed: Mode::Campaign,
        }
    }
    /// The multiplayer screen: a screen of its own, never the campaign one.
    pub fn multiplayer(store: Arc<Store>) -> Self {
        SkullsScreen {
            fixed: Mode::Multiplayer,
            ..Self::new(store)
        }
    }
    /// The screen shows `profile`'s skulls from now (the pressing player's;
    /// one screen of each instance is open at a time).
    pub fn open(&self, profile: String) {
        self.open_with(profile, &[]);
    }
    /// The campaign page from now: `profile`'s skulls under `options`.
    pub fn open_with(&self, profile: String, options: &'static [&'static OptionRow]) {
        *self.open.lock().unwrap_or_else(|e| e.into_inner()) = Some((profile, self.fixed));
        *self.options.lock().unwrap_or_else(|e| e.into_inner()) = options;
    }
    /// The options the page shows above the skulls.
    pub fn options(&self) -> &'static [&'static OptionRow] {
        *self.options.lock().unwrap_or_else(|e| e.into_inner())
    }
    /// Campaign row `row`: Start, an option, a skull (gold first).
    pub fn page_row(&self, row: i16) -> Option<PageRow> {
        let r = usize::try_from(row).ok()?;
        let options = self.options();
        if r == 0 {
            return Some(PageRow::Start);
        }
        if let Some(o) = options.get(r - 1) {
            return Some(PageRow::Option(o));
        }
        ORDER.get(r - 1 - options.len()).map(|i| PageRow::Skull(*i))
    }
    /// The campaign page's row of skull `i`.
    pub fn row_of_skull(&self, i: usize) -> Option<i16> {
        row_of(i).map(|r| r + 1 + self.options().len() as i16)
    }
    pub fn profile(&self) -> Option<String> {
        self.open
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|(p, _)| p.clone())
    }
    /// The store the screen reads and writes.
    pub fn store(&self) -> Arc<Store> {
        Arc::clone(&self.store)
    }
    pub fn mode(&self) -> Mode {
        self.fixed
    }
    /// Multiplayer row `row`'s state for the open profile; Locked with none.
    pub fn mp_state(&self, row: usize) -> State {
        self.profile()
            .and_then(|p| crate::skulls_mp::state(&self.store, &p, row))
            .unwrap_or(State::Locked)
    }
    /// Campaign row `row`'s skull: gold ones first,
    /// then silver ([`skulls::by_tier`]).
    fn skull(&self, row: i16) -> Option<usize> {
        match self.page_row(row)? {
            PageRow::Skull(i) => Some(i),
            _ => None,
        }
    }
    /// Skull `i`'s state for the open profile; Locked with none.
    pub fn state(&self, i: usize) -> State {
        self.profile()
            .and_then(|p| skulls::state(&self.store, &p, i))
            .unwrap_or(State::Locked)
    }
    /// Left or right on campaign row `row`: either flips a collected skull
    /// (two values, so both directions step to the other, as a value row
    /// with arrows on both sides wraps). What to log, `None` for no such
    /// row or the multiplayer screen (whose A toggles).
    pub fn step_line(&self, row: i16, right: bool) -> Option<String> {
        if self.mode() == Mode::Multiplayer {
            return None;
        }
        let line = self.press_line(row)?;
        Some(format!(
            "skulls page: {} on row {row}: {line}",
            if right { "right" } else { "left" }
        ))
    }
    /// A toggle of row `row` (A on the multiplayer screen, left, right or
    /// A on the campaign page): what to log, `None` for no such row or the
    /// Start row.
    pub fn press_line(&self, row: i16) -> Option<String> {
        if self.mode() == Mode::Multiplayer {
            let r = usize::try_from(row)
                .ok()
                .filter(|r| *r < crate::skulls_mp::MP.len())?;
            let profile = self.profile()?;
            let line = crate::skulls_mp::press(&self.store, &profile, r);
            // An open lobby's advertisement follows the change.
            crate::skulls_mp::changed(&profile);
            return line;
        }
        if let PageRow::Option(o) = self.page_row(row)? {
            return Some(crate::h1_options::step(&self.store, o));
        }
        let i = self.skull(row)?;
        let Some(profile) = self.profile() else {
            return Some("skulls menu: no profile for this screen; nothing changed".into());
        };
        press(&self.store, &profile, i)
    }
}

impl Rows for SkullsScreen {
    fn count(&self) -> usize {
        match self.mode() {
            Mode::Campaign => 1 + self.options().len() + TOTAL,
            Mode::Multiplayer => crate::skulls_mp::MP.len(),
        }
    }
    fn title(&self) -> u32 {
        match self.mode() {
            Mode::Campaign => PAGE_TITLE_ID,
            Mode::Multiplayer => MP_TITLE_ID,
        }
    }
    fn texts(&self, row: i16) -> Vec<(u32, u32)> {
        if self.mode() == Mode::Multiplayer {
            let Some(r) = usize::try_from(row)
                .ok()
                .filter(|r| *r < crate::skulls_mp::MP.len())
            else {
                return Vec::new();
            };
            return vec![(
                LABEL_SLOT,
                row_label_id(crate::skulls_mp::MP[r].index, self.mp_state(r)),
            )];
        }
        match self.page_row(row) {
            Some(PageRow::Start) => vec![(LABEL_SLOT, START_ID)],
            Some(PageRow::Option(o)) => vec![(
                LABEL_SLOT,
                crate::h1_options::label_id(o, crate::h1_options::is_halo1(&self.store, o)),
            )],
            Some(PageRow::Skull(i)) => vec![(LABEL_SLOT, arrow_label_id(i, self.state(i)))],
            None => Vec::new(),
        }
    }
    /// A on the campaign page's Start row goes on to the level (the
    /// class's action); on an option or a skull it changes it, as left
    /// and right do. On the multiplayer screen it toggles.
    fn press(&self, row: i16) -> Chosen {
        match self.mode() {
            Mode::Campaign => match self.page_row(row) {
                Some(PageRow::Start) => Chosen::Act,
                Some(_) => self
                    .press_line(row)
                    .map_or(Chosen::Nothing, Chosen::Stepped),
                None => Chosen::Nothing,
            },
            Mode::Multiplayer => self
                .press_line(row)
                .map_or(Chosen::Nothing, Chosen::Stepped),
        }
    }
    fn sideways(&self, row: i16, right: bool) -> Option<String> {
        self.step_line(row, right)
    }
    fn description(&self, row: i16) -> Option<u32> {
        if self.mode() == Mode::Multiplayer {
            return usize::try_from(row)
                .ok()
                .filter(|r| *r < crate::skulls_mp::MP.len())
                .map(mp_description_id);
        }
        match self.page_row(row)? {
            PageRow::Start => Some(START_DESCRIPTION_ID),
            PageRow::Option(o) => Some(crate::h1_options::description_id(o)),
            PageRow::Skull(i) => Some(description_id(i)),
        }
    }
    fn tint(&self, row: i16) -> Option<[f32; 3]> {
        let r = usize::try_from(row).ok()?;
        let (index, state) = match self.mode() {
            Mode::Multiplayer => (crate::skulls_mp::MP.get(r)?.index, self.mp_state(r)),
            Mode::Campaign => {
                let i = self.skull(row)?;
                (i, self.state(i))
            }
        };
        Some(tint(ABOUT.get(index)?.tier, state))
    }
}

const _: () = assert!(TOTAL as u32 <= LABELS.end - LABELS.start);
const _: () = assert!(TOTAL as u32 <= DESCRIPTIONS.end - DESCRIPTIONS.start);
const _: () = assert!(TOTAL as u32 <= FOUND.end - FOUND.start);
const _: () = assert!(3 * TOTAL as u32 <= ARROW_LABELS - ROW_LABEL_BASE);
const _: () = assert!(ARROW_LABELS - ROW_LABEL_BASE + 2 * TOTAL as u32 <= 0x100);
const _: () =
    assert!(crate::skulls_mp::MP.len() as u32 <= MP_DESCRIPTIONS.end - MP_DESCRIPTIONS.start);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout_glue::is_ours;
    use crate::list_class::Rows;
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct Fake(BTreeMap<u32, u8>);
    impl Gm for Fake {
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

    fn store() -> Arc<Store> {
        let defs: Vec<&'static settings::Def> = SKULLS.iter().collect();
        Arc::new(Store::defaults(&defs))
    }

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn text(id: u32) -> String {
        strings()
            .iter()
            .find(|(i, _)| *i == id)
            .map_or_else(|| format!("{id:#x}"), |(_, t)| t.to_string())
    }

    /// Each skull row as drawn, in order (the campaign page's Start Level
    /// and option rows left out): its one label, split into the name and
    /// the state.
    fn drawn(s: &SkullsScreen) -> Vec<(String, String)> {
        (0..s.count() as i16)
            .filter(|r| {
                s.mode() == Mode::Multiplayer || matches!(s.page_row(*r), Some(PageRow::Skull(_)))
            })
            .map(|r| {
                let t = s.texts(r);
                assert_eq!(t.len(), 1, "one text: the list skin has one");
                assert_eq!(t[0].0, LABEL_SLOT);
                let label = text(t[0].1);
                let (name, state) = label.rsplit_once(": ").expect("Name: State");
                // The campaign page's On and Off read as a stepped value.
                let state = match s.mode() {
                    Mode::Campaign if state != LOCKED => state
                        .strip_prefix("< ")
                        .and_then(|v| v.strip_suffix(" >"))
                        .expect("< On > or < Off >"),
                    _ => state,
                };
                (name.to_string(), state.to_string())
            })
            .collect()
    }

    #[test]
    fn every_skull_is_listed_gold_first() {
        let s = SkullsScreen::new(store());
        s.open("Chief".into());
        assert_eq!(s.count(), 1 + TOTAL, "Start Level, then every skull");
        assert_eq!(s.texts(0), [(LABEL_SLOT, START_ID)]);
        assert_eq!(text(START_ID), "Start Level");
        let names: Vec<_> = drawn(&s).into_iter().map(|(n, _)| n).collect();
        assert_eq!(
            names,
            [
                // Gold.
                "Assassins",
                "Thunderstorm",
                "Famine",
                "Blind",
                "Black Eye",
                "Catch",
                "Iron",
                "Mythic",
                "Angry",
                // Silver.
                "Envy",
                "Grunt Birthday Party",
                "IWHBYD",
                "Ghost",
                "Sputnik",
                "Bandana",
                // Ours, after retail's: gold, then silver.
                "Fall Damage",
                "Acrophobia",
                "Low Gravity",
                "Unlimited Ammo",
                "Unlimited Grenades",
                "Invincible",
            ]
        );
        assert!(s.texts(s.count() as i16).is_empty());
        assert!(s.texts(-1).is_empty());
        assert_eq!(s.title(), PAGE_TITLE_ID);
        assert_eq!(text(PAGE_TITLE_ID), "Game Options");
    }

    #[test]
    fn unlocked_comes_from_the_profiles_settings() {
        let st = store();
        let s = SkullsScreen::new(Arc::clone(&st));
        // No profile yet: everything locked.
        assert!(drawn(&s).iter().all(|(_, v)| v == "Locked"));
        skulls::record_pickup(&st, &names(&["Chief"]), 12);
        skulls::record_pickup(&st, &names(&["Chief"]), 1);
        skulls::toggle(&st, "Chief", "skulls.grunt_birthday_party").unwrap();
        skulls::record_pickup(&st, &names(&["Arbiter"]), 3);
        s.open("Chief".into());
        let values: Vec<_> = drawn(&s).into_iter().map(|(_, v)| v).collect();
        for (r, v) in values.iter().enumerate() {
            let i = ORDER[r];
            let want = match i {
                1 => "Off",
                12 => "On",
                _ => "Locked",
            };
            assert_eq!(*v, want, "skull {i}");
        }
        // The other co-op player's screen is theirs.
        s.open("Arbiter".into());
        assert_eq!(s.state(3), State::On);
        assert_eq!(s.state(12), State::Locked);
    }

    #[test]
    fn locked_skulls_do_not_toggle_and_collected_ones_do() {
        let st = store();
        let s = SkullsScreen::new(Arc::clone(&st));
        assert_eq!(
            s.press_line(s.row_of_skull(0).unwrap()).as_deref(),
            Some("skulls menu: no profile for this screen; nothing changed")
        );
        s.open("Chief".into());
        assert_eq!(
            s.press_line(s.row_of_skull(11).unwrap()).as_deref(),
            Some(
                "skulls menu: Iron is locked for Chief: not collected yet (found on High Charity, Legendary only)"
            )
        );
        assert_eq!(
            s.press_line(s.row_of_skull(6).unwrap()).as_deref(),
            Some("skulls menu: Blind is locked for Chief: not collected yet (found on Outskirts)")
        );
        assert_eq!(s.state(11), State::Locked, "a locked press changes nothing");
        assert_eq!(skulls::collected(&st, "Chief"), []);
        skulls::record_pickup(&st, &names(&["Chief"]), 11);
        assert_eq!(
            s.press_line(s.row_of_skull(11).unwrap()).as_deref(),
            Some("skulls: Iron off for Chief")
        );
        assert_eq!(s.state(11), State::Off);
        assert_eq!(
            s.press_line(s.row_of_skull(11).unwrap()).as_deref(),
            Some("skulls: Iron on for Chief")
        );
        assert_eq!(s.state(11), State::On);
        assert_eq!(s.press_line(s.count() as i16), None);
        assert_eq!(s.press_line(-1), None);
    }

    /// What the screen leaves is what the campaign start turns on:
    /// `skulls::selection` over the signed-in profiles, written to the
    /// array by `skulls::apply`.
    #[test]
    fn the_selection_is_carried_to_the_campaign_start() {
        let st = store();
        let s = SkullsScreen::new(Arc::clone(&st));
        for i in [0, 3, 12] {
            skulls::record_pickup(&st, &names(&["Chief"]), i);
        }
        skulls::record_pickup(&st, &names(&["Arbiter"]), 7);
        s.open("Chief".into());
        s.press_line(s.row_of_skull(3).unwrap()); // Thunderstorm off
        s.press_line(s.row_of_skull(9).unwrap()); // Catch: locked, nothing
        let mut want = [false; COUNT];
        want[0] = true;
        want[12] = true;
        assert_eq!(skulls::selection(&st, &names(&["Chief"])), Some(want));
        // Co-op: the other player turns theirs off on their own screen; the
        // start takes the union of both.
        s.open("Arbiter".into());
        s.press_line(s.row_of_skull(7).unwrap());
        let both = names(&["Chief", "Arbiter"]);
        assert_eq!(skulls::selection(&st, &both), Some(want));
        s.press_line(s.row_of_skull(7).unwrap());
        want[7] = true;
        assert_eq!(skulls::selection(&st, &both), Some(want));
        // Toggling back restores it.
        s.open("Chief".into());
        s.press_line(s.row_of_skull(3).unwrap());
        want[3] = true;
        assert_eq!(skulls::selection(&st, &both), Some(want));
    }

    #[test]
    fn each_description_says_what_it_does_where_and_if_legendary() {
        let s = SkullsScreen::new(store());
        for (i, a) in ABOUT.iter().enumerate() {
            let id = s.description(s.row_of_skull(i).unwrap()).unwrap();
            assert_eq!(id, description_id(i));
            let shown = text(id);
            // The buttons are the footer's, not under the description;
            // within a widget's 256.
            assert!(!shown.contains('\u{e100}'), "{shown}");
            assert_eq!(text(FOOTER_ID), PROMPT);
            assert!(shown.encode_utf16().count() <= 256, "{shown}");
            let d = description(i);
            assert!(d.contains(a.effect), "{d}");
            assert!(d.contains(&format!("Found on {}", a.level)), "{d}");
            assert_eq!(d.contains("Legendary only"), a.legendary, "{d}");
            assert_eq!(d.contains("any difficulty"), !a.legendary, "{d}");
            assert!(d.starts_with(&format!("{} skull: ", a.tier.name())), "{d}");
            // A text widget holds 256 UTF-16 units, and the box is small.
            assert!(d.encode_utf16().count() <= 140, "{d}");
        }
        assert_eq!(
            description(6),
            "Gold skull: Your HUD and first-person weapon are hidden. Found on Outskirts, any difficulty."
        );
        assert_eq!(
            description(5),
            "Silver skull: I Would Have Been Your Daddy: Rare lines play far more often. \
             Found on Outskirts, Legendary only (solo; one try in seven)."
        );
        assert_eq!(
            description(crate::skulls_custom::FALL_DAMAGE),
            "Gold skull: A hard landing hurts: past a jump's speed, a fall drains your shields, then \
             your health. Found on Metropolis, any difficulty."
        );
        assert_eq!(s.description(s.count() as i16), None);
    }

    #[test]
    fn the_pressing_players_profile_is_the_one_shown() {
        let mut f = Fake::default();
        // [esp+4] -> p -> record; record+4 is the player index.
        f.set_u32(0x1004, 0x5000);
        f.set_u32(0x5000, 0x6000);
        f.set_u32(0x6004, 1);
        assert_eq!(player_index(&f, 0x1000), Some(1));
        let base = skulls::SLOTS + skulls::SLOT_STRIDE;
        f.set_u32(base, skulls::SLOT_SIGNED_IN);
        let mut name = vec![0u8; skulls::NAME_UNITS * 2];
        for (i, u) in "Arbiter".encode_utf16().enumerate() {
            name[i * 2..i * 2 + 2].copy_from_slice(&u.to_le_bytes());
        }
        f.wr(base + skulls::SLOT_NAME, &name);
        assert_eq!(profile_of(&f, 1).as_deref(), Some("Arbiter"));
        // A slot with no named profile shows the default profile's skulls,
        // where a pickup with no named profile was kept.
        assert_eq!(profile_of(&f, 0).as_deref(), Some(skulls::DEFAULT_PROFILE));
        assert_eq!(profile_of(&f, 4), None, "no such slot");
        f.set_u32(0x6004, 4);
        assert_eq!(player_index(&f, 0x1000), None);
    }

    /// The user's earlier report: a skull picked up with no named profile
    /// signed in. The pickup is kept under the default profile, and the
    /// pressing player (slot 0, nobody signed in) sees it unlocked.
    #[test]
    fn a_pickup_with_no_named_profile_shows_on_the_default_profiles_screen() {
        let st = Arc::new(Store::defaults(crate::setting_defs::ALL));
        let none = skulls::profiles_or_default(&Fake::default());
        assert!(
            none.1
                .as_deref()
                .is_some_and(|w| w.contains("no named profile signed in"))
        );
        skulls::record_pickup(&st, &none.0, 12);
        let s = SkullsScreen::new(Arc::clone(&st));
        s.open(profile_of(&Fake::default(), 0).unwrap());
        assert_eq!(s.state(12), State::On);
    }

    #[test]
    fn the_description_text_can_be_moved() {
        assert_eq!(description_block(None), (DESCRIPTION_TEXT, None));
        let (b, line) = description_block(Some("100, -200, 0, 200"));
        assert_eq!(b.rect, [100, -200, 0, 200]);
        assert_eq!(b.font, DESCRIPTION_TEXT.font);
        assert!(line.unwrap().contains("[100, -200, 0, 200]"));
        let (b, line) = description_block(Some("1,2,3"));
        assert_eq!(b, DESCRIPTION_TEXT);
        assert!(line.unwrap().contains("is not top,left,bottom,right"));
        let [top, left, bottom, right] = DESCRIPTION_TEXT.rect;
        assert!(top > bottom && right > left, "Y up: top above bottom");
        assert!(
            top <= 240 && bottom >= -240 && left >= -320 && right <= 320,
            "on screen"
        );
    }

    #[test]
    fn strings_are_ours_distinct_and_clear_of_the_models() {
        let all = strings();
        // Labels and descriptions for all, a found message for each of
        // ours, the titles (Skulls, Game Options, the multiplayer one),
        // Locked, the footer legend and Start Level with its description.
        assert_eq!(
            all.len(),
            2 * TOTAL + 7 + 5 * TOTAL + crate::skulls_mp::MP.len() + crate::skulls_custom::OURS
        );
        let mut ids: Vec<u32> = all.iter().map(|(i, _)| *i).collect();
        assert!(ids.iter().all(|i| is_ours(*i)));
        assert!(
            ids.iter()
                .all(|i| !crate::video_screen::is_model_id(*i))
        );
        assert!(
            ids.iter().all(|i| *i >= ID_BASE + 0x100),
            "clear of the other rows' ids"
        );
        let picker = crate::campaign_menu::ID_BASE
            ..=crate::campaign_menu::ID_BASE + crate::campaign_menu::MAX_ROWS as u32;
        assert!(
            ids.iter().all(|i| !picker.contains(i)),
            "clear of the campaign picker's ids"
        );
        let n = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), n);
        assert!(all.contains(&(label_id(12), "Mythic")));
        assert!(all.contains(&(TITLE_ID, "Skulls")));
        assert!(all.contains(&(LOCKED_ID, "Locked")));
        assert!(all.contains(&(MP_TITLE_ID, "Multiplayer Skulls")));
    }

    /// The multiplayer instance: only the skulls that work in multiplayer,
    /// each the profile's multiplayer On, Off or Locked, and a press flips
    /// the multiplayer choice, never the campaign one.
    #[test]
    fn the_multiplayer_mode_lists_only_multiplayer_skulls() {
        let st = Arc::new(Store::defaults(crate::setting_defs::ALL));
        let s = SkullsScreen::multiplayer(Arc::clone(&st));
        s.open("Chief".into());
        assert_eq!(s.mode(), Mode::Multiplayer);
        assert_eq!(s.count(), 3);
        assert_eq!(s.title(), MP_TITLE_ID);
        let shown: Vec<_> = drawn(&s).into_iter().map(|(n, _)| n).collect();
        assert_eq!(shown, ["Famine", "Blind", "Low Gravity"]);
        assert!(
            drawn(&s).iter().all(|(_, v)| v == "Locked"),
            "nothing collected"
        );
        assert!(s.texts(3).is_empty());
        assert_eq!(s.description(1), Some(mp_description_id(1)));
        assert_eq!(
            text(mp_description_id(1)),
            "Every player's HUD and first-person weapon are hidden. The host's choice is \
             everyone's. Found on Outskirts."
        );
        assert!(text(mp_description_id(0)).ends_with("Found on The Oracle, Legendary only."));
        skulls::record_pickup(&st, &names(&["Chief"]), 6);
        assert_eq!(s.mp_state(1), State::Off, "collected, off for multiplayer");
        assert_eq!(
            s.press_line(1).as_deref(),
            Some("skulls: Blind on in multiplayer for Chief")
        );
        assert_eq!(s.mp_state(1), State::On);
        assert!(
            s.press_line(0)
                .unwrap()
                .contains("Famine is locked for Chief in multiplayer")
        );
        assert_eq!(s.press_line(3), None);
        assert_eq!(crate::skulls_mp::chosen(&st, "Chief"), vec![6]);
        // Opening it again keeps it the multiplayer screen.
        s.open("Chief".into());
        assert_eq!(s.count(), crate::skulls_mp::MP.len());
        assert_eq!(s.title(), MP_TITLE_ID);
        // The campaign screen still lists all of them, its own states.
        let s = SkullsScreen::new(Arc::clone(&st));
        s.open("Chief".into());
        assert_eq!(s.count(), 1 + TOTAL, "Start Level and every skull");
        assert_eq!(
            s.state(6),
            State::On,
            "the pickup turned it on for the campaign"
        );
        assert_eq!(s.title(), PAGE_TITLE_ID);
    }

    /// The picture follows the focused skull: gold or silver by tier,
    /// dimmed while locked, on both screens.
    #[test]
    fn the_picture_is_gold_or_silver_and_dim_while_locked() {
        use skulls::SkullTier::{Gold, Silver};
        assert_eq!(tint(Gold, State::On), [1.0, 0.78, 0.3]);
        assert_eq!(tint(Gold, State::Off), tint(Gold, State::On));
        assert_eq!(tint(Silver, State::Off), [0.86, 0.9, 1.0]);
        let dim = tint(Gold, State::Locked);
        assert!(dim.iter().zip(tint(Gold, State::On)).all(|(d, c)| *d < c));
        let st = Arc::new(Store::defaults(crate::setting_defs::ALL));
        let s = SkullsScreen::new(Arc::clone(&st));
        s.open("Chief".into());
        // Start Level shows no skull; the first skull row is the first gold
        // skull, the last row a silver one.
        let first = s.row_of_skull(ORDER[0]).unwrap();
        let last = (s.count() - 1) as i16;
        assert_eq!(s.tint(0), None, "Start Level: no picture");
        assert_eq!(s.tint(first), Some(tint(Gold, State::Locked)));
        assert_eq!(s.tint(last), Some(tint(Silver, State::Locked)));
        skulls::record_pickup(&st, &names(&["Chief"]), ORDER[0]);
        assert_eq!(
            s.tint(first),
            Some(tint(Gold, State::On)),
            "collected and on"
        );
        assert_eq!(s.tint(s.count() as i16), None);
        let mp = SkullsScreen::multiplayer(Arc::clone(&st));
        mp.open("Chief".into());
        assert_eq!(
            mp.tint(1),
            Some(tint(Gold, State::Locked)),
            "Blind, not collected"
        );
        skulls::record_pickup(&st, &names(&["Chief"]), 6);
        assert_eq!(mp.tint(1), Some(tint(Gold, State::Off)));
        assert_eq!(mp.tint(crate::skulls_mp::MP.len() as i16), None);
    }

    /// The layout is the campaign level select's (screen 11, read with
    /// `h2 ui mainmenu 'ui\\screens\\game_shell\\load_level\\load_level'`):
    /// its list anchor and skin, its picture box with the skull's visible
    /// part inside it, its description rectangle under the picture.
    #[test]
    fn the_layout_is_the_level_selects() {
        assert_eq!(LIST_LAYOUT.anchor, [-215, 130]);
        assert_eq!(LIST_LAYOUT.skin, 0, "ui\\list_skins\\default\\default");
        assert_eq!(PICTURE_BOX, [177, 39, -30, 259]);
        assert_eq!(DESCRIPTION_TEXT.rect, [-30, 39, -133, 260]);
        assert_eq!(DESCRIPTION_TEXT.font, 1);
        assert_eq!((PICTURE.screen_id, PICTURE.pane, PICTURE.image), (12, 3, 3));
        // The skull's visible part, in screen units.
        let [x, y] = PICTURE.at;
        let [al, at, ar, ab] = SKULL_ART;
        let (left, top, right, bottom) = (x + al, y - at, x + ar, y - ab);
        let [bt, bl, bb, br] = PICTURE_BOX;
        assert_eq!(top, bt, "the skull's top on the box's top");
        assert!(left >= bl && right <= br && bottom >= bb, "inside the box");
        assert!(((left - bl) - (br - right)).abs() <= 1, "centred across it");
        assert_eq!(PICTURE.at, [36, 180]);
        // The description is under the skull and as wide as the box.
        let [tt, tl, tb, tr] = DESCRIPTION_TEXT.rect;
        assert!(tt <= bottom && tb < tt, "under the picture");
        assert_eq!((tl, tr), (bl, br + 1));
    }

    /// Pin the visible row art and label rectangles in screen units.
    #[test]
    fn the_campaign_list_and_its_labels_fit_between_the_bracket_and_picture() {
        use crate::ui_kit::screens::sample;
        use crate::ui_kit::view::{Geometry, Synthetic, view_of};

        let screen = sample("skulls").unwrap();
        let geometry = Geometry::fallback(screen.list);
        let view = view_of(&screen, &geometry, &Synthetic, "first");
        let rect = |id| view.widget(id).unwrap().rect;

        let first_cap = rect("row.0.art.1");
        let last_cap = rect("row.12.art.1");
        let first_bar = rect("row.0.art.0");
        let list_right = first_bar[3];
        assert_eq!(
            [first_cap[0], first_cap[1], last_cap[2], list_right],
            [163, -215, -135, -5],
        );
        assert_eq!(geometry.visible, 13);
        assert_eq!(rect("row.0.label"), [158, -195, 138, -5]);
        assert_eq!(rect("row.12.label"), [-106, -195, -126, -5]);
        assert_eq!(rect("art.0"), [177, -259, -79, -131]);
        assert!(geometry.frame_art.is_empty(), "no Controls frame art");
        assert!(list_right < PICTURE_BOX[1]);
        assert!(list_right < DESCRIPTION_TEXT.rect[1]);
        let [top, left, bottom, right] = DESCRIPTION_TEXT.rect;
        assert!(
            top <= 240 && bottom >= -240 && left >= -320 && right <= 320,
            "on screen"
        );
    }

    #[test]
    fn the_list_and_picture_can_be_moved_without_a_rebuild() {
        let none = placement(|_| None);
        assert_eq!(none, (LIST_LAYOUT, PICTURE, vec![]));
        let env = |k: &str| match k {
            "H2_SKULLS_LIST_AT" => Some("-250, 120".to_string()),
            "H2_SKULLS_LIST_SKIN" => Some("8".to_string()),
            "H2_SKULLS_PICTURE_AT" => Some("x".to_string()),
            _ => None,
        };
        let (layout, picture, lines) = placement(env);
        assert_eq!(layout.anchor, [-250, 120]);
        assert_eq!(layout.skin, 8);
        assert_eq!(picture, PICTURE, "not understood: kept");
        assert_eq!(lines.len(), 3);
        assert!(lines[2].contains("not understood"), "{lines:?}");
    }

    /// Each row's label carries its state, and fits the list skin's 190
    /// units: the longest renders about 180 wide in conduit-12.
    /// The campaign page's On and Off are a value to step ("< On >"),
    /// Locked is not; the longest, "Grunt Birthday Party: < Off >", is a
    /// character more than its Locked label, and its brackets and spaces are
    /// narrower than "Locked"'s letters.
    #[test]
    fn row_labels_carry_the_state() {
        assert_eq!(text(row_label_id(12, State::On)), "Mythic: On");
        assert_eq!(text(row_label_id(12, State::Off)), "Mythic: Off");
        assert_eq!(text(row_label_id(12, State::Locked)), "Mythic: Locked");
        assert_eq!(text(arrow_label_id(12, State::On)), "Mythic: < On >");
        assert_eq!(text(arrow_label_id(12, State::Off)), "Mythic: < Off >");
        assert_eq!(
            arrow_label_id(12, State::Locked),
            row_label_id(12, State::Locked)
        );
        let longest = |f: fn(usize, State) -> u32, st| {
            (0..TOTAL).map(|i| text(f(i, st)).len()).max().unwrap()
        };
        assert!(longest(row_label_id, State::Locked) <= 28);
        assert!(longest(arrow_label_id, State::Off) <= 29);
    }

    /// The campaign page: left and right flip a collected skull (either
    /// way, both steps), its row then reads the new value; a locked one
    /// stays locked. A changes a skull too; only Start Level goes on. The
    /// multiplayer screen keeps A as its toggle and has no sideways step.
    #[test]
    fn left_right_and_a_toggle_and_start_level_goes_on() {
        let st = store();
        let s = SkullsScreen::new(Arc::clone(&st));
        s.open("Chief".into());
        skulls::record_pickup(&st, &names(&["Chief"]), 12);
        let mythic = s.row_of_skull(12).unwrap();
        let label = |row: i16| text(s.texts(row)[0].1);
        assert_eq!(label(mythic), "Mythic: < On >");
        assert_eq!(
            s.sideways(mythic, true).as_deref(),
            Some(&*format!(
                "skulls page: right on row {mythic}: skulls: Mythic off for Chief"
            ))
        );
        assert_eq!(s.state(12), State::Off);
        assert_eq!(label(mythic), "Mythic: < Off >");
        assert!(
            s.sideways(mythic, true)
                .unwrap()
                .ends_with("Mythic on for Chief")
        );
        assert_eq!(s.state(12), State::On, "right again: back on");
        assert!(
            s.sideways(mythic, false)
                .unwrap()
                .ends_with("Mythic off for Chief")
        );
        assert!(
            s.sideways(mythic, false)
                .unwrap()
                .ends_with("Mythic on for Chief")
        );
        assert_eq!(s.state(12), State::On);
        // Locked: says so, stays.
        let iron = s.row_of_skull(11).unwrap();
        assert!(
            s.sideways(iron, true)
                .unwrap()
                .contains("Iron is locked for Chief")
        );
        assert_eq!(s.state(11), State::Locked);
        assert_eq!(label(iron), "Iron: Locked");
        assert_eq!(s.sideways(s.count() as i16, true), None);
        assert_eq!(s.sideways(-1, false), None);
        // A on a skull changes it, as right does (a locked one says so);
        // only Start Level, the first row, goes on to the level.
        assert!(matches!(s.press(mythic), Chosen::Stepped(_)));
        assert_eq!(s.state(12), State::Off, "A flipped it");
        assert!(matches!(s.press(iron), Chosen::Stepped(_)));
        assert_eq!(s.state(11), State::Locked);
        assert_eq!(s.press(0), Chosen::Act);
        assert_eq!(s.sideways(0, true), None, "Start Level has no value");
        assert_eq!(s.press(s.count() as i16), Chosen::Nothing);
        // The multiplayer screen.
        let m = SkullsScreen::multiplayer(Arc::clone(&st));
        m.open("Chief".into());
        assert_eq!(m.sideways(0, true), None);
        assert!(matches!(m.press(0), Chosen::Stepped(_)));
    }

    /// The tier table: gold is the MCC multiplier above 1.0, and the
    /// description says it.
    #[test]
    fn gold_and_silver_follow_the_table() {
        use skulls::SkullTier::{Gold, Silver};
        let tier = |i: usize| ABOUT[i].tier;
        for i in [2, 3, 4, 6, 8, 9, 11, 12, 13] {
            assert_eq!(tier(i), Gold, "{}", ABOUT[i].name);
        }
        for i in [0, 1, 5, 7, 10, 14] {
            assert_eq!(tier(i), Silver, "{}", ABOUT[i].name);
        }
        assert_eq!(
            description(0),
            "Silver skull: Your flashlight is swapped for active camouflage. Found on Delta Halo, Legendary only."
        );
        assert!(description(0).starts_with("Silver skull: "));
        assert!(description(11).starts_with("Gold skull: "));
    }
}

/// Against your disc: the constants above are the retail tags' geometry.
/// Reads `maps/mainmenu.map` off the ISO named by `H2_DISC` (default
/// `disc/halo2.iso`); writes nothing.
/// `cargo test -p boot --lib skulls_menu::disc -- --ignored --nocapture`
#[cfg(test)]
mod disc {
    use super::*;
    use std::fs::File;
    use std::io::BufReader;
    use std::path::PathBuf;
    use tags::ui::{self as tui, MapSource, UiTag};

    fn screen(name: &str) -> tui::Screen {
        let path = std::env::var_os("H2_DISC")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../disc/halo2.iso")
            });
        let mut iso = xiso::Xiso::open(BufReader::new(File::open(&path).unwrap())).unwrap();
        let part = iso.partition_offset();
        let e = iso
            .entries()
            .unwrap()
            .into_iter()
            .find(|e| e.path.eq_ignore_ascii_case("/maps/mainmenu.map"))
            .expect("mainmenu.map on the disc");
        let mut m = map::Map::from_region(
            BufReader::new(File::open(&path).unwrap()),
            part + u64::from(e.sector) * xiso::SECTOR,
            u64::from(e.size),
        )
        .unwrap();
        let id = m
            .tag(map::TagQuery::NameInGroup(name, tui::SCREEN))
            .unwrap()
            .id;
        let mut src = MapSource::new(&mut m);
        match src.decode_tag(id).unwrap().tag {
            UiTag::Screen(s) => s,
            _ => panic!("{name} is not a screen"),
        }
    }

    #[test]
    #[ignore = "reads the disc named by H2_DISC"]
    fn the_constants_are_the_retail_screens_geometry() {
        let level = screen(r"ui\screens\game_shell\load_level\load_level");
        assert_eq!(level.screen_id, 11);
        let p = &level.panes[0];
        let l = &p.lists[0];
        assert_eq!(
            (l.skin, [l.anchor.x, l.anchor.y]),
            (LIST_LAYOUT.skin, LIST_LAYOUT.anchor)
        );
        let t = &p.text_blocks[0];
        let r = t.rect;
        assert_eq!([r.top, r.left, r.bottom, r.right], DESCRIPTION_TEXT.rect);
        assert_eq!(t.font, DESCRIPTION_TEXT.font);
        eprintln!("load_level description colour {:?}", t.text_color);
        assert_eq!(t.text_color, DESCRIPTION_TEXT.colour);
        let b = &p.bitmaps[0];
        assert_eq!(
            [b.top_left.y, b.top_left.x],
            [PICTURE_BOX[0], PICTURE_BOX[1]]
        );
        let diff = screen(r"ui\screens\game_shell\difficulty\difficulty");
        assert_eq!(diff.screen_id, PICTURE.screen_id as i16);
        let b = &diff.panes[PICTURE.pane as usize].bitmaps[PICTURE.bitmap as usize];
        assert_eq!(b.image, PICTURE.image);
        let controls =
            screen(r"ui\screens\game_shell\settings_screen\player_profile\controller_settings");
        let xs: Vec<i16> = controls.panes[0]
            .bitmaps
            .iter()
            .map(|b| b.top_left.x)
            .collect();
        assert_eq!(xs, [-280, 150], "the two Controls frame bitmaps");
        let controls_list = &controls.panes[0].lists[0];
        assert_eq!(
            (
                controls_list.flags,
                controls_list.animation,
                controls_list.item_value,
            ),
            (l.flags, l.animation, l.item_value),
            "the list fields inherited by the tag copy"
        );
        // Our picture and description slide with the page as the level
        // select's own do.
        assert_eq!(p.text_blocks[0].animation.0, DESCRIPTION_ANIMATION);
        assert_eq!(p.bitmaps[0].animation.0, PICTURE_ANIMATION);
        let f = &p.bitmaps[FRAME.bitmap as usize];
        assert_eq!(f.animation.0, FRAME_ANIMATION, "the bracket's own");
        assert_eq!([f.top_left.x, f.top_left.y], FRAME.at);
        // The rows end above the footer band of the tag the page copies.
        let path = std::env::var_os("H2_DISC")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../disc/halo2.iso")
            });
        let d = crate::ui_kit::preview::Disc::open(&path).unwrap();
        let geometry = d.geometry(Some(LIST_LAYOUT).into()).unwrap();
        let (label, font, _) = geometry.label;
        assert_eq!(
            [label.top, label.left, label.bottom, label.right],
            [28, 20, 8, 210]
        );
        assert_eq!(font, 1);
        for (_, text) in ROW_LABELS.iter() {
            let width = d.fonts.width(font, text);
            assert!(width <= label.width(), "{text:?} is {width} units wide");
        }
        let footer = d.shared.header_layouts[d.controls.header_layout()].secondary;
        let last_bottom = LIST_LAYOUT.anchor[1] - VISIBLE_ROWS * ITEM_HEIGHT;
        assert!(
            last_bottom > footer.top,
            "{last_bottom} runs into the footer at {}",
            footer.top
        );
        assert!(
            VISIBLE_ROWS < l.visible_items,
            "the level select's own runs further"
        );
    }
}
