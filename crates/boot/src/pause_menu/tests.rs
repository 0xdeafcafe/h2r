use super::*;

use std::sync::Arc;

use engine::testing::FakeGuest;
use settings::{Store, Value};

use crate::list_class::ListClass;
use crate::retail_rows;
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

/// Video then Voice, below Controller Settings: the campaign menu's rows
/// 4 and 5 (its row 4 shows at 6), the multiplayer menu's actions 6 and 7.
#[test]
fn both_pause_menus_get_video_then_voice_below_controller_settings() {
    let m = model();
    for (list, retail, first) in [
        (List::Pause, PAUSE_ROWS, PAUSE_CONTROLLER_ROW + 1),
        (List::MpPause, MP_ACTIONS, MP_ACTIONS as i16),
    ] {
        let kinds: Vec<_> = m.rows(list).iter().map(|r| r.kind.clone()).collect();
        assert_eq!(kinds, [RowKind::Open(VIDEO), RowKind::Open(VOICE)], "{list:?}");
        assert_eq!(m.count(list, retail), retail + 2, "{list:?}");
        assert_eq!(m.count(list, retail + 1), retail + 1, "not the list we know");
        assert_eq!(m.index_of(list, &RowKind::Open(VIDEO)), Some(first));
        assert_eq!(m.index_of(list, &RowKind::Open(VOICE)), Some(first + 1));
    }
    // The campaign menu by index: rows 0 to 3 retail's, ours, then row 4.
    let shows: Vec<Option<i16>> = (0..8).map(|i| m.retail_index(List::Pause, i)).collect();
    assert_eq!(shows, [Some(0), Some(1), Some(2), Some(3), None, None, Some(4), None]);
    assert_eq!(retail_rows::moved(&m, List::Pause, 6), Some(4));
    for i in [0i16, 3, 4, 5, 7] {
        assert_eq!(retail_rows::moved(&m, List::Pause, i), None, "{i}");
    }
    assert_eq!(list_count(&m, PAUSE_LIST_NAME, 5), 7);
    assert_eq!(list_count(&m, MP_LIST_NAME, 6), 8);
    assert_eq!(list_count(&m, video_screen::SETTINGS_LIST_NAME, 2), 2, "not a pause list");
    assert_eq!(mp_actions(&m), [6, 7]);
    for a in [-1i16, 0, 5, 8] {
        assert!(!is_ours_mp(&m, a), "{a}");
    }
    assert!(is_ours_mp(&m, 6) && is_ours_mp(&m, 7));
}

#[test]
fn our_rows_draw_their_labels_and_the_retail_rows_are_left_alone() {
    let m = model();
    let text = |list, row| {
        retail_rows::texts(&m, list, row, true).map(|t| {
            assert_eq!(t.len(), 1, "one text: the label");
            label(&m, t[0].1)
        })
    };
    assert_eq!(text(List::Pause, 4).as_deref(), Some("Video"));
    assert_eq!(text(List::Pause, 5).as_deref(), Some("Voice"));
    assert_eq!(text(List::MpPause, 6).as_deref(), Some("Video"));
    assert_eq!(text(List::MpPause, 7).as_deref(), Some("Voice"));
    for r in [-1i16, 0, 1, 2, 3, 6, 7] {
        assert_eq!(text(List::Pause, r), None, "campaign row {r}");
    }
    for a in [0i16, 1, 2, 3, 4, 5, 8] {
        assert_eq!(text(List::MpPause, a), None, "multiplayer action {a}");
    }
}

#[test]
fn a_press_on_our_row_opens_our_screen_and_nothing_else() {
    let m = model();
    let press = |list, row| m.row(list, row).map(|r| m.press(r));
    assert_eq!(press(List::Pause, 4), Some(Chosen::Open(VIDEO)));
    assert_eq!(press(List::Pause, 5), Some(Chosen::Open(VOICE)));
    assert_eq!(press(List::MpPause, 6), Some(Chosen::Open(VIDEO)));
    assert_eq!(press(List::MpPause, 7), Some(Chosen::Open(VOICE)));
    assert_eq!(press(List::Pause, 6), None, "retail's row 4, moved down");
    assert_eq!(press(List::MpPause, 5), None, "action 5 is retail's");
}

