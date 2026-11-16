//! [`Store`]: every setting's running and saved value, the handles, the
//! one writer.

use std::fmt;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::def::{Apply, Def, Value};
use crate::file;

/// Where a setting's value in effect came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    Default,
    File,
    /// The environment variable named.
    Env(&'static str),
    /// A command-line flag: the setting's own (`--fps`) or `--set`.
    Flag(&'static str),
    /// The run decided, for the reason given (`audio is off`).
    Forced(String),
}

impl Source {
    /// A row cannot change a value from here.
    pub fn locks(&self) -> bool {
        !matches!(self, Source::Default | Source::File)
    }
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Source::Default => f.write_str("default"),
            Source::File => f.write_str("settings file"),
            Source::Env(v) => f.write_str(v),
            Source::Flag(v) => f.write_str(v),
            Source::Forced(why) => f.write_str(why),
        }
    }
}

/// A value given on the command line: `key` is a setting key, `value` its
/// text, `flag` how it was given (`--fps`, `--set`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Flag {
    pub key: String,
    pub value: String,
    pub flag: &'static str,
}

impl Flag {
    /// `key=value` from `--set`.
    pub fn parse_set(text: &str) -> Result<Flag, String> {
        let (k, v) = text
            .split_once('=')
            .ok_or_else(|| format!("--set {text:?}: expected key=value"))?;
        Ok(Flag {
            key: k.trim().into(),
            value: v.trim().into(),
            flag: "--set",
        })
    }
}

/// A bad value somewhere, reported once; the next source was used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub key: Option<&'static str>,
    pub message: String,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

/// A setting as a row, the CLI and the log see it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    /// What the run uses now (the handles' value).
    pub value: Value,
    /// What the file holds, or would hold: the user's choice.
    pub saved: Value,
    /// What a row shows: `value` when locked, else `saved`.
    pub shown: Value,
    pub source: Source,
    pub locked: bool,
    /// `shown` takes effect only after a restart (or the next map).
    pub pending: bool,
}

/// What a [`Store::set`] or [`Store::step`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    /// The setting afterwards.
    pub state: State,
    /// The saved value changed.
    pub changed: bool,
    /// Why the file was not written, when it was not.
    pub not_saved: Option<String>,
}

impl Outcome {
    /// What the log says after a row press: `graphics.fps: 60 from the
    /// next launch`, `graphics.fov: set by H2_FOV; row locked`.
    pub fn describe(&self, def: &Def) -> String {
        let name = def.key;
        let s = &self.state;
        match (s.locked, s.pending) {
            (true, _) => format!("{name}: set by {}; row locked", s.source),
            (false, true) => format!("{name}: {} from the next launch", def.name(s.shown)),
            (false, false) => format!("{name}: {}", def.name(s.shown)),
        }
    }
}

/// Why a [`Store::set`] was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SetError {
    Unknown(String),
    Invalid(String),
}

impl fmt::Display for SetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SetError::Unknown(k) => write!(f, "no setting {k:?}"),
            SetError::Invalid(e) => f.write_str(e),
        }
    }
}

impl std::error::Error for SetError {}

/// A type a [`Setting`] handle reads as.
pub trait SettingType: Copy {
    fn from_bits(bits: i64) -> Self;
}

impl SettingType for bool {
    fn from_bits(bits: i64) -> Self {
        bits != 0
    }
}

impl SettingType for i64 {
    fn from_bits(bits: i64) -> Self {
        bits
    }
}

impl SettingType for u32 {
    fn from_bits(bits: i64) -> Self {
        u32::try_from(bits).unwrap_or(0)
    }
}

/// A setting's value in effect, read with one atomic load. Safe to read
/// with guest memory held: it takes no lock.
pub struct Setting<T> {
    cell: Arc<AtomicI64>,
    _t: PhantomData<fn() -> T>,
}

impl<T> Clone for Setting<T> {
    fn clone(&self) -> Self {
        Setting {
            cell: Arc::clone(&self.cell),
            _t: PhantomData,
        }
    }
}

