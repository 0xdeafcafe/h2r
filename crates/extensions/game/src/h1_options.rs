//! The Halo 1 / Halo 2 options a game on a Halo 1 map is played with, as
//! rows on a pre-game screen (docs/halo1/multiplayer.md "Halo 1
//! options", docs/halo1/campaign.md §5).
//!
//! - **Campaign:** on the Game Options page between the difficulty and a
//!   Halo 1 level (`crate::skulls_screen`, the skulls page), above the
//!   skulls: [`CAMPAIGN`].
//! - **Multiplayer:** a "Halo 1 Options" row on a custom game's Game Setup
//!   dialog, next to the map and game type, while the chosen map is a
//!   converted Halo 1 map (`crate::h1_options_row`), opens a screen of
//!   these rows ([`OptionsScreen`]): [`MULTIPLAYER`]. The screen is the
//!   lobby's own settings dialog, Quick Options, inside and out
//!   (`DialogBase::QuickOptions`, docs/game/ui-builder.md "The Dialog
//!   kind"): its list on the `setting_lists` skin, each row's name left
//!   and its choice alone in the value column ("Sandbox  Halo 1"),
//!   Quick Options' header (the focused row's icon, [`OptionRow::icon`],
//!   in its game type picture and the row's one line,
//!   [`OptionRow::help`], in its help line; docs/game/ui-builder.md "A
//!   page's header"), its title bar and its footer. A on a row opens the
//!   value picker, which says what the option does and what each choice
//!   means ([`OptionRow::explain`]).
//!
//! Each row is one setting of the store with two choices, Halo 1's and
//! Halo 2's ([`OptionRow`]); a new option is one more [`OptionRow`] in the
//! lists. A opens the value picker on either page. Left or right changes
//! the choice directly. The store saves it, and ([`install_switches`]) moves
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
use crate::ui::{Chosen, LABEL_SLOT, VALUE_SLOT};
use crate::ui_kit::ids::{self, Family};

/// Our string ids: the screen's title, then per row its two labels and its
/// description.
pub const IDS: Family = ids::family("h1 options");
pub const TITLE_ID: u32 = IDS.id(0);
pub const FOOTER_ID: u32 = IDS.id(1);
/// The Game Setup row that opens the screen: its own text, so the screen's
/// title stays "Halo 1 Options" (`crate::h1_options_row`).
pub const ROW_LABEL_ID: u32 = IDS.id(2);
pub const ROW_LABEL: &str = "Halo 1 Options...";
/// What the Game Setup row does, in Game Setup's description while it has
/// the focus, as retail's rows have theirs (`crate::game_setup_help`).
pub const ROW_HELP_ID: u32 = IDS.id(3);
pub const ROW_HELP: &str =
    "Choose whether this Halo 1 map plays with Halo 1's weapons, vehicles and HUD, or Halo 2's.";
/// Rows' strings start here, [`PER_ROW`] ids each.
const ROW_IDS: u32 = 4;
const PER_ROW: u32 = 3;
/// In capitals, as Quick Options' header ("QUICK OPTIONS") is.
pub const TITLE: &str = "HALO 1 OPTIONS";
/// The options screen's buttons, in the footer as retail's are.
pub const PROMPT: &str = "\u{e10a}  \u{e10b} CHANGE  \u{e100} SELECT  \u{e101} BACK";

