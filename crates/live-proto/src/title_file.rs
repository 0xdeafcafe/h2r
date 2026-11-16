//! The 16-byte header on Halo 2's title-storage files
//! (`message_of_the_day.dat`, `multiplayer_hopper.dat`), checked by the
//! game's validator `0x00081230` before either file's handler runs
//! (docs/live/live-motd.md §2.4):
//!
//! ```text
//! +0x00  u32 LE  CRC register over bytes [4, size): reflected polynomial
//!                0xedb88320, initial value 0xffffffff, no final xor (the
//!                bitwise NOT of the standard CRC-32)
//! +0x04  u32 LE  the file class's version, exactly: MOTD 1, hopper 4
//! +0x08  u32 LE  the file's size, equal to the bytes received
//! +0x0c  u32     not checked (written as 0)
//! +0x10          the body: u32 BE inflated size, then a zlib stream
//! ```
//!
//! Files this project built before the header was traced carry 16 zero
//! bytes there; [`check_or_legacy`] accepts those so they can be read and
//! republished with a real header.

/// The header's size.
pub const HEADER: usize = 16;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum HeaderError {
    #[error("shorter than the 16-byte header")]
    Short,
    #[error("the header says {header} bytes, the file is {actual}")]
    Size { header: u32, actual: usize },
    #[error("the file is {0} bytes; the game takes at most {1}")]
    TooLarge(usize, usize),
    #[error("version {got}, the game wants exactly {want}")]
    Version { got: u32, want: u32 },
    #[error("checksum {got:#010x}, the bytes give {want:#010x}")]
    Crc { got: u32, want: u32 },
    #[error(
        "the header is 16 zero bytes (built before the header was known); \
         the game rejects it: republish the file"
    )]
    Legacy,
}

/// The game's CRC (routine `0x00163ba0`): the CRC-32 register with no
/// final xor.
pub fn crc_register(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for b in bytes {
        crc ^= u32::from(*b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    crc
}

fn le32(file: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(file[at..at + 4].try_into().expect("4 bytes"))
}

/// Prefixes `body` with the header for `version`.
pub fn seal(version: u32, body: &[u8]) -> Vec<u8> {
    let mut file = vec![0u8; HEADER];
    file.extend_from_slice(body);
    let size = file.len() as u32;
    file[4..8].copy_from_slice(&version.to_le_bytes());
    file[8..12].copy_from_slice(&size.to_le_bytes());
    let crc = crc_register(&file[4..]);
    file[0..4].copy_from_slice(&crc.to_le_bytes());
    file
}

/// What the game's validator checks, in its order; on success, the body.
pub fn check(file: &[u8], version: u32, cap: usize) -> Result<&[u8], HeaderError> {
    if file.len() < HEADER {
        return Err(HeaderError::Short);
    }
    if file[..HEADER].iter().all(|b| *b == 0) {
        return Err(HeaderError::Legacy);
    }
    let size = le32(file, 8);
    if size as usize != file.len() {
        return Err(HeaderError::Size {
            header: size,
            actual: file.len(),
        });
    }
    if file.len() > cap {
        return Err(HeaderError::TooLarge(file.len(), cap));
    }
    let got = le32(file, 4);
    if got != version {
        return Err(HeaderError::Version { got, want: version });
    }
    let (got, want) = (le32(file, 0), crc_register(&file[4..]));
    if got != want {
        return Err(HeaderError::Crc { got, want });
    }
    Ok(&file[HEADER..])
}

/// Whether `file` has the all-zero header of this project's early builds.
pub fn is_legacy(file: &[u8]) -> bool {
    file.len() >= HEADER && file[..HEADER].iter().all(|b| *b == 0)
}

/// [`check`], but a legacy zero header is accepted too (for migration and
/// imports, never for serving).
pub fn check_or_legacy(file: &[u8], version: u32, cap: usize) -> Result<&[u8], HeaderError> {
    if is_legacy(file) {
        Ok(&file[HEADER..])
    } else {
        check(file, version, cap)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_crc_is_the_crc32_register_without_the_final_xor() {
        // The CRC-32 check value of "123456789" is 0xcbf43926; without the
        // final xor the register holds its complement.
        assert_eq!(crc_register(b"123456789"), 0x340b_c6d9);
        assert_eq!(!crc_register(b"123456789"), 0xcbf4_3926);
        assert_eq!(crc_register(b""), 0xffff_ffff);
    }

    #[test]
    fn a_sealed_file_has_the_fields_the_validator_reads() {
        let file = seal(1, b"body");
        assert_eq!(file.len(), 20);
        assert_eq!(&file[4..8], &[1, 0, 0, 0]);
        assert_eq!(&file[8..12], &[20, 0, 0, 0]);
        assert_eq!(&file[12..16], &[0; 4]);
        assert_eq!(&file[16..], b"body");
        // Computed independently (Python's zlib.crc32 of
        // 01 00 00 00 14 00 00 00 00 00 00 00 62 6f 64 79, xor 0xffffffff).
        assert_eq!(le32(&file, 0), 0x48de_2716);
        assert_eq!(&file[0..4], &[0x16, 0x27, 0xde, 0x48]);
        assert_eq!(check(&file, 1, 0x8000), Ok(&b"body"[..]));
    }

    #[test]
    fn the_validator_rejects_what_the_game_rejects() {
        let file = seal(4, b"hopper body");
        assert_eq!(check(&file, 4, 0x20000), Ok(&b"hopper body"[..]));
        assert_eq!(check(&file[..8], 4, 0x20000), Err(HeaderError::Short));
        assert_eq!(
            check(&file, 1, 0x20000),
            Err(HeaderError::Version { got: 4, want: 1 })
        );
        assert_eq!(
            check(&file, 4, 16),
            Err(HeaderError::TooLarge(file.len(), 16))
        );
        let mut longer = file.clone();
        longer.push(0);
        assert!(matches!(
            check(&longer, 4, 0x20000),
            Err(HeaderError::Size {
                header: 27,
                actual: 28
            })
        ));
        let mut flipped = file.clone();
        *flipped.last_mut().unwrap() ^= 1;
        assert!(matches!(
            check(&flipped, 4, 0x20000),
            Err(HeaderError::Crc { .. })
        ));
        let mut legacy = vec![0u8; 16];
        legacy.extend_from_slice(b"old");
        assert_eq!(check(&legacy, 1, 0x8000), Err(HeaderError::Legacy));
        assert!(is_legacy(&legacy));
        assert!(!is_legacy(&file));
        assert_eq!(check_or_legacy(&legacy, 1, 0x8000), Ok(&b"old"[..]));
        assert_eq!(check_or_legacy(&file, 4, 0x20000), Ok(&b"hopper body"[..]));
    }
}
