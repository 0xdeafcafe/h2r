//! Many placements selected at once, and the edits made to them together:
//! a field set on every selected entry that has it, delete, duplicate, a
//! gizmo drag moving them all, and adding where the cursor meets the
//! level. Each is one undo step.
//!
//! The primary selection ([`Session::selected`]) is the one the gizmo and
//! the inspector's title follow; the others ride along. Selecting one
//! entry ([`Session::select`]) drops the others.
use super::*;
use crate::edit::{Op, SetField};
use crate::layout::FieldDef;

/// An edit to one field of every selected entry.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FieldEdit {
    /// The whole value.
    Set(Value),
    /// One component of a vector field (the others stay as each entry has
    /// them), e.g. only x of a position.
    Component(usize, f32),
    /// One bit of a flags field set or cleared (the other bits stay as
    /// each entry has them).
    Bit(u32, bool),
}

impl FieldEdit {
    /// The value an entry holding `old` gets.
    pub fn apply(self, old: Value) -> Option<Value> {
        match (self, old) {
            (FieldEdit::Set(v), _) => Some(v),
            (FieldEdit::Component(k, x), Value::Vec3(mut v)) if k < 3 => {
                v[k] = x;
                Some(Value::Vec3(v))
            }
            (FieldEdit::Bit(bit, on), Value::Int(x)) if bit < 32 => {
                let mask = 1i64 << bit;
                Some(Value::Int(if on { x | mask } else { x & !mask }))
            }
            _ => None,
        }
    }
}

impl Session {
    /// Everything selected, the primary first. Entries an undo took away
    /// are left out.
    pub fn selection(&self) -> Vec<(BlockKind, usize)> {
        let Some(primary) = self.selected else {
            return Vec::new();
        };
        let valid = |&(k, i): &(BlockKind, usize)| {
            self.scenario()
                .map_or(self.reference.is_some(), |s| i < s.block(k).len())
        };
        let mut out = vec![primary];
        out.extend(
            self.extra
                .iter()
                .copied()
                .filter(|e| *e != primary && valid(e)),
        );
        out
    }

    pub fn is_selected(&self, entry: (BlockKind, usize)) -> bool {
        self.selected == Some(entry) || (self.selected.is_some() && self.extra.contains(&entry))
    }

    /// Add an entry to the selection, as the primary, or take it out
    /// (Ctrl/Cmd + click).
    pub fn toggle_select(&mut self, entry: (BlockKind, usize)) {
        let mut all = self.selection();
        if let Some(at) = all.iter().position(|e| *e == entry) {
            all.remove(at);
        } else {
            all.insert(0, entry);
        }
        self.select_many(all);
    }

    /// Select these entries, the first as the primary.
    pub fn select_many(&mut self, entries: Vec<(BlockKind, usize)>) {
        let mut entries = entries.into_iter();
        let first = entries.next();
        self.select(first);
        let mut rest: Vec<_> = Vec::new();
        for e in entries {
            if Some(e) != first && !rest.contains(&e) {
                rest.push(e);
            }
        }
        self.extra = rest;
        self.markers_rev += 1;
    }

