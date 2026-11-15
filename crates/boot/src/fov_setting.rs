//! The field-of-view setting behind the "Field of View" row on the game's
//! Controls screen (docs/settings-menu.md "Field of view").
//!
//! The value is the first-person camera's unzoomed horizontal field of view
//! in whole degrees. 70 is the retail value and means "leave the game
//! alone": the camera hook then hands back the game's own number untouched.
//! The row steps it from 70 to 120 in 5-degree steps and wraps; each step
//! applies at once and is written to the settings file as
//!
//! ```toml
//! [graphics]
//! fov = 90
//! ```
//!
//! `H2_FOV`, when set, wins: `0` (or `70`) locks the retail camera, any other
//! whole number from 30 to 150 locks that value, and the row shows it and
//! does nothing. Nothing here touches guest memory.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The retail first-person field of view, in degrees (the observer's
/// default and the stock bipeds' camera value).
pub const RETAIL: u32 = 70;
/// The row's range and step.
pub const ROW_MIN: u32 = RETAIL;
pub const ROW_MAX: u32 = 120;
pub const ROW_STEP: u32 = 5;
/// What the file and `H2_FOV` may hold besides 0 (retail).
pub const ANY_MIN: u32 = 30;
pub const ANY_MAX: u32 = 150;

const TABLE: &str = "graphics";
const KEY: &str = "fov";

/// A value from the file or the variable: 0 is retail, else degrees in
/// [`ANY_MIN`]..=[`ANY_MAX`].
fn valid(v: i64) -> Option<u32> {
    let v = u32::try_from(v).ok()?;
    (v == 0 || (ANY_MIN..=ANY_MAX).contains(&v)).then_some(v)
}

/// Reads the saved value: `None` when the file or the key is absent. A file
/// that is not TOML, or a value that is not a whole number in range, is an
/// error that names the file.
pub fn load(path: &Path) -> Result<Option<u32>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let table: toml::Table = text
        .parse()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    match table.get(TABLE) {
        None => Ok(None),
        Some(toml::Value::Table(t)) => match t.get(KEY) {
            None => Ok(None),
            Some(toml::Value::Integer(i)) => valid(*i).map(Some).ok_or_else(|| {
                format!(
                    "{}: {TABLE}.{KEY} must be 0 or {ANY_MIN} to {ANY_MAX}",
                    path.display()
                )
            }),
            Some(_) => Err(format!(
                "{}: {TABLE}.{KEY} must be a whole number of degrees",
                path.display()
            )),
        },
        Some(_) => Err(format!("{}: {TABLE} must be a table", path.display())),
    }
}

