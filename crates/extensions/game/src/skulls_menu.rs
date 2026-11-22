//! The in-game Skulls pages (docs/game/skulls.md "Menu", "Choosing them"):
//! after the campaign difficulty is chosen, a page of the pressing player's
//! skulls, when that profile has collected any, before the level starts;
//! and the multiplayer one the variant editor's Skulls row opens.
//!
//! **The flow** is `crate::skulls_screen`'s: a press on a difficulty opens
//! the campaign page. A on Start Level runs the difficulty's own press and
//! starts the level.
//!
//! **The screen** is our own list class (`crate::list_class`, as the Video
//! screen is), feature `skulls-screen`, showing a [`SkullsScreen`]: Start
//! Level, the level's options (a Halo 1 level's), then every skull, gold
//! first. Both pages, the campaign one and the multiplayer one, are one
//! design (docs/game/skulls.md "The page"):
//!
//! - A collected skull's row is its name and its value, "Mythic: On" (the
//!   level select's skin has one text a row, so the value is in the
//!   label, `components::row::pick` on a one-text skin). A opens the value
//!   picker on On and Off ([`SkullsScreen::pick`]), as retail's settings
//!   do for their two-value settings; the store saves the choice and the
//!   next campaign start turns on exactly the On ones
//!   ([`skulls::selection`]). A Halo 1 option's row opens its picker the
//!   same way. There are no left and right steps.
//! - A locked skull's row is its name alone (`components::row::disabled`):
//!   Locked is not a value. A on it does nothing. Its help says what it
//!   does, then on a line of its own where to find it.
//! - The legend follows the focused row ([`RowLegend`]): `A START  B BACK`
//!   on Start Level, `A CHANGE  B BACK` on a row A changes, `B BACK` on a
//!   locked skull; the same on both pages. B goes back.
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
//! On or Off, or locked until the profile has collected it.
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
use crate::ui_kit::input::Prompt;
use crate::ui_kit::picker::Pick;

/// The difficulty list's array name (`0x45d6e0`, "difficulty list").
pub const DIFFICULTY_LIST_NAME: u32 = 0x0045_d6e0;
/// The difficulty screen's id (`screen + 0x70`).
pub const DIFFICULTY_SCREEN_ID: i16 = 12;
/// The difficulty list's row chosen (`ret 8`).
pub const DIFFICULTY_ROW_CHOSEN: u32 = 0x002c_9d38;
/// The screen's list class (`features.skulls-screen` switches it).
pub const FEATURE: &str = "skulls-screen";

/// Our strings: a name per skull, the titles, the legends, Start Level
/// with its help, and the help lines. `layout_glue::is_ours` looks at the
/// middle bytes only, so a base one higher in the top byte than
/// [`ID_BASE`] is ours too and clear of every id the model and the other
/// rows take (`ID_BASE + 0x00..0xff`).
pub const SKULLS_ID_BASE: u32 = crate::ui_kit::ids::family("skulls").base;
/// "Skulls": the variant editor's row (`crate::skulls_mp_row`).
pub const TITLE_ID: u32 = SKULLS_ID_BASE + 0x10;
/// Both pages' help per multiplayer row (`skulls_mp::MP`): while it is
/// collected, then while it is locked.
const MP_DESCRIPTIONS: Range<u32> = SKULLS_ID_BASE + 0x30..SKULLS_ID_BASE + 0x34;
const MP_LOCKED_DESCRIPTIONS: Range<u32> = SKULLS_ID_BASE + 0x34..SKULLS_ID_BASE + 0x38;
/// A name and a help line per skull, ours included (room for 64). The
/// name is a locked skull's row.
const LABELS: Range<u32> = SKULLS_ID_BASE + 0x40..SKULLS_ID_BASE + 0x80;
const DESCRIPTIONS: Range<u32> = SKULLS_ID_BASE + 0x80..SKULLS_ID_BASE + 0xc0;
/// The HUD message when one of our skulls is picked up
/// (`crate::skulls_custom`), by skull index.
const FOUND: Range<u32> = SKULLS_ID_BASE + 0xc0..SKULLS_ID_BASE + 0x100;
/// Each skull's row texts, three per skull: its label On ("Mythic: On"),
/// its label Off, and its help while locked. The list skin has one text
/// per row, so a collected skull's value is in its label.
const ROW_BASE: u32 = crate::ui_kit::ids::family("skull rows").base;
pub const TITLE: &str = "Skulls";
/// Both pages' title, the campaign one and the multiplayer one.
pub const PAGE_TITLE: &str = "SKULLS";
pub const PAGE_TITLE_ID: u32 = SKULLS_ID_BASE + 0x14;
/// The campaign page's first row, and what it says.
pub const START: &str = "Start Level";
pub const START_ID: u32 = SKULLS_ID_BASE + 0x15;
pub const START_DESCRIPTION: &str =
    "Start the level with the options and skulls below.";
