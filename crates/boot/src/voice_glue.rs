//! Voice chat's communicator in boot (docs/live/live-voice.md, 3 and 5).
//!
//! [`install`] registers `voice::hle` (XVoiceCreateMediaObjectEx, the XMO's
//! Process and Release, and the engine DPC). [`Link::pump`] runs between
//! slices. It works out the communicator ports and the talker from the
//! Voice settings ([`plan`]) and hands them to XAPI's device types and the
//! media objects. It also opens the host devices again when the chosen
//! microphone or output changes.
//!
//! **Off unless a microphone is chosen.** With `voice.input_device` at
//! System default the mask stays 0, so the game sees no communicator, as
//! before voice. Once a microphone is chosen on the pause menu's Voice
//! screen, every port with a pad gets a communicator.
//!
//! **Talker.** In open-mic mode, it is the first such port. In push-to-talk
//! mode it is the first such port whose pad holds the chosen button, as the
//! game last read it (`Xapi::last_input_state`); nobody talks while no pad
//! holds it. `voice.mic_muted` silences it in either mode.
//!
//! **"Same as Game".** With `voice.output_device` at System default the
//! headphone mix plays in the game's own mix ([`GameMix`]: a host stream
//! in DirectSound's mixer), so it comes out of the game's output device.
//! A chosen output device plays it on its own stream instead.
//!
//! **Volumes.** The input and output volumes (and the two mutes) reach the
//! media objects as gains (`VoiceHandles::capture_gain`, `playback_gain`).

use crate::voice_settings::{
    MODE_OPEN, MODE_PUSH, PTT_BACK, PTT_DPAD_DOWN, PTT_DPAD_LEFT, PTT_DPAD_RIGHT, PTT_DPAD_UP,
    VoiceHandles, device_defs,
};
use audio::voice::SYSTEM_DEFAULT;
use input::gamepad::Buttons;
use settings::{Kind, Store};
use std::sync::{Arc, Mutex, MutexGuard};
use voice::{
    HOST_FRAME, HOST_RATE,
    hle::{self, Voice},
    io::{Capture, Frame, NullVoiceIo, OpenIo, Playback, VoiceIo},
};

/// What the settings and the pads ask for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    /// Ports with a communicator (bits 0..=3).
    pub ports: u32,
    /// The port the microphone feeds.
    pub talker: Option<u8>,
}

/// The ports and talker for these settings, the connected pads and the
/// pads holding the push-to-talk button (`held`, a port mask).
pub fn plan(input_device: i64, mic_muted: bool, mode: i64, pads: u32, held: u32) -> Plan {
    let ports = if input_device == SYSTEM_DEFAULT {
        0
    } else {
        pads & 0xf
    };
    let can_talk = match mode {
        MODE_OPEN => ports,
        MODE_PUSH => ports & held,
        _ => 0,
    };
    let talker = (can_talk != 0 && !mic_muted).then(|| can_talk.trailing_zeros() as u8);
    Plan { ports, talker }
}

/// The pad button a `PTT_*` value names; `None` for a value not listed.
pub fn push_to_talk_button(value: i64) -> Option<Buttons> {
    Some(match value {
        PTT_DPAD_DOWN => Buttons::DPAD_DOWN,
        PTT_DPAD_UP => Buttons::DPAD_UP,
        PTT_DPAD_LEFT => Buttons::DPAD_LEFT,
        PTT_DPAD_RIGHT => Buttons::DPAD_RIGHT,
        PTT_BACK => Buttons::BACK,
        _ => return None,
    })
}

/// The ports whose pad holds `button`, from each port's last state.
pub fn held(
    button: Option<Buttons>,
    state: impl Fn(usize) -> Option<input::gamepad::State>,
) -> u32 {
    let Some(button) = button else { return 0 };
    (0..4)
        .filter(|&port| state(port).is_some_and(|s| s.gamepad.buttons.contains(button)))
        .fold(0, |mask, port| mask | 1 << port)
}

