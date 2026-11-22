//! The game variant editor's state and commands.
//!
//! A list of saved and built-in variants on the left, the open variant's
//! settings on the right, grouped into the game's pages, with each
//! setting's help on hover, the changes from the game type's defaults and
//! anything the game would reject. Built-ins are read-only: as in the game,
//! editing one starts from a named copy. Every edit can be undone.
//!
//! The editing operations are plain methods so `h2-editor` (menus, keyboard
//! shortcuts, its variant tool) and tests drive them without a frame.

use std::path::PathBuf;

use crate::ext::{Ext, Mode};
use crate::portable::{self, PortableError};
use crate::record::{GameType, GameVariant, RecordError, SettingId};
use crate::save::{SaveError, SaveStore, SavedVariant};
use crate::schema::{Issue, Schema};

/// Where the open variant came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// Built-in `i` of the schema (read-only).
    Builtin(usize),
    /// A save directory.
    Saved(PathBuf),
    /// Not saved yet (new, a copy of a built-in, or imported).
    New,
}

#[derive(Debug, thiserror::Error)]
pub enum EditError {
    #[error("no variant is open")]
    NoVariant,
    #[error("built-in variants cannot be edited; make a copy")]
    ReadOnly,
    #[error("no save storage is configured")]
    NoStore,
    #[error(transparent)]
    Record(#[from] RecordError),
    #[error(transparent)]
    Save(#[from] SaveError),
    #[error(transparent)]
    Import(#[from] PortableError),
    /// A script option edit the record cannot take.
    #[error("{0}")]
    ScriptOption(String),
}

/// What the last edit changed, so typing a name or dragging one of our
/// options is one undo step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Name,
    Setting(SettingId),
    /// One extension option (its index in [`ext_changed`]).
    Ext(u8),
    /// One of the script's options (its index in the IR): dragging it is
    /// one step, and a different option is another.
    ScriptOption(u8),
}

impl Target {
    /// Runs of edits to the same target merge into one undo step.
    fn merges(self) -> bool {
        matches!(self, Target::Name | Target::Ext(_) | Target::ScriptOption(_))
    }
}

/// Which extension option differs between `a` and `b` (the first one).
fn ext_changed(a: &Ext, b: &Ext) -> u8 {
    let fields = [
        a.gravity != b.gravity,
        a.damage != b.damage,
        a.shield != b.shield,
        a.health != b.health,
        a.speed != b.speed,
        a.unlimited_grenades != b.unlimited_grenades,
        a.primary != b.primary,
        a.secondary != b.secondary,
        a.team_size != b.team_size,
        a.superbounce != b.superbounce,
        a.script != b.script,
    ];
    fields.iter().position(|&d| d).unwrap_or(fields.len()) as u8
}

#[derive(Debug, Clone)]
struct Doc {
    origin: Origin,
    variant: GameVariant,
    /// The variant as last loaded or saved.
    base: GameVariant,
    undo: Vec<GameVariant>,
    redo: Vec<GameVariant>,
    last: Option<Target>,
}

const MAX_UNDO: usize = 256;

pub struct VariantEditor {
    schema: Schema,
    store: Option<SaveStore>,
    builtins: Vec<GameVariant>,
    saved: Vec<SavedVariant>,
    doc: Option<Doc>,
    copy_name: String,
    status: Option<(bool, String)>,
}

impl VariantEditor {
    /// An editor over `schema` (from the disc, or [`Schema::fallback`]) and
    /// the saves in `store`, if any.
    pub fn new(schema: Schema, store: Option<SaveStore>) -> Self {
        let builtins = schema
            .builtins
            .iter()
            .map(|b| {
                let mut v = schema
                    .build(b)
                    .unwrap_or_else(|_| schema.new_variant(b.game_type));
                v.set_builtin_flag(true);
                v
            })
            .collect();
        let mut e = VariantEditor {
            schema,
            store,
            builtins,
            saved: Vec::new(),
            doc: None,
            copy_name: String::new(),
            status: None,
        };
        e.refresh();
        e
    }

    /// Re-reads the saves.
    pub fn refresh(&mut self) {
        if let Some(store) = &self.store {
            match store.list() {
                Ok(l) => self.saved = l,
                Err(err) => self.status = Some((true, format!("cannot list saves: {err}"))),
            }
        }
    }

    pub fn schema(&self) -> &Schema {
        &self.schema
    }

    pub fn builtins(&self) -> &[GameVariant] {
        &self.builtins
    }

    pub fn saved(&self) -> &[SavedVariant] {
        &self.saved
    }

    /// The open variant.
    pub fn current(&self) -> Option<&GameVariant> {
        self.doc.as_ref().map(|d| &d.variant)
    }

    pub fn origin(&self) -> Option<&Origin> {
        self.doc.as_ref().map(|d| &d.origin)
    }

    pub fn is_read_only(&self) -> bool {
        matches!(self.origin(), Some(Origin::Builtin(_)))
    }

    /// The open variant has changes not written to its save (a variant
    /// that was never saved counts as changed).
    pub fn is_dirty(&self) -> bool {
        self.doc
            .as_ref()
            .is_some_and(|d| d.origin == Origin::New || d.variant != d.base)
    }

    /// The last status message, `true` for an error.
    pub fn status(&self) -> Option<(bool, &str)> {
        self.status.as_ref().map(|(e, s)| (*e, s.as_str()))
    }

    /// The open variant differs from its last load or save (what Revert
    /// undoes; a never-saved variant without edits has none).
    pub fn has_changes(&self) -> bool {
        self.doc.as_ref().is_some_and(|d| d.variant != d.base)
    }

    /// The name a copy of the open built-in gets by default.
    pub fn suggested_copy_name(&self) -> &str {
        &self.copy_name
    }

    /// Takes the last status message, `true` for an error (for a host that
    /// logs messages rather than showing the latest).
    pub fn take_status(&mut self) -> Option<(bool, String)> {
        self.status.take()
    }

    /// Closes the open variant, dropping its changes.
    pub fn close(&mut self) {
        self.doc = None;
    }

    fn open(&mut self, origin: Origin, v: GameVariant) {
        self.doc = Some(Doc {
            origin,
            base: v.clone(),
            variant: v,
            undo: Vec::new(),
            redo: Vec::new(),
            last: None,
        });
    }

    pub fn open_builtin(&mut self, i: usize) {
        if let Some(v) = self.builtins.get(i).cloned() {
            self.copy_name = format!("{} copy", strip_icon(&v.name()));
            self.open(Origin::Builtin(i), v);
        }
    }

    pub fn open_saved(&mut self, i: usize) {
        if let Some(s) = self.saved.get(i) {
            let (dir, v, signed) = (s.dir.clone(), s.contents.variant.clone(), s.contents.signed);
            self.open(Origin::Saved(dir), v);
            if !signed {
                self.status = Some((
                    true,
                    "this save's signature does not match the signing key; saving re-signs it"
                        .into(),
                ));
            }
        }
    }

    /// A new, unsaved variant of `ty` with the type's defaults.
    pub fn new_variant(&mut self, ty: GameType) {
        let mut v = self.schema.new_variant(ty);
        let _ = v.set_name(&format!("New {}", ty.label()));
        self.open(Origin::New, v);
    }

    /// A new, unsaved variant of a script: the record
    /// [`portable::record_for`] builds from it and `options` (an option
    /// left out takes its default). Nothing is opened when the script's
    /// record is refused.
    pub fn new_from_script(
        &mut self,
        ir: &rules::Ir,
        options: &[(String, rules::ir::OptionValue)],
    ) -> Result<(), EditError> {
        let mut v = portable::record_for(ir, options)?;
        v.set_builtin_flag(false);
        self.open(Origin::New, v);
        Ok(())
    }

    /// The first 8 bytes of the hash of the script the open variant names
    /// (mode 2 of its script reference), if it names one.
    pub fn script_prefix(&self) -> Option<[u8; 8]> {
        let s = self.current()?.ext().script;
        (s.mode == Mode::Rules).then_some(s.prefix)
    }

    /// Copies the open built-in under `name`, as the game's Edit does
    /// (`0x212b2c`): the built-in flag cleared, the new name set.
    pub fn copy_builtin(&mut self, name: &str) -> Result<(), EditError> {
        let doc = self.doc.as_ref().ok_or(EditError::NoVariant)?;
        let mut v = doc.variant.clone();
        v.set_builtin_flag(false);
        v.set_name(name)?;
        self.open(Origin::New, v);
        Ok(())
    }

    fn edit(
        &mut self,
        target: Target,
        f: impl FnOnce(&mut GameVariant) -> Result<(), RecordError>,
    ) -> Result<(), EditError> {
        let read_only = self.is_read_only();
        let doc = self.doc.as_mut().ok_or(EditError::NoVariant)?;
        if read_only {
            return Err(EditError::ReadOnly);
        }
        let before = doc.variant.clone();
        f(&mut doc.variant)?;
        if doc.variant == before {
            return Ok(());
        }
        if doc.last != Some(target) || !target.merges() {
            doc.undo.push(before);
            if doc.undo.len() > MAX_UNDO {
                doc.undo.remove(0);
            }
        }
        doc.redo.clear();
        doc.last = Some(target);
        Ok(())
    }

    pub fn set_setting(&mut self, id: SettingId, value: i32) -> Result<(), EditError> {
        self.edit(Target::Setting(id), |v| v.set(id, value))
    }

    pub fn set_name(&mut self, name: &str) -> Result<(), EditError> {
        self.edit(Target::Name, |v| v.set_name(name))
    }

    /// Sets our extension options (docs/game/game-options.md section 3).
    /// The game's own rules clear the block.
    pub fn set_ext(&mut self, e: &Ext) -> Result<(), EditError> {
        let now = self.current().map(GameVariant::ext).unwrap_or_default();
        self.edit(Target::Ext(ext_changed(&now, e)), |v| v.set_ext(e))
    }

    /// Sets option `index` of the script `ir` (which the open variant
    /// must name) to `value`, in the option's own terms: its number, a
    /// bool as 0 or 1, an enum's value as `lo` plus its position. It
    /// rewrites the block's option bits and nothing else; the edit is one
    /// undo step, and a run of edits to the same option is one.
    pub fn set_script_option(
        &mut self,
        ir: &rules::Ir,
        index: usize,
        value: i32,
    ) -> Result<(), EditError> {
        let now = self.current().ok_or(EditError::NoVariant)?.ext();
        if now.script.mode != Mode::Rules || now.script.prefix != ir.id().0[..8] {
            return Err(EditError::ScriptOption(
                "the open variant does not name this script".into(),
            ));
        }
        let bits = with_option(ir, now.script.options, index, value)?;
        let mut next = now;
        next.script.options = bits;
        self.edit(Target::ScriptOption(index as u8), |v| v.set_ext(&next))
    }

    pub fn can_undo(&self) -> bool {
        self.doc.as_ref().is_some_and(|d| !d.undo.is_empty())
    }

    pub fn can_redo(&self) -> bool {
        self.doc.as_ref().is_some_and(|d| !d.redo.is_empty())
    }

    pub fn undo(&mut self) -> bool {
        let Some(d) = self.doc.as_mut() else {
            return false;
        };
        let Some(prev) = d.undo.pop() else {
            return false;
        };
        d.redo.push(std::mem::replace(&mut d.variant, prev));
        d.last = None;
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(d) = self.doc.as_mut() else {
            return false;
        };
        let Some(next) = d.redo.pop() else {
            return false;
        };
        d.undo.push(std::mem::replace(&mut d.variant, next));
        d.last = None;
        true
    }

    /// Back to the variant as last loaded or saved (undoable).
    pub fn revert(&mut self) {
        if let Some(d) = self.doc.as_mut()
            && d.variant != d.base
        {
            let base = d.base.clone();
            d.undo.push(std::mem::replace(&mut d.variant, base));
            d.redo.clear();
            d.last = None;
        }
    }

    /// Writes the open variant: a saved one in place (a new save if its
    /// name changed), a new one as a new save. Built-ins cannot be saved.
    pub fn save(&mut self) -> Result<(), EditError> {
        let doc = self.doc.as_ref().ok_or(EditError::NoVariant)?;
        let store = self.store.as_ref().ok_or(EditError::NoStore)?;
        let saved = match &doc.origin {
            Origin::Builtin(_) => return Err(EditError::ReadOnly),
            Origin::New => store.create(&doc.variant)?,
            Origin::Saved(dir) => {
                let old = self
                    .saved
                    .iter()
                    .find(|s| &s.dir == dir)
                    .cloned()
                    .map_or_else(|| store.read(dir), Ok)?;
                store.update(&old, &doc.variant)?
            }
        };
        let doc = self.doc.as_mut().unwrap();
        doc.origin = Origin::Saved(saved.dir.clone());
        doc.base = doc.variant.clone();
        self.status = Some((false, format!("saved as \"{}\"", saved.save_name)));
        self.refresh();
        Ok(())
    }

    /// Deletes the open saved variant.
    pub fn delete(&mut self) -> Result<(), EditError> {
        let store = self.store.as_ref().ok_or(EditError::NoStore)?;
        let Some(Origin::Saved(dir)) = self.origin().cloned() else {
            return Err(EditError::NoVariant);
        };
        let s = store.read(&dir)?;
        store.delete(&s)?;
        self.doc = None;
        self.refresh();
        Ok(())
    }

    /// The open variant as a portable file.
    pub fn export_current(&self) -> Option<String> {
        let mut v = self.current()?.clone();
        v.set_builtin_flag(false);
        Some(portable::export(&v))
    }

    /// Opens a portable file as a new, unsaved variant.
    pub fn import(&mut self, text: &str) -> Result<(), EditError> {
        let (mut v, problems) = portable::import(text)?;
        v.set_builtin_flag(false);
        self.open(Origin::New, v);
        if !problems.is_empty() {
            self.status = Some((
                true,
                format!(
                    "imported; the game would reject {} field(s) until fixed",
                    problems.len()
                ),
            ));
        }
        Ok(())
    }

    /// Findings for the open variant.
    pub fn issues(&self) -> Vec<Issue> {
        self.current()
            .map(|v| self.schema.validate(v))
            .unwrap_or_default()
    }

    /// Opens a list entry (a built-in or a save) by its origin.
    pub fn open_origin(&mut self, p: Origin) {
        match p {
            Origin::Builtin(i) => self.open_builtin(i),
            Origin::Saved(dir) => {
                if let Some(i) = self.saved.iter().position(|s| s.dir == dir) {
                    self.open_saved(i);
                }
            }
            Origin::New => {}
        }
    }

    /// A setting value as its choice label, else the number.
    pub fn value_text(&self, id: SettingId, value: i32) -> String {
        match self.schema.value_label(id, value) {
            Some(l) => l.to_string(),
            None => value.to_string(),
        }
    }
}

/// The option bits `bits` with option `index` of `ir` set to `value`.
pub fn with_option(ir: &rules::Ir, bits: u32, index: usize, value: i32) -> Result<u32, EditError> {
    let bad = |m: String| Err(EditError::ScriptOption(m));
    let Some(o) = ir.options.get(index) else {
        return bad(format!("the script has no option {index}"));
    };
    let name = ir.option_name(index);
    if !(o.lo..=o.hi).contains(&value) {
        return bad(format!("{name} takes {} to {}, not {value}", o.lo, o.hi));
    }
    let width = o.bits();
    if width == 0 {
        return Ok(bits);
    }
    let off = ir.option_offsets()[index];
    if off + width > rules::ir::OPTION_BITS {
        return bad(format!("{name} does not fit in the record's option bits"));
    }
    let mask = ((1u64 << width) - 1) << off;
    let raw = (i64::from(value) - i64::from(o.lo)) as u64;
    Ok(((u64::from(bits) & !mask) | (raw << off)) as u32)
}

/// How a script option is edited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OptionKind {
    Bool,
    Int { lo: i32, hi: i32 },
    /// The value names, from `lo`.
    Enum { lo: i32, values: Vec<String> },
}

/// One option of a script, with its value in a record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptionView {
    /// The option's index in the IR ([`VariantEditor::set_script_option`]).
    pub index: usize,
    pub name: String,
    pub label: String,
    pub description: String,
    pub kind: OptionKind,
    pub value: i32,
    pub default: i32,
    /// The script's `quick` line names it.
    pub quick: bool,
}

