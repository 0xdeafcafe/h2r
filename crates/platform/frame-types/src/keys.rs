//! Identities across frames: a view's ([`ViewKey`]) and an object's
//! ([`InstanceKey`]). `frame_scene::View` and `frame_scene::Instance` carry
//! them; the device's carried draws and residency name them.

use std::fmt;

use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum ViewKind {
    Game,
    Cinematic,
    Alternate,
    /// The first-person draw `0x0003fdf0` of a game view: its own
    /// projection and lists.
    FirstPerson,
}

/// A view's identity across scenes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct ViewKey {
    pub kind: ViewKind,
    pub player: u8,
    /// The guest's view index.
    pub view: u8,
}

impl fmt::Display for ViewKey {
    /// `game:0:0` (kind:player:view), as `render.scene` names it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self.kind {
            ViewKind::Game => "game",
            ViewKind::Cinematic => "cinematic",
            ViewKind::Alternate => "alternate",
            ViewKind::FirstPerson => "first_person",
        };
        write!(f, "{kind}:{}:{}", self.player, self.view)
    }
}

/// An object's identity across scenes: its datum handle with its salt.
/// Opaque to the renderer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct InstanceKey(pub u32);
