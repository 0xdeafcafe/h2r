//! A row of many choices over a setting of the profile store
//! (docs/game/ui-screens.md "Choice rows").
//!
//! A setting's choices come from its [`Def`]: a [`Kind::Choice`]'s labels,
//! a [`Kind::Int`]'s steps (else every whole number of its range), or Off
//! and On for a [`Kind::Bool`] ([`choices`]). A settings dialog's
//! `setting:` row is one of these with no Rust of its own
//! (`crate::ui_kit::settings`): left and right step through the choices
//! and wrap ([`step`]), A opens the value picker on them
//! (`crate::ui_kit::picker`, retail's `low_level_settings`), and each
//! change is saved at once like any setting ([`set`]; the store's change
//! callbacks see it).
//!
//! A [`ChoiceRow`] is the same row for a kit screen that is not a settings
//! dialog (the kit's list examples):
//!
//! ```ignore
//! static COUNT: ChoiceRow = ChoiceRow::new(&BOTS_COUNT, "Count", "How many bots join.");
//! // Its title, layout and legend: assets/ui/screens/<SCREEN>.yaml.
//! let page = KitScreen::new(FEATURE, SCREEN, IDS)
//!     .row(COUNT.row(Arc::clone(&store)))
//!     .row(DIFFICULTY.row(store));
//! ```

use std::sync::Arc;

use settings::{Def, Kind, Store, Value};

use crate::ui_kit::screen::Row;

/// Setting `def`'s choices in order: each value and how a row shows it.
pub fn choices(def: &Def) -> Vec<(Value, String)> {
    match &def.kind {
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
        Kind::Resolution(_) => vec![(def.default, def.label(def.default))],
        Kind::Choice(cs) => cs
            .iter()
            .map(|c| (Value::Int(c.value), c.label.to_string()))
            .collect(),
    }
}

/// The choice of `def` that `store` holds (the first when its value is
/// none of them).
pub fn index(def: &Def, store: &Store) -> usize {
    let now = store.get(def.key);
    choices(def)
        .iter()
        .position(|(v, _)| Some(v) == now.as_ref())
        .unwrap_or(0)
}

/// Sets choice `i` of `def`, saved at once; what to log.
pub fn set(def: &Def, store: &Store, i: usize) -> String {
    let cs = choices(def);
    let Some((v, name)) = cs.get(i) else {
        return format!("{}: no choice {i}", def.key);
    };
    match store.set(def.key, *v) {
        Ok(o) if !o.changed && index(def, store) != i => format!(
            "{}: stays {} (locked by its variable)",
            def.key,
            cs[index(def, store)].1
        ),
        Ok(o) => match o.not_saved {
            None => format!("{} -> {name}", def.key),
            Some(why) => format!("{} -> {name}; not saved: {why}", def.key),
        },
        Err(e) => format!("{}: {e}", def.key),
    }
}

/// One step right (or left) through `def`'s choices, wrapping; what to
/// log.
pub fn step(def: &Def, store: &Store, right: bool) -> String {
    let n = choices(def).len().max(1);
    let i = index(def, store);
    set(def, store, if right { (i + 1) % n } else { (i + n - 1) % n })
}

/// One row of many choices: a setting, its label and what it does.
#[derive(Debug)]
pub struct ChoiceRow {
    pub def: &'static Def,
    /// The row's name, before its value ("Count: 4").
    pub label: &'static str,
    /// The description pane's text while the row has the focus.
    pub description: &'static str,
    /// What the value picker A opens says about the setting: what it does
    /// and what its choices mean, in a few short sentences. The
    /// description when empty.
    pub explain: &'static str,
}

impl ChoiceRow {
    pub const fn new(def: &'static Def, label: &'static str, description: &'static str) -> Self {
        ChoiceRow {
            def,
            label,
            description,
            explain: "",
        }
    }
    /// The value picker's text ([`ChoiceRow::explain`]).
    pub const fn explain(mut self, text: &'static str) -> Self {
        self.explain = text;
        self
    }
    /// What the value picker says: [`ChoiceRow::explain`], else the
    /// description.
    pub fn picker_text(&self) -> &'static str {
        if self.explain.is_empty() {
            self.description
        } else {
            self.explain
        }
    }

    /// The choices in order: each value and how the row shows it
    /// ([`choices`]).
    pub fn choices(&self) -> Vec<(Value, String)> {
        choices(self.def)
    }

    /// The choice `store` holds ([`index`]).
    pub fn index(&self, store: &Store) -> usize {
        index(self.def, store)
    }

    /// Sets choice `i`, saved at once; what to log ([`set`]).
    pub fn set(&self, store: &Store, i: usize) -> String {
        set(self.def, store, i)
    }

    /// One step right (or left), wrapping; what to log ([`step`]).
    pub fn step(&self, store: &Store, right: bool) -> String {
        step(self.def, store, right)
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
        .explain(self.picker_text())
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