impl OptionView {
    /// A value in words: On or Off, an enum's name, or the number.
    pub fn text(&self, value: i32) -> String {
        match &self.kind {
            OptionKind::Bool => (if value != 0 { "On" } else { "Off" }).to_string(),
            OptionKind::Int { .. } => value.to_string(),
            OptionKind::Enum { lo, values } => usize::try_from(i64::from(value) - i64::from(*lo))
                .ok()
                .and_then(|i| values.get(i))
                .cloned()
                .unwrap_or_else(|| value.to_string()),
        }
    }
}

/// One Quick Options mark of a script, in its order, with the value the
/// record has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkView {
    pub label: String,
    /// An option of the script, else a record setting of the table.
    pub option: bool,
    /// The value in the record, in words.
    pub text: String,
}

/// What the variant tool shows of the script a record names: the model
/// of its Script section, built from the script and the record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptView {
    pub name: String,
    pub about: String,
    /// `scoring carrier`: the script only sets settings.
    pub carrier_scoring: bool,
    pub options: Vec<OptionView>,
    /// The Quick Options rows, in the script's order.
    pub marks: Vec<MarkView>,
    /// The record settings the `quick` line names (a Quick mark goes
    /// beside each on its page). A mark on a setting the `hx` block holds
    /// has no page row and shows in [`Self::marks`] only.
    pub marked_settings: Vec<SettingId>,
}

