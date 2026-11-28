use super::*;

use std::sync::Arc;

use guest_layout::testing::FakeGuest;
use settings::{Store, Value};

use crate::list_class::ListClass;
use crate::setting_defs::{ALL, FOV, PAD_MUTE};
use crate::ui::{Chosen, Model, RowKind};
use crate::ui_kit::input::Input;
use crate::ui_kit::page::{Focus, ListEffect, ListPage, Page as PageModel};
use crate::ui_kit::spec::ScreenSpec;
use crate::video_screen::{self, Switches, VOICE_LIST};
use crate::voice_settings::{self, MIC_MUTED, VoiceHandles};

fn model() -> Arc<Model> {
    Arc::new(video_screen::model(Arc::new(Store::defaults(ALL)), Switches::ALL).unwrap())
}

fn label(m: &Model, id: u32) -> String {
    if let Some((_, text)) = TEXTS.iter().find(|(i, _)| *i == id) {
        return (*text).into();
    }
    m.strings()
        .iter()
        .find(|(i, _)| *i == id)
        .unwrap_or_else(|| panic!("no string for {id:#x}"))
        .1
        .clone()
}

fn page(m: &Arc<Model>, id: &'static str) -> ListPage {
    let class = ListClass::new("test", id, Arc::clone(m)).lines();
    ListPage::new(class, move || ScreenSpec::new(id, id))
}

const LIST: u32 = 0x1000;
const ARRAY: u32 = 0x2000;
const NAME: u32 = 0x3000;
const ITEM: u32 = 0x4000;
const ROW: u32 = 0x5000;

fn guest(capacity: u32, actions: &[u16]) -> (FakeGuest, Vec<DatumHandle>) {
    let mut g = FakeGuest::new();
    g.map(LIST, 0x100).map(ARRAY, 0x1000).map(NAME, 0x20);
    g.map(ITEM, 0x80).map(ROW, 4);
    ListHandler::at(LIST).set_rows(&mut g, ARRAY).unwrap();
    let rows = DatumArray::at(ARRAY);
    rows.initialize_contiguous(&mut g, NAME, capacity as i32, 4, 0, 0)
        .unwrap();
    rows.make_valid(&mut g).unwrap();
    let handles = actions
        .iter()
        .map(|&action| {
            let handle = rows.new_datum(&mut g).unwrap().unwrap();
            let datum = rows.try_get(&g, handle).unwrap().unwrap();
            g.set_u16(datum + ACTION_WORD, action).unwrap();
            handle
        })
        .collect();
    (g, handles)
}

#[test]
fn player_settings_is_labelled_at_the_retail_index_or_action() {
    let (mut g, handles) = guest(6, &[0, 2, 1, 5]);
    for (index, handle) in handles.into_iter().enumerate() {
        Item::at(ITEM).set_row(&mut g, Some(handle)).unwrap();
        g.put_u32(ROW, handle.raw());
        assert_eq!(
            settings_label(&g, Mode::Campaign, LIST, ITEM),
            (index == 3).then_some(PLAYER)
        );
        assert_eq!(campaign_settings_pressed(&g, ROW), index == 3);
        assert_eq!(
            settings_label(&g, Mode::Multiplayer, LIST, ITEM),
            (index == 2).then_some(PLAYER)
        );
    }
    g.put_u32(ROW, u32::MAX);
    assert!(!campaign_settings_pressed(&g, ROW));
}

#[test]
fn pause_settings_keeps_the_native_multiplayer_press() {
    let mut registry = hooks::Registry::new(hooks::Switches::defaults());
    for mode in [Mode::Campaign, Mode::Multiplayer] {
        assert!(registry.add(&Settings(mode)).unwrap());
    }
    let plan = registry.build(&hooks::Base).unwrap().text();
    for line in [
        "PAUSE_ROW_TEXT: before [pause-rows:name the Player Settings row] -> original",
        "PAUSE_ROW_CHOSEN: before [pause-rows:open Player Settings] -> original",
        "MP_MENU_ROW_TEXT: before [pause-mp-rows:name the Player Settings row] -> original",
    ] {
        assert!(plan.contains(line), "{line}\n{plan}");
    }
    assert!(!plan.contains("MP_MENU_ROW_CHOSEN"));
}

#[test]
fn either_native_list_labels_and_opens_video_and_voice_after_its_own_rows() {
    let m = model();
    let pages = Pages::new(true, true, &hooks::Switches::defaults());
    // The native player list can omit retail rows. Our labels and presses
    // both follow the action word rather than the resulting row index.
    let mut controller: Vec<u16> = (0..6).collect();
    controller.extend(speaker_row::small_values(true, true, true));
    for (list, kind, retail) in [
        (NativeList::Controller, 0, controller.as_slice()),
        (NativeList::Controller, 1, &[0, 1, 2, 3, 4, 5][..]),
        (NativeList::Player, 1, &[0, 1, 2, 3][..]),
        (NativeList::Player, 1, &[0, 3][..]),
    ] {
        let (array_name, retail_capacity) = list.array();
        assert!(NativeList::from_array(array_name, retail_capacity).is_some());
        assert!(NativeList::from_array(array_name, retail_capacity + 1).is_none());
        let extras: Vec<_> = pages.for_game(list, Some(kind)).collect();
        let mut actions = retail.to_vec();
        actions.extend(extras.iter().map(|page| page.value()));
        let (mut g, handles) = guest(retail.len() as u32 + extras.len() as u32, &actions);
        for (index, handle) in handles.into_iter().enumerate() {
            Item::at(ITEM).set_row(&mut g, Some(handle)).unwrap();
            g.put_u32(ROW, handle.raw());
            let pressed = pages.page(list, Some(kind), chosen_action(&g, LIST, ROW));
            let shown = pages.page(list, Some(kind), item_action(&g, LIST, ITEM));
            assert_eq!(pressed, shown);
            match index.checked_sub(retail.len()) {
                Some(i) => {
                    let page = pressed.unwrap();
                    assert_eq!(page.screen(), [VIDEO, VOICE][i]);
                    assert_eq!(
                        label(&m, m.title_id(page.screen()).unwrap()),
                        ["Video", "Voice"][i]
                    );
                }
                None => assert_eq!(pressed, None),
            }
        }
        g.put_u32(ROW, u32::MAX);
        assert_eq!(chosen_action(&g, LIST, ROW), None);
    }
}

