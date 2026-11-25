//! The game start of a film (docs/game/film-format.md, "The film file";
//! docs/game/theater-design.md, "The start state"): what a game started
//! from, and nothing of the machine that recorded it. The player loads the
//! map with [`GameStart::options`] and applies the film's updates.
//!
//! The encoding is a `u32` section count and that many sections, each a
//! four-byte tag, a `u32` length and the bytes, all little-endian, stored in
//! the file as one zstd frame. A reader refuses a missing required section,
//! a repeated or unknown one, a length past the frame, bytes after the last
//! section, and an options block of another size.
//!
//! | Tag | Holds |
//! |---|---|
//! | `STMP` | JSON [`Stamp`] |
//! | `GAME` | JSON [`Game`] |
//! | `OPTS` | the game options block, [`OPTIONS_BYTES`] bytes |
//! | `RAND` | the two shared random words, `u32` each |
//! | `HASH` | the arena hash at entry to the first update, `u64` |
//! | `SETS` | JSON [`Settings`] |
//! | `VARI` | the rules script of the variant, when it has one |
//! | `MAPV` | the Forge map variant, when the game has one |
//! | `HDET` | what the start hash is made of ([`HashDetail`]), to name what differs when a start does not reproduce |
use crate::blam::MatchHeader;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::collections::BTreeMap;

/// The retail game options block (`level_boot::OPTIONS_SIZE`).
pub const OPTIONS_BYTES: usize = 0x1118;
/// The most a game start inflates to: a Forge map variant is the largest
/// section by far.
pub const MAX_INFLATED: usize = 16 << 20;
const MAX_SECTIONS: u32 = 16;
const MAX_JSON: usize = 1 << 20;

pub const STMP: [u8; 4] = *b"STMP";
pub const GAME: [u8; 4] = *b"GAME";
pub const OPTS: [u8; 4] = *b"OPTS";
pub const RAND: [u8; 4] = *b"RAND";
pub const HASH: [u8; 4] = *b"HASH";
pub const SETS: [u8; 4] = *b"SETS";
pub const VARI: [u8; 4] = *b"VARI";
pub const MAPV: [u8; 4] = *b"MAPV";
pub const HDET: [u8; 4] = *b"HDET";

const REQUIRED: [[u8; 4]; 6] = [STMP, GAME, OPTS, RAND, HASH, SETS];
const ALL: [[u8; 4]; 9] = [STMP, GAME, OPTS, RAND, HASH, SETS, VARI, MAPV, HDET];
/// The arena is hashed in pages of this many bytes for [`HashDetail`].
pub const DETAIL_PAGE: usize = 4096;

/// What decides whether a build can play the film: the recording's XBE, its
/// simulation contract and its hook digest.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stamp {
    pub xbe_sha256: String,
    /// `theater::compatibility::SIMULATION_ABI` at the recording.
    pub simulation_abi: String,
    /// The guest image's contract digest, when the image has one.
    pub contract_sha256: Option<String>,
    /// The installed simulation hooks' digest (`theater::hook_digest`).
    pub hooks_sha256: Option<String>,
    /// The recording's image stamp, which names the build.
    pub build: String,
}

/// The game, as the selector and the player read it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Game {
    pub started_ms: u64,
    /// The film tick (1 is the first the file holds) of the game's first
    /// update.
    pub first_update: u64,
    /// The engine's tick count at that update.
    pub first_game_tick: u64,
    /// The map, game type, mode, difficulty, skulls, roster and host, as
    /// the recorder observed them.
    pub descriptor: MatchHeader,
}

/// The values of the settings whose scope is `Sim`, and the feature switch
/// spec, by value.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    /// Setting key to value; a bool is 0 or 1.
    pub values: BTreeMap<String, i64>,
    /// `hooks::Switches::spec`.
    pub switches: String,
}

