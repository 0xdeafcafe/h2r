//! The front end's types: what the checker gives every expression. They
//! live in the front end only and are erased before the IR
//! ([`Type::erase`]). See `docs/game/rules-language-types.md`, "Front-end
//! types".

use crate::ty::Ty;

/// A handle type: a value that names a thing, and may be `none` as `T?`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Handle {
    Player,
    Team,
    /// Anything placed or made that has a handle.
    Object,
    /// An object with a volume: a Forge trigger volume, or a netgame flag
    /// with its capture volume.
    Zone,
    /// A tag path of any object definition.
    Tag,
    /// A tag path that must be a weapon.
    Weapon,
    /// A tag path that must be a sound's (`snd!`).
    Sound,
}

impl Handle {
    pub fn name(self) -> &'static str {
        match self {
            Handle::Player => "player",
            Handle::Team => "team",
            Handle::Object => "object",
            Handle::Zone => "zone",
            Handle::Tag => "tag",
            Handle::Weapon => "weapon",
            Handle::Sound => "sound",
        }
    }

    /// The IR type the handle erases to.
    pub fn erase(self) -> Ty {
        match self {
            Handle::Player => Ty::Player,
            Handle::Team => Ty::Team,
            Handle::Object | Handle::Zone => Ty::Object,
            Handle::Tag | Handle::Weapon | Handle::Sound => Ty::Tag,
        }
    }

    /// Whether a value of `self` is a value of `to`: the same handle, a
    /// zone as an object, or a weapon as a tag.
    pub fn fits(self, to: Handle) -> bool {
        self == to || matches!((self, to), (Handle::Zone, Handle::Object) | (Handle::Weapon | Handle::Sound, Handle::Tag))
    }

    /// `a`, `an` and the name: "a zone", "an object".
    pub fn a(self) -> String {
        let n = self.name();
        if matches!(n.as_bytes()[0], b'o') { format!("an {n}") } else { format!("a {n}") }
    }
}

/// A set: what a loop visits, or what a set parameter takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SetKind {
    Players,
    Teams,
    Objectives,
    Objects,
    Zones,
    /// Starting locations: passed to `spawn_areas`, never visited.
    Spawns,
    /// The points of a flight path: passed to `fly`, never visited.
    Paths,
}

impl SetKind {
    pub fn name(self) -> &'static str {
        match self {
            SetKind::Players => "players",
            SetKind::Teams => "teams",
            SetKind::Objectives => "objectives",
            SetKind::Objects => "objects",
            SetKind::Zones => "zones",
            SetKind::Spawns => "spawns",
            SetKind::Paths => "paths",
        }
    }

    /// The type of a member a loop visits; none for spawn areas.
    pub fn member(self) -> Option<Handle> {
        match self {
            SetKind::Players => Some(Handle::Player),
            SetKind::Teams => Some(Handle::Team),
            SetKind::Objectives | SetKind::Objects => Some(Handle::Object),
            SetKind::Zones => Some(Handle::Zone),
            SetKind::Spawns | SetKind::Paths => None,
        }
    }

    /// The message for a set with no members to visit (`to` is "visit" or
    /// "test"), saying which call takes it; `None` for a set a script can
    /// visit.
    pub fn unvisited(self, name: &str, to: &str) -> Option<String> {
        match self {
            SetKind::Spawns => Some(format!(
                "{name} names spawn areas, which have no objects to {to}. Pass it to spawn_areas."
            )),
            SetKind::Paths => Some(format!(
                "{name} names a flight path, whose points are not objects to {to}. Pass it to fly."
            )),
            _ => None,
        }
    }

    /// Whether a set of `self` is a set of `to`: a set of zones is a set
    /// of objects.
    pub fn fits(self, to: SetKind) -> bool {
        self == to || (self == SetKind::Zones && to == SetKind::Objects)
    }

    /// The words a message says the set with: "a set of zones", "spawn
    /// areas".
    pub fn words(self) -> &'static str {
        match self {
            SetKind::Spawns => "spawn areas",
            SetKind::Paths => "a flight path",
            SetKind::Zones => "a set of zones",
            SetKind::Players => "a set of players",
            SetKind::Teams => "a set of teams",
            SetKind::Objectives | SetKind::Objects => "a set of objects",
        }
    }
}

