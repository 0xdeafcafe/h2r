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
//! - the row toggles it through the store (or [`SpeakerSetting::toggle`]),
//!   which applies at once and writes the choice to the settings file.
//!
//! The value itself is `pad_speaker.enabled` in the [`Store`]
//! (`crate::setting_defs::PAD_SPEAKER`): `H2_PAD_SPEAKER`, when set, wins
//! and locks the row, and `h2 play` forces it off when audio is off.
//! Nothing here touches guest memory.

use std::sync::{Arc, Mutex, MutexGuard};

use dsound::padroute::PadRouter;
use settings::{Setting, Store, Value};

use crate::setting_defs::PAD_SPEAKER;

type RouterSink = Box<dyn FnMut(Option<PadRouter>) + Send>;
type HidSink = Box<dyn FnMut(bool) + Send>;

struct Inner {
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
}

impl Inner {
    /// Bring DirectSound and the HID path in line with the setting `on`.
    fn apply(&mut self, on: bool) {
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

/// The live controller-speaker switch: `pad_speaker.enabled` in the
/// [`Store`], plus what it drives. Shared (`Arc`) between the sound index
/// thread and the audio output thread; the rows change it through the
/// store, and the store's change callback applies it here.
pub struct SpeakerSetting {
    store: Arc<Store>,
    on: Setting<bool>,
    inner: Arc<Mutex<Inner>>,
}

fn lock(inner: &Mutex<Inner>) -> MutexGuard<'_, Inner> {
    inner.lock().unwrap_or_else(|e| e.into_inner())
}

impl SpeakerSetting {
    /// The switch over `store`'s `pad_speaker.enabled`. Each change of the
    /// setting (a row press, `Store::set`) is applied at once.
    pub fn new(store: Arc<Store>) -> Self {
        let on = store.handle::<bool>(PAD_SPEAKER.key);
        let inner = Arc::new(Mutex::new(Inner {
            router: None,
            side_live: false,
            install: None,
            hid: None,
            installed: false,
            fresh: false,
            hid_on: false,
        }));
        let (i, h) = (Arc::clone(&inner), on.clone());
        store.on_change(PAD_SPEAKER.key, move |_| lock(&i).apply(h.get()));
        SpeakerSetting { store, on, inner }
    }

    fn apply(&self) {
        lock(&self.inner).apply(self.on.get());
    }

    /// The setting in effect.
    pub fn enabled(&self) -> bool {
        self.on.get()
    }

    /// `H2_PAD_SPEAKER` (or the run) decides; the row cannot change it.
    pub fn locked(&self) -> bool {
        self.store.state(PAD_SPEAKER.key).is_some_and(|s| s.locked)
    }

    /// Where DirectSound's router goes (`DirectSound::set_pad_router`).
    /// Called once the HLE exists; applies the current state at once.
    pub fn attach_router_sink(&self, f: impl FnMut(Option<PadRouter>) + Send + 'static) {
        let mut g = lock(&self.inner);
        g.install = Some(Box::new(f));
        g.installed = false;
        g.apply(self.on.get());
    }

    /// Where the HID speaker path goes (`input::pad_hid::SpeakerSwitch::set`).
    pub fn attach_hid(&self, f: impl FnMut(bool) + Send + 'static) {
        let mut g = lock(&self.inner);
        g.hid = Some(Box::new(f));
        g.hid_on = false;
        g.apply(self.on.get());
    }

    /// A new router from the sound index; installed now if the setting is on,
    /// else kept for when it is turned on.
    pub fn offer_router(&self, router: PadRouter) {
        let mut g = lock(&self.inner);
        g.router = Some(router);
        g.fresh = true;
        g.apply(self.on.get());
    }

    /// The controller output's sink went live (`true`) or down.
    pub fn side_live(&self, live: bool) {
        let mut g = lock(&self.inner);
        g.side_live = live;
        g.apply(self.on.get());
    }

    /// Sets the user's choice, applies it and saves it. Returns the setting
    /// in effect afterwards. Locked, it changes nothing and writes nothing.
    pub fn set(&self, on: bool) -> bool {
        if let Ok(o) = self.store.set(PAD_SPEAKER.key, Value::Bool(on))
            && let Some(e) = o.not_saved
        {
            eprintln!("pad speaker: setting not saved: {e}");
        }
        self.apply();
        self.enabled()
    }

