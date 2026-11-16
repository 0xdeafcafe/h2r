//! The in-game Skulls menu (docs/game/skulls.md "Menu"): a "Skulls" row on the
//! campaign difficulty list that opens the Skulls screen, which lists all
//! fifteen skulls with what each does, where it is found (and whether only
//! on Legendary), and the pressing player's profile's Locked, On or Off.
//!
//! **The row.** The difficulty screen (screen id 12, loaders `0x2bb3ed` and
//! `0x2bb432`) builds its list handler with `0x2c9c12`: a four-row datum
//! array named "difficulty list" (`0x45d6e0`) through the shared
//! constructor (`0x1a480d`, count in EAX). Its row text `0x2c9ce7` (item at
//! `[esp+4]`, row at `[item+0x70]`, `ret 8`) gives anything past row 3 the
//! null id, and its row chosen `0x2c9d38` (`ret 8`, a pointer to the row at
//! `[esp+8]`, `[[esp+4]+4]` the player index, as every list callback of
//! this kind gets) clamps the row to 0..3, stores the difficulty and starts
//! the campaign. So we grow the list to five, draw row 4 ourselves and, for
//! a press on row 4, open our screen and never run the original. The
//! handler embeds four item widgets, so the list scrolls to the fifth row
//! as lists with more rows than items do. Moving onto row 4 would set pane
//! 4 of a tag that has four (`0x230427`, no bounds check), so the pane
//! change is skipped for it and the previous difficulty's pane stays.
//!
//! **The screen** is our own list class (`crate::list_class`, as the Video
//! screen is), feature `skulls-screen`, showing a [`SkullsScreen`]: fifteen
//! rows in the engine's order, each the skull's name and the game's own On
//! or Off, or our "Locked"; a description text widget of ours with the
//! focused skull's effect and where it is found. The row's press sets the
//! profile ([`SkullsScreen::open`]) and opens it with
//! `0x149f1e(1 << player, loader)`. A press on a collected skull flips it
//! in the store at once ([`skulls::toggle`]); the next campaign start turns
//! on exactly the On ones ([`skulls::selection`]). B returns to the
//! difficulty list.
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

use crate::layout_glue::{Gm, ID_BASE};
use crate::list_class::{Rows, TextBlock};
use crate::skulls::{self, ABOUT, COUNT, SKULLS, State, TOTAL};
use crate::speaker_row;
use crate::ui::{Chosen, LABEL_SLOT, VALUE_SLOT};

/// The difficulty list: its array name, retail rows and our row.
pub const DIFFICULTY_LIST_NAME: u32 = 0x0045_d6e0;
pub const DIFFICULTY_ROWS: u32 = 4;
pub const ROW: i16 = 4;
/// The difficulty screen's id (`screen + 0x70`).
pub const DIFFICULTY_SCREEN_ID: i16 = 12;
/// The difficulty list's row text (`ret 8`) and row chosen (`ret 8`).
pub const DIFFICULTY_ROW_TEXT: u32 = 0x002c_9ce7;
pub const DIFFICULTY_ROW_CHOSEN: u32 = 0x002c_9d38;
/// The screen's list class (`features.skulls-screen` switches it).
pub const FEATURE: &str = "skulls-screen";

/// Our strings: a label per skull, the title (also the row's label), our
/// "Locked" and a description per skull. `layout_glue::is_ours` looks at
/// the middle bytes only, so a base one higher in the top byte than
/// [`ID_BASE`] is ours too and clear of every id the model and the other
/// rows take (`ID_BASE + 0x00..0xff`).
pub const SKULLS_ID_BASE: u32 = ID_BASE + 0x0100_0000;
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
pub const TITLE: &str = "Skulls";
pub const MP_TITLE: &str = "Multiplayer Skulls";
pub const LOCKED: &str = "Locked";

/// The description text: under the picture, as wide as it. Above the list,
/// beside the header, it ran over the header bar (seen in game). Font 1 is
/// the difficulty screen's description font.
/// `H2_SKULLS_TEXT_RECT=top,left,bottom,right` moves it without a rebuild.
pub const DESCRIPTION_TEXT: TextBlock = TextBlock {
    rect: [-112, 56, -230, 312],
    font: 1,
    colour: [0.8, 0.88, 1.0],
};

/// The picture: the difficulty screen's (12) Legendary pane (3), its first
/// bitmap, `difficulty_options` image 3 (a 256 by 256 skull), right of the
/// list with its top-left at (56, 150); the description starts just under it.
pub const PICTURE: crate::list_class::PictureSpec = crate::list_class::PictureSpec {
    screen_id: DIFFICULTY_SCREEN_ID as u32,
    pane: 3,
    bitmap: 0,
    image: 3,
    at: [56, 150],
};