    /// The fields every selected entry has (by name and type), in the
    /// primary's order: what the inspector edits together.
    pub fn shared_fields(&self) -> Vec<&'static FieldDef> {
        let all = self.selection();
        let Some(&(first, _)) = all.first() else {
            return Vec::new();
        };
        crate::layout::block(first)
            .fields
            .iter()
            .filter(|f| {
                all.iter().all(|&(k, _)| {
                    crate::layout::block(k)
                        .field(f.name)
                        .is_some_and(|g| g.ty == f.ty && g.role == f.role)
                })
            })
            .collect()
    }

    /// Each selected entry's value of `field` (entries without it left
    /// out).
    pub fn selection_values(&self, field: &str) -> Vec<Value> {
        let Some(s) = self.scenario() else {
            return Vec::new();
        };
        self.selection()
            .into_iter()
            .filter_map(|(k, i)| s.get(k, i, field).ok())
            .collect()
    }

    /// Set `field` on every selected entry that has it, as one undo step;
    /// edits with the same `merge` key in a row (one drag in the
    /// inspector) stay one step. Returns how many entries changed.
    pub fn edit_selection(&mut self, field: &str, edit: FieldEdit, merge: u64) -> Result<usize> {
        let s = self.scenario().ok_or(Error::Save("no map is open"))?;
        let mut ops = Vec::new();
        for (kind, index) in self.selection() {
            let Some(def) = crate::layout::block(kind).field(field) else {
                continue;
            };
            if !def.editable {
                continue;
            }
            let Ok(old) = s.get(kind, index, def.name) else {
                continue;
            };
            let Some(value) = edit.apply(old) else {
                continue;
            };
            if value != old {
                ops.push(Op::Set(SetField {
                    block: kind,
                    index,
                    field: def.name,
                    value,
                }));
            }
        }
        let n = ops.len();
        if n == 0 {
            return Ok(0);
        }
        self.apply(Command {
            label: if n == 1 {
                format!("set {field}")
            } else {
                format!("set {field} on {n} placements")
            },
            ops,
            merge: Some(merge),
        })?;
        Ok(n)
    }

    /// Why the selection can't be deleted, if it can't.
    pub fn remove_selection_refusal(&self) -> Option<&'static str> {
        let all = self.selection();
        if all.is_empty() {
            return Some("select a placement");
        }
        all.iter().find_map(|&(k, _)| self.remove_refusal(k))
    }

    /// Delete everything selected, as one undo step. Each block's entries
    /// go highest index first, so the swap-removes never move one still
    /// to go. Returns how many were removed.
    pub fn remove_selection(&mut self) -> Result<usize> {
        if let Some(why) = self.remove_selection_refusal() {
            return Err(Error::Refused(why));
        }
        let mut all = self.selection();
        all.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
        let n = all.len();
        let command = Command {
            label: if n == 1 {
                format!("remove {:?} {}", all[0].0, all[0].1)
            } else {
                format!("remove {n} placements")
            },
            ops: all
                .into_iter()
                .map(|(block, index)| Op::Remove { block, index })
                .collect(),
            merge: None,
        };
        self.apply(command)?;
        self.select(None);
        Ok(n)
    }

    /// Why the selection can't be duplicated, if it can't: a block it
    /// would fill past its room ([`GROWTH_REFUSAL`] while growth is off)
    /// or the engine's limit.
    pub fn duplicate_selection_refusal(&self) -> Option<&'static str> {
        let s = self.scenario()?;
        let all = self.selection();
        if all.is_empty() {
            return Some("select a placement");
        }
        for &(kind, _) in &all {
            if let Some(why) = s.add_refusal(kind) {
                return Some(why);
            }
            let b = s.block(kind);
            let adding = all.iter().filter(|e| e.0 == kind).count();
            let len = b.len() + adding;
            if !self.allow_growth && len > b.capacity() {
                return Some(GROWTH_REFUSAL);
            }
            if crate::layout::block(kind)
                .max_count
                .is_some_and(|m| len > m)
            {
                return Some("the block is at the engine's entry limit");
            }
        }
        None
    }

    /// Copy everything selected in place, as one undo step, and select the
    /// copies (the primary's copy as the primary).
    pub fn duplicate_selection(&mut self) -> Result<Vec<(BlockKind, usize)>> {
        if let Some(why) = self.duplicate_selection_refusal() {
            return Err(Error::Refused(why));
        }
        let s = self.scenario().ok_or(Error::Save("no map is open"))?;
        let all = self.selection();
        let mut next: Vec<(BlockKind, usize)> = Vec::new();
        let mut ops = Vec::new();
        let mut copies = Vec::new();
        for &(kind, i) in &all {
            let entry = s.new_entry(kind, Some(i), None)?;
            let n = next.iter().filter(|e| e.0 == kind).count();
            next.push((kind, i));
            copies.push((kind, s.block(kind).len() + n));
            ops.push(Op::Add { block: kind, entry });
        }
        let n = ops.len();
        self.apply(Command {
            label: if n == 1 {
                format!("duplicate {:?} {}", all[0].0, all[0].1)
            } else {
                format!("duplicate {n} placements")
            },
            ops,
            merge: None,
        })?;
        self.select_many(copies.clone());
        Ok(copies)
    }

    /// Where `ray` first meets the level's solid surfaces.
    pub fn surface_hit(&self, ray: &Ray) -> Option<Vec3> {
        geometry::surface(self.world().0, ray)
    }

    /// Add an entry at `at` (where the cursor met the level), as one undo
    /// step: a copy of `like`, or a new entry with the palette's first
    /// item. Selects and returns its index.
    pub fn add_entry_at(
        &mut self,
        kind: BlockKind,
        like: Option<usize>,
        at: Vec3,
    ) -> Result<usize> {
        if let Some(why) = self.add_refusal(kind) {
            return Err(Error::Refused(why));
        }
        let s = self.scenario().ok_or(Error::Save("no map is open"))?;
        let entry = s.new_entry(kind, like, Some(at))?;
        let index = s.block(kind).len();
        let mut command = Command::add(kind, entry);
        command.label = format!("place {kind:?}");
        self.apply(command)?;
        self.select(Some((kind, index)));
        Ok(index)
    }

    /// The other selected entries' share of a gizmo move of the primary:
    /// the same offset, and the same turn added to their own rotation.
    pub(super) fn ride_along(&self, moved: Vec3, turned: Vec3) -> Vec<Op> {
        let Some(s) = self.scenario() else {
            return Vec::new();
        };
        let mut ops = Vec::new();
        let finite = |v: &[f32]| v.iter().all(|x| x.is_finite());
        for (kind, index) in self.selection().into_iter().skip(1) {
            let def = crate::layout::block(kind);
            if moved != [0.0; 3]
                && finite(&moved)
                && let Some(f) = def.role(Role::Position)
                && let Ok(Value::Vec3(p)) = s.get(kind, index, f.name)
            {
                ops.push(set_op(
                    kind,
                    index,
                    f.name,
                    Value::Vec3(geometry::add(p, moved)),
                ));
            }
            if turned != [0.0; 3] && finite(&turned) {
                if let Some(f) = def.role(Role::Rotation)
                    && let Ok(Value::Vec3(r)) = s.get(kind, index, f.name)
                {
                    ops.push(set_op(
                        kind,
                        index,
                        f.name,
                        Value::Vec3(geometry::add(r, turned)),
                    ));
                } else if let Some(f) = def.role(Role::Facing)
                    && let Ok(Value::Float(y)) = s.get(kind, index, f.name)
                {
                    ops.push(set_op(kind, index, f.name, Value::Float(y + turned[0])));
                }
            }
        }
        ops
    }

    /// The primary's scale (a block with a scale field; 0 reads as 1).
    pub fn selected_scale(&self) -> Option<f32> {
        let (kind, index) = self.selected?;
        let f = crate::layout::block(kind).role(Role::Scale)?;
        let s = self.scenario()?.get(kind, index, f.name).ok()?.as_f32()?;
        Some(if s == 0.0 { 1.0 } else { s })
    }

    /// Scale the primary to `scale` within the current gizmo gesture, and
    /// every other selected entry with a scale by the same factor.
    pub fn gesture_scale(&mut self, scale: f32) -> Result<()> {
        if !scale.is_finite() || scale <= 0.0 {
            return Ok(());
        }
        let Some(before) = self.selected_scale() else {
            return Ok(());
        };
        if (before - scale).abs() <= 1e-5 {
            return Ok(());
        }
        self.begin_gesture();
        let key = self.gesture.expect("just begun");
        let factor = scale / before;
        let s = self.scenario().ok_or(Error::Save("no map is open"))?;
        let mut ops = Vec::new();
        for (kind, index) in self.selection() {
            let Some(f) = crate::layout::block(kind).role(Role::Scale) else {
                continue;
            };
            let Some(old) = s.get(kind, index, f.name).ok().and_then(Value::as_f32) else {
                continue;
            };
            let old = if old == 0.0 { 1.0 } else { old };
            ops.push(set_op(kind, index, f.name, Value::Float(old * factor)));
        }
        self.apply(Command {
            label: "scale".into(),
            ops,
            merge: Some(key),
        })
    }

    /// A sphere round everything selected: the placements' models (their
    /// bounds placed and scaled) or, without one, their glyphs.
    pub fn selection_sphere(&self) -> Option<(Vec3, f32)> {
        let all = self.selection();
        let mut bounds = Bounds::EMPTY;
        for m in self
            .markers
            .iter()
            .filter(|m| all.contains(&(m.block, m.index)))
        {
            let model = self
                .scene()
                .and_then(|s| s.model_of(m).and_then(|i| s.models.get(i)))
                .filter(|model| !model.bounds.is_empty());
            let (c, r) = match model {
                Some(model) => (
                    crate::scene::model_to_world(m, model.bounds.center()),
                    model.bounds.radius() * m.scale,
                ),
                None => (m.position, 0.5),
            };
            bounds.add(geometry::sub(c, [r; 3]));
            bounds.add(geometry::add(c, [r; 3]));
        }
        (!bounds.is_empty()).then(|| (bounds.center(), bounds.radius()))
    }
}

#[cfg(test)]
mod tests;
