//! Skulls on the wire (docs/skulls.md): a game's or a session's skulls are
//! indices, sorted and once each: `0..15` are the engine's (byte `n` of the
//! array at `0x4f55dc`, `ice_cream_flavor_stock n`); `15..LIMIT` are left
//! for skulls of our own that later builds may add. An empty set is left
//! out of every payload, so a sender that knows nothing of skulls and one
//! with none on write the same bytes.

/// How many skulls the engine has.
pub const COUNT: u8 = 15;
/// One past the highest index the wire takes: the retail fifteen and room
/// for our own.
pub const LIMIT: u8 = 64;

/// The skulls our runtime makes work in a multiplayer game (docs/skulls.md
/// "In multiplayer"): Famine (4) and Blind (6). Every other skull's code is
/// campaign-only or needs AI, so a multiplayer game never names one.
pub const MULTIPLAYER: [u8; 2] = [4, 6];

/// Checks a set as it arrives: indices below [`LIMIT`], ascending, no
/// repeats, and not empty (an empty set is sent as no field).
pub fn validate(skulls: &[u8]) -> Result<(), String> {
    if skulls.is_empty() {
        return Err("skulls is left out when none are on, never empty".into());
    }
    if skulls.iter().any(|s| *s >= LIMIT) {
        return Err(format!("skulls are indices 0 to {}", LIMIT - 1));
    }
    if !skulls.windows(2).all(|w| w[0] < w[1]) {
        return Err("skulls are ascending, each once".into());
    }
    Ok(())
}

/// A set as it is sent: sorted, each once, only real indices; `None` when
/// that leaves nothing (the field is left out).
pub fn normalize(skulls: impl IntoIterator<Item = u8>) -> Option<Vec<u8>> {
    let mut v: Vec<u8> = skulls.into_iter().filter(|s| *s < LIMIT).collect();
    v.sort_unstable();
    v.dedup();
    (!v.is_empty()).then_some(v)
}

/// A set from the engine's array layout (`on[n]` for skull `n`).
pub fn from_flags(on: &[bool]) -> Option<Vec<u8>> {
    normalize(
        on.iter()
            .enumerate()
            .filter(|(_, o)| **o)
            .filter_map(|(i, _)| u8::try_from(i).ok()),
    )
}

/// Whether a multiplayer game may name `skulls`: only [`MULTIPLAYER`] ones.
pub fn multiplayer_ok(skulls: &[u8]) -> bool {
    skulls.iter().all(|s| MULTIPLAYER.contains(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_are_sorted_unique_and_in_range() {
        assert_eq!(validate(&[0, 6, 14]), Ok(()));
        assert_eq!(validate(&[15, 63]), Ok(()), "room for skulls of our own");
        assert!(validate(&[]).is_err());
        assert!(validate(&[64]).is_err());
        assert!(validate(&[6, 4]).is_err());
        assert!(validate(&[4, 4]).is_err());
        assert_eq!(normalize([14, 6, 6, 99, 0, 20]), Some(vec![0, 6, 14, 20]));
        assert_eq!(normalize([99]), None);
        assert_eq!(normalize([]), None);
        let mut on = [false; 15];
        assert_eq!(from_flags(&on), None);
        on[12] = true;
        on[3] = true;
        assert_eq!(from_flags(&on), Some(vec![3, 12]));
    }

    #[test]
    fn multiplayer_names_only_the_skulls_that_work_there() {
        assert!(multiplayer_ok(&[4, 6]));
        assert!(multiplayer_ok(&[6]));
        assert!(!multiplayer_ok(&[6, 10]));
        assert!(MULTIPLAYER.iter().all(|s| *s < COUNT));
    }
}
