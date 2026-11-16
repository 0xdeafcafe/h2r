//! Our "Video" screen: a "Video" row on Halo 2's top-level Settings list
//! (after Player Profiles and Game Variants) that opens a Controls screen
//! holding our video rows (Field of View, Frame Rate, Resolution,
//! Lighting, Mute Button, Aim Assist: every setting whose `Def` names this screen, through the `ui`
//! model) instead of the retail six. See docs/game/settings-menu.md "Video
//! screen" for the trace.
//!
//! **The row.** The Settings list (screen id 19, list handler `0x2b1cca` at
//! `screen + 0x614`, vtable `0x45b060`) builds its datum array named
//! "settings list" with two rows and reports `2 + (byte [handler+0x220] !=
//! 0)` item widgets (slot 19, `0x2b1d60`); the constructor clears that byte
//! just before the array is made and allocates room for three. The list
//! hook makes the array three rows and sets the byte, so a third widget is
//! built. Its text (`0x2b1d6e`) and press (`0x2b1da9`) are wrapped like
//! the Controls ones; the press opens the Video screen the way the Settings
//! screen opens Controls. The tag has a pane per retail row only, so the
//! pane hook keeps the shown pane for row 2 (as the Skulls row does).
//!
//! **The screen** is our own list class (`crate::list_class`, docs/formats/ui.md
//! "Screen and list classes"): the row calls `0x149f1e(player mask,
//! loader)` with the class's loader, an entry of ours. It builds the screen
//! with the Controls constructor (so the Controls tag's layout), then points
//! the screen and its list handler at our vtable copies, whose row text,
//! row chosen, pane set-up (our title on the header) and loader slots are
//! Rust. No retail function is patched for it; the Controls hooks never see
//! it. The new screen sits on top of Settings like Player Profiles does,
//! and B returns to it.
//!
//! The Settings row's hooks are registered by `layout_glue::install`; the
//! logic is here, over [`Gm`].

use std::ops::Range;
use std::sync::Arc;

use settings::Store;

use crate::layout_glue::{Gm, ID_BASE};
use crate::setting_defs::{
    FOV, PAD_SPEAKER, STEAM_LEFT_PAD, STEAM_SENSITIVITY, STEAM_SPEAKER, TOUCH_ACCEL,
    TOUCH_AUTO_FIRE, TOUCH_CROUCH, TOUCH_GYRO, TOUCH_LOOK,
};
use crate::speaker_row;
use crate::ui::{self, List, Model, Row, Screen};

/// The Controls screen loader (stdcall, the request pointer, `ret 4`).
pub const CONTROLS_LOADER: u32 = 0x002b_7714;
/// Build a screen request for (player mask, loader) and call the loader
/// with it (stdcall, `ret 8`).
pub const OPEN_SCREEN: u32 = 0x0014_9f1e;
/// Build a screen's header text (stdcall, the screen, `ret 4`).
pub const HEADER_BUILD: u32 = 0x0022_fda6;
/// The Controls screen tag's header string id.
pub const CONTROLS_HEADER: u32 = 0x0600_0233;
/// Where a Controls screen keeps its list handler.
pub const SCREEN_LIST: u32 = 0x614;

/// Our screen's id, and its list in the model.
pub const SCREEN: &str = "video";
pub const LIST: List = List::Screen(SCREEN);
/// The string ids the model's texts take: past the layouts' (`0x00` to
/// `0x1f`) and the small Controls list's (`0x30`, `0x31`), short of the list
/// name (`0xff`).
pub const MODEL_IDS: Range<u32> = ID_BASE + 0x40..ID_BASE + 0xff;

