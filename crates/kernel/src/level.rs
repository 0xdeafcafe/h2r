//! The map the game has loaded, for host-side display only (`h2 play`'s
//! watermark). Nothing here feeds back into the guest.
//!
//! The authority is the loaded cache header's name, which boot publishes
//! through [`publish`] each time a game starts (the main menu is a game too,
//! so it covers the menus and every level). The file-open record below is
//! only the answer before the first publish: the game opens `.map` files
//! for other reasons than loading them (a header read for a list, a
//! preload, a presence check; the split-screen menu showed `01a_tutorial`
//! while loading Ascension, and the exact opener there is not traced), so a
//! later open never overrides a published name.
//!
//! Every successful `NtCreateFile`/`NtOpenFile` passes its path to
//! [`note_open`]; a file named `<name>.map` in a `maps` directory, other
//! than the shared resource maps, becomes the current map. Process-wide:
//! one game runs per process.

use std::sync::Mutex;

static LAST: Mutex<Option<String>> = Mutex::new(None);
static LOADED: Mutex<Option<String>> = Mutex::new(None);

/// Publishes the map the game has loaded (its cache header's name). An
/// empty name is ignored.
pub fn publish(name: &str) {
    let name = name.trim().to_ascii_lowercase();
    if !name.is_empty() {
        *LOADED.lock().unwrap_or_else(|e| e.into_inner()) = Some(name);
    }
}

/// Maps every level opens alongside its own; they never name a level.
const SHARED: [&str; 2] = ["shared", "single_player_shared"];

/// The level map `path` names: the file stem of `...\maps\<stem>.map`,
/// lower case, unless it is a shared resource map.
pub fn level_map(path: &str) -> Option<String> {
    let mut parts = path.rsplit(['\\', '/']);
    let file = parts.next()?;
    let dir = parts.next()?;
    if !dir.eq_ignore_ascii_case("maps") {
        return None;
    }
    let (stem, ext) = file.rsplit_once('.')?;
    if !ext.eq_ignore_ascii_case("map") || stem.is_empty() {
        return None;
    }
    let stem = stem.to_ascii_lowercase();
    (!SHARED.contains(&stem.as_str())).then_some(stem)
}

/// Records an opened file's path (cheap when it is not a level map).
pub(crate) fn note_open(path: &str) {
    let tail = path.len().saturating_sub(4);
    if !path.is_char_boundary(tail) || !path[tail..].eq_ignore_ascii_case(".map") {
        return;
    }
    if let Some(m) = level_map(path) {
        *LAST.lock().unwrap_or_else(|e| e.into_inner()) = Some(m);
    }
}

/// The map the game has loaded, else (before any game has started) the
/// level map opened last, if any.
pub fn last_map() -> Option<String> {
    if let Some(m) = LOADED.lock().unwrap_or_else(|e| e.into_inner()).clone() {
        return Some(m);
    }
    LAST.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tests share the process-wide slots; one at a time.
    static SERIAL: Mutex<()> = Mutex::new(());

    #[test]
    fn only_level_maps_in_a_maps_directory_count() {
        assert_eq!(
            level_map(r"\Device\CdRom0\maps\mainmenu.map").as_deref(),
            Some("mainmenu")
        );
        assert_eq!(
            level_map(r"\??\D:\MAPS\01A_Tutorial.MAP").as_deref(),
            Some("01a_tutorial")
        );
        assert_eq!(level_map(r"d:\maps\shared.map"), None);
        assert_eq!(level_map(r"d:\maps\single_player_shared.map"), None);
        assert_eq!(level_map(r"z:\cache000.map"), None);
        assert_eq!(level_map(r"d:\maps\fonts\font_table.txt"), None);
        assert_eq!(level_map(r"d:\maps\.map"), None);
        assert_eq!(level_map("mainmenu.map"), None);
    }

    #[test]
    fn the_published_map_beats_later_opens() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        publish("  Ascension ");
        publish("");
        note_open(r"d:\maps\01a_tutorial.map");
        assert_eq!(last_map().as_deref(), Some("ascension"));
        *LOADED.lock().unwrap() = None;
    }

    #[test]
    fn the_last_level_opened_is_current() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        *LOADED.lock().unwrap_or_else(|e| e.into_inner()) = None;
        note_open(r"d:\maps\mainmenu.map");
        note_open(r"d:\maps\shared.map");
        assert_eq!(last_map().as_deref(), Some("mainmenu"));
        note_open(r"d:\maps\02a_outskirts.map");
        note_open(r"z:\cache001.map");
        assert_eq!(last_map().as_deref(), Some("02a_outskirts"));
    }
}
