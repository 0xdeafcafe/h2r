use super::*;

use std::sync::Arc;

use settings::{Store, Value};

use crate::list_class::ListClass;
use crate::setting_defs::{ALL, FOV, PAD_MUTE};
use crate::ui::RowKind;
use crate::ui_kit::input::Input;
use crate::ui_kit::page::{ListEffect, ListPage, Page};
use crate::ui_kit::spec::ScreenSpec;
use crate::video_screen::{self, SCREEN as VIDEO, Switches, VOICE_LIST, VOICE_SCREEN as VOICE};
use crate::voice_settings::{self, MIC_MUTED, VoiceHandles};

fn model_with(store: Arc<Store>, on: Switches) -> Arc<Model> {
    Arc::new(video_screen::model(store, on).unwrap())
}

fn model() -> Arc<Model> {
    model_with(Arc::new(Store::defaults(ALL)), Switches::ALL)
}

fn label(m: &Model, id: u32) -> String {
    m.strings()
        .iter()
        .find(|(i, _)| *i == id)
        .map(|(_, t)| t.clone())
        .unwrap_or_else(|| format!("{id:#x}"))
}

/// A page over our class for screen `id`, as the game steps it.
fn page(m: &Arc<Model>, id: &'static str) -> ListPage {
    let class = ListClass::new("test", id, Arc::clone(m));
    ListPage::new(class, move || ScreenSpec::new(id, id))
}

#[test]
fn the_settings_dialog_gets_video_then_voice_after_the_speaker_row() {
    let m = model();
    let kinds: Vec<_> = m
        .rows(List::GameSettings)
        .iter()
        .map(|r| r.kind.clone())
        .collect();
    assert_eq!(kinds, [RowKind::Open(VIDEO), RowKind::Open(VOICE)]);
    assert_eq!(
        FIRST_VALUE, 7,
        "after the retail values 0 to 5 and the speaker's 6"
    );
    assert_eq!(
        m.index_of(List::GameSettings, &RowKind::Open(VIDEO)),
        Some(7)
    );
    assert_eq!(
        m.index_of(List::GameSettings, &RowKind::Open(VOICE)),
        Some(8)
    );
    // The datum array grows by our rows, whatever else it grew by.
    assert_eq!(list_count(&m, SETTINGS_LIST_NAME, SETTINGS_ROWS), 8);
    assert_eq!(list_count(&m, SETTINGS_LIST_NAME, SETTINGS_ROWS + 1), 9);
    assert_eq!(
        list_count(&m, video_screen::SETTINGS_LIST_NAME, 2),
        2,
        "not the dialog's"
    );
    // What the dialog's hooks are given at install.
    let rows = settings_rows(&m);
    let got: Vec<_> = rows
        .iter()
        .map(|r| (r.value, label(&m, r.label), r.screen))
        .collect();
    assert_eq!(
        got,
        [
            (7, "Video".to_string(), VIDEO),
            (8, "Voice".to_string(), VOICE)
        ]
    );
}

#[test]
fn a_press_on_our_row_opens_our_screen_and_nothing_else() {
    let m = model();
    let press = |row| m.row(List::GameSettings, row).map(|r| m.press(r));
    assert_eq!(press(7), Some(Chosen::Open(VIDEO)));
    assert_eq!(press(8), Some(Chosen::Open(VOICE)));
    for v in [-1i16, 0, 5, 6, 9] {
        assert_eq!(press(v), None, "value {v} is not ours");
    }
}

