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
//! **Talker.** In open-mic mode, it is the first such port. Push-to-talk is
//! not wired to the pad yet, so in that mode the microphone stays silent.
//! `voice.mic_muted` silences it too.

use crate::voice_settings::{MODE_OPEN, VoiceHandles, device_defs};
use audio::voice::SYSTEM_DEFAULT;
use settings::{Kind, Store};
use std::sync::{Arc, Mutex, MutexGuard};
use voice::{
    hle::{self, Voice},
    io::{NullVoiceIo, OpenIo},
};

/// What the settings and the pads ask for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    /// Ports with a communicator (bits 0..=3).
    pub ports: u32,
    /// The port the microphone feeds.
    pub talker: Option<u8>,
}

/// The ports and talker for these settings and connected pads.
pub fn plan(input_device: i64, mic_muted: bool, mode: i64, pads: u32) -> Plan {
    let ports = if input_device == SYSTEM_DEFAULT {
        0
    } else {
        pads & 0xf
    };
    let talker =
        (ports != 0 && !mic_muted && mode == MODE_OPEN).then(|| ports.trailing_zeros() as u8);
    Plan { ports, talker }
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

pub struct Link {
    voice: hle::Shared,
    xapi: crate::xapi_glue::Shared,
    handles: VoiceHandles,
    open: Option<OpenIo>,
    /// The devices open now: (input key, output key).
    opened: Option<(i64, i64)>,
    last: Option<Plan>,
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
        opened: None,
        last: None,
    })
}

impl Link {
    /// Between slices, with no guest memory, XAPI or voice lock held.
    pub fn pump(&mut self) {
        let input = self.handles.input_device.get();
        let output = self.handles.output_device.get();
        let pads = lock(&self.xapi).input.connected_mask();
        let plan = plan(
            input,
            self.handles.mic_muted.get(),
            self.handles.mode.get(),
            pads,
        );

        if plan.ports != 0
            && self.opened != Some((input, output))
            && let Some(open) = &self.open
        {
            let [input_def, output_def] = device_defs();
            let io = open(
                device_id(input_def, input).as_deref(),
                device_id(output_def, output).as_deref(),
            );
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voice_settings::MODE_PUSH;

    #[test]
    fn no_microphone_chosen_means_no_communicator() {
        assert_eq!(
            plan(SYSTEM_DEFAULT, false, MODE_OPEN, 0b1111),
            Plan {
                ports: 0,
                talker: None
            }
        );
    }

    #[test]
    fn a_chosen_microphone_gives_every_pad_a_communicator() {
        assert_eq!(
            plan(42, false, MODE_OPEN, 0b0110),
            Plan {
                ports: 0b0110,
                talker: Some(1)
            }
        );
        assert_eq!(
            plan(42, false, MODE_OPEN, 0),
            Plan {
                ports: 0,
                talker: None
            }
        );
        assert_eq!(plan(42, false, MODE_OPEN, 0xff).ports, 0xf);
    }

    #[test]
    fn muted_or_push_to_talk_feeds_no_port() {
        assert_eq!(
            plan(42, true, MODE_OPEN, 1),
            Plan {
                ports: 1,
                talker: None
            }
        );
        assert_eq!(
            plan(42, false, MODE_PUSH, 1),
            Plan {
                ports: 1,
                talker: None
            }
        );
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
