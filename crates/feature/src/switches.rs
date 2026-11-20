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
                    "H2_FEATURES: {name} is not a switch (crates/feature/src/switches/table.rs lists them)"
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

    /// Headphone binaural filtering for positioned audio, off unless requested.
    pub fn audio_hrtf(&self) -> bool {
        self.feature("audio.hrtf", false)
    }

    /// `features.<id>`.
    pub fn feature(&self, id: &str, default: bool) -> bool {
        self.set.get(id).copied().unwrap_or(default)
    }

    /// Optional extra wet level and direct HF loss for positioned voices.
    pub fn distance_reverb(&self) -> bool {
        self.feature("audio.distance_reverb", false)
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

/// The default for native stages without an explicit setting.
pub(crate) fn native_render_default(switches: &Switches) -> bool {
    switches.feature("impl.render.native", true)
}

/// Native stages use exact keys. The older `impl.render` group controls
/// the lifted render-loop ports independently.
pub(crate) fn native_stage_on(switches: &Switches, stage: &str) -> bool {
    switches.feature(stage, native_render_default(switches))
}

/// Scene walks require both the master renderer and the owning stage.
pub(crate) fn native_walk_on(switches: &Switches, stage: &str, walk: &str) -> bool {
    native_render_default(switches) && native_stage_on(switches, stage) && switches.feature(walk, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_walks_default_on_and_require_the_owning_stage_and_master() {
        let enabled = |spec| {
            let switches = Switches::parse(spec).unwrap();
            [
                crate::native_transparents::walk_on(&switches),
                crate::native_decals::walk_on(&switches),
                crate::native_particles::walk_on(&switches),
                crate::native_particles::effects_walk_on(&switches),
                crate::native_first_person::walk_on(&switches),
                crate::native_hud::walk_on(&switches),
                crate::native_post::walk_on(&switches),
            ]
        };
        assert_eq!(enabled(""), [true; 7]);
        assert_eq!(enabled("impl.render.native=off,impl.render.native_hud_walk=on,impl.render.native_hud=on"), [false; 7]);
        assert_eq!(enabled("impl.render.native_particles_walk=off"), [true, true, false, true, true, true, true]);
        assert_eq!(enabled("impl.render.native_particles=off"), [true, true, false, false, true, true, true]);
    }

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
    fn distance_reverb_is_an_exact_opt_in() {
        assert!(!Switches::defaults().distance_reverb());
        assert!(!Switches::parse("audio=on").unwrap().distance_reverb());
        assert!(
            Switches::parse("audio.distance_reverb=on")
                .unwrap()
                .distance_reverb()
        );
        assert!(
            !Switches::parse("audio.distance_reverb=off")
                .unwrap()
                .distance_reverb()
        );
    }

    #[test]
    fn audio_hrtf_requires_opt_in() {
        assert!(!Switches::defaults().audio_hrtf());
        assert!(!Switches::parse("audio=on").unwrap().audio_hrtf());
        assert!(Switches::parse("audio.hrtf=on").unwrap().audio_hrtf());
        assert!(!Switches::parse("audio.hrtf=off").unwrap().audio_hrtf());
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