/// Writes the value, keeping every other key in the file. Atomic: a
/// temporary file next to it, then a rename. An unreadable existing file is
/// not overwritten.
pub fn store(path: &Path, degrees: u32) -> Result<(), String> {
    let mut table: toml::Table = match std::fs::read_to_string(path) {
        Ok(t) => t
            .parse()
            .map_err(|e| format!("{}: {e}; not overwritten", path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => toml::Table::new(),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let entry = table
        .entry(TABLE)
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    let toml::Value::Table(t) = entry else {
        return Err(format!(
            "{}: {TABLE} must be a table; not overwritten",
            path.display()
        ));
    };
    t.insert(KEY.into(), toml::Value::Integer(i64::from(degrees)));
    let text = toml::to_string(&table).map_err(|e| format!("{}: {e}", path.display()))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("toml.fov.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

/// What `H2_FOV` forces: `None` when unset or empty (the row and the file
/// decide), `Some(0)` for `0` (retail), `Some(n)` for a whole number of
/// degrees in range. Anything else is an error.
pub fn forced_by_env(var: Option<&str>) -> Result<Option<u32>, String> {
    let Some(v) = var.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    v.parse::<i64>()
        .ok()
        .and_then(valid)
        .map(Some)
        .ok_or_else(|| format!("H2_FOV={v}: must be 0 (retail) or {ANY_MIN} to {ANY_MAX} degrees"))
}

/// The next row value after `shown`: the next 5-degree step, wrapping from
/// 120 back to 70. A value off the steps (a hand-edited file) goes to the
/// next step above it.
pub fn next_step(shown: u32) -> u32 {
    if !(ROW_MIN..ROW_MAX).contains(&shown) {
        return ROW_MIN;
    }
    (shown / ROW_STEP + 1) * ROW_STEP
}

struct Inner {
    /// The user's choice: the row, else the file, else retail (0).
    wanted: u32,
    forced: Option<u32>,
    file: Option<PathBuf>,
}

impl Inner {
    fn degrees(&self) -> u32 {
        self.forced.unwrap_or(self.wanted)
    }
}

/// The live field-of-view value, shared between the Controls row and the
/// camera hook.
pub struct FovSetting {
    inner: Mutex<Inner>,
}

impl FovSetting {
    /// `forced` from [`forced_by_env`]; `file` is read once here (a bad file
    /// is reported and treated as absent) and written on each step.
    pub fn new(forced: Option<u32>, file: Option<PathBuf>) -> Self {
        let saved = file.as_deref().and_then(|p| {
            load(p)
                .map_err(|e| eprintln!("fov: settings file ignored: {e}"))
                .ok()
                .flatten()
        });
        FovSetting {
            inner: Mutex::new(Inner {
                wanted: saved.unwrap_or(0),
                forced,
                file,
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The value in effect, as stored: 0 for retail.
    pub fn degrees(&self) -> u32 {
        self.lock().degrees()
    }

    /// The value the row shows: retail shows as 70.
    pub fn shown(&self) -> u32 {
        match self.degrees() {
            0 => RETAIL,
            d => d,
        }
    }

    /// The camera's unzoomed field of view in radians, or `None` for retail
    /// (0 or 70): the hook then changes nothing.
    pub fn target(&self) -> Option<f32> {
        match self.degrees() {
            0 | RETAIL => None,
            d => Some((d as f32).to_radians()),
        }
    }

    /// `H2_FOV` decides; the row cannot change it.
    pub fn locked(&self) -> bool {
        self.lock().forced.is_some()
    }

    /// Sets the user's choice and saves it. Returns the value shown
    /// afterwards. Locked, it changes nothing and writes nothing.
    pub fn set(&self, degrees: u32) -> u32 {
        {
            let mut g = self.lock();
            if g.forced.is_none() && g.wanted != degrees {
                g.wanted = degrees;
                if let Some(p) = g.file.clone()
                    && let Err(e) = store(&p, degrees)
                {
                    eprintln!("fov: setting not saved: {e}");
                }
            }
        }
        self.shown()
    }

    /// The row was pressed: the next step ([`next_step`], [`Self::set`]).
    pub fn step(&self) -> u32 {
        let next = next_step(self.shown());
        self.set(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("h2-fov-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.join(crate::speaker_setting::SETTINGS_FILE)
    }

    #[test]
    fn retail_by_default() {
        let s = FovSetting::new(None, None);
        assert_eq!(s.degrees(), 0);
        assert_eq!(s.shown(), 70);
        assert_eq!(s.target(), None);
        assert!(!s.locked());
    }

    #[test]
    fn stepping_goes_70_to_120_by_five_and_wraps() {
        let s = FovSetting::new(None, None);
        let mut seen = vec![s.shown()];
        for _ in 0..11 {
            seen.push(s.step());
        }
        assert_eq!(seen, [70, 75, 80, 85, 90, 95, 100, 105, 110, 115, 120, 70]);
        // 70 reached by wrapping is retail again.
        assert_eq!(s.target(), None);
        s.step();
        assert_eq!(s.target(), Some(75f32.to_radians()));
    }

    #[test]
    fn values_off_the_steps_go_to_the_next_step() {
        assert_eq!(next_step(93), 95);
        assert_eq!(next_step(119), 120);
        assert_eq!(next_step(121), 70);
        assert_eq!(next_step(150), 70);
        assert_eq!(next_step(40), 70);
        assert_eq!(next_step(0), 70);
    }

    #[test]
    fn the_choice_persists_across_runs_and_keeps_other_keys() {
        let path = tmp("persist");
        std::fs::write(&path, "[pad_speaker]\nenabled = false\n").unwrap();
        let s = FovSetting::new(None, Some(path.clone()));
        assert_eq!(s.shown(), 70);
        s.set(95);
        assert_eq!(load(&path), Ok(Some(95)));
        assert_eq!(FovSetting::new(None, Some(path.clone())).shown(), 95);
        assert_eq!(crate::speaker_setting::load(&path), Ok(Some(false)));
        let again = FovSetting::new(None, Some(path.clone()));
        again.step();
        assert_eq!(FovSetting::new(None, Some(path.clone())).shown(), 100);
        // The speaker row's writes keep ours.
        crate::speaker_setting::store(&path, true).unwrap();
        assert_eq!(load(&path), Ok(Some(100)));
        assert!(!path.with_extension("toml.fov.tmp").exists());
    }

    #[test]
    fn bad_files_are_reported_and_not_overwritten() {
        let path = tmp("bad");
        std::fs::write(&path, "graphics = 3").unwrap();
        assert!(load(&path).unwrap_err().contains("must be a table"));
        assert!(store(&path, 90).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "graphics = 3");
        std::fs::write(&path, "[graphics]\nfov = 200\n").unwrap();
        assert!(load(&path).unwrap_err().contains("0 or 30 to 150"));
        std::fs::write(&path, "[graphics]\nfov = 90.5\n").unwrap();
        assert!(load(&path).unwrap_err().contains("whole number"));
        // A bad file reads as absent: retail.
        assert_eq!(FovSetting::new(None, Some(path)).target(), None);
        assert_eq!(load(&tmp("missing")), Ok(None));
    }

    #[test]
    fn the_environment_wins() {
        assert_eq!(forced_by_env(None), Ok(None));
        assert_eq!(forced_by_env(Some(" ")), Ok(None));
        assert_eq!(forced_by_env(Some("0")), Ok(Some(0)));
        assert_eq!(forced_by_env(Some("100")), Ok(Some(100)));
        assert_eq!(forced_by_env(Some("93")), Ok(Some(93)));
        assert!(forced_by_env(Some("20")).is_err());
        assert!(forced_by_env(Some("151")).is_err());
        assert!(forced_by_env(Some("-5")).is_err());
        assert!(forced_by_env(Some("wide")).is_err());

        // H2_FOV=0 beats a file that says 100: retail, locked, file kept.
        let path = tmp("env");
        store(&path, 100).unwrap();
        let s = FovSetting::new(Some(0), Some(path.clone()));
        assert!(s.locked());
        assert_eq!(s.target(), None);
        assert_eq!(s.step(), 70);
        assert_eq!(load(&path), Ok(Some(100)));
        // H2_FOV=93 beats the file and shows as is.
        let s = FovSetting::new(Some(93), Some(path.clone()));
        assert_eq!(s.shown(), 93);
        assert_eq!(s.step(), 93);
        assert_eq!(s.target(), Some(93f32.to_radians()));
        assert_eq!(load(&path), Ok(Some(100)));
    }
}