/// The picture's tint for a skull of tier `tier`: gold or silver, and
/// dimmed to a third while `state` is Locked. The art is a pale silver, so
/// silver keeps it nearly as drawn.
pub fn tint(tier: skulls::SkullTier, state: State) -> [f32; 3] {
    let c = match tier {
        skulls::SkullTier::Gold => [1.0, 0.78, 0.3],
        skulls::SkullTier::Silver => [0.86, 0.9, 1.0],
    };
    if state == State::Locked {
        c.map(|v| v * 0.35)
    } else {
        c
    }
}

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

/// The campaign screen's row for skull `i`.
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
    v.push((LOCKED_ID, LOCKED));
    v.push((MP_TITLE_ID, MP_TITLE));
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

/// Whether the menu is on (`H2_NO_SKULLS` or `H2_NO_SKULLS_MENU` take it
/// out) for a store that has the skull settings.
pub fn enabled(store: &Store) -> bool {
    std::env::var_os("H2_NO_SKULLS").is_none()
        && std::env::var_os("H2_NO_SKULLS_MENU").is_none()
        && store.def(SKULLS[0].key).is_some()
}

/// The difficulty list's count when the game builds the array `name` with
/// `retail` rows: one more (the Skulls row), only for that list at its
/// retail size.
pub fn difficulty_count(name: u32, retail: u32) -> u32 {
    if name == DIFFICULTY_LIST_NAME && retail == DIFFICULTY_ROWS {
        retail + 1
    } else {
        retail
    }
}

/// The local player index of a list callback (`[[esp+4]]+4`), as
/// `video_screen::player_mask` reads it.
pub fn player_index<M: Gm>(m: &M, esp: u32) -> Option<u32> {
    let p = m.u32(esp.wrapping_add(4))?;
    let record = m.u32(p)?;
    let index = m.u32(record.wrapping_add(4))?;
    (index < skulls::LOCAL_PLAYERS).then_some(index)
}

/// A pane change on screen `screen_id` for list row `row` that must be
/// skipped: our row on the difficulty screen, which has no pane for it.
pub fn skip_pane(screen_id: i16, row: i16) -> bool {
    screen_id == DIFFICULTY_SCREEN_ID && row == ROW
}

/// The profile whose skulls a press by local player `index` shows: the
/// one signed in on that player's slot, else [`skulls::DEFAULT_PROFILE`]
/// (where a pickup with no named profile is kept).
pub fn profile_of<M: Gm>(m: &M, index: u32) -> Option<String> {
    (index < skulls::LOCAL_PLAYERS).then(|| skulls::profile_or_default(m, index))
}

/// The line the difficulty list's build logs for `profiles` (from
/// [`skulls::profiles_or_default`]): the row is always there, and this
/// says how many skulls each profile has.
pub fn difficulty_line(store: &Store, (names, fallback): &(Vec<String>, Option<String>)) -> String {
    let why = fallback
        .as_ref()
        .map_or(String::new(), |w| format!(" ({w})"));
    let counts: Vec<String> = names
        .iter()
        .map(|p| {
            format!(
                "{p} {} of {TOTAL} collected",
                skulls::collected(store, p).len()
            )
        })
        .collect();
    format!(
        "skulls menu: difficulty list: Skulls row shown; {}{why}",
        counts.join(", ")
    )
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
    /// All fifteen, for the next campaign start (the difficulty list's row).
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
    /// What this instance lists, whoever opens it.
    fixed: Mode,
}

impl SkullsScreen {
    /// The campaign screen.
    pub fn new(store: Arc<Store>) -> Self {
        SkullsScreen {
            store,
            open: Mutex::new(None),
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
        *self.open.lock().unwrap_or_else(|e| e.into_inner()) = Some((profile, self.fixed));
    }
    pub fn profile(&self) -> Option<String> {
        self.open
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|(p, _)| p.clone())
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
    /// Row `row`'s skull: gold ones first, then silver
    /// ([`skulls::by_tier`]).
    fn skull(row: i16) -> Option<usize> {
        usize::try_from(row)
            .ok()
            .and_then(|r| ORDER.get(r).copied())
    }
    /// Skull `i`'s state for the open profile; Locked with none.
    pub fn state(&self, i: usize) -> State {
        self.profile()
            .and_then(|p| skulls::state(&self.store, &p, i))
            .unwrap_or(State::Locked)
    }
    /// A press on row `row`: what to log, `None` for no such row.
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
        let i = Self::skull(row)?;
        let Some(profile) = self.profile() else {
            return Some("skulls menu: no profile for this screen; nothing changed".into());
        };
        press(&self.store, &profile, i)
    }
}

