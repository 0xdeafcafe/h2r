//! Problems shared by the link pill, Pending, the inspector and the game plate.

use std::cell::{OnceCell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ops::{Deref, DerefMut};
use std::sync::{Arc, LazyLock};

use map::TagId;
use scenario_edit::BlockKind;
use serde_json::{Value, json};

use super::{LinkView, sync, tag_sync};
use crate::app::{App, tag_api};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Waiting,
    Reload,
    Mismatch,
    Refused,
}

impl Kind {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Waiting => "waiting",
            Self::Reload => "reload",
            Self::Mismatch => "mismatch",
            Self::Refused => "refused",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Issue {
    pub kind: Kind,
    pub text: String,
    pub target: Value,
    /// The address used by Take and Overwrite for a conflicting field.
    pub at: Option<u32>,
    placement: Option<(BlockKind, usize)>,
    field: Option<String>,
}

impl Issue {
    pub(super) fn refused_move(why: &str) -> Self {
        Self {
            kind: Kind::Refused,
            text: why.to_owned(),
            target: json!({"panel": "pending"}),
            at: None,
            placement: None,
            field: None,
        }
    }

    pub fn wire(&self) -> Value {
        json!({"kind": self.kind.name(), "text": self.text, "target": self.target})
    }
}

fn target(app: &App, row: &Value) -> (Value, Option<(BlockKind, usize)>) {
    let target = &row["target"];
    if row["resolvable"] == false {
        return (json!({"panel": "pending"}), None);
    }
    if target["block"].is_string()
        && let Ok(entry) = crate::api::P(target).entry(app)
    {
        return (target.clone(), Some(entry));
    }
    if target["panel"] == "pending" || row["doc"] == "scenario" || row["doc"] == "object" {
        // A block reload or a deleted slot has no selectable entry.
        return (json!({"panel": "pending"}), None);
    }
    let Some(tag) = target.get("tag").or_else(|| row.get("tag")).filter(|tag| !tag.is_null()) else {
        return (json!({"panel": "pending"}), None);
    };
    let Ok(tag) = tag_api::lookup(&app.tags, tag) else {
        return (json!({"panel": "pending"}), None);
    };
    let mut result = json!({"tag": tag.0});
    if let Some(path) = target["path"].as_str() {
        if !tag_api::field_focus(&app.tags, tag, path)
            .is_ok_and(|focus| row["at"] == focus.address)
        {
            return (json!({"panel": "pending"}), None);
        }
        result["path"] = json!(path);
    }
    (result, None)
}

fn for_pending(app: &App) -> Vec<Issue> {
    app.pairing.pending.iter().map(|row| {
        let kind = if row["conflict"] == true {
            Kind::Mismatch
        } else if row["refused"] == true {
            Kind::Refused
        } else if tag_sync::Effect::is_reload(row) {
            Kind::Reload
        } else {
            Kind::Waiting
        };
        let (target, placement) = target(app, row);
        Issue {
            kind,
            text: row["why"].as_str().unwrap_or("edit waiting for the game").to_owned(),
            target,
            placement,
            at: row["at"].as_u64().and_then(|n| u32::try_from(n).ok()),
            field: row["field"].as_str().map(str::to_owned),
        }
    }).collect()
}

/// All consumers share this projection until either the rows or their navigation targets change.
#[derive(Debug, Default)]
pub struct Pending {
    rows: Vec<Value>,
    cache: RefCell<Option<Cached>>,
    changes: OnceCell<super::playtest::Changes>,
    class_order: OnceCell<Vec<usize>>,
}

#[derive(Debug)]
struct Cached {
    revisions: (u64, u64, u64),
    refused_move: Option<String>,
    index: Arc<IssueIndex>,
}

impl Deref for Pending {
    type Target = Vec<Value>;

    fn deref(&self) -> &Self::Target {
        &self.rows
    }
}

impl DerefMut for Pending {
    fn deref_mut(&mut self) -> &mut Self::Target {
        *self.cache.get_mut() = None;
        let _ = self.changes.take();
        let _ = self.class_order.take();
        &mut self.rows
    }
}

impl Pending {
    pub(super) fn playtest_changes(&self) -> super::playtest::Changes {
        *self.changes.get_or_init(|| super::playtest::Changes::of(&self.rows))
    }

    /// The Pending panel groups by effect (in `tag_sync::Effect`'s order,
    /// rows with none last) and preserves insertion order within each group.
    pub(crate) fn sorted(&self) -> impl Iterator<Item = &Value> {
        let order = self.class_order.get_or_init(|| {
            let mut order: Vec<_> = (0..self.rows.len()).collect();
            order.sort_by(|&a, &b| {
                let key = |n: usize| {
                    tag_sync::Effect::of_row(&self.rows[n]).map_or(usize::MAX, |e| e as usize)
                };
                key(a).cmp(&key(b))
            });
            order
        });
        order.iter().map(|&n| &self.rows[n])
    }

