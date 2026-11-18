//! OptiMatch search: what the retail title's search procedure 1 asks for.
//!
//! The game calls `XOnlineMatchSearch(1, 50, params, 12, ..)` with twelve
//! anonymous integer parameters (built at `0x000b4a90`), and advertises its
//! sessions with fifteen integer attributes (built at `0x000b4b80`). Retail
//! Live ran the matching stored procedure; this module is our reading of it:
//! which attribute each parameter is compared with, and how. Values are the
//! title's own (hopper ids, Halo 2's 1..50 skill levels, party sizes), so the
//! server can filter and order sessions without knowing anything else about
//! the game. See docs/live/live-matchmaking.md §7.
use crate::session::AttributeValue;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The procedure number both of the title's searches use.
pub const PROCEDURE: u32 = 1;
/// How many positional parameters procedure 1 takes.
pub const PARAMS: usize = 12;

/// Session attribute ids, as `0x000b4b80` writes them.
pub mod attr {
    /// The playlist (hopper id); -1 for a custom game.
    pub const HOPPER: u32 = 1;
    /// The mean skill level of the session's players.
    pub const AVERAGE: u32 = 2;
    /// Public slots still open.
    pub const OPEN: u32 = 3;
    /// The lowest level the session counts as having: its lowest player's
    /// level, raised to the lowest level its highest player may play with.
    pub const LOWEST: u32 = 5;
    /// The session's highest player level.
    pub const HIGHEST: u32 = 6;
    /// How long the session has been up (title time units).
    pub const AGE: u32 = 7;
    /// Bit `n - 1` set: a party of `n` players fits.
    pub const ROOM: u32 = 8;
    /// `ROOM_FOR_1 + n - 1` is 1 when a party of `n` fits (n = 1..7); the
    /// last one, id 16, is 1 when a party of 8 or more fits.
    pub const ROOM_FOR_1: u32 = 9;
    pub const ROOM_FOR_8_OR_MORE: u32 = 16;
}

/// One decoded procedure-1 search.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Query {
    /// Parameter 0: the playlist, or -1 from the custom games browser.
    pub hopper: i64,
    /// Parameter 1: the searching party's mean level; results are ordered by
    /// how close a session's mean is to it.
    pub average: i64,
    /// Parameter 2: sessions whose `LOWEST` is below this are skipped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lowest: Option<i64>,
    /// Parameter 3: sessions whose `HIGHEST` is above this are skipped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub highest: Option<i64>,
    /// Parameters 4..11, one-hot: the searching party's size (8 means 8 or
    /// more). Sessions without room for it are skipped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub party: Option<u32>,
}

fn int(v: &AttributeValue) -> Option<i64> {
    match v {
        AttributeValue::Integer(v) => Some(*v as i64),
        _ => None,
    }
}

impl Query {
    /// Decodes the twelve positional parameters, or `None` when they are not
    /// procedure 1's shape (so the caller can fall back to no filter).
    pub fn from_params(params: &[AttributeValue]) -> Option<Query> {
        if params.len() != PARAMS {
            return None;
        }
        let optional = |v: &AttributeValue| match v {
            AttributeValue::Null => Some(None),
            v => int(v).map(Some),
        };
        let mut party = None;
        for (k, v) in params[4..].iter().enumerate() {
            match optional(v)? {
                None => {}
                Some(1) if party.is_none() => party = Some(k as u32 + 1),
                Some(_) => return None,
            }
        }
        Some(Query {
            hopper: int(&params[0])?,
            average: int(&params[1])?,
            lowest: optional(&params[2])?,
            highest: optional(&params[3])?,
            party,
        })
    }

    /// The twelve parameters this query was decoded from (for tests and
    /// tools; the game builds its own).
    pub fn to_params(&self) -> Vec<AttributeValue> {
        let i = |v: i64| AttributeValue::Integer(v as u64);
        let o = |v: Option<i64>| v.map_or(AttributeValue::Null, i);
        let mut out = vec![
            i(self.hopper),
            i(self.average),
            o(self.lowest),
            o(self.highest),
        ];
        for k in 1..=8 {
            out.push(if self.party == Some(k) {
                i(1)
            } else {
                AttributeValue::Null
            });
        }
        out
    }

    /// Whether a session advertising `attrs` satisfies the search.
    ///
    /// The hopper must match. A session with no hopper attribute (one the
    /// title did not advertise itself, such as a co-op lobby) only shows to
    /// the custom games browser's `-1`.
    pub fn matches(&self, attrs: &BTreeMap<u32, AttributeValue>) -> bool {
        let get = |id: u32| attrs.get(&id).and_then(int);
        match get(attr::HOPPER) {
            Some(h) if h != self.hopper => return false,
            None if self.hopper != -1 => return false,
            _ => {}
        }
        if let Some(lowest) = self.lowest
            && get(attr::LOWEST).is_none_or(|v| v < lowest)
        {
            return false;
        }
        if let Some(highest) = self.highest
            && get(attr::HIGHEST).is_none_or(|v| v > highest)
        {
            return false;
        }
        match self.party {
            None => true,
            Some(n) => room_for(attrs, n),
        }
    }

    /// How far a matching session's mean level is from the party's: lower is
    /// a better match. Sessions without a mean sort last.
    pub fn distance(&self, attrs: &BTreeMap<u32, AttributeValue>) -> u64 {
        attrs
            .get(&attr::AVERAGE)
            .and_then(int)
            .map_or(u64::MAX, |v| v.abs_diff(self.average))
    }
}

