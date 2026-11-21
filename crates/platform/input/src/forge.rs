//! Desktop actions for the Forge monitor. Movement and the held keys track
//! both edges. What each does is the bindings table's
//! (`boot::forge_controls::bindings::BINDINGS`).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Toggle,
    Forward,
    Backward,
    StrafeLeft,
    StrafeRight,
    Rise,
    Fall,
    /// Held: flying is faster.
    Thrust,
    /// Fine movement, on or off.
    Fine,
    /// The object menu, or Properties of the thing aimed at or held.
    Objects,
    Delete,
    Back,
    Accept,
    Tools,
    Settings,
    Undo,
    Redo,
}