#[test]
fn switches_take_rows_out_and_the_dialog_stays_retail() {
    let store = || Arc::new(Store::defaults(ALL));
    let none = model_with(
        store(),
        Switches {
            pause_rows: false,
            ..Switches::ALL
        },
    );
    assert!(none.rows(List::GameSettings).is_empty());
    assert!(settings_rows(&none).is_empty());
    assert_eq!(list_count(&none, SETTINGS_LIST_NAME, 7), 7);
    // No Video screen: Voice alone, at the first value after the speaker's.
    let voice_only = model_with(
        store(),
        Switches {
            video_screen: false,
            ..Switches::ALL
        },
    );
    let rows = settings_rows(&voice_only);
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0].value, rows[0].screen), (7, VOICE));
    let no_voice = model_with(
        store(),
        Switches {
            voice_screen: false,
            ..Switches::ALL
        },
    );
    assert!(no_voice.rows(VOICE_LIST).is_empty());
    let rows = settings_rows(&no_voice);
    assert_eq!(rows.iter().map(|r| r.screen).collect::<Vec<_>>(), [VIDEO]);
}

#[derive(Default)]
struct Fake(std::collections::BTreeMap<u32, u8>);
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

const SCREEN: u32 = 0x1_0000;
const LIST: u32 = SCREEN + SCREEN_LIST;
const ARRAY: u32 = 0x2_0000;
const DATA: u32 = 0x3_0000;
const ESP: u32 = 0x4_0000;
const ITEM: u32 = 0x5_0000;
const ROW_PTR: u32 = 0x6_0000;
const CHAIN: u32 = 0x7_0000;
const RECORD: u32 = 0x7_1000;

/// The Settings dialog's list with the values the constructor and our
/// hook give its datums (handle `i` for value `values[i]`) and a row
/// chosen call by player 1 for `chosen`.
fn dialog(values: &[u16], chosen: u32) -> Fake {
    let mut f = Fake::default();
    f.set_u32(LIST + LIST_ROWS, ARRAY);
    f.set_u32(ARRAY + 0x44, DATA);
    for (i, &v) in values.iter().enumerate() {
        let handle = 0xe000_0000 | i as u32;
        assert!(crate::speaker_row::set_datum_value(
            &mut f, ARRAY, handle, v
        ));
    }
    // The row chosen call: the player chain first, the row's handle second.
    f.set_u32(ESP + 4, CHAIN);
    f.set_u32(CHAIN, RECORD);
    f.set_u32(RECORD + 4, 1);
    f.set_u32(ESP + 8, ROW_PTR);
    f.set_u32(ROW_PTR, chosen);
    // The dialog's stack (window 5, slot 4, as the campaign menu opens it).
    f.set_u32(SCREEN + crate::ui_kit::lifecycle::SCREEN_WINDOW, 5);
    f.set_u32(SCREEN + crate::ui_kit::lifecycle::SCREEN_SLOT, 4);
    f
}

#[test]
fn our_datums_are_found_by_value_and_the_retail_ones_are_left_alone() {
    let m = model();
    let rows = settings_rows(&m);
    // Retail 0 to 5, the speaker's 6, then ours.
    let values = [0, 1, 2, 3, 4, 5, 6, 7, 8];
    for (handle, want) in [
        (7u32, Some(VIDEO)),
        (8, Some(VOICE)),
        (0, None),
        (5, None),
        (6, None),
    ] {
        let f = dialog(&values, 0xe000_0000 | handle);
        assert_eq!(
            chosen_row(&f, LIST, ESP, &rows).map(|r| r.screen),
            want,
            "{handle}"
        );
        assert_eq!(
            row_value(&f, LIST, 0xe000_0000 | handle),
            Some(values[handle as usize])
        );
    }
    assert_eq!(
        chosen_row(&dialog(&values, u32::MAX), LIST, ESP, &rows),
        None,
        "no row"
    );
    // A row text call: the item at [esp+4], its row's handle at +0x70.
    let mut f = dialog(&values, 0);
    f.set_u32(ESP + 4, ITEM);
    f.set_u32(ITEM + ITEM_ROW, 0xe000_0008);
    assert_eq!(
        text_row(&f, LIST, ESP, &rows)
            .map(|r| label(&m, r.label))
            .as_deref(),
        Some("Voice")
    );
    f.set_u32(ITEM + ITEM_ROW, 0xe000_0002);
    assert_eq!(text_row(&f, LIST, ESP, &rows), None);
    // Without a speaker row our values still start at 7, wherever their
    // datums are.
    let f = dialog(&[0, 1, 2, 3, 4, 5, 7, 8], 0xe000_0006);
    assert_eq!(
        chosen_row(&f, LIST, ESP, &rows).map(|r| r.screen),
        Some(VIDEO)
    );
}