/// Which of our rows are on: `H2_NO_SPEAKER_ROW`, `H2_NO_STEAM_ROWS`,
/// `H2_NO_TOUCH_ROWS`, `H2_NO_FOV_ROW` and `H2_NO_VIDEO_SCREEN` take theirs
/// out. On iOS the touch controls' rows take the Steam Controller's place
/// (no Steam Controller reaches an iPhone or iPad); elsewhere there are no
/// touch controls, so no touch rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Switches {
    pub speaker_row: bool,
    /// The Steam Controller's three rows on Controls, after the speaker's.
    pub steam_rows: bool,
    /// The touch controls' three rows on Controls (iOS), after the
    /// speaker's: look speed, crouch, gyro.
    pub touch_rows: bool,
    pub fov_row: bool,
    pub video_screen: bool,
}

impl Switches {
    pub const ALL: Switches = Switches {
        speaker_row: true,
        steam_rows: true,
        touch_rows: false,
        fov_row: true,
        video_screen: true,
    };

    pub fn from_env() -> Switches {
        let off = |k: &str| std::env::var_os(k).is_some();
        Switches {
            speaker_row: !off("H2_NO_SPEAKER_ROW"),
            steam_rows: !IOS && !off("H2_NO_STEAM_ROWS"),
            touch_rows: IOS && !off("H2_NO_TOUCH_ROWS"),
            fov_row: !off("H2_NO_FOV_ROW"),
            video_screen: !off("H2_NO_VIDEO_SCREEN"),
        }
    }
}

/// The touch controls' rows replace the Steam Controller's here.
const IOS: bool = cfg!(target_os = "ios");

/// The retail Settings list: its screen id, its datum array's name (VA)
/// and row count, its row text (thiscall, item at `[esp+4]`, `ret 8`) and
/// row chosen (thiscall, `[esp+4]` the player chain, `[esp+8]` the row,
/// `ret 8`), and the handler byte that adds a third item widget.
pub const SETTINGS_SCREEN_ID: i16 = 19;
pub const SETTINGS_LIST_NAME: u32 = 0x0045_b128;
pub const SETTINGS_ROWS: u32 = 2;
pub const SETTINGS_ROW_TEXT: u32 = 0x002b_1d6e;
pub const SETTINGS_ROW_CHOSEN: u32 = 0x002b_1da9;
pub const SETTINGS_THIRD_ITEM: u32 = 0x220;

/// The model of our rows: the Video screen from the settings that name it,
/// the Video row that opens it on the Settings list (left out when the
/// screen has no rows), and on Controls the Controller Speaker row and the
/// Steam Controller's three (on/off rows with the game's own On and Off).
pub fn model(store: Arc<Store>, on: Switches) -> Result<Model, String> {
    let mut video = Screen::new(SCREEN).title("Video");
    if on.video_screen {
        video = video.rows_from_settings(&store.defs());
    }
    if !on.fov_row {
        video = video.without(FOV.key);
    }
    let mut controls = ui::extend(List::Controls, speaker_row::VANILLA_ROWS);
    if on.speaker_row {
        controls = controls
            .row(Row::setting(PAD_SPEAKER.key).bool_ids(speaker_row::ON_ID, speaker_row::OFF_ID));
    }
    // The Steam Controller's rows (docs/input/steam-controller.md) sit with
    // the controller and speaker options.
    if on.steam_rows {
        controls = controls
            .row(Row::setting(STEAM_SPEAKER.key).bool_ids(speaker_row::ON_ID, speaker_row::OFF_ID))
            .row(Row::setting(STEAM_SENSITIVITY.key))
            .row(Row::setting(STEAM_LEFT_PAD.key));
    }
    // The iOS touch controls' rows (docs/runtime/ios.md).
    if on.touch_rows {
        controls = controls
            .row(Row::setting(TOUCH_LOOK.key))
            .row(Row::setting(TOUCH_CROUCH.key))
            .row(Row::setting(TOUCH_GYRO.key))
            .row(Row::setting(TOUCH_ACCEL.key))
            .row(
                Row::setting(TOUCH_AUTO_FIRE.key).bool_ids(speaker_row::ON_ID, speaker_row::OFF_ID),
            );
    }
    let settings = ui::extend(List::Settings, SETTINGS_ROWS).row(Row::open(SCREEN));
    Model::new(store, vec![video], vec![controls, settings], MODEL_IDS)
}

