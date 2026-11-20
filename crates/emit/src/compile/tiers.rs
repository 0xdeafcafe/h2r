//! Tiered optimisation, off unless asked for: sources that hold none of a
//! profile's hot guest code compile at a cheaper `-O` level.
//!
//! Clang's time on a shard is mostly the optimiser and the register
//! allocator, and both grow faster than linearly with a function's size
//! (docs/lifter/notes/build-perf.md). `-O0` compiles a shard about eight times
//! faster than `-O2`, `-O1` about a sixth faster. The game does the same
//! thing at any level; only how fast cold code runs changes, so this stays
//! off by default and is a measured choice: `H2_OPT_COLD` names the cold
//! level and `H2_HOT` a file naming hot guest addresses.
//!
//! A source is hot when it holds the instruction marker (`/* 0x0001234 */`,
//! which `c.rs` writes before every guest instruction) of any hot address.
//! Every other source, the registry included, is cold. A profile's
//! `profile.folded` or `profile.txt` (`guest 0x…`, `@0x…`) works as the
//! list as it is, and so does a hand-written list of addresses or
//! `h2fn_` names.

use super::{Error, err};
use crate::fnv64;

/// Which sources compile at which level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tiers {
    /// The cold sources' `H2_OPT` level: `0`, `1`, `g`, `s` or `2`.
    pub cold: String,
    /// Hot guest addresses, sorted and unique.
    pub hot: Vec<u32>,
}

impl Tiers {
    /// From `H2_OPT_COLD` and `H2_HOT`. `None` when `H2_OPT_COLD` is unset
    /// or empty (the default: every source at `H2_OPT`).
    pub fn from_env() -> Result<Option<Tiers>, Error> {
        let cold = std::env::var("H2_OPT_COLD").ok().filter(|v| !v.is_empty());
        let hot = std::env::var_os("H2_HOT").filter(|v| !v.is_empty());
        let Some(cold) = cold else {
            if hot.is_some() {
                return Err(err(
                    "H2_HOT is set without H2_OPT_COLD: set both, or neither",
                ));
            }
            return Ok(None);
        };
        let hot = hot.ok_or_else(|| {
            err("H2_OPT_COLD needs H2_HOT, a file naming the hot guest addresses")
        })?;
        let text = std::fs::read(&hot)
            .map_err(|e| err(format!("H2_HOT={}: {e}", hot.to_string_lossy())))?;
        Tiers::new(&cold, &String::from_utf8_lossy(&text)).map(Some)
    }

    /// Cold sources at `cold`, hot ones holding an address in `list` (see
    /// [`hot_addresses`]).
    pub fn new(cold: &str, list: &str) -> Result<Tiers, Error> {
        if !["0", "1", "g", "s", "2"].contains(&cold) {
            return Err(err(format!("H2_OPT_COLD={cold}: expected 0, 1, g, s or 2")));
        }
        let hot = hot_addresses(list);
        if hot.is_empty() {
            return Err(err(
                "H2_HOT names no guest address (0x… or h2fn_…): every source would be cold",
            ));
        }
        Ok(Tiers {
            cold: cold.to_owned(),
            hot,
        })
    }

    /// `flags` (from [`super::cflags`]) with the cold level in place of
    /// `-O`.
    pub fn cold_flags(&self, flags: &[String]) -> Vec<String> {
        flags
            .iter()
            .map(|f| {
                if f.starts_with("-O") {
                    format!("-O{}", self.cold)
                } else {
                    f.clone()
                }
            })
            .collect()
    }

    /// Whether `source` holds a hot guest instruction.
    pub fn is_hot(&self, source: &[u8]) -> bool {
        markers(source).any(|pc| self.hot.binary_search(&pc).is_ok())
    }

    /// What decides the tiers, for a build stamp: the cold level and the
    /// hot list.
    pub fn spec(&self) -> String {
        let bytes: Vec<u8> = self.hot.iter().flat_map(|a| a.to_le_bytes()).collect();
        format!(
            "cold -O{} hot {} {:016x}",
            self.cold,
            self.hot.len(),
            fnv64(&bytes)
        )
    }
}

