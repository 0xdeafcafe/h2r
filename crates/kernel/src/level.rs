//! The map the game opened last, for host-side display only (`h2 play`'s
//! watermark). Nothing here feeds back into the guest.
//!
//! Every successful `NtCreateFile`/`NtOpenFile` passes its path to
//! [`note_open`]; a file named `<name>.map` in a `maps` directory, other
//! than the shared resource maps, becomes the current map. Process-wide:
//! one game runs per process.

use std::sync::Mutex;

static LAST: Mutex<Option<String>> = Mutex::new(None);

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

/// The level map opened last, if any.
pub fn last_map() -> Option<String> {
    LAST.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn the_last_level_opened_is_current() {
        note_open(r"d:\maps\mainmenu.map");
        note_open(r"d:\maps\shared.map");
        assert_eq!(last_map().as_deref(), Some("mainmenu"));
        note_open(r"d:\maps\02a_outskirts.map");
        note_open(r"z:\cache001.map");
        assert_eq!(last_map().as_deref(), Some("02a_outskirts"));
    }
}