/// The item count when the game builds the array named `name` with
/// `retail` rows: ours added for the Controls and Settings lists, else
/// unchanged.
pub fn list_count(m: &Model, name: u32, retail: u32) -> u32 {
    match name {
        speaker_row::LIST_NAME => m.count(List::Controls, retail),
        SETTINGS_LIST_NAME => m.count(List::Settings, retail),
        _ => retail,
    }
}

/// Whether the Settings list just built with `count` rows (array `name`)
/// needs its third item widget: the handler byte at
/// [`SETTINGS_THIRD_ITEM`] set, so slot 19 reports three.
pub fn settings_third_item(name: u32, count: u32) -> bool {
    name == SETTINGS_LIST_NAME && count > SETTINGS_ROWS
}

/// The pane call for row `row` of screen `id`: `true` for our row on the
/// Settings list, which has no pane (the shown one stays).
pub fn skip_settings_pane(id: i16, row: i16) -> bool {
    id == SETTINGS_SCREEN_ID && row >= SETTINGS_ROWS as i16
}

/// What a Settings list row text call draws for row `row`: our row's label
/// (its one text, slot 0), or `None` for a retail row.
pub fn settings_text(m: &Model, row: Option<i16>) -> Option<(u32, u32)> {
    let r = m.row(List::Settings, row?)?;
    Some(m.texts(r)[0])
}

/// The player mask a Controls row-chosen call (`[esp+4]` points at a
/// pointer to the player record, whose `+4` is the local player index)
/// opens screens for, as `0x2c8719` computes it: `1 << index`. `None` for an
/// unreadable chain or an index past 3.
pub fn player_mask<M: Gm>(m: &M, esp: u32) -> Option<u32> {
    let p = m.u32(esp.wrapping_add(4))?;
    let record = m.u32(p)?;
    let index = m.u32(record.wrapping_add(4))?;
    (index < 4).then(|| 1 << index)
}

