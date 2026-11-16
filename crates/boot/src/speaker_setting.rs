//! The controller-speaker on/off setting behind the "Controller Speaker" row
//! on the game's Controls screen (docs/playstation-audio.md "In-game
//! setting", docs/settings-menu.md).
//!
//! [`SpeakerSetting`] is the one switch the row, the sound index and the
//! controller output all go through:
//!
//! - the sound index offers each new router ([`SpeakerSetting::offer_router`]);
//!   it reaches DirectSound only while the setting is on, and turning the
//!   setting off installs no router, so every sound plays on the main output;
//! - the controller output reports when its sink goes live
//!   ([`SpeakerSetting::side_live`]); the HID speaker path is switched on only
//!   while the sink is live and the setting is on;
//! - the row toggles it ([`SpeakerSetting::toggle`]), which applies at once
//!   and writes the choice to the settings file.
//!
//! `H2_PAD_SPEAKER`, when set, wins: the setting is locked to what it says
//! and the row shows that value and does nothing ([`forced_by_env`]). Nothing
//! here touches guest memory.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use dsound::padroute::{PadRouter, PadSpeakerConfig};

/// The host settings file, under `h2 play`'s data root (`--data`), next to
/// the guest's `disk/` tree but not in it.
pub const SETTINGS_FILE: &str = "settings.toml";

/// The settings file under the data root `data`: `<data>/settings.toml`.
/// Every in-game setting row (Controller Speaker, Field of View) saves here.
pub fn settings_path(data: &Path) -> PathBuf {
    data.join(SETTINGS_FILE)
}
/// The TOML table and key of the setting.
const TABLE: &str = "pad_speaker";
const KEY: &str = "enabled";

/// Reads the saved choice: `None` when the file or the key is absent.
/// A file that is not TOML, or a key that is not a boolean, is an error
/// that names the file.
pub fn load(path: &Path) -> Result<Option<bool>, String> {
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
            Some(toml::Value::Boolean(b)) => Ok(Some(*b)),
            Some(_) => Err(format!(
                "{}: {TABLE}.{KEY} must be true or false",
                path.display()
            )),
        },
        Some(_) => Err(format!("{}: {TABLE} must be a table", path.display())),
    }
}

