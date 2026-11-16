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
}
