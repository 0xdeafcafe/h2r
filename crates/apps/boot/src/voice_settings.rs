//! Voice chat's settings (docs/game/settings-menu.md "Voice screen"): one
//! [`Def`] each, every one naming the Voice screen, and the handles the
//! voice chat reads ([`VoiceHandles`]). Capture and playback are the voice
//! chat's own; this is the settings and menu side.
//!
//! **Devices.** `voice.input_device` and `voice.output_device` are choices
//! of "System default" and the host's devices, listed once at start
//! ([`use_host_devices`], from an [`audio::voice::Devices`]). A device's
//! value is [`audio::voice::key`] of its host id; the file keeps the id
//! itself (`input_device = "coreaudio:<UID>"`), so a replug or a reboot
//! finds it again. A device saved in the file but not plugged in at start
//! is reported once and the row shows System default; the file keeps it
//! until another device is chosen. With no host list (tests, `h2 run`)
//! the rows offer System default only.
//!
//! **On the audio path** every read is one atomic load: muting is
//! `VoiceHandles::mic_muted.get()`, which takes no lock.

use std::sync::OnceLock;

use audio::voice::{Device, Devices, SYSTEM_DEFAULT};
use settings::{Apply, Choice, Def, Kind, Menu, Scope, Setting, Store, Tier, Value};

/// The Voice screen's id (`ui` model, `list_class`).
pub const SCREEN: &str = "voice";

/// The device rows' first choice.
pub const DEFAULT_CHOICE: Choice = Choice {
    value: SYSTEM_DEFAULT,
    name: "default",
    label: "System default",
};
static DEFAULT_ONLY: &[Choice] = &[DEFAULT_CHOICE];

/// The output row's first choice: mixed into the game's own output (its
/// device and volume), no second stream.
pub const GAME_CHOICE: Choice = Choice {
    value: SYSTEM_DEFAULT,
    name: "game",
    label: "Same as Game",
};
static GAME_ONLY: &[Choice] = &[GAME_CHOICE];

/// At most this many devices a row offers (the `ui` model holds a string
/// per choice; a host with more lists the first ones).
pub const MAX_DEVICES: usize = 12;
/// A device name longer than this is cut, with an ellipsis, to fit the
/// row's value text.
pub const MAX_LABEL: usize = 24;

pub static INPUT_DEVICE: Def = Def {
    key: "voice.input_device",
    label: "Microphone",
    help: "The microphone voice chat captures from: System default (follows the \
           host's choice) or one device, kept by its host id. H2_VOICE_INPUT takes \
           an id or part of a name.",
    kind: Kind::Choice(DEFAULT_ONLY),
    default: Value::Int(SYSTEM_DEFAULT),
    env: Some("H2_VOICE_INPUT"),
    flag: None,
    aliases: &[("system", Value::Int(SYSTEM_DEFAULT))],
    parse_env: Some(input_env),
    apply: Apply::Live,
    scope: Scope::Host,
    tier: Tier::User,
    menu: Some(Menu {
        screen: SCREEN,
        order: 10,
    }),
};

pub static OUTPUT_DEVICE: Def = Def {
    key: "voice.output_device",
    label: "Voice Output",
    help: "Where other players' voices play: Same as Game (mixed into the game's \
           own output) or one device, a headset, kept by its host id. H2_VOICE_OUTPUT \
           takes an id or part of a name.",
    kind: Kind::Choice(GAME_ONLY),
    default: Value::Int(SYSTEM_DEFAULT),
    env: Some("H2_VOICE_OUTPUT"),
    flag: None,
    aliases: &[("default", Value::Int(SYSTEM_DEFAULT))],
    parse_env: Some(output_env),
    apply: Apply::Live,
    scope: Scope::Host,
    tier: Tier::User,
    menu: Some(Menu {
        screen: SCREEN,
        order: 20,
    }),
};

/// [`MODE`]'s values.
pub const MODE_OPEN: i64 = 0;
pub const MODE_PUSH: i64 = 1;

