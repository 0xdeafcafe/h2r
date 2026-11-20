//! Halo 1's campaign as the import writes it and the game lists it.

/// The campaign's play order: level file names. The import writes one
/// card per level (`h1convert::ui::campaign_card`, under
/// `<data>/campaigns/<key>/cards/`); the game picker takes the first it
/// finds in this order (`boot::campaign_cards::picker`).
pub const CAMPAIGN_ORDER: [&str; 10] = [
    "a10", "a30", "a50", "b30", "b40", "c10", "c20", "c40", "d20", "d40",
];
