//! The system keyboard on iOS (and tvOS) typing into the game's virtual
//! keyboard (docs/host-keyboard.md, docs/ios.md).
//!
//! The game side is the Mac's: a guest-thread hook notices when the
//! virtual keyboard screen has an editable, focused field
//! (`KeyboardMouse::text_active`) and applies queued edits through the
//! game's own insert, Backspace, Done and Cancel paths. Here the host's
//! `UIKeyInput` calls become those edits, and [`Edges`] turns the field's
//! state into the moments the host shows or hides its keyboard.

use input::backend::keyboard::{KeyboardMouse, TextAction};

/// What changed about the game's text field since the host last asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    None,
    /// A field opened: show the system keyboard.
    Opened,
    /// The field closed (Done, Cancel, or the screen went away): hide it.
    Closed,
}

impl Edge {
    /// The C API's number: 1 opened, -1 closed, 0 nothing new.
    pub fn code(self) -> i32 {
        match self {
            Edge::None => 0,
            Edge::Opened => 1,
            Edge::Closed => -1,
        }
    }
}

/// Edge detection over the field's state, polled once per display frame.
#[derive(Debug, Default)]
pub struct Edges {
    active: bool,
}

impl Edges {
    pub fn update(&mut self, active: bool) -> Edge {
        let was = std::mem::replace(&mut self.active, active);
        match (was, active) {
            (false, true) => Edge::Opened,
            (true, false) => Edge::Closed,
            _ => Edge::None,
        }
    }
}

/// `UIKeyInput.insertText` text as edits. The system keyboard's Return
/// arrives as `"\n"` and is Done; other control characters are dropped
/// (the game ignores them anyway); the printable runs between are inserted
/// as committed text, which the hook filters further as it does on the Mac.
pub fn insert_actions(text: &str) -> Vec<TextAction> {
    let mut out = Vec::new();
    let mut run = String::new();
    let mut prev = None;
    for c in text.chars() {
        let after_cr = std::mem::replace(&mut prev, Some(c)) == Some('\r');
        if c == '\r' || c == '\n' {
            // "\r\n" is one Return.
            if c == '\n' && after_cr {
                continue;
            }
            if !run.is_empty() {
                out.push(TextAction::Insert(std::mem::take(&mut run)));
            }
            out.push(TextAction::Accept);
        } else if !c.is_control() {
            run.push(c);
        }
    }
    if !run.is_empty() {
        out.push(TextAction::Insert(run));
    }
    out
}

/// Queues `actions` for the field. False when no field is open (the edit
/// is dropped, as a stale one would be).
pub fn apply(keyboard: &KeyboardMouse, actions: Vec<TextAction>) -> bool {
    let mut any = false;
    for a in actions {
        any |= match a {
            TextAction::Insert(s) => keyboard.commit_text(&s),
            other => keyboard.text_key(None, true, false, Some(other)),
        };
    }
    any
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edges_fire_once_per_change() {
        let mut e = Edges::default();
        assert_eq!(e.update(false), Edge::None);
        assert_eq!(e.update(true), Edge::Opened);
        assert_eq!(e.update(true), Edge::None);
        assert_eq!(e.update(false), Edge::Closed);
        assert_eq!(e.update(false), Edge::None);
        assert_eq!(
            [Edge::None, Edge::Opened, Edge::Closed].map(Edge::code),
            [0, 1, -1]
        );
    }

    #[test]
    fn return_is_done_and_controls_are_dropped() {
        use TextAction::*;
        assert_eq!(insert_actions("abc"), [Insert("abc".into())]);
        assert_eq!(insert_actions("\n"), [Accept]);
        assert_eq!(insert_actions("ab\ncd"), [Insert("ab".into()), Accept, Insert("cd".into())]);
        assert_eq!(insert_actions("\r\n"), [Accept]);
        assert_eq!(insert_actions("\n\n"), [Accept, Accept]);
        assert_eq!(insert_actions("\r\r\n"), [Accept, Accept]);
        assert_eq!(insert_actions("a\tb\u{7}"), [Insert("ab".into())]);
        assert_eq!(insert_actions("Ünïcödé ✓"), [Insert("Ünïcödé ✓".into())]);
        assert!(insert_actions("").is_empty());
    }

    #[test]
    fn edits_reach_the_open_field_and_are_dropped_without_one() {
        let kb = KeyboardMouse::new(Default::default());
        assert!(!apply(&kb, insert_actions("x")), "no field yet");
        kb.set_text_target(Some(9));
        assert!(kb.text_active());
        assert!(apply(&kb, insert_actions("hi\n")));
        assert!(apply(&kb, vec![TextAction::Backspace, TextAction::Cancel]));
        assert_eq!(
            kb.take_text(9),
            [
                TextAction::Insert("hi".into()),
                TextAction::Accept,
                TextAction::Backspace,
                TextAction::Cancel
            ]
        );
        kb.set_text_target(None);
        assert!(!apply(&kb, vec![TextAction::Backspace]));
    }
}