/// The Script section's model for the script `ir` in the record `v`.
pub fn script_view(ir: &rules::Ir, v: &GameVariant) -> ScriptView {
    use rules::Ty;
    use rules::ir::{Quick, Scoring};
    let bits = v.ext().script.options;
    let carrier = ir.header.carrier;
    let string = |i: u16| ir.strings.get(usize::from(i)).cloned().unwrap_or_default();
    let options: Vec<OptionView> = ir
        .options
        .iter()
        .enumerate()
        .map(|(index, o)| {
            let kind = match o.ty {
                Ty::Bool => OptionKind::Bool,
                Ty::Enum(_) => OptionKind::Enum {
                    lo: o.lo,
                    values: ir
                        .option_values(index)
                        .iter()
                        .map(|s| s.to_string())
                        .collect(),
                },
                _ => OptionKind::Int { lo: o.lo, hi: o.hi },
            };
            OptionView {
                index,
                name: ir.option_name(index).to_string(),
                label: string(o.label),
                description: o.description.map(string).unwrap_or_default(),
                kind,
                value: ir
                    .option_value(bits, ir.option_name(index))
                    .unwrap_or(o.default),
                default: o.default,
                quick: ir.quick.contains(&Quick::Option(index as u16)),
            }
        })
        .collect();
    let mut marks = Vec::new();
    let mut marked_settings = Vec::new();
    for q in &ir.quick {
        match *q {
            Quick::Setting(i) => {
                let Some(row) = rules::settings::row_at(i) else {
                    continue;
                };
                let text = portable::setting_value(v, row, carrier)
                    .map_or_else(|| "stock".to_string(), |x| rules::settings::value_label(i, x));
                marks.push(MarkView {
                    label: row.label.to_string(),
                    option: false,
                    text,
                });
                if let Some(id) = row.setting(carrier) {
                    marked_settings.push(SettingId(id));
                }
            }
            Quick::Option(i) => {
                if let Some(o) = options.get(usize::from(i)) {
                    marks.push(MarkView {
                        label: o.label.clone(),
                        option: true,
                        text: o.text(o.value),
                    });
                }
            }
        }
    }
    ScriptView {
        name: ir.header.name.clone(),
        about: ir.header.about.clone(),
        carrier_scoring: ir.header.scoring == Scoring::Carrier,
        options,
        marks,
        marked_settings,
    }
}

