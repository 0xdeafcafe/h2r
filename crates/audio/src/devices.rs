//! Choosing a host output device by name, without touching a device (the
//! cpal half is `output::DeviceSink::open_named`). Used for the PlayStation
//! controller speaker; see `docs/audio/playstation-audio.md`.

/// Names macOS gives a wired DualSense's USB audio function
/// (`DualSense Wireless Controller`) and a wired DualShock 4's
/// (`Wireless Controller`). Tried in order by [`pick_controller`].
pub const CONTROLLER_NAMES: &[&str] = &["DualSense", "Wireless Controller"];

/// The first device whose name contains `pattern`, ignoring ASCII case. An
/// empty pattern matches nothing (an unset setting must not pick the first
/// device).
pub fn pick<S: AsRef<str>>(names: &[S], pattern: &str) -> Option<usize> {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return None;
    }
    let want = pattern.to_ascii_lowercase();
    names
        .iter()
        .position(|n| n.as_ref().to_ascii_lowercase().contains(&want))
}

/// `pattern`, or with `auto` each of [`CONTROLLER_NAMES`] in turn.
pub fn pick_controller<S: AsRef<str>>(names: &[S], pattern: &str) -> Option<usize> {
    if pattern.trim().eq_ignore_ascii_case("auto") {
        CONTROLLER_NAMES.iter().find_map(|p| pick(names, p))
    } else {
        pick(names, pattern)
    }
}

/// Where the controller-speaker sounds go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Speaker {
    /// The output device at this index (a DualSense or DualShock 4's audio).
    Device(usize),
    /// A Steam Controller's trackpad actuators (`docs/input/steam-controller.md`):
    /// no audio device, a haptic rendition of the sound.
    Haptics,
}

/// The device setting that asks for the Steam Controller's haptics.
pub const HAPTICS: &str = "haptics";

/// Chooses the controller speaker. `haptics` says the Steam Controller
/// side is running. With `auto`, a PlayStation controller's audio device
/// wins (a real speaker), then the haptics; `haptics` asks for those alone;
/// any other pattern is a device name only.
pub fn pick_speaker<S: AsRef<str>>(names: &[S], pattern: &str, haptics: bool) -> Option<Speaker> {
    let p = pattern.trim();
    if p.eq_ignore_ascii_case(HAPTICS) || p.eq_ignore_ascii_case("steam") {
        return haptics.then_some(Speaker::Haptics);
    }
    if let Some(i) = pick_controller(names, p) {
        return Some(Speaker::Device(i));
    }
    (p.eq_ignore_ascii_case("auto") && haptics).then_some(Speaker::Haptics)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOST: &[&str] = &[
        "MacBook Pro Speakers",
        "Wireless Controller",
        "DualSense Wireless Controller",
    ];

    #[test]
    fn substring_ignoring_case() {
        assert_eq!(pick(HOST, "macbook"), Some(0));
        assert_eq!(pick(HOST, "dualsense"), Some(2));
        assert_eq!(pick(HOST, "  DUALSENSE "), Some(2));
        assert_eq!(pick(HOST, "AirPods"), None);
        assert_eq!(pick(HOST, ""), None);
        assert_eq!(pick(HOST, "   "), None);
    }

    #[test]
    fn auto_prefers_dualsense_over_a_ds4() {
        assert_eq!(pick_controller(HOST, "auto"), Some(2));
        assert_eq!(pick_controller(&HOST[..2], "Auto"), Some(1));
        assert_eq!(pick_controller(&HOST[..1], "auto"), None);
        assert_eq!(pick_controller(HOST, "speakers"), Some(0));
    }

    #[test]
    fn the_speaker_for_each_device() {
        // A DualSense attached: its speaker, haptics or not.
        assert_eq!(pick_speaker(HOST, "auto", true), Some(Speaker::Device(2)));
        assert_eq!(pick_speaker(HOST, "auto", false), Some(Speaker::Device(2)));
        // No PlayStation audio: the Steam Controller's haptics if running.
        let mac = &HOST[..1];
        assert_eq!(pick_speaker(mac, "auto", true), Some(Speaker::Haptics));
        assert_eq!(pick_speaker(mac, "auto", false), None);
        // Asked for by name.
        assert_eq!(pick_speaker(HOST, "haptics", true), Some(Speaker::Haptics));
        assert_eq!(pick_speaker(HOST, " Steam ", true), Some(Speaker::Haptics));
        assert_eq!(pick_speaker(HOST, "haptics", false), None);
        // A named device never falls back to the haptics.
        assert_eq!(pick_speaker(mac, "DualSense", true), None);
        assert_eq!(
            pick_speaker(HOST, "speakers", true),
            Some(Speaker::Device(0))
        );
    }
}