/// What the start hash is made of: the digest of the extension records
/// folded into it, and a hash of each [`DETAIL_PAGE`] of the arena. A start
/// that does not reproduce is compared part by part, so the report names
/// the arena pages or the extension state that differ rather than the
/// whole.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HashDetail {
    pub ext: Option<u64>,
    pub pages: Vec<u64>,
}

impl HashDetail {
    fn to_bytes(&self) -> Vec<u8> {
        let mut out = vec![u8::from(self.ext.is_some())];
        out.extend_from_slice(&self.ext.unwrap_or(0).to_le_bytes());
        out.extend(self.pages.iter().flat_map(|p| p.to_le_bytes()));
        out
    }

    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() >= 9 && (bytes.len() - 9) % 8 == 0 && bytes.len() <= MAX_JSON,
            "film start hash detail is malformed"
        );
        ensure!(bytes[0] <= 1, "film start hash detail has a bad flag");
        let word = |at: usize| u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
        Ok(Self {
            ext: (bytes[0] == 1).then(|| word(1)),
            pages: (9..bytes.len()).step_by(8).map(word).collect(),
        })
    }

    /// What differs from `now`: the extension digest, and the byte offsets
    /// of the first few arena pages.
    pub fn differences(&self, now: &HashDetail) -> String {
        let pages: Vec<String> = self
            .pages
            .iter()
            .zip(&now.pages)
            .enumerate()
            .filter(|(_, (a, b))| a != b)
            .map(|(i, _)| format!("{:#x}", i * DETAIL_PAGE))
            .collect();
        let mut out = Vec::new();
        if self.pages.len() != now.pages.len() {
            out.push(format!("the arena is {} pages, not {}", now.pages.len(), self.pages.len()));
        }
        if !pages.is_empty() {
            let shown = pages.iter().take(12).cloned().collect::<Vec<_>>().join(" ");
            let more = pages.len().saturating_sub(12);
            out.push(format!(
                "{} arena page(s) differ, at offsets {shown}{}",
                pages.len(),
                if more > 0 { format!(" and {more} more") } else { String::new() }
            ));
        }
        if self.ext != now.ext {
            out.push(format!("extension records differ (film {:x?}, now {:x?})", self.ext, now.ext));
        }
        if out.is_empty() {
            "arena pages and extension records are the same".into()
        } else {
            out.join("; ")
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameStart {
    pub stamp: Stamp,
    pub game: Game,
    /// The engine's options block, copied whole when the map activated.
    pub options: Vec<u8>,
    /// The shared random words at entry to the first update.
    pub random: [u32; 2],
    /// `lockstep::hash_sim` of the arena at entry to the first update.
    pub start_hash: u64,
    pub settings: Settings,
    pub variant: Option<Vec<u8>>,
    pub map_variant: Option<Vec<u8>>,
    /// What `start_hash` is made of, when the recorder could read it.
    pub hash_detail: Option<HashDetail>,
}

fn json<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let bytes = serde_json::to_vec(value)?;
    ensure!(bytes.len() <= MAX_JSON, "film game start section is too large");
    Ok(bytes)
}

fn from_json<T: DeserializeOwned>(bytes: &[u8], what: &str) -> Result<T> {
    ensure!(bytes.len() <= MAX_JSON, "film game start {what} is too large");
    serde_json::from_slice(bytes).with_context(|| format!("film game start {what} is not valid"))
}

impl GameStart {
    /// The sections, uncompressed.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        ensure!(
            self.options.len() == OPTIONS_BYTES,
            "game options are {} bytes, not {OPTIONS_BYTES}",
            self.options.len()
        );
        let mut sections: Vec<([u8; 4], Vec<u8>)> = vec![
            (STMP, json(&self.stamp)?),
            (GAME, json(&self.game)?),
            (OPTS, self.options.clone()),
            (
                RAND,
                self.random.iter().flat_map(|w| w.to_le_bytes()).collect(),
            ),
            (HASH, self.start_hash.to_le_bytes().to_vec()),
            (SETS, json(&self.settings)?),
        ];
        if let Some(variant) = &self.variant {
            sections.push((VARI, variant.clone()));
        }
        if let Some(map) = &self.map_variant {
            sections.push((MAPV, map.clone()));
        }
        if let Some(detail) = &self.hash_detail {
            sections.push((HDET, detail.to_bytes()));
        }
        let mut out = Vec::new();
        out.extend_from_slice(&(sections.len() as u32).to_le_bytes());
        for (tag, bytes) in sections {
            out.extend_from_slice(&tag);
            out.extend_from_slice(&u32::try_from(bytes.len())?.to_le_bytes());
            out.extend_from_slice(&bytes);
        }
        ensure!(out.len() <= MAX_INFLATED, "film game start is too large");
        Ok(out)
    }

    /// The zstd frame a film file stores.
    pub fn encode(&self) -> Result<Vec<u8>> {
        Ok(zstd::bulk::compress(&self.to_bytes()?, 3)?)
    }

    /// Inverse of [`GameStart::encode`].
    pub fn decode(frame: &[u8]) -> Result<Self> {
        let bytes = zstd::bulk::decompress(frame, MAX_INFLATED)
            .context("film game start does not inflate")?;
        Self::from_bytes(&bytes)
    }

    /// Inverse of [`GameStart::to_bytes`].
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        ensure!(bytes.len() >= 4, "film game start is cut short");
        let count = u32::from_le_bytes(bytes[..4].try_into()?);
        ensure!(count <= MAX_SECTIONS, "film game start has too many sections");
        let mut at = 4usize;
        let mut found: BTreeMap<[u8; 4], &[u8]> = BTreeMap::new();
        for _ in 0..count {
            ensure!(bytes.len() - at >= 8, "film game start is cut short");
            let tag: [u8; 4] = bytes[at..at + 4].try_into()?;
            let len = u32::from_le_bytes(bytes[at + 4..at + 8].try_into()?) as usize;
            at += 8;
            ensure!(
                ALL.contains(&tag),
                "film game start has an unknown section {}",
                String::from_utf8_lossy(&tag)
            );
            ensure!(
                len <= bytes.len() - at,
                "film game start section {} runs past its frame",
                String::from_utf8_lossy(&tag)
            );
            ensure!(
                found.insert(tag, &bytes[at..at + len]).is_none(),
                "film game start repeats section {}",
                String::from_utf8_lossy(&tag)
            );
            at += len;
        }
        ensure!(at == bytes.len(), "film game start has bytes after its sections");
        for tag in REQUIRED {
            ensure!(
                found.contains_key(&tag),
                "film game start lacks section {}",
                String::from_utf8_lossy(&tag)
            );
        }
        let options = found[&OPTS];
        ensure!(
            options.len() == OPTIONS_BYTES,
            "film game options are {} bytes, not {OPTIONS_BYTES}",
            options.len()
        );
        let random = found[&RAND];
        ensure!(random.len() == 8, "film random words are not two words");
        let hash = found[&HASH];
        ensure!(hash.len() == 8, "film start hash is not one word");
        Ok(Self {
            stamp: from_json(found[&STMP], "stamp")?,
            game: from_json(found[&GAME], "game")?,
            options: options.to_vec(),
            random: [
                u32::from_le_bytes(random[..4].try_into()?),
                u32::from_le_bytes(random[4..].try_into()?),
            ],
            start_hash: u64::from_le_bytes(hash.try_into()?),
            settings: from_json(found[&SETS], "settings")?,
            variant: found.get(&VARI).map(|b| b.to_vec()),
            map_variant: found.get(&MAPV).map(|b| b.to_vec()),
            hash_detail: found.get(&HDET).map(|b| HashDetail::from_bytes(b)).transpose()?,
        })
    }
}

#[cfg(test)]
pub(crate) mod tests;
