//! The one registry of our string-id families (docs/game/ui-screens.md
//! "Ids"). A screen asks for its ids by name ([`family`]); the registry is
//! checked at compile time so no two families share an id, and no family
//! whose lookup compares fewer than 32 bits (`mask`) claims another's.
//!
//! Two collisions reached play-tests before this existed: the skull labels
//! sat on the campaign picker's ids, and the co-op texts (`0x20ffe0xx`)
//! were answered by the community variants, whose lookup then matched the
//! low 24 bits only, so the co-op screen showed "Zombies" and "SWAT".
//! `crate::string_ids` still checks each family's real lookup function
//! against every other family's ids at test time.
//!
//! Adding a family: one line in [`FAMILIES`] with an unused range, then
//! `const MINE: Family = ids::family("mine");` where the screen needs it.
//! A typo or a missing line fails the build; an overlap fails the build.

/// A family of string ids: `len` ids from `base`, answered by a lookup that
/// compares the bits in `mask` (`u32::MAX`: the whole id).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Family {
    pub name: &'static str,
    pub base: u32,
    pub len: u32,
    pub mask: u32,
}

impl Family {
    const fn new(name: &'static str, base: u32, len: u32) -> Self {
        Family {
            name,
            base,
            len,
            mask: u32::MAX,
        }
    }
    /// The family's `n`th id. Out of range fails the build in a const
    /// context and panics otherwise.
    pub const fn id(self, n: u32) -> u32 {
        assert!(n < self.len, "string id past its family's range");
        self.base + n
    }
    /// Whether `id` is one of the family's own.
    pub const fn owns(self, id: u32) -> bool {
        id >= self.base && id - self.base < self.len
    }
    /// Whether the family's lookup answers `id` (its masked bits fall in
    /// the family's masked range).
    pub const fn claims(self, id: u32) -> bool {
        let (lo, v) = (self.base & self.mask, id & self.mask);
        v >= lo && v - lo < self.len
    }
    /// The family's ids.
    pub fn ids(self) -> impl Iterator<Item = u32> {
        (0..self.len).map(move |n| self.base + n)
    }
}

/// The shared table's space (`layout_glue::ID_BASE`): `0x??fff0xx`. The
/// table answers exact ids only, so its members are told apart by the top
/// byte; [`crate::layout_glue::is_ours`] is its pre-filter.
pub const TABLE: u32 = 0x20ff_f000;

/// Every family of ours. Keep it sorted by base.
pub const FAMILIES: &[Family] = &[
    Family::new("community variants", 0x00ff_e000, 0x100),
    Family::new("variant options", 0x00ff_e100, 0x100),
    Family::new("kit.example", 0x20ff_c000, 0x40),
    Family::new("kit.choices", 0x20ff_c040, 0x40),
    Family::new("co-op texts", 0x20ff_d000, 128),
    Family::new("layouts", TABLE, 0x20),
    Family::new("speaker", TABLE + 0x30, 2),
    Family::new("live menu rows", TABLE + 0x32, 6),
    Family::new("ui model", TABLE + 0x40, 0xbf),
    Family::new("skulls", TABLE + 0x0100_0000, 0x100),
    Family::new("campaign picker", TABLE + 0x0200_0000, 65),
    Family::new("skull row labels", TABLE + 0x0300_0000, 0x100),
    Family::new("h1 options", TABLE + 0x0400_0000, 0x40),
    Family::new("signin", TABLE + 0x0500_0000, 0x100),
];

const fn same(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// The family registered as `name`. In a const (the way screens use it) a
/// missing name fails the build.
pub const fn family(name: &str) -> Family {
    let mut i = 0;
    while i < FAMILIES.len() {
        if same(FAMILIES[i].name, name) {
            return FAMILIES[i];
        }
        i += 1;
    }
    panic!("no string-id family of that name in ui_kit::ids::FAMILIES");
}

/// `None` when every family is clear of every other, else the pair's
/// indices: no shared id, no id another family's lookup claims, no name
/// twice, nothing wrapping past `u32::MAX`.
pub const fn first_clash(f: &[Family]) -> Option<(usize, usize)> {
    let mut a = 0;
    while a < f.len() {
        if f[a].len == 0 || f[a].base.checked_add(f[a].len).is_none() {
            return Some((a, a));
        }
        let mut b = 0;
        while b < f.len() {
            if a != b {
                if same(f[a].name, f[b].name) {
                    return Some((a, b));
                }
                let mut n = 0;
                while n < f[a].len {
                    if f[b].claims(f[a].base + n) {
                        return Some((a, b));
                    }
                    n += 1;
                }
            }
            b += 1;
        }
        a += 1;
    }
    None
}

const _: () = assert!(
    first_clash(FAMILIES).is_none(),
    "two string-id families overlap"
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_registry_has_no_clash_and_finds_by_name() {
        assert_eq!(first_clash(FAMILIES), None);
        assert_eq!(family("signin").base, 0x25ff_f000);
        assert_eq!(family("co-op texts").id(3), 0x20ff_d003);
    }

    /// The co-op bug as the registry would have seen it: a family whose
    /// lookup matched the low 24 bits only, and the co-op texts at
    /// `0x20ffe000`.
    #[test]
    fn a_low_24_bit_lookup_over_another_family_is_a_clash() {
        let community = Family {
            mask: 0x00ff_ffff,
            ..Family::new("community", 0x00ff_e000, 0x100)
        };
        let coop = Family::new("co-op", 0x20ff_e000, 128);
        assert_eq!(first_clash(&[community, coop]), Some((1, 0)));
        let moved = Family::new("co-op", 0x20ff_d000, 128);
        assert_eq!(first_clash(&[community, moved]), None);
    }

    /// The skulls bug: labels laid over the campaign picker's.
    #[test]
    fn overlapping_ranges_and_twice_named_families_clash() {
        let picker = Family::new("picker", 0x22ff_f000, 65);
        let labels = Family::new("labels", 0x22ff_f010, 0x40);
        assert!(first_clash(&[picker, labels]).is_some());
        let again = Family::new("picker", 0x30ff_f000, 1);
        assert!(first_clash(&[picker, again]).is_some());
    }

    #[test]
    #[should_panic]
    fn an_id_past_the_family_panics() {
        let _ = std::hint::black_box(family("signin")).id(0x100);
    }
}