/// One option: a setting of two choices, Halo 1's then Halo 2's.
pub struct OptionRow {
    pub def: &'static Def,
    /// The row's name, before its value ("Sandbox: Halo 1").
    pub label: &'static str,
    /// What each choice means, Halo 1's then Halo 2's, read after "Halo 1:" and
    /// "Halo 2:"; short, for the five lines of the description pane.
    pub means: [&'static str; 2],
    /// One line for the multiplayer screen's help line (Quick Options'
    /// one-line `quick_options_help`, [`HELP_UNITS`] at most): what the
    /// option switches, shown while the row has the focus.
    pub help: &'static str,
    /// What the value picker A opens says: what the option does, what
    /// each choice means and when it takes effect, in a few short
    /// sentences ([`EXPLAIN_UNITS`] at most).
    pub explain: &'static str,
    /// The row's icon in Quick Options' game type picture while it has
    /// the focus (`crate::ui_kit::dialog::icon`).
    pub icon: u16,
}

/// The longest picker text: about seven lines of the picker's
/// description block (235 units wide in font 1).
pub const EXPLAIN_UNITS: usize = 200;

/// The longest help line: retail's own ("Temporarily change settings for
/// this game variant.") is 51.
pub const HELP_UNITS: usize = 52;

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
    help: "Weapons, vehicles and health, from the next game.",
    explain: "Which weapons, vehicles and health this map plays with. \
              Halo 1 keeps its own, health packs included. Halo 2 swaps in \
              Halo 2's nearest weapons and vehicles, and its health. \
              From the next game.",
    icon: crate::ui_kit::dialog::icon::BOMB,
};

pub static ASSETS: OptionRow = OptionRow {
    def: &HALO1_ASSETS,
    label: "Assets",
    means: [
        "its characters, weapons and vehicles.",
        "Halo 2's; cutscenes stay Halo 1's. From the next level.",
    ],
    help: "Characters, weapons, vehicles: from the next level.",
    explain: "Which characters, weapons and vehicles this level uses. \
              Halo 1 keeps its own. Halo 2 swaps in Halo 2's; the \
              cutscenes stay Halo 1's. Changes from the next level.",
    icon: crate::ui_kit::dialog::icon::SPARTAN,
};

pub static HUD: OptionRow = OptionRow {
    def: &HALO1_HUD,
    label: "HUD",
    means: [
        "its shield and health meters, motion tracker and ammo counter.",
        "Halo 2's. From the next game.",
    ],
    help: "Shields, health, motion tracker and ammo counter.",
    explain: "Which HUD you play with. Halo 1 shows its own shield and \
              health meters, motion tracker and ammo counter. Halo 2 \
              shows Halo 2's. Changes from the next game.",
    icon: crate::ui_kit::dialog::icon::HELMET,
};

/// The campaign's options, above the skulls on the Game Options page.
pub static CAMPAIGN: &[&OptionRow] = &[&ASSETS, &HUD];
/// A multiplayer game's options, on the Halo 1 Options screen.
pub static MULTIPLAYER: &[&OptionRow] = &[&SANDBOX, &HUD];
/// Every row, each once: the order its strings are numbered in.
pub static ALL: &[&OptionRow] = &[&SANDBOX, &ASSETS, &HUD];

/// The multiplayer screen's strings, after the rows': the two choices
/// (its value column), then each row's name and each row's help line.
const MP_IDS: u32 = ROW_IDS + PER_ROW * 3;
const _: () = assert!(MP_IDS + 2 + 2 * 3 <= IDS.len);

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

/// The multiplayer screen's value column: the choice, "Halo 1" or
/// "Halo 2".
pub fn choice_id(halo1: bool) -> u32 {
    IDS.base + MP_IDS + u32::from(!halo1)
}

/// `row`'s name alone ("Sandbox"), the multiplayer screen's label column.
pub fn name_id(row: &OptionRow) -> u32 {
    IDS.base + MP_IDS + 2 + index(row)
}

