//! The Halo 1 / Halo 2 options a game on a Halo 1 map is played with, as
//! rows on a pre-game screen (docs/game/h1-multiplayer.md "Halo 1
//! options", docs/game/campaigns.md §5).
//!
//! - **Campaign:** on the Game Options page between the difficulty and a
//!   Halo 1 level (`crate::skulls_screen`, the skulls page), above the
//!   skulls: [`CAMPAIGN`].
//! - **Multiplayer:** a "Halo 1 Options" row on a custom game's Game Setup
//!   dialog, next to the map and game type, while the chosen map is a
//!   converted Halo 1 map (`crate::h1_options_row`), opens a screen of
//!   these rows ([`OptionsScreen`]): [`MULTIPLAYER`].
//!
//! Each row is one setting of the store with two choices, Halo 1's and
//! Halo 2's ([`OptionRow`]); a new option is one more [`OptionRow`] in the
//! lists. Left or right (or A) steps it, the store saves it like any other
//! setting, and the setting's change callback ([`install_switches`]) moves
//! the run's switch at once:
//!
//! | Setting | Switch | Takes effect |
//! |---|---|---|
//! | `multiplayer.halo1_sandbox` | `crate::h1_variants::set_mp_sandbox` | the map's next load |
//! | `campaign.halo1_assets` | `crate::h1_variants::set_campaign_assets` | the level's next load |
//! | `halo1.hud` | `crate::hud::set_choice` | the next game start |
//!
//! None of these is in Settings any more: the Video screen's "Halo 1 Maps"
//! row is gone, since the choice is made per game here.

use std::sync::Arc;

use settings::{Def, Store, Value};

use crate::list_class::Rows;
use crate::setting_defs::{self, HALO1_ASSETS, HALO1_HUD, HALO1_SANDBOX};
use crate::ui::{Chosen, LABEL_SLOT};
use crate::ui_kit::ids::{self, Family};

/// Our string ids: the screen's title, then per row its two labels and its
/// description.
pub const IDS: Family = ids::family("h1 options");
pub const TITLE_ID: u32 = IDS.id(0);
pub const FOOTER_ID: u32 = IDS.id(1);
/// Rows' strings start here, [`PER_ROW`] ids each.
const ROW_IDS: u32 = 4;
const PER_ROW: u32 = 3;
pub const TITLE: &str = "Halo 1 Options";
/// The options screen's buttons, in the footer as retail's are.
pub const PROMPT: &str = "\u{e10a}  \u{e10b} CHANGE  \u{e101} BACK";

/// One option: a setting of two choices, Halo 1's then Halo 2's.
pub struct OptionRow {
    pub def: &'static Def,
    /// The row's name, before its value ("Sandbox: < Halo 1 >").
    pub label: &'static str,
    /// What each choice means, Halo 1's then Halo 2's, read after "Halo 1:" and
    /// "Halo 2:"; short, for the five lines of the description pane.
    pub means: [&'static str; 2],
}

/// An option is its setting.
impl PartialEq for OptionRow {
    fn eq(&self, other: &Self) -> bool {
        self.def.key == other.def.key
    }
}
impl Eq for OptionRow {}
impl std::fmt::Debug for OptionRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "OptionRow({})", self.def.key)
    }
}

pub static SANDBOX: OptionRow = OptionRow {
    def: &HALO1_SANDBOX,
    label: "Sandbox",
    means: [
        "its weapons, vehicles and health packs.",
        "Halo 2's nearest ones, and its health. From the next game.",
    ],
};

pub static ASSETS: OptionRow = OptionRow {
    def: &HALO1_ASSETS,
    label: "Assets",
    means: [
        "its characters, weapons and vehicles.",
        "Halo 2's; cutscenes stay Halo 1's. From the next level.",
    ],
};

pub static HUD: OptionRow = OptionRow {
    def: &HALO1_HUD,
    label: "HUD",
    means: [
        "its shield and health meters, motion tracker and ammo counter.",
        "Halo 2's. From the next game.",
    ],
};

