//! The Halo 1 / Halo 2 options a game on a Halo 1 map is played with, as
//! rows on a pre-game screen (docs/halo1/multiplayer.md "Halo 1
//! options", docs/halo1/campaign.md §5).
//!
//! - **Campaign:** on the Skulls page between the difficulty and a Halo 1
//!   level (`crate::skulls_screen`), under Start Level and above the
//!   skulls: [`CAMPAIGN`]. A on one opens the value picker.
//! - **Multiplayer:** a "Halo 1 Options..." row on a custom game's Game
//!   Setup dialog, next to the map and game type, while the chosen map is
//!   a converted Halo 1 map (`crate::h1_options_row`), opens the Halo 1
//!   Options page ([`page`]): the shared settings dialog
//!   (`crate::ui_kit::settings`) its doc declares,
//!   `content/ui/screens/h1-options.yaml`, with a row per [`MULTIPLAYER`]
//!   option, each its name and its choice ("Sandbox  Halo 1"). Its header
//!   (icon, name and what the window is for) stays whatever has the
//!   focus; the focused row's help is under the rows, and the value picker
//!   A opens says what the option does and what each choice means
//!   (docs/game/ui-builder.md "The settings dialog").
//!
//! Each row is one setting of the store with two choices, Halo 1's and
//! Halo 2's ([`OptionRow`]). A opens the value picker on either page. Left
//! or right changes the choice directly. The store saves it, and
//! ([`install_switches`]) moves the run's switch at once:
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

use crate::setting_defs::{self, HALO1_ASSETS, HALO1_HUD, HALO1_SANDBOX};
use crate::ui_kit::ids::{self, Family};
use crate::ui_kit::screen::KitScreen;
use crate::ui_kit::settings::Bindings;

/// The Skulls page's option rows' string ids: per row its two labels and
/// its description.
pub const IDS: Family = ids::family("h1 options");
/// The multiplayer page's string ids ([`page`]).
pub const PAGE_IDS: Family = ids::family("h1-options");
/// Rows' strings start here, [`PER_ROW`] ids each.
const ROW_IDS: u32 = 4;
const PER_ROW: u32 = 3;

/// One option: a setting of two choices, Halo 1's then Halo 2's.
pub struct OptionRow {
    pub def: &'static Def,
    /// The row's name, before its value ("Sandbox: Halo 1").
    pub label: &'static str,
    /// What each choice means, Halo 1's then Halo 2's, read after "Halo 1:" and
    /// "Halo 2:"; short, for the five lines of the description pane.
    pub means: [&'static str; 2],
    /// What the value picker A opens says: what the option does, what
    /// each choice means and when it takes effect, in a few short
    /// sentences (`crate::ui_kit::settings::EXPLAIN_UNITS` at most). The
    /// multiplayer page's rows say the same in their doc.
    pub explain: &'static str,
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
    explain: "Which weapons, vehicles and health this map plays with. \
              Halo 1 keeps its own, health packs included. Halo 2 swaps in \
              Halo 2's nearest weapons and vehicles, and its health. \
              From the next game.",
};

pub static ASSETS: OptionRow = OptionRow {
    def: &HALO1_ASSETS,
    label: "Assets",
    means: [
        "its characters, weapons and vehicles.",
        "Halo 2's; cutscenes stay Halo 1's. From the next level.",
    ],
    explain: "Which characters, weapons and vehicles this level uses. \
              Halo 1 keeps its own. Halo 2 swaps in Halo 2's; the \
              cutscenes stay Halo 1's. Changes from the next level.",
};

pub static HUD: OptionRow = OptionRow {
    def: &HALO1_HUD,
    label: "HUD",
    means: [
        "its shield and health meters, motion tracker and ammo counter.",
        "Halo 2's. From the next game.",
    ],
    explain: "Which HUD you play with. Halo 1 shows its own shield and \
              health meters, motion tracker and ammo counter. Halo 2 \
              shows Halo 2's. Changes from the next game.",
};

/// The campaign's options, above the skulls on the Skulls page.
pub static CAMPAIGN: &[&OptionRow] = &[&ASSETS, &HUD];
/// A multiplayer game's options, on the Halo 1 Options page: its doc's
/// rows, in order.
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

/// A choice's name.
pub fn choice(halo1: bool) -> &'static str {
    if halo1 { "Halo 1" } else { "Halo 2" }
}

/// `row`'s label showing a choice: "Assets: Halo 1".
pub fn label(row: &OptionRow, halo1: bool) -> String {
    format!("{}: {}", row.label, choice(halo1))
}

/// `row`'s description: both choices, said plainly.
pub fn description(row: &OptionRow) -> String {
    format!("Halo 1: {} Halo 2: {}", row.means[0], row.means[1])
}

