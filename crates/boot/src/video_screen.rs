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

use std::sync::Arc;

use crate::fov_row;
use crate::fov_setting::FovSetting;
use crate::layout_glue::{Gm, ID_BASE};
use crate::speaker_row;
use crate::video_setting::{Kind, LaunchSetting};

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

/// Our strings, in `layout_glue::is_ours`'s range.
pub const ROW_LABEL_ID: u32 = ID_BASE + 0xc0;
pub const TITLE_ID: u32 = ID_BASE + 0xc1;
pub const FPS_LABEL_ID: u32 = ID_BASE + 0xc2;
pub const SCALE_LABEL_ID: u32 = ID_BASE + 0xc3;
pub const EMPTY_ID: u32 = ID_BASE + 0xc4;
const FPS_VALUE_BASE: u32 = ID_BASE + 0xd0;
const SCALE_VALUE_BASE: u32 = ID_BASE + 0xe0;

/// A row of the Video screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    Fov,
    Fps,
    Scale,
}

/// What the Video screen shows: each setting that is on.
#[derive(Clone, Default)]
pub struct Settings {
    pub fov: Option<Arc<FovSetting>>,
    pub fps: Option<Arc<LaunchSetting>>,
    pub scale: Option<Arc<LaunchSetting>>,
}

impl Settings {
    /// The rows, in order: Field of View, Frame Rate, Resolution, each when
    /// its setting is given.
    pub fn rows(&self) -> Vec<Row> {
        let mut v = Vec::new();
        if self.fov.is_some() {
            v.push(Row::Fov);
        }
        if self.fps.is_some() {
            v.push(Row::Fps);
        }
        if self.scale.is_some() {
            v.push(Row::Scale);
        }
        v
    }

    /// No rows: no Video row on Controls either.
    pub fn is_empty(&self) -> bool {
        self.rows().is_empty()
    }

    /// The label and value string ids of `row`.
    pub fn texts(&self, row: Row) -> Option<[(u32, u32); 2]> {
        let (label, value) = match row {
            Row::Fov => (fov_row::LABEL_ID, fov_row::value_id(self.fov.as_ref()?.shown())),
            Row::Fps => {
                let s = self.fps.as_ref()?;
                (FPS_LABEL_ID, value_id(Kind::Fps, s.shown(), s.pending())?)
            }
            Row::Scale => {
                let s = self.scale.as_ref()?;
                (SCALE_LABEL_ID, value_id(Kind::RenderScale, s.shown(), s.pending())?)
            }
        };
        Some([(speaker_row::LABEL_TEXT_INDEX, label), (speaker_row::VALUE_TEXT_INDEX, value)])
    }

    /// `row` was pressed: step its setting. Returns what to say on stderr.
    pub fn press(&self, row: Row) -> Option<String> {
        match row {
            Row::Fov => {
                let s = self.fov.as_ref()?;
                s.step();
                Some(format!("fov: {} degrees", s.shown()))
            }
            Row::Fps | Row::Scale => {
                let s = if row == Row::Fps { self.fps.as_ref()? } else { self.scale.as_ref()? };
                let v = s.step();
                let k = s.kind();
                Some(match (s.locked(), s.pending()) {
                    (true, _) => format!("{}: set by {}; row locked", k.key(), k.env()),
                    (false, true) => format!("{}: {} from the next launch", k.key(), k.name(v)),
                    (false, false) => format!("{}: {}", k.key(), k.name(v)),
                })
            }
        }
    }
}

/// The Video row on the Controls screen: after the retail six and the
/// Controller Speaker row when that row is there.
pub fn controls_row(with_speaker: bool) -> i16 {
    speaker_row::VANILLA_ROWS as i16 + i16::from(with_speaker)
}

