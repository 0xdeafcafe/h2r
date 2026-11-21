//! A self-contained game variant: the `.h2variant` format 2 shape
//! (`variants::portable` format 2: name, description, author, engine, the
//! 0x130-byte record with its `hx` block, and the script it names with its
//! IR), as the store reads and writes it.
//!
//! This is the store's own reader and writer of that shape; when
//! `variants::portable` gains format 2 this module becomes a thin wrapper
//! over it. A Live variant's bundle may lack the IR (`ir_b64` absent) until
//! the store fetches it: its script is then named by hash only.

use content::Sha256;
use serde::{Deserialize, Serialize};
use variants::ext::Mode;
use variants::record::{GameVariant, RECORD_SIZE};

/// The format number.
pub const FORMAT: u32 = 2;
/// The file extension.
pub const EXTENSION: &str = "h2variant";
/// The largest IR a bundle carries (docs/game/rules-engine.md, "Limits").
pub const MAX_IR: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Author {
    pub xuid: String,
    pub gamertag: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleScript {
    pub name: String,
    /// The IR's SHA-256, 64 lowercase hex digits.
    pub hash: String,
    /// The IR, standard base64. Absent for a Live variant whose IR is not
    /// fetched yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ir_b64: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bundle {
    pub format: u32,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<Author>,
    pub engine: i32,
    pub record_hex: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script: Option<BundleScript>,
    /// The Live variant's id, when it came from Live.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_id: Option<String>,
}

/// A bundle that checked out: its record, and its script's hash and IR.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checked {
    pub record: [u8; RECORD_SIZE],
    /// The script's full hash, when the bundle names one.
    pub hash: Option<Sha256>,
    /// The IR, when the bundle carries it (its SHA-256 is `hash`).
    pub ir: Option<Vec<u8>>,
}

impl Bundle {
    /// A bundle of `record` with no script.
    pub fn new(name: &str, description: &str, record: &[u8; RECORD_SIZE]) -> Self {
        let engine = GameVariant::from_bytes(record).map_or(0, |v| v.game_type().engine());
        Bundle {
            format: FORMAT,
            name: name.to_owned(),
            description: description.to_owned(),
            author: None,
            engine,
            record_hex: hex(record),
            script: None,
            live_id: None,
        }
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let b: Bundle =
            serde_json::from_str(text).map_err(|e| format!("not a game variant file: {e}"))?;
        if b.format != FORMAT {
            return Err(format!("game variant format {} is not {FORMAT}", b.format));
        }
        Ok(b)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("plain data serializes")
    }

    /// The bundle's identity: the first 16 hex digits of the record's
    /// SHA-256, so a Live variant and a file of the same record are one.
    pub fn id(&self) -> String {
        let s = Sha256::of(self.record_hex.trim().to_ascii_lowercase().as_bytes()).to_string();
        s[..16].to_owned()
    }

    /// Checks the bundle: a 0x130-byte record of its engine; a script hash
    /// of 64 hex digits whose first 8 bytes the record's `hx` block names;
    /// an IR, when carried, of at most [`MAX_IR`] bytes hashing to it.
    pub fn check(&self) -> Result<Checked, String> {
        let record = unhex(self.record_hex.trim())
            .filter(|r| r.len() == RECORD_SIZE)
            .ok_or_else(|| format!("the record is not {RECORD_SIZE:#x} bytes of hex"))?;
        let record: [u8; RECORD_SIZE] = record.try_into().expect("length checked");
        let v = GameVariant::from_bytes(&record).map_err(|e| format!("the record: {e}"))?;
        let engine = v.game_type().engine();
        if engine != self.engine {
            return Err(format!(
                "the record is engine {engine}, and the file says {}",
                self.engine
            ));
        }
        let named = v.ext().script;
        let Some(s) = &self.script else {
            if named.mode == Mode::Rules {
                return Err("the record names a script, and the file carries none".into());
            }
            return Ok(Checked {
                record,
                hash: None,
                ir: None,
            });
        };
        let hash: Sha256 = s
            .hash
            .parse()
            .map_err(|_| format!("the script hash {:?} is not 64 hex digits", s.hash))?;
        if named.mode != Mode::Rules || named.prefix != hash.0[..8] {
            return Err(format!(
                "the record does not name the script {hash} (its block names {})",
                hex(&named.prefix)
            ));
        }
        let ir = match &s.ir_b64 {
            None => None,
            Some(b) => {
                let ir = base64_decode(b).ok_or("the script's IR is not base64")?;
                if ir.len() > MAX_IR {
                    return Err(format!(
                        "the script's IR is {} bytes, over {MAX_IR}",
                        ir.len()
                    ));
                }
                let got = Sha256::of(&ir);
                if got != hash {
                    return Err(format!("the script's IR hashes to {got}, not {hash}"));
                }
                Some(ir)
            }
        };
        Ok(Checked {
            record,
            hash: Some(hash),
            ir,
        })
    }

    /// A Live variant as a bundle (its IR not yet carried).
    pub fn from_live(v: &live_hle::game_variants::LiveVariant) -> Self {
        Bundle {
            format: FORMAT,
            name: v.name.clone(),
            description: v.description.clone(),
            author: Some(Author {
                xuid: v.author.xuid.clone(),
                gamertag: v.author.gamertag.clone(),
            }),
            engine: v.engine,
            record_hex: v.record_hex.trim().to_ascii_lowercase(),
            script: v.script_hash.as_ref().map(|h| BundleScript {
                name: v.script_name.clone().unwrap_or_default(),
                hash: h.clone(),
                ir_b64: None,
                source: None,
            }),
            live_id: Some(v.id.clone()),
        }
    }

    /// The bundle with `ir` carried (its hash already checked).
    pub fn with_ir(mut self, ir: &[u8]) -> Self {
        if let Some(s) = &mut self.script {
            s.ir_b64 = Some(base64_encode(ir));
        }
        self
    }
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 || !s.is_ascii() {
        return None;
    }
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok())
        .collect()
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding.
pub fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(B64[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Standard base64, padding optional; whitespace is skipped.
pub fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0u32);
    let mut ended = false;
    for c in s.bytes() {
        if c.is_ascii_whitespace() {
            continue;
        }
        if c == b'=' {
            ended = true;
            continue;
        }
        if ended {
            return None;
        }
        let v = B64.iter().position(|&x| x == c)? as u32;
        acc = acc << 6 | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips() {
        for n in 0..10usize {
            let bytes: Vec<u8> = (0..n as u8).map(|i| i.wrapping_mul(37)).collect();
            let s = base64_encode(&bytes);
            assert_eq!(s.len() % 4, 0);
            assert_eq!(base64_decode(&s).unwrap(), bytes, "{s}");
        }
        assert_eq!(base64_encode(b"Man"), "TWFu");
        assert_eq!(base64_encode(b"Ma"), "TWE=");
        assert_eq!(base64_decode("TWE").unwrap(), b"Ma");
        assert_eq!(base64_decode("T!=="), None);
    }
}
