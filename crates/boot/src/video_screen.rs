//! Our "Video" screen: a "Video" row on Halo 2's Settings, Controls screen
//! that opens a second Controls screen holding our video rows (Field of
//! View, Frame Rate, Resolution) instead of the retail six. See
//! docs/settings-menu.md "Video screen" for the trace.
//!
//! **Opening it.** The Settings screen opens Controls by building a
//! 0x20-byte screen request and calling the Controls loader through
//! `0x149f1e(player mask, loader)` (request `+2` the player mask, `+4` 5,
//! `+8` 4, `+0x1c` the loader, which it then calls with the request). The
//! loader `0x2b7714` allocates 0x9b4 bytes, runs the screen constructor
//! `0x2b7752` (base screen `0x230451` with screen id 39, then the Controls
//! list handler `0x2c84b8` at `screen + 0x614`) and pushes the screen
//! (`0x147f6d`). Our row makes the very same call, so the new screen sits on
//! top of Controls like any of Controls' own dialogs and B returns to it.
//!
//! **Telling it apart.** The list handler's constructor makes its datum
//! array through `0x1a480d`, which copies the name it is given into the
//! array (`0x16b650`, 0x20 bytes at `+0`). While our row's open call runs,
//! the list hook hands that constructor our own name and our row count, so
//! the Video list is the one whose array is named [`LIST_NAME_TEXT`]: no
//! table of live screens to keep.
//!
//! **Its rows** go through the same Controls row text (`0x2c8548`) and row
//! chosen (`0x2c8719`) functions, which `layout_glue` already wraps: for a
//! Video list every row is ours and the originals never run.
//!
//! **Its title.** The header is built once by `0x22fda6(screen)`, which sets
//! the tag's header string (`0x06000233` for Controls) through `0x253b1a`.
//! While that runs for a Video screen, the text hook swaps the id for ours.
//!
//! The hooks are registered by `layout_glue::install`; the logic is here,
//! over [`Gm`].

use std::ops::Range;
use std::sync::Arc;

use settings::Store;

use crate::layout_glue::{Gm, ID_BASE};
use crate::setting_defs::{FOV, PAD_SPEAKER};
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
/// The name our list's datum array is made with (it fits the array's 0x20
/// bytes, NUL included).
pub const LIST_NAME_TEXT: &str = "h2r video settings";

/// Our screen's id, and its list in the model.
pub const SCREEN: &str = "video";
pub const LIST: List = List::Screen(SCREEN);
/// The string ids the model's texts take: past the layouts' (`0x00` to
/// `0x1f`) and the small Controls list's (`0x30`, `0x31`), short of the list
/// name (`0xff`).
pub const MODEL_IDS: Range<u32> = ID_BASE + 0x40..ID_BASE + 0xff;

/// Which of our rows are on: `H2_NO_SPEAKER_ROW`, `H2_NO_FOV_ROW` and
/// `H2_NO_VIDEO_SCREEN` take theirs out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Switches {
    pub speaker_row: bool,
    pub fov_row: bool,
    pub video_screen: bool,
}

impl Switches {
    pub const ALL: Switches = Switches {
        speaker_row: true,
        fov_row: true,
        video_screen: true,
    };

    pub fn from_env() -> Switches {
        let off = |k: &str| std::env::var_os(k).is_some();
        Switches {
            speaker_row: !off("H2_NO_SPEAKER_ROW"),
            fov_row: !off("H2_NO_FOV_ROW"),
            video_screen: !off("H2_NO_VIDEO_SCREEN"),
        }
    }
}

/// The model of our rows: the Video screen from the settings that name it,
/// and on the Controls list the Controller Speaker row (with the game's
/// own On and Off) and the Video row that opens the screen (left out when
/// the screen has no rows).
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
    controls = controls.row(Row::open(SCREEN));
    Model::new(store, vec![video], vec![controls], MODEL_IDS)
}

/// The Controls list's item count when the game builds the array named
/// `name` with `retail` rows: ours added for the Controls list, else
/// unchanged.
pub fn list_count(m: &Model, name: u32, retail: u32) -> u32 {
    if name == speaker_row::LIST_NAME {
        m.count(List::Controls, retail)
    } else {
        retail
    }
}

