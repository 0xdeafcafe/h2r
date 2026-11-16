//! `features.<id>` and `impl.<subsystem>[.<entry>]` switches.
//!
//! Read from `H2_FEATURES`: a comma-separated list of `id` (on), `-id`
//! (off) or `id=on|off`. An `impl.` prefix names an implementation switch
//! (`impl.engine=on`). Anything not named keeps its default. These become
//! settings (`Def`s) when the overlay lists them; the variable stays.

use std::collections::BTreeMap;

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
            set.insert(name.to_owned(), on);
        }
        Ok(Switches { set })
    }

    /// Sets one switch (`id` or `impl.x`).
    pub fn with(mut self, name: &str, on: bool) -> Self {
        self.set.insert(name.to_owned(), on);
        self
    }

    /// `features.<id>`.
    pub fn feature(&self, id: &str, default: bool) -> bool {
        self.set.get(id).copied().unwrap_or(default)
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
}