pub static MODE: Def = Def {
    key: "voice.mode",
    label: "Talk",
    help: "Open Mic sends whenever you speak; Push to Talk only while the Talk \
           Button is held.",
    kind: Kind::Choice(&[
        Choice {
            value: MODE_OPEN,
            name: "open",
            label: "Open Mic",
        },
        Choice {
            value: MODE_PUSH,
            name: "push",
            label: "Push to Talk",
        },
    ]),
    default: Value::Int(MODE_OPEN),
    env: Some("H2_VOICE_MODE"),
    flag: None,
    aliases: &[("ptt", Value::Int(MODE_PUSH)), ("open_mic", Value::Int(MODE_OPEN))],
    parse_env: None,
    apply: Apply::Live,
    scope: Scope::Host,
    tier: Tier::User,
    menu: Some(Menu {
        screen: SCREEN,
        order: 30,
    }),
};

/// [`PTT_BUTTON`]'s values: pad buttons Halo 2's default layouts leave
/// free in a game (the d-pad), and Back.
pub const PTT_DPAD_DOWN: i64 = 0;
pub const PTT_DPAD_UP: i64 = 1;
pub const PTT_DPAD_LEFT: i64 = 2;
pub const PTT_DPAD_RIGHT: i64 = 3;
pub const PTT_BACK: i64 = 4;

pub static PTT_BUTTON: Def = Def {
    key: "voice.push_to_talk_button",
    label: "Talk Button",
    help: "The pad button held to talk in Push to Talk. Back also shows the \
           multiplayer scoreboard.",
    kind: Kind::Choice(&[
        Choice {
            value: PTT_DPAD_DOWN,
            name: "dpad_down",
            label: "D-Pad Down",
        },
        Choice {
            value: PTT_DPAD_UP,
            name: "dpad_up",
            label: "D-Pad Up",
        },
        Choice {
            value: PTT_DPAD_LEFT,
            name: "dpad_left",
            label: "D-Pad Left",
        },
        Choice {
            value: PTT_DPAD_RIGHT,
            name: "dpad_right",
            label: "D-Pad Right",
        },
        Choice {
            value: PTT_BACK,
            name: "back",
            label: "Back",
        },
    ]),
    default: Value::Int(PTT_DPAD_DOWN),
    env: Some("H2_VOICE_PTT_BUTTON"),
    flag: None,
    aliases: &[],
    parse_env: None,
    apply: Apply::Live,
    scope: Scope::Host,
    tier: Tier::User,
    menu: Some(Menu {
        screen: SCREEN,
        order: 40,
    }),
};

pub static MIC_MUTED: Def = Def {
    key: "voice.mic_muted",
    label: "Mute My Mic",
    help: "Send no voice at all, whatever the Talk mode.",
    kind: Kind::Bool,
    default: Value::Bool(false),
    env: Some("H2_VOICE_MIC_MUTED"),
    flag: None,
    aliases: &[],
    parse_env: None,
    apply: Apply::Live,
    scope: Scope::Host,
    tier: Tier::User,
    menu: Some(Menu {
        screen: SCREEN,
        order: 50,
    }),
};

pub static INCOMING_MUTED: Def = Def {
    key: "voice.incoming_muted",
    label: "Mute Everyone",
    help: "Play no other player's voice.",
    kind: Kind::Bool,
    default: Value::Bool(false),
    env: Some("H2_VOICE_INCOMING_MUTED"),
    flag: None,
    aliases: &[],
    parse_env: None,
    apply: Apply::Live,
    scope: Scope::Host,
    tier: Tier::User,
    menu: Some(Menu {
        screen: SCREEN,
        order: 60,
    }),
};

/// A volume's choices: 0% to 100% in tens. The file keeps the number.
const VOLUMES: &[Choice] = &[
    volume(0, "0", "0%"),
    volume(10, "10", "10%"),
    volume(20, "20", "20%"),
    volume(30, "30", "30%"),
    volume(40, "40", "40%"),
    volume(50, "50", "50%"),
    volume(60, "60", "60%"),
    volume(70, "70", "70%"),
    volume(80, "80", "80%"),
    volume(90, "90", "90%"),
    volume(100, "100", "100%"),
];

const fn volume(value: i64, name: &'static str, label: &'static str) -> Choice {
    Choice { value, name, label }
}

pub static INPUT_VOLUME: Def = Def {
    key: "voice.input_volume",
    label: "Mic Volume",
    help: "How loud your voice is sent, 0 to 100 percent in tens.",
    kind: Kind::Choice(VOLUMES),
    default: Value::Int(100),
    env: Some("H2_VOICE_INPUT_VOLUME"),
    flag: None,
    aliases: &[],
    parse_env: None,
    apply: Apply::Live,
    scope: Scope::Host,
    tier: Tier::User,
    menu: Some(Menu {
        screen: SCREEN,
        order: 70,
    }),
};

