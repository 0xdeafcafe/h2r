//! A row of many choices over a setting of the profile store: the sibling
//! of the Halo 1 options' two-choice [`crate::h1_options::OptionRow`], for
//! pages such as a bots page's Count and Difficulty (docs/game/ui-screens.md
//! "Choice rows").
//!
//! A [`ChoiceRow`] names a [`Def`] and how the row reads. Its choices come
//! from the setting: a [`Kind::Choice`]'s labels, a [`Kind::Int`]'s steps
//! (else every whole number of its range), or Off and On for a
//! [`Kind::Bool`]. On a kit screen it is a value row ([`ChoiceRow::row`]):
//! left and right step through the choices and wrap, A steps right, as
//! every value row of the kit does, and each step is saved at once like
//! any setting (the store's change callbacks see it).
//!
//! ```ignore
//! static COUNT: ChoiceRow = ChoiceRow::new(&BOTS_COUNT, "Count", "How many bots join.");
//! let page = KitScreen::new(FEATURE, SCREEN, IDS, "Clankers")
//!     .layout(skulls_menu::LIST_LAYOUT, ValueStyle::Inline)
//!     .row(COUNT.row(Arc::clone(&store)))
//!     .row(DIFFICULTY.row(store));
//! ```

use std::sync::Arc;

use settings::{Def, Kind, Store, Value};

use crate::ui_kit::screen::Row;

/// One row of many choices: a setting, its label and what it does.
#[derive(Debug)]
pub struct ChoiceRow {
    pub def: &'static Def,
    /// The row's name, before its value ("Count: < 4 >").
    pub label: &'static str,
    /// The description pane's text while the row has the focus.
    pub description: &'static str,
}

impl ChoiceRow {
    pub const fn new(def: &'static Def, label: &'static str, description: &'static str) -> Self {
        ChoiceRow {
            def,
            label,
            description,
        }
    }

    /// The choices in order: each value and how the row shows it.
    pub fn choices(&self) -> Vec<(Value, String)> {
        match &self.def.kind {
            Kind::Bool => vec![
                (Value::Bool(false), "Off".into()),
                (Value::Bool(true), "On".into()),
            ],
            Kind::Int { valid, steps, .. } => {
                let all: Vec<i64> = if steps.is_empty() {
                    valid.clone().collect()
                } else {
                    steps.to_vec()
                };
                all.into_iter()
                    .map(|v| (Value::Int(v), v.to_string()))
                    .collect()
            }
            Kind::Choice(cs) => cs
                .iter()
                .map(|c| (Value::Int(c.value), c.label.to_string()))
                .collect(),
        }
    }

    /// The choice `store` holds (the first when its value is none of
    /// them).
    pub fn index(&self, store: &Store) -> usize {
        let now = store.get(self.def.key);
        self.choices()
            .iter()
            .position(|(v, _)| Some(v) == now.as_ref())
            .unwrap_or(0)
    }

    /// Sets choice `i`, saved at once; what to log.
    pub fn set(&self, store: &Store, i: usize) -> String {
        let cs = self.choices();
        let Some((v, name)) = cs.get(i) else {
            return format!("{}: no choice {i}", self.def.key);
        };
        match store.set(self.def.key, *v) {
            Ok(o) if !o.changed && self.index(store) != i => format!(
                "{}: stays {} (locked by its variable)",
                self.def.key,
                cs[self.index(store)].1
            ),
            Ok(o) => match o.not_saved {
                None => format!("{} -> {name}", self.def.key),
                Some(why) => format!("{} -> {name}; not saved: {why}", self.def.key),
            },
            Err(e) => format!("{}: {e}", self.def.key),
        }
    }

    /// One step right (or left), wrapping; what to log.
    pub fn step(&self, store: &Store, right: bool) -> String {
        let n = self.choices().len().max(1);
        let i = self.index(store);
        self.set(store, if right { (i + 1) % n } else { (i + n - 1) % n })
    }

