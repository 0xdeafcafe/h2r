//! Named guest locations: the one table of hook points and globals.
//!
//! Every guest function a feature hooks and every global more than one
//! module reads is named here once, keyed by its address in the base (1.0)
//! image, with the page that records the evidence. Code elsewhere refers to
//! the name (`points::GAME_TICK.base`), never to a second copy of the
//! number. This is the anchor list docs/title-updates.md section 3.3 asks
//! for; a title update maps these through `ImageAddrs` (docs/feature-engine.md
//! "Version key").
//!
//! No game data here: addresses and argument locations are research notes,
//! and each carries a doc reference.
//!
//! ```text
//! points! {
//!     /// The game tick.
//!     GAME_TICK @ 0x0013_7fe0: [fn() ret 0], hot; "docs/engine-map.md";
//!     /// Not read yet.
//!     SOMETHING @ 0x0012_3450: [opaque]; "docs/x.md";
//! }
//! ```

/// Where one argument or the result lives at the function's entry or exit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Loc {
    Eax,
    Al,
    Ecx,
    Edx,
    Ebx,
    Esi,
    Edi,
    Xmm0,
    /// The n-th dword argument on the stack, 0-based: `[esp + 4 + 4n]` at
    /// entry (the return address is at `[esp]`).
    Stack(u8),
}

/// A point's calling convention, as far as it has been read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sig {
    /// Not read: observers and after-hooks only, never typed arguments.
    Opaque,
    Fn {
        args: &'static [(&'static str, Loc)],
        ret: Option<Loc>,
        /// Bytes the callee pops (`ret n`).
        pop: u16,
    },
}

impl Sig {
    pub fn is_opaque(&self) -> bool {
        matches!(self, Sig::Opaque)
    }
    /// The location of the named argument.
    pub fn arg(&self, name: &str) -> Option<Loc> {
        match self {
            Sig::Opaque => None,
            Sig::Fn { args, .. } => args.iter().find(|(n, _)| *n == name).map(|(_, l)| *l),
        }
    }
}

/// Whether an image without this point can still run the features on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// A hook on it is required: missing, its feature is turned off.
    Required,
    /// A hook on it is dropped with a note when missing.
    Optional,
}

/// Flags the `points!` macro takes after the signature.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flag {
    /// Called thousands of times a frame: a hook leaves the direct-call fast
    /// path, and the plan warns.
    Hot,
    /// [`Class::Optional`].
    Optional,
}

/// A named guest function.
#[derive(Debug, PartialEq, Eq)]
pub struct Point {
    pub name: &'static str,
    /// Address in the base (1.0) image.
    pub base: u32,
    pub sig: Sig,
    pub class: Class,
    pub hot: bool,
    /// The page that records the evidence.
    pub doc: &'static str,
}

impl Point {
    pub const fn new(name: &'static str, base: u32, sig: Sig, doc: &'static str) -> Self {
        Point {
            name,
            base,
            sig,
            class: Class::Required,
            hot: false,
            doc,
        }
    }
    pub const fn with_flags(mut self, flags: &[Flag]) -> Self {
        let mut i = 0;
        while i < flags.len() {
            match flags[i] {
                Flag::Hot => self.hot = true,
                Flag::Optional => self.class = Class::Optional,
            }
            i += 1;
        }
        self
    }
}

/// A named guest global (a word or a structure at a fixed address).
#[derive(Debug, PartialEq, Eq)]
pub struct Global {
    pub name: &'static str,
    pub base: u32,
    pub doc: &'static str,
}

