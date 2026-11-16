//! The launch-time video settings behind the Video screen's "Frame Rate" and
//! "Resolution" rows (docs/settings-menu.md "Video screen").
//!
//! Both are read once when `h2 play` starts (the window's display rate and
//! the render scale are fixed for a run), so a row press changes the value
//! saved for the next launch and the row says "(restart)" until then. Saved
//! in the settings file as
//!
//! ```toml
//! [graphics]
//! fps = 60            # 30, 60, 120 or "uncapped"
//! render_scale = 2    # 1 to 6; the row steps 1 to 4
//! ```
//!
//! The environment wins, as for the other rows: `H2_FPS` (or `h2 play
//! --fps`) and `H2_RENDER_SCALE`, when set, decide the run, the row shows
//! that value and pressing it does nothing. A bad variable is reported and
//! ignored. Nothing here touches guest memory.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

const TABLE: &str = "graphics";

/// `fps` value for "uncapped".
pub const UNCAPPED: u32 = 0;
/// The largest render scale the file or `H2_RENDER_SCALE` may hold
/// (`d3d::scaled::MAX_SCALE`).
pub const MAX_SCALE: u32 = d3d::scaled::MAX_SCALE;

/// Which setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The window's display rate (`H2_FPS`): 30, 60, 120 or [`UNCAPPED`].
    Fps,
    /// The render scale (`H2_RENDER_SCALE`): 1 to [`MAX_SCALE`].
    RenderScale,
}

impl Kind {
    pub fn key(self) -> &'static str {
        match self {
            Kind::Fps => "fps",
            Kind::RenderScale => "render_scale",
        }
    }

    pub fn env(self) -> &'static str {
        match self {
            Kind::Fps => "H2_FPS",
            Kind::RenderScale => "H2_RENDER_SCALE",
        }
    }

    /// The value with nothing saved and nothing set.
    pub fn default_value(self) -> u32 {
        match self {
            Kind::Fps => 30,
            Kind::RenderScale => 1,
        }
    }

    /// The values the row steps through, in order (it wraps).
    pub fn steps(self) -> &'static [u32] {
        match self {
            Kind::Fps => &[30, 60, 120, UNCAPPED],
            Kind::RenderScale => &[1, 2, 3, 4],
        }
    }

    /// Every value the file or the variable may hold.
    pub fn valid(self, v: u32) -> bool {
        match self {
            Kind::Fps => [30, 60, 120, UNCAPPED].contains(&v),
            Kind::RenderScale => (1..=MAX_SCALE).contains(&v),
        }
    }

    /// Every value the row can show, for its strings.
    pub fn all(self) -> Vec<u32> {
        match self {
            Kind::Fps => vec![30, 60, 120, UNCAPPED],
            Kind::RenderScale => (1..=MAX_SCALE).collect(),
        }
    }

    /// The value as the variable spells it: `30`, `uncapped`, `2`.
    pub fn name(self, v: u32) -> String {
        match (self, v) {
            (Kind::Fps, UNCAPPED) => "uncapped".into(),
            _ => v.to_string(),
        }
    }

    /// The value as the row shows it: `30`, `Uncapped`, `2x`.
    pub fn label(self, v: u32) -> String {
        match (self, v) {
            (Kind::Fps, UNCAPPED) => "Uncapped".into(),
            (Kind::Fps, v) => v.to_string(),
            (Kind::RenderScale, v) => format!("{v}x"),
        }
    }

    /// A variable's text: `None` when unset or empty.
    pub fn parse(self, s: Option<&str>) -> Result<Option<u32>, String> {
        let Some(t) = s.map(str::trim).filter(|t| !t.is_empty()) else {
            return Ok(None);
        };
        let v = match (self, t.to_ascii_lowercase().as_str()) {
            (Kind::Fps, "uncapped" | "unlimited" | "max") => Some(UNCAPPED),
            (_, n) => n.parse::<u32>().ok().filter(|v| *v != UNCAPPED || self == Kind::RenderScale),
        };
        v.filter(|v| self.valid(*v)).map(Some).ok_or_else(|| match self {
            Kind::Fps => format!("{}={t:?}: expected 30, 60, 120 or uncapped", self.env()),
            Kind::RenderScale => format!("{}={t:?}: expected a whole number from 1 to {MAX_SCALE}", self.env()),
        })
    }

    /// The value after `v` on the row: the next step, wrapping; a value off
    /// the steps (5x from a file) goes back to the first.
    pub fn next(self, v: u32) -> u32 {
        let s = self.steps();
        match s.iter().position(|x| *x == v) {
            Some(i) => s[(i + 1) % s.len()],
            None => s[0],
        }
    }

    fn to_toml(self, v: u32) -> toml::Value {
        match (self, v) {
            (Kind::Fps, UNCAPPED) => toml::Value::String("uncapped".into()),
            _ => toml::Value::Integer(i64::from(v)),
        }
    }

    fn read_toml(self, v: &toml::Value) -> Option<u32> {
        let v = match v {
            toml::Value::Integer(i) => u32::try_from(*i).ok().filter(|v| *v != UNCAPPED || self == Kind::RenderScale)?,
            toml::Value::String(s) if self == Kind::Fps => self.parse(Some(s)).ok().flatten()?,
            _ => return None,
        };
        self.valid(v).then_some(v)
    }
}