/// A front-end type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Type {
    Int,
    Fixed,
    /// A duration or a tick count, in ticks (`5s`, `300t`).
    Time,
    Bool,
    Squad,
    Class,
    Text,
    /// A script-declared enum, by its index in the enum table.
    Enum(u8),
    /// A handle; with `true`, `T?`.
    H(Handle, bool),
    /// A set (`zones`): read where a set is read, never a value.
    Set(SetKind),
    /// A point or an offset in world units: three `fixed` registers the
    /// lowering keeps together. It is the value of an expression
    /// (`vec(x, y, z)`, `o.position`, `a + b`) or a `let`, and is never
    /// stored in a variable, an option, a table or a parameter.
    Vec,
}

impl Type {
    pub const PLAYER: Type = Type::H(Handle::Player, false);
    pub const TEAM: Type = Type::H(Handle::Team, false);
    pub const OBJECT: Type = Type::H(Handle::Object, false);
    pub const ZONE: Type = Type::H(Handle::Zone, false);
    pub const TAG: Type = Type::H(Handle::Tag, false);
    pub const WEAPON: Type = Type::H(Handle::Weapon, false);
    pub const SOUND: Type = Type::H(Handle::Sound, false);

    /// The IR type; a set has none (its argument is a `Label` constant).
    pub fn erase(self) -> Option<Ty> {
        Some(match self {
            Type::Int | Type::Time => Ty::Int,
            Type::Fixed => Ty::Fixed,
            Type::Bool => Ty::Bool,
            Type::Squad => Ty::Squad,
            Type::Class => Ty::Class,
            Type::Text => Ty::Text,
            Type::Enum(e) => Ty::Enum(e),
            Type::H(h, _) => h.erase(),
            // The type of each of its registers.
            Type::Vec => Ty::Fixed,
            Type::Set(_) => return None,
        })
    }

    /// The registers a value of the type takes: three for a `vec`, one
    /// for the rest.
    pub fn width(self) -> usize {
        if self == Type::Vec { 3 } else { 1 }
    }

    /// The register or argument type: a set argument is a label index.
    pub fn ir(self) -> Ty {
        self.erase().unwrap_or(Ty::Label)
    }

    /// The front-end type an IR type reads as where no signature says
    /// more: a handle that is never `none`. `timer` reads as `time`; a
    /// label is not a value.
    pub fn from_ir(t: Ty) -> Option<Type> {
        Some(match t {
            Ty::Int => Type::Int,
            Ty::Timer => Type::Time,
            Ty::Fixed => Type::Fixed,
            Ty::Bool => Type::Bool,
            Ty::Player => Type::PLAYER,
            Ty::Team => Type::TEAM,
            Ty::Object => Type::OBJECT,
            Ty::Squad => Type::Squad,
            Ty::Class => Type::Class,
            Ty::Tag => Type::TAG,
            Ty::Text => Type::Text,
            Ty::Enum(e) => Type::Enum(e),
            Ty::Label => return None,
        })
    }

    /// `T?`.
    pub fn is_opt(self) -> bool {
        matches!(self, Type::H(_, true))
    }

    pub fn handle(self) -> Option<Handle> {
        match self {
            Type::H(h, _) => Some(h),
            _ => None,
        }
    }

    /// `T` for `T?`; itself otherwise.
    pub fn unwrapped(self) -> Type {
        match self {
            Type::H(h, _) => Type::H(h, false),
            t => t,
        }
    }

    /// `T?` for a handle `T`; itself otherwise.
    pub fn optional(self) -> Type {
        match self {
            Type::H(h, _) => Type::H(h, true),
            t => t,
        }
    }

    pub fn numeric(self) -> bool {
        matches!(self, Type::Int | Type::Fixed)
    }

    /// Whether a value of `self` may stand where `to` is wanted: the same
    /// type, a zone as an object, a weapon as a tag, `T` as `T?`, and a
    /// set of zones as a set of objects. Nothing else converts.
    pub fn fits(self, to: Type) -> bool {
        match (self, to) {
            (Type::H(a, ao), Type::H(b, bo)) => a.fits(b) && (!ao || bo),
            (Type::Set(a), Type::Set(b)) => a.fits(b),
            (a, b) => a == b,
        }
    }

    /// Whether `==` and `!=` compare the two: one fits the other, `none`
    /// aside.
    pub fn comparable(self, other: Type) -> bool {
        match (self, other) {
            (Type::H(a, _), Type::H(b, _)) => a.fits(b) || b.fits(a),
            (Type::Set(_), _) | (_, Type::Set(_)) => false,
            (a, b) => a == b,
        }
    }

