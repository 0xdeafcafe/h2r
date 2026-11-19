//! `world.raw`: an object's state as raw guest bytes, for diffing one
//! moment against another. `world.poke` replaces bytes in one part
//! (docs/runtime/agent-api.md). Wire types and the diff `h2 agent raw-diff` prints.

use serde::{Deserialize, Serialize};

/// Method names. A game method: `session.methods` lists it where the game
/// registered it (`agent::raw::install`).
pub mod method {
    /// Raw bytes ([`super::RawParams`] → [`super::Raw`]).
    pub const RAW: &str = "world.raw";

    /// Replace bytes ([`super::PokeParams`] → [`super::Poke`]).
    pub const POKE: &str = "world.poke";

    pub const ALL: &[&str] = &[RAW, POKE];
}

/// The part names, in the order they are found and answered.
pub const PARTS: &[&str] = &["object", "physics_component", "entity", "motion", "shape"];

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawParams {
    /// An object id: `player.N` (its unit) or `kind.xxxxxxxx`.
    pub id: String,
    /// Which parts ([`PARTS`]); all of them by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parts: Option<Vec<String>>,
    /// Bytes of the object when its header gives no size; default `0x400`,
    /// at most `0x2000`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub len: Option<u32>,
}

/// One part: its bytes, or why there are none.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RawPart {
    /// One of [`PARTS`].
    pub part: String,
    /// The guest address of the first byte, 8 hex digits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    /// The bytes as lowercase hex, two digits each.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hex: Option<String>,
    /// Why the part was left out (an unset pointer, a fault).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub missing: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Raw {
    pub id: String,
    /// The object datum handle, 8 hex digits.
    pub datum: String,
    pub parts: Vec<RawPart>,
}

/// Maximum bytes per `world.poke` call.
pub const POKE_MAX: usize = 64;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PokeParams {
    /// An object id: `player.N` (its unit) or `kind.xxxxxxxx`.
    pub id: String,
    /// One of [`PARTS`]; defaults to `object`.
    #[serde(default = "object_part")]
    pub part: String,
    /// Byte offset from the part's base.
    pub offset: u32,
    /// Replacement bytes as hex, two digits each.
    pub hex: String,
}