/// `row`'s help line, the multiplayer screen's description.
pub fn help_id(row: &OptionRow) -> u32 {
    IDS.base + MP_IDS + 2 + ALL.len() as u32 + index(row)
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
    let mut v = vec![
        (TITLE_ID, TITLE.to_string()),
        (FOOTER_ID, PROMPT.to_string()),
        (ROW_LABEL_ID, ROW_LABEL.to_string()),
        (ROW_HELP_ID, ROW_HELP.to_string()),
        (choice_id(true), choice(true).to_string()),
        (choice_id(false), choice(false).to_string()),
    ];
    for r in ALL {
        v.push((label_id(r, true), label(r, true)));
        v.push((label_id(r, false), label(r, false)));
        v.push((description_id(r), description(r)));
        v.push((name_id(r), r.label.to_string()));
        v.push((help_id(r), r.help.to_string()));
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

/// The multiplayer Halo 1 Options screen: [`MULTIPLAYER`]'s rows, each its
/// name and its choice in Quick Options' two columns.
pub struct OptionsScreen {
    pub store: Arc<Store>,
}

impl Rows for OptionsScreen {
    fn content_spec(&self) -> Option<crate::ui_kit::spec::ScreenSpec> {
        Some(crate::ui_kit::spec::Describe::spec(self))
    }

    fn title(&self) -> u32 {
        TITLE_ID
    }
    fn count(&self) -> usize {
        MULTIPLAYER.len()
    }
    fn texts(&self, row: i16) -> Vec<(u32, u32)> {
        option_at(MULTIPLAYER, row)
            .map(|r| {
                vec![
                    (LABEL_SLOT, name_id(r)),
                    (VALUE_SLOT, choice_id(is_halo1(&self.store, r))),
                ]
            })
            .unwrap_or_default()
    }
    /// A opens the value picker on the row's two choices, as the game's
    /// own settings rows open theirs (`crate::ui_kit::picker`).
    fn press(&self, row: i16) -> Chosen {
        let Some(r) = option_at(MULTIPLAYER, row) else {
            return Chosen::Nothing;
        };
        match crate::ui_kit::picker::offer(pick(&self.store, r)) {
            Some(picker) => Chosen::Open(picker),
            None => Chosen::Stepped(step(&self.store, r)),
        }
    }
    fn sideways(&self, row: i16, right: bool) -> Option<String> {
        let r = option_at(MULTIPLAYER, row)?;
        Some(format!(
            "h1 options: {} on row {row}: {}",
            if right { "right" } else { "left" },
            step(&self.store, r)
        ))
    }
    /// The focused row's one line ([`OptionRow::help`]) in Quick
    /// Options' help line; the value picker says more.
    fn description(&self, row: i16) -> Option<u32> {
        option_at(MULTIPLAYER, row).map(help_id)
    }
    /// The first row's icon: the focus starts there.
    fn picture_image(&self) -> Option<u16> {
        self.row_image(0)
    }
    /// The focused row's icon in Quick Options' game type picture.
    fn row_image(&self, row: i16) -> Option<u16> {
        DIALOG.picture()?;
        option_at(MULTIPLAYER, row).map(|r| r.icon)
    }
    fn describe(&self, count: usize) -> Option<String> {
        let rows: Vec<String> = MULTIPLAYER
            .iter()
            .take(count)
            .enumerate()
            .map(|(i, r)| {
                format!(
                    "[{i}] \"{}\" = \"{}\"",
                    r.label,
                    choice(is_halo1(&self.store, r))
                )
            })
            .collect();
        Some(format!("\"{TITLE}\"; rows: {}", rows.join(" ")))
    }
}

impl crate::ui_kit::spec::Describe for OptionsScreen {
    fn spec(&self) -> crate::ui_kit::spec::ScreenSpec {
        use crate::ui_kit::dialog::DialogSpec;
        use crate::ui_kit::input::{Glyph, Input, Prompt};
        use crate::ui_kit::spec::{ListPlace, Press, RowSpec, ScreenSpec};
        let base = DIALOG;
        let mut s = ScreenSpec::new(SCREEN, TITLE);
        s.ids = vec![TITLE_ID, FOOTER_ID];
        s.list = ListPlace::from(Some(crate::ui_kit::dialog_tag::base_layout(
            base,
            MULTIPLAYER.len(),
        )));
        s.style = base.style();
        s.description = base.help().map(Into::into);
        s.content.row_help = true;
        s.footer = true;
        s.prompts = vec![
            Prompt::new(&[Glyph::DpadLeft, Glyph::DpadRight], "Change"),
            Prompt::new(&[Glyph::A], "Select"),
            Prompt::new(&[Glyph::B], "Back"),
        ];
        s.inputs = vec![
            Input::Up,
            Input::Down,
            Input::Left,
            Input::Right,
            Input::A,
            Input::B,
        ];
        for (row, r) in MULTIPLAYER.iter().enumerate() {
            let halo1 = is_halo1(&self.store, r);
            let mut ids: Vec<u32> = self
                .texts(row as i16)
                .into_iter()
                .map(|(_, id)| id)
                .collect();
            ids.push(help_id(r));
            let mut spec =
                RowSpec::value(r.label, &[choice(true), choice(false)], usize::from(!halo1))
                    .press(Press::Open(crate::ui_kit::picker::NAME.into()))
                    .describe(r.help)
                    .ids(&ids);
            if base.picture().is_some() {
                spec = spec.art(crate::ui_kit::dialog::quick_icon(r.icon));
            }
            s.rows.push(spec);
        }
        s.dialog = Some(DialogSpec {
            base,
            rows: s.rows.clone(),
            ..DialogSpec::new("", crate::ui_kit::view::Opener::GAME_SETUP)
        });
        s
    }
}

/// The dialog the multiplayer screen is: the lobby's Quick Options.
pub const DIALOG: crate::ui_kit::dialog::DialogBase =
    crate::ui_kit::dialog::DialogBase::QuickOptions;

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
        // The Game Setup row says "...Options..." as its neighbours do; the
        // screen's title keeps its name.
        let text = |id| v.iter().find(|(i, _)| *i == id).map(|(_, t)| *t);
        assert_eq!(text(ROW_LABEL_ID), Some("Halo 1 Options..."));
        assert_eq!(text(TITLE_ID), Some("HALO 1 OPTIONS"));
        // The multiplayer screen's columns and help lines.
        assert_eq!(text(name_id(&SANDBOX)), Some("Sandbox"));
        assert_eq!(text(choice_id(true)), Some("Halo 1"));
        assert_eq!(text(choice_id(false)), Some("Halo 2"));
        assert_eq!(text(help_id(&HUD)), Some(HUD.help));
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

    /// The multiplayer screen is Quick Options inside: each row its name
    /// and its choice alone in the two columns, its help one line as
    /// retail's is, and the whole of it a dialog on Quick Options.
    #[test]
    fn the_multiplayer_screen_is_quick_options_inside() {
        use crate::ui_kit::spec::{Describe, ValueStyle};
        for r in ALL {
            assert!(!r.help.is_empty(), "{}", r.label);
            assert!(
                r.help.encode_utf16().count() <= HELP_UNITS,
                "{}: {:?} is more than Quick Options' one line",
                r.label,
                r.help
            );
            assert!(!r.help.contains('\n'), "{}", r.label);
        }
        let screen = OptionsScreen {
            store: Arc::new(Store::defaults(DEFS)),
        };
        let s = screen.spec();
        assert_eq!(s.style, ValueStyle::Setting);
        let d = s.dialog.as_ref().expect("a dialog");
        assert_eq!(d.base, DIALOG);
        assert_eq!(d.base, crate::ui_kit::dialog::DialogBase::QuickOptions);
        assert_eq!(d.rows, s.rows);
        assert_eq!(
            s.description,
            Some(crate::ui_kit::dialog::QUICK_HELP_TEXT.into())
        );
        let (label, value) = s.rows[0].cells(s.style);
        assert_eq!(
            (label.as_str(), value.as_deref()),
            ("Sandbox", Some("Halo 1"))
        );
        assert_eq!(s.rows[1].description.as_deref(), Some(HUD.help));
        // Quick Options' header: the focused row's one line in the help
        // line and its icon in the game type picture, following the focus.
        assert_eq!(screen.description(0), Some(help_id(&SANDBOX)));
        assert_eq!(screen.description(1), Some(help_id(&HUD)));
        assert_eq!(screen.description(2), None);
        assert_eq!(s.help, None, "no page line in place of the rows'");
        assert!(s.art.is_empty(), "the icon is the focused row's");
        let text = strings();
        let text = |id| text.iter().find(|(i, _)| *i == id).map(|(_, t)| *t);
        let room = i32::from(crate::ui_kit::dialog::QUICK_HELP.right)
            - i32::from(crate::ui_kit::dialog::QUICK_HELP.left);
        for (row, r) in MULTIPLAYER.iter().enumerate() {
            let focused = crate::ui_kit::spec::ScreenSpec {
                focus: row,
                ..s.clone()
            };
            assert_eq!(focused.description_text().as_deref(), Some(r.help));
            assert_eq!(text(help_id(r)), Some(r.help));
            assert_eq!(
                focused.shown_art(),
                [crate::ui_kit::dialog::quick_icon(r.icon)],
                "{}",
                r.label
            );
            assert_eq!(screen.row_image(row as i16), Some(r.icon));
            // One line: the kit's own measure (7.5 units a character)
            // inside Quick Options' 494-unit help line.
            assert!(crate::ui_kit::spec::fallback::width(r.help) <= room, "{}", r.label);
        }
        assert_eq!(screen.row_image(MULTIPLAYER.len() as i16), None);
        assert_eq!(screen.picture_image(), Some(MULTIPLAYER[0].icon));
    }

    /// Every option, on either page, has an icon of Quick Options'
    /// picture, a one-line help, and an explanation for the value picker
    /// that says more than the help and is player copy, not a note.
    #[test]
    fn every_option_has_an_icon_a_line_and_real_picker_copy() {
        use crate::ui_kit::dialog::{PICKER_HELP_TEXT, icon};
        use crate::ui_kit::spec::{fallback, wrap};
        let width = i32::from(PICKER_HELP_TEXT.rect[3]) - i32::from(PICKER_HELP_TEXT.rect[1]);
        let height = i32::from(PICKER_HELP_TEXT.rect[0]) - i32::from(PICKER_HELP_TEXT.rect[2]);
        for r in ALL {
            assert!(r.icon < icon::COUNT, "{}", r.label);
            assert!(!r.help.is_empty() && r.help.encode_utf16().count() <= HELP_UNITS);
            let e = r.explain;
            assert!(!e.is_empty() && e != r.help, "{}", r.label);
            assert!(e.encode_utf16().count() <= EXPLAIN_UNITS, "{}: {e}", r.label);
            let lines = wrap(e, width, &fallback::width).len() as i32;
            assert!(lines * fallback::LINE <= height, "{}: {lines} lines", r.label);
            let lower = e.to_lowercase();
            for bad in ["docs/", "todo", "placeholder", "lorem", "tbd"] {
                assert!(!lower.contains(bad), "{}: {e}", r.label);
            }
        }
        // Rows of one page show different icons.
        assert_ne!(SANDBOX.icon, HUD.icon);
        assert_ne!(ASSETS.icon, HUD.icon);
    }

    /// Left and right each step to the other choice, A opens the value
    /// picker whose choice is saved too; a store loaded again has it.
    #[test]
    fn stepping_saves_and_a_new_run_reads_it() {
        let dir = scratch("step");
        let s = Arc::new(store_at(&dir));
        let screen = OptionsScreen {
            store: Arc::clone(&s),
        };
        assert_eq!(
            screen.texts(0),
            [
                (LABEL_SLOT, name_id(&SANDBOX)),
                (VALUE_SLOT, choice_id(true))
            ]
        );
        let line = screen.sideways(0, true).unwrap();
        assert!(
            line.contains("multiplayer.halo1_sandbox -> Halo 2"),
            "{line}"
        );
        assert_eq!(
            screen.texts(0),
            [
                (LABEL_SLOT, name_id(&SANDBOX)),
                (VALUE_SLOT, choice_id(false))
            ]
        );
        // A opens the value picker on HUD: Halo 1 then Halo 2, Halo 1
        // held; choosing Halo 2 there saves it.
        assert_eq!(screen.press(1), Chosen::Open(crate::ui_kit::picker::NAME));
        let p = crate::ui_kit::picker::current().expect("the picker's pick");
        assert_eq!(p.title, "HUD");
        assert_eq!(p.values, ["Halo 1", "Halo 2"]);
        assert_eq!(p.current, 0);
        assert_eq!(p.description, HUD.explain);
        assert!(
            (p.set)(0).contains("stays Halo 1"),
            "the held one: nothing changes"
        );
        assert!((p.set)(1).contains("halo1.hud -> Halo 2"));
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