impl<T: SettingType> Setting<T> {
    pub fn get(&self) -> T {
        T::from_bits(self.cell.load(Ordering::Acquire))
    }
}

struct Entry {
    def: &'static Def,
    live: Arc<AtomicI64>,
    running: Value,
    saved: Value,
    source: Source,
}

impl Entry {
    fn state(&self) -> State {
        let locked = self.source.locks();
        let shown = if locked { self.running } else { self.saved };
        State {
            value: self.running,
            saved: self.saved,
            shown,
            source: self.source.clone(),
            locked,
            pending: shown != self.running,
        }
    }

    fn run_with(&mut self, v: Value) -> bool {
        let changed = self.running != v;
        self.running = v;
        self.live.store(v.bits(), Ordering::Release);
        changed
    }
}

type Callback = Arc<dyn Fn(&Value) + Send + Sync>;

struct Inner {
    entries: Vec<Entry>,
    callbacks: Vec<(usize, Callback)>,
}

/// Every setting of a run.
pub struct Store {
    file: Option<PathBuf>,
    inner: Mutex<Inner>,
}

impl Store {
    /// Loads `defs` from `file` (read once; written on each change), the
    /// environment (`env` answers a variable's text) and `flags`. Returns
    /// the store and every problem met, each reported once.
    pub fn load(
        defs: &[&'static Def],
        file: Option<&Path>,
        env: &dyn Fn(&str) -> Option<String>,
        flags: &[Flag],
    ) -> (Store, Vec<Problem>) {
        let mut problems = Vec::new();
        let table = match file.map(file::read_table) {
            Some(Ok(t)) => t,
            Some(Err(e)) => {
                problems.push(Problem {
                    key: None,
                    message: format!("{e}; settings file ignored"),
                });
                None
            }
            None => None,
        };
        let mut keys = std::collections::HashSet::new();
        for d in defs {
            if !keys.insert(d.key) {
                problems.push(Problem {
                    key: Some(d.key),
                    message: format!("{}: defined twice", d.key),
                });
            }
        }
        for f in flags
            .iter()
            .filter(|f| !defs.iter().any(|d| d.key == f.key))
        {
            problems.push(Problem {
                key: None,
                message: format!("{} {}: no such setting; ignored", f.flag, f.key),
            });
        }
        let mut entries = Vec::new();
        for &def in defs {
            let mut report = |message: String| {
                problems.push(Problem {
                    key: Some(def.key),
                    message,
                })
            };
            let from_file = table.as_ref().and_then(|t| {
                file::lookup(t, def)
                    .map_err(|e| {
                        let path = file.map(|p| p.display().to_string()).unwrap_or_default();
                        report(format!("{path}: {e}; ignored"))
                    })
                    .ok()
                    .flatten()
            });
            let from_env = def.env.and_then(|name| {
                let text = env(name)?;
                let text = text.trim();
                if text.is_empty() {
                    return None;
                }
                def.parse_env_text(text)
                    .map_err(|e| report(format!("{name}={text:?}: {e}; ignored")))
                    .ok()
                    .map(|v| (v, Source::Env(name)))
            });
            let from_flag = flags
                .iter()
                .rev()
                .find(|f| f.key == def.key && !f.value.trim().is_empty())
                .and_then(|f| {
                    def.parse(&f.value)
                        .map_err(|e| report(format!("{} {}: {e}; ignored", f.flag, def.key)))
                        .ok()
                        .map(|v| (v, Source::Flag(f.flag)))
                });
            let saved = from_file.unwrap_or(def.default);
            let (running, source) = from_flag.or(from_env).unwrap_or(match from_file {
                Some(v) => (v, Source::File),
                None => (def.default, Source::Default),
            });
            entries.push(Entry {
                def,
                live: Arc::new(AtomicI64::new(running.bits())),
                running,
                saved,
                source,
            });
        }
        let store = Store {
            file: file.map(Path::to_path_buf),
            inner: Mutex::new(Inner {
                entries,
                callbacks: Vec::new(),
            }),
        };
        (store, problems)
    }

    /// A store of `defs` at their defaults, with no file: for tests and
    /// headless runs.
    pub fn defaults(defs: &[&'static Def]) -> Store {
        Store::load(defs, None, &|_| None, &[]).0
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn index(inner: &Inner, key: &str) -> Option<usize> {
        inner.entries.iter().position(|e| e.def.key == key)
    }

    /// The settings file, if the store writes one.
    pub fn file(&self) -> Option<&Path> {
        self.file.as_deref()
    }

    /// Every setting, in definition order.
    pub fn defs(&self) -> Vec<&'static Def> {
        self.lock().entries.iter().map(|e| e.def).collect()
    }

    pub fn def(&self, key: &str) -> Option<&'static Def> {
        let g = self.lock();
        Self::index(&g, key).map(|i| g.entries[i].def)
    }

    /// The handle of `key`. Panics if no such setting was loaded: keys
    /// are static and a test covers every one used.
    pub fn handle<T: SettingType>(&self, key: &str) -> Setting<T> {
        let g = self.lock();
        let i = Self::index(&g, key).unwrap_or_else(|| panic!("no setting {key:?}"));
        Setting {
            cell: Arc::clone(&g.entries[i].live),
            _t: PhantomData,
        }
    }

    pub fn state(&self, key: &str) -> Option<State> {
        let g = self.lock();
        Self::index(&g, key).map(|i| g.entries[i].state())
    }

    /// The value in effect.
    pub fn get(&self, key: &str) -> Option<Value> {
        self.state(key).map(|s| s.value)
    }

    /// The run decides `key` (`reason` says why): it runs with `v`, locked,
    /// whatever the flags, the environment and the file said. The file is
    /// not written.
    pub fn force(&self, key: &str, v: Value, reason: &str) -> Result<(), SetError> {
        let fire = {
            let mut g = self.lock();
            let i = Self::index(&g, key).ok_or_else(|| SetError::Unknown(key.into()))?;
            let v = g.entries[i].def.check(v).map_err(SetError::Invalid)?;
            let e = &mut g.entries[i];
            e.source = Source::Forced(reason.into());
            e.run_with(v).then(|| callbacks(&g, i))
        };
        fire_all(fire, v);
        Ok(())
    }

    /// Sets the user's choice of `key` and saves it. A `Live` setting
    /// changes the handles at once and runs the `on_change` callbacks
    /// (after the store's lock is released); the others keep running with
    /// what they started with. Locked, nothing changes and nothing is
    /// written.
    pub fn set(&self, key: &str, v: Value) -> Result<Outcome, SetError> {
        let (outcome, fire) = {
            let mut g = self.lock();
            let i = Self::index(&g, key).ok_or_else(|| SetError::Unknown(key.into()))?;
            let def = g.entries[i].def;
            let v = def.check(v).map_err(SetError::Invalid)?;
            let e = &mut g.entries[i];
            if e.source.locks() || e.saved == v {
                let state = e.state();
                return Ok(Outcome {
                    state,
                    changed: false,
                    not_saved: None,
                });
            }
            e.saved = v;
            let ran = def.apply == Apply::Live && e.run_with(v);
            let not_saved = self
                .file
                .as_deref()
                .and_then(|p| file::write_value(p, def, v).err());
            let state = g.entries[i].state();
            let fire = ran.then(|| callbacks(&g, i));
            let outcome = Outcome {
                state,
                changed: true,
                not_saved,
            };
            (outcome, fire)
        };
        fire_all(fire, outcome.state.value);
        Ok(outcome)
    }

    /// A row press: the value after the one shown ([`Def::next`]), set.
    pub fn step(&self, key: &str) -> Result<Outcome, SetError> {
        let (def, shown) = {
            let g = self.lock();
            let i = Self::index(&g, key).ok_or_else(|| SetError::Unknown(key.into()))?;
            (g.entries[i].def, g.entries[i].state().shown)
        };
        self.set(key, def.next(shown))
    }

    /// `f` runs with the new value each time `key`'s value in effect
    /// changes, after the store's lock is released.
    pub fn on_change(&self, key: &str, f: impl Fn(&Value) + Send + Sync + 'static) {
        let mut g = self.lock();
        let i = Self::index(&g, key).unwrap_or_else(|| panic!("no setting {key:?}"));
        g.callbacks.push((i, Arc::new(f)));
    }
}

fn callbacks(g: &Inner, i: usize) -> Vec<Callback> {
    g.callbacks
        .iter()
        .filter(|(at, _)| *at == i)
        .map(|(_, f)| Arc::clone(f))
        .collect()
}

fn fire_all(fire: Option<Vec<Callback>>, v: Value) {
    for f in fire.into_iter().flatten() {
        f(&v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::def::tests::{FOV, FPS, SPEAKER};
    use std::sync::atomic::AtomicUsize;

    const ALL: &[&Def] = &[&FOV, &FPS, &SPEAKER];

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("h2-settings-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.join(crate::SETTINGS_FILE)
    }

    fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |k| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.to_string())
        }
    }