    /// The type a word names, with `?` for `optional`: `int`, `fixed`,
    /// `time`, `bool`, `player`, `team`, `object`, `zone`, `squad`, `class`
    /// (`Class`), `tag`, `weapon`, `sound`, `text`, the set words `objects`,
    /// `zones` and `spawns`, or a declared enum. `timer` is a variable's
    /// type, not a value's, and is the caller's to read. `None` for an
    /// unknown word or a `?` on a type with no `none`.
    pub fn named(word: &str, optional: bool, enums: &[String]) -> Option<Type> {
        let t = match word {
            "int" => Type::Int,
            "fixed" => Type::Fixed,
            "time" => Type::Time,
            "bool" => Type::Bool,
            "player" => Type::PLAYER,
            "team" => Type::TEAM,
            "object" => Type::OBJECT,
            "zone" => Type::ZONE,
            "squad" => Type::Squad,
            "Class" | "class" => Type::Class,
            "tag" => Type::TAG,
            "weapon" => Type::WEAPON,
            "sound" => Type::SOUND,
            "text" => Type::Text,
            "objects" => Type::Set(SetKind::Objects),
            "zones" => Type::Set(SetKind::Zones),
            "spawns" => Type::Set(SetKind::Spawns),
            "vec" => Type::Vec,
            other => Type::Enum(enums.iter().position(|n| n == other)? as u8),
        };
        match (t, optional) {
            (Type::H(h, _), true) => Some(Type::H(h, true)),
            (_, true) => None,
            (t, false) => Some(t),
        }
    }

    /// The name the language spells the type with: `player?`, `zones`,
    /// an enum's own name.
    pub fn name(self, enums: &[String]) -> String {
        match self {
            Type::Int => "int".into(),
            Type::Fixed => "fixed".into(),
            Type::Time => "time".into(),
            Type::Bool => "bool".into(),
            Type::Squad => "squad".into(),
            Type::Class => "Class".into(),
            Type::Text => "text".into(),
            Type::Enum(e) => enums.get(e as usize).cloned().unwrap_or_else(|| "enum".into()),
            Type::H(h, false) => h.name().into(),
            Type::H(h, true) => format!("{}?", h.name()),
            Type::Set(s) => s.name().into(),
            Type::Vec => "vec".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_conversions_and_nothing_else() {
        assert!(Type::ZONE.fits(Type::OBJECT));
        assert!(Type::WEAPON.fits(Type::TAG));
        assert!(Type::PLAYER.fits(Type::PLAYER.optional()));
        assert!(Type::ZONE.fits(Type::OBJECT.optional()));
        assert!(!Type::OBJECT.fits(Type::ZONE));
        assert!(!Type::TAG.fits(Type::WEAPON));
        assert!(!Type::PLAYER.optional().fits(Type::PLAYER));
        assert!(!Type::Int.fits(Type::Fixed));
        assert!(Type::Set(SetKind::Zones).fits(Type::Set(SetKind::Objects)));
        assert!(!Type::Set(SetKind::Objects).fits(Type::Set(SetKind::Spawns)));
        assert!(Type::PLAYER.optional().comparable(Type::PLAYER));
        assert!(Type::OBJECT.comparable(Type::ZONE));
        assert!(!Type::PLAYER.comparable(Type::TEAM));
    }

    #[test]
    fn types_erase_to_the_ir() {
        assert_eq!(Type::ZONE.erase(), Some(Ty::Object));
        assert_eq!(Type::WEAPON.optional().erase(), Some(Ty::Tag));
        assert_eq!(Type::Set(SetKind::Spawns).erase(), None);
        assert_eq!(Type::Set(SetKind::Spawns).ir(), Ty::Label);
        assert_eq!(Type::named("zone", true, &[]), Some(Type::ZONE.optional()));
        assert_eq!(Type::named("int", true, &[]), None);
        assert_eq!(Type::named("time", false, &[]), Some(Type::Time));
        assert_eq!(Type::Time.erase(), Some(Ty::Int));
        assert_eq!(Type::from_ir(Ty::Timer), Some(Type::Time));
        assert!(!Type::Time.fits(Type::Int));
        assert!(!Type::Int.fits(Type::Time));
        assert_eq!(Type::named("Role", false, &["Role".into()]), Some(Type::Enum(0)));
        assert_eq!(Type::ZONE.optional().name(&[]), "zone?");
        assert_eq!(Type::named("vec", false, &[]), Some(Type::Vec));
        assert_eq!(Type::named("vec", true, &[]), None);
        assert_eq!(Type::Vec.name(&[]), "vec");
        assert_eq!((Type::Vec.erase(), Type::Vec.width(), Type::Fixed.width()), (Some(Ty::Fixed), 3, 1));
        assert!(!Type::Vec.fits(Type::Fixed) && !Type::Fixed.fits(Type::Vec) && Type::Vec.fits(Type::Vec));
    }
}