static STRINGS: std::sync::LazyLock<Vec<(u32, String)>> = std::sync::LazyLock::new(|| {
    let mut v = Vec::new();
    for r in ALL {
        v.push((label_id(r, true), label(r, true)));
        v.push((label_id(r, false), label(r, false)));
        v.push((description_id(r), description(r)));
    }
    v
});

/// Every string the Skulls page's option rows show.
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
    let name = if to == setting_defs::HALO1 {
        "Halo 1"
    } else {
        "Halo 2"
    };
    match store.set(row.def.key, Value::Int(to)) {
        Ok(o) if !o.changed => format!(
            "h1 options: {} stays {} (locked by its variable)",
            row.def.key,
            if is_halo1(store, row) {
                "Halo 1"
            } else {
                "Halo 2"
            }
        ),
        Ok(o) => match o.not_saved {
            None => format!("h1 options: {} -> {name}", row.def.key),
            Some(why) => format!("h1 options: {} -> {name}; not saved: {why}", row.def.key),
        },
        Err(e) => format!("h1 options: {}: {e}", row.def.key),
    }
}

/// The value picker's pick for `row`: Halo 1 then Halo 2, the one held
/// focused, both choices' meanings on the right.
pub fn pick(store: &Arc<Store>, row: &'static OptionRow) -> crate::ui_kit::picker::Pick {
    let held = Arc::clone(store);
    let set = Arc::new(move |i: usize| {
        if (i == 0) == is_halo1(&held, row) {
            format!("h1 options: {} stays {}", row.def.key, choice(i == 0))
        } else {
            step(&held, row)
        }
    });
    crate::ui_kit::picker::Pick::new(
        row.label,
        vec![choice(true).to_string(), choice(false).to_string()],
        usize::from(!is_halo1(store, row)),
        row.explain,
        set,
    )
    .over(crate::ui_kit::view::Opener::GAME_SETUP)
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

/// The multiplayer screen's id for `Open` rows and the loader registry.
pub const SCREEN: &str = "h1-options";

/// The Halo 1 Options page over `store`: the shared settings dialog
/// (`crate::ui_kit::settings`) its doc declares,
/// `content/ui/screens/h1-options.yaml`: its header, a row per
/// [`MULTIPLAYER`] option ("Halo 1" or "Halo 2" in its value column), and
/// the Game Setup row's text and help (`settings.caller`). Its class is
/// `features.h1-options-screen` (`crate::h1_options_row::SCREEN_FEATURE`).
pub fn page(store: Arc<Store>) -> KitScreen {
    crate::ui_kit::settings::screen(
        crate::h1_options_row::SCREEN_FEATURE,
        SCREEN,
        PAGE_IDS,
        Some(store),
        Bindings::new(),
    )
    .unwrap_or_else(|e| panic!("{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::list_class::Rows;
    use crate::setting_defs::ALL as DEFS;
    use crate::ui::Chosen;

    fn store_at(dir: &std::path::Path) -> Store {
        Store::load(DEFS, Some(&settings::path_in(dir)), &|_| None, &[]).0
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("h2-h1opt-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn doc() -> Arc<crate::ui_kit::data::ScreenDoc> {
        crate::ui_kit::data::require(SCREEN).unwrap_or_else(|e| panic!("{e}"))
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
        assert_eq!(label(&SANDBOX, true), "Sandbox: Halo 1");
        assert_eq!(label(&HUD, false), "HUD: Halo 2");
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

    /// The multiplayer page is the shared settings dialog: its rows are
    /// [`MULTIPLAYER`]'s, each its name and its choice alone in Quick
    /// Options' two columns, its header the same whatever has the focus,
    /// and the focused row's help under the rows.
    #[test]
    fn the_multiplayer_page_is_the_shared_settings_dialog() {
        use crate::ui_kit::dialog::{DialogBase, SETTINGS_HELP_TEXT, icon, quick_icon};
        use crate::ui_kit::spec::{Describe, ScreenSpec, ValueStyle};
        let d = doc();
        let keys: Vec<&str> = d.rows.values().filter_map(|r| r.setting.as_deref()).collect();
        let options: Vec<&str> = MULTIPLAYER.iter().map(|r| r.def.key).collect();
        assert_eq!(keys, options, "a row per multiplayer option, in order");
        for (r, o) in d.rows.values().zip(MULTIPLAYER) {
            assert_eq!(r.text, o.label);
            assert_eq!(r.explain.as_deref(), Some(o.explain), "{}: the Skulls page says the same", o.label);
        }
        let p = page(Arc::new(Store::defaults(DEFS)));
        assert!(p.slots() <= PAGE_IDS.len);
        let s = p.spec();
        assert_eq!(s.style, ValueStyle::Setting);
        let dialog = s.dialog.as_ref().expect("a dialog");
        assert_eq!(dialog.base, DialogBase::QuickOptions);
        assert_eq!(dialog.rows, s.rows);
        assert_eq!(s.description.map(|b| b.rect), Some(SETTINGS_HELP_TEXT.rect));
        let (label, value) = s.rows[0].cells(s.style);
        assert_eq!((label.as_str(), value.as_deref()), ("Sandbox", Some("Halo 1")));
        assert_eq!(s.help, None, "no page line in place of the rows'");
        assert_eq!(s.art, [quick_icon(icon::HELMET)]);
        assert_eq!(Rows::picture_image(&p), Some(icon::HELMET));
        let texts: Vec<&str> = s.texts.iter().map(|t| t.text.as_str()).collect();
        // The about line is the Game Setup row's help: the two always agree.
        let caller = d.settings.clone().and_then(|s| s.caller).expect("a caller");
        assert_eq!(texts, ["This Halo 1 map", caller.help.as_str()]);
        assert_eq!(
            caller.help,
            "Play this Halo 1 map with Halo 1's weapons and HUD, or Halo 2's."
        );
        for (row, r) in d.rows.values().enumerate() {
            let focused = ScreenSpec {
                focus: row,
                ..s.clone()
            };
            assert_eq!(focused.description_text(), r.help.clone());
            assert_eq!((focused.art.clone(), focused.texts.clone()), (s.art.clone(), s.texts.clone()));
            assert!(Rows::description(&p, row as i16).is_some_and(|id| PAGE_IDS.owns(id)));
        }
        assert_eq!(Rows::description(&p, 2), None);
        // The Game Setup row's text and help are the page's own slots.
        assert_ne!(p.row_label_id(), p.title());
        assert!(p.row_help_id().is_some_and(|id| PAGE_IDS.owns(id)));
    }

    /// Every option has an explanation for the value picker that is
    /// player copy and fits its block.
    #[test]
    fn every_option_has_real_picker_copy() {
        use crate::ui_kit::dialog::PICKER_HELP_TEXT;
        use crate::ui_kit::spec::{fallback, wrap};
        let width = i32::from(PICKER_HELP_TEXT.rect[3]) - i32::from(PICKER_HELP_TEXT.rect[1]);
        let height = i32::from(PICKER_HELP_TEXT.rect[0]) - i32::from(PICKER_HELP_TEXT.rect[2]);
        for r in ALL {
            let e = r.explain;
            assert!(!e.is_empty(), "{}", r.label);
            assert!(e.encode_utf16().count() <= crate::ui_kit::settings::EXPLAIN_UNITS, "{}: {e}", r.label);
            let lines = wrap(e, width, &fallback::width).len() as i32;
            assert!(lines * fallback::LINE <= height, "{}: {lines} lines", r.label);
            let lower = e.to_lowercase();
            for bad in ["docs/", "todo", "placeholder", "lorem", "tbd"] {
                assert!(!lower.contains(bad), "{}: {e}", r.label);
            }
        }
    }

    /// Left and right each step to the other choice, A opens the value
    /// picker whose choice is saved too; a store loaded again has it.
    #[test]
    fn stepping_saves_and_a_new_run_reads_it() {
        use crate::ui_kit::spec::Describe;
        let dir = scratch("step");
        let s = Arc::new(store_at(&dir));
        let p = page(Arc::clone(&s));
        let value = |row: usize| {
            let spec = p.spec();
            spec.rows[row].cells(spec.style).1.unwrap_or_default()
        };
        assert_eq!(value(0), "Halo 1");
        let line = p.sideways(0, true).unwrap();
        assert!(line.contains("multiplayer.halo1_sandbox ->"), "{line}");
        assert_eq!(value(0), "Halo 2");
        // A opens the value picker on HUD: Halo 1 then Halo 2, Halo 1
        // held, its explanation on the right; choosing Halo 2 there saves
        // it.
        assert_eq!(p.press(1), Chosen::Open(crate::ui_kit::picker::NAME));
        let pick = crate::ui_kit::picker::current().expect("the picker's pick");
        assert_eq!(pick.title, "HUD");
        assert_eq!(pick.values, ["Halo 1", "Halo 2"]);
        assert_eq!(pick.current, 0);
        assert_eq!(pick.description, HUD.explain);
        assert!((pick.set)(1).contains("halo1.hud ->"));
        let again = store_at(&dir);
        assert_eq!(setting_defs::halo1_sandbox(&again), "h2");
        assert!(!is_halo1(&again, &HUD));
        p.sideways(0, false).unwrap();
        assert_eq!(value(0), "Halo 1");
        assert_eq!(p.press(2), Chosen::Nothing, "no such row");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The switches follow the store at boot and on every change.
    #[test]
    fn the_switches_follow_each_choice() {
        let _g = crate::hud::TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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