    pub(super) fn issues(&self, app: &App, refused_move: Option<&str>) -> Arc<IssueIndex> {
        let revisions = (
            app.session.world_rev(),
            app.session.markers_rev(),
            app.tags.rev(),
        );
        let mut cache = self.cache.borrow_mut();
        if let Some(saved) = cache.as_ref()
            && revisions == saved.revisions
            && refused_move == saved.refused_move.as_deref()
        {
            return Arc::clone(&saved.index);
        }
        let mut rows = for_pending(app);
        if let Some(why) = refused_move {
            rows.push(Issue::refused_move(why));
        }
        let index = Arc::new(IssueIndex::new(rows));
        *cache = Some(Cached {
            revisions,
            refused_move: refused_move.map(str::to_owned),
            index: Arc::clone(&index),
        });
        index
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct IssueIndex {
    rows: Vec<Issue>,
    summary: Option<String>,
    placement_rows: HashMap<(BlockKind, usize), usize>,
    placement_fields: HashMap<(BlockKind, usize), HashMap<String, Vec<usize>>>,
    tag_rows: HashMap<TagId, usize>,
    tag_fields: HashMap<(TagId, u32), Vec<usize>>,
    waiting_tags: HashSet<TagId>,
}

impl Deref for IssueIndex {
    type Target = [Issue];

    fn deref(&self) -> &[Issue] {
        &self.rows
    }
}

impl IssueIndex {
    pub(crate) fn empty() -> &'static Self {
        static EMPTY: LazyLock<IssueIndex> = LazyLock::new(IssueIndex::default);
        &EMPTY
    }

    fn new(rows: Vec<Issue>) -> Self {
        let mut index = Self {
            rows,
            ..Self::default()
        };
        for (n, issue) in index.rows.iter().enumerate() {
            let tag = issue.target["tag"]
                .as_u64()
                .and_then(|id| u32::try_from(id).ok())
                .map(TagId);
            if matches!(issue.kind, Kind::Waiting | Kind::Reload) {
                if let Some(id) = tag {
                    index.waiting_tags.insert(id);
                }
                continue;
            }
            let row_mark = if let Some(entry) = issue.placement {
                if let Some(field) = &issue.field {
                    index.placement_fields
                        .entry(entry)
                        .or_default()
                        .entry(field.clone())
                        .or_default()
                        .push(n);
                }
                index.placement_rows.entry(entry).or_insert(n)
            } else if let Some(id) = tag {
                if let Some(at) = issue.at {
                    index.tag_fields.entry((id, at)).or_default().push(n);
                }
                index.tag_rows.entry(id).or_insert(n)
            } else {
                continue;
            };
            if issue.kind == Kind::Refused || index.rows[*row_mark].kind != Kind::Refused {
                *row_mark = n;
            }
        }
        index.summary = index.summarize();
        index
    }

    pub(crate) fn placement_row(&self, block: BlockKind, index: usize) -> Option<&Issue> {
        self.placement_rows.get(&(block, index)).map(|&n| &self.rows[n])
    }

    pub(crate) fn placement_field(
        &self,
        block: BlockKind,
        index: usize,
        field: &str,
    ) -> impl Iterator<Item = &Issue> {
        self.placement_fields
            .get(&(block, index))
            .and_then(|fields| fields.get(field))
            .into_iter()
            .flatten()
            .map(|&n| &self.rows[n])
    }

    pub(crate) fn tag_row(&self, tag: TagId) -> Option<&Issue> {
        self.tag_rows.get(&tag).map(|&n| &self.rows[n])
    }

    pub(crate) fn tag_field(&self, tag: TagId, at: u32) -> impl Iterator<Item = &Issue> {
        self.tag_fields
            .get(&(tag, at))
            .into_iter()
            .flatten()
            .map(|&n| &self.rows[n])
    }

    pub(crate) fn tag_waiting(&self, tag: TagId) -> bool {
        self.waiting_tags.contains(&tag)
    }
}

impl LinkView {
    pub fn issue_summary(&self) -> Option<String> {
        self.issues.summary.clone()
    }
}

impl IssueIndex {
    /// The largest issue count. A tie gives the actionable problem first.
    fn summarize(&self) -> Option<String> {
        let mut largest = 0;
        let mut summary = None;
        for kind in [Kind::Refused, Kind::Mismatch, Kind::Reload, Kind::Waiting] {
            let n = self.rows.iter().filter(|issue| issue.kind == kind).count();
            if n > largest {
                largest = n;
                let label = match (kind, n) {
                    (Kind::Refused, 1) => "write refused",
                    (Kind::Refused, _) => "writes refused",
                    (Kind::Mismatch, 1) => "field changed in game",
                    (Kind::Mismatch, _) => "fields changed in game",
                    (Kind::Reload, 1) => "needs a reload",
                    (Kind::Reload, _) => "need a reload",
                    (Kind::Waiting, 1) => "edit waiting",
                    (Kind::Waiting, _) => "edits waiting",
                };
                summary = Some(format!("{n} {label}"));
            }
        }
        summary
    }
}

/// Without a field identity the host has rejected this combination of writes.
/// Changing or undoing any member releases the whole group for another batch.
#[derive(Default)]
pub(super) struct Refusals {
    groups: Vec<RefusedGroup>,
}

struct RefusedGroup {
    fields: HashMap<String, (String, String)>,
    why: String,
}

fn requested<'a>(
    tags: &'a [tag_sync::Write],
    placements: &'a [sync::Write],
) -> HashMap<&'a str, (String, String)> {
    tags.iter()
        .map(|w| (w.path.as_str(), (sync::hex(&w.was), sync::hex(&w.value))))
        .chain(placements.iter().map(|w| (w.path.as_str(), (w.was.clone(), w.value.clone()))))
        .collect()
}

