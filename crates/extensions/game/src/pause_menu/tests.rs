use super::*;

use engine::entry::{Regs, testing};
use engine::testing::FakeGuest;
use settings::{Store, Value};

use crate::setting_defs::{ALL, FOV, PAD_MUTE};
use crate::ui::RowKind;
use crate::ui_kit::input::Input;
use crate::ui_kit::page::{Focus, ListEffect, ListPage, Page, StackPage};
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

#[test]
fn both_settings_lists_show_native_options_video_and_voice() {
    let m = model();
    for (mode, retail) in [
        (Mode::Campaign, "Controller Settings"),
        (Mode::Multiplayer, "Player Settings"),
    ] {
        let class = Settings::new(mode, &m, true, true).unwrap().class(0);
        let behaviour = class.behaviour();
        let rows = behaviour.rows;
        assert_eq!(label(&m, rows.title()), "Settings");
        assert_eq!(rows.count(), 3);
        let rendered = [0, 1, 2].map(|i| {
            let texts = rows.texts(i);
            assert_eq!(texts.len(), 1);
            assert_eq!(texts[0].0, 0);
            label(&m, texts[0].1)
        });
        assert_eq!(rendered, [retail, "Video", "Voice"]);
        assert_eq!(rows.press(0), Chosen::Act);
        assert_eq!(rows.press(1), Chosen::Push(VIDEO));
        assert_eq!(rows.press(2), Chosen::Push(VOICE));
        assert_eq!(rows.press(3), Chosen::Nothing);
        assert!(rows.texts(3).is_empty());
        assert!(behaviour.action_buttons.is_empty());
    }
}

#[test]
fn settings_only_links_to_installed_pages() {
    let m = model();
    for mode in [Mode::Campaign, Mode::Multiplayer] {
        assert!(Settings::new(mode, &m, false, false).is_none());
        for (video, voice, expected) in [(true, false, VIDEO), (false, true, VOICE)] {
            let class = Settings::new(mode, &m, video, voice).unwrap().class(0);
            let rows = class.behaviour().rows;
            assert_eq!(rows.count(), 2);
            assert_eq!(rows.press(1), Chosen::Push(expected));
        }
    }
}

#[test]
fn back_from_video_and_voice_reveals_settings_at_the_same_row() {
    let m = model();
    for mode in [Mode::Campaign, Mode::Multiplayer] {
        let id = mode.screen();
        let class = Settings::new(mode, &m, true, true).unwrap().class(0);
        let settings = ListPage::new(class, || ScreenSpec::new("settings", "Settings"));
        let stack = StackPage::new(id, settings)
            .screen(VIDEO, page(&m, VIDEO))
            .screen(VOICE, page(&m, VOICE));
        let mut state = stack.after(&[Input::Down, Input::A]);
        assert_eq!(StackPage::top(&state), Some(VIDEO));
        assert_eq!(stack.step(&mut state, Input::B), ListEffect::Close);
        assert_eq!(state.open, [(id, Focus(1))]);
        assert_eq!(stack.step(&mut state, Input::Down), ListEffect::Focus(2));
        assert_eq!(stack.step(&mut state, Input::A), ListEffect::Open(VOICE));
        assert_eq!(StackPage::top(&state), Some(VOICE));
        assert_eq!(stack.step(&mut state, Input::Back), ListEffect::Close);
        assert_eq!(state.open, [(id, Focus(2))]);
    }
}

#[test]
fn the_campaign_hook_relabels_and_redirects_only_settings() {
    let m = model();
    let mut g = FakeGuest::new();
    let item = 0x1000;
    let arg = 0x2000;
    g.map(item, 0x100).map(arg, 4);
    for index in [0, 1, 2, 3, 4] {
        let handle = DatumHandle::new(0x8000, index);
        Item::at(item).set_row(&mut g, Some(handle)).unwrap();
        g.put_u32(arg, handle.raw());
        let text = campaign_row_label(&g, item).map(|id| label(&m, id));
        assert_eq!(text.as_deref(), (index == 3).then_some("Settings"));
        assert_eq!(settings_pressed(&g, 0, arg, Mode::Campaign), index == 3);
    }
    g.put_u32(arg, u32::MAX);
    assert!(!settings_pressed(&g, 0, arg, Mode::Campaign));
}

const OVER: u32 = 0x1000;
const MP_PARENT: u32 = 0x2000;
const MP_LIST: u32 = MP_PARENT + ui::LIST;
const ARRAY: u32 = 0x3000;
const NAME: u32 = 0x4000;
const STACK: u32 = 0x5000;
const ROW: u32 = 0x5010;