/// The datum array constructor with name `name` and count `retail` while
/// our open call runs: the count for the Video list (our rows), or `None`
/// for any other list (the hook leaves it alone).
pub fn video_count(name: u32, retail: u32, opening: bool, rows: usize) -> Option<u32> {
    (opening && name == speaker_row::LIST_NAME && retail == speaker_row::VANILLA_ROWS && rows > 0)
        .then_some(rows as u32)
}

/// Whether the list `handler` is a Video list: its datum array
/// (`[handler+0x70]`) is named [`LIST_NAME_TEXT`].
pub fn is_video_list<M: Gm>(m: &M, handler: u32) -> bool {
    let Some(array) = m.u32(handler.wrapping_add(0x70)) else {
        return false;
    };
    let want = LIST_NAME_TEXT.as_bytes();
    let mut got = vec![0; want.len() + 1];
    m.rd(array, &mut got) && got[..want.len()] == *want && got[want.len()] == 0
}

/// Whether `screen` is a Video screen (its list handler at `+0x614`).
pub fn is_video_screen<M: Gm>(m: &M, screen: u32) -> bool {
    is_video_list(m, screen.wrapping_add(SCREEN_LIST))
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
    use crate::setting_defs::{ALL, FPS, RENDER_SCALE};
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
    const HANDLER: u32 = 0x3000;
    const ARRAY: u32 = 0x4000;

    fn all() -> Model {
        model(Arc::new(Store::defaults(ALL)), Switches::ALL).unwrap()
    }

    fn list(name: &[u8]) -> Fake {
        let mut f = Fake::default();
        f.set_u32(HANDLER + 0x70, ARRAY);
        let mut n = name.to_vec();
        n.resize(0x20, 0);
        f.wr(ARRAY, &n);
        f
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
                RowKind::Setting(RENDER_SCALE.key)
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
        assert_eq!(no_fov.rows(LIST).len(), 2);
        // No Video screen: no Video row on Controls either.
        let none = model(
            store(),
            Switches {
                video_screen: false,
                ..Switches::ALL
            },
        )
        .unwrap();
        assert!(none.rows(LIST).is_empty());
        assert_eq!(none.count(List::Controls, 6), 7);
        let bare = model(
            store(),
            Switches {
                video_screen: false,
                speaker_row: false,
                fov_row: true,
            },
        )
        .unwrap();
        assert_eq!(bare.count(List::Controls, 6), 6);
    }

    #[test]
    fn the_video_row_sits_after_the_speaker_row() {
        let m = all();
        assert_eq!(m.index_of(List::Controls, &RowKind::Open(SCREEN)), Some(7));
        assert_eq!(
            drawn(&m, List::Controls, 6),
            s("Controller Speaker", "0x2000181")
        );
        assert_eq!(drawn(&m, List::Controls, 7), s("Video", ""));
        for r in [0i16, 5, 8, -1] {
            assert_eq!(drawn(&m, List::Controls, r), None, "{r}");
        }
        let no_speaker = model(
            Arc::new(Store::defaults(ALL)),
            Switches {
                speaker_row: false,
                ..Switches::ALL
            },
        )
        .unwrap();
        assert_eq!(
            no_speaker.index_of(List::Controls, &RowKind::Open(SCREEN)),
            Some(6)
        );
    }

    #[test]
    fn the_controls_list_grows_by_our_rows() {
        let m = all();
        assert_eq!(list_count(&m, speaker_row::LIST_NAME, 6), 8);
        assert_eq!(list_count(&m, speaker_row::LIST_NAME, 4), 4);
        assert_eq!(list_count(&m, crate::layout_glue::LIST_NAME, 6), 6);
        assert_eq!(list_count(&m, 0x1234, 6), 6);
    }

    #[test]
    fn only_the_list_built_by_our_open_call_becomes_the_video_list() {
        assert_eq!(video_count(speaker_row::LIST_NAME, 6, true, 3), Some(3));
        assert_eq!(
            video_count(speaker_row::LIST_NAME, 6, false, 3),
            None,
            "a plain Controls screen"
        );
        assert_eq!(
            video_count(crate::layout_glue::LIST_NAME, 4, true, 3),
            None,
            "another list"
        );
        assert_eq!(video_count(speaker_row::SMALL_LIST_NAME, 6, true, 3), None);
        assert_eq!(video_count(speaker_row::LIST_NAME, 6, true, 0), None);
    }

    #[test]
    fn a_video_list_is_known_by_its_arrays_name() {
        let mut name = LIST_NAME_TEXT.as_bytes().to_vec();
        assert!(name.len() < 0x20);
        assert!(is_video_list(&list(&name), HANDLER));
        assert!(is_video_screen(&list(&name), HANDLER - SCREEN_LIST));
        assert!(!is_video_list(&list(b"controls list"), HANDLER));
        // A longer name that starts the same is not ours.
        name.extend_from_slice(b"2");
        assert!(!is_video_list(&list(&name), HANDLER));
        assert!(!is_video_list(&Fake::default(), HANDLER));
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
        assert_eq!(drawn(&m, LIST, 2), s("Resolution", "1x"));
        assert_eq!(drawn(&m, LIST, 3), None);
        assert_eq!(drawn(&m, LIST, -1), None);
    }

    #[test]
    fn pressing_steps_the_row_saves_it_and_the_text_says_restart() {
        let dir = std::env::temp_dir().join(format!("h2-video-{}-press", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = settings::path_in(&dir);
        std::fs::write(&path, "[pad_speaker]\nenabled = false\n").unwrap();
        let store = Arc::new(Store::load(ALL, Some(&path), &|_| None, &[]).0);
        let fov = store.handle::<u32>(FOV.key);
        let fps = store.handle::<u32>(FPS.key);
        let m = model(Arc::clone(&store), Switches::ALL).unwrap();
        assert_eq!(
            press(&m, LIST, 1),
            Chosen::Stepped("graphics.fps: 60 from the next launch".into())
        );
        assert_eq!(drawn(&m, LIST, 1), s("Frame Rate", "60 (restart)"));
        press(&m, LIST, 1);
        press(&m, LIST, 1);
        assert_eq!(drawn(&m, LIST, 1), s("Frame Rate", "Uncapped (restart)"));
        assert_eq!(fps.get(), 30, "this run keeps its rate");
        press(&m, LIST, 1);
        assert_eq!(
            drawn(&m, LIST, 1),
            s("Frame Rate", "30"),
            "back to this run's"
        );
        assert_eq!(
            press(&m, LIST, 0),
            Chosen::Stepped("graphics.fov: 75".into())
        );
        assert_eq!(drawn(&m, LIST, 0), s("Field of View", "75"));
        assert_eq!(fov.get(), 75, "applies at once");
        press(&m, LIST, 2);
        assert_eq!(drawn(&m, LIST, 2), s("Resolution", "2x (restart)"));
        assert_eq!(press(&m, List::Controls, 7), Chosen::Open(SCREEN));
        assert_eq!(
            drawn(&m, List::Controls, 6),
            s("Controller Speaker", "0x3000182")
        );
        press(&m, List::Controls, 6);
        assert_eq!(
            drawn(&m, List::Controls, 6),
            s("Controller Speaker", "0x2000181")
        );
        // Rows past ours, or no row, are not ours.
        assert_eq!(press(&m, LIST, 3), Chosen::Nothing);
        assert_eq!(press(&m, List::Controls, 5), Chosen::Nothing);
        // Saved as the old code saved them; other keys kept.
        let t: toml::Table = std::fs::read_to_string(&path).unwrap().parse().unwrap();
        assert_eq!(t["graphics"]["fov"].as_integer(), Some(75));
        assert_eq!(t["graphics"]["fps"].as_integer(), Some(30));
        assert_eq!(t["graphics"]["render_scale"].as_integer(), Some(2));
        assert_eq!(t["pad_speaker"]["enabled"].as_bool(), Some(true));
        let next = Store::load(ALL, Some(&path), &|_| None, &[]).0;
        assert_eq!(next.get(RENDER_SCALE.key), Some(Value::Int(2)));
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
            "60 (restart)",
            "6x",
            "6x (restart)",
            "150",
            "30",
            "",
        ] {
            assert!(text(t), "{t:?}");
        }
        assert!(!text("75 (restart)"), "the field of view applies at once");
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