/// The host id of the device `key` names among a device `Def`'s choices
/// (a choice's name is the device's id); `None` for System default or a
/// key not listed.
pub fn device_id(def: &settings::Def, key: i64) -> Option<String> {
    if key == SYSTEM_DEFAULT {
        return None;
    }
    match &def.kind {
        Kind::Choice(choices) => choices
            .iter()
            .find(|c| c.value == key)
            .map(|c| c.name.to_owned()),
        _ => None,
    }
}

/// The headphone mix as a stream in the game's mixer ("Same as Game"):
/// 48 kHz mono, one 20 ms packet per media-object tick, on no mix bins
/// (straight to both speakers). A stream that ran dry gets 20 ms of
/// silence in front of the next frame, so the tick's jitter against the
/// mixer does not cut it up; at most [`GameMix::MAX_QUEUED`] frames wait,
/// and frames past that are dropped. Removed from the mixer when dropped.
pub struct GameMix {
    mixer: Arc<Mutex<audio::Mixer>>,
    stream: Option<audio::StreamId>,
    bytes: Vec<u8>,
}

impl GameMix {
    /// 100 ms.
    pub const MAX_QUEUED: usize = 5;

    pub fn new(mixer: &Arc<Mutex<audio::Mixer>>) -> Self {
        let stream = audio::WaveFormat::pcm16(1, HOST_RATE)
            .and_then(audio::Stream::new)
            .map(|s| lock(mixer).add_stream(s))
            .map_err(|e| eprintln!("voice: no stream in the game mix: {e}"))
            .ok();
        Self {
            mixer: Arc::clone(mixer),
            stream,
            bytes: Vec::with_capacity(HOST_FRAME * 2),
        }
    }
}

impl Playback for GameMix {
    fn write(&mut self, frame: &Frame) {
        let Some(id) = self.stream else { return };
        let mut mixer = lock(&self.mixer);
        let Some(stream) = mixer.stream_mut(id) else {
            return;
        };
        stream.take_completed();
        if stream.pending() >= Self::MAX_QUEUED {
            return;
        }
        if stream.pending() == 0 {
            let _ = stream.submit(&[0; HOST_FRAME * 2]);
        }
        self.bytes.clear();
        self.bytes
            .extend(frame.iter().flat_map(|s| s.to_le_bytes()));
        let _ = stream.submit(&self.bytes);
    }
}

impl Drop for GameMix {
    fn drop(&mut self) {
        if let Some(id) = self.stream {
            lock(&self.mixer).remove_stream(id);
        }
    }
}

/// The host microphone with the headphone mix going to the game's mix.
struct SameAsGame {
    io: Box<dyn VoiceIo>,
    game: GameMix,
}

impl VoiceIo for SameAsGame {
    fn capture(&mut self) -> &mut dyn Capture {
        self.io.capture()
    }
    fn playback(&mut self) -> &mut dyn Playback {
        &mut self.game
    }
}