fn mp_list(actions: &[u16]) -> (FakeGuest, Vec<DatumHandle>) {
    let mut g = FakeGuest::new();
    g.map(OVER, 0x100).map(MP_PARENT, 0x1000).map(ARRAY, 0x1000);
    g.map(NAME, 0x20).map(STACK, 0x100);
    g.put_u32(OVER + lifecycle::SCREEN_NEXT, MP_PARENT);
    Screen::at(MP_PARENT).set_id(&mut g, MP_PAUSE).unwrap();
    ListHandler::at(MP_LIST).set_rows(&mut g, ARRAY).unwrap();
    let rows = DatumArray::at(ARRAY);
    rows.initialize_contiguous(&mut g, NAME, MP_ROWS, 4, 0, 0)
        .unwrap();
    rows.make_valid(&mut g).unwrap();
    let handles = actions
        .iter()
        .map(|action| {
            let h = rows.new_datum(&mut g).unwrap().unwrap();
            let e = rows.try_get(&g, h).unwrap().unwrap();
            g.set_u16(e + ACTION_WORD, *action).unwrap();
            h
        })
        .collect();
    (g, handles)
}

#[test]
fn multiplayer_uses_the_action_word_and_preserves_the_native_settings_press() {
    let (mut g, handles) = mp_list(&[0, 2, 1, 5]);
    for (i, handle) in handles.iter().enumerate() {
        g.put_u32(ROW, handle.raw());
        assert_eq!(
            settings_pressed(&g, MP_LIST, ROW, Mode::Multiplayer),
            i == 2
        );
    }
    assert_eq!(mp_settings(&g, OVER), Some((MP_LIST, handles[2])));
    assert_eq!(mp_settings(&mp_list(&[0, 2, 5]).0, OVER), None);

    let chain = 0x6000;
    g.put_u32(STACK + 4, chain);
    let entry = |cx| -> EntryFuture {
        Box::pin(async move {
            open_retail_settings(cx, OVER, Mode::Multiplayer, ROW).await?;
            Ok(Ret::new(0, 8))
        })
    };
    let run = testing::run(
        &entry,
        g,
        Regs {
            esp: STACK,
            ..Regs::default()
        },
        |_, _| Regs::default(),
    );
    assert_eq!(run.result, Ok(Ret::new(0, 8)));
    assert_eq!(
        run.calls,
        [GuestCall::new(points::MP_MENU_ROW_CHOSEN.base)
            .ecx(MP_LIST)
            .args([chain, ROW])]
    );
    assert_eq!(run.guest.u32(ROW), Ok(handles[2].raw()));
}

#[test]
fn both_pause_hooks_redirect_only_the_existing_settings_row() {
    let m = model();
    let mut r = hooks::Registry::new(hooks::Switches::defaults());
    for mode in [Mode::Campaign, Mode::Multiplayer] {
        assert!(
            r.add(&Settings::new(mode, &m, true, true).unwrap())
                .unwrap()
        );
    }
    let text = r.build(&hooks::Base).unwrap().text();
    for line in [
        "0x00231f6c PAUSE_ROW_TEXT: before [pause-rows:name the Settings row] -> original",
        "0x00231fe3 PAUSE_ROW_CHOSEN: before [pause-rows:open Settings] -> original",
        "0x002328b5 MP_MENU_ROW_CHOSEN: before [pause-mp-rows:open Settings] -> original",
    ] {
        assert!(text.contains(line), "{line}\n{text}");
    }
    assert!(!text.contains("MP_MENU_LIST_CTOR"));
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
        Chosen::Stepped("pad.mute_button: false".into())
    );
    assert_eq!(m.line(row), "Mute Button: Off");
    assert!(!pad_mute.get());

    let audio = VoiceHandles::new(m.store());
    let p = page(&m, VOICE);
    let mut focus = Focus(4);
    assert_eq!(
        p.step(&mut focus, Input::A),
        ListEffect::Stepped("voice.mic_muted: true".into())
    );
    assert_eq!(m.store().get(MIC_MUTED.key), Some(Value::Bool(true)));
    assert_eq!(audio.capture_gain(), 0.0);
    assert_eq!(m.line(m.row(VOICE_LIST, 4).unwrap()), "Mute My Mic: On");
    assert_eq!(
        p.step(&mut Focus(7), Input::A),
        ListEffect::Stepped("voice.output_volume: 0".into())
    );
    assert_eq!(audio.output_volume.get(), 0);
}

#[test]
fn video_still_steps_display_settings_live() {
    let m = model();
    let p = page(&m, VIDEO);
    let mut focus = Focus(0);
    assert_eq!(
        p.step(&mut focus, Input::A),
        ListEffect::Stepped("graphics.fov: 75".into())
    );
    assert_eq!(m.store().handle::<u32>(FOV.key).get(), 75);
    assert_eq!(
        m.line(m.row(video_screen::LIST, 0).unwrap()),
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
    assert_eq!(label(&m, m.texts(mic)[1].1), "System default");
    m.press(mic);
    assert_eq!(label(&m, m.texts(mic)[1].1), "in device number 0");
}
