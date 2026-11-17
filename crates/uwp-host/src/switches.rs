//! The app's run-time switches. A UWP app is started by the shell with no
//! environment and no arguments, so each switch is a default here that a
//! flag file in LocalState overrides (put it there with the Device Portal's
//! file explorer, then relaunch). The app turns a switch into the variable
//! the game reads before the game starts (`shell`).

use std::path::Path;

/// The host perf line (`boot::host_perf`, `H2_HOST_PERF`): where frame
/// time goes, every 100 frames or 5 s in `h2uwp.log`. On by default while
/// the console's frame rate is being worked on.
pub const HOST_PERF_DEFAULT: bool = true;
/// A file of this name in LocalState turns the host perf line off...
pub const HOST_PERF_OFF: &str = "host-perf-off";
/// ...and one of this name turns it on (when the default is off).
pub const HOST_PERF_ON: &str = "host-perf-on";

/// Whether the host perf line is on for an app whose LocalState is
/// `local`: the off file wins, then the on file, then the default.
pub fn host_perf(local: &Path) -> bool {
    flag(local, HOST_PERF_DEFAULT, HOST_PERF_ON, HOST_PERF_OFF)
}

/// A switch with `default`, turned on by a file `on` in `local` and off by
/// a file `off` (which wins). Only the files' metadata is read: the app
/// container refuses opening directories, not looking at files.
pub fn flag(local: &Path, default: bool, on: &str, off: &str) -> bool {
    if local.join(off).is_file() {
        false
    } else if local.join(on).is_file() {
        true
    } else {
        default
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_off_file_wins_then_the_on_file_then_the_default() {
        let d = std::env::temp_dir().join(format!("uwp-switches-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        assert!(flag(&d, true, "on", "off"));
        assert!(!flag(&d, false, "on", "off"));
        std::fs::write(d.join("on"), b"").unwrap();
        assert!(flag(&d, false, "on", "off"));
        std::fs::write(d.join("off"), b"").unwrap();
        assert!(!flag(&d, true, "on", "off"));
        // A directory of the name is not the flag.
        std::fs::remove_file(d.join("off")).unwrap();
        std::fs::create_dir(d.join("off")).unwrap();
        assert!(flag(&d, false, "on", "off"));
        std::fs::remove_dir_all(&d).unwrap();
        assert_eq!(
            host_perf(Path::new("/nonexistent/local/state")),
            HOST_PERF_DEFAULT
        );
    }
}