/// The Controls list's item count after the other rows grew it to `n`: one
/// more for the Video row (`retail` is the count the game asked for).
pub fn list_count(name: u32, retail: u32, n: u32, video_row: bool) -> u32 {
    if video_row && name == speaker_row::LIST_NAME && retail == speaker_row::VANILLA_ROWS { n + 1 } else { n }
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
    let Some(array) = m.u32(handler.wrapping_add(0x70)) else { return false };
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

/// The row a Video list's row text call (item at `[esp+4]`) is for.
pub fn text_row<M: Gm>(m: &M, esp: u32, rows: &[Row]) -> Option<Row> {
    let item = m.u32(esp.wrapping_add(4))?;
    let r = usize::try_from(speaker_row::item_row(m, item)?).ok()?;
    rows.get(r).copied()
}

/// The row of a Video list's row-chosen `chosen` (from
/// `layout_glue::chosen_row`).
pub fn chosen(chosen: Option<i16>, rows: &[Row]) -> Option<Row> {
    rows.get(usize::try_from(chosen?).ok()?).copied()
}

/// The Video row's texts on the Controls list (item at `[esp+4]`): our
/// label and an empty value, or `None` for any other row.
pub fn controls_row_texts<M: Gm>(m: &M, esp: u32, row: i16) -> Option<[(u32, u32); 2]> {
    let item = m.u32(esp.wrapping_add(4))?;
    (speaker_row::item_row(m, item)? == row).then_some([
        (speaker_row::LABEL_TEXT_INDEX, ROW_LABEL_ID),
        (speaker_row::VALUE_TEXT_INDEX, EMPTY_ID),
    ])
}

/// The string id that shows value `v` of `kind`, `pending` adding
/// "(restart)".
pub fn value_id(kind: Kind, v: u32, pending: bool) -> Option<u32> {
    let (base, i) = match kind {
        Kind::Fps => (FPS_VALUE_BASE, kind.all().iter().position(|x| *x == v)? as u32),
        Kind::RenderScale => (SCALE_VALUE_BASE, kind.all().iter().position(|x| *x == v)? as u32),
    };
    Some(base + i * 2 + u32::from(pending))
}

/// Every string the screen needs (the FOV row's come from `fov_row`).
pub fn strings() -> Vec<(u32, String)> {
    let mut v = vec![
        (ROW_LABEL_ID, "Video".to_string()),
        (TITLE_ID, "Video".to_string()),
        (FPS_LABEL_ID, "Frame Rate".to_string()),
        (SCALE_LABEL_ID, "Resolution".to_string()),
        (EMPTY_ID, String::new()),
    ];
    for kind in [Kind::Fps, Kind::RenderScale] {
        for x in kind.all() {
            for pending in [false, true] {
                let text = if pending { format!("{} (restart)", kind.label(x)) } else { kind.label(x) };
                v.push((value_id(kind, x, pending).expect("a listed value"), text));
            }
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout_glue::{chosen_row, is_ours};
    use crate::video_setting::UNCAPPED;
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

    fn all() -> Settings {
        Settings {
            fov: Some(Arc::new(FovSetting::new(None, None))),
            fps: Some(Arc::new(LaunchSetting::new(Kind::Fps, None, None))),
            scale: Some(Arc::new(LaunchSetting::new(Kind::RenderScale, None, None))),
        }
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

    #[test]
    fn rows_follow_the_settings_given() {
        assert_eq!(all().rows(), [Row::Fov, Row::Fps, Row::Scale]);
        let s = Settings { fov: None, ..all() };
        assert_eq!(s.rows(), [Row::Fps, Row::Scale]);
        assert!(Settings::default().is_empty());
        assert!(!all().is_empty());
    }

    #[test]
    fn the_video_row_sits_after_the_speaker_row() {
        assert_eq!(controls_row(true), 7);
        assert_eq!(controls_row(true), speaker_row::ROW + 1);
        assert_eq!(controls_row(false), 6);
        let t = text_call(7);
        assert_eq!(controls_row_texts(&t, ESP, 7), Some([(0, ROW_LABEL_ID), (1, EMPTY_ID)]));
        for r in [0i16, 5, 6, 8, -1] {
            assert_eq!(controls_row_texts(&text_call(r), ESP, 7), None, "{r}");
        }
    }

    #[test]
    fn the_controls_list_grows_by_the_video_row() {
        let speaker = speaker_row::list_count(speaker_row::LIST_NAME, 6);
        assert_eq!(list_count(speaker_row::LIST_NAME, 6, speaker, true), 8);
        assert_eq!(list_count(speaker_row::LIST_NAME, 6, speaker, false), 7);
        assert_eq!(list_count(speaker_row::LIST_NAME, 6, 6, true), 7);
        assert_eq!(list_count(crate::layout_glue::LIST_NAME, 4, 7, true), 7);
        assert_eq!(list_count(0x1234, 6, 6, true), 6);
    }

    #[test]
    fn only_the_list_built_by_our_open_call_becomes_the_video_list() {
        assert_eq!(video_count(speaker_row::LIST_NAME, 6, true, 3), Some(3));
        assert_eq!(video_count(speaker_row::LIST_NAME, 6, false, 3), None, "a plain Controls screen");
        assert_eq!(video_count(crate::layout_glue::LIST_NAME, 4, true, 3), None, "another list");
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
    fn every_row_draws_its_label_and_value() {
        let s = all();
        let rows = s.rows();
        assert_eq!(text_row(&text_call(0), ESP, &rows), Some(Row::Fov));
        assert_eq!(text_row(&text_call(2), ESP, &rows), Some(Row::Scale));
        assert_eq!(text_row(&text_call(3), ESP, &rows), None);
        assert_eq!(text_row(&text_call(-1), ESP, &rows), None);
        assert_eq!(s.texts(Row::Fov), Some([(0, fov_row::LABEL_ID), (1, fov_row::value_id(70))]));
        assert_eq!(s.texts(Row::Fps), Some([(0, FPS_LABEL_ID), (1, value_id(Kind::Fps, 30, false).unwrap())]));
        assert_eq!(
            s.texts(Row::Scale),
            Some([(0, SCALE_LABEL_ID), (1, value_id(Kind::RenderScale, 1, false).unwrap())])
        );
        // A row whose setting is off has no text.
        assert_eq!(Settings::default().texts(Row::Fps), None);
    }

    #[test]
    fn pressing_steps_the_row_and_the_text_says_restart() {
        let s = all();
        let rows = s.rows();
        let mut f = Fake::default();
        f.set_u32(ESP + 8, 0x7000);
        f.set_u16(0x7000, 1);
        let row = chosen(chosen_row(&f, ESP), &rows).unwrap();
        assert_eq!(row, Row::Fps);
        assert_eq!(s.press(row).unwrap(), "fps: 60 from the next launch");
        assert_eq!(s.texts(Row::Fps).unwrap()[1].1, value_id(Kind::Fps, 60, true).unwrap());
        s.press(Row::Fps);
        s.press(Row::Fps);
        assert_eq!(s.fps.as_ref().unwrap().shown(), UNCAPPED);
        s.press(Row::Fps);
        assert_eq!(s.texts(Row::Fps).unwrap()[1].1, value_id(Kind::Fps, 30, false).unwrap(), "back to this run's");
        assert_eq!(s.press(Row::Fov).unwrap(), "fov: 75 degrees");
        assert_eq!(s.texts(Row::Fov).unwrap()[1].1, fov_row::value_id(75));
        s.press(Row::Scale);
        assert_eq!(s.texts(Row::Scale).unwrap()[1].1, value_id(Kind::RenderScale, 2, true).unwrap());
        // Rows past ours, or no row, are not ours.
        assert_eq!(chosen(Some(3), &rows), None);
        assert_eq!(chosen(Some(-1), &rows), None);
        assert_eq!(chosen(None, &rows), None);
    }

    #[test]
    fn locked_rows_say_so_and_do_not_move() {
        let s = Settings {
            fps: Some(Arc::new(LaunchSetting::new(Kind::Fps, Some(120), None))),
            ..Settings::default()
        };
        assert_eq!(s.press(Row::Fps).unwrap(), "fps: set by H2_FPS; row locked");
        assert_eq!(s.texts(Row::Fps).unwrap()[1].1, value_id(Kind::Fps, 120, false).unwrap());
    }

    #[test]
    fn strings_are_ours_distinct_and_say_what_they_show() {
        let all = strings();
        let text = |id| all.iter().find(|(i, _)| *i == id).map(|(_, t)| t.as_str());
        assert_eq!(text(TITLE_ID), Some("Video"));
        assert_eq!(text(value_id(Kind::Fps, UNCAPPED, false).unwrap()), Some("Uncapped"));
        assert_eq!(text(value_id(Kind::Fps, 60, true).unwrap()), Some("60 (restart)"));
        assert_eq!(text(value_id(Kind::RenderScale, 6, false).unwrap()), Some("6x"));
        assert_eq!(text(EMPTY_ID), Some(""));
        let mut ids: Vec<u32> = all.iter().map(|(i, _)| *i).collect();
        assert!(ids.iter().all(|i| is_ours(*i)));
        ids.extend(fov_row::strings().iter().map(|(i, _)| *i));
        ids.extend(crate::layout_glue::served_strings(true, true).iter().map(|(i, _)| *i));
        let n = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), n, "no id is used twice");
        assert_eq!(value_id(Kind::Fps, 45, false), None);
    }
}