/// The campaign's options, above the skulls on the Game Options page.
pub static CAMPAIGN: &[&OptionRow] = &[&ASSETS, &HUD];
/// A multiplayer game's options, on the Halo 1 Options screen.
pub static MULTIPLAYER: &[&OptionRow] = &[&SANDBOX, &HUD];
/// Every row, each once: the order its strings are numbered in.
pub static ALL: &[&OptionRow] = &[&SANDBOX, &ASSETS, &HUD];

const _: () = assert!(ROW_IDS + PER_ROW * 3 <= IDS.len);

fn index(row: &OptionRow) -> u32 {
    ALL.iter()
        .position(|r| std::ptr::eq(*r, row))
        .expect("every option is in ALL") as u32
}

/// The label id of `row` showing its choice (`halo1` or not).
pub fn label_id(row: &OptionRow, halo1: bool) -> u32 {
    IDS.base + ROW_IDS + PER_ROW * index(row) + u32::from(!halo1)
}

/// The description id of `row`.
pub fn description_id(row: &OptionRow) -> u32 {
    IDS.base + ROW_IDS + PER_ROW * index(row) + 2
}

/// `row`'s label showing a choice: "Sandbox: < Halo 1 >".
pub fn label(row: &OptionRow, halo1: bool) -> String {
    format!("{}: < {} >", row.label, if halo1 { "Halo 1" } else { "Halo 2" })
}

/// `row`'s description: both choices, said plainly.
pub fn description(row: &OptionRow) -> String {
    format!("Halo 1: {} Halo 2: {}", row.means[0], row.means[1])
}

static STRINGS: std::sync::LazyLock<Vec<(u32, String)>> = std::sync::LazyLock::new(|| {
    let mut v = vec![
        (TITLE_ID, TITLE.to_string()),
        (FOOTER_ID, PROMPT.to_string()),
    ];
    for r in ALL {
        v.push((label_id(r, true), label(r, true)));
        v.push((label_id(r, false), label(r, false)));
        v.push((description_id(r), description(r)));
    }
    v
});

/// Every string we serve.
pub fn strings() -> Vec<(u32, &'static str)> {
    STRINGS.iter().map(|(i, t)| (*i, t.as_str())).collect()
}

/// Whether `row` is on Halo 1's choice in `store`.
pub fn is_halo1(store: &Store, row: &OptionRow) -> bool {
    setting_defs::is_halo1(store, row.def)
}

/// Steps `row` to its other choice (either direction: two choices), saved
/// at once; what to log.
pub fn step(store: &Store, row: &OptionRow) -> String {
    let to = if is_halo1(store, row) {
        setting_defs::HALO2
    } else {
        setting_defs::HALO1
    };
    let name = if to == setting_defs::HALO1 { "Halo 1" } else { "Halo 2" };
    match store.set(row.def.key, Value::Int(to)) {
        Ok(o) if !o.changed => format!(
            "h1 options: {} stays {} (locked by its variable)",
            row.def.key,
            if is_halo1(store, row) { "Halo 1" } else { "Halo 2" }
        ),
        Ok(o) => match o.not_saved {
            None => format!("h1 options: {} -> {name}", row.def.key),
            Some(why) => format!("h1 options: {} -> {name}; not saved: {why}", row.def.key),
        },
        Err(e) => format!("h1 options: {}: {e}", row.def.key),
    }
}

/// Moves the run's switch for `def` to `halo1`.
fn switch(def: &Def, halo1: bool) {
    if def.key == HALO1_SANDBOX.key {
        crate::h1_variants::set_mp_sandbox(halo1);
    } else if def.key == HALO1_ASSETS.key {
        crate::h1_variants::set_campaign_assets(halo1);
    } else if def.key == HALO1_HUD.key {
        crate::hud::set_choice(halo1);
    }
}

/// Sets every switch from `store` now, and again each time a choice
/// changes. Once, at boot.
pub fn install_switches(store: &Store) {
    for r in ALL {
        let def = r.def;
        switch(def, is_halo1(store, r));
        store.on_change(def.key, move |v| {
            switch(def, !matches!(v, Value::Int(setting_defs::HALO2)));
        });
    }
}

