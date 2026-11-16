//! What the hopper tool's variant picker offers, without any UI: the
//! disc's built-in variants (built the way the game builds them), the
//! user's saved variants, and `.h2variant` files loaded this session. Each
//! choice carries what the game's clean-up would say about it; the picker
//! only links choices with nothing to say ([`model::set_variant`]
//! refuses the rest).
//!
//! [`model::set_variant`]: super::model::set_variant

use std::path::{Path, PathBuf};

use variants::record::{self, Problem};
use variants::save::SaveStore;
use variants::{GameVariant, Schema, portable};

/// Where a choice comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// One of the disc's built-in variants.
    Builtin,
    /// A saved variant (its save directory).
    Saved(PathBuf),
    /// A `.h2variant` file.
    File(PathBuf),
}

/// One variant the picker offers.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    pub origin: Origin,
    pub variant: GameVariant,
    /// What the game's clean-up would reject (empty: usable).
    pub problems: Vec<Problem>,
}

impl Choice {
    pub fn new(origin: Origin, variant: GameVariant) -> Self {
        let problems = record::check(&variant);
        Choice {
            origin,
            variant,
            problems,
        }
    }

    /// The picker may link it.
    pub fn usable(&self) -> bool {
        self.problems.is_empty()
    }

    /// `<name> - <game type>`, the name without the built-ins' trailing
    /// icon glyphs.
    pub fn label(&self) -> String {
        variant_label(&self.variant)
    }
}

/// `<name> - <game type>[ - teams]` for a variant record.
pub fn variant_label(v: &GameVariant) -> String {
    let name = strip_icon(&v.name());
    let name = if name.is_empty() { "(unnamed)" } else { &name };
    let teams = if v.team_play() { " - teams" } else { "" };
    format!("{name} - {}{teams}", v.game_type().label())
}

/// The built-ins' names end in private-use glyphs (the game's icons).
fn strip_icon(name: &str) -> String {
    name.trim_end_matches(|c: char| ('\u{e000}'..='\u{e0ff}').contains(&c))
        .trim_end()
        .to_string()
}

/// Everything the picker offers.
#[derive(Debug, Clone, Default)]
pub struct Choices {
    pub builtins: Vec<Choice>,
    pub saved: Vec<Choice>,
    pub files: Vec<Choice>,
    /// What could not be listed (a built-in the schema cannot build, an
    /// unreadable save directory).
    pub notes: Vec<String>,
}

impl Choices {
    /// The built-ins of `schema` and the saves in `store`.
    pub fn new(schema: &Schema, store: Option<&SaveStore>) -> Self {
        let mut c = Choices::default();
        for b in &schema.builtins {
            match schema.build(b) {
                Ok(mut v) => {
                    // As the variant editor lists them, and as the game's
                    // content list carries them: the built-in flag set.
                    v.set_builtin_flag(true);
                    c.builtins.push(Choice::new(Origin::Builtin, v));
                }
                Err(e) => c
                    .notes
                    .push(format!("built-in {:?} cannot be built: {e}", b.name)),
            }
        }
        if let Some(store) = store {
            c.refresh_saved(store);
        }
        c
    }

    /// Re-reads the saves.
    pub fn refresh_saved(&mut self, store: &SaveStore) {
        self.notes.retain(|n| !n.starts_with("saved variants: "));
        match store.list() {
            Ok(list) => {
                self.saved = list
                    .into_iter()
                    .map(|s| Choice::new(Origin::Saved(s.dir), s.contents.variant))
                    .collect();
            }
            Err(e) => {
                self.saved.clear();
                self.notes.push(format!("saved variants: {e}"));
            }
        }
    }

    /// Reads a `.h2variant` file and adds it to [`Choices::files`]
    /// (replacing an earlier load of the same path); returns it.
    pub fn load_file(&mut self, path: &Path) -> Result<&Choice, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let (v, _) = portable::import(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let choice = Choice::new(Origin::File(path.to_owned()), v);
        let at = match self.files.iter().position(|c| c.origin == choice.origin) {
            Some(i) => {
                self.files[i] = choice;
                i
            }
            None => {
                self.files.push(choice);
                self.files.len() - 1
            }
        };
        Ok(&self.files[at])
    }