#[test]
fn switches_take_rows_out_and_the_lists_stay_retail() {
    let store = || Arc::new(Store::defaults(ALL));
    let none = model_with(
        store(),
        Switches {
            pause_rows: false,
            ..Switches::ALL
        },
    );
    assert!(none.rows(List::Pause).is_empty() && none.rows(List::MpPause).is_empty());
    assert_eq!(list_count(&none, PAUSE_LIST_NAME, 5), 5);
    assert_eq!(list_count(&none, MP_LIST_NAME, 6), 6);
    assert!(mp_actions(&none).is_empty());
    // No Video screen: Voice alone, at the first place after retail's.
    let voice_only = model_with(
        store(),
        Switches {
            video_screen: false,
            ..Switches::ALL
        },
    );
    assert_eq!(voice_only.count(List::Pause, 5), 6);
    assert_eq!(mp_actions(&voice_only), [6]);
    assert_eq!(
        voice_only.row(List::MpPause, 6).map(|r| r.kind.clone()),
        Some(RowKind::Open(VOICE))
    );
    let no_voice = model_with(
        store(),
        Switches {
            voice_screen: false,
            ..Switches::ALL
        },
    );
    assert!(no_voice.rows(VOICE_LIST).is_empty());
    assert_eq!(
        no_voice
            .rows(List::Pause)
            .iter()
            .map(|r| r.kind.clone())
            .collect::<Vec<_>>(),
        [RowKind::Open(VIDEO)]
    );
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
    assert!(audio_side.mic_muted.get(), "live, through the one atomic cell");
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
    assert_eq!(m.press(m.row(List::Pause, 4).unwrap()), Chosen::Open(VIDEO));
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
    // Start: the campaign pause menu is made and pauses.
    stack.push("pause");
    let mut paused = true;
    update(&stack, &mut paused);
    assert!(paused);
    for row in [4i16, 5] {
        let Chosen::Open(id) = m.press(m.row(List::Pause, row).unwrap()) else {
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
        assert!(paused, "back on the pause menu from {id}");
        assert_eq!(stack, ["pause"]);
    }
    stack.pop();
    update(&stack, &mut paused);
    assert!(!paused, "resumed when the pause menu closes");
    // Multiplayer: never paused, before, during or after.
    assert!(!resumes(false, true) && !resumes(false, false));
}

/// A multiplayer pause list at `LIST` whose array has room for `room` rows,
/// with `actions` added as the constructor adds them.
fn mp_list(room: i32, actions: &[i16]) -> FakeGuest {
    const LIST: u32 = 0x1000;
    const ARRAY: u32 = 0x2000;
    const NAME: u32 = 0x1800;
    let mut g = FakeGuest::new();
    g.map(LIST, 0x100).map(NAME, 0x20).map(ARRAY, 0x1000);
    g.put_u32(LIST + LIST_ROWS, ARRAY);
    let a = DatumArray::at(ARRAY);
    a.initialize_contiguous(&mut g, NAME, room, 4, 0, 0).unwrap();
    a.make_valid(&mut g).unwrap();
    for &act in actions {
        let h = a.new_datum(&mut g).unwrap().unwrap();
        let e = a.try_get(&g, h).unwrap().unwrap();
        g.set_u16(e + ROW_ACTION, act as u16).unwrap();
    }
    g
}

#[test]
fn our_actions_are_added_once_after_the_games_and_read_back_by_handle() {
    const LIST: u32 = 0x1000;
    let mut g = mp_list(8, &[0, 1, 2, 4]);
    assert_eq!(actions(&g, LIST).unwrap(), [0, 1, 2, 4]);
    assert_eq!(add_actions(&mut g, LIST, &[6, 7]).unwrap(), [6, 7]);
    assert_eq!(actions(&g, LIST).unwrap(), [0, 1, 2, 4, 6, 7]);
    assert!(add_actions(&mut g, LIST, &[6, 7]).unwrap().is_empty(), "once");
    let a = DatumArray::at(g.u32(LIST + LIST_ROWS).unwrap());
    let h = a.handle_of(&g, 4).unwrap().unwrap();
    assert_eq!(row_action(&g, LIST, h.0), Some(6));
    assert_eq!(row_action(&g, LIST, u32::MAX), None, "no row");
    // The game's six-row room, full: refused, and said.
    let mut full = mp_list(6, &[0, 1, 2, 3, 4, 5]);
    assert!(add_actions(&mut full, LIST, &[6]).unwrap_err().contains("full"));
}

#[test]
fn the_hooks_are_on_the_plan_before_the_originals() {
    let m = model();
    let mut r = feature::Registry::new(feature::Switches::defaults());
    r.add(&retail_rows::Extension {
        id: FEATURE,
        title: "Our rows on the campaign pause menu",
        list: List::Pause,
        text: &engine::points::PAUSE_ROW_TEXT,
        chosen: &engine::points::PAUSE_ROW_CHOSEN,
        label_only: true,
        model: Arc::clone(&m),
    })
    .unwrap();
    r.add(&MpPauseRows { model: m }).unwrap();
    let text = r.build(&feature::Base).unwrap().text();
    for line in [
        "0x00231f6c PAUSE_ROW_TEXT: before [pause-rows:our rows' text] -> original\n",
        "0x00231fe3 PAUSE_ROW_CHOSEN: before [pause-rows:our rows' press] -> original\n",
        "0x00232708 MP_MENU_LIST_CTOR: before [pause-mp-rows:note the pause list] -> original -> after [pause-mp-rows:add our pause rows]\n",
        "0x0023284e MP_MENU_ROW_TEXT: before [pause-mp-rows:our pause rows' text] -> original\n",
        "0x002328b5 MP_MENU_ROW_CHOSEN: before [pause-mp-rows:our pause rows' press] -> original\n",
    ] {
        assert!(text.contains(line), "{line}\n{text}");
    }
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
    assert!(m.strings().iter().all(|(i, _)| video_screen::is_model_id(*i)));
    assert!(
        m.strings().iter().any(|(i, _)| video_screen::MORE_MODEL_IDS.contains(i)),
        "the devices spill into the second family"
    );
    let mic = m.row(VOICE_LIST, 0).unwrap();
    assert_eq!(label(&m, m.texts(mic)[1].1), "System default");
    m.press(mic);
    assert_eq!(label(&m, m.texts(mic)[1].1), "in device number 0");
}


/// The multiplayer menu's rows go by their order in the datum array: ours
/// are added last, then the action words are written again so they sit
/// below Controller Settings (action 1). A menu without action 1 keeps
/// them last.
#[test]
fn our_multiplayer_rows_move_below_controller_settings() {
    assert_eq!(ordered(&[0, 1, 2, 3, 4, 5, 6, 7], &[6, 7], 1), [0, 1, 6, 7, 2, 3, 4, 5]);
    assert_eq!(ordered(&[0, 1, 2, 6, 7], &[6, 7], 1), [0, 1, 6, 7, 2]);
    assert_eq!(ordered(&[0, 1, 6, 7, 2], &[6, 7], 1), [0, 1, 6, 7, 2], "once");
    assert_eq!(ordered(&[0, 2, 6, 7], &[6, 7], 1), [0, 2, 6, 7], "no action 1");
    assert_eq!(ordered(&[0, 1, 2, 7], &[6, 7], 1), [0, 1, 7, 2], "Voice alone");

    const LIST: u32 = 0x1000;
    let mut g = mp_list(8, &[0, 1, 2, 3]);
    assert_eq!(add_actions(&mut g, LIST, &[6, 7]).unwrap(), [6, 7]);
    assert_eq!(actions(&g, LIST).unwrap(), [0, 1, 2, 3, 6, 7]);
    assert_eq!(
        order_actions(&mut g, LIST, &[6, 7], MP_CONTROLLER_ACTION).unwrap(),
        [0, 1, 6, 7, 2, 3]
    );
    assert_eq!(actions(&g, LIST).unwrap(), [0, 1, 6, 7, 2, 3]);
    // Each row still answers by its action: the third row is Video's.
    let a = DatumArray::at(g.u32(LIST + LIST_ROWS).unwrap());
    let third = a.handle_of(&g, 2).unwrap().unwrap();
    assert_eq!(row_action(&g, LIST, third.0), Some(6));
}