pub const START_DESCRIPTION_ID: u32 = SKULLS_ID_BASE + 0x16;
/// What a locked skull's help says before where to find it.
pub const LOCKED: &str = "Locked";
/// A line break in a served text (the game's, as `crate::coop_live`'s).
const BREAK: &str = "\r\n";

/// The legends, one per [`RowLegend`] (the footer's place, set as the
/// focus moves: `Rows::focused_footer`).
pub const PROMPT: &str = "\u{e100} START  \u{e101} BACK";
pub const FOOTER_ID: u32 = SKULLS_ID_BASE + 0x13;
pub const SETTING_PROMPT: &str = "\u{e100} CHANGE  \u{e101} BACK";
pub const SETTING_FOOTER_ID: u32 = SKULLS_ID_BASE + 0x17;
pub const LOCKED_PROMPT: &str = "\u{e101} BACK";
pub const LOCKED_FOOTER_ID: u32 = SKULLS_ID_BASE + 0x18;

/// What the legend offers on a row, by what A does there: the same on
/// both pages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowLegend {
    /// Start Level: A starts the level.
    Start,
    /// A collected skull or a Halo 1 option: A opens the value picker.
    Setting,
    /// A locked skull: A does nothing.
    Locked,
}

impl RowLegend {
    /// Its string id, the footer's text.
    pub fn id(self) -> u32 {
        match self {
            Self::Start => FOOTER_ID,
            Self::Setting => SETTING_FOOTER_ID,
            Self::Locked => LOCKED_FOOTER_ID,
        }
    }
    /// Its text as served.
    pub fn text(self) -> &'static str {
        match self {
            Self::Start => PROMPT,
            Self::Setting => SETTING_PROMPT,
            Self::Locked => LOCKED_PROMPT,
        }
    }
    /// Its prompts, the design system's (`components::legend`).
    pub fn prompts(self) -> Vec<Prompt> {
        use crate::ui_kit::components::legend;
        match self {
            Self::Start => vec![legend::start(), legend::back()],
            Self::Setting => legend::setting_row(),
            Self::Locked => legend::disabled_row(),
        }
    }
}

/// The reference layout: the campaign level select
/// (`ui\screens\game_shell\load_level\load_level`, screen 11, mainmenu.map,
/// read with `h2 ui`): list anchor (-215, 130) on skin 0
/// (`ui\list_skins\default\default`, rows 200 wide, one text), picture box
/// `[177 39 -30 259]` (top, left, bottom, right; Y up, screen centre
/// origin), description text `[-30 39 -133 260]`, font 1. The Controls
/// frame art right of the centre (`br_06`, `[-30 150 -286 406]`) is left out:
/// it sits where the description goes.
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
/// cut its arm off (docs/game/ui-builder.md, "The checks"), so none of the
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
pub const SCROLL_ARROWS: ScrollArrows = ScrollArrows {
    up: [224, 46],
    down: [224, -(VISIBLE_ROWS - 1) * ITEM_HEIGHT - 8],
};