#[doc(hidden)]
#[macro_export]
macro_rules! point_loc {
    (eax) => {
        $crate::points::Loc::Eax
    };
    (al) => {
        $crate::points::Loc::Al
    };
    (ecx) => {
        $crate::points::Loc::Ecx
    };
    (edx) => {
        $crate::points::Loc::Edx
    };
    (ebx) => {
        $crate::points::Loc::Ebx
    };
    (esi) => {
        $crate::points::Loc::Esi
    };
    (edi) => {
        $crate::points::Loc::Edi
    };
    (xmm0) => {
        $crate::points::Loc::Xmm0
    };
    (stack $n:literal) => {
        $crate::points::Loc::Stack($n)
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! point_sig {
    (opaque) => {
        $crate::points::Sig::Opaque
    };
    (fn ( $($arg:ident : $loc:ident $($n:literal)?),* ) $(-> $ret:ident)? ret $pop:literal) => {
        $crate::points::Sig::Fn {
            args: &[$((stringify!($arg), $crate::point_loc!($loc $($n)?))),*],
            ret: $crate::point_ret!($($ret)?),
            pop: $pop,
        }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! point_ret {
    () => {
        None
    };
    ($r:ident) => {
        Some($crate::point_loc!($r))
    };
}

/// Declares named [`Point`]s (see the module docs for the grammar) and adds
/// them to a slice named by the first token (`points! { ALL; ... }`).
#[macro_export]
macro_rules! points {
    ($all:ident; $( $(#[doc = $d:literal])* $name:ident @ $addr:literal : [$($sig:tt)*] $(, $flag:ident)* ; $doc:literal ; )*) => {
        $(
            $(#[doc = $d])*
            pub static $name: $crate::points::Point = $crate::points::Point::new(
                stringify!($name),
                $addr,
                $crate::point_sig!($($sig)*),
                $doc,
            )
            .with_flags(&[$($crate::points::Flag::$flag),*]);
        )*
        /// Every point declared in this table.
        pub static $all: &[&$crate::points::Point] = &[$(&$name),*];
    };
}

/// Declares named [`Global`]s: `NAME @ 0x...; "doc";`.
#[macro_export]
macro_rules! globals {
    ($all:ident; $( $(#[doc = $d:literal])* $name:ident @ $addr:literal ; $doc:literal ; )*) => {
        $(
            $(#[doc = $d])*
            pub static $name: $crate::points::Global = $crate::points::Global {
                name: stringify!($name),
                base: $addr,
                doc: $doc,
            };
        )*
        /// Every global declared in this table.
        pub static $all: &[&$crate::points::Global] = &[$(&$name),*];
    };
}

points! { ALL;
    /// `game_tick`: the per-tick phases, then the tick count. Called once per
    /// simulated tick by `game_update_ticks`.
    GAME_TICK @ 0x0013_7fe0: [fn() ret 0]; "docs/engine-map.md";
    /// Game activation after the scenario's cache is loaded: options in EAX,
    /// AL 1 once the map and its first BSP are in.
    GAME_ACTIVATE @ 0x0018_f000: [fn(options: eax) -> al ret 0]; "docs/level-boot.md";
    /// The game-end path; the end latch is game globals `+0x1128`.
    GAME_END @ 0x0013_89c0: [opaque]; "docs/live-gamestate.md";
    /// A player was killed: `(attacker player, victim player, betrayal,
    /// packed damage)` on the stack. Who pops them is not read.
    PLAYER_KILLED @ 0x0015_ce70: [opaque]; "docs/live-gamestate.md";
    /// One weapon barrel fires: stdcall `(weapon object datum, barrel
    /// index)`. One call is one shot (likely).
    WEAPON_FIRE @ 0x0010_4150: [fn(weapon: stack 0, barrel: stack 1) ret 8], Optional; "docs/live-gamestate.md";
    /// A damage report is applied to its victim: the report in EDI, the
    /// victim object datum on the stack. Called once per damage instance.
    DAMAGE_REPORT @ 0x000d_9640: [fn(report: edi, victim: stack 0) ret 4], Optional; "docs/live-gamestate.md";
    /// `locale_string_id_find`: the string for the packed id in EDI, its
    /// address in EAX.
    LOCALE_FIND @ 0x001a_02d0: [fn(id: edi) -> eax ret 0], Hot; "docs/engine/locale_string_id_find.md";
    /// `locale_string_id_exists`: whether the packed id has a string (AL).
    LOCALE_EXISTS @ 0x001a_0310: [opaque]; "docs/engine/locale_string_id_find.md";
    /// `ice_cream_flavor_stock`'s worker: turns skull ESI on (byte ESI of
    /// the 15-byte array at `0x4f55dc`, only when it was 0), then the HUD
    /// message and sound.
    SKULL_STOCK @ 0x001d_f3e0: [fn(index: esi) ret 0]; "docs/skulls.md";
    /// Queue new game options (EAX = the options; `+0` the mode, 1 a
    /// campaign). The menu's campaign start and level boot both end here.
    OPTIONS_QUEUE @ 0x0018_e790: [fn(options: eax) ret 0]; "docs/skulls.md";
}

globals! { GLOBALS;
    /// Pointer to the game globals: active options at `+8`, "map active" at
    /// `+0x1120`, the end latch at `+0x1128`.
    GAME_GLOBALS @ 0x004e_6948; "docs/level-boot.md";
    /// Pointer to the game-time record; `+8` is the tick count.
    GAME_TIME @ 0x0051_0c54; "docs/high-framerate.md";
    /// Pointer to the players datum array (`0x21c`-byte players).
    PLAYERS @ 0x004e_8c24; "docs/live-gamestate.md";
    /// Pointer to the objects datum array (`0xc`-byte headers).
    OBJECTS @ 0x004e_0300; "docs/objects.md";
    /// Holds the guest address of the tag-index header.
    TAG_INDEX @ 0x0054_7f00; "docs/engine/tag_get_checked.md";
    /// Holds the guest address of the 16-byte tag-entry table.
    TAG_ENTRIES @ 0x004e_3b44; "docs/engine/tag_get_checked.md";
}

/// The point at `base`, if the table names one.
pub fn by_base(base: u32) -> Option<&'static Point> {
    ALL.iter().copied().find(|p| p.base == base)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_address_is_named_once() {
        let mut seen = std::collections::BTreeMap::new();
        for p in ALL {
            assert!(seen.insert(p.base, p.name).is_none(), "{} twice", p.name);
            assert!(p.doc.starts_with("docs/"), "{}: doc {}", p.name, p.doc);
        }
        for g in GLOBALS {
            assert!(seen.insert(g.base, g.name).is_none(), "{} twice", g.name);
        }
    }

    #[test]
    fn the_macro_reads_signatures_and_flags() {
        assert_eq!(
            GAME_TICK.sig,
            Sig::Fn {
                args: &[],
                ret: None,
                pop: 0
            }
        );
        assert_eq!(GAME_ACTIVATE.sig.arg("options"), Some(Loc::Eax));
        assert_eq!(WEAPON_FIRE.sig.arg("barrel"), Some(Loc::Stack(1)));
        assert_eq!(WEAPON_FIRE.class, Class::Optional);
        assert!(LOCALE_FIND.hot && !GAME_TICK.hot);
        assert!(PLAYER_KILLED.sig.is_opaque());
        assert_eq!(by_base(0x0013_7fe0).map(|p| p.name), Some("GAME_TICK"));
    }
}
