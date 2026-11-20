//! Package identity: the publisher id Windows derives from the manifest's
//! `Publisher`, the package family name and the app's AUMID, which the
//! Device Portal launches by (platform/xbox/deploy.sh).

use sha2::{Digest, Sha256};

/// Windows' base32 alphabet for publisher ids: Crockford's, lower case,
/// without i, l, o and u.
const ALPHABET: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";

/// The 13-character publisher id: the first 64 bits of SHA-256 over the
/// UTF-16LE `Publisher` string, padded with a zero bit to 65 and written
/// 5 bits at a time.
pub fn publisher_id(publisher: &str) -> String {
    let utf16: Vec<u8> = publisher.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let digest = Sha256::digest(&utf16);
    let bits = u128::from(u64::from_be_bytes(digest[..8].try_into().unwrap())) << 1;
    (0..13).rev().map(|i| ALPHABET[((bits >> (i * 5)) & 31) as usize] as char).collect()
}

/// `<Name>_<publisher id>`.
pub fn family_name(name: &str, publisher: &str) -> String {
    format!("{name}_{}", publisher_id(publisher))
}

/// The application user model id: `<family name>!<Application Id>`.
pub fn aumid(name: &str, publisher: &str, app_id: &str) -> String {
    format!("{}!{app_id}", family_name(name, publisher))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn microsofts_publisher_gives_its_well_known_id() {
        // Every inbox app's family name ends in it (Microsoft.WindowsCalculator_8wekyb3d8bbwe).
        let ms = "CN=Microsoft Corporation, O=Microsoft Corporation, L=Redmond, S=Washington, C=US";
        assert_eq!(publisher_id(ms), "8wekyb3d8bbwe");
        assert_eq!(family_name("Microsoft.WindowsCalculator", ms), "Microsoft.WindowsCalculator_8wekyb3d8bbwe");
        assert_eq!(aumid("Microsoft.WindowsCalculator", ms, "App"), "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App");
    }

    #[test]
    fn ids_are_thirteen_alphabet_characters_and_depend_on_the_publisher() {
        let a = publisher_id("CN=h2recomp-dev");
        assert_eq!(a.len(), 13);
        assert!(a.bytes().all(|b| ALPHABET.contains(&b)), "{a}");
        // The 65th bit is padding, so the last character is always even.
        assert_eq!(ALPHABET.iter().position(|&c| c == *a.as_bytes().last().unwrap()).unwrap() % 2, 0);
        assert_ne!(a, publisher_id("CN=h2recomp-dev2"));
    }
}