    fn load(
        path: &Path,
        e: &'static [(&'static str, &'static str)],
        flags: &[Flag],
    ) -> (Store, Vec<Problem>) {
        Store::load(ALL, Some(path), &env(e), flags)
    }

    fn fps_flag(v: &str) -> Flag {
        Flag {
            key: "graphics.fps".into(),
            value: v.into(),
            flag: "--fps",
        }
    }

    #[test]
    fn precedence_is_flag_env_file_default() {
        let path = tmp("precedence");
        std::fs::write(&path, "[graphics]\nfps = 120\nfov = 90\n").unwrap();
        let (s, p) = load(&path, &[("H2_FPS", "60")], &[fps_flag("uncapped")]);
        assert!(p.is_empty(), "{p:?}");
        let fps = s.state("graphics.fps").unwrap();
        assert_eq!(
            (fps.value, fps.source.clone(), fps.locked),
            (Value::Int(0), Source::Flag("--fps"), true)
        );
        assert_eq!(fps.saved, Value::Int(120));
        let (s, _) = load(&path, &[("H2_FPS", "60")], &[]);
        assert_eq!(
            s.state("graphics.fps").unwrap().source,
            Source::Env("H2_FPS")
        );
        assert_eq!(s.get("graphics.fps"), Some(Value::Int(60)));
        let (s, _) = load(&path, &[], &[]);
        let fov = s.state("graphics.fov").unwrap();
        assert_eq!(
            (fov.value, fov.source, fov.locked),
            (Value::Int(90), Source::File, false)
        );
        assert_eq!(
            s.state("pad_speaker.enabled").unwrap().source,
            Source::Default
        );
        assert!(s.handle::<bool>("pad_speaker.enabled").get());
        // An empty variable is unset.
        let (s, p) = load(&path, &[("H2_FOV", " ")], &[]);
        assert!(p.is_empty());
        assert_eq!(s.get("graphics.fov"), Some(Value::Int(90)));
    }

