//! Wire contract for publishing a sealed input film and its stats companion.
use super::PART_BYTES;
use serde::{Deserialize, Serialize};

pub const MAX_FILM: u64 = 256 << 20;
pub const ACCOUNT_QUOTA: u64 = 1_000_000_000;
pub const HEADER: usize = 64;
pub const MAX_SEED: usize = 256 << 10;

#[derive(Clone, Copy, Default, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    #[default]
    Private,
    Link,
    Public,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Object {
    pub size: u64,
    pub sha256: String,
    pub parts: Vec<String>,
}

pub fn digest(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

impl Object {
    pub fn valid(&self) -> bool {
        (1..=MAX_FILM).contains(&self.size)
            && self.parts.len() as u64 == self.size.div_ceil(PART_BYTES)
            && digest(&self.sha256)
            && self.parts.iter().all(|s| digest(s))
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Upload {
    pub film: Object,
    pub stats: Object,
    #[serde(default)]
    pub visibility: Visibility,
}

/// The v4 header identifies machine-free, sealed engine recordings. A
/// participant-only machine recording must never enter player file shares.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Layout {
    pub seed_end: u64,
    pub trailer: u64,
    pub chunks: u32,
    pub ticks: u32,
}

impl Layout {
    pub fn parse(h: &[u8], size: u64) -> Result<Self, &'static str> {
        if h.len() != HEADER || h[..8] != *b"H2FILM4!" || h[8..10] != 4u16.to_le_bytes() {
            return Err("sharing requires a split input film");
        }
        if h[10..12] != 2u16.to_le_bytes() {
            return Err("sharing requires a sealed film without machine state or a start point");
        }
        let ticks = u32::from_le_bytes(h[32..36].try_into().unwrap());
        let seed = u32::from_le_bytes(h[36..40].try_into().unwrap());
        let trailer = u64::from_le_bytes(h[40..48].try_into().unwrap());
        let chunks = ticks.div_ceil(256);
        let seed_end = HEADER as u64 + u64::from(seed);
        if !(1..=MAX_FILM).contains(&size)
            || h[12] == 0
            || h[13] != 16
            || h[14..16] != 256u16.to_le_bytes()
            || ticks == 0
            || seed == 0
            || seed as usize > MAX_SEED
            || seed_end >= trailer
            || trailer.checked_add(4 + 8 * u64::from(chunks)) != Some(size)
            || u64::from(chunks) * 6 > trailer - seed_end
        {
            return Err("invalid sealed film bounds");
        }
        Ok(Self {
            seed_end,
            trailer,
            chunks,
            ticks,
        })
    }
}

/// A published header identifies only its game. Local filenames and recorder
/// identifiers have no bearing on the simulation or the transfer cache.
pub fn header_id(game: &str) -> [u8; 16] {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(game.as_bytes());
    hash[..16].try_into().expect("SHA-256 prefix")
}

/// Private metadata on a local seed may name its recording account and
/// machine. The upload never needs these fields. Only simulation fields are
/// admitted at the public boundary, including joins' game-local slot IDs.
pub fn clean_seed(bytes: &[u8]) -> Result<serde_json::Value, &'static str> {
    use serde_json::Value;
    let mut seed: Value = serde_json::from_slice(bytes).map_err(|_| "invalid film seed")?;
    let obj = seed.as_object_mut().ok_or("invalid film seed")?;
    for key in [
        "xuid",
        "account",
        "server",
        "machine_id",
        "address",
        "lease_id",
        "host_id",
        "path",
    ] {
        obj.remove(key);
    }
    if obj.keys().any(|s| {
        !matches!(
            s.as_str(),
            "compat"
                | "game_id"
                | "map"
                | "map_sha256"
                | "variant"
                | "game_options"
                | "seed"
                | "roster"
        )
    }) {
        return Err("unrecognized film seed field");
    }
    let game = obj
        .get("game_id")
        .and_then(Value::as_str)
        .filter(|id| crate::game::valid_id(id))
        .ok_or("invalid game id in film seed")?;
    let identifiers = crate::game::GameIdentifiers::for_game(game);
    let compat: Compat = serde_json::from_value(
        obj.get("compat")
            .cloned()
            .ok_or("missing film compatibility")?,
    )
    .map_err(|_| "invalid film compatibility")?;
    if !digest(&compat.xbe)
        || !digest(&compat.contract)
        || !digest(&compat.hooks)
        || compat.abi.len() > 64
        || compat.abi.is_empty()
    {
        return Err("invalid film compatibility");
    }
    for field in ["variant", "game_options"] {
        if obj.get(field).is_some_and(|v| {
            !v.as_array()
                .is_some_and(|a| a.iter().all(|b| b.as_u64().is_some_and(|n| n <= 255)))
        }) {
            return Err("invalid game options or variant");
        }
    }
    if !obj
        .get("map")
        .and_then(Value::as_str)
        .is_some_and(crate::game::valid_id)
        || !obj
            .get("map_sha256")
            .and_then(Value::as_str)
            .is_some_and(digest)
        || obj.get("seed").and_then(Value::as_u64).is_none()
    {
        return Err("invalid film start");
    }
    let roster = obj
        .get_mut("roster")
        .and_then(Value::as_array_mut)
        .ok_or("missing film roster")?;
    let mut seats = 0u16;
    for entry in roster {
        let seat = entry.as_object_mut().ok_or("invalid film roster")?;
        for key in ["xuid", "account", "machine_id", "address", "host_id"] {
            seat.remove(key);
        }
        if seat.keys().any(|s| {
            !matches!(
                s.as_str(),
                "seat" | "name" | "team" | "seat_id" | "machine" | "appearance"
            )
        }) || !seat
            .get("name")
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty() && s.len() <= 64 && !s.chars().any(char::is_control))
            || !seat
                .get("team")
                .and_then(Value::as_i64)
                .is_some_and(|v| (-1..=15).contains(&v))
        {
            return Err("invalid film roster");
        }
        let n = seat
            .get("seat")
            .and_then(Value::as_u64)
            .filter(|n| *n < 16)
            .ok_or("invalid film seat")?;
        if seat
            .get("seat_id")
            .is_some_and(|id| id.as_str() != Some(identifiers.seats[n as usize].as_str()))
            || seat.get("machine").is_some_and(|id| {
                !identifiers
                    .machines
                    .iter()
                    .any(|allowed| Some(allowed.as_str()) == id.as_str())
            })
        {
            return Err("film roster has an identifier from outside this game");
        }
        if let Some(look) = seat.get("appearance") {
            let look = look.as_object().ok_or("invalid film appearance")?;
            if look.iter().any(|(key, value)| {
                !matches!(
                    key.as_str(),
                    "primary"
                        | "secondary"
                        | "tertiary"
                        | "quaternary"
                        | "emblem_foreground"
                        | "emblem_background"
                ) || !value.as_u64().is_some_and(|v| v < 64)
            }) {
                return Err("invalid film appearance");
            }
        }
        if seats & (1 << n) != 0 {
            return Err("duplicate film seat");
        }
        seats |= 1 << n;
    }
    Ok(seed)
}

/// Public identifying fields of the simulation contract. The seed itself
/// also contains the game options, variant, seed and seat configurations.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Compat {
    pub xbe: String,
    pub contract: String,
    pub abi: String,
    pub hooks: String,
}