/// The level select's frame and list, with room for the active arrows on
/// lists long enough to scroll. Skin overrides keep their own arrows.
pub fn level_select_list(class: ListClass, layout: ListLayout) -> ListClass {
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
/// top: top-left (36, 180).
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

/// The id of skull `i`'s row label in `state`: "Mythic: On", "Mythic:
/// Off", or a locked skull's name alone ([`label_id`]).
pub fn row_label_id(i: usize, state: State) -> u32 {
    match state {
        State::On => ROW_BASE + 3 * i as u32,
        State::Off => ROW_BASE + 3 * i as u32 + 1,
        State::Locked => label_id(i),
    }
}

/// The id of skull `i`'s help while it is locked.
pub fn locked_description_id(i: usize) -> u32 {
    ROW_BASE + 3 * i as u32 + 2
}

/// The id of skull `i`'s help in `state`.
pub fn help_id(i: usize, state: State) -> u32 {
    match state {
        State::Locked => locked_description_id(i),
        State::On | State::Off => description_id(i),
    }
}

/// Skull `i`'s help in `state`, as served.
pub fn help(i: usize, state: State) -> String {
    match state {
        State::Locked => locked_description(i),
        State::On | State::Off => description(i),
    }
}

static ROW_TEXTS: LazyLock<Vec<(u32, String)>> = LazyLock::new(|| {
    (0..TOTAL)
        .flat_map(|i| {
            let label = SKULLS[i].label;
            [
                (row_label_id(i, State::On), format!("{label}: On")),
                (row_label_id(i, State::Off), format!("{label}: Off")),
                (locked_description_id(i), locked_description(i)),
            ]
        })
        .collect()
});

/// The label id of skull `i`: its name.
pub fn label_id(i: usize) -> u32 {
    LABELS.start + i as u32
}

/// The description id of skull `i`, collected.
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

/// What skull `i` does: its effect, after the game's full name when the
/// row shows a short one ("IWHBYD").
pub fn effect(i: usize) -> String {
    let (d, a) = (&SKULLS[i], &ABOUT[i]);
    if d.label == a.name {
        a.effect.to_string()
    } else {
        format!("{}: {}", a.name, a.effect)
    }
}

/// Where skull `index` is: its level, "on Legendary" when only a
/// Legendary game places it, and anything else its script asks.
fn whereabouts(index: usize) -> String {
    let a = &ABOUT[index];
    let legendary = if a.legendary { " on Legendary" } else { "" };
    let note = a.note.map_or(String::new(), |n| format!(" ({n})"));
    format!("{}{legendary}{note}", a.level)
}

/// The help's second line for collected skull `index`: its tier and where
/// it is found.
fn found_line(index: usize) -> String {
    format!(
        "{} skull, found on {}.",
        ABOUT[index].tier.name(),
        whereabouts(index)
    )
}

/// The help's second line for locked skull `index`: where to find it.
fn locked_line(index: usize) -> String {
    format!("{LOCKED}: find it on {}.", whereabouts(index))
}

/// Collected skull `i`'s help: what it does, then on a line of its own its
/// tier and where it is found.
pub fn description(i: usize) -> String {
    format!("{}{BREAK}{}", effect(i), found_line(i))
}

/// Locked skull `i`'s help: what it does, then on a line of its own that
/// it is locked and where to find it ("Locked: find it on The Arbiter on
/// Legendary.").
pub fn locked_description(i: usize) -> String {
    format!("{}{BREAK}{}", effect(i), locked_line(i))
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

/// The help id of multiplayer row `row`, collected.
pub fn mp_description_id(row: usize) -> u32 {
    MP_DESCRIPTIONS.start + row as u32
}

/// The help id of multiplayer row `row` while it is locked.
pub fn mp_locked_description_id(row: usize) -> u32 {
    MP_LOCKED_DESCRIPTIONS.start + row as u32
}

/// What multiplayer row `row` does in a multiplayer game, and that the
/// host's choice is everyone's.
fn mp_effect(row: usize) -> String {
    format!(
        "{} The host's choice is everyone's.",
        crate::skulls_mp::MP[row].effect
    )
}

/// Multiplayer row `row`'s help, collected: what it does, then its tier
/// and where it is found.
pub fn mp_description(row: usize) -> String {
    let index = crate::skulls_mp::MP[row].index;
    format!("{}{BREAK}{}", mp_effect(row), found_line(index))
}

/// Multiplayer row `row`'s help while locked: what it does, then that it
/// is locked and where to find it in the campaign.
pub fn mp_locked_description(row: usize) -> String {
    let index = crate::skulls_mp::MP[row].index;
    format!("{}{BREAK}{}", mp_effect(row), locked_line(index))
}

/// Multiplayer row `row`'s help in `state`.
pub fn mp_help(row: usize, state: State) -> String {
    match state {
        State::Locked => mp_locked_description(row),
        State::On | State::Off => mp_description(row),
    }
}

static MP_DESCRIPTION_TEXTS: LazyLock<Vec<(u32, String)>> = LazyLock::new(|| {
    (0..crate::skulls_mp::MP.len())
        .flat_map(|r| {
            [
                (mp_description_id(r), mp_description(r)),
                (mp_locked_description_id(r), mp_locked_description(r)),
            ]
        })
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
    for l in [RowLegend::Start, RowLegend::Setting, RowLegend::Locked] {
        v.push((l.id(), l.text()));
    }
    v.extend(ROW_TEXTS.iter().map(|(i, t)| (*i, t.as_str())));
    v.extend(
        DESCRIPTION_TEXTS
            .iter()
            .enumerate()
            .map(|(i, t)| (description_id(i), t.as_str())),
    );
    v.extend(MP_DESCRIPTION_TEXTS.iter().map(|(i, t)| (*i, t.as_str())));
    v.extend(FOUND_TEXTS.iter().map(|(i, t)| (found_id(*i), t.as_str())));
    v
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

/// A skull's two values, as its picker lists them: On first.
pub const VALUES: [&str; 2] = ["On", "Off"];

/// `state`'s place in [`VALUES`]; `None` for Locked.
fn value_index(state: State) -> Option<usize> {
    match state {
        State::On => Some(0),
        State::Off => Some(1),
        State::Locked => None,
    }
}

fn word(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

/// The picker for collected skull `i` of `profile`'s campaign page: On or
/// Off, the held one focused; its choice is saved to the profile at once.
/// `None` while the skull is locked.
pub fn skull_pick(store: &Arc<Store>, profile: &str, i: usize) -> Option<Pick> {
    let current = value_index(skulls::state(store, profile, i)?)?;
    let (held, profile_held) = (Arc::clone(store), profile.to_string());
    let (label, key) = (SKULLS[i].label, SKULLS[i].key);
    let set = Arc::new(move |v: usize| {
        let on = v == 0;
        let p = profile_held.as_str();
        match skulls::state(&held, p, i) {
            Some(State::Locked) | None => {
                format!("skulls menu: {label} is locked for {p}; nothing changed")
            }
            Some(s) if (s == State::On) == on => {
                format!("skulls: {label} stays {} for {p}", word(on))
            }
            Some(_) => skulls::toggle(&held, p, key).unwrap_or_else(|e| e),
        }
    });
    Some(Pick::new(
        label,
        VALUES.iter().map(|v| v.to_string()).collect(),
        current,
        &description(i),
        set,
    ))
}

/// The picker for multiplayer row `row` of `profile`'s screen: On or Off;
/// its choice is saved and an open lobby's advertisement follows it.
/// `None` while the skull is locked.
pub fn mp_pick(store: &Arc<Store>, profile: &str, row: usize) -> Option<Pick> {
    let m = crate::skulls_mp::MP.get(row)?;
    let current = value_index(crate::skulls_mp::state(store, profile, row)?)?;
    let (held, profile_held) = (Arc::clone(store), profile.to_string());
    let label = SKULLS[m.index].label;
    let set = Arc::new(move |v: usize| {
        let on = v == 0;
        let p = profile_held.as_str();
        let line = match crate::skulls_mp::state(&held, p, row) {
            Some(State::Locked) | None => {
                format!("skulls menu: {label} is locked for {p} in multiplayer; nothing changed")
            }
            Some(s) if (s == State::On) == on => {
                format!("skulls: {label} stays {} in multiplayer for {p}", word(on))
            }
            Some(_) => crate::skulls_mp::press(&held, p, row).unwrap_or_default(),
        };
        // An open lobby's advertisement follows the change.
        crate::skulls_mp::changed(p);
        line
    });
    Some(Pick::new(
        label,
        VALUES.iter().map(|v| v.to_string()).collect(),
        current,
        &mp_description(row),
        set,
    ))
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
    /// Multiplayer row `row`, when there is one.
    fn mp_row(&self, row: i16) -> Option<usize> {
        usize::try_from(row)
            .ok()
            .filter(|r| *r < crate::skulls_mp::MP.len())
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
    /// Row `row`'s skull and its state, on either page: `None` for Start
    /// Level, an option or no row.
    pub fn skull_state(&self, row: i16) -> Option<(usize, State)> {
        match self.mode() {
            Mode::Multiplayer => {
                let r = self.mp_row(row)?;
                Some((crate::skulls_mp::MP[r].index, self.mp_state(r)))
            }
            Mode::Campaign => {
                let i = self.skull(row)?;
                Some((i, self.state(i)))
            }
        }
    }
    /// The value picker A opens on row `row`: a Halo 1 option's, or a
    /// collected skull's On and Off, on either page. `None` for Start
    /// Level, a locked skull, no profile or no row: A does nothing there.
    pub fn pick(&self, row: i16) -> Option<Pick> {
        match self.mode() {
            Mode::Multiplayer => mp_pick(&self.store, &self.profile()?, self.mp_row(row)?),
            Mode::Campaign => match self.page_row(row)? {
                PageRow::Start => None,
                PageRow::Option(o) => Some(crate::h1_options::pick(&self.store, o)),
                PageRow::Skull(i) => skull_pick(&self.store, &self.profile()?, i),
            },
        }
    }
    /// What the legend offers on row `row`.
    pub fn row_legend(&self, row: i16) -> RowLegend {
        if self.mode() == Mode::Campaign {
            match self.page_row(row) {
                Some(PageRow::Start) => return RowLegend::Start,
                Some(PageRow::Option(_)) => return RowLegend::Setting,
                Some(PageRow::Skull(_)) | None => {}
            }
        }
        match self.skull_state(row) {
            Some((_, State::On | State::Off)) => RowLegend::Setting,
            Some((_, State::Locked)) | None => RowLegend::Locked,
        }
    }
    /// Row `row`'s legend prompts.
    pub fn row_prompts(&self, row: i16) -> Vec<Prompt> {
        self.row_legend(row).prompts()
    }
}

impl Rows for SkullsScreen {
    fn content_spec(&self) -> Option<crate::ui_kit::spec::ScreenSpec> {
        Some(crate::ui_kit::spec::Describe::spec(self))
    }

    fn focused_footer(&self, row: Option<i16>) -> Option<u32> {
        Some(self.row_legend(row.unwrap_or(0)).id())
    }

    fn count(&self) -> usize {
        match self.mode() {
            Mode::Campaign => 1 + self.options().len() + TOTAL,
            Mode::Multiplayer => crate::skulls_mp::MP.len(),
        }
    }
    /// Both pages: [`PAGE_TITLE`].
    fn title(&self) -> u32 {
        PAGE_TITLE_ID
    }
    fn texts(&self, row: i16) -> Vec<(u32, u32)> {
        if self.mode() == Mode::Campaign {
            match self.page_row(row) {
                Some(PageRow::Start) => return vec![(LABEL_SLOT, START_ID)],
                Some(PageRow::Option(o)) => {
                    return vec![(
                        LABEL_SLOT,
                        crate::h1_options::label_id(o, crate::h1_options::is_halo1(&self.store, o)),
                    )];
                }
                Some(PageRow::Skull(_)) | None => {}
            }
        }
        self.skull_state(row)
            .map(|(i, state)| vec![(LABEL_SLOT, row_label_id(i, state))])
            .unwrap_or_default()
    }
    /// A on Start Level starts the level (the class's action); on a Halo 1
    /// option or a collected skull it opens the value picker
    /// ([`SkullsScreen::pick`]); on a locked skull it does nothing.
    fn press(&self, row: i16) -> Chosen {
        if self.mode() == Mode::Campaign && self.page_row(row) == Some(PageRow::Start) {
            return Chosen::Act;
        }
        self.pick(row)
            .and_then(crate::ui_kit::picker::offer)
            .map_or(Chosen::Nothing, Chosen::Open)
    }
    fn description(&self, row: i16) -> Option<u32> {
        if self.mode() == Mode::Multiplayer {
            let r = self.mp_row(row)?;
            return Some(match self.mp_state(r) {
                State::Locked => mp_locked_description_id(r),
                State::On | State::Off => mp_description_id(r),
            });
        }
        match self.page_row(row)? {
            PageRow::Start => Some(START_DESCRIPTION_ID),
            PageRow::Option(o) => Some(crate::h1_options::description_id(o)),
            PageRow::Skull(i) => Some(help_id(i, self.state(i))),
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
const _: () = assert!(3 * TOTAL as u32 <= 0x100);
const _: () =
    assert!(crate::skulls_mp::MP.len() as u32 <= MP_DESCRIPTIONS.end - MP_DESCRIPTIONS.start);
const _: () = assert!(
    crate::skulls_mp::MP.len() as u32 <= MP_LOCKED_DESCRIPTIONS.end - MP_LOCKED_DESCRIPTIONS.start
);

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
    /// the value ("Mythic: On"); a locked skull's is its name alone.
    fn drawn(s: &SkullsScreen) -> Vec<(String, Option<String>)> {
        (0..s.count() as i16)
            .filter(|r| {
                s.mode() == Mode::Multiplayer || matches!(s.page_row(*r), Some(PageRow::Skull(_)))
            })
            .map(|r| {
                let t = s.texts(r);
                assert_eq!(t.len(), 1, "one text: the list skin has one");
                assert_eq!(t[0].0, LABEL_SLOT);
                let label = text(t[0].1);
                match label.rsplit_once(": ") {
                    Some((name, value)) => (name.to_string(), Some(value.to_string())),
                    None => (label, None),
                }
            })
            .collect()
    }

    /// A on row `row`, then the other value picked: what the picker's set
    /// logs; `None` where A opens no picker.
    fn flip(s: &SkullsScreen, row: i16) -> Option<String> {
        let p = s.pick(row)?;
        Some((p.set)(1 - p.current))
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
        assert_eq!(text(PAGE_TITLE_ID), "SKULLS");
    }

    #[test]
    fn unlocked_comes_from_the_profiles_settings() {
        let st = store();
        let s = SkullsScreen::new(Arc::clone(&st));
        // No profile yet: everything locked, each its name alone.
        assert!(drawn(&s).iter().all(|(_, v)| v.is_none()));
        skulls::record_pickup(&st, &names(&["Chief"]), 12);
        skulls::record_pickup(&st, &names(&["Chief"]), 1);
        skulls::toggle(&st, "Chief", "skulls.grunt_birthday_party").unwrap();
        skulls::record_pickup(&st, &names(&["Arbiter"]), 3);
        s.open("Chief".into());
        let values: Vec<_> = drawn(&s).into_iter().map(|(_, v)| v).collect();
        for (r, v) in values.iter().enumerate() {
            let i = ORDER[r];
            let want = match i {
                1 => Some("Off"),
                12 => Some("On"),
                _ => None,
            };
            assert_eq!(v.as_deref(), want, "skull {i}");
        }
        // The other co-op player's screen is theirs.
        s.open("Arbiter".into());
        assert_eq!(s.state(3), State::On);
        assert_eq!(s.state(12), State::Locked);
    }

    /// A on a collected skull opens the value picker on On and Off, the
    /// held one focused, and the pick is saved at once; a locked skull, or
    /// a page with no profile, has no picker and A does nothing.
    #[test]
    fn locked_skulls_have_no_picker_and_collected_ones_pick_on_or_off() {
        let st = store();
        let s = SkullsScreen::new(Arc::clone(&st));
        let first = s.row_of_skull(0).unwrap();
        assert!(s.pick(first).is_none(), "no profile: nothing to pick");
        assert_eq!(s.press(first), Chosen::Nothing);
        s.open("Chief".into());
        let iron = s.row_of_skull(11).unwrap();
        assert!(s.pick(iron).is_none(), "locked: no picker");
        assert_eq!(s.press(iron), Chosen::Nothing);
        assert_eq!(s.state(11), State::Locked, "a locked press changes nothing");
        assert_eq!(skulls::collected(&st, "Chief"), []);
        skulls::record_pickup(&st, &names(&["Chief"]), 11);
        let pick = s.pick(iron).expect("collected: the picker");
        assert_eq!(pick.title, "IRON");
        assert_eq!(pick.values, ["On", "Off"]);
        assert_eq!(pick.current, 0, "a pickup turns it on");
        assert_eq!(pick.description, description(11));
        assert_eq!(s.state(11), State::On, "opening the picker changes nothing");
        assert_eq!((pick.set)(1), "skulls: Iron off for Chief");
        assert_eq!(s.state(11), State::Off);
        assert_eq!((pick.set)(1), "skulls: Iron stays off for Chief");
        assert_eq!(s.state(11), State::Off);
        assert_eq!((pick.set)(0), "skulls: Iron on for Chief");
        assert_eq!(s.state(11), State::On);
        assert_eq!(s.pick(iron).unwrap().current, 0, "the held one focused");
        assert!(s.pick(0).is_none(), "Start Level has no value");
        assert!(s.pick(s.count() as i16).is_none());
        assert!(s.pick(-1).is_none());
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
        flip(&s, s.row_of_skull(3).unwrap()); // Thunderstorm off
        assert_eq!(flip(&s, s.row_of_skull(9).unwrap()), None, "Catch: locked");
        let mut want = [false; COUNT];
        want[0] = true;
        want[12] = true;
        assert_eq!(skulls::selection(&st, &names(&["Chief"])), Some(want));
        // Co-op: the other player turns theirs off on their own screen; the
        // start takes the union of both.
        s.open("Arbiter".into());
        flip(&s, s.row_of_skull(7).unwrap());
        let both = names(&["Chief", "Arbiter"]);
        assert_eq!(skulls::selection(&st, &both), Some(want));
        flip(&s, s.row_of_skull(7).unwrap());
        want[7] = true;
        assert_eq!(skulls::selection(&st, &both), Some(want));
        // Toggling back restores it.
        s.open("Chief".into());
        flip(&s, s.row_of_skull(3).unwrap());
        want[3] = true;
        assert_eq!(skulls::selection(&st, &both), Some(want));
    }

    /// A skull's help says what it does, then on a line of its own where
    /// it is: collected, its tier and where it was found; locked, "Locked:
    /// find it on <level>", "on Legendary" when only a Legendary game
    /// places it. The row's help follows its state.
    #[test]
    fn each_help_says_what_it_does_then_where_it_is() {
        let st = store();
        let s = SkullsScreen::new(Arc::clone(&st));
        s.open("Chief".into());
        for (i, a) in ABOUT.iter().enumerate() {
            let row = s.row_of_skull(i).unwrap();
            assert_eq!(s.description(row), Some(locked_description_id(i)), "locked");
            for (shown, second) in [
                (description(i), found_line(i)),
                (locked_description(i), locked_line(i)),
            ] {
                let lines: Vec<&str> = shown.split(BREAK).collect();
                assert_eq!(lines, [effect(i).as_str(), second.as_str()], "{shown}");
                assert!(lines[0].ends_with(a.effect), "{shown}");
                assert!(second.contains(a.level), "{shown}");
                assert_eq!(second.contains(" on Legendary"), a.legendary, "{shown}");
                // The buttons are the footer's, not under the description;
                // a text widget holds 256 UTF-16 units, and the box is small.
                assert!(!shown.contains('\u{e100}'), "{shown}");
                assert!(shown.encode_utf16().count() <= 160, "{shown}");
            }
            assert!(
                locked_line(i).starts_with(&format!("Locked: find it on {}", a.level)),
                "{}",
                locked_line(i)
            );
            assert!(found_line(i).starts_with(&format!("{} skull, found on {}", a.tier.name(), a.level)));
            assert_eq!(text(description_id(i)), description(i));
            assert_eq!(text(locked_description_id(i)), locked_description(i));
            assert_eq!(help(i, State::Locked), locked_description(i));
            assert_eq!(help(i, State::Off), description(i));
        }
        assert_eq!(
            locked_description(1),
            "A headshot on a Grunt bursts into confetti and cheering.\r\n\
             Locked: find it on The Arbiter on Legendary."
        );
        assert_eq!(
            description(6),
            "Your HUD and first-person weapon are hidden.\r\nGold skull, found on Outskirts."
        );
        assert_eq!(
            description(5),
            "I Would Have Been Your Daddy: Rare lines play far more often.\r\n\
             Silver skull, found on Outskirts on Legendary (solo; one try in seven)."
        );
        assert_eq!(
            description(crate::skulls_custom::FALL_DAMAGE),
            "A hard landing hurts: past a jump's speed, a fall drains your shields, then your \
             health.\r\nGold skull, found on Metropolis."
        );
        // Collected, the row's help is the collected one.
        skulls::record_pickup(&st, &names(&["Chief"]), 6);
        assert_eq!(s.description(s.row_of_skull(6).unwrap()), Some(description_id(6)));
        assert_eq!(s.description(s.count() as i16), None);
        assert_eq!(text(FOOTER_ID), PROMPT);
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
    fn the_description_text_is_on_screen() {
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
        // A name and a collected help for all, three row texts each (On,
        // Off, the locked help), two helps per multiplayer row, a found
        // message for each of ours, the row's and the pages' titles, Start
        // Level with its description and the three legends.
        assert_eq!(
            all.len(),
            2 * TOTAL + 7 + 3 * TOTAL + 2 * crate::skulls_mp::MP.len() + crate::skulls_custom::OURS
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
        assert!(all.contains(&(PAGE_TITLE_ID, "SKULLS")));
        assert!(all.contains(&(SETTING_FOOTER_ID, SETTING_PROMPT)));
        assert!(all.contains(&(LOCKED_FOOTER_ID, LOCKED_PROMPT)));
    }

    /// The multiplayer instance: only the skulls that work in multiplayer,
    /// each the profile's multiplayer On or Off, or its name alone while
    /// locked; A opens the same picker as the campaign page's, and the pick
    /// changes the multiplayer choice, never the campaign one.
    #[test]
    fn the_multiplayer_mode_lists_only_multiplayer_skulls() {
        let st = Arc::new(Store::defaults(crate::setting_defs::ALL));
        let s = SkullsScreen::multiplayer(Arc::clone(&st));
        s.open("Chief".into());
        assert_eq!(s.mode(), Mode::Multiplayer);
        assert_eq!(s.count(), 3);
        assert_eq!(s.title(), PAGE_TITLE_ID, "the campaign page's title");
        let shown: Vec<_> = drawn(&s).into_iter().map(|(n, _)| n).collect();
        assert_eq!(shown, ["Famine", "Blind", "Low Gravity"]);
        assert!(
            drawn(&s).iter().all(|(_, v)| v.is_none()),
            "nothing collected: names alone"
        );
        assert!(s.texts(3).is_empty());
        assert_eq!(s.description(1), Some(mp_locked_description_id(1)));
        assert_eq!(
            text(mp_locked_description_id(1)),
            "Every player's HUD and first-person weapon are hidden. The host's choice is \
             everyone's.\r\nLocked: find it on Outskirts."
        );
        assert!(
            text(mp_locked_description_id(0))
                .ends_with("\r\nLocked: find it on The Oracle on Legendary.")
        );
        assert!(s.pick(1).is_none(), "locked: no picker");
        assert_eq!(s.press(1), Chosen::Nothing);
        assert_eq!(s.row_legend(1), RowLegend::Locked);
        skulls::record_pickup(&st, &names(&["Chief"]), 6);
        assert_eq!(s.mp_state(1), State::Off, "collected, off for multiplayer");
        assert_eq!(s.row_legend(1), RowLegend::Setting);
        assert_eq!(s.description(1), Some(mp_description_id(1)));
        assert_eq!(
            text(mp_description_id(1)),
            "Every player's HUD and first-person weapon are hidden. The host's choice is \
             everyone's.\r\nGold skull, found on Outskirts."
        );
        assert_eq!(text(s.texts(1)[0].1), "Blind: Off");
        let pick = s.pick(1).expect("collected: the picker");
        assert_eq!(pick.title, "BLIND");
        assert_eq!(pick.values, ["On", "Off"]);
        assert_eq!(pick.current, 1);
        assert_eq!((pick.set)(0), "skulls: Blind on in multiplayer for Chief");
        assert_eq!(s.mp_state(1), State::On);
        assert_eq!((pick.set)(0), "skulls: Blind stays on in multiplayer for Chief");
        assert_eq!(text(s.texts(1)[0].1), "Blind: On");
        assert!(s.pick(0).is_none(), "Famine: locked");
        assert!(s.pick(3).is_none());
        assert_eq!(crate::skulls_mp::chosen(&st, "Chief"), vec![6]);
        // Opening it again keeps it the multiplayer screen.
        s.open("Chief".into());
        assert_eq!(s.count(), crate::skulls_mp::MP.len());
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

    /// A collected skull's label carries its value in the one-text list
    /// skin; a locked skull's is its name alone (Locked is not a value).
    #[test]
    fn row_labels_carry_the_value_and_a_locked_skull_its_name_alone() {
        assert_eq!(text(row_label_id(12, State::On)), "Mythic: On");
        assert_eq!(text(row_label_id(12, State::Off)), "Mythic: Off");
        assert_eq!(text(row_label_id(12, State::Locked)), "Mythic");
        assert_eq!(row_label_id(12, State::Locked), label_id(12));
        for i in 0..TOTAL {
            assert!(text(row_label_id(i, State::Off)).len() <= 28);
        }
    }

    /// The legend follows the focus, the same on both pages: Start Level
    /// `A START  B BACK`, a Halo 1 option or a collected skull `A CHANGE
    /// B BACK`, a locked skull `B BACK`; A on an option opens its picker.
    #[test]
    fn campaign_options_open_the_picker_and_footers_follow_focus() {
        use crate::h1_options::{self, ASSETS, CAMPAIGN, HUD};
        use crate::ui_kit::{input::footer_legend, picker};

        let defs: Vec<&'static settings::Def> = SKULLS
            .iter()
            .chain([ASSETS.def, HUD.def])
            .collect();
        let store = Arc::new(Store::defaults(&defs));
        let screen = SkullsScreen::new(Arc::clone(&store));
        screen.open_with("Chief".into(), CAMPAIGN);
        let locked = screen.row_of_skull(12).unwrap();
        skulls::record_pickup(&store, &names(&["Chief"]), 2);
        let collected = screen.row_of_skull(2).unwrap();
        assert_eq!(text(screen.focused_footer(Some(0)).unwrap()), PROMPT);
        assert_eq!(text(screen.focused_footer(Some(1)).unwrap()), SETTING_PROMPT);
        assert_eq!(text(screen.focused_footer(Some(collected)).unwrap()), SETTING_PROMPT);
        assert_eq!(text(screen.focused_footer(Some(locked)).unwrap()), LOCKED_PROMPT);
        for row in [0, 1, collected, locked] {
            assert_eq!(
                text(screen.focused_footer(Some(row)).unwrap()),
                footer_legend(&screen.row_prompts(row))
            );
        }
        let mp = SkullsScreen::multiplayer(Arc::clone(&store));
        mp.open("Chief".into());
        assert_eq!(mp.focused_footer(Some(0)), Some(LOCKED_FOOTER_ID), "Famine: locked");

        let halo1 = h1_options::is_halo1(&store, &ASSETS);
        assert_eq!(screen.press(1), Chosen::Open(picker::NAME));
        let pick = screen.pick(1).expect("campaign asset picker");
        assert_eq!(pick.title, "ASSETS");
        assert_eq!(pick.values, ["Halo 1", "Halo 2"]);
        assert_eq!(pick.current, usize::from(!halo1));
        assert_eq!(h1_options::is_halo1(&store, &ASSETS), halo1);
        (pick.set)(usize::from(halo1));
        assert_eq!(h1_options::is_halo1(&store, &ASSETS), !halo1);
        assert_eq!(screen.focused_footer(Some(1)), Some(SETTING_FOOTER_ID));
    }

    /// No row steps sideways on either page: A opens the picker on a
    /// collected skull (the pick changes it, A alone doesn't), does nothing
    /// on a locked one, and only Start Level goes on.
    #[test]
    fn a_opens_the_picker_and_nothing_steps_sideways() {
        let st = store();
        let s = SkullsScreen::new(Arc::clone(&st));
        s.open("Chief".into());
        skulls::record_pickup(&st, &names(&["Chief"]), 12);
        let mythic = s.row_of_skull(12).unwrap();
        let label = |row: i16| text(s.texts(row)[0].1);
        assert_eq!(label(mythic), "Mythic: On");
        for row in -1..=s.count() as i16 {
            assert_eq!(s.sideways(row, true), None, "row {row}");
            assert_eq!(s.sideways(row, false), None, "row {row}");
        }
        assert_eq!(s.press(mythic), Chosen::Open(crate::ui_kit::picker::NAME));
        assert_eq!(s.state(12), State::On, "A alone changes nothing");
        assert_eq!(flip(&s, mythic).as_deref(), Some("skulls: Mythic off for Chief"));
        assert_eq!(s.state(12), State::Off);
        assert_eq!(label(mythic), "Mythic: Off");
        let iron = s.row_of_skull(11).unwrap();
        assert_eq!(s.press(iron), Chosen::Nothing);
        assert_eq!(s.state(11), State::Locked);
        assert_eq!(label(iron), "Iron");
        assert_eq!(s.press(0), Chosen::Act);
        assert_eq!(s.press(s.count() as i16), Chosen::Nothing);
        let m = SkullsScreen::multiplayer(Arc::clone(&st));
        m.open("Chief".into());
        for row in 0..m.count() as i16 {
            assert_eq!(m.sideways(row, true), None);
        }
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
            "Your flashlight is swapped for active camouflage.\r\nSilver skull, found on Delta Halo on Legendary."
        );
        assert!(found_line(0).starts_with("Silver skull, "));
        assert!(found_line(11).starts_with("Gold skull, "));
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
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../disc/halo2.iso")
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
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../disc/halo2.iso")
            });
        let d = crate::ui_kit::preview::Disc::open(&path).unwrap();
        let geometry = d.geometry(Some(LIST_LAYOUT).into()).unwrap();
        let (label, font, _) = geometry.label;
        assert_eq!(
            [label.top, label.left, label.bottom, label.right],
            [28, 20, 8, 210]
        );
        assert_eq!(font, 1);
        let labels = (0..TOTAL).flat_map(|i| {
            let l = SKULLS[i].label;
            [format!("{l}: On"), format!("{l}: Off"), l.to_string()]
        });
        for text in labels {
            let width = d.fonts.width(font, &text);
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