pub struct Link {
    voice: hle::Shared,
    xapi: crate::xapi_glue::Shared,
    handles: VoiceHandles,
    open: Option<OpenIo>,
    /// DirectSound's mixer, for "Same as Game"; `None` without DirectSound.
    game_mix: Option<Arc<Mutex<audio::Mixer>>>,
    /// The devices open now: (input key, output key).
    opened: Option<(i64, i64)>,
    last: Option<Plan>,
    /// The input and output gains last given to the media objects.
    gains: Option<(f32, f32)>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Registers the media objects. `None` without settings (headless runs)
/// or when the image has no engine DPC to wrap.
pub fn install(
    rt: &runtime::Runtime,
    xapi: &crate::xapi_glue::Shared,
    settings: Option<&Arc<Store>>,
    open: Option<OpenIo>,
    game_mix: Option<Arc<Mutex<audio::Mixer>>>,
) -> Option<Link> {
    let store = settings?;
    let voice: hle::Shared = Arc::new(Mutex::new(Voice::new(Box::new(NullVoiceIo))));
    if let Err(e) = hle::register(rt, &voice) {
        eprintln!("voice: media objects not installed: {e}");
        return None;
    }
    Some(Link {
        voice,
        xapi: Arc::clone(xapi),
        handles: VoiceHandles::new(store),
        open,
        game_mix,
        opened: None,
        last: None,
        gains: None,
    })
}

impl Link {
    /// Between slices, with no guest memory, XAPI or voice lock held.
    pub fn pump(&mut self) {
        let input = self.handles.input_device.get();
        let output = self.handles.output_device.get();
        let mode = self.handles.mode.get();
        let (pads, held) = {
            let xapi = lock(&self.xapi);
            let held = if mode == MODE_PUSH {
                let button = push_to_talk_button(self.handles.push_to_talk_button.get());
                held(button, |port| xapi.last_input_state(port))
            } else {
                0
            };
            (xapi.input.connected_mask(), held)
        };
        let plan = plan(input, self.handles.mic_muted.get(), mode, pads, held);

        if plan.ports != 0
            && self.opened != Some((input, output))
            && let Some(open) = &self.open
        {
            let [input_def, output_def] = device_defs();
            let output_id = device_id(output_def, output);
            let mut io = open(device_id(input_def, input).as_deref(), output_id.as_deref());
            if output_id.is_none()
                && let Some(mixer) = &self.game_mix
            {
                io = Box::new(SameAsGame {
                    io,
                    game: GameMix::new(mixer),
                });
            }
            lock(&self.voice).io = io;
            self.opened = Some((input, output));
        } else if plan.ports == 0 && self.opened.is_some() {
            lock(&self.voice).io = Box::new(NullVoiceIo);
            self.opened = None;
        }

        if self.last != Some(plan) {
            lock(&self.xapi).set_voice_ports(plan.ports);
            let mut voice = lock(&self.voice);
            // Packets of a pulled communicator fail; the engine tears the
            // talker down from its own device-change poll, so their
            // callbacks are not needed.
            let _ = voice.objects.set_present(plan.ports);
            voice.objects.set_talker(plan.talker);
            self.last = Some(plan);
        }

        let gains = (self.handles.capture_gain(), self.handles.playback_gain());
        if self.gains != Some(gains) {
            lock(&self.voice).objects.set_gains(gains.0, gains.1);
            self.gains = Some(gains);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_microphone_chosen_means_no_communicator() {
        assert_eq!(
            plan(SYSTEM_DEFAULT, false, MODE_OPEN, 0b1111, 0b1111),
            Plan {
                ports: 0,
                talker: None
            }
        );
    }

    #[test]
    fn a_chosen_microphone_gives_every_pad_a_communicator() {
        assert_eq!(
            plan(42, false, MODE_OPEN, 0b0110, 0),
            Plan {
                ports: 0b0110,
                talker: Some(1)
            }
        );
        assert_eq!(
            plan(42, false, MODE_OPEN, 0, 0),
            Plan {
                ports: 0,
                talker: None
            }
        );
        assert_eq!(plan(42, false, MODE_OPEN, 0xff, 0).ports, 0xf);
    }

    #[test]
    fn push_to_talk_feeds_the_first_pad_holding_the_button() {
        // Pads on ports 0, 2 and 3; 2 and 3 hold the button.
        assert_eq!(plan(42, false, MODE_PUSH, 0b1101, 0b1100).talker, Some(2));
        // A held button on a port with no communicator does not count.
        assert_eq!(plan(42, false, MODE_PUSH, 0b0001, 0b0010).talker, None);
        // Released: nobody.
        assert_eq!(plan(42, false, MODE_PUSH, 0b1101, 0).talker, None);
        // Muted wins.
        assert_eq!(plan(42, true, MODE_PUSH, 0b1101, 0b0001).talker, None);
        // Open mic ignores the button.
        assert_eq!(plan(42, false, MODE_OPEN, 0b1100, 0b1000).talker, Some(2));
    }

    #[test]
    fn the_held_mask_reads_each_ports_last_state() {
        use input::gamepad::{Gamepad, State};
        let pad = |buttons: Buttons| State {
            packet_number: 1,
            gamepad: Gamepad {
                buttons,
                ..Gamepad::default()
            },
        };
        let states = [
            Some(pad(Buttons::DPAD_DOWN | Buttons::START)),
            None,
            Some(pad(Buttons::BACK)),
            Some(pad(Buttons::DPAD_DOWN)),
        ];
        let state = |port: usize| states[port];
        assert_eq!(held(push_to_talk_button(PTT_DPAD_DOWN), state), 0b1001);
        assert_eq!(held(push_to_talk_button(PTT_BACK), state), 0b0100);
        assert_eq!(held(push_to_talk_button(PTT_DPAD_UP), state), 0);
        assert_eq!(
            held(push_to_talk_button(99), state),
            0,
            "an unknown value holds nothing"
        );
        for (value, button) in [
            (PTT_DPAD_DOWN, Buttons::DPAD_DOWN),
            (PTT_DPAD_UP, Buttons::DPAD_UP),
            (PTT_DPAD_LEFT, Buttons::DPAD_LEFT),
            (PTT_DPAD_RIGHT, Buttons::DPAD_RIGHT),
            (PTT_BACK, Buttons::BACK),
        ] {
            assert_eq!(push_to_talk_button(value), Some(button));
        }
    }

    #[test]
    fn muted_or_push_to_talk_feeds_no_port() {
        assert_eq!(
            plan(42, true, MODE_OPEN, 1, 0),
            Plan {
                ports: 1,
                talker: None
            }
        );
        assert_eq!(
            plan(42, false, MODE_PUSH, 1, 0),
            Plan {
                ports: 1,
                talker: None
            }
        );
    }

    #[test]
    fn same_as_game_plays_the_headphone_mix_in_the_game_mix() {
        let mixer = Arc::new(Mutex::new(audio::Mixer::new(HOST_RATE)));
        let mut game = GameMix::new(&mixer);
        let mut out = vec![0.0f32; 2 * HOST_FRAME];
        // Rendered dry, then one frame: 20 ms of silence goes first.
        lock(&mixer).render(&mut out);
        game.write(&[8192; HOST_FRAME]);
        lock(&mixer).render(&mut out);
        assert!(out.iter().all(|&v| v == 0.0), "the cushion");
        lock(&mixer).render(&mut out);
        assert!(
            out.iter().all(|&v| v == 0.25),
            "both speakers: {:?}",
            &out[..4]
        );
        // A mixer that does not keep up holds at most 100 ms.
        for _ in 0..20 {
            game.write(&[8192; HOST_FRAME]);
        }
        let id = game.stream.unwrap();
        assert_eq!(
            lock(&mixer).stream_mut(id).unwrap().pending(),
            GameMix::MAX_QUEUED
        );
        // Dropping it takes the stream out of the mix.
        drop(game);
        assert!(lock(&mixer).stream_mut(id).is_none());
        assert_eq!(lock(&mixer).stream_count(), 0);
    }

    #[test]
    fn same_as_game_keeps_the_microphone() {
        let mixer = Arc::new(Mutex::new(audio::Mixer::new(HOST_RATE)));
        let fake = voice::io::FakeVoiceIo::new();
        fake.speak([7; HOST_FRAME]);
        let mut io = SameAsGame {
            io: Box::new(fake.clone()),
            game: GameMix::new(&mixer),
        };
        let mut frame = [0; HOST_FRAME];
        assert!(io.capture().read(&mut frame));
        assert_eq!(frame[0], 7);
        io.playback().write(&[1; HOST_FRAME]);
        assert!(fake.take_played().is_empty(), "not on the voice device");
        assert_eq!(lock(&mixer).stream_count(), 1);
    }

    #[test]
    fn device_ids_come_from_the_choice_names() {
        let devices = [
            audio::voice::Device::new("coreaudio:Mic", "Mic"),
            audio::voice::Device::new("coreaudio:Headset", "Headset"),
        ];
        let def = crate::voice_settings::device_def(&crate::voice_settings::INPUT_DEVICE, &devices);
        assert_eq!(
            device_id(&def, devices[1].key()).as_deref(),
            Some("coreaudio:Headset")
        );
        assert_eq!(device_id(&def, SYSTEM_DEFAULT), None);
        assert_eq!(device_id(&def, 7), None);
    }
}
