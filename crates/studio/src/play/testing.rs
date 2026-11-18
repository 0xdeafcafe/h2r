//! Test helpers: the scenario crate's invented map (no game data) in a
//! scratch directory.

use std::path::PathBuf;

use scenario_edit::{MapSource, Session, fixture};

/// A scratch directory, removed on drop.
pub struct Temp(pub PathBuf);

impl Temp {
    pub fn new(tag: &str) -> Temp {
        let dir = std::env::temp_dir().join(format!("h2e-play-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Temp(dir)
    }
    /// The invented map, written as `<name>.map`.
    pub fn map(&self, name: &str) -> PathBuf {
        let p = self.0.join(format!("{name}.map"));
        std::fs::write(&p, fixture::fixture()).unwrap();
        p
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A session with the invented map open.
pub fn session(tag: &str) -> (Temp, Session) {
    let t = Temp::new(tag);
    let mut s = Session::new();
    s.open(MapSource::file(t.map("example"))).unwrap();
    (t, s)
}