pub static OUTPUT_VOLUME: Def = Def {
    key: "voice.output_volume",
    label: "Voice Volume",
    help: "How loud other players' voices play, 0 to 100 percent in tens.",
    kind: Kind::Choice(VOLUMES),
    default: Value::Int(100),
    env: Some("H2_VOICE_OUTPUT_VOLUME"),
    flag: None,
    aliases: &[],
    parse_env: None,
    apply: Apply::Live,
    scope: Scope::Host,
    tier: Tier::User,
    menu: Some(Menu {
        screen: SCREEN,
        order: 80,
    }),
};

/// Every voice setting, in the order the Voice screen shows them.
pub static ALL: &[&Def] = &[
    &INPUT_DEVICE,
    &OUTPUT_DEVICE,
    &MODE,
    &PTT_BUTTON,
    &MIC_MUTED,
    &INCOMING_MUTED,
    &INPUT_VOLUME,
    &OUTPUT_VOLUME,
];

/// What the voice chat reads, each with one atomic load and no lock (safe
/// on an audio callback).
#[derive(Clone)]
pub struct VoiceHandles {
    /// [`audio::voice::key`] of the chosen microphone's id, or
    /// [`SYSTEM_DEFAULT`]; [`audio::voice::find`] gives the device.
    pub input_device: Setting<i64>,
    pub output_device: Setting<i64>,
    /// [`MODE_OPEN`] or [`MODE_PUSH`].
    pub mode: Setting<i64>,
    /// One of the `PTT_*` buttons.
    pub push_to_talk_button: Setting<i64>,
    pub mic_muted: Setting<bool>,
    pub incoming_muted: Setting<bool>,
    /// Percent, 0 to 100.
    pub input_volume: Setting<u32>,
    pub output_volume: Setting<u32>,
}

impl VoiceHandles {
    /// The handles of `store`. Panics when the store lacks a voice setting
    /// (every store made from [`crate::setting_defs::defs`] has them).
    pub fn new(store: &Store) -> Self {
        VoiceHandles {
            input_device: store.handle(INPUT_DEVICE.key),
            output_device: store.handle(OUTPUT_DEVICE.key),
            mode: store.handle(MODE.key),
            push_to_talk_button: store.handle(PTT_BUTTON.key),
            mic_muted: store.handle(MIC_MUTED.key),
            incoming_muted: store.handle(INCOMING_MUTED.key),
            input_volume: store.handle(INPUT_VOLUME.key),
            output_volume: store.handle(OUTPUT_VOLUME.key),
        }
    }

    /// The gain to send the microphone with: 0 while muted (checked
    /// first, one load), else the volume.
    pub fn capture_gain(&self) -> f32 {
        if self.mic_muted.get() {
            return 0.0;
        }
        self.input_volume.get().min(100) as f32 / 100.0
    }

    /// The gain to play other players with: 0 while everyone is muted.
    pub fn playback_gain(&self) -> f32 {
        if self.incoming_muted.get() {
            return 0.0;
        }
        self.output_volume.get().min(100) as f32 / 100.0
    }
}

/// The host's lists, once: the devices and the two `Def`s made of them.
struct Host {
    inputs: Vec<Device>,
    outputs: Vec<Device>,
    input_def: &'static Def,
    output_def: &'static Def,
}

static HOST: OnceLock<Host> = OnceLock::new();

/// Lists the host's voice devices for this run, once, before the settings
/// are loaded ([`crate::setting_defs::load`]). Later calls do nothing and
/// return false.
pub fn use_host_devices(devices: &dyn Devices) -> bool {
    let mut first = false;
    HOST.get_or_init(|| {
        first = true;
        let inputs = devices.inputs();
        let outputs = devices.outputs();
        Host {
            input_def: leak(device_def(&INPUT_DEVICE, &inputs)),
            output_def: leak(device_def(&OUTPUT_DEVICE, &outputs)),
            inputs,
            outputs,
        }
    });
    first
}

/// The device `Def`s in effect: the host's once listed, else the static
/// ones (System default only).
pub fn device_defs() -> [&'static Def; 2] {
    match HOST.get() {
        Some(h) => [h.input_def, h.output_def],
        None => [&INPUT_DEVICE, &OUTPUT_DEVICE],
    }
}