/// Reads the saved value of `kind`: `None` when the file or the key is
/// absent. A file that is not TOML, or a value out of range, is an error
/// that names the file.
pub fn load(path: &Path, kind: Kind) -> Result<Option<u32>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let table: toml::Table = text.parse().map_err(|e| format!("{}: {e}", path.display()))?;
    match table.get(TABLE) {
        None => Ok(None),
        Some(toml::Value::Table(t)) => match t.get(kind.key()) {
            None => Ok(None),
            Some(v) => kind
                .read_toml(v)
                .map(Some)
                .ok_or_else(|| format!("{}: {TABLE}.{} is not a value it can hold", path.display(), kind.key())),
        },
        Some(_) => Err(format!("{}: {TABLE} must be a table", path.display())),
    }
}

/// Writes the value, keeping every other key. Atomic (a temporary file,
/// then a rename); an unreadable existing file is not overwritten.
pub fn store(path: &Path, kind: Kind, v: u32) -> Result<(), String> {
    let mut table: toml::Table = match std::fs::read_to_string(path) {
        Ok(t) => t.parse().map_err(|e| format!("{}: {e}; not overwritten", path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => toml::Table::new(),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let entry = table.entry(TABLE).or_insert_with(|| toml::Value::Table(toml::Table::new()));
    let toml::Value::Table(t) = entry else {
        return Err(format!("{}: {TABLE} must be a table; not overwritten", path.display()));
    };
    t.insert(kind.key().into(), kind.to_toml(v));
    let text = toml::to_string(&table).map_err(|e| format!("{}: {e}", path.display()))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension(format!("toml.{}.tmp", kind.key()));
    std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

struct Inner {
    /// What the next launch gets: the row, else the file, else the default.
    wanted: u32,
    file: Option<PathBuf>,
}

/// One launch-time setting: the value this run uses, and the one saved for
/// the next.
pub struct LaunchSetting {
    kind: Kind,
    /// The value this run started with.
    running: u32,
    /// Set by the environment (or a command-line flag): the row cannot
    /// change it.
    locked: bool,
    inner: Mutex<Inner>,
}

impl LaunchSetting {
    /// `forced` is the environment's (or a flag's) value, already parsed;
    /// `file` is read once here (a bad file is reported and treated as
    /// absent) and written on each press.
    pub fn new(kind: Kind, forced: Option<u32>, file: Option<PathBuf>) -> Self {
        let saved = file.as_deref().and_then(|p| {
            load(p, kind)
                .map_err(|e| eprintln!("{}: settings file ignored: {e}", kind.key()))
                .ok()
                .flatten()
        });
        let wanted = saved.unwrap_or(kind.default_value());
        LaunchSetting {
            kind,
            running: forced.unwrap_or(wanted),
            locked: forced.is_some(),
            inner: Mutex::new(Inner { wanted, file }),
        }
    }

    /// [`Self::new`] with the variable read from `var` (its text): a bad
    /// value is reported and ignored.
    pub fn from_var(kind: Kind, var: Option<&str>, file: Option<PathBuf>) -> Self {
        let forced = kind
            .parse(var)
            .map_err(|e| eprintln!("play: {e}; ignored"))
            .unwrap_or(None);
        Self::new(kind, forced, file)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// The value this run uses.
    pub fn running(&self) -> u32 {
        self.running
    }

    /// The value as `H2_FPS` / `H2_RENDER_SCALE` would spell it, for the
    /// code that reads the setting at launch.
    pub fn running_name(&self) -> String {
        self.kind.name(self.running)
    }

    pub fn locked(&self) -> bool {
        self.locked
    }

    /// What the row shows: the environment's value when locked, else the
    /// value saved for the next launch.
    pub fn shown(&self) -> u32 {
        if self.locked { self.running } else { self.lock().wanted }
    }

    /// Whether what the row shows only takes effect after a restart.
    pub fn pending(&self) -> bool {
        self.shown() != self.running
    }

    /// The row was pressed: step and save. Locked, nothing changes and
    /// nothing is written. Returns the value shown afterwards.
    pub fn step(&self) -> u32 {
        if self.locked {
            return self.running;
        }
        let mut g = self.lock();
        g.wanted = self.kind.next(g.wanted);
        if let Some(p) = g.file.clone()
            && let Err(e) = store(&p, self.kind, g.wanted)
        {
            eprintln!("{}: setting not saved: {e}", self.kind.key());
        }
        g.wanted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("h2-video-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.join(crate::speaker_setting::SETTINGS_FILE)
    }

    #[test]
    fn the_rows_step_and_wrap() {
        let mut v = 30;
        let mut seen = vec![v];
        for _ in 0..4 {
            v = Kind::Fps.next(v);
            seen.push(v);
        }
        assert_eq!(seen, [30, 60, 120, UNCAPPED, 30]);
        let mut v = 1;
        let mut seen = vec![v];
        for _ in 0..4 {
            v = Kind::RenderScale.next(v);
            seen.push(v);
        }
        assert_eq!(seen, [1, 2, 3, 4, 1]);
        // 5x and 6x (file or variable only) step back to 1x.
        assert_eq!(Kind::RenderScale.next(6), 1);
        assert_eq!(Kind::Fps.next(45), 30);
    }

    #[test]
    fn variables_parse_as_the_launch_code_reads_them() {
        assert_eq!(Kind::Fps.parse(None), Ok(None));
        assert_eq!(Kind::Fps.parse(Some(" ")), Ok(None));
        assert_eq!(Kind::Fps.parse(Some("60")), Ok(Some(60)));
        assert_eq!(Kind::Fps.parse(Some("Uncapped")), Ok(Some(UNCAPPED)));
        assert!(Kind::Fps.parse(Some("0")).is_err());
        assert!(Kind::Fps.parse(Some("45")).is_err());
        assert_eq!(Kind::RenderScale.parse(Some("6")), Ok(Some(6)));
        assert!(Kind::RenderScale.parse(Some("0")).is_err());
        assert!(Kind::RenderScale.parse(Some("7")).is_err());
        assert!(Kind::RenderScale.parse(Some("2x")).is_err());
        for k in [Kind::Fps, Kind::RenderScale] {
            for v in k.all() {
                assert_eq!(k.parse(Some(&k.name(v))), Ok(Some(v)), "{k:?} {v}");
            }
        }
        assert_eq!(Kind::Fps.label(UNCAPPED), "Uncapped");
        assert_eq!(Kind::RenderScale.label(3), "3x");
    }

    #[test]
    fn a_press_saves_for_the_next_launch_and_keeps_other_keys() {
        let path = tmp("persist");
        std::fs::write(&path, "[pad_speaker]\nenabled = false\n\n[graphics]\nfov = 90\n").unwrap();
        let fps = LaunchSetting::new(Kind::Fps, None, Some(path.clone()));
        assert_eq!((fps.running(), fps.shown(), fps.pending()), (30, 30, false));
        assert_eq!(fps.step(), 60);
        assert_eq!(fps.running(), 30, "this run keeps its rate");
        assert!(fps.pending());
        assert_eq!(load(&path, Kind::Fps), Ok(Some(60)));
        fps.step();
        fps.step();
        assert_eq!(load(&path, Kind::Fps), Ok(Some(UNCAPPED)));
        let t: toml::Table = std::fs::read_to_string(&path).unwrap().parse().unwrap();
        assert_eq!(t["graphics"]["fps"].as_str(), Some("uncapped"));
        let scale = LaunchSetting::new(Kind::RenderScale, None, Some(path.clone()));
        scale.step();
        assert_eq!(load(&path, Kind::RenderScale), Ok(Some(2)));
        // Everything else is kept.
        assert_eq!(crate::speaker_setting::load(&path), Ok(Some(false)));
        assert_eq!(crate::fov_setting::load(&path), Ok(Some(90)));
        assert_eq!(load(&path, Kind::Fps), Ok(Some(UNCAPPED)));
        // The next launch runs with them.
        let next = LaunchSetting::new(Kind::Fps, None, Some(path.clone()));
        assert_eq!((next.running(), next.running_name(), next.pending()), (UNCAPPED, "uncapped".into(), false));
        assert_eq!(LaunchSetting::new(Kind::RenderScale, None, Some(path.clone())).running(), 2);
        assert!(!path.with_extension("toml.fps.tmp").exists());
        // A press back to the running value clears "(restart)".
        let s = LaunchSetting::new(Kind::RenderScale, None, Some(path.clone()));
        for _ in 0..3 {
            s.step();
        }
        assert!(s.pending());
        s.step();
        assert_eq!(s.shown(), 2);
        assert!(!s.pending());
    }

    #[test]
    fn bad_files_are_reported_and_not_overwritten() {
        let path = tmp("bad");
        std::fs::write(&path, "graphics = 3").unwrap();
        assert!(load(&path, Kind::Fps).unwrap_err().contains("must be a table"));
        assert!(store(&path, Kind::Fps, 60).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "graphics = 3");
        for bad in ["fps = 45", "fps = 0", "fps = \"fast\"", "render_scale = 9", "render_scale = \"2\""] {
            std::fs::write(&path, format!("[graphics]\n{bad}\n")).unwrap();
            let k = if bad.starts_with("fps") { Kind::Fps } else { Kind::RenderScale };
            assert!(load(&path, k).is_err(), "{bad}");
            // Read as absent: the default.
            assert_eq!(LaunchSetting::new(k, None, Some(path.clone())).running(), k.default_value());
        }
        assert_eq!(load(&tmp("missing"), Kind::Fps), Ok(None));
    }

    #[test]
    fn the_environment_wins_and_locks_the_row() {
        let path = tmp("env");
        store(&path, Kind::Fps, 120).unwrap();
        let s = LaunchSetting::from_var(Kind::Fps, Some("60"), Some(path.clone()));
        assert!(s.locked());
        assert_eq!((s.running(), s.shown(), s.pending()), (60, 60, false));
        assert_eq!(s.step(), 60);
        assert_eq!(load(&path, Kind::Fps), Ok(Some(120)), "the file is left alone");
        // A bad variable is ignored: the file decides.
        let s = LaunchSetting::from_var(Kind::Fps, Some("fast"), Some(path.clone()));
        assert!(!s.locked());
        assert_eq!(s.running(), 120);
        // Unset: the file.
        let s = LaunchSetting::from_var(Kind::RenderScale, None, Some(path.clone()));
        assert_eq!(s.running(), 1);
        let s = LaunchSetting::from_var(Kind::RenderScale, Some("5"), None);
        assert_eq!((s.running(), s.shown()), (5, 5));
    }
}