/// The rows' part of a screen that shows `rows` first: row `row`'s option.
pub fn option_at(rows: &[&'static OptionRow], row: i16) -> Option<&'static OptionRow> {
    usize::try_from(row).ok().and_then(|r| rows.get(r).copied())
}

/// The multiplayer Halo 1 Options screen: [`MULTIPLAYER`]'s rows.
pub struct OptionsScreen {
    pub store: Arc<Store>,
}

impl Rows for OptionsScreen {
    fn title(&self) -> u32 {
        TITLE_ID
    }
    fn count(&self) -> usize {
        MULTIPLAYER.len()
    }
    fn texts(&self, row: i16) -> Vec<(u32, u32)> {
        option_at(MULTIPLAYER, row)
            .map(|r| vec![(LABEL_SLOT, label_id(r, is_halo1(&self.store, r)))])
            .unwrap_or_default()
    }
    /// A steps the row, as on the game's own settings lists.
    fn press(&self, row: i16) -> Chosen {
        option_at(MULTIPLAYER, row)
            .map_or(Chosen::Nothing, |r| Chosen::Stepped(step(&self.store, r)))
    }
    fn sideways(&self, row: i16, right: bool) -> Option<String> {
        let r = option_at(MULTIPLAYER, row)?;
        Some(format!(
            "h1 options: {} on row {row}: {}",
            if right { "right" } else { "left" },
            step(&self.store, r)
        ))
    }
    fn description(&self, row: i16) -> Option<u32> {
        option_at(MULTIPLAYER, row).map(description_id)
    }
    fn describe(&self, count: usize) -> Option<String> {
        let rows: Vec<String> = MULTIPLAYER
            .iter()
            .take(count)
            .enumerate()
            .map(|(i, r)| format!("[{i}] \"{}\"", label(r, is_halo1(&self.store, r))))
            .collect();
        Some(format!("\"{TITLE}\"; rows: {}", rows.join(" ")))
    }
}

impl crate::ui_kit::spec::Describe for OptionsScreen {
    fn spec(&self) -> crate::ui_kit::spec::ScreenSpec {
        use crate::ui_kit::input::{Glyph, Input, Prompt};
        use crate::ui_kit::spec::{ListPlace, Press, RowSpec, ScreenSpec, ValueStyle};
        let mut s = ScreenSpec::new(SCREEN, TITLE);
        s.ids = vec![TITLE_ID, FOOTER_ID];
        s.list = ListPlace::from(Some(crate::skulls_menu::LIST_LAYOUT));
        s.style = ValueStyle::Inline;
        s.description = Some(crate::skulls_menu::DESCRIPTION_TEXT.into());
        s.footer = true;
        s.prompts = vec![
            Prompt::new(&[Glyph::DpadLeft, Glyph::DpadRight], "Change"),
            Prompt::new(&[Glyph::B], "Back"),
        ];
        s.inputs = vec![Input::Up, Input::Down, Input::Left, Input::Right, Input::A, Input::B];
        for (row, r) in MULTIPLAYER.iter().enumerate() {
            let halo1 = is_halo1(&self.store, r);
            let mut ids: Vec<u32> = self.texts(row as i16).into_iter().map(|(_, id)| id).collect();
            ids.push(description_id(r));
            s.rows.push(
                RowSpec::value(r.label, &["Halo 1", "Halo 2"], usize::from(!halo1))
                    .press(Press::Step)
                    .describe(&description(r))
                    .ids(&ids),
            );
        }
        s
    }
}

/// The multiplayer screen's id for `Open` rows and the loader registry.
pub const SCREEN: &str = "h1-options";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::setting_defs::ALL as DEFS;

    fn store_at(dir: &std::path::Path) -> Store {
        Store::load(DEFS, Some(&settings::path_in(dir)), &|_| None, &[]).0
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("h2-h1opt-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn each_screen_lists_its_options_halo1_first_by_default() {
        let s = Store::defaults(DEFS);
        let names: Vec<&str> = MULTIPLAYER.iter().map(|r| r.label).collect();
        assert_eq!(names, ["Sandbox", "HUD"]);
        let names: Vec<&str> = CAMPAIGN.iter().map(|r| r.label).collect();
        assert_eq!(names, ["Assets", "HUD"]);
        for r in ALL {
            assert!(is_halo1(&s, r), "{}", r.label);
        }
        assert_eq!(label(&SANDBOX, true), "Sandbox: < Halo 1 >");
        assert_eq!(label(&HUD, false), "HUD: < Halo 2 >");
    }

    #[test]
    fn strings_are_distinct_ours_and_in_the_family() {
        let v = strings();
        let mut ids: Vec<u32> = v.iter().map(|(i, _)| *i).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), v.len());
        assert!(ids.iter().all(|i| IDS.owns(*i)));
        assert!(ids.iter().all(|i| crate::layout_glue::is_ours(*i)));
        assert_ne!(label_id(&SANDBOX, true), label_id(&SANDBOX, false));
        assert_ne!(label_id(&SANDBOX, false), label_id(&ASSETS, true));
        // A description fits the pane's five lines (the kit's disc check
        // measures it; a skull's, at most 140, does).
        for r in ALL {
            let d = description(r);
            assert!(d.encode_utf16().count() <= 120, "{d}");
        }
        assert_eq!(
            description(&HUD),
            "Halo 1: its shield and health meters, motion tracker and ammo counter. \
             Halo 2: Halo 2's. From the next game."
        );
    }

    /// Left, right and A each step to the other choice; it is saved, and a
    /// store loaded again has it.
    #[test]
    fn stepping_saves_and_a_new_run_reads_it() {
        let dir = scratch("step");
        let s = Arc::new(store_at(&dir));
        let screen = OptionsScreen {
            store: Arc::clone(&s),
        };
        assert_eq!(screen.texts(0), [(LABEL_SLOT, label_id(&SANDBOX, true))]);
        let line = screen.sideways(0, true).unwrap();
        assert!(line.contains("multiplayer.halo1_sandbox -> Halo 2"), "{line}");
        assert_eq!(screen.texts(0), [(LABEL_SLOT, label_id(&SANDBOX, false))]);
        assert!(matches!(screen.press(1), Chosen::Stepped(l) if l.contains("halo1.hud -> Halo 2")));
        let again = store_at(&dir);
        assert_eq!(setting_defs::halo1_sandbox(&again), "h2");
        assert!(!is_halo1(&again, &HUD));
        assert!(screen.sideways(0, false).unwrap().contains("-> Halo 1"));
        assert_eq!(screen.press(2), Chosen::Nothing, "no such row");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The switches follow the store at boot and on every change.
    #[test]
    fn the_switches_follow_each_choice() {
        let _g = crate::hud::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = scratch("switch");
        let s = store_at(&dir);
        install_switches(&s);
        assert!(crate::hud::halo1_chosen());
        assert!(crate::h1_variants::mp_sandbox_halo1());
        step(&s, &HUD);
        assert!(!crate::hud::halo1_chosen());
        step(&s, &SANDBOX);
        assert!(!crate::h1_variants::mp_sandbox_halo1());
        step(&s, &ASSETS);
        assert!(!crate::h1_variants::campaign_assets_halo1());
        for r in ALL {
            step(&s, r);
        }
        assert!(crate::hud::halo1_chosen());
        assert!(crate::h1_variants::mp_sandbox_halo1());
        assert!(crate::h1_variants::campaign_assets_halo1());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_variable_locks_a_choice_and_the_row_says_so() {
        let env = |k: &str| (k == "H2_HALO1_HUD").then(|| "halo2".to_string());
        let s = Store::load(DEFS, None, &env, &[]).0;
        assert!(!is_halo1(&s, &HUD));
        let line = step(&s, &HUD);
        assert!(line.contains("stays Halo 2"), "{line}");
    }
}