    /// The row on a kit screen over `store`.
    pub fn row(&'static self, store: Arc<Store>) -> Row {
        let names: Vec<String> = self.choices().into_iter().map(|(_, n)| n).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let get = Arc::clone(&store);
        Row::value(
            self.label,
            &names,
            move || self.index(&get),
            move |i| self.set(&store, i),
        )
        .describe(self.description)
    }
}

/// The kit's example of choice rows (`h2 ui preview kit.choices`): a page
/// of two, a count and a difficulty, over settings of its own that no
/// profile file holds.
pub mod example {
    use super::*;
    use settings::{Apply, Choice, Scope, Tier, Unit};

    pub static COUNT_DEF: Def = Def {
        key: "kit.example_count",
        label: "Count",
        help: "The kit's example: how many.",
        kind: Kind::Int {
            valid: 0..=8,
            steps: &[],
            unit: Unit::Plain,
        },
        default: Value::Int(4),
        env: None,
        flag: None,
        aliases: &[],
        parse_env: None,
        apply: Apply::Live,
        scope: Scope::Host,
        tier: Tier::Diagnostic,
        menu: None,
    };

    pub static LEVEL_DEF: Def = Def {
        key: "kit.example_level",
        label: "Difficulty",
        help: "The kit's example: how hard.",
        kind: Kind::Choice(&[
            Choice {
                value: 0,
                name: "easy",
                label: "Easy",
            },
            Choice {
                value: 1,
                name: "normal",
                label: "Normal",
            },
            Choice {
                value: 2,
                name: "heroic",
                label: "Heroic",
            },
            Choice {
                value: 3,
                name: "legendary",
                label: "Legendary",
            },
        ]),
        default: Value::Int(1),
        env: None,
        flag: None,
        aliases: &[],
        parse_env: None,
        apply: Apply::Live,
        scope: Scope::Host,
        tier: Tier::Diagnostic,
        menu: None,
    };

    pub static COUNT: ChoiceRow = ChoiceRow::new(&COUNT_DEF, "Count", "How many join the game.");
    pub static LEVEL: ChoiceRow = ChoiceRow::new(&LEVEL_DEF, "Difficulty", "How well they play.");

    /// A store holding just the example's settings, at their defaults.
    pub fn store() -> Arc<Store> {
        Arc::new(Store::defaults(&[&COUNT_DEF, &LEVEL_DEF]))
    }
}

#[cfg(test)]
mod tests {
    use super::example::*;
    use super::*;

    #[test]
    fn choices_come_from_the_setting() {
        let names =
            |r: &ChoiceRow| -> Vec<String> { r.choices().into_iter().map(|(_, n)| n).collect() };
        assert_eq!(names(&COUNT), ["0", "1", "2", "3", "4", "5", "6", "7", "8"]);
        assert_eq!(names(&LEVEL), ["Easy", "Normal", "Heroic", "Legendary"]);
        let s = store();
        assert_eq!(COUNT.index(&s), 4);
        assert_eq!(LEVEL.index(&s), 1);
    }

    /// Steps wrap both ways and are saved in the store.
    #[test]
    fn steps_wrap_and_save() {
        let s = store();
        assert_eq!(LEVEL.step(&s, false), "kit.example_level -> Easy");
        assert_eq!(LEVEL.step(&s, false), "kit.example_level -> Legendary");
        assert_eq!(s.get(LEVEL_DEF.key), Some(Value::Int(3)));
        assert_eq!(LEVEL.step(&s, true), "kit.example_level -> Easy");
        for _ in 0..4 {
            COUNT.step(&s, true);
        }
        assert_eq!(COUNT.index(&s), 8);
        COUNT.step(&s, true);
        assert_eq!(COUNT.index(&s), 0, "wraps to the first");
        assert!(COUNT.set(&s, 99).contains("no choice"));
    }
}
