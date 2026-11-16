//! `.h2variant`: our file for sharing a variant outside the game. JSON, so
//! people can read what it is, with the 0x130-byte record as hex as the
//! source of truth (unknown bytes travel too). The game never reads it, so
//! it has no signature; importing checks the record like a save would.

use serde::{Deserialize, Serialize};

use crate::record::{self, GameVariant, RECORD_SIZE};

pub const FORMAT: &str = "h2recomp-game-variant";
pub const VERSION: u32 = 1;
pub const EXTENSION: &str = "h2variant";

#[derive(Debug, Serialize, Deserialize)]
struct File {
    format: String,
    version: u32,
    /// For people; the record decides.
    name: String,
    game_type: String,
    record: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PortableError {
    #[error("not a game variant file: {0}")]
    Json(String),
    #[error("not a game variant file (format {0:?})")]
    Format(String),
    #[error("variant file version {0} is newer than this editor ({VERSION})")]
    Version(u32),
    #[error("the record is not {RECORD_SIZE:#x} bytes of hex")]
    Record,
}

pub fn export(v: &GameVariant) -> String {
    let f = File {
        format: FORMAT.into(),
        version: VERSION,
        name: v.name(),
        game_type: v.game_type().label(),
        record: v.bytes().iter().map(|b| format!("{b:02x}")).collect(),
    };
    serde_json::to_string_pretty(&f).expect("plain data serializes")
}

/// The variant in a portable file and what the game's clean-up would say
/// about it (empty: the game takes it as is).
pub fn import(text: &str) -> Result<(GameVariant, Vec<record::Problem>), PortableError> {
    let f: File = serde_json::from_str(text).map_err(|e| PortableError::Json(e.to_string()))?;
    if f.format != FORMAT {
        return Err(PortableError::Format(f.format));
    }
    if f.version > VERSION {
        return Err(PortableError::Version(f.version));
    }
    let hex = f.record.trim();
    if hex.len() != 2 * RECORD_SIZE || !hex.is_ascii() {
        return Err(PortableError::Record);
    }
    let bytes = (0..RECORD_SIZE)
        .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16))
        .collect::<Result<Vec<u8>, _>>()
        .map_err(|_| PortableError::Record)?;
    let v = GameVariant::from_bytes(&bytes).map_err(|_| PortableError::Record)?;
    let problems = record::check(&v);
    Ok((v, problems))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{GameType, SettingId};

    #[test]
    fn round_trip_keeps_every_byte() {
        let mut v = GameVariant::blank(GameType::King);
        v.set_name("Hill \u{e006}").unwrap();
        v.set(SettingId(40), 60).unwrap();
        let mut raw = v.to_bytes();
        raw[0x10c] = 0xab; // a byte nobody models
        let v = GameVariant::from_bytes(&raw).unwrap();
        let text = export(&v);
        assert!(text.contains("\"King of the Hill\""));
        let (back, problems) = import(&text).unwrap();
        assert_eq!(back.to_bytes(), raw);
        assert!(problems.is_empty());
    }

    #[test]
    fn bad_files_are_refused() {
        let good = export(&GameVariant::blank(GameType::Slayer));
        assert!(matches!(import("{"), Err(PortableError::Json(_))));
        assert!(matches!(import(&good.replace(FORMAT, "other")), Err(PortableError::Format(_))));
        assert_eq!(import(&good.replace("\"version\": 1", "\"version\": 9")).unwrap_err(), PortableError::Version(9));
        assert_eq!(import(&good.replace("\"record\": \"00", "\"record\": \"zz")).unwrap_err(), PortableError::Record);
        assert_eq!(import(&good.replace("\"record\": \"00", "\"record\": \"")).unwrap_err(), PortableError::Record);
        // A record the game would clean up imports with its problems listed.
        let mut v = GameVariant::blank(GameType::Slayer);
        v.set(SettingId(19), 5).unwrap();
        assert_eq!(import(&export(&v)).unwrap().1.len(), 1);
    }
}
