//! Problems shared by the link pill, Pending, the inspector and the game plate.

use std::collections::HashMap;

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
    fn name(self) -> &'static str {
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

    pub fn matches_placement(&self, block: BlockKind, index: usize, field: Option<&str>) -> bool {
        self.placement == Some((block, index))
            && field.is_none_or(|f| self.field.as_deref() == Some(f))
    }

    pub fn matches_tag(&self, tag: TagId, at: Option<u32>) -> bool {
        self.target["tag"].as_u64() == Some(u64::from(tag.0))
            && at.is_none_or(|a| self.at == Some(a))
    }
}

fn target(app: &App, row: &Value) -> (Value, Option<(BlockKind, usize)>) {
    let target = &row["target"];
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
        result["path"] = json!(path);
    }
    (result, None)
}

pub(super) fn for_pending(app: &App) -> Vec<Issue> {
    app.pairing.pending.iter().map(|row| {
        let kind = if row["conflict"] == true {
            Kind::Mismatch
        } else if row["refused"] == true {
            Kind::Refused
        } else if row["class"] == tag_sync::RELOAD {
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

impl LinkView {
    /// The largest issue count. A tie gives the actionable problem first.
    pub fn issue_summary(&self) -> Option<String> {
        let mut largest = 0;
        let mut summary = None;
        for kind in [Kind::Refused, Kind::Mismatch, Kind::Reload, Kind::Waiting] {
            let n = self.issues.iter().filter(|issue| issue.kind == kind).count();
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

/// A refused atomic batch marks just the pending writes still asking for those bytes.
#[derive(Default)]
pub(super) struct Refusals {
    fields: HashMap<String, (String, String)>,
}

impl Refusals {
    pub(super) fn record(&mut self, tags: &[tag_sync::Write], placements: &[sync::Write], why: &str) {
        for w in tags {
            self.fields.insert(w.path.clone(), (sync::hex(&w.value), why.into()));
        }
        for w in placements {
            self.fields.insert(w.path.clone(), (w.value.clone(), why.into()));
        }
    }

    pub(super) fn mark(
        &mut self,
        rows: &mut [Value],
        tags: &[tag_sync::Write],
        placements: &[sync::Write],
    ) {
        if self.fields.is_empty() {
            return;
        }
        let mut remaining = HashMap::new();
        for w in tags {
            if let Some((path, refusal)) = self.fields.remove_entry(&w.path)
                && sync::unhex(&refusal.0).as_deref() == Some(w.value.as_slice())
            {
                remaining.insert(path, refusal);
            }
        }
        for w in placements {
            if let Some((path, refusal)) = self.fields.remove_entry(&w.path) && refusal.0 == w.value {
                remaining.insert(path, refusal);
            }
        }
        for row in rows {
            if let Some((_, why)) = row["path"].as_str().and_then(|p| remaining.get(p)) {
                row["refused"] = json!(true);
                row["why"] = json!(why);
            }
        }
        self.fields = remaining;
    }

    pub(super) fn exclude_unchanged(
        &self,
        tags: &mut Vec<tag_sync::Write>,
        placements: &mut Vec<sync::Write>,
    ) {
        tags.retain(|w| !self.fields.contains_key(&w.path));
        placements.retain(|w| !self.fields.contains_key(&w.path));
    }
}

#[cfg(all(test, unix))]
mod tests;