impl Rows for SkullsScreen {
    fn count(&self) -> usize {
        match self.mode() {
            Mode::Campaign => TOTAL,
            Mode::Multiplayer => crate::skulls_mp::MP.len(),
        }
    }
    fn title(&self) -> u32 {
        match self.mode() {
            Mode::Campaign => TITLE_ID,
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
            let value = match self.mp_state(r) {
                State::On => speaker_row::ON_ID,
                State::Off => speaker_row::OFF_ID,
                State::Locked => LOCKED_ID,
            };
            return vec![
                (LABEL_SLOT, label_id(crate::skulls_mp::MP[r].index)),
                (VALUE_SLOT, value),
            ];
        }
        let Some(i) = Self::skull(row) else {
            return Vec::new();
        };
        let value = match self.state(i) {
            State::On => speaker_row::ON_ID,
            State::Off => speaker_row::OFF_ID,
            State::Locked => LOCKED_ID,
        };
        vec![(LABEL_SLOT, label_id(i)), (VALUE_SLOT, value)]
    }
    fn press(&self, row: i16) -> Chosen {
        self.press_line(row)
            .map_or(Chosen::Nothing, Chosen::Stepped)
    }
    fn description(&self, row: i16) -> Option<u32> {
        if self.mode() == Mode::Multiplayer {
            return usize::try_from(row)
                .ok()
                .filter(|r| *r < crate::skulls_mp::MP.len())
                .map(mp_description_id);
        }
        Self::skull(row).map(description_id)
    }
    fn tint(&self, row: i16) -> Option<[f32; 3]> {
        let r = usize::try_from(row).ok()?;
        let (index, state) = match self.mode() {
            Mode::Multiplayer => (crate::skulls_mp::MP.get(r)?.index, self.mp_state(r)),
            Mode::Campaign => {
                let i = Self::skull(row)?;
                (i, self.state(i))
            }
        };
        Some(tint(ABOUT.get(index)?.tier, state))
    }
}

