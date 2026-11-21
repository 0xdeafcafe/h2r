//! Forge labels: a name and an optional number a script finds a placement by.
//!
//! A label is a field of a placement in the document, set through the same
//! edit log as any other field. It has no scenario bytes, so a session that
//! runs no script pays nothing for it. A label op names a placement the way
//! a link does ([`Endpoint`]), so the label survives later appends and
//! swap-removes, and [`ForgeDoc::labels`](crate::ForgeDoc::labels) resolves
//! the active labels to final indices.
//!
//! A placement has at most one label; setting another replaces it. Labels
//! need not be unique: many placements share `"ctf_flag_spawn"`, and the
//! number tells them apart. A script asks for all of them.
//!
//! A document with a label op is format 2 ([`FORMAT_LABELS`](crate::FORMAT_LABELS)).
//! One with none stays format 1, so every older document keeps its bytes and
//! its identity.
use serde::{Deserialize, Serialize};

use crate::{BlockKind, Endpoint, OpId, Refusal, Result};

/// The most bytes a label's text takes.
pub const MAX_LABEL_BYTES: usize = 32;

/// The characters a label's text may hold: lowercase ASCII letters, digits,
/// `_`, `-` and `.`. A label is a name for a script to match, not display
/// text, and the fixed set keeps its comparison byte-exact.
pub fn label_char(c: char) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-' | '.')
}

/// A label: text of 1 to 32 bytes from [`label_char`] and an optional number
/// (`"checkpoint" 3`, `"hill" 0`).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Label {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<i32>,
}

impl Label {
    /// A validated label.
    pub fn new(text: impl Into<String>, number: Option<i32>) -> Result<Self> {
        let label = Self {
            text: text.into(),
            number,
        };
        label.check()?;
        Ok(label)
    }

    /// Refuse text that is empty, over 32 bytes or outside [`label_char`].
    pub fn check(&self) -> Result<()> {
        if self.text.is_empty() || self.text.len() > MAX_LABEL_BYTES {
            return Err(Refusal::invalid(
                "label",
                format!("want 1 to {MAX_LABEL_BYTES} bytes of text"),
            ));
        }
        if !self.text.chars().all(label_char) {
            return Err(Refusal::invalid(
                "label",
                "want lowercase letters, digits, '_', '-' and '.'",
            ));
        }
        Ok(())
    }
}

/// A label an active op gives a placement, with the placement's final index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveLabel {
    /// The op that set it.
    pub id: OpId,
    pub block: BlockKind,
    /// The entry's index after every placement edit.
    pub index: u32,
    pub label: Label,
}

/// The document's active labels, resolved against the base map's counts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Labels {
    /// By block, then final index.
    pub active: Vec<ActiveLabel>,
}

/// The label of a netgame flag of the given type on a map with no Forge
/// document (or no label on that placement), so `objects labelled
/// "ctf_flag_spawn"` works on every map. The number is the hill, 0 to 7,
/// for King of the Hill corners and none otherwise.
///
/// Types 0, 1, 4, 7, 8, 10 and 11 to 18 are traced (docs/game/bots.md 3.7;
/// `engine::objectives::kind`). Types 2 and 3 are the Assault bomb spawn and
/// goal, which bots.md 3.7 gives as "bomb and plate" without naming the
/// plate's role precisely, so their text is the less certain one. Types 5, 6
/// and 9 are not traced and have no built-in label.
pub fn builtin_label(flag_type: u16) -> Option<Label> {
    let (text, number) = match flag_type {
        0 => ("ctf_flag_spawn", None),
        1 => ("ctf_flag_return", None),
        2 => ("assault_bomb_spawn", None),
        3 => ("assault_bomb_return", None),
        4 => ("oddball_spawn", None),
        7 => ("teleporter_sender", None),
        8 => ("teleporter_receiver", None),
        10 => ("territory", None),
        11..=18 => ("hill_corner", Some(i32::from(flag_type) - 11)),
        _ => return None,
    };
    Some(Label {
        text: text.into(),
        number,
    })
}

/// The check that needs only the op.
pub(crate) fn check_op(target: &Endpoint, label: Option<&Label>) -> Result<()> {
    match *target {
        Endpoint::Base { index, .. } if index > i16::MAX as u32 => {
            return Err(Refusal::invalid("label", "entry index exceeds block limit"));
        }
        Endpoint::Added(OpId(0)) => {
            return Err(Refusal::invalid("label", "want the ID of an add op"));
        }
        _ => {}
    }
    label.map_or(Ok(()), Label::check)
}
