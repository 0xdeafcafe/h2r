//! Voice chat's host devices: which microphones and voice outputs the host
//! has ([`Devices`]), and the stable key a setting keeps for one ([`key`]).
//! Nothing here opens a device or moves a sample; capture and playback
//! belong to the voice chat itself (docs/game/settings-menu.md "Voice
//! screen").
//!
//! A device is known by its host id (CoreAudio's device UID through cpal,
//! which survives a reboot and a replug) and shown by its name. The
//! `voice.input_device` and `voice.output_device` settings hold [`key`] of
//! the id, so the audio side reads its choice with one atomic load and
//! finds the device again with [`find`]; 0 ([`SYSTEM_DEFAULT`]) is the
//! host's default device.
//!
//! [`HostDevices`] lists the host's devices through cpal (feature
//! `device`); [`FakeDevices`] is a fixed list for tests and for hosts with
//! no audio input.

/// The key of "the host's default device".
pub const SYSTEM_DEFAULT: i64 = 0;

/// One host audio device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    /// The host's id for it, stable across runs where the host allows
    /// (cpal's `DeviceId` as text, `coreaudio:<UID>` on macOS).
    pub id: String,
    /// What the host calls it (`MacBook Pro Microphone`).
    pub name: String,
}

impl Device {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Device {
            id: id.into(),
            name: name.into(),
        }
    }

    /// [`key`] of its id.
    pub fn key(&self) -> i64 {
        key(&self.id)
    }
}

/// The host's voice devices. Called rarely (at start, and when a list is
/// shown); never on the audio path.
pub trait Devices: Send + Sync {
    /// Devices that can capture: microphones, headsets, interfaces.
    fn inputs(&self) -> Vec<Device>;
    /// Devices that can play: speakers, headphones, headsets.
    fn outputs(&self) -> Vec<Device>;
}

/// The setting value for device id `id`: FNV-1a over its bytes, folded to a
/// positive `i64` that is never [`SYSTEM_DEFAULT`]. The same id gives the
/// same key in every run and on every host.
pub fn key(id: &str) -> i64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in id.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    match (h & i64::MAX as u64) as i64 {
        SYSTEM_DEFAULT => 1,
        k => k,
    }
}

/// The device of `devices` whose key is `key`: `None` for
/// [`SYSTEM_DEFAULT`] and for a device that is not there now (unplugged);
/// either way the caller uses the host's default.
pub fn find(devices: &[Device], key: i64) -> Option<&Device> {
    if key == SYSTEM_DEFAULT {
        return None;
    }
    devices.iter().find(|d| d.key() == key)
}

/// A fixed list: for tests, and for a host with nothing to list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FakeDevices {
    pub inputs: Vec<Device>,
    pub outputs: Vec<Device>,
}

impl FakeDevices {
    pub fn new(inputs: Vec<Device>, outputs: Vec<Device>) -> Self {
        FakeDevices { inputs, outputs }
    }
}

impl Devices for FakeDevices {
    fn inputs(&self) -> Vec<Device> {
        self.inputs.clone()
    }
    fn outputs(&self) -> Vec<Device> {
        self.outputs.clone()
    }
}

/// The host's devices through cpal's default host (CoreAudio on macOS).
/// A device whose id or name cannot be read (unplugged while listing) is
/// left out; a host that cannot list gives an empty list.
#[cfg(feature = "device")]
#[derive(Clone, Copy, Debug, Default)]
pub struct HostDevices;

#[cfg(feature = "device")]
impl HostDevices {
    fn list(
        devices: Result<impl Iterator<Item = cpal::Device>, cpal::Error>,
    ) -> Vec<Device> {
        use cpal::traits::DeviceTrait;
        let Ok(devices) = devices else {
            return Vec::new();
        };
        devices
            .filter_map(|d| {
                let id = d.id().ok()?.to_string();
                let name = d.description().ok()?.name().to_string();
                Some(Device { id, name })
            })
            .collect()
    }
}

#[cfg(feature = "device")]
impl Devices for HostDevices {
    fn inputs(&self) -> Vec<Device> {
        use cpal::traits::HostTrait;
        Self::list(cpal::default_host().input_devices())
    }
    fn outputs(&self) -> Vec<Device> {
        use cpal::traits::HostTrait;
        Self::list(cpal::default_host().output_devices())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake() -> FakeDevices {
        FakeDevices::new(
            vec![
                Device::new("coreaudio:BuiltInMic", "MacBook Pro Microphone"),
                Device::new("coreaudio:USBHeadset-in", "USB Headset"),
            ],
            vec![Device::new("coreaudio:BuiltInSpeaker", "MacBook Pro Speakers")],
        )
    }

    #[test]
    fn keys_are_stable_positive_and_never_the_default() {
        // FNV-1a 64 of "a" is 0xaf63dc4c8601ec8c; the top bit is folded off.
        assert_eq!(key("a"), 0x2f63_dc4c_8601_ec8c);
        assert_eq!(key("coreaudio:BuiltInMic"), key("coreaudio:BuiltInMic"));
        assert_ne!(key("coreaudio:BuiltInMic"), key("coreaudio:BuiltInmic"));
        for id in ["", "x", "coreaudio:BuiltInMic", "a much longer id with spaces"] {
            assert!(key(id) > 0, "{id:?}");
        }
    }

    #[test]
    fn a_key_finds_its_device_and_the_default_or_a_gone_device_finds_none() {
        let f = fake();
        let inputs = f.inputs();
        assert_eq!(inputs.len(), 2);
        let k = inputs[1].key();
        assert_eq!(find(&inputs, k).map(|d| d.name.as_str()), Some("USB Headset"));
        assert_eq!(find(&inputs, SYSTEM_DEFAULT), None);
        assert_eq!(find(&inputs, key("coreaudio:Unplugged")), None);
        // Inputs and outputs are separate lists.
        assert_eq!(find(&f.outputs(), k), None);
        assert_eq!(f.outputs()[0].name, "MacBook Pro Speakers");
    }

    #[test]
    fn an_empty_host_lists_nothing() {
        let f = FakeDevices::default();
        assert!(f.inputs().is_empty() && f.outputs().is_empty());
    }

    /// The real host: every device listed has an id and a name, and the
    /// ids are distinct. Not run by default (a CI host may have no audio).
    #[cfg(feature = "device")]
    #[test]
    #[ignore = "lists this machine's audio devices"]
    fn the_host_lists_devices_with_ids_and_names() {
        for list in [HostDevices.inputs(), HostDevices.outputs()] {
            let mut ids: Vec<&str> = list.iter().map(|d| d.id.as_str()).collect();
            assert!(list.iter().all(|d| !d.id.is_empty() && !d.name.is_empty()), "{list:?}");
            ids.sort();
            ids.dedup();
            assert_eq!(ids.len(), list.len(), "{list:?}");
        }
    }
}
