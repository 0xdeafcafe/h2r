//! The pogo stick's shared data: its looks, its rig and which weapon
//! tags each served map holds. The kit (`content-kit`) builds the tags from
//! `content/pogo/` (its `kit.yaml` holds the same rig, which a test pins to
//! [`RIG`]); the game reads them to hand a player a stick and pose a rider.

/// The folder every look's tags sit under.
pub const FOLDER: &str = r"objects\weapons\multiplayer\pogo";

/// A look. The index is the script's `enum PogoStyle { unsc, covenant,
/// forerunner }`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Style {
    Unsc,
    Covenant,
    Forerunner,
}

impl Style {
    pub const ALL: [Style; 3] = [Style::Unsc, Style::Covenant, Style::Forerunner];

    pub fn index(self) -> usize {
        match self {
            Style::Unsc => 0,
            Style::Covenant => 1,
            Style::Forerunner => 2,
        }
    }

    pub fn from_index(i: i32) -> Option<Style> {
        usize::try_from(i)
            .ok()
            .and_then(|i| Style::ALL.get(i).copied())
    }

    /// `unsc`, `covenant` or `forerunner`.
    pub fn id(self) -> &'static str {
        match self {
            Style::Unsc => "unsc",
            Style::Covenant => "covenant",
            Style::Forerunner => "forerunner",
        }
    }

    /// The look's tags' path, the same for each group:
    /// `objects\weapons\multiplayer\pogo\unsc\pogo_unsc`.
    pub fn tag_path(self) -> String {
        let id = self.id();
        format!(r"{FOLDER}\{id}\pogo_{id}")
    }
}

/// Where a rider meets the stick, model space, world units: +X forward, +Y
/// left, +Z up, the origin the right grip.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rig {
    pub grip_r: [f32; 3],
    pub grip_l: [f32; 3],
    pub peg_r: [f32; 3],
    pub peg_l: [f32; 3],
    /// The foot's underside, the piston out.
    pub foot: [f32; 3],
    /// The piston node, in the frame.
    pub piston: [f32; 3],
    /// How far the piston slides up into the sleeve.
    pub travel: f32,
}

pub const RIG: Rig = Rig {
    grip_r: [0.0, 0.0, 0.0],
    grip_l: [0.0, 0.118, 0.0],
    peg_r: [0.0, 0.023, -0.279],
    peg_l: [0.0, 0.095, -0.279],
    foot: [0.0, 0.059, -0.361],
    piston: [0.0, 0.059, -0.200],
    travel: 0.049,
};

/// The weapon tags the kit put in each map, by map stem: written when a
/// map is served with the kit, read by the code that hands a player a
/// stick.
pub mod placed {
    use std::collections::HashMap;
    use std::sync::{Mutex, MutexGuard, PoisonError};

    /// The three looks' `weap` datums, by [`crate::Style::index`].
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Placed {
        pub weapons: [u32; 3],
    }

    static PLACED: Mutex<Option<HashMap<String, Placed>>> = Mutex::new(None);

    fn table() -> MutexGuard<'static, Option<HashMap<String, Placed>>> {
        PLACED.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn key(stem: &str) -> String {
        stem.to_ascii_lowercase()
    }

    pub fn set(stem: &str, p: Placed) {
        table()
            .get_or_insert_with(HashMap::new)
            .insert(key(stem), p);
    }

    pub fn get(stem: &str) -> Option<Placed> {
        table().as_ref()?.get(&key(stem)).copied()
    }

    pub fn clear(stem: &str) {
        if let Some(t) = table().as_mut() {
            t.remove(&key(stem));
        }
    }
}