#[test]
fn our_screen_opens_on_the_dialogs_own_stack_for_the_pressing_player() {
    let f = dialog(&[0, 1, 2, 3, 4, 5, 6, 7, 8], 0xe000_0007);
    let loader = 0xa0f0_7000;
    assert_eq!(
        open_request(&f, LIST, ESP, loader),
        Some(crate::skulls_mp::open_request(1 << 1, 5, 4, loader))
    );
    // No player record: not opened.
    let mut f = f;
    f.set_u32(RECORD + 4, 9);
    assert_eq!(open_request(&f, LIST, ESP, loader), None);
}

#[test]
fn the_voice_screen_lists_every_voice_setting_in_order() {
    let m = model();
    let keys: Vec<_> = m.rows(VOICE_LIST).iter().map(|r| r.kind.clone()).collect();
    let want: Vec<_> = voice_settings::ALL
        .iter()
        .map(|d| RowKind::Setting(d.key))
        .collect();
    assert_eq!(keys, want);
    assert_eq!(label(&m, m.title_id(VOICE).unwrap()), "Voice");
    // And none of them is on the Video screen.
    assert!(
        !m.rows(video_screen::LIST)
            .iter()
            .any(|r| matches!(r.kind, RowKind::Setting(k) if k.starts_with("voice.")))
    );
}

/// The Voice screen as the game steps it: the focus moves, A steps a row
/// and the handle the audio side holds sees it at once, B closes.
#[test]
fn the_voice_page_steps_mutes_live_and_closes_on_b() {
    let store = Arc::new(Store::defaults(ALL));
    let audio_side = VoiceHandles::new(&store);
    let m = model_with(Arc::clone(&store), Switches::ALL);
    let p = page(&m, VOICE);
    let mut focus = p.states()[0].1;
    // Microphone: with no host list the only choice is System default.
    assert_eq!(
        p.step(&mut focus, Input::A),
        ListEffect::Stepped("voice.input_device: default".into())
    );
    for _ in 0..4 {
        p.step(&mut focus, Input::Down);
    }
    assert_eq!(focus.0, 4, "Mute My Mic");
    assert_eq!(
        p.step(&mut focus, Input::A),
        ListEffect::Stepped("voice.mic_muted: true".into())
    );
    assert!(
        audio_side.mic_muted.get(),
        "live, through the one atomic cell"
    );
    assert_eq!(audio_side.capture_gain(), 0.0);
    let row = m.row(VOICE_LIST, 4).unwrap();
    assert_eq!(label(&m, m.texts(row)[1].1), "On");
    p.step(&mut focus, Input::Down);
    assert_eq!(
        p.step(&mut focus, Input::A),
        ListEffect::Stepped("voice.incoming_muted: true".into())
    );
    assert!(audio_side.incoming_muted.get());
    // Past the last row the focus stays.
    for _ in 0..5 {
        p.step(&mut focus, Input::Down);
    }
    assert_eq!(focus.0, 7);
    assert_eq!(
        p.step(&mut focus, Input::A),
        ListEffect::Stepped("voice.output_volume: 0".into()),
        "100% wraps to 0%"
    );
    assert_eq!(audio_side.output_volume.get(), 0);
    assert_eq!(p.step(&mut focus, Input::B), ListEffect::Close);
    assert_eq!(p.step(&mut focus, Input::Back), ListEffect::Close);
    assert_eq!(store.get(MIC_MUTED.key), Some(Value::Bool(true)));
}