/// A built-in's name without the trailing icon glyph (U+E000..U+E0FF) the
/// game's list draws as a padlock or similar.
pub fn strip_icon(name: &str) -> String {
    name.trim_end_matches(|c: char| ('\u{e000}'..='\u{e0ff}').contains(&c))
        .trim_end()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::save::{CONTENT_TYPES, XbeNames};
    use crate::schema::{Builtin, Choice, Page, SettingDef};
    use std::collections::BTreeMap;

    /// A small invented schema: one page with team play and a starting
    /// weapon, Slayer defaults, and one built-in.
    fn schema() -> Schema {
        let mut settings = BTreeMap::new();
        let choice = |value, label: &str| Choice {
            value,
            label: label.into(),
            default: value == 0,
            ours: false,
        };
        settings.insert(
            SettingId(91),
            SettingDef {
                id: SettingId(91),
                title: "Team".into(),
                help: "Teams or not".into(),
                choices: vec![choice(0, "Off"), choice(1, "On")],
            },
        );
        settings.insert(
            SettingId(89),
            SettingDef {
                id: SettingId(89),
                title: "Weapon".into(),
                help: String::new(),
                choices: vec![choice(0, "A"), choice(3, "B"), choice(9, "C")],
            },
        );
        let mut defaults = BTreeMap::new();
        defaults.insert(GameType::Slayer, vec![(SettingId(89), 3)]);
        Schema {
            settings,
            pages: vec![Page {
                id: 12,
                title: "Weapons".into(),
                settings: vec![SettingId(91), SettingId(89)],
            }],
            defaults,
            builtins: vec![Builtin {
                name: "Classic \u{e008}".into(),
                game_type: GameType::Slayer,
                description: 4,
                pairs: vec![(SettingId(91), 1)],
            }],
            source: "test".into(),
        }
    }

    fn store(tag: &str) -> SaveStore {
        let root =
            std::env::temp_dir().join(format!("variants-panel-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        SaveStore::new(
            root,
            Some(XbeNames {
                file_names: (0..CONTENT_TYPES).map(|i| format!("t{i}")).collect(),
                labels: (0..CONTENT_TYPES).map(|i| format!("L{i}")).collect(),
            }),
        )
    }

    #[test]
    fn builtins_are_read_only_and_copy_on_edit() {
        let mut e = VariantEditor::new(schema(), None);
        assert_eq!(e.builtins().len(), 1);
        e.open_builtin(0);
        assert!(e.is_read_only() && !e.is_dirty());
        let v = e.current().unwrap();
        assert!(v.builtin_flag() && v.team_play());
        assert_eq!(v.get(SettingId(89)), Some(3), "the type's defaults first");
        assert_eq!(v.description(), 4);
        assert!(matches!(
            e.set_setting(SettingId(89), 9),
            Err(EditError::ReadOnly)
        ));
        assert!(matches!(
            e.save(),
            Err(EditError::NoStore | EditError::ReadOnly)
        ));

        e.copy_builtin("Mine").unwrap();
        assert_eq!(e.origin(), Some(&Origin::New));
        assert!(e.is_dirty() && !e.is_read_only());
        let v = e.current().unwrap();
        assert!(!v.builtin_flag());
        assert_eq!(v.name(), "Mine");
        assert!(v.team_play(), "settings come along");
    }

    #[test]
    fn undo_and_redo() {
        let mut e = VariantEditor::new(schema(), None);
        e.new_variant(GameType::Slayer);
        let start = e.current().unwrap().clone();
        e.set_setting(SettingId(89), 9).unwrap();
        e.set_setting(SettingId(91), 1).unwrap();
        // Typing a name is one step.
        e.set_name("A").unwrap();
        e.set_name("AB").unwrap();
        e.set_name("ABC").unwrap();
        assert!(e.undo());
        assert_eq!(e.current().unwrap().name(), start.name());
        assert!(e.undo());
        assert!(!e.current().unwrap().team_play());
        assert!(e.redo());
        assert!(e.current().unwrap().team_play());
        assert!(e.undo() && e.undo());
        assert_eq!(e.current().unwrap(), &start);
        assert!(!e.undo() && e.can_redo());
        // A new edit drops the redo history.
        e.set_setting(SettingId(89), 0).unwrap();
        assert!(!e.can_redo());
        // An edit that changes nothing is not a step.
        e.set_setting(SettingId(89), 0).unwrap();
        assert!(e.undo() && !e.can_undo());
    }

    #[test]
    fn extension_options_edit_and_undo_per_option() {
        let mut e = VariantEditor::new(schema(), None);
        e.open_builtin(0);
        assert!(matches!(
            e.set_ext(&Ext {
                gravity: 50,
                ..Ext::default()
            }),
            Err(EditError::ReadOnly)
        ));
        e.new_variant(GameType::Slayer);
        // Dragging gravity is one step; then damage is another.
        for g in [90, 70, 50] {
            e.set_ext(&Ext {
                gravity: g,
                ..Ext::default()
            })
            .unwrap();
        }
        let both = Ext {
            gravity: 50,
            damage: 200,
            ..Ext::default()
        };
        e.set_ext(&both).unwrap();
        assert_eq!(e.current().unwrap().ext(), both);
        assert!(e.undo());
        assert_eq!(e.current().unwrap().ext().gravity, 50);
        assert_eq!(e.current().unwrap().ext().damage, 100);
        assert!(e.undo());
        assert_eq!(e.current().unwrap().ext(), Ext::default());
        assert!(e.current().unwrap().ext_block().iter().all(|&b| b == 0));
        // Out of range is refused and changes nothing.
        assert!(matches!(
            e.set_ext(&Ext {
                gravity: 1,
                ..Ext::default()
            }),
            Err(EditError::Record(RecordError::ExtRange("gravity")))
        ));
        // It travels through export and import.
        e.set_ext(&both).unwrap();
        let text = e.export_current().unwrap();
        e.import(&text).unwrap();
        assert_eq!(e.current().unwrap().ext(), both);
    }

    #[test]
    fn save_writes_and_tracks_dirty() {
        let st = store("save");
        let root = st.root.clone();
        let mut e = VariantEditor::new(schema(), Some(st));
        e.open_builtin(0);
        e.copy_builtin("Mine").unwrap();
        e.save().unwrap();
        assert!(!e.is_dirty());
        assert_eq!(e.saved().len(), 1);
        assert_eq!(e.saved()[0].save_name, "L1: Mine");
        assert!(matches!(e.origin(), Some(Origin::Saved(_))));

        e.set_setting(SettingId(89), 9).unwrap();
        assert!(e.is_dirty());
        e.revert();
        assert!(!e.is_dirty());
        assert!(e.undo() && e.is_dirty());
        e.save().unwrap();
        assert_eq!(e.saved()[0].contents.variant.get(SettingId(89)), Some(9));

        // A value the game rejects is not written.
        e.set_setting(SettingId(19), 3).unwrap();
        assert!(matches!(
            e.save(),
            Err(EditError::Save(SaveError::Rejected(_)))
        ));
        e.undo();

        // Export and import round trip into a new, unsaved variant.
        let text = e.export_current().unwrap();
        e.import(&text).unwrap();
        assert_eq!(e.origin(), Some(&Origin::New));
        assert_eq!(e.current().unwrap().get(SettingId(89)), Some(9));

        e.open_saved(0);
        e.delete().unwrap();
        assert!(e.saved().is_empty() && e.current().is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn diff_and_issues() {
        let mut e = VariantEditor::new(schema(), None);
        e.new_variant(GameType::Slayer);
        assert!(e.issues().is_empty());
        e.set_setting(SettingId(89), 5).unwrap();
        let diff = e.schema().diff_from_defaults(e.current().unwrap());
        assert_eq!(diff, vec![(SettingId(89), 3, 5)]);
        assert_eq!(
            e.issues(),
            vec![Issue::NotOffered {
                id: SettingId(89),
                value: 5
            }]
        );
    }

    fn option_index(ir: &rules::Ir, name: &str) -> usize {
        (0..ir.options.len())
            .find(|&i| ir.option_name(i) == name)
            .unwrap()
    }

    #[test]
    fn new_from_script_is_record_for() {
        use rules::ir::OptionValue;
        let ir = rules::compile(rules::builtin::SUPERBOUNCE).unwrap();
        let mut e = VariantEditor::new(schema(), None);
        e.new_from_script(&ir, &[]).unwrap();
        let want = portable::record_for(&ir, &[]).unwrap();
        assert_eq!(e.current().unwrap(), &want);
        assert_eq!(e.origin(), Some(&Origin::New));
        assert!(e.is_dirty() && !e.is_read_only() && !e.can_undo());
        assert_eq!(e.script_prefix(), Some(ir.id().0[..8].try_into().unwrap()));
        assert_eq!(e.current().unwrap().name(), ir.header.name);

        // With options: the same record the builder makes for them.
        let given = [("bounce_points".to_string(), OptionValue::Int(4))];
        e.new_from_script(&ir, &given).unwrap();
        assert_eq!(
            e.current().unwrap(),
            &portable::record_for(&ir, &given).unwrap()
        );
        let view = script_view(&ir, e.current().unwrap());
        let points = &view.options[option_index(&ir, "bounce_points")];
        assert_eq!(points.value, 4);

        // A refused option opens nothing new: the open variant stays.
        let before = e.current().unwrap().clone();
        let bad = [("bounce_points".to_string(), OptionValue::Int(99))];
        assert!(matches!(
            e.new_from_script(&ir, &bad),
            Err(EditError::Import(_))
        ));
        assert_eq!(e.current().unwrap(), &before);
    }

    #[test]
    fn an_option_edit_is_one_undo_step() {
        let ir = rules::compile(rules::builtin::SUPERBOUNCE).unwrap();
        let points = option_index(&ir, "bounce_points");
        let costs = option_index(&ir, "suicide_costs");
        let mut e = VariantEditor::new(schema(), None);
        e.new_from_script(&ir, &[]).unwrap();
        let start = e.current().unwrap().clone();
        let value = |e: &VariantEditor, name: &str| {
            ir.option_value(e.current().unwrap().ext().script.options, name)
        };
        assert_eq!(value(&e, "bounce_points"), Some(1));
        assert_eq!(value(&e, "suicide_costs"), Some(1));

        // Dragging one option is one step.
        for n in [2, 3, 4] {
            e.set_script_option(&ir, points, n).unwrap();
        }
        assert_eq!(value(&e, "bounce_points"), Some(4));
        // Another option is another step, and leaves the first alone.
        e.set_script_option(&ir, costs, 0).unwrap();
        assert_eq!(value(&e, "suicide_costs"), Some(0));
        assert_eq!(value(&e, "bounce_points"), Some(4));
        // An edit that changes nothing is no step.
        e.set_script_option(&ir, costs, 0).unwrap();

        assert!(e.undo());
        assert_eq!(value(&e, "suicide_costs"), Some(1));
        assert_eq!(value(&e, "bounce_points"), Some(4));
        assert!(e.undo());
        assert_eq!(e.current().unwrap(), &start);
        assert!(!e.can_undo());
        assert!(e.redo() && e.redo());
        assert_eq!(value(&e, "suicide_costs"), Some(0));

        // Only the option bits changed: the rest of the record is the
        // start's, the script reference included.
        let mut want = e.current().unwrap().ext();
        want.script.options = start.ext().script.options;
        assert_eq!(want, start.ext());

        // Out of range, an unknown option and a script the record does not
        // name change nothing.
        let now = e.current().unwrap().clone();
        assert!(matches!(
            e.set_script_option(&ir, points, 99),
            Err(EditError::ScriptOption(_))
        ));
        assert!(matches!(
            e.set_script_option(&ir, 99, 0),
            Err(EditError::ScriptOption(_))
        ));
        let other = rules::compile(rules::builtin::SWAT).unwrap();
        assert!(matches!(
            e.set_script_option(&other, 0, 0),
            Err(EditError::ScriptOption(_))
        ));
        assert_eq!(e.current().unwrap(), &now);

        // A built-in is read-only.
        e.open_builtin(0);
        assert!(e.script_prefix().is_none());
        assert!(matches!(
            e.set_script_option(&ir, points, 2),
            Err(EditError::ScriptOption(_) | EditError::ReadOnly)
        ));
    }

    #[test]
    fn the_script_view_lists_options_and_marks_in_order() {
        let ir = rules::compile(rules::builtin::SUPERBOUNCE).unwrap();
        let v = portable::record_for(&ir, &[]).unwrap();
        let view = script_view(&ir, &v);
        assert_eq!(view.name, "Superbounce");
        assert!(!view.carrier_scoring);
        assert_eq!(view.options.len(), ir.options.len());
        // The quick line is `respawn_time, starting_weapon, shields,
        // bounce_points, suicide_costs`.
        let labels: Vec<&str> = view.marks.iter().map(|m| m.label.as_str()).collect();
        assert_eq!(labels.len(), 5);
        assert_eq!(&labels[3..], ["Bounce points", "Suicide penalty"]);
        assert!(view.marks[3].option && !view.marks[0].option);
        assert_eq!(view.marks[0].text, "3 seconds");
        assert_eq!(view.marks[3].text, "1");
        assert_eq!(view.marks[4].text, "On");
        assert!(view.options.iter().filter(|o| o.quick).count() == 2);
        // Marked settings that have a record row get a mark beside them.
        assert!(!view.marked_settings.is_empty());
    }

    #[test]
    fn icons_are_stripped_from_names() {
        assert_eq!(strip_icon("Slayer \u{e008}"), "Slayer");
        assert_eq!(strip_icon("Plain"), "Plain");
    }
}
