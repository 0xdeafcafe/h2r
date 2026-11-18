//! The Multiplayer Map Pack's maps, as packages name their bases
//! (docs/game/maps.md, "Importing a mod"; docs/game/map-pack.md).
//!
//! `h2 import-mappack` installs each map byte for byte as
//! `T:\$c\<offer ID>\<map>.map` on the data partition, one content package
//! per map, with offers whose low half is in [`OFFERS`]. A package whose
//! `[base] map` is one of [`MAPS`] is laid over that installed copy, not a
//! disc map: the names never collide with the Halo 2 disc's.

/// The map pack's maps: the map file's stem, then the name the game shows.
pub const MAPS: [(&str, &str); 9] = [
    ("backwash", "Backwash"),
    ("containment", "Containment"),
    ("deltatap", "Sanctuary"),
    ("dune", "Relic"),
    ("elongation", "Elongation"),
    ("gemini", "Gemini"),
    ("triplicate", "Terminal"),
    ("turf", "Turf"),
    ("warlock", "Warlock"),
];

/// The low half of the offer IDs `h2 import-mappack` gives the map pack's
/// packages (docs/game/map-pack.md §5): `…0101` to `…03ff`.
pub const OFFERS: std::ops::RangeInclusive<u32> = 0x0101..=0x03ff;

/// Whether `map` (a stem like `turf`, any case) is a map pack map.
pub fn is_map(map: &str) -> bool {
    MAPS.iter().any(|(m, _)| m.eq_ignore_ascii_case(map))
}

/// The name the game shows for map pack map `map` (`Relic` for `dune`).
pub fn title(map: &str) -> Option<&'static str> {
    MAPS.iter()
        .find(|(m, _)| m.eq_ignore_ascii_case(map))
        .map(|(_, t)| *t)
}

/// Whether `offer` (a whole 64-bit offer ID) is one the import gives a
/// map pack package: `title`'s, low half in [`OFFERS`].
pub fn is_offer(title: u32, offer: u64) -> bool {
    (offer >> 32) as u32 == title && OFFERS.contains(&(offer as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_titles_and_offers() {
        assert!(is_map("TURF") && is_map("deltatap") && !is_map("lockout"));
        assert_eq!(title("dune"), Some("Relic"));
        assert_eq!(title("Triplicate"), Some("Terminal"));
        assert_eq!(title("lockout"), None);
        assert!(is_offer(0x4D53_0064, 0x4D53_0064_0000_0101));
        assert!(is_offer(0x4D53_0064, 0x4D53_0064_0000_03ff));
        assert!(!is_offer(0x4D53_0064, 0x4D53_0064_0000_0400));
        assert!(!is_offer(0x4D53_0064, 0x4D53_0065_0000_0101));
    }
}