    /// Flips the user's choice ([`Self::set`]).
    pub fn toggle(&self) -> bool {
        let shown = self.store.state(PAD_SPEAKER.key).map(|s| s.shown);
        self.set(shown != Some(Value::Bool(true)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dsound::padroute::PadSpeakerConfig;

    fn fresh() -> SpeakerSetting {
        SpeakerSetting::new(Arc::new(Store::defaults(&[&PAD_SPEAKER])))
    }

    fn router() -> PadRouter {
        let config = PadSpeakerConfig::default();
        PadRouter::new(
            &dsound::padroute::SoundIndex::new(),
            &dsound::padroute::RouteTable::new(&config),
        )
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
        let s = fresh();
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
        let s = fresh();
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
        let s = fresh();
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
        let s = fresh();
        let log = wired(&s);
        for _ in 0..4 {
            s.toggle();
        }
        let l = log.lock().unwrap();
        assert!(l.routers.is_empty());
        assert!(l.hid.is_empty());
        // And with nothing attached at all.
        let bare = fresh();
        assert!(!bare.toggle());
        assert!(bare.toggle());
    }

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("h2-speaker-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        settings::path_in(&d)
    }

    fn stored(path: &std::path::Path, vars: &'static [(&'static str, &'static str)]) -> Arc<Store> {
        let env = move |k: &str| {
            vars.iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.to_string())
        };
        Arc::new(Store::load(&[&PAD_SPEAKER], Some(path), &env, &[]).0)
    }

    #[test]
    fn the_choice_persists_across_runs() {
        let path = tmp("persist");
        let s = SpeakerSetting::new(stored(&path, &[]));
        assert!(s.enabled(), "no file: on");
        s.set(false);
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text, "[pad_speaker]\nenabled = false\n");
        assert!(!SpeakerSetting::new(stored(&path, &[])).enabled());
        let again = SpeakerSetting::new(stored(&path, &[]));
        again.toggle();
        assert!(SpeakerSetting::new(stored(&path, &[])).enabled());
    }

    #[test]
    fn a_press_through_the_store_applies_at_once() {
        let store = Arc::new(Store::defaults(&[&PAD_SPEAKER]));
        let s = SpeakerSetting::new(Arc::clone(&store));
        let log = wired(&s);
        s.offer_router(router());
        s.side_live(true);
        store.step(PAD_SPEAKER.key).unwrap();
        assert!(!s.enabled());
        assert_eq!(log.lock().unwrap().routers, [true, false]);
        assert_eq!(log.lock().unwrap().hid, [true, false]);
    }

    #[test]
    fn the_environment_wins() {
        // H2_PAD_SPEAKER=0 beats a file that says on: locked off, the row
        // changes nothing and the file is not written.
        let path = tmp("env");
        std::fs::write(&path, "[pad_speaker]\nenabled = true\n").unwrap();
        let s = SpeakerSetting::new(stored(&path, &[("H2_PAD_SPEAKER", "0")]));
        let log = wired(&s);
        s.offer_router(router());
        s.side_live(true);
        assert!(!s.enabled() && s.locked());
        assert!(!s.toggle());
        assert!(!s.set(true));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[pad_speaker]\nenabled = true\n"
        );
        assert!(log.lock().unwrap().routers.is_empty());
        assert!(log.lock().unwrap().hid.is_empty());
        // And `1` beats a file that says off.
        std::fs::write(&path, "[pad_speaker]\nenabled = false\n").unwrap();
        let s = SpeakerSetting::new(stored(&path, &[("H2_PAD_SPEAKER", "1")]));
        assert!(s.enabled());
        assert!(s.toggle());
        // Audio off (or bad routes): the run forces it off and locks it.
        let store = stored(&path, &[]);
        store
            .force(PAD_SPEAKER.key, Value::Bool(false), "audio is off")
            .unwrap();
        let s = SpeakerSetting::new(store);
        assert!(!s.enabled() && s.locked());
        assert!(!s.toggle());
    }
}