/// The Video screen opened from the pause menu is the same screen: a
/// press applies at once.
#[test]
fn the_video_page_from_the_pause_menu_applies_at_once() {
    let store = Arc::new(Store::defaults(ALL));
    let fov = store.handle::<u32>(FOV.key);
    let mute = store.handle::<bool>(PAD_MUTE.key);
    let m = model_with(Arc::clone(&store), Switches::ALL);
    assert_eq!(
        m.press(m.row(List::GameSettings, 7).unwrap()),
        Chosen::Open(VIDEO)
    );
    let p = page(&m, VIDEO);
    let mut focus = p.states()[0].1;
    assert_eq!(
        p.step(&mut focus, Input::A),
        ListEffect::Stepped("graphics.fov: 75".into())
    );
    assert_eq!(fov.get(), 75);
    for _ in 0..4 {
        p.step(&mut focus, Input::Down);
    }
    assert_eq!(
        p.step(&mut focus, Input::A),
        ListEffect::Stepped("pad.mute_button: false".into())
    );
    assert!(!mute.get());
    assert_eq!(p.step(&mut focus, Input::B), ListEffect::Close);
}

/// The pause rule (`0x1482c4`) over the screens a player opens and closes:
/// paused while the pause menu or a screen of ours over it is up, resumed
/// only when the last one closes. Nothing of ours writes the byte.
#[test]
fn a_screen_over_the_pause_menu_keeps_the_game_paused_until_the_last_closes() {
    let m = model();
    let mut stack: Vec<&'static str> = Vec::new();
    fn update(stack: &[&'static str], paused: &mut bool) {
        if resumes(*paused, !stack.is_empty()) {
            *paused = false;
        }
    }
    // Start: the campaign pause menu is made and pauses; its Settings
    // closes it and opens the dialog.
    stack.push("settings");
    let mut paused = true;
    update(&stack, &mut paused);
    assert!(paused);
    for row in [7i16, 8] {
        let Chosen::Open(id) = m.press(m.row(List::GameSettings, row).unwrap()) else {
            panic!("row {row}")
        };
        stack.push(id);
        update(&stack, &mut paused);
        assert!(paused, "{id} up");
        let p = page(&m, id);
        let mut focus = p.states()[0].1;
        assert_eq!(p.step(&mut focus, Input::B), ListEffect::Close);
        stack.pop();
        update(&stack, &mut paused);
        assert!(paused, "back on Settings from {id}");
        assert_eq!(stack, ["settings"]);
    }
    stack.pop();
    update(&stack, &mut paused);
    assert!(!paused, "resumed when Settings closes");
    // Multiplayer: never paused, before, during or after.
    assert!(!resumes(false, true) && !resumes(false, false));
}

/// With a host of many devices the model still has a string id for every
/// text: twelve of each kind at most.
#[test]
fn a_host_full_of_devices_fits_the_models_string_ids() {
    use audio::voice::Device;
    let many = |what: &str| -> Vec<Device> {
        (0..30)
            .map(|i| Device::new(format!("{what}{i}"), format!("{what} device number {i}")))
            .collect()
    };
    let input: &'static settings::Def = Box::leak(Box::new(voice_settings::device_def(
        &voice_settings::INPUT_DEVICE,
        &many("in"),
    )));
    let output: &'static settings::Def = Box::leak(Box::new(voice_settings::device_def(
        &voice_settings::OUTPUT_DEVICE,
        &many("out"),
    )));
    let defs: Vec<&'static settings::Def> = ALL
        .iter()
        .map(|d| match d.key {
            k if k == input.key => input,
            k if k == output.key => output,
            _ => *d,
        })
        .collect();
    let m = video_screen::model(Arc::new(Store::defaults(&defs)), Switches::ALL)
        .expect("every text has an id");
    assert!(
        m.strings()
            .iter()
            .all(|(i, _)| video_screen::is_model_id(*i))
    );
    assert!(
        m.strings()
            .iter()
            .any(|(i, _)| video_screen::MORE_MODEL_IDS.contains(i)),
        "the devices spill into the second family"
    );
    let mic = m.row(VOICE_LIST, 0).unwrap();
    assert_eq!(label(&m, m.texts(mic)[1].1), "System default");
    m.press(mic);
    assert_eq!(label(&m, m.texts(mic)[1].1), "in device number 0");
}