const _: () = assert!(TOTAL as u32 <= LABELS.end - LABELS.start);
const _: () = assert!(TOTAL as u32 <= DESCRIPTIONS.end - DESCRIPTIONS.start);
const _: () = assert!(TOTAL as u32 <= FOUND.end - FOUND.start);
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

    /// Each row as drawn: name and value text.
    fn drawn(s: &SkullsScreen) -> Vec<(String, String)> {
        (0..s.count() as i16)
            .map(|r| {
                let t = s.texts(r);
                assert_eq!(t.len(), 2);
                assert_eq!((t[0].0, t[1].0), (LABEL_SLOT, VALUE_SLOT));
                (text(t[0].1), text(t[1].1))
            })
            .collect()
    }

    #[test]
    fn every_skull_is_listed_gold_first() {
        let s = SkullsScreen::new(store());
        s.open("Chief".into());
        assert_eq!(s.count(), TOTAL);
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
        assert!(s.texts(TOTAL as i16).is_empty());
        assert!(s.texts(-1).is_empty());
        assert_eq!(s.title(), TITLE_ID);
        assert_eq!(text(TITLE_ID), "Skulls");
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
                1 => format!("{:#x}", speaker_row::OFF_ID),
                12 => format!("{:#x}", speaker_row::ON_ID),
                _ => "Locked".into(),
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
            s.press_line(row_of(0).unwrap()).as_deref(),
            Some("skulls menu: no profile for this screen; nothing changed")
        );
        s.open("Chief".into());
        assert_eq!(
            s.press_line(row_of(11).unwrap()).as_deref(),
            Some(
                "skulls menu: Iron is locked for Chief: not collected yet (found on High Charity, Legendary only)"
            )
        );
        assert_eq!(
            s.press_line(row_of(6).unwrap()).as_deref(),
            Some("skulls menu: Blind is locked for Chief: not collected yet (found on Outskirts)")
        );
        assert_eq!(s.state(11), State::Locked, "a locked press changes nothing");
        assert_eq!(skulls::collected(&st, "Chief"), []);
        skulls::record_pickup(&st, &names(&["Chief"]), 11);
        assert_eq!(
            s.press_line(row_of(11).unwrap()).as_deref(),
            Some("skulls: Iron off for Chief")
        );
        assert_eq!(s.state(11), State::Off);
        assert_eq!(
            s.press_line(row_of(11).unwrap()).as_deref(),
            Some("skulls: Iron on for Chief")
        );
        assert_eq!(s.state(11), State::On);
        assert_eq!(s.press_line(TOTAL as i16), None);
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
        s.press_line(row_of(3).unwrap()); // Thunderstorm off
        s.press_line(row_of(9).unwrap()); // Catch: locked, nothing
        let mut want = [false; COUNT];
        want[0] = true;
        want[12] = true;
        assert_eq!(skulls::selection(&st, &names(&["Chief"])), Some(want));
        // Co-op: the other player turns theirs off on their own screen; the
        // start takes the union of both.
        s.open("Arbiter".into());
        s.press_line(row_of(7).unwrap());
        let both = names(&["Chief", "Arbiter"]);
        assert_eq!(skulls::selection(&st, &both), Some(want));
        s.press_line(row_of(7).unwrap());
        want[7] = true;
        assert_eq!(skulls::selection(&st, &both), Some(want));
        // Toggling back restores it.
        s.open("Chief".into());
        s.press_line(row_of(3).unwrap());
        want[3] = true;
        assert_eq!(skulls::selection(&st, &both), Some(want));
    }

    #[test]
    fn each_description_says_what_it_does_where_and_if_legendary() {
        let s = SkullsScreen::new(store());
        for (i, a) in ABOUT.iter().enumerate() {
            let id = s.description(row_of(i).unwrap()).unwrap();
            assert_eq!(id, description_id(i));
            let d = text(id);
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
        assert_eq!(s.description(TOTAL as i16), None);
    }

    #[test]
    fn the_difficulty_list_always_gets_the_row() {
        let st = store();
        assert_eq!(difficulty_count(DIFFICULTY_LIST_NAME, 4), 5);
        assert_eq!(
            difficulty_count(DIFFICULTY_LIST_NAME, 5),
            5,
            "not at its retail size"
        );
        assert_eq!(
            difficulty_count(speaker_row::LIST_NAME, 4),
            4,
            "another list"
        );
        skulls::record_pickup(&st, &names(&["Arbiter"]), 12);
        assert_eq!(
            difficulty_line(&st, &(names(&["Chief", "Arbiter"]), None)),
            "skulls menu: difficulty list: Skulls row shown; Chief 0 of 21 collected, Arbiter 1 of 21 collected"
        );
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
        let line = difficulty_line(&st, &none);
        assert!(line.contains("default 0 of 21 collected"), "{line}");
        assert!(line.contains("no named profile signed in"), "{line}");
        skulls::record_pickup(&st, &none.0, 12);
        let s = SkullsScreen::new(Arc::clone(&st));
        s.open(profile_of(&Fake::default(), 0).unwrap());
        assert_eq!(s.state(12), State::On);
    }

    #[test]
    fn only_our_row_on_the_difficulty_screen_skips_its_pane() {
        assert!(skip_pane(DIFFICULTY_SCREEN_ID, ROW));
        assert!(!skip_pane(DIFFICULTY_SCREEN_ID, 3));
        assert!(!skip_pane(40, ROW));
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
        // Labels and descriptions for all, a found message for each of ours.
        assert_eq!(
            all.len(),
            2 * TOTAL + 3 + crate::skulls_mp::MP.len() + crate::skulls_custom::OURS
        );
        let mut ids: Vec<u32> = all.iter().map(|(i, _)| *i).collect();
        assert!(ids.iter().all(|i| is_ours(*i)));
        assert!(
            ids.iter()
                .all(|i| !crate::video_screen::MODEL_IDS.contains(i))
        );
        assert!(
            ids.iter().all(|i| *i >= ID_BASE + 0x100),
            "clear of the other rows' ids"
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
        assert_eq!(s.count(), TOTAL);
        assert_eq!(
            s.state(6),
            State::On,
            "the pickup turned it on for the campaign"
        );
        assert_eq!(s.title(), TITLE_ID);
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
        // Row 0 is the first gold skull; the last row a silver one.
        let last = (s.count() - 1) as i16;
        assert_eq!(s.tint(0), Some(tint(Gold, State::Locked)));
        assert_eq!(s.tint(last), Some(tint(Silver, State::Locked)));
        skulls::record_pickup(&st, &names(&["Chief"]), ORDER[0]);
        assert_eq!(s.tint(0), Some(tint(Gold, State::On)), "collected and on");
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

    /// The picture sits right of the list and the description under it.
    #[test]
    fn the_description_sits_under_the_picture() {
        let [top, left, bottom, right] = DESCRIPTION_TEXT.rect;
        let picture_bottom = PICTURE.at[1] - 256;
        assert!(top < picture_bottom && bottom < top, "under the picture");
        assert_eq!(left, PICTURE.at[0]);
        assert_eq!(right, PICTURE.at[0] + 256);
        assert!(right <= 320 && bottom >= -240, "on screen");
        assert_eq!((PICTURE.screen_id, PICTURE.pane, PICTURE.image), (12, 3, 3));
    }
}
