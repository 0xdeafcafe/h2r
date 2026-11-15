//! Builds and reads Halo 2's `message_of_the_day.dat` (docs/live-motd.md
//! §3.1):
//!
//! ```text
//! +0x00  16 bytes   header (zeros; the game skips it)
//! +0x10  u32 BE     inflated size, 0x25224
//! +0x14  zlib stream inflating to:
//!          9 x 512 bytes      per-language header strings
//!          9 x 0x4004 bytes   per-language blocks: u32 LE count (<= 32),
//!                             then 32 x 512-byte strings
//! ```
//!
//! Strings are UTF-16LE, NUL-terminated, at most 255 code units. The same
//! text goes into all nine language slots.

use std::io::{Read, Write};

/// The size the game requires the payload to inflate to.
pub const INFLATED_SIZE: usize = 0x25224;
pub const LANGUAGES: usize = 9;
pub const MAX_MESSAGES: usize = 32;
/// Longest string, in UTF-16 code units (one 512-byte slot minus the NUL).
pub const MAX_UNITS: usize = 255;
/// The largest file the game takes: its one attachment download passes a
/// 0x499a-byte buffer (docs/live-motd-fetch.md §5). A larger file fails
/// that download and the game silently shows no message of the day.
pub const MAX_FILE: usize = 0x499a;

const SLOT: usize = 512;
const BLOCK: usize = 4 + MAX_MESSAGES * SLOT;
const BLOCKS_AT: usize = LANGUAGES * SLOT;
const PREFIX: usize = 16;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum MotdError {
    #[error("a message of the day needs 1 to 32 messages, got {0}")]
    Count(usize),
    #[error("{0:?} is longer than 255 UTF-16 code units")]
    TooLong(String),
    #[error("{0:?} contains a NUL")]
    Nul(String),
    #[error("the file is {0} bytes; the game downloads at most 18842 (0x499a)")]
    TooLarge(usize),
    #[error("not a message_of_the_day.dat: {0}")]
    Format(&'static str),
}

/// The text of one message of the day, as the game shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Motd {
    pub header: String,
    pub messages: Vec<String>,
}

fn put_string(slot: &mut [u8], s: &str) -> Result<(), MotdError> {
    if s.contains('\0') {
        return Err(MotdError::Nul(s.into()));
    }
    let units: Vec<u16> = s.encode_utf16().collect();
    if units.len() > MAX_UNITS {
        return Err(MotdError::TooLong(s.into()));
    }
    for (i, u) in units.iter().enumerate() {
        slot[i * 2..i * 2 + 2].copy_from_slice(&u.to_le_bytes());
    }
    Ok(())
}

fn get_string(slot: &[u8]) -> String {
    let units: Vec<u16> = slot
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .take_while(|u| *u != 0)
        .collect();
    String::from_utf16_lossy(&units)
}

impl Motd {
    /// The file the game downloads.
    pub fn build(&self) -> Result<Vec<u8>, MotdError> {
        let n = self.messages.len();
        if !(1..=MAX_MESSAGES).contains(&n) {
            return Err(MotdError::Count(n));
        }
        let mut raw = vec![0u8; INFLATED_SIZE];
        for lang in 0..LANGUAGES {
            put_string(&mut raw[lang * SLOT..][..SLOT], &self.header)?;
            let block = &mut raw[BLOCKS_AT + lang * BLOCK..][..BLOCK];
            block[..4].copy_from_slice(&(n as u32).to_le_bytes());
            for (i, m) in self.messages.iter().enumerate() {
                put_string(&mut block[4 + i * SLOT..][..SLOT], m)?;
            }
        }
        let mut out = vec![0u8; PREFIX];
        out.extend_from_slice(&(INFLATED_SIZE as u32).to_be_bytes());
        let mut z = flate2::write::ZlibEncoder::new(out, flate2::Compression::best());
        z.write_all(&raw).expect("writing to a Vec");
        let file = z.finish().expect("writing to a Vec");
        if file.len() > MAX_FILE {
            return Err(MotdError::TooLarge(file.len()));
        }
        Ok(file)
    }