fn object_part() -> String {
    "object".to_owned()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Poke {
    pub id: String,
    /// The object datum handle, 8 hex digits.
    pub datum: String,
    pub part: String,
    /// The guest address of the part's first byte, 8 hex digits.
    pub base: String,
    pub offset: u32,
    /// The bytes at the offset, before and after the write, in lowercase hex.
    pub before: String,
    pub after: String,
}

/// Lowercase hex, two digits a byte.
pub fn to_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// The bytes of [`to_hex`]'s text; `None` when it is not hex.
pub fn from_hex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

/// A 32-bit word that differs between two reads of a part.
#[derive(Debug, Clone, PartialEq)]
pub struct WordDiff {
    /// Offset from the part's base.
    pub offset: u32,
    pub before: u32,
    pub after: u32,
}

/// One part's differences.
#[derive(Debug, Clone, PartialEq)]
pub struct PartDiff {
    pub part: String,
    /// Each side's base and length, or why it is missing.
    pub before: Result<(u32, usize), String>,
    pub after: Result<(u32, usize), String>,
    /// Words that differ, over the bytes both sides have.
    pub words: Vec<WordDiff>,
}

fn side(p: Option<&RawPart>) -> Result<(u32, Vec<u8>), String> {
    let p = p.ok_or("not asked for")?;
    if let Some(m) = &p.missing {
        return Err(m.clone());
    }
    let base = p
        .base
        .as_deref()
        .and_then(|b| u32::from_str_radix(b, 16).ok())
        .ok_or("no base")?;
    let bytes = p.hex.as_deref().and_then(from_hex).ok_or("no bytes")?;
    Ok((base, bytes))
}

fn word(b: &[u8], i: usize) -> u32 {
    let mut w = [0u8; 4];
    for (k, x) in w.iter_mut().enumerate() {
        *x = b.get(i + k).copied().unwrap_or(0);
    }
    u32::from_le_bytes(w)
}

/// The words that differ between two answers, part by part in
/// [`PARTS`]'s order (then any other part names). A part on one side only
/// is listed with no words; so is one whose base moved, since its bytes
/// are then another structure's.
pub fn diff(before: &Raw, after: &Raw) -> Vec<PartDiff> {
    let mut names: Vec<&str> = PARTS.to_vec();
    for p in before.parts.iter().chain(&after.parts) {
        if !names.contains(&p.part.as_str()) {
            names.push(&p.part);
        }
    }
    let find = |r: &Raw, n: &str| r.parts.iter().find(|p| p.part == n).cloned();
    let mut out = Vec::new();
    for n in names {
        let (a, b) = (find(before, n), find(after, n));
        if a.is_none() && b.is_none() {
            continue;
        }
        let (a, b) = (side(a.as_ref()), side(b.as_ref()));
        let mut words = Vec::new();
        if let (Ok((ba, x)), Ok((bb, y))) = (&a, &b)
            && ba == bb
        {
            let len = x.len().min(y.len());
            for i in (0..len).step_by(4) {
                let (p, q) = (word(&x[..len], i), word(&y[..len], i));
                if p != q {
                    words.push(WordDiff {
                        offset: i as u32,
                        before: p,
                        after: q,
                    });
                }
            }
        }
        out.push(PartDiff {
            part: n.to_owned(),
            before: a.map(|(base, x)| (base, x.len())),
            after: b.map(|(base, y)| (base, y.len())),
            words,
        });
    }
    out
}

/// A word as a float, when it reads as one a person would write: finite,
/// zero or between 1e-6 and 1e7 in size.
pub fn plausible_f32(w: u32) -> Option<f32> {
    let f = f32::from_bits(w);
    let a = f.abs();
    (f.is_finite() && (a == 0.0 || (1e-6..1e7).contains(&a))).then_some(f)
}

/// [`diff`] as text: per part a header line, then one line per differing
/// word (`+offset  before -> after`, with both as floats when both are
/// plausible ones).
pub fn render(diffs: &[PartDiff]) -> String {
    let mut s = String::new();
    let side = |r: &Result<(u32, usize), String>| match r {
        Ok((base, len)) => format!("{base:08x} ({len:#x} bytes)"),
        Err(e) => format!("missing: {e}"),
    };
    for d in diffs {
        let same_base = matches!((&d.before, &d.after), (Ok((a, _)), Ok((b, _))) if a == b);
        let head = if same_base {
            side(&d.before)
        } else {
            format!("{} -> {}", side(&d.before), side(&d.after))
        };
        let count = match d.words.len() {
            0 if same_base => "no change".to_owned(),
            0 => "not compared".to_owned(),
            1 => "1 word differs".to_owned(),
            n => format!("{n} words differ"),
        };
        s.push_str(&format!("{}  {head}  {count}\n", d.part));
        for w in &d.words {
            s.push_str(&format!(
                "  +{:#05x}  {:08x} -> {:08x}",
                w.offset, w.before, w.after
            ));
            if let (Some(a), Some(b)) = (plausible_f32(w.before), plausible_f32(w.after)) {
                s.push_str(&format!("   {a} -> {b}"));
            }
            s.push('\n');
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(name: &str, base: u32, bytes: &[u8]) -> RawPart {
        RawPart {
            part: name.into(),
            base: Some(format!("{base:08x}")),
            hex: Some(to_hex(bytes)),
            missing: None,
        }
    }

    fn raw(parts: Vec<RawPart>) -> Raw {
        Raw {
            id: "player.0".into(),
            datum: "e0010001".into(),
            parts,
        }
    }

    fn words(w: &[u32]) -> Vec<u8> {
        w.iter().flat_map(|x| x.to_le_bytes()).collect()
    }

    #[test]
    fn hex_round_trips_and_refuses_junk() {
        let b = [0u8, 0x7f, 0xff, 0x10];
        assert_eq!(to_hex(&b), "007fff10");
        assert_eq!(from_hex("007fff10").unwrap(), b);
        assert_eq!(from_hex("0"), None);
        assert_eq!(from_hex("zz"), None);
        assert_eq!(from_hex("+f"), None);
        assert_eq!(from_hex("é0"), None);
    }

    #[test]
    fn diff_lists_the_words_that_changed_per_part() {
        let before = raw(vec![
            part("object", 0x1000, &words(&[1, 0.725f32.to_bits(), 3])),
            part("shape", 0x2000, &words(&[0x0045_5500, 7])),
            RawPart {
                part: "motion".into(),
                missing: Some("no motion".into()),
                ..Default::default()
            },
        ]);
        let after = raw(vec![
            part("object", 0x1000, &words(&[1, 0.5f32.to_bits(), 4])),
            // The shape is another one: not compared.
            part("shape", 0x3000, &words(&[0x0045_5500, 7])),
            part("motion", 0x4000, &words(&[0])),
        ]);
        let d = diff(&before, &after);
        let names: Vec<&str> = d.iter().map(|p| p.part.as_str()).collect();
        assert_eq!(names, ["object", "motion", "shape"]);
        assert_eq!(
            d[0].words,
            [
                WordDiff {
                    offset: 4,
                    before: 0.725f32.to_bits(),
                    after: 0.5f32.to_bits(),
                },
                WordDiff {
                    offset: 8,
                    before: 3,
                    after: 4,
                },
            ]
        );
        assert_eq!(d[1].before, Err("no motion".into()));
        assert_eq!(d[1].after, Ok((0x4000, 4)));
        assert!(d[1].words.is_empty());
        assert_eq!(d[2].before, Ok((0x2000, 8)));
        assert!(d[2].words.is_empty());

        let text = render(&d);
        assert!(
            text.contains("object  00001000 (0xc bytes)  2 words differ"),
            "{text}"
        );
        assert!(
            text.contains("  +0x004  3f39999a -> 3f000000   0.725 -> 0.5"),
            "{text}"
        );
        // 3 and 4 are denormals as floats: words only.
        assert!(text.contains("  +0x008  00000003 -> 00000004\n"), "{text}");
        assert!(
            text.contains("motion  missing: no motion -> 00004000"),
            "{text}"
        );
        assert!(
            text.contains("shape  00002000 (0x8 bytes) -> 00003000"),
            "{text}"
        );
        assert!(text.contains("not compared"), "{text}");
    }

    #[test]
    fn identical_reads_say_no_change_and_ragged_tails_compare() {
        let a = raw(vec![part("entity", 0x10, &[1, 2, 3, 4, 5, 6])]);
        let b = raw(vec![part("entity", 0x10, &[1, 2, 3, 4, 5, 7])]);
        let d = diff(&a, &a);
        assert!(d[0].words.is_empty());
        assert!(render(&d).contains("entity  00000010 (0x6 bytes)  no change"));
        let d = diff(&a, &b);
        assert_eq!(d[0].words.len(), 1);
        assert_eq!(d[0].words[0].offset, 4);
        assert_eq!(
            (d[0].words[0].before, d[0].words[0].after),
            (0x0605, 0x0705)
        );
    }
}