/// Writes the choice, keeping every other key in the file. Atomic: a
/// temporary file next to it, then a rename. An unreadable existing file is
/// not overwritten.
pub fn store(path: &Path, on: bool) -> Result<(), String> {
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
    t.insert(KEY.into(), toml::Value::Boolean(on));
    let text = toml::to_string(&table).map_err(|e| format!("{}: {e}", path.display()))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

/// What `H2_PAD_SPEAKER` and the run force, if anything: `Some(false)` with
/// audio off or a settings error (`config` `None`), the variable's own
/// on/off when it is set (`0`, `1` or a TOML file's `enabled`), and `None`
/// (the row and the file decide) when it is unset or empty.
pub fn forced_by_env(
    var: Option<&str>,
    config: Option<&PadSpeakerConfig>,
    audio: bool,
) -> Option<bool> {
    let Some(config) = config.filter(|_| audio) else {
        return Some(false);
    };
    var.filter(|v| !v.trim().is_empty()).map(|_| config.enabled)
}

type RouterSink = Box<dyn FnMut(Option<PadRouter>) + Send>;
type HidSink = Box<dyn FnMut(bool) + Send>;

struct Inner {
    /// The user's choice: the row, else the file, else on.
    wanted: bool,
    forced: Option<bool>,
    /// The newest router from the sound index.
    router: Option<PadRouter>,
    /// The controller output's sink is open and connected.
    side_live: bool,
    install: Option<RouterSink>,
    hid: Option<HidSink>,
    /// What was last sent to each, so a toggle that changes nothing sends
    /// nothing.
    installed: bool,
    /// `router` is newer than the one installed.
    fresh: bool,
    hid_on: bool,
    file: Option<PathBuf>,
}

impl Inner {
    fn enabled(&self) -> bool {
        self.forced.unwrap_or(self.wanted)
    }

    /// Bring DirectSound and the HID path in line with the setting.
    fn apply(&mut self) {
        let on = self.enabled();
        let route = on && self.router.is_some();
        if let Some(f) = self.install.as_mut()
            && (route && (self.fresh || !self.installed) || !route && self.installed)
        {
            f(if route { self.router.clone() } else { None });
            self.installed = route;
            self.fresh = false;
        }
        let hid = on && self.side_live;
        if let Some(f) = self.hid.as_mut()
            && hid != self.hid_on
        {
            f(hid);
            self.hid_on = hid;
        }
    }
}

/// The live controller-speaker switch. Shared (`Arc`) between the Controls
/// row, the sound index thread and the audio output thread.
pub struct SpeakerSetting {
    inner: Mutex<Inner>,
}

impl SpeakerSetting {
    /// `forced` from [`forced_by_env`]; `file` is read once here (a bad file
    /// is reported and treated as absent) and written on each toggle.
    pub fn new(forced: Option<bool>, file: Option<PathBuf>) -> Self {
        let saved = file.as_deref().and_then(|p| {
            load(p)
                .map_err(|e| eprintln!("pad speaker: settings file ignored: {e}"))
                .ok()
                .flatten()
        });
        SpeakerSetting {
            inner: Mutex::new(Inner {
                wanted: saved.unwrap_or(true),
                forced,
                router: None,
                side_live: false,
                install: None,
                hid: None,
                installed: false,
                fresh: false,
                hid_on: false,
                file,
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The setting in effect: the forced value, else the user's choice.
    pub fn enabled(&self) -> bool {
        self.lock().enabled()
    }

    /// `H2_PAD_SPEAKER` (or the run) decides; the row cannot change it.
    pub fn locked(&self) -> bool {
        self.lock().forced.is_some()
    }

    /// Where DirectSound's router goes (`DirectSound::set_pad_router`).
    /// Called once the HLE exists; applies the current state at once.
    pub fn attach_router_sink(&self, f: impl FnMut(Option<PadRouter>) + Send + 'static) {
        let mut g = self.lock();
        g.install = Some(Box::new(f));
        g.installed = false;
        g.apply();
    }

    /// Where the HID speaker path goes (`input::pad_hid::SpeakerSwitch::set`).
    pub fn attach_hid(&self, f: impl FnMut(bool) + Send + 'static) {
        let mut g = self.lock();
        g.hid = Some(Box::new(f));
        g.hid_on = false;
        g.apply();
    }

    /// A new router from the sound index; installed now if the setting is on,
    /// else kept for when it is turned on.
    pub fn offer_router(&self, router: PadRouter) {
        let mut g = self.lock();
        g.router = Some(router);
        g.fresh = true;
        g.apply();
    }

    /// The controller output's sink went live (`true`) or down.
    pub fn side_live(&self, live: bool) {
        let mut g = self.lock();
        g.side_live = live;
        g.apply();
    }

    /// Sets the user's choice, applies it and saves it. Returns the setting
    /// in effect afterwards. Locked, it changes nothing and writes nothing.
    pub fn set(&self, on: bool) -> bool {
        let mut g = self.lock();
        if g.forced.is_some() || g.wanted == on {
            return g.enabled();
        }
        g.wanted = on;
        g.apply();
        if let Some(p) = g.file.clone()
            && let Err(e) = store(&p, on)
        {
            eprintln!("pad speaker: setting not saved: {e}");
        }
        g.enabled()
    }

    /// Flips the user's choice ([`Self::set`]).
    pub fn toggle(&self) -> bool {
        let on = !self.lock().wanted;
        self.set(on)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn router() -> PadRouter {
        let config = PadSpeakerConfig::default();
        PadRouter::new(
            &dsound::padroute::SoundIndex::new(),
            &dsound::padroute::RouteTable::new(&config),
        )
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("h2-speaker-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.join(SETTINGS_FILE)
    }

    /// Records what reached DirectSound (`true` = a router) and the HID path.
    #[derive(Default)]
    struct Log {
        routers: Vec<bool>,
        hid: Vec<bool>,
    }

    fn wired(s: &SpeakerSetting) -> Arc<Mutex<Log>> {
        let log = Arc::new(Mutex::new(Log::default()));
        let l = Arc::clone(&log);
        s.attach_router_sink(move |r| l.lock().unwrap().routers.push(r.is_some()));
        let l = Arc::clone(&log);
        s.attach_hid(move |on| l.lock().unwrap().hid.push(on));
        log
    }

    #[test]
    fn on_by_default_and_routes_once_a_router_arrives() {
        let s = SpeakerSetting::new(None, None);
        let log = wired(&s);
        assert!(s.enabled() && !s.locked());
        assert!(
            log.lock().unwrap().routers.is_empty(),
            "nothing to install yet"
        );
        s.offer_router(router());
        s.side_live(true);
        assert_eq!(log.lock().unwrap().routers, [true]);
        assert_eq!(log.lock().unwrap().hid, [true]);
        // Each newer router from the index replaces the installed one.
        s.offer_router(router());
        assert_eq!(log.lock().unwrap().routers, [true, true]);
        s.side_live(false);
        assert_eq!(log.lock().unwrap().hid, [true, false]);
    }

    #[test]
    fn toggling_off_on_off_applies_live() {
        let s = SpeakerSetting::new(None, None);
        let log = wired(&s);
        s.offer_router(router());
        s.side_live(true);
        assert!(!s.toggle());
        assert!(s.toggle());
        assert!(!s.toggle());
        let l = log.lock().unwrap();
        // Off removes the router (every sound on the main output) and puts
        // the pad's audio back on the jack; on restores both.
        assert_eq!(l.routers, [true, false, true, false]);
        assert_eq!(l.hid, [true, false, true, false]);
    }

    #[test]
    fn off_keeps_new_routers_back_until_turned_on() {
        let s = SpeakerSetting::new(None, None);
        let log = wired(&s);
        s.set(false);
        s.offer_router(router());
        s.offer_router(router());
        s.side_live(true);
        assert_eq!(log.lock().unwrap().routers, Vec::<bool>::new());
        assert_eq!(log.lock().unwrap().hid, Vec::<bool>::new());
        s.set(true);
        assert_eq!(log.lock().unwrap().routers, [true]);
        assert_eq!(log.lock().unwrap().hid, [true]);
    }

    #[test]
    fn without_a_controller_toggling_is_harmless() {
        // No router ever arrives and the side never goes live: toggling
        // sends nothing to the HID path and installs nothing.
        let s = SpeakerSetting::new(None, None);
        let log = wired(&s);
        for _ in 0..4 {
            s.toggle();
        }
        let l = log.lock().unwrap();
        assert!(l.routers.is_empty());
        assert!(l.hid.is_empty());
        // And with nothing attached at all.
        let bare = SpeakerSetting::new(None, None);
        assert!(!bare.toggle());
        assert!(bare.toggle());
    }

    #[test]
    fn the_choice_persists_across_runs() {
        let path = tmp("persist");
        let s = SpeakerSetting::new(None, Some(path.clone()));
        assert!(s.enabled(), "no file: on");
        s.set(false);
        assert_eq!(load(&path), Ok(Some(false)));
        assert!(!SpeakerSetting::new(None, Some(path.clone())).enabled());
        let again = SpeakerSetting::new(None, Some(path.clone()));
        again.toggle();
        assert!(SpeakerSetting::new(None, Some(path.clone())).enabled());
        assert!(!path.with_extension("toml.tmp").exists());
    }

    #[test]
    fn storing_keeps_other_keys() {
        let path = tmp("keys");
        std::fs::write(
            &path,
            "[display]\nvsync = false\n\n[pad_speaker]\nenabled = true\nnote = \"x\"\n",
        )
        .unwrap();
        store(&path, false).unwrap();
        let t: toml::Table = std::fs::read_to_string(&path).unwrap().parse().unwrap();
        assert_eq!(t["display"]["vsync"].as_bool(), Some(false));
        assert_eq!(t["pad_speaker"]["note"].as_str(), Some("x"));
        assert_eq!(load(&path), Ok(Some(false)));
    }

    #[test]
    fn bad_files_are_reported_and_not_overwritten() {
        let path = tmp("bad");
        std::fs::write(&path, "pad_speaker = 3").unwrap();
        assert!(load(&path).unwrap_err().contains("must be a table"));
        assert!(store(&path, true).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "pad_speaker = 3");
        std::fs::write(&path, "[pad_speaker]\nenabled = \"yes\"").unwrap();
        assert!(load(&path).unwrap_err().contains("true or false"));
        std::fs::write(&path, "not toml [").unwrap();
        assert!(load(&path).is_err());
        assert!(store(&path, true).is_err());
        // A bad file reads as absent: on.
        assert!(SpeakerSetting::new(None, Some(path)).enabled());
        assert_eq!(load(&tmp("missing")), Ok(None));
    }

    #[test]
    fn the_environment_wins() {
        let on = PadSpeakerConfig::default();
        let off = PadSpeakerConfig {
            enabled: false,
            ..PadSpeakerConfig::default()
        };
        assert_eq!(forced_by_env(None, Some(&on), true), None);
        assert_eq!(forced_by_env(Some(""), Some(&on), true), None);
        assert_eq!(forced_by_env(Some("0"), Some(&off), true), Some(false));
        assert_eq!(forced_by_env(Some("1"), Some(&on), true), Some(true));
        assert_eq!(
            forced_by_env(Some("/x.toml"), Some(&off), true),
            Some(false)
        );
        // No audio, or a bad setting: off, locked.
        assert_eq!(forced_by_env(None, Some(&on), false), Some(false));
        assert_eq!(forced_by_env(None, None, true), Some(false));

        // H2_PAD_SPEAKER=0 beats a file that says on: locked off, the row
        // changes nothing and the file is not written.
        let path = tmp("env");
        store(&path, true).unwrap();
        let s = SpeakerSetting::new(Some(false), Some(path.clone()));
        let log = wired(&s);
        s.offer_router(router());
        s.side_live(true);
        assert!(!s.enabled() && s.locked());
        assert!(!s.toggle());
        assert!(!s.set(true));
        assert_eq!(load(&path), Ok(Some(true)));
        assert!(log.lock().unwrap().routers.is_empty());
        assert!(log.lock().unwrap().hid.is_empty());
        // And `1` beats a file that says off.
        store(&path, false).unwrap();
        let s = SpeakerSetting::new(Some(true), Some(path.clone()));
        assert!(s.enabled());
        assert!(s.toggle());
        assert_eq!(load(&path), Ok(Some(false)));
    }
}