#[test]
fn settings_rows_require_their_class_and_the_correct_pause_feature() {
    let switches = hooks::Switches::defaults().with("pause-mp-rows", false);
    let pages = Pages::new(false, true, &switches);
    let options = |list, kind| pages.for_game(list, Some(kind)).collect::<Vec<_>>();
    assert_eq!(options(NativeList::Controller, 0), [Page::Voice]);
    assert!(options(NativeList::Controller, 1).is_empty());
    assert!(options(NativeList::Controller, 2).is_empty());
    assert!(options(NativeList::Player, 1).is_empty());
    assert!(options(NativeList::Player, 0).is_empty());
    assert!(
        !Pages::new(false, false, &hooks::Switches::defaults()).enabled(NativeList::Controller)
    );
}

#[test]
fn the_pause_rows_are_audited_for_snapshots() {
    let mut registry = hooks::Registry::new(hooks::Switches::defaults());
    for mode in [Mode::Campaign, Mode::Multiplayer] {
        assert!(registry.add(&Settings(mode)).unwrap());
    }
    let plan = registry.build(&hooks::Base).unwrap();
    // They draw their rows from guest memory and keep no host state, so
    // they must not forbid every keyframe for the whole session.
    assert!(plan.snapshot_blockers().is_empty(), "{:?}", plan.snapshot_blockers());
}

#[test]
fn voice_contains_mute_button_and_uses_the_live_host_settings() {
    let m = model();
    let keys: Vec<_> = m.rows(VOICE_LIST).iter().map(|r| r.kind.clone()).collect();
    let mut want: Vec<_> = voice_settings::ALL
        .iter()
        .map(|d| RowKind::Setting(d.key))
        .collect();
    want.push(RowKind::Setting(PAD_MUTE.key));
    assert_eq!(keys, want);
    let row = m.row(VOICE_LIST, 8).unwrap();
    let pad_mute = m.store().handle::<bool>(PAD_MUTE.key);
    assert_eq!(m.line(row), "Mute Button: On");
    assert_eq!(
        m.press(row),
        Chosen::Stepped("voice.mute_button: false".into())
    );
    assert_eq!(m.line(row), "Mute Button: Off");
    assert!(!pad_mute.get());

    let audio = VoiceHandles::new(m.store());
    let p = page(&m, VOICE);
    let mut focus = Focus(6);
    assert_eq!(
        p.step(&mut focus, Input::A),
        ListEffect::Stepped("voice.mute_microphone: true".into())
    );
    assert_eq!(m.store().get(MIC_MUTED.key), Some(Value::Bool(true)));
    assert_eq!(audio.capture_gain(), 0.0);
    assert_eq!(m.line(m.row(VOICE_LIST, 6).unwrap()), "Mute Microphone: On");
    assert_eq!(
        p.step(&mut Focus(5), Input::A),
        ListEffect::Stepped("voice.volume: 0".into())
    );
    assert_eq!(audio.output_volume.get(), 0);
}

#[test]
fn video_still_steps_display_settings_live() {
    let m = model();
    let p = page(&m, VIDEO);
    let mut focus = Focus(2);
    assert_eq!(
        p.step(&mut focus, Input::A),
        ListEffect::Stepped("graphics.fov: 75".into())
    );
    assert_eq!(m.store().handle::<u32>(FOV.key).get(), 75);
    assert_eq!(
        m.line(m.row(video_screen::LIST, 2).unwrap()),
        "Field of View: 75"
    );
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
    assert_eq!(label(&m, m.texts(mic)[1].1), "System Default");
    m.press(mic);
    assert_eq!(label(&m, m.texts(mic)[1].1), "in device number 0");
}

#[test]
fn the_forge_driver_leaves_the_multiplayer_row_text_to_player_settings() {
    // GAME MENU keeps the retail rows in a Forge game: the Forge driver
    // names none of them (forge_screens' game_menu_has_the_retail_rows),
    // so Player Settings is the only plan entry at MP_MENU_ROW_TEXT and
    // the plan needs no order between the two.
    let mut registry = hooks::Registry::new(hooks::Switches::defaults());
    assert!(registry.add(&Settings(Mode::Multiplayer)).unwrap());
    crate::forge_screens::install(&mut registry).unwrap();
    let plan = registry.build(&hooks::Base).unwrap().text();
    let line = plan
        .lines()
        .find(|l| l.contains(" MP_MENU_ROW_TEXT: "))
        .unwrap_or_else(|| panic!("no MP_MENU_ROW_TEXT line\n{plan}"));
    assert!(line.contains("pause-mp-rows:"), "{line}");
    assert!(!line.contains("forge-screens:"), "{line}");
}
