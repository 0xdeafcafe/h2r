//! The bodies a host gives through `set_body`: Halo 2's two multiplayer
//! bodies, each by the tag path a script names. See
//! `docs/game/rules-engine.md`, "The engine API", and the evidence in
//! `docs/game/notes/2026-11-24-siege-spawns-and-bodies.md`.
//!
//! A multiplayer player's body is the map's player representation that the
//! player's model byte picks (the active configuration's model, player
//! `+0x88`: 0 the Spartan, 1 the Elite); the map's globals decide which
//! biped that is. The path only says which model a script asks for. A path
//! outside the list is refused: the checker refuses it written in the
//! call, and the VM stops on it otherwise ([`crate::vm::ErrorKind::Body`]).

/// One of the two multiplayer bodies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Body {
    Spartan,
    Elite,
}

impl Body {
    /// The model byte the representation pick reads for it.
    pub fn model(self) -> u8 {
        match self {
            Body::Spartan => 0,
            Body::Elite => 1,
        }
    }

    /// Its byte in the VM's state.
    pub fn code(self) -> u8 {
        match self {
            Body::Spartan => 1,
            Body::Elite => 2,
        }
    }

    pub fn from_code(c: u8) -> Option<Body> {
        Some(match c {
            1 => Body::Spartan,
            2 => Body::Elite,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Body::Spartan => "spartan",
            Body::Elite => "elite",
        }
    }
}

/// Every body a host gives, by the tag path a script names.
pub const LIST: &[(&str, Body)] = &[
    ("objects/characters/masterchief/masterchief_mp", Body::Spartan),
    ("objects/characters/elite/elite_mp", Body::Elite),
];

/// The body a tag path names, in either slash and any case.
pub fn of_path(path: &str) -> Option<Body> {
    let p = crate::weapons::canonical(path);
    LIST.iter().find(|(q, _)| *q == p).map(|&(_, b)| b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_body_is_named_by_its_path_in_either_slash() {
        assert_eq!(of_path("objects/characters/elite/elite_mp"), Some(Body::Elite));
        assert_eq!(
            of_path("Objects\\Characters\\Masterchief\\Masterchief_MP"),
            Some(Body::Spartan)
        );
        assert_eq!(of_path("objects/characters/elite/elite"), None);
        for &(_, b) in LIST {
            assert_eq!(Body::from_code(b.code()), Some(b));
        }
        assert_eq!(Body::from_code(0), None);
        assert_eq!((Body::Spartan.model(), Body::Elite.model()), (0, 1));
    }
}
