//! The field panel's search and **Changed only**
//! (docs/editor/tag-editing.md, "Finding a field"): which fields a
//! query's words name, over every element of every list from the element
//! shown, and the budget that stops the walk. [`crate::ui::tags`] draws
//! the hits in the filter's place; `tag.find` and `tag.view`'s `search`
//! and `changed` serve them.

use std::sync::Arc;

use map::TagId;
use serde_json::Value;

use crate::app::TagDoc;
use crate::app::tag_api::{self, ElementStep};
use crate::fields::row::{self, Context, Link, Row};
use crate::play::issues::IssueIndex;
use tags::registry::{Field, Kind, Layout};

/// List elements the walk enters, at most.
pub const MAX_ELEMENTS: usize = 5_000;
/// Lists below the element shown the walk enters, at most.
pub const MAX_DEPTH: usize = 12;
/// What the panel prints when the budget stops the search.
pub const BUDGET: &str = "Search stopped after 5,000 list items. Open a list and search there.";

/// One field the search or **Changed only** found.
pub struct Hit {
    /// The field's `tag.fields` row, as the panel draws it in its own
    /// element (its `path` is from the tag's root).
    pub row: Value,
    /// The crumbs from the tag to the hit's element, as drawn.
    pub breadcrumbs: Vec<String>,
    /// The text the query's words matched (the label for an empty query
    /// and in **Changed only**).
    pub matched: String,
}

/// What a search found: the hits in the panel's order (fields in offset
/// order, lists in order, elements in order), and whether the walk
/// stopped at the budget.
pub struct Found {
    pub hits: Vec<Hit>,
    pub truncated: bool,
    /// The budget's sentence, when the walk stopped.
    pub why: Option<&'static str>,
}

impl Found {
    fn empty() -> Found {
        Found {
            hits: Vec::new(),
            truncated: false,
            why: None,
        }
    }
}

/// The fields of `tag` that `query`'s words name — or, `changed_only`,
/// that differ from the loaded file — walking every element of every
/// list from the element at `path` (the root when empty), to
/// [`MAX_DEPTH`] lists below it and [`MAX_ELEMENTS`] elements. `unnamed`
/// leaves the unnamed rows out. A `path` that names no element of the
/// tag is an error.
pub fn find(
    doc: &TagDoc,
    tag: TagId,
    path: &str,
    link: Link,
    issues: &IssueIndex,
    query: &str,
    changed_only: bool,
    unnamed: bool,
) -> Result<Found, String> {
    let mut steps = tag_api::element_steps(doc, tag, path)?;
    Ok(find_from(
        doc,
        tag,
        &mut steps,
        link,
        issues,
        query,
        changed_only,
        unnamed,
    ))
}

/// [`find`] over the element the caller already stepped to: the Tags
/// tool's shown element. `steps` is the element's path from the root
/// (empty for it); the walk fills it further as it descends.
pub fn find_from(
    doc: &TagDoc,
    tag: TagId,
    steps: &mut Vec<ElementStep>,
    link: Link,
    issues: &IssueIndex,
    query: &str,
    changed_only: bool,
    unnamed: bool,
) -> Found {
    let Some(t) = doc.tag(tag) else {
        return Found::empty();
    };
    let Some(root) = doc.layout(tag) else {
        return Found::empty();
    };
    let (at, layout) = match steps.last() {
        Some(s) => (s.at, s.layout.clone()),
        None => (t.address, root.clone()),
    };
    let prefix = steps.iter().fold(String::new(), |acc, s| {
        format!("{}{}[{}].", acc, s.field.name, s.index)
    });
    let mut walk = Walk {
        doc,
        tag,
        link,
        issues,
        words: query
            .split_whitespace()
            .map(str::to_ascii_lowercase)
            .collect(),
        changed_only,
        unnamed,
        hits: Vec::new(),
        elements: 0,
        truncated: false,
    };
    walk.element(at, &layout, &prefix, steps, 0);
    let why = walk.truncated.then_some(BUDGET);
    Found {
        hits: walk.hits,
        truncated: walk.truncated,
        why,
    }
}

