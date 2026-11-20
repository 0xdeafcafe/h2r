//! Tiered optimisation: sources that hold none of a profile's hot guest
//! code compile at a cheaper `-O` level.
//!
//! Clang's time on a shard is mostly the optimiser and the register
//! allocator, and both grow faster than linearly with a function's size
//! (docs/lifter/notes/build-perf.md). `-O0` compiles a shard about eight times
//! faster than `-O2`, `-O1` about a sixth faster. The game does the same
//! thing at any level; only how fast cold code runs changes.
//!
//! On by default for an optimised build: with `H2_OPT` at `2` or `3` and
//! `H2_OPT_COLD` unset, the cold level is `1` and the hot list is
//! [`HOT_LIST`], the repository's list of profiled guest functions. With
//! `H2_OPT` at `1` or below there is no cheaper level worth its cost by
//! default, and tiers are off. `H2_OPT_COLD` names another cold level, or
//! `off`; `H2_HOT` a file naming the hot guest addresses instead of
//! [`HOT_LIST`].
//!
//! A source is hot when it holds the instruction marker (`/* 0x0001234 */`,
//! which `c.rs` writes before every guest instruction) of any hot address.
//! Every other source, the registry included, is cold. A profile's
//! `profile.folded` or `profile.txt` (`guest 0x…`, `@0x…`) works as the
//! list as it is, and so does a hand-written list of addresses or
//! `h2fn_` names.

use super::{Error, err};
use crate::fnv64;

/// The default hot list: guest function entries from the profiles in
/// docs/perf, one per line (`hot.txt`, which says how to replace it).
pub const HOT_LIST: &str = include_str!("hot.txt");

/// The cold level when `H2_OPT` is `2` or `3` and `H2_OPT_COLD` is unset:
/// `-O1` costs 84% of `-O2`'s clang time, and leaves cold code about a
/// sixth more instructions (notes/build-perf.md, docs/runtime/xbox.md).
pub const DEFAULT_COLD: &str = "1";

/// Which sources compile at which level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tiers {
    /// The cold sources' `H2_OPT` level: `0`, `1`, `g`, `s` or `2`.
    pub cold: String,
    /// Hot guest addresses, sorted and unique.
    pub hot: Vec<u32>,
}

impl Tiers {
    /// From `H2_OPT_COLD`, `H2_HOT` and `H2_OPT` ([`Tiers::from_settings`]).
    pub fn from_env() -> Result<Option<Tiers>, Error> {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let hot = std::env::var_os("H2_HOT").filter(|v| !v.is_empty());
        let list = match &hot {
            Some(path) => {
                let text = std::fs::read(path)
                    .map_err(|e| err(format!("H2_HOT={}: {e}", path.to_string_lossy())))?;
                Some(String::from_utf8_lossy(&text).into_owned())
            }
            None => None,
        };
        Tiers::from_settings(var("H2_OPT_COLD").as_deref(), list.as_deref(), var("H2_OPT").as_deref())
    }

    /// The tiers for `H2_OPT_COLD` = `cold`, a hot list `list` (the text of
    /// `H2_HOT`'s file; `None` for [`HOT_LIST`]) and `H2_OPT` = `opt`.
    /// `None` (every source at `H2_OPT`) when `cold` is `off`, or unset with
    /// `opt` below `2`. A hot list given with tiers off is an error: it
    /// would do nothing.
    pub fn from_settings(cold: Option<&str>, list: Option<&str>, opt: Option<&str>) -> Result<Option<Tiers>, Error> {
        let cold = match cold {
            Some("off") => {
                if list.is_some() {
                    return Err(err("H2_HOT is set with H2_OPT_COLD=off: unset one"));
                }
                return Ok(None);
            }
            Some(level) => level,
            None => match opt.unwrap_or("1") {
                "2" | "3" => DEFAULT_COLD,
                level => {
                    if list.is_some() {
                        return Err(err(format!(
                            "H2_HOT is set but H2_OPT={level} has no default cold level: set H2_OPT_COLD, or H2_OPT=2"
                        )));
                    }
                    return Ok(None);
                }
            },
        };
        Tiers::new(cold, list.unwrap_or(HOT_LIST)).map(Some)
    }

    /// Cold sources at `cold`, hot ones holding an address in `list` (see
    /// [`hot_addresses`]).
    pub fn new(cold: &str, list: &str) -> Result<Tiers, Error> {
        if !["0", "1", "g", "s", "2"].contains(&cold) {
            return Err(err(format!("H2_OPT_COLD={cold}: expected 0, 1, g, s, 2 or off")));
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
    fn tiers_are_on_by_default_for_an_optimised_build_only() {
        // H2_OPT 2 or 3: the shipped list, cold at -O1.
        for opt in ["2", "3"] {
            let t = Tiers::from_settings(None, None, Some(opt)).unwrap().unwrap();
            assert_eq!(t.cold, DEFAULT_COLD);
            assert_eq!(t.hot, hot_addresses(HOT_LIST));
        }
        // The default level and below: off.
        for opt in [None, Some("1"), Some("0"), Some("g"), Some("s")] {
            assert_eq!(Tiers::from_settings(None, None, opt).unwrap(), None, "{opt:?}");
        }
        // Off, or another level, on request; another list replaces the
        // shipped one.
        assert_eq!(Tiers::from_settings(Some("off"), None, Some("2")).unwrap(), None);
        let t = Tiers::from_settings(Some("0"), Some("0x1234"), None).unwrap().unwrap();
        assert_eq!((t.cold.as_str(), t.hot.as_slice()), ("0", &[0x1234][..]));
        let t = Tiers::from_settings(None, Some("0x1234"), Some("2")).unwrap().unwrap();
        assert_eq!((t.cold.as_str(), t.hot.as_slice()), (DEFAULT_COLD, &[0x1234][..]));
        // A list that would do nothing is an error, not ignored.
        assert!(Tiers::from_settings(Some("off"), Some("0x1234"), Some("2")).is_err());
        assert!(Tiers::from_settings(None, Some("0x1234"), Some("1")).is_err());
    }

    #[test]
    fn the_shipped_hot_list_names_only_addresses_in_its_lines() {
        let hot = hot_addresses(HOT_LIST);
        let lines: Vec<&str> = HOT_LIST.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()).collect();
        // Every address is on a line of its own; the comments name none.
        assert_eq!(hot.len(), lines.len(), "{lines:?}");
        assert!(hot.len() > 50);
        assert!(hot.iter().all(|&a| (0x1_0000..0x80_0000).contains(&a)), "{hot:x?}");
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