/// hot-tags' field errors carry text. Its allocation and guest-fault errors
/// identify an address, and its structural and raw-block errors identify a path.
fn identified_fields<'a>(
    tags: &'a [tag_sync::Write],
    placements: &'a [sync::Write],
    why: &str,
) -> Vec<&'a str> {
    fn address(s: &str) -> Option<u64> {
        u64::from_str_radix(s.strip_prefix("0x")?, 16).ok()
    }
    if let Some((start, end)) = why
        .strip_suffix(" outside loaded tags")
        .and_then(|s| s.split_once(".."))
        .and_then(|(start, end)| Some((address(start)?, address(end)?)))
    {
        return tags
            .iter()
            .filter(|w| u64::from(w.at) == start && start + w.value.len() as u64 == end)
            .map(|w| w.path.as_str())
            .collect();
    }
    if let Some(fault) = why.strip_prefix("guest fault at ").and_then(address) {
        return tags
            .iter()
            .filter(|w| {
                fault.checked_sub(u64::from(w.at)).is_some_and(|off| off < w.value.len() as u64)
            })
            .map(|w| w.path.as_str())
            .collect();
    }
    let structural = why.strip_suffix(" is structural");
    let slot = why.split_once(": the block has ").map(|(slot, _)| slot);
    let block = why.split_once(": ").and_then(|(block, error)| {
        (error.starts_with("guest fault at ") || error.ends_with(" is not a block"))
            .then_some(block)
    });
    placements
        .iter()
        .filter(|w| {
            let offset = scenario_edit::layout::block(w.block).offset;
            structural == Some(w.path.as_str())
                || slot.is_some_and(|s| s == format!("+{offset:#x}[{}]", w.index))
                || block.is_some_and(|b| b == format!("+{offset:#x}"))
        })
        .map(|w| w.path.as_str())
        .collect()
}

impl Refusals {
    pub(super) fn record(
        &mut self,
        tags: &[tag_sync::Write],
        placements: &[sync::Write],
        why: &str,
    ) {
        let identified = identified_fields(tags, placements, why);
        let fields = requested(tags, placements)
            .into_iter()
            .filter(|(path, _)| identified.is_empty() || identified.contains(path))
            .map(|(path, bytes)| (path.to_owned(), bytes))
            .collect();
        self.groups.push(RefusedGroup {
            fields,
            why: why.to_owned(),
        });
    }

    pub(super) fn hold(
        &mut self,
        rows: &mut Vec<Value>,
        tags: &mut Vec<tag_sync::Write>,
        placements: &mut Vec<sync::Write>,
    ) {
        if self.groups.is_empty() {
            return;
        }
        let current = requested(tags, placements);
        self.groups.retain(|group| {
            group.fields.iter().all(|(p, bytes)| current.get(p.as_str()) == Some(bytes))
        });
        drop(current);
        let mut held = HashMap::new();
        for group in &self.groups {
            let singleton = group.fields.len() == 1;
            for path in group.fields.keys() {
                held.insert(path.as_str(), singleton.then_some(group.why.as_str()));
            }
            if !singleton {
                let mut row = tag_sync::row(
                    "batch",
                    Value::Null,
                    "Field edits",
                    tag_sync::Effect::NotKnown,
                    &group.why,
                );
                row["text"] = json!("Batch refused");
                row["refused"] = json!(true);
                row["target"] = json!({"panel": "pending"});
                rows.push(row);
            }
        }
        for row in rows {
            if let Some(Some(why)) = row["path"].as_str().and_then(|p| held.get(p)) {
                row["refused"] = json!(true);
                row["why"] = json!(why);
            }
        }
        tags.retain(|w| !held.contains_key(w.path.as_str()));
        placements.retain(|w| !held.contains_key(w.path.as_str()));
    }
}

#[cfg(all(test, unix))]
mod refusal_tests;
#[cfg(all(test, unix))]
mod tests;