/// The row index a row text call (item at `[esp+4]`) draws.
pub fn text_row<M: Gm>(m: &M, esp: u32) -> Option<i16> {
    let item = m.u32(esp.wrapping_add(4))?;
    speaker_row::item_row(m, item)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout_glue::{chosen_row, is_ours};
    use crate::setting_defs::{ALL, FPS, LIGHTING, PAD_MUTE, RENDER_SCALE};
    use crate::ui::{Chosen, RowKind};
    use settings::Value;
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

    const ESP: u32 = 0x1000;
    const ITEM: u32 = 0x2000;

    fn all() -> Model {
        model(Arc::new(Store::defaults(ALL)), Switches::ALL).unwrap()
    }

    fn text_call(row: i16) -> Fake {
        let mut f = Fake::default();
        f.set_u32(ESP + 4, ITEM);
        f.set_u16(ITEM + 0x70, row as u16);
        f
    }

    fn chosen_call(row: i16) -> Fake {
        let mut f = Fake::default();
        f.set_u32(ESP + 8, 0x7000);
        f.set_u16(0x7000, row as u16);
        f
    }

    /// The label and value texts a row text call for `row` of `list` draws.
    fn drawn(m: &Model, list: List, row: i16) -> Option<(String, String)> {
        let row = m.row(list, text_row(&text_call(row), ESP)?)?;
        let [(0, l), (1, v)] = m.texts(row) else {
            panic!("slots")
        };
        let text = |id| {
            m.strings()
                .iter()
                .find(|(i, _)| *i == id)
                .map(|(_, t)| t.clone())
        };
        Some((text(l)?, text(v).unwrap_or_else(|| format!("{v:#x}"))))
    }

    /// A press on `row` of `list`, as the row-chosen hook reads it.
    fn press(m: &Model, list: List, row: i16) -> Chosen {
        match chosen_row(&chosen_call(row), ESP).and_then(|r| m.row(list, r)) {
            Some(r) => m.press(r),
            None => Chosen::Nothing,
        }
    }

    fn s(a: &str, b: &str) -> Option<(String, String)> {
        Some((a.into(), b.into()))
    }

    #[test]
    fn rows_follow_the_settings_and_switches() {
        let m = all();
        let kinds: Vec<_> = m.rows(LIST).iter().map(|r| r.kind.clone()).collect();
        assert_eq!(
            kinds,
            [
                RowKind::Setting(FOV.key),
                RowKind::Setting(FPS.key),
                RowKind::Setting(RENDER_SCALE.key),
                RowKind::Setting(LIGHTING.key),
                RowKind::Setting(PAD_MUTE.key),
                RowKind::Setting(crate::setting_defs::AIM_ASSIST.key)
            ]
        );
        let store = || Arc::new(Store::defaults(ALL));
        let no_fov = model(
            store(),
            Switches {
                fov_row: false,
                ..Switches::ALL
            },
        )
        .unwrap();
        assert_eq!(no_fov.rows(LIST).len(), 5);
        assert_eq!(m.count(List::Settings, SETTINGS_ROWS), 3);
        assert_eq!(m.count(List::Controls, 6), 10, "the speaker's and Steam's");
        // No Video screen: no Video row on Settings either.
        let none = model(
            store(),
            Switches {
                video_screen: false,
                ..Switches::ALL
            },
        )
        .unwrap();
        assert!(none.rows(LIST).is_empty());
        assert_eq!(none.count(List::Controls, 6), 10);
        assert_eq!(none.count(List::Settings, SETTINGS_ROWS), 2);
        let bare = model(
            store(),
            Switches {
                video_screen: false,
                speaker_row: false,
                steam_rows: false,
                touch_rows: false,
                fov_row: true,
            },
        )
        .unwrap();
        assert_eq!(bare.count(List::Controls, 6), 6);
    }

    /// iOS: the touch controls' rows where Steam's would be, drawn with
    /// their values and stepped by a press.
    #[test]
    fn the_touch_rows_take_the_steam_rows_place_on_ios() {
        let ios = Switches {
            steam_rows: false,
            touch_rows: true,
            ..Switches::ALL
        };
        let m = model(Arc::new(Store::defaults(ALL)), ios).unwrap();
        assert_eq!(
            m.count(List::Controls, 6),
            12,
            "the speaker's and the touch rows"
        );
        assert_eq!(drawn(&m, List::Controls, 7), s("Touch Look Speed", "5"));
        assert_eq!(drawn(&m, List::Controls, 8), s("Touch Crouch", "Toggle"));
        assert_eq!(drawn(&m, List::Controls, 9), s("Gyro Aim", "0"));
        assert_eq!(drawn(&m, List::Controls, 10), s("Look Acceleration", "5"));
        assert!(drawn(&m, List::Controls, 11).is_some(), "Touch Auto Fire");
        assert_eq!(drawn(&m, List::Controls, 12), None);
        assert_eq!(
            m.index_of(List::Controls, &RowKind::Setting(STEAM_SPEAKER.key)),
            None
        );
        const { assert!(!Switches::ALL.touch_rows, "no touch controls off iOS") };
        if cfg!(target_os = "ios") {
            assert_eq!(Switches::from_env(), ios);
        } else {
            assert!(!Switches::from_env().touch_rows);
        }
    }

    #[test]
    fn the_video_row_is_the_settings_lists_third() {
        let m = all();
        assert_eq!(m.index_of(List::Settings, &RowKind::Open(SCREEN)), Some(2));
        assert_eq!(m.index_of(List::Controls, &RowKind::Open(SCREEN)), None);
        assert_eq!(drawn(&m, List::Settings, 2), s("Video", ""));
        assert_eq!(
            drawn(&m, List::Controls, 6),
            s("Controller Speaker", "0x2000181")
        );
        for r in [0i16, 1, 3, -1] {
            assert_eq!(drawn(&m, List::Settings, r), None, "{r}");
        }
        assert_eq!(
            drawn(&m, List::Controls, 7),
            s("Steam Pad Speaker", "0x2000181")
        );
        assert_eq!(drawn(&m, List::Controls, 8), s("Trackpad Aim", "5"));
        assert_eq!(drawn(&m, List::Controls, 9), s("Left Trackpad", "D-Pad"));
        assert_eq!(drawn(&m, List::Controls, 10), None);
        // The Settings row text draws one text: the label, in slot 0.
        let title = m.title_id(SCREEN).unwrap();
        assert_eq!(settings_text(&m, Some(2)), Some((0, title)));
        assert_eq!(settings_text(&m, Some(1)), None, "Game Variants is retail");
        assert_eq!(settings_text(&m, None), None);
        assert_eq!(press(&m, List::Settings, 2), Chosen::Open(SCREEN));
        assert_eq!(press(&m, List::Settings, 0), Chosen::Nothing);
        assert_eq!(press(&m, List::Settings, 1), Chosen::Nothing);
        let no_speaker = model(
            Arc::new(Store::defaults(ALL)),
            Switches {
                speaker_row: false,
                ..Switches::ALL
            },
        )
        .unwrap();
        assert_eq!(
            no_speaker.index_of(List::Settings, &RowKind::Open(SCREEN)),
            Some(2)
        );
        assert_eq!(no_speaker.count(List::Controls, 6), 9, "Steam's rows only");
        assert_eq!(
            no_speaker.index_of(List::Controls, &RowKind::Setting(STEAM_SPEAKER.key)),
            Some(6)
        );
        let no_steam = model(
            Arc::new(Store::defaults(ALL)),
            Switches {
                steam_rows: false,
                ..Switches::ALL
            },
        )
        .unwrap();
        assert_eq!(no_steam.count(List::Controls, 6), 7, "the speaker row only");
    }

    #[test]
    fn the_settings_list_gets_a_third_widget_and_no_pane_for_it() {
        let m = all();
        let n = list_count(&m, SETTINGS_LIST_NAME, SETTINGS_ROWS);
        assert_eq!(n, 3);
        assert!(settings_third_item(SETTINGS_LIST_NAME, n));
        assert!(!settings_third_item(SETTINGS_LIST_NAME, 2), "not grown");
        assert!(!settings_third_item(speaker_row::LIST_NAME, 7));
        // A Settings list built with another count is not the one we know.
        assert_eq!(list_count(&m, SETTINGS_LIST_NAME, 4), 4);
        assert!(skip_settings_pane(SETTINGS_SCREEN_ID, 2));
        assert!(!skip_settings_pane(SETTINGS_SCREEN_ID, 0));
        assert!(!skip_settings_pane(SETTINGS_SCREEN_ID, 1));
        assert!(!skip_settings_pane(speaker_row::SCREEN_ID, 2));
        assert!(!skip_settings_pane(
            crate::skulls_menu::DIFFICULTY_SCREEN_ID,
            2
        ));
    }

    #[test]
    fn the_controls_list_grows_by_our_rows() {
        let m = all();
        assert_eq!(list_count(&m, speaker_row::LIST_NAME, 6), 10);
        assert_eq!(list_count(&m, speaker_row::LIST_NAME, 4), 4);
        assert_eq!(list_count(&m, crate::layout_glue::LIST_NAME, 6), 6);
        assert_eq!(list_count(&m, 0x1234, 6), 6);
    }

    #[test]
    fn the_player_mask_is_read_as_the_row_handler_reads_it() {
        for index in 0..4u32 {
            let mut f = Fake::default();
            f.set_u32(ESP + 4, 0x5000);
            f.set_u32(0x5000, 0x6000);
            f.set_u32(0x6004, index);
            assert_eq!(player_mask(&f, ESP), Some(1 << index));
        }
        let mut f = Fake::default();
        f.set_u32(ESP + 4, 0x5000);
        f.set_u32(0x5000, 0x6000);
        f.set_u32(0x6004, 4);
        assert_eq!(player_mask(&f, ESP), None);
        assert_eq!(player_mask(&Fake::default(), ESP), None);
    }

    #[test]
    fn every_row_draws_its_label_and_value_as_before() {
        let m = all();
        assert_eq!(drawn(&m, LIST, 0), s("Field of View", "70"));
        assert_eq!(drawn(&m, LIST, 1), s("Frame Rate", "30"));
        assert_eq!(drawn(&m, LIST, 2), s("Resolution", "640x480"));
        assert_eq!(drawn(&m, LIST, 3), s("Lighting", "Retail"));
        assert_eq!(drawn(&m, LIST, 4), s("Mute Button", "On"));
        assert_eq!(drawn(&m, LIST, 5), s("Aim Assist", "Auto"));
        assert_eq!(drawn(&m, LIST, 6), None);
        assert_eq!(drawn(&m, LIST, -1), None);
    }

    #[test]
    fn pressing_steps_the_row_saves_it_and_applies_it_at_once() {
        let dir = std::env::temp_dir().join(format!("h2-video-{}-press", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = settings::path_in(&dir);
        std::fs::write(&path, "[pad_speaker]\nenabled = false\n").unwrap();
        let store = Arc::new(Store::load(ALL, Some(&path), &|_| None, &[]).0);
        let fov = store.handle::<u32>(FOV.key);
        let fps = store.handle::<u32>(FPS.key);
        let scale = store.handle::<u32>(RENDER_SCALE.key);
        let mute = store.handle::<bool>(PAD_MUTE.key);
        let lighting = store.handle::<u32>(LIGHTING.key);
        // What the window, the view backend and the HID thread are told.
        let told = Arc::new(std::sync::Mutex::new(Vec::new()));
        for key in [FPS.key, RENDER_SCALE.key, LIGHTING.key, PAD_MUTE.key] {
            let told = Arc::clone(&told);
            store.on_change(key, move |v| told.lock().unwrap().push((key, *v)));
        }
        let m = model(Arc::clone(&store), Switches::ALL).unwrap();
        assert_eq!(
            press(&m, LIST, 1),
            Chosen::Stepped("graphics.fps: 60".into())
        );
        assert_eq!(drawn(&m, LIST, 1), s("Frame Rate", "60"));
        assert_eq!(fps.get(), 60, "applies at once");
        press(&m, LIST, 1);
        press(&m, LIST, 1);
        assert_eq!(drawn(&m, LIST, 1), s("Frame Rate", "Uncapped"));
        assert_eq!(fps.get(), 0);
        press(&m, LIST, 1);
        assert_eq!(drawn(&m, LIST, 1), s("Frame Rate", "30"));
        assert_eq!(
            press(&m, LIST, 0),
            Chosen::Stepped("graphics.fov: 75".into())
        );
        assert_eq!(drawn(&m, LIST, 0), s("Field of View", "75"));
        assert_eq!(fov.get(), 75, "applies at once");
        press(&m, LIST, 2);
        assert_eq!(drawn(&m, LIST, 2), s("Resolution", "1280x960"));
        assert_eq!(scale.get(), 2, "applies at once");
        assert_eq!(press(&m, List::Settings, 2), Chosen::Open(SCREEN));
        assert_eq!(
            drawn(&m, List::Controls, 6),
            s("Controller Speaker", "0x3000182")
        );
        press(&m, List::Controls, 6);
        assert_eq!(
            drawn(&m, List::Controls, 6),
            s("Controller Speaker", "0x2000181")
        );
        // The Steam Controller's rows step and save like the speaker's.
        assert_eq!(
            press(&m, List::Controls, 7),
            Chosen::Stepped("steam_pad.speaker: false".into())
        );
        assert_eq!(drawn(&m, List::Controls, 8), s("Trackpad Aim", "5"));
        press(&m, List::Controls, 8);
        assert_eq!(drawn(&m, List::Controls, 8), s("Trackpad Aim", "6"));
        press(&m, List::Controls, 9);
        assert_eq!(drawn(&m, List::Controls, 9), s("Left Trackpad", "Stick"));
        assert_eq!(
            press(&m, LIST, 3),
            Chosen::Stepped("graphics.lighting: e3".into())
        );
        assert_eq!(drawn(&m, LIST, 3), s("Lighting", "E3"));
        assert_eq!(
            crate::setting_defs::lighting_of(lighting.get()),
            d3d::Lighting::E3,
            "applies at once"
        );
        assert_eq!(
            press(&m, LIST, 4),
            Chosen::Stepped("pad.mute_button: false".into())
        );
        assert_eq!(drawn(&m, LIST, 4), s("Mute Button", "Off"));
        assert!(!mute.get(), "applies at once");
        // Each change reached its listener, in order, once.
        assert_eq!(
            *told.lock().unwrap(),
            [
                (FPS.key, Value::Int(60)),
                (FPS.key, Value::Int(120)),
                (FPS.key, Value::Int(0)),
                (FPS.key, Value::Int(30)),
                (RENDER_SCALE.key, Value::Int(2)),
                (LIGHTING.key, Value::Int(1)),
                (PAD_MUTE.key, Value::Bool(false)),
            ]
        );
        // Rows past ours, or no row, are not ours.
        assert_eq!(press(&m, LIST, 6), Chosen::Nothing);
        assert_eq!(press(&m, List::Controls, 5), Chosen::Nothing);
        assert_eq!(press(&m, List::Controls, 10), Chosen::Nothing);
        // Saved as the old code saved them; other keys kept.
        let t: toml::Table = std::fs::read_to_string(&path).unwrap().parse().unwrap();
        assert_eq!(t["graphics"]["fov"].as_integer(), Some(75));
        assert_eq!(t["graphics"]["fps"].as_integer(), Some(30));
        assert_eq!(t["graphics"]["render_scale"].as_integer(), Some(2));
        assert_eq!(t["graphics"]["lighting"].as_str(), Some("e3"));
        assert_eq!(t["pad_speaker"]["enabled"].as_bool(), Some(true));
        assert_eq!(t["pad"]["mute_button"].as_bool(), Some(false));
        assert_eq!(t["steam_pad"]["speaker"].as_bool(), Some(false));
        assert_eq!(t["steam_pad"]["sensitivity"].as_integer(), Some(6));
        assert_eq!(t["steam_pad"]["left_pad"].as_str(), Some("stick"));
        let next = Store::load(ALL, Some(&path), &|_| None, &[]).0;
        assert_eq!(next.get(RENDER_SCALE.key), Some(Value::Int(2)));
        assert_eq!(next.get(LIGHTING.key), Some(Value::Int(1)));
    }

    #[test]
    fn locked_rows_say_so_and_do_not_move() {
        let env = |k: &str| (k == "H2_FPS").then(|| "120".to_string());
        let store = Arc::new(Store::load(ALL, None, &env, &[]).0);
        let m = model(store, Switches::ALL).unwrap();
        assert_eq!(
            press(&m, LIST, 1),
            Chosen::Stepped("graphics.fps: set by H2_FPS; row locked".into())
        );
        assert_eq!(drawn(&m, LIST, 1), s("Frame Rate", "120"));
    }

    #[test]
    fn strings_are_ours_distinct_and_say_what_they_show() {
        let m = all();
        let all = m.strings();
        let text = |t: &str| all.iter().any(|(_, s)| s == t);
        for t in [
            "Video",
            "Uncapped",
            "60",
            "640x480",
            "2560x1920",
            "3840x2880",
            "Off",
            "150",
            "30",
            "",
        ] {
            assert!(text(t), "{t:?}");
        }
        // Every row applies at once: no "(restart)" text at all.
        assert!(all.iter().all(|(_, s)| !s.contains("(restart)")), "{all:?}");
        let mut ids: Vec<u32> = all.iter().map(|(i, _)| *i).collect();
        assert!(ids.iter().all(|i| is_ours(*i) && MODEL_IDS.contains(i)));
        ids.extend(
            crate::layout_glue::served_strings(true, true)
                .iter()
                .map(|(i, _)| *i),
        );
        let n = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), n, "no id is used twice");
    }
}