/// Whether a session has room for a party of `n` (8 meaning 8 or more).
pub fn room_for(attrs: &BTreeMap<u32, AttributeValue>, n: u32) -> bool {
    if n == 0 {
        return true;
    }
    if let Some(AttributeValue::Integer(bits)) = attrs.get(&attr::ROOM) {
        return if n >= 8 {
            bits & !0x7f != 0
        } else {
            bits & (1 << (n - 1)) != 0
        };
    }
    let id = attr::ROOM_FOR_1 + n.min(8) - 1;
    attrs.get(&id) == Some(&AttributeValue::Integer(1))
}

/// Matching sessions, best first: closest mean level, then the given order.
pub fn select<'a, T>(
    query: &Query,
    items: impl IntoIterator<Item = T>,
    attrs: impl Fn(&T) -> &'a BTreeMap<u32, AttributeValue>,
    limit: usize,
) -> Vec<T> {
    let mut out: Vec<(u64, usize, T)> = items
        .into_iter()
        .filter(|t| query.matches(attrs(t)))
        .enumerate()
        .map(|(i, t)| (query.distance(attrs(&t)), i, t))
        .collect();
    if query.hopper != -1 {
        out.sort_by_key(|(d, i, _)| (*d, *i));
    }
    out.into_iter().take(limit).map(|(_, _, t)| t).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn session(pairs: &[(u32, i64)]) -> BTreeMap<u32, AttributeValue> {
        pairs
            .iter()
            .map(|(k, v)| (*k, AttributeValue::Integer(*v as u64)))
            .collect()
    }
    fn ranked(lowest: i64, highest: i64, party: u32) -> Query {
        Query {
            hopper: 3,
            average: 10,
            lowest: Some(lowest),
            highest: Some(highest),
            party: Some(party),
        }
    }

    #[test]
    fn the_twelve_positional_params_round_trip() {
        let q = ranked(8, 14, 2);
        let params = q.to_params();
        assert_eq!(params.len(), PARAMS);
        assert_eq!(params[5], AttributeValue::Integer(1));
        assert_eq!(params[4], AttributeValue::Null);
        assert_eq!(Query::from_params(&params), Some(q));
        // The custom games browser: hopper -1, mean 0, nothing else.
        let mut browser = vec![
            AttributeValue::Integer(u64::MAX),
            AttributeValue::Integer(0),
        ];
        browser.extend(std::iter::repeat_n(AttributeValue::Null, 10));
        assert_eq!(
            Query::from_params(&browser),
            Some(Query {
                hopper: -1,
                average: 0,
                lowest: None,
                highest: None,
                party: None
            })
        );
        assert_eq!(Query::from_params(&params[..11]), None);
        let mut two_sizes = params.clone();
        two_sizes[6] = AttributeValue::Integer(1);
        assert_eq!(Query::from_params(&two_sizes), None);
    }

    #[test]
    fn sessions_match_on_hopper_level_window_and_room() {
        let q = ranked(8, 14, 2);
        // bits 0..2: room for 1, 2 or 3.
        let fits = session(&[(1, 3), (2, 11), (5, 9), (6, 13), (8, 0b111)]);
        assert!(q.matches(&fits));
        let other_playlist = session(&[(1, 4), (2, 11), (5, 9), (6, 13), (8, 0b111)]);
        assert!(!q.matches(&other_playlist));
        let too_low = session(&[(1, 3), (2, 9), (5, 7), (6, 13), (8, 0b111)]);
        assert!(!q.matches(&too_low));
        let too_high = session(&[(1, 3), (2, 12), (5, 9), (6, 15), (8, 0b111)]);
        assert!(!q.matches(&too_high));
        let one_seat = session(&[(1, 3), (2, 11), (5, 9), (6, 13), (8, 0b1)]);
        assert!(!q.matches(&one_seat));
        // The per-size flags stand in when the bitmask is absent.
        let flags = session(&[(1, 3), (5, 9), (6, 13), (10, 1)]);
        assert!(q.matches(&flags));
        // No window: an unranked search after it stops widening.
        let open = Query {
            lowest: None,
            highest: None,
            ..q
        };
        assert!(open.matches(&too_low) && open.matches(&too_high));
        // Eight or more.
        let big = Query {
            party: Some(8),
            ..open
        };
        assert!(big.matches(&session(&[(1, 3), (8, 0xff)])));
        assert!(!big.matches(&session(&[(1, 3), (8, 0x7f)])));
    }

    #[test]
    fn sessions_without_a_hopper_show_only_to_the_browser() {
        let coop = BTreeMap::new();
        assert!(!ranked(1, 50, 1).matches(&coop));
        let browser = Query {
            hopper: -1,
            average: 0,
            lowest: None,
            highest: None,
            party: None,
        };
        assert!(browser.matches(&coop));
        assert!(browser.matches(&session(&[(1, -1)])));
        assert!(!browser.matches(&session(&[(1, 3)])));
    }

    #[test]
    fn the_closest_mean_level_comes_first() {
        let q = Query {
            lowest: None,
            highest: None,
            ..ranked(1, 50, 1)
        };
        let s = [
            session(&[(1, 3), (2, 20), (8, 1)]),
            session(&[(1, 3), (2, 9), (8, 1)]),
            session(&[(1, 3), (2, 12), (8, 1)]),
            session(&[(1, 4), (2, 10), (8, 1)]),
        ];
        let got = select(&q, s.iter(), |t| t, 2);
        assert_eq!(got, vec![&s[1], &s[2]]);
    }
}