    /// Reads a file the way the game does, returning the first language's
    /// text.
    pub fn parse(file: &[u8]) -> Result<Self, MotdError> {
        let size = file
            .get(PREFIX..PREFIX + 4)
            .ok_or(MotdError::Format("too short"))?;
        if u32::from_be_bytes(size.try_into().expect("4 bytes")) as usize != INFLATED_SIZE {
            return Err(MotdError::Format("inflated size is not 0x25224"));
        }
        let mut raw = Vec::with_capacity(INFLATED_SIZE);
        flate2::read::ZlibDecoder::new(&file[PREFIX + 4..])
            .take(INFLATED_SIZE as u64 + 1)
            .read_to_end(&mut raw)
            .map_err(|_| MotdError::Format("bad zlib stream"))?;
        if raw.len() != INFLATED_SIZE {
            return Err(MotdError::Format(
                "payload does not inflate to 0x25224 bytes",
            ));
        }
        let block = &raw[BLOCKS_AT..][..BLOCK];
        let count = u32::from_le_bytes(block[..4].try_into().expect("4 bytes")) as usize;
        if count > MAX_MESSAGES {
            return Err(MotdError::Format("more than 32 messages"));
        }
        Ok(Motd {
            header: get_string(&raw[..SLOT]),
            messages: (0..count)
                .map(|i| get_string(&block[4 + i * SLOT..][..SLOT]))
                .collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn motd() -> Motd {
        Motd {
            header: "Halo 2 is back".into(),
            messages: vec!["Welcome, Spartan.".into(), "Ünïcödé and 𝄞 survive".into()],
        }
    }

    #[test]
    fn layout_matches_the_game() {
        assert_eq!(BLOCKS_AT + LANGUAGES * BLOCK, INFLATED_SIZE);
        let file = motd().build().unwrap();
        assert_eq!(&file[..16], &[0u8; 16]);
        assert_eq!(&file[16..20], &[0x00, 0x02, 0x52, 0x24]);

        let mut raw = Vec::new();
        flate2::read::ZlibDecoder::new(&file[20..])
            .read_to_end(&mut raw)
            .unwrap();
        assert_eq!(raw.len(), 152_100);
        // Every language slot carries the same text.
        for lang in 0..LANGUAGES {
            assert_eq!(get_string(&raw[lang * 512..][..512]), "Halo 2 is back");
            let block = &raw[0x1200 + lang * 0x4004..][..0x4004];
            assert_eq!(u32::from_le_bytes(block[..4].try_into().unwrap()), 2);
            assert_eq!(get_string(&block[4..516]), "Welcome, Spartan.");
        }
        // UTF-16LE: "H" then "a".
        assert_eq!(&raw[..4], &[b'H', 0, b'a', 0]);
    }

    #[test]
    fn round_trips() {
        let m = motd();
        assert_eq!(Motd::parse(&m.build().unwrap()).unwrap(), m);
        let full = Motd {
            header: "x".repeat(255),
            messages: vec!["y".repeat(255); 32],
        };
        assert_eq!(Motd::parse(&full.build().unwrap()).unwrap(), full);
    }

    /// Text that does not compress: 32 full messages of varied characters.
    #[test]
    fn the_largest_motd_fits_the_games_download_buffer() {
        let mut seed = 0x2545f491u32;
        let mut line = || {
            (0..MAX_UNITS)
                .map(|_| {
                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    char::from(b' ' + (seed >> 24) as u8 % 95)
                })
                .collect::<String>()
        };
        let worst = Motd {
            header: line(),
            messages: (0..MAX_MESSAGES).map(|_| line()).collect(),
        };
        match worst.build() {
            Ok(file) => assert!(file.len() <= MAX_FILE),
            Err(e) => assert!(matches!(e, MotdError::TooLarge(n) if n > MAX_FILE), "{e}"),
        }
    }

    #[test]
    fn rejects_what_the_game_cannot_show() {
        let too_long = Motd {
            header: "x".repeat(256),
            messages: vec!["m".into()],
        };
        assert!(matches!(too_long.build(), Err(MotdError::TooLong(_))));
        // 128 surrogate pairs are 256 units.
        let pairs = Motd {
            header: "h".into(),
            messages: vec!["𝄞".repeat(128)],
        };
        assert!(matches!(pairs.build(), Err(MotdError::TooLong(_))));
        assert_eq!(
            Motd {
                header: "h".into(),
                messages: vec![]
            }
            .build(),
            Err(MotdError::Count(0))
        );
        assert_eq!(
            Motd {
                header: "h".into(),
                messages: vec!["m".into(); 33]
            }
            .build(),
            Err(MotdError::Count(33))
        );
        assert!(matches!(
            Motd {
                header: "a\0b".into(),
                messages: vec!["m".into()]
            }
            .build(),
            Err(MotdError::Nul(_))
        ));

        assert!(Motd::parse(b"short").is_err());
        let mut file = motd().build().unwrap();
        file[19] ^= 1;
        assert!(Motd::parse(&file).is_err());
    }
}