/// The walk: an element's rows, then each list's elements.
struct Walk<'a> {
    doc: &'a TagDoc,
    tag: TagId,
    link: Link,
    issues: &'a IssueIndex,
    words: Vec<String>,
    changed_only: bool,
    unnamed: bool,
    hits: Vec<Hit>,
    /// List elements entered.
    elements: usize,
    truncated: bool,
}

impl Walk<'_> {
    fn element(
        &mut self,
        at: u32,
        layout: &Arc<Layout>,
        prefix: &str,
        steps: &mut Vec<ElementStep>,
        depth: usize,
    ) {
        let cx = Context {
            link: self.link,
            issues: self.issues,
            prefix,
        };
        for r in row::rows(self.doc, self.tag, at, layout, &cx) {
            if let Some(matched) = self
                .match_of(&r)
                .filter(|_| self.unnamed || r.field.named)
                .filter(|_| !self.changed_only || r.edited)
            {
                self.hits.push(Hit {
                    breadcrumbs: row::breadcrumbs(self.doc, self.tag, steps)
                        .into_iter()
                        .map(|(text, _)| text)
                        .collect(),
                    row: r.json(self.doc, self.tag, at),
                    matched,
                });
            }
            if let (Kind::Block(child), Some(list)) = (&r.field.kind, r.list) {
                self.list(r.field, child, list, prefix, steps, depth);
            }
        }
    }

    /// One list's elements, in order, until the budget.
    fn list(
        &mut self,
        field: &Field,
        child: &Arc<Layout>,
        list: row::List,
        prefix: &str,
        steps: &mut Vec<ElementStep>,
        depth: usize,
    ) {
        if depth >= MAX_DEPTH {
            return;
        }
        for i in 0..list.count {
            if self.elements >= MAX_ELEMENTS {
                self.truncated = true;
                return;
            }
            self.elements += 1;
            let at = list.address + i * child.size;
            let step = if prefix.is_empty() {
                format!("{}[{i}]", field.name)
            } else {
                format!("{prefix}{}[{i}]", field.name)
            };
            steps.push(ElementStep {
                field: field.clone(),
                index: i,
                at,
                layout: child.clone(),
                path: step.clone(),
            });
            let inner = format!("{step}.");
            self.element(at, child, &inner, steps, depth + 1);
            steps.pop();
        }
    }

    /// Whether every word of the query is in the row's candidates, and
    /// the first candidate that alone holds them all (the label when the
    /// words are split across candidates). An empty query matches every
    /// field, matched by its label.
    fn match_of(&self, r: &Row) -> Option<String> {
        if self.words.is_empty() {
            return Some(r.label.clone());
        }
        let candidates = self.candidates(r);
        let lowered: Vec<String> = candidates.iter().map(|c| c.to_ascii_lowercase()).collect();
        if !self
            .words
            .iter()
            .all(|w| lowered.iter().any(|t| t.contains(w)))
        {
            return None;
        }
        Some(
            candidates
                .iter()
                .zip(lowered.iter())
                .find(|(_, t)| self.words.iter().all(|w| t.contains(w)))
                .map(|(c, _)| c.clone())
                .unwrap_or_else(|| r.label.clone()),
        )
    }

    /// The texts a query's words are matched against, in order: the
    /// label, the name, the description, the type words, the choice and
    /// switch names, and the referenced tag's row text.
    fn candidates(&self, r: &Row) -> Vec<String> {
        let mut out = vec![r.label.clone(), r.field.name.clone()];
        if let Some(doc) = &r.field.doc {
            out.push(doc.clone());
        }
        out.push(r.type_words.to_owned());
        match &r.field.kind {
            Kind::Enum { values, .. } => {
                out.extend(values.iter().filter(|n| row::is_name(n)).cloned());
            }
            Kind::Flags { bits, .. } => {
                out.extend(bits.iter().filter(|n| row::is_name(n)).cloned());
            }
            Kind::TagRef { .. } | Kind::Datum { .. } => {
                if let Some(text) = &r.summary {
                    out.push(text.clone());
                }
            }
            _ => {}
        }
        out
    }
}