fn leak<T>(t: T) -> &'static T {
    Box::leak(Box::new(t))
}

fn leak_str(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

/// `base` with its own first choice (System default, Same as Game) and
/// `devices` as its choices (the first [`MAX_DEVICES`]; a device listed
/// twice is offered once).
pub fn device_def(base: &'static Def, devices: &[Device]) -> Def {
    let first = match &base.kind {
        Kind::Choice([first, ..]) => *first,
        _ => DEFAULT_CHOICE,
    };
    let choices: Vec<Choice> = std::iter::once(first)
        .chain(device_choices(devices).into_iter().map(|(key, id, label)| Choice {
            value: key,
            name: leak_str(id),
            label: leak_str(label),
        }))
        .collect();
    Def {
        kind: Kind::Choice(Box::leak(choices.into_boxed_slice())),
        ..base.clone()
    }
}

/// The choices `devices` give: `(key, id, label)`, in the host's order, a
/// repeated id once, at most [`MAX_DEVICES`]. A label is the device's name
/// cut to [`MAX_LABEL`]; a name already shown gets ` 2`, ` 3` and so on.
pub fn device_choices(devices: &[Device]) -> Vec<(i64, String, String)> {
    let mut out: Vec<(i64, String, String)> = Vec::new();
    for d in devices {
        if out.len() == MAX_DEVICES {
            break;
        }
        let key = d.key();
        if out.iter().any(|(k, _, _)| *k == key) {
            continue;
        }
        let base = cut(d.name.trim());
        let base = if base.is_empty() { "Device".to_string() } else { base };
        let mut label = base.clone();
        let mut n = 1;
        while out.iter().any(|(_, _, l)| *l == label)
            || label == DEFAULT_CHOICE.label
            || label == GAME_CHOICE.label
        {
            n += 1;
            label = format!("{base} {n}");
        }
        out.push((key, d.id.clone(), label));
    }
    out
}

fn cut(name: &str) -> String {
    if name.chars().count() <= MAX_LABEL {
        return name.to_string();
    }
    let kept: String = name.chars().take(MAX_LABEL - 3).collect();
    format!("{}...", kept.trim_end())
}

/// `H2_VOICE_INPUT`: a listed input's id, else the first whose name holds
/// the text, else `default`.
fn input_env(text: &str) -> Result<Value, String> {
    env_device(text, HOST.get().map(|h| h.inputs.as_slice()).unwrap_or(&[]))
}

fn output_env(text: &str) -> Result<Value, String> {
    env_device(text, HOST.get().map(|h| h.outputs.as_slice()).unwrap_or(&[]))
}

/// A device variable's text against `devices`.
pub fn env_device(text: &str, devices: &[Device]) -> Result<Value, String> {
    let t = text.trim();
    if ["default", "system", "game"].iter().any(|w| t.eq_ignore_ascii_case(w)) {
        return Ok(Value::Int(SYSTEM_DEFAULT));
    }
    let listed: Vec<&Device> = devices.iter().take(MAX_DEVICES).collect();
    if let Some(d) = listed.iter().find(|d| d.id == t) {
        return Ok(Value::Int(d.key()));
    }
    let names: Vec<&str> = listed.iter().map(|d| d.name.as_str()).collect();
    match audio::devices::pick(&names, t) {
        Some(i) => Ok(Value::Int(listed[i].key())),
        None => Err(format!("no voice device named {t:?} (default, a device id or part of a name)")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use audio::voice::FakeDevices;
    use settings::Outcome;

    fn host() -> FakeDevices {
        FakeDevices::new(
            vec![
                Device::new("coreaudio:BuiltInMic", "MacBook Pro Microphone"),
                Device::new("coreaudio:Headset-in", "USB Headset"),
                Device::new("coreaudio:Headset2-in", "USB Headset"),
                Device::new("coreaudio:BuiltInMic", "MacBook Pro Microphone"),
                Device::new("coreaudio:Long", "A Very Long Audio Interface Name That Goes On"),
            ],
            vec![Device::new("coreaudio:BuiltInSpeaker", "MacBook Pro Speakers")],
        )
    }

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("h2-voice-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        settings::path_in(&d)
    }

    #[test]
    fn every_voice_setting_is_live_host_only_and_on_the_voice_screen() {
        let mut orders = Vec::new();
        for d in ALL {
            assert!(d.key.starts_with("voice."), "{}", d.key);
            assert_eq!(d.apply, Apply::Live, "{}", d.key);
            assert_eq!(d.scope, Scope::Host, "{} never touches the guest", d.key);
            assert_eq!(d.tier, Tier::User, "{}", d.key);
            assert_eq!(d.menu.map(|m| m.screen), Some(SCREEN), "{}", d.key);
            assert_eq!(d.check(d.default), Ok(d.default), "{}", d.key);
            assert!(d.env.is_some_and(|e| e.starts_with("H2_VOICE_")), "{}", d.key);
            orders.push(d.menu.unwrap().order);
        }
        let mut sorted = orders.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(orders, sorted, "ALL is in screen order, no order twice");
        for v in ALL {
            assert!(
                crate::setting_defs::ALL.iter().any(|d| std::ptr::eq(*d, *v)),
                "{} is in setting_defs::ALL",
                v.key
            );
        }
    }

    #[test]
    fn defaults_are_open_mic_unmuted_full_volume_and_the_system_devices() {
        let store = Store::defaults(ALL);
        let h = VoiceHandles::new(&store);
        assert_eq!(h.input_device.get(), SYSTEM_DEFAULT);
        assert_eq!(h.output_device.get(), SYSTEM_DEFAULT);
        assert_eq!(h.mode.get(), MODE_OPEN);
        assert_eq!(h.push_to_talk_button.get(), PTT_DPAD_DOWN);
        assert!(!h.mic_muted.get() && !h.incoming_muted.get());
        assert_eq!((h.input_volume.get(), h.output_volume.get()), (100, 100));
        assert_eq!((h.capture_gain(), h.playback_gain()), (1.0, 1.0));
        assert_eq!(INPUT_DEVICE.label(Value::Int(0)), "System default");
        assert_eq!(INPUT_VOLUME.label(Value::Int(70)), "70%");
        assert_eq!(MODE.label(Value::Int(MODE_PUSH)), "Push to Talk");
    }

    /// Muting reaches a handle the audio side already holds, at once: the
    /// handle is the one atomic cell the store writes.
    #[test]
    fn muting_and_volumes_reach_held_handles_at_once() {
        let store = Store::defaults(ALL);
        let h = VoiceHandles::new(&store);
        let on_audio_thread = h.clone();
        store.step(MIC_MUTED.key).unwrap();
        assert!(on_audio_thread.mic_muted.get());
        assert_eq!(on_audio_thread.capture_gain(), 0.0, "muted wins over the volume");
        store.step(INCOMING_MUTED.key).unwrap();
        assert_eq!(on_audio_thread.playback_gain(), 0.0);
        store.step(INCOMING_MUTED.key).unwrap();
        // Volume steps wrap from 100 to 0, then 10.
        store.step(OUTPUT_VOLUME.key).unwrap();
        store.step(OUTPUT_VOLUME.key).unwrap();
        assert_eq!(on_audio_thread.output_volume.get(), 10);
        assert!((on_audio_thread.playback_gain() - 0.1).abs() < 1e-6);
        store.step(MODE.key).unwrap();
        assert_eq!(on_audio_thread.mode.get(), MODE_PUSH);
    }

    #[test]
    fn device_choices_follow_the_host_once_each_cut_and_told_apart() {
        let inputs = host().inputs();
        let c = device_choices(&inputs);
        let labels: Vec<&str> = c.iter().map(|(_, _, l)| l.as_str()).collect();
        assert_eq!(
            labels,
            ["MacBook Pro Microphone", "USB Headset", "USB Headset 2", "A Very Long Audio Int..."]
        );
        assert!(labels.iter().all(|l| l.chars().count() <= MAX_LABEL));
        assert_eq!(c[0].0, audio::voice::key("coreaudio:BuiltInMic"));
        assert_eq!(c[1].1, "coreaudio:Headset-in");
        let many: Vec<Device> = (0..20).map(|i| Device::new(format!("id{i}"), format!("Mic {i}"))).collect();
        assert_eq!(device_choices(&many).len(), MAX_DEVICES);
        // A device named like the default choice is told apart from it.
        let odd = [Device::new("x", "System default")];
        assert_eq!(device_choices(&odd)[0].2, "System default 2");
        assert_eq!(device_choices(&[Device::new("y", "  ")])[0].2, "Device");
    }

    #[test]
    fn a_host_device_is_saved_by_its_id_and_found_again_next_run() {
        let inputs = host().inputs();
        let def: &'static Def = leak(device_def(&INPUT_DEVICE, &inputs));
        assert_eq!(def.values().len(), 5, "System default and four devices");
        let path = tmp("save");
        let defs = [def];
        let store = Store::load(&defs, Some(&path), &|_| None, &[]).0;
        let handle = store.handle::<i64>(def.key);
        let Outcome { state, .. } = store.step(def.key).unwrap();
        assert_eq!(def.label(state.value), "MacBook Pro Microphone");
        store.step(def.key).unwrap();
        assert_eq!(handle.get(), audio::voice::key("coreaudio:Headset-in"), "live at once");
        assert_eq!(
            audio::voice::find(&inputs, handle.get()).map(|d| d.id.as_str()),
            Some("coreaudio:Headset-in")
        );
        let t: toml::Table = std::fs::read_to_string(&path).unwrap().parse().unwrap();
        assert_eq!(t["voice"]["input_device"].as_str(), Some("coreaudio:Headset-in"));
        let (next, problems) = Store::load(&defs, Some(&path), &|_| None, &[]);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(next.get(def.key), Some(Value::Int(audio::voice::key("coreaudio:Headset-in"))));
    }

    #[test]
    fn an_unplugged_device_is_reported_shows_the_default_and_stays_in_the_file() {
        let path = tmp("gone");
        std::fs::write(&path, "[voice]\ninput_device = \"coreaudio:Gone\"\nmic_muted = true\n").unwrap();
        let def: &'static Def = leak(device_def(&INPUT_DEVICE, &host().inputs()));
        let defs = [def, &MIC_MUTED];
        let (store, problems) = Store::load(&defs, Some(&path), &|_| None, &[]);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert_eq!(problems[0].key, Some(INPUT_DEVICE.key));
        assert_eq!(store.get(def.key), Some(Value::Int(SYSTEM_DEFAULT)));
        // Another row's change keeps the saved device.
        store.step(MIC_MUTED.key).unwrap();
        let t: toml::Table = std::fs::read_to_string(&path).unwrap().parse().unwrap();
        assert_eq!(t["voice"]["input_device"].as_str(), Some("coreaudio:Gone"));
        assert_eq!(t["voice"]["mic_muted"].as_bool(), Some(false));
    }

    #[test]
    fn the_device_variable_takes_an_id_part_of_a_name_or_default() {
        let inputs = host().inputs();
        let k = |id: &str| Value::Int(audio::voice::key(id));
        assert_eq!(env_device("coreaudio:Headset2-in", &inputs), Ok(k("coreaudio:Headset2-in")));
        assert_eq!(env_device("headset", &inputs), Ok(k("coreaudio:Headset-in")));
        assert_eq!(env_device(" Default ", &inputs), Ok(Value::Int(SYSTEM_DEFAULT)));
        assert!(env_device("AirPods", &inputs).unwrap_err().contains("AirPods"));
        assert!(env_device("headset", &[]).is_err(), "no host list");
    }

    #[test]
    fn with_no_host_list_the_device_rows_offer_the_default_only() {
        assert_eq!(INPUT_DEVICE.values(), [Value::Int(SYSTEM_DEFAULT)]);
        assert_eq!(OUTPUT_DEVICE.values(), [Value::Int(SYSTEM_DEFAULT)]);
        assert_eq!(OUTPUT_DEVICE.label(Value::Int(SYSTEM_DEFAULT)), "Same as Game");
        assert_eq!(OUTPUT_DEVICE.parse("game"), Ok(Value::Int(SYSTEM_DEFAULT)));
        let out = device_def(&OUTPUT_DEVICE, &host().outputs());
        assert_eq!(out.label(Value::Int(SYSTEM_DEFAULT)), "Same as Game");
        assert_eq!(out.values().len(), 2);
        assert_eq!(INPUT_DEVICE.parse("default"), Ok(Value::Int(SYSTEM_DEFAULT)));
        assert_eq!(INPUT_DEVICE.parse("system"), Ok(Value::Int(SYSTEM_DEFAULT)));
    }
}
