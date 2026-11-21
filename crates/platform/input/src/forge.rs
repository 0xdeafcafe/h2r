//! Desktop actions for the Forge monitor. Movement tracks both edges.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Toggle,
    Forward,
    Backward,
    StrafeLeft,
    StrafeRight,
    Rise,
    Fall,
    Palette,
    Properties,
    Delete,
    Back,
    Accept,
    Up,
    Down,
    Undo,
    Redo,
    /// The Links tool: switch to it, or start or cancel a link.
    Link,
}
