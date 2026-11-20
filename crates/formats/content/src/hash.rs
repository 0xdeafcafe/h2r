//! SHA-256 digests as lowercase hex, the form every manifest and `MapId` uses.
use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::Digest;

/// A SHA-256 digest. Parses from 64 hex digits in either case; displays and
/// serialises as lowercase hex.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Sha256(pub [u8; 32]);

/// Not 64 hex digits.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a SHA-256 hex digest (want 64 hex digits): {0:?}")]
pub struct BadHex(pub String);

impl Sha256 {
    /// The digest of `bytes`.
    pub fn of(bytes: &[u8]) -> Self {
        Self(sha2::Sha256::digest(bytes).into())
    }
}

pub(crate) fn parse_hex32(s: &str) -> Option<[u8; 32]> {
    let b = s.as_bytes();
    if b.len() != 64 {
        return None;
    }
    let mut out = [0; 32];
    for (i, pair) in b.as_chunks::<2>().0.iter().enumerate() {
        let hi = (pair[0] as char).to_digit(16)?;
        let lo = (pair[1] as char).to_digit(16)?;
        out[i] = (hi * 16 + lo) as u8;
    }
    Some(out)
}

pub(crate) fn write_hex(f: &mut fmt::Formatter<'_>, bytes: &[u8]) -> fmt::Result {
    for b in bytes {
        write!(f, "{b:02x}")?;
    }
    Ok(())
}

impl FromStr for Sha256 {
    type Err = BadHex;
    fn from_str(s: &str) -> Result<Self, BadHex> {
        parse_hex32(s).map(Self).ok_or_else(|| BadHex(s.to_owned()))
    }
}
impl fmt::Display for Sha256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_hex(f, &self.0)
    }
}
impl fmt::Debug for Sha256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Sha256({self})")
    }
}
impl Serialize for Sha256 {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}
impl<'de> Deserialize<'de> for Sha256 {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// Incremental SHA-256.
#[derive(Default, Clone)]
pub struct Hasher(sha2::Sha256);
impl Hasher {
    pub fn update(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }
    pub fn finish(self) -> Sha256 {
        Sha256(self.0.finalize().into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trip_and_case() {
        let h = Sha256::of(b"abc");
        let s = h.to_string();
        assert_eq!(
            s,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(s.parse::<Sha256>().unwrap(), h);
        assert_eq!(s.to_uppercase().parse::<Sha256>().unwrap(), h);
        for bad in ["", "ab", &s[1..], &format!("{s}0"), &s.replace('b', "g")] {
            assert!(bad.parse::<Sha256>().is_err(), "{bad}");
        }
        assert_eq!(serde_json::to_string(&h).unwrap(), format!("\"{s}\""));
        let mut inc = Hasher::default();
        inc.update(b"a");
        inc.update(b"bc");
        assert_eq!(inc.finish(), h);
    }
}
