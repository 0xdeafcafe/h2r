//! `features.<id>` and `impl.<subsystem>[.<entry>]` switches.
//!
//! Read from `H2_FEATURES`: a comma-separated list of `id` (on), `-id`
//! (off) or `id=on|off`. An `impl.` prefix names an implementation switch
//! (`impl.engine=on`). Anything not named keeps its default. These become
//! settings (`Def`s) when the overlay lists them; the variable stays.
//!
//! Every `impl.*` switch is a row of [`table::TABLE`] with its default,
//! owner, age and removal condition; one not there does not parse.

use std::collections::BTreeMap;

pub mod table;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Switches {
    set: BTreeMap<String, bool>,
}

impl Switches {
    /// Every switch at its default.
    pub fn defaults() -> Self {
        Self::default()
    }

    pub fn from_env() -> Result<Self, String> {
        match std::env::var("H2_FEATURES") {
            Ok(v) => Self::parse(&v),
            Err(_) => Ok(Self::default()),
        }
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let mut set = BTreeMap::new();
        for item in text.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            let (name, on) = if let Some(name) = item.strip_prefix('-') {
                (name, false)
            } else if let Some((name, v)) = item.split_once('=') {
                let on = match v.trim() {
                    "on" | "1" | "true" => true,
                    "off" | "0" | "false" => false,
                    other => return Err(format!("H2_FEATURES: {name}={other}: want on or off")),
                };
                (name.trim(), on)
            } else {
                (item, true)
            };
            if name.is_empty() {
                return Err(format!("H2_FEATURES: empty name in {item:?}"));
            }
            if name.starts_with("impl.") && !table::known(name) {
                return Err(format!(
                    "H2_FEATURES: {name} is not a switch (crates/hooking/hooks/src/switches/table.rs lists them)"
                ));
            }
            set.insert(name.to_owned(), on);
        }
        Ok(Switches { set })
    }

    /// Every switch set, sorted, as `name=on` or `name=off` joined by
    /// commas: the same text for the same switches, however `H2_FEATURES`
    /// spelled them (a machine snapshot's build key).
    pub fn spec(&self) -> String {
        self.set
            .iter()
            .map(|(name, on)| format!("{name}={}", if *on { "on" } else { "off" }))
            .collect::<Vec<_>>()
            .join(",")
    }

    /// Sets one switch (`id` or `impl.x`).
    pub fn with(mut self, name: &str, on: bool) -> Self {
        self.set.insert(name.to_owned(), on);
        self
    }

    /// Headphone binaural filtering for positioned audio, on unless `audio.hrtf=off`.
    pub fn audio_hrtf(&self) -> bool {
        self.feature("audio.hrtf", true)
    }

    /// `features.<id>`.
    pub fn feature(&self, id: &str, default: bool) -> bool {
        self.set.get(id).copied().unwrap_or(default)
    }

    /// Extra wet level and direct HF loss for positioned voices, on unless
    /// `audio.distance_reverb=off`.
    pub fn distance_reverb(&self) -> bool {
        self.feature("audio.distance_reverb", true)
    }

    /// `impl.<name>`, falling back to its parent (`impl.ui.row_text` then
    /// `impl.ui`), then `default`.
    pub fn implementation(&self, name: &str, default: bool) -> bool {
        let mut key = format!("impl.{name}");
        loop {
            if let Some(&on) = self.set.get(&key) {
                return on;
            }
            match key.rfind('.') {
                Some(i) if i > "impl".len() => key.truncate(i),
                _ => return default,
            }
        }
    }
}

/// Whether the native renderer is on: always, as there is no other path.
/// Callers drop the check as their files are next touched.
pub fn native_render_on(_: &Switches) -> bool {
    true
}

/// Whether the Rust visibility producers are on: always.
pub fn native_visibility_on(_: &Switches) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_lookup() {
        let s = Switches::parse("lockstep, -live-telemetry, impl.engine=on,x=off").unwrap();
        assert!(s.feature("lockstep", false));
        assert!(!s.feature("live-telemetry", true));
        assert!(!s.feature("x", true));
        assert!(s.feature("unnamed", true));
        assert!(s.implementation("engine", false));
        assert!(
            s.implementation("engine.locale_find", false),
            "falls back to the parent"
        );
        assert!(!s.implementation("ui", false));
        assert!(Switches::parse("a=maybe").is_err());
        assert!(Switches::parse("-").is_err());
    }

    #[test]
    fn distance_reverb_is_on_and_switches_off_exactly() {
        assert!(Switches::defaults().distance_reverb());
        assert!(Switches::parse("audio=on").unwrap().distance_reverb());
        assert!(
            !Switches::parse("audio.distance_reverb=off")
                .unwrap()
                .distance_reverb()
        );
        assert!(
            Switches::parse("audio.distance_reverb=on")
                .unwrap()
                .distance_reverb()
        );
    }

    #[test]
    fn audio_hrtf_is_on_and_switches_off_exactly() {
        assert!(Switches::defaults().audio_hrtf());
        assert!(Switches::parse("audio=on").unwrap().audio_hrtf());
        assert!(!Switches::parse("audio.hrtf=off").unwrap().audio_hrtf());
        assert!(Switches::parse("audio.hrtf=on").unwrap().audio_hrtf());
    }

    #[test]
    fn the_spec_is_canonical() {
        let a = Switches::parse("lockstep, -live-telemetry,impl.engine=1").unwrap();
        let b = Switches::parse("impl.engine=on,live-telemetry=off,lockstep=true").unwrap();
        assert_eq!(a.spec(), "impl.engine=on,live-telemetry=off,lockstep=on");
        assert_eq!(a.spec(), b.spec());
        assert_eq!(Switches::default().spec(), "");
    }
}