    /// Every choice: built-ins, then saves, then files.
    pub fn all(&self) -> impl Iterator<Item = &Choice> {
        self.builtins.iter().chain(&self.saved).chain(&self.files)
    }
}

/// The invented schema the hopper tests use.
#[cfg(test)]
pub(crate) fn tests_schema() -> Schema {
    tests::schema()
}

#[cfg(test)]
mod tests {
    use super::*;
    use variants::GameType;
    use variants::schema::Builtin;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("h2-editor-choices-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The fallback schema with two invented built-ins.
    pub(crate) fn schema() -> Schema {
        let mut s = Schema::fallback();
        s.builtins = vec![
            Builtin {
                name: "Alpha\u{e001}".into(),
                game_type: GameType::Slayer,
                description: 3,
                pairs: Vec::new(),
            },
            Builtin {
                name: "Bravo".into(),
                game_type: GameType::King,
                description: 4,
                pairs: Vec::new(),
            },
        ];
        s
    }

    #[test]
    fn builtins_are_built_the_games_way_and_flagged() {
        let s = schema();
        let c = Choices::new(&s, None);
        assert_eq!(c.builtins.len(), 2);
        let a = &c.builtins[0];
        let mut want = s.build(&s.builtins[0]).unwrap();
        want.set_builtin_flag(true);
        assert_eq!(a.variant, want);
        assert_eq!(a.variant.description(), 3);
        assert_eq!(a.label(), "Alpha - Slayer");
        assert_eq!(c.builtins[1].variant.game_type(), GameType::King);
        assert!(c.all().all(Choice::usable), "{:?}", c.notes);
        assert!(c.saved.is_empty() && c.notes.is_empty());
    }

    #[test]
    fn saves_are_listed_from_the_store() {
        let root = scratch("store");
        let store = SaveStore::new(&root, None);
        let mut v = GameVariant::blank(GameType::Oddball);
        v.set_name("My ball").unwrap();
        // A store without the XBE's names cannot create; write the save
        // the way the store reads it instead.
        let dir = root.join(variants::save::directory_name("Oddball: My ball"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(variants::save::SAVE_META),
            variants::save::encode_meta("Oddball: My ball"),
        )
        .unwrap();
        std::fs::write(
            dir.join("payload"),
            variants::save::encode_file(&v, &store.key),
        )
        .unwrap();
        let mut c = Choices::new(&Schema::fallback(), Some(&store));
        assert_eq!(c.saved.len(), 1, "{:?}", c.notes);
        assert_eq!(c.saved[0].variant, v);
        assert_eq!(c.saved[0].origin, Origin::Saved(dir.clone()));
        std::fs::remove_dir_all(&dir).unwrap();
        c.refresh_saved(&store);
        assert!(c.saved.is_empty());
    }

    #[test]
    fn portable_files_load_and_report_problems() {
        let dir = scratch("files");
        let mut v = GameVariant::blank(GameType::King);
        v.set_name("Hill").unwrap();
        let good = dir.join("hill.h2variant");
        std::fs::write(&good, portable::export(&v)).unwrap();
        let mut c = Choices::default();
        let got = c.load_file(&good).unwrap();
        assert_eq!(got.variant, v);
        assert!(got.usable());
        // Loading the same path again replaces it.
        c.load_file(&good).unwrap();
        assert_eq!(c.files.len(), 1);

        let mut bytes = v.to_bytes();
        bytes[0x44] = 5; // engine 5: the clean-up resets it
        let bad_v = GameVariant::from_bytes(&bytes).unwrap();
        let bad = dir.join("bad.h2variant");
        std::fs::write(&bad, portable::export(&bad_v)).unwrap();
        let got = c.load_file(&bad).unwrap();
        assert!(!got.usable());
        assert_eq!(c.files.len(), 2);

        let junk = dir.join("junk.h2variant");
        std::fs::write(&junk, "{}").unwrap();
        assert!(
            c.load_file(&junk)
                .unwrap_err()
                .contains("not a game variant")
        );
        assert!(c.load_file(&dir.join("missing")).is_err());
        assert_eq!(c.all().count(), 2);
    }
}