    #[test]
    fn bad_values_are_problems_and_the_next_source_is_used() {
        let path = tmp("bad");
        std::fs::write(
            &path,
            "[graphics]\nfps = 45\nfov = 95\n\n[pad_speaker]\nenabled = \"yes\"\n",
        )
        .unwrap();
        let (s, p) = load(&path, &[("H2_FOV", "wide")], &[fps_flag("fast")]);
        let keys: Vec<_> = p.iter().map(|p| p.key).collect();
        assert_eq!(
            keys,
            [
                Some("graphics.fov"),
                Some("graphics.fps"),
                Some("graphics.fps"),
                Some("pad_speaker.enabled")
            ]
        );
        assert!(p[0].message.contains("H2_FOV=\"wide\""), "{}", p[0]);
        assert!(
            p[1].message.contains("fps must be 30, 60, 120 or uncapped"),
            "{}",
            p[1]
        );
        assert!(p[2].message.contains("--fps graphics.fps"), "{}", p[2]);
        // The environment's bad value falls to the file; the file's bad
        // values to the defaults.
        assert_eq!(s.get("graphics.fov"), Some(Value::Int(95)));
        assert_eq!(s.get("graphics.fps"), Some(Value::Int(30)));
        assert_eq!(s.get("pad_speaker.enabled"), Some(Value::Bool(true)));
        // A file that is not TOML: one problem, every setting at its default.
        std::fs::write(&path, "not toml [").unwrap();
        let (s, p) = load(&path, &[], &[]);
        assert_eq!(p.len(), 1);
        assert!(p[0].message.ends_with("settings file ignored"));
        assert_eq!(s.get("graphics.fov"), Some(Value::Int(70)));
        // ... and is never overwritten.
        let o = s.set("graphics.fov", Value::Int(90)).unwrap();
        assert!(o.not_saved.unwrap().contains("not overwritten"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "not toml [");
        std::fs::write(&path, "graphics = 3").unwrap();
        let (s, p) = load(&path, &[], &[]);
        assert!(
            p.iter()
                .any(|p| p.message.contains("graphics must be a table"))
        );
        assert!(
            s.set("graphics.fov", Value::Int(90))
                .unwrap()
                .not_saved
                .is_some()
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "graphics = 3");
        // Unknown flag keys and set values out of range.
        let flag = Flag::parse_set("graphics.nope=1").unwrap();
        let (_, p) = load(&tmp("unknown"), &[], &[flag]);
        assert!(p[0].message.contains("no such setting"));
        assert!(Flag::parse_set("graphics.fov").is_err());
        let s = Store::defaults(ALL);
        assert!(matches!(
            s.set("graphics.fov", Value::Int(200)),
            Err(SetError::Invalid(_))
        ));
        assert!(matches!(
            s.set("graphics.nope", Value::Int(1)),
            Err(SetError::Unknown(_))
        ));
    }

    #[test]
    fn changes_persist_across_runs_and_keep_other_keys() {
        let path = tmp("persist");
        std::fs::write(
            &path,
            "[display]\nvsync = false\n\n[pad_speaker]\nenabled = true\nnote = \"x\"\n",
        )
        .unwrap();
        let (s, _) = load(&path, &[], &[]);
        s.set("graphics.fov", Value::Int(95)).unwrap();
        s.set("pad_speaker.enabled", Value::Bool(false)).unwrap();
        s.step("graphics.fps").unwrap();
        s.step("graphics.fps").unwrap();
        s.step("graphics.fps").unwrap();
        let t: toml::Table = std::fs::read_to_string(&path).unwrap().parse().unwrap();
        assert_eq!(t["display"]["vsync"].as_bool(), Some(false));
        assert_eq!(t["pad_speaker"]["note"].as_str(), Some("x"));
        assert_eq!(t["pad_speaker"]["enabled"].as_bool(), Some(false));
        assert_eq!(t["graphics"]["fov"].as_integer(), Some(95));
        assert_eq!(t["graphics"]["fps"].as_str(), Some("uncapped"));
        assert!(!path.with_extension("toml.tmp").exists());
        let (next, p) = load(&path, &[], &[]);
        assert!(p.is_empty(), "{p:?}");
        assert_eq!(next.get("graphics.fov"), Some(Value::Int(95)));
        assert_eq!(next.get("graphics.fps"), Some(Value::Int(0)));
        assert_eq!(next.get("pad_speaker.enabled"), Some(Value::Bool(false)));
    }

    #[test]
    fn todays_settings_file_loads() {
        // As the three writers before this crate left it.
        let path = tmp("today");
        std::fs::write(
            &path,
            "[pad_speaker]\nenabled = false\n\n[graphics]\nfov = 0\nfps = \"uncapped\"\nrender_scale = 2\n",
        )
        .unwrap();
        let (s, p) = load(&path, &[], &[]);
        assert!(p.is_empty(), "{p:?}");
        assert_eq!(s.get("graphics.fov"), Some(Value::Int(70)), "0 is retail");
        assert_eq!(s.get("graphics.fps"), Some(Value::Int(0)));
        assert_eq!(s.get("pad_speaker.enabled"), Some(Value::Bool(false)));
    }

    #[test]
    fn live_settings_apply_at_once_and_call_back_without_the_lock() {
        let s = Arc::new(Store::defaults(ALL));
        let fov = s.handle::<u32>("graphics.fov");
        let calls = Arc::new(AtomicUsize::new(0));
        let (c, inner) = (Arc::clone(&calls), Arc::clone(&s));
        s.on_change("graphics.fov", move |v| {
            // The store's lock is free here.
            assert_eq!(inner.get("graphics.fov"), Some(*v));
            c.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(fov.get(), 70);
        let o = s.step("graphics.fov").unwrap();
        assert_eq!(
            (o.state.value, o.state.pending, o.changed),
            (Value::Int(75), false, true)
        );
        assert_eq!(fov.get(), 75);
        assert_eq!(o.describe(&FOV), "graphics.fov: 75");
        s.set("graphics.fov", Value::Int(75)).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1, "no change, no call");
    }

    #[test]
    fn restart_settings_keep_running_and_saved_apart() {
        let path = tmp("restart");
        let (s, _) = load(&path, &[], &[]);
        let fps = s.handle::<u32>("graphics.fps");
        let o = s.step("graphics.fps").unwrap();
        assert_eq!(fps.get(), 30, "this run keeps its rate");
        assert_eq!(
            (o.state.value, o.state.saved, o.state.shown),
            (Value::Int(30), Value::Int(60), Value::Int(60))
        );
        assert!(o.state.pending);
        assert_eq!(o.describe(&FPS), "graphics.fps: 60 from the next launch");
        // Stepping back round to the running value clears the pending mark.
        for _ in 0..3 {
            s.step("graphics.fps").unwrap();
        }
        let st = s.state("graphics.fps").unwrap();
        assert_eq!((st.shown, st.pending), (Value::Int(30), false));
        // The next launch runs with what was saved.
        s.step("graphics.fps").unwrap();
        let (next, _) = load(&path, &[], &[]);
        assert_eq!(next.handle::<u32>("graphics.fps").get(), 60);
    }

    #[test]
    fn locked_settings_do_not_move_or_write() {
        let path = tmp("locked");
        std::fs::write(&path, "[graphics]\nfov = 100\n").unwrap();
        let (s, _) = load(&path, &[("H2_FOV", "0")], &[]);
        let st = s.state("graphics.fov").unwrap();
        assert_eq!(
            (st.value, st.shown, st.locked),
            (Value::Int(70), Value::Int(70), true)
        );
        let o = s.step("graphics.fov").unwrap();
        assert!(!o.changed);
        assert_eq!(o.state.shown, Value::Int(70));
        assert_eq!(o.describe(&FOV), "graphics.fov: set by H2_FOV; row locked");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[graphics]\nfov = 100\n"
        );
        // The run can force a value too.
        s.force("pad_speaker.enabled", Value::Bool(false), "audio is off")
            .unwrap();
        assert!(!s.handle::<bool>("pad_speaker.enabled").get());
        let o = s.step("pad_speaker.enabled").unwrap();
        assert!(!o.changed && o.state.locked);
        assert_eq!(o.state.source.to_string(), "audio is off");
    }

    #[test]
    fn parse_env_reads_a_richer_variable() {
        static RICH: Def = Def {
            parse_env: Some(|t| Ok(Value::Bool(t != "0" && !t.ends_with("off.toml")))),
            ..SPEAKER_CLONE
        };
        const SPEAKER_CLONE: Def = Def {
            key: "pad_speaker.enabled",
            label: "",
            help: "",
            kind: crate::Kind::Bool,
            default: Value::Bool(true),
            env: Some("H2_PAD_SPEAKER"),
            flag: None,
            aliases: &[],
            parse_env: None,
            apply: Apply::Live,
            scope: crate::Scope::Host,
            tier: crate::Tier::User,
            menu: None,
        };
        let (s, p) = Store::load(
            &[&RICH],
            None,
            &env(&[("H2_PAD_SPEAKER", "/x/off.toml")]),
            &[],
        );
        assert!(p.is_empty());
        assert_eq!(s.get("pad_speaker.enabled"), Some(Value::Bool(false)));
        assert!(s.state("pad_speaker.enabled").unwrap().locked);
    }
}