/// Every guest address `text` names: `0x` and one to eight hex digits, or
/// `h2fn_` and eight, anywhere (so a folded profile's `guest 0x00012340`
/// and `@0x00012345` frames both count), sorted and unique.
pub fn hot_addresses(text: &str) -> Vec<u32> {
    let b = text.as_bytes();
    let mut v = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let digits = if b[i..].starts_with(b"0x") || b[i..].starts_with(b"0X") {
            Some(i + 2)
        } else if b[i..].starts_with(b"h2fn_") {
            Some(i + 5)
        } else {
            None
        };
        if let Some(start) = digits {
            let preceded =
                i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_') && b[i] == b'0';
            let end = start
                + b[start..]
                    .iter()
                    .take_while(|c| c.is_ascii_hexdigit())
                    .count();
            let run = end - start;
            if !preceded && (1..=8).contains(&run) {
                v.push(u32::from_str_radix(&text[start..end], 16).unwrap());
            }
            i = end.max(i + 1);
        } else {
            i += 1;
        }
    }
    v.sort_unstable();
    v.dedup();
    v
}

/// The guest instruction markers in generated C: `/* 0x` eight hex digits
/// ` */`.
fn markers(source: &[u8]) -> impl Iterator<Item = u32> + '_ {
    const OPEN: &[u8] = b"/* 0x";
    let mut at = 0;
    std::iter::from_fn(move || {
        while let Some(off) = find(&source[at..], OPEN) {
            let start = at + off + OPEN.len();
            at = start;
            let hex = source.get(start..start + 8)?;
            if hex.iter().all(u8::is_ascii_hexdigit) && source[start + 8..].starts_with(b" */") {
                return u32::from_str_radix(std::str::from_utf8(hex).unwrap(), 16).ok();
            }
        }
        None
    })
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    let first = needle[0];
    let mut i = 0;
    while let Some(p) = hay[i..].iter().position(|&c| c == first) {
        let at = i + p;
        if hay[at..].starts_with(needle) {
            return Some(at);
        }
        i = at + 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hot_lists_come_from_profiles_or_plain_addresses() {
        let folded = "game;main;guest 0x00073100;@0x0007311a 5\nother;guest 0x0006b640 2\n";
        assert_eq!(hot_addresses(folded), [0x6b640, 0x73100, 0x7311a]);
        assert_eq!(
            hot_addresses("h2fn_0006cd50\n0x10\n# 0x10 again\n"),
            [0x10, 0x6cd50]
        );
        // Not addresses: longer than 32 bits, or the tail of a word.
        assert!(hot_addresses("0x123456789 abc0x12 fn_00001234").is_empty());
    }

    #[test]
    fn a_source_is_hot_only_when_it_holds_a_hot_instruction() {
        let t = Tiers::new("0", "guest 0x00073100").unwrap();
        let src = b"L_00073100:;\n    /* 0x00073100 */ {\n    }\n";
        assert!(t.is_hot(src));
        // A call to it, a prototype or a resume selector is not its code.
        let caller = b"TransferAbi h2fn_00073100(CpuAbi *, ExecCtxOpaque *, uint32_t);\n    case 0x00073100u: goto L;\n    /* 0x00073105 */ {\n";
        assert!(!t.is_hot(caller));
        assert!(!t.is_hot(b"/* 0x0007310 */ /* 0x"));
    }

    #[test]
    fn cold_flags_change_only_the_level_and_are_validated() {
        let t = Tiers::new("1", "0x1").unwrap();
        let flags: Vec<String> = ["-std=c11", "-O2", "-gline-tables-only"]
            .map(String::from)
            .into();
        assert_eq!(
            t.cold_flags(&flags),
            ["-std=c11", "-O1", "-gline-tables-only"]
        );
        assert!(Tiers::new("3", "0x1").is_err());
        assert!(Tiers::new("0", "no addresses here").is_err());
        assert_ne!(
            Tiers::new("0", "0x1").unwrap().spec(),
            Tiers::new("0", "0x2").unwrap().spec()
        );
        assert_ne!(
            Tiers::new("0", "0x1").unwrap().spec(),
            Tiers::new("1", "0x1").unwrap().spec()
        );
    }
}
