//! Account recovery (docs/live/live-security.md §1, "Recovery"): a TOTP
//! secret per account (RFC 6238: HMAC-SHA1, 30 s steps, six digits) that the
//! server holds, any signed-in device can show the current code of, and an
//! authenticator app can enrol from the secret or its `otpauth://` URI. On a
//! new device the user types the gamertag and the current code; the device
//! sends a [`RecoverRequest`] signed by its new key with the code beside it,
//! and the server adds that key to the account.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha1::Sha1;

use crate::sign::{self, Jwk, Jws, KeyPair, MAX_EVENT_SKEW_S, PublicKey, SignError, TYP_RECOVER};

/// Seconds per code.
pub const PERIOD: i64 = 30;
/// Digits per code.
pub const DIGITS: u32 = 6;
/// Secret bytes (160 bits, RFC 4226's recommended length).
pub const SECRET_BYTES: usize = 20;
/// Steps either side of now a code is still accepted for.
pub const SKEW_STEPS: i64 = 1;

const BASE32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// A TOTP secret. Its `Debug` hides it.
#[derive(Clone, PartialEq, Eq)]
pub struct TotpSecret([u8; SECRET_BYTES]);

impl std::fmt::Debug for TotpSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TotpSecret(..)")
    }
}

/// The time step of unix second `t`.
pub fn step_of(t: i64) -> i64 {
    t.div_euclid(PERIOD)
}

impl TotpSecret {
    pub fn generate() -> Self {
        let mut b = [0u8; SECRET_BYTES];
        getrandom::fill(&mut b).expect("the OS random source failed");
        TotpSecret(b)
    }

    pub fn from_bytes(b: [u8; SECRET_BYTES]) -> Self {
        TotpSecret(b)
    }

    pub fn bytes(&self) -> &[u8; SECRET_BYTES] {
        &self.0
    }

    /// RFC 4648 base32 without padding: what authenticator apps take.
    pub fn base32(&self) -> String {
        let mut out = String::new();
        let (mut acc, mut bits) = (0u32, 0u32);
        for b in self.0 {
            acc = (acc << 8) | u32::from(b);
            bits += 8;
            while bits >= 5 {
                bits -= 5;
                out.push(BASE32[((acc >> bits) & 31) as usize] as char);
            }
        }
        if bits > 0 {
            out.push(BASE32[((acc << (5 - bits)) & 31) as usize] as char);
        }
        out
    }

    /// Reads [`Self::base32`] back (any case, spaces ignored).
    pub fn from_base32(s: &str) -> Option<Self> {
        let mut bytes = Vec::with_capacity(SECRET_BYTES);
        let (mut acc, mut bits) = (0u32, 0u32);
        for c in s.chars().filter(|c| !c.is_whitespace() && *c != '=') {
            let v = BASE32
                .iter()
                .position(|b| *b as char == c.to_ascii_uppercase())?;
            acc = (acc << 5) | v as u32;
            bits += 5;
            if bits >= 8 {
                bits -= 8;
                bytes.push((acc >> bits) as u8);
            }
        }
        Some(TotpSecret(bytes.try_into().ok()?))
    }

    /// The code for time step `step` (RFC 4226 HOTP over the step).
    pub fn code_at_step(&self, step: i64) -> u32 {
        let mut mac = Hmac::<Sha1>::new_from_slice(&self.0).expect("any key length");
        mac.update(&(step as u64).to_be_bytes());
        let h = mac.finalize().into_bytes();
        let o = usize::from(h[19] & 0xf);
        let bin = u32::from_be_bytes([h[o] & 0x7f, h[o + 1], h[o + 2], h[o + 3]]);
        bin % 10u32.pow(DIGITS)
    }

    /// The code at unix second `t`.
    pub fn code_at(&self, t: i64) -> u32 {
        self.code_at_step(step_of(t))
    }

    /// The step `code` is valid for at `now`, allowing [`SKEW_STEPS`]
    /// either side; `None` when it matches none.
    pub fn verify(&self, code: u32, now: i64) -> Option<i64> {
        let step = step_of(now);
        (step - SKEW_STEPS..=step + SKEW_STEPS).find(|s| self.code_at_step(*s) == code)
    }

    /// The `otpauth://` URI authenticator apps enrol from (often as a QR
    /// code): label `issuer:gamertag`.
    pub fn otpauth(&self, issuer: &str, gamertag: &str) -> String {
        let issuer = uri_escape(issuer);
        format!(
            "otpauth://totp/{issuer}:{}?secret={}&issuer={issuer}&algorithm=SHA1&digits={DIGITS}&period={PERIOD}",
            uri_escape(gamertag),
            self.base32(),
        )
    }
}

fn uri_escape(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A code as shown: `123-456`.
pub fn format_code(code: u32) -> String {
    let s = format!("{code:06}");
    format!("{}-{}", &s[..3], &s[3..])
}

/// A code as typed: six digits, spaces and dashes ignored.
pub fn parse_code(typed: &str) -> Option<u32> {
    let digits: String = typed
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect();
    (digits.len() == DIGITS as usize && digits.bytes().all(|b| b.is_ascii_digit()))
        .then(|| digits.parse().ok())
        .flatten()
}

/// What the recovery endpoints answer: the secret, its URI and the current
/// code with when it rolls over.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryInfo {
    /// [`TotpSecret::base32`].
    pub secret: String,
    pub otpauth: String,
    /// [`format_code`] of the current code.
    pub code: String,
    pub period: i64,
    /// Unix seconds when `code` rolls over.
    pub expires_at: i64,
}

impl RecoveryInfo {
    pub fn new(secret: &TotpSecret, issuer: &str, gamertag: &str, now: i64) -> Self {
        RecoveryInfo {
            secret: secret.base32(),
            otpauth: secret.otpauth(issuer, gamertag),
            code: format_code(secret.code_at(now)),
            period: PERIOD,
            expires_at: (step_of(now) + 1) * PERIOD,
        }
    }
}

/// `POST /v1/accounts/{gamertag}/recover`, the JWS half: signed by the new
/// device key (proof of possession), naming the account and the server.
/// The code goes beside it, not inside, so the logged proof never holds one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecoverRequest {
    pub gamertag: String,
    /// The new device key.
    pub jwk: Jwk,
    /// The server id it is for.
    pub aud: String,
    /// When it was signed (unix seconds), within [`MAX_EVENT_SKEW_S`].
    pub iat: i64,
    /// Random, so one request is used once.
    pub nonce: String,
}

/// The body of `POST /v1/accounts/{gamertag}/recover`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecoverBody {
    /// A compact JWS of [`RecoverRequest`].
    pub request: String,
    /// The code as typed ([`parse_code`]).
    pub code: String,
}

impl RecoverRequest {
    pub fn sign(device: &KeyPair, gamertag: &str, aud: &str, iat: i64) -> String {
        device.sign_json(
            TYP_RECOVER,
            &RecoverRequest {
                gamertag: gamertag.into(),
                jwk: device.public().jwk(),
                aud: aud.into(),
                iat,
                nonce: sign::random_id(),
            },
        )
    }

    /// Opens and verifies a request against the key it carries, and checks
    /// it is for `aud` and recent.
    pub fn open(compact: &str, aud: &str, now: i64) -> Result<(Self, PublicKey), SignError> {
        let jws = Jws::parse(compact)?;
        let req: RecoverRequest = serde_json::from_value(jws.json_payload(TYP_RECOVER)?)
            .map_err(|_| SignError::Malformed("recover request fields"))?;
        let key = PublicKey::from_jwk(&req.jwk)?;
        if jws.kid() != Some(key.kid()) {
            return Err(SignError::KidMismatch);
        }
        jws.verify(&key)?;
        if req.aud != aud {
            return Err(SignError::Claims("recover request is for another server"));
        }
        if (req.iat - now).abs() > MAX_EVENT_SKEW_S {
            return Err(SignError::Claims(
                "recover request iat is more than 5 minutes off",
            ));
        }
        Ok((req, key))
    }
}

/// Sealing for secrets kept at rest by the server (the TOTP secrets): with
/// a 32-byte key, `nonce(16) || ct || tag(16)` where the key stream is
/// HMAC-SHA256(key, "enc" || nonce) and the tag HMAC-SHA256(key, "tag" ||
/// nonce || ct), truncated; base64url. For values up to 32 bytes.
pub mod seal {
    use super::*;
    use sha2::Sha256;

    fn prf(key: &[u8; 32], label: &[u8], parts: &[&[u8]]) -> [u8; 32] {
        let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("any key length");
        mac.update(label);
        for p in parts {
            mac.update(p);
        }
        mac.finalize().into_bytes().into()
    }

    pub fn seal(key: &[u8; 32], plain: &[u8]) -> String {
        assert!(plain.len() <= 32, "seal is for short secrets");
        let mut nonce = [0u8; 16];
        getrandom::fill(&mut nonce).expect("the OS random source failed");
        let stream = prf(key, b"enc", &[&nonce]);
        let ct: Vec<u8> = plain.iter().zip(stream).map(|(p, k)| p ^ k).collect();
        let tag = prf(key, b"tag", &[&nonce, &ct]);
        let mut out = nonce.to_vec();
        out.extend_from_slice(&ct);
        out.extend_from_slice(&tag[..16]);
        B64.encode(out)
    }

    pub fn open(key: &[u8; 32], sealed: &str) -> Option<Vec<u8>> {
        let raw = B64.decode(sealed).ok()?;
        if raw.len() < 32 || raw.len() > 64 {
            return None;
        }
        let (nonce, rest) = raw.split_at(16);
        let (ct, tag) = rest.split_at(rest.len() - 16);
        let want = prf(key, b"tag", &[nonce, ct]);
        // Constant-time enough for a 16-byte compare of a MAC.
        if want[..16]
            .iter()
            .zip(tag)
            .fold(0u8, |a, (x, y)| a | (x ^ y))
            != 0
        {
            return None;
        }
        let stream = prf(key, b"enc", &[nonce]);
        Some(ct.iter().zip(stream).map(|(c, k)| c ^ k).collect())
    }
}

/// Invite codes typed by people (docs/live/live-admin.md §5): generated
/// ones in the old Xbox Live shape, 25 characters in five groups of five
/// from an alphabet without vowels or look-alikes; custom ones anything
/// printable an admin picks. Both match ignoring case, spaces and dashes.
pub mod invite_code {
    /// The generated codes' alphabet: no vowels, no 0/O, 1/I/L, 5/S.
    pub const ALPHABET: &[u8; 24] = b"BCDFGHJKMPQRTVWXY2346789";
    pub const LEN: usize = 25;
    pub const MIN_CUSTOM: usize = 4;
    pub const MAX_CUSTOM: usize = 32;

    /// A generated code from (at least 25) pseudo-random bytes.
    pub fn from_bytes(bytes: &[u8]) -> String {
        let chars: Vec<u8> = bytes[..LEN]
            .iter()
            .map(|b| ALPHABET[usize::from(*b) % ALPHABET.len()])
            .collect();
        chars
            .chunks(5)
            .map(|g| std::str::from_utf8(g).expect("ASCII"))
            .collect::<Vec<_>>()
            .join("-")
    }

    /// The form codes are matched in: upper case without spaces or dashes.
    pub fn normalize(code: &str) -> String {
        code.chars()
            .filter(|c| !c.is_whitespace() && *c != '-')
            .map(|c| c.to_ascii_uppercase())
            .collect()
    }

    /// Whether an admin may pick `code`: printable ASCII, 4 to 32
    /// characters once normalised.
    pub fn valid_custom(code: &str) -> bool {
        let n = normalize(code);
        code.len() <= 64
            && (MIN_CUSTOM..=MAX_CUSTOM).contains(&n.len())
            && code.chars().all(|c| c.is_ascii_graphic() || c == ' ')
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc6238_sha1_vectors() {
        // RFC 6238 Appendix B, SHA-1 (key "12345678901234567890", 8 digits
        // there; the low six digits here).
        let s = TotpSecret::from_bytes(*b"12345678901234567890");
        for (t, eight) in [
            (59, 94287082u32),
            (1111111109, 7081804),
            (1111111111, 14050471),
            (1234567890, 89005924),
            (2000000000, 69279037),
            (20000000000, 65353130),
        ] {
            assert_eq!(s.code_at(t), eight % 1_000_000, "t={t}");
        }
        assert_eq!(s.base32(), "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ");
        assert_eq!(
            TotpSecret::from_base32("gezd gnbv gy3t qojq gezd gnbv gy3t qojq"),
            Some(s)
        );
        assert_eq!(TotpSecret::from_base32("GEZD"), None);
        assert_eq!(TotpSecret::from_base32("GEZ1"), None);
    }

    #[test]
    fn verify_allows_one_step_either_side() {
        let s = TotpSecret::generate();
        let now = 1_700_000_015;
        let step = step_of(now);
        assert_eq!(s.verify(s.code_at(now), now), Some(step));
        assert_eq!(s.verify(s.code_at(now - 30), now), Some(step - 1));
        assert_eq!(s.verify(s.code_at(now + 30), now), Some(step + 1));
        let far = s.code_at(now + 90);
        if ![-1, 0, 1].iter().any(|d| s.code_at_step(step + d) == far) {
            assert_eq!(s.verify(far, now), None);
        }
    }

    #[test]
    fn codes_format_and_parse() {
        assert_eq!(format_code(42), "000-042");
        assert_eq!(parse_code("000-042"), Some(42));
        assert_eq!(parse_code(" 123 456 "), Some(123456));
        assert_eq!(parse_code("12345"), None);
        assert_eq!(parse_code("1234567"), None);
        assert_eq!(parse_code("12a456"), None);
        let info = RecoveryInfo::new(
            &TotpSecret::from_bytes([7; 20]),
            "live.example",
            "Sneaky Pete",
            100,
        );
        assert_eq!(info.expires_at, 120);
        assert_eq!(info.period, 30);
        assert!(
            info.otpauth
                .starts_with("otpauth://totp/live.example:Sneaky%20Pete?secret=")
        );
        assert!(
            info.otpauth
                .contains("&issuer=live.example&algorithm=SHA1&digits=6&period=30")
        );
        assert!(!format!("{:?}", TotpSecret::from_bytes([7; 20])).contains(&info.secret));
    }

    #[test]
    fn a_recover_request_is_signed_by_the_key_it_adds() {
        let device = KeyPair::generate();
        let jws = RecoverRequest::sign(&device, "Alpha", "srv", 1000);
        let (req, key) = RecoverRequest::open(&jws, "srv", 1100).unwrap();
        assert_eq!((req.gamertag.as_str(), &key), ("Alpha", device.public()));
        assert!(RecoverRequest::open(&jws, "other", 1000).is_err());
        assert!(RecoverRequest::open(&jws, "srv", 1000 + 301).is_err());
        // Signed by another key than the one it carries.
        let forged = KeyPair::generate().sign_json(TYP_RECOVER, &req);
        assert!(RecoverRequest::open(&forged, "srv", 1000).is_err());
        let token = sign::issue_token(&device, "acct:x@srv", "srv", "read", 1000);
        assert!(RecoverRequest::open(&token, "srv", 1000).is_err());
    }

    #[test]
    fn sealed_secrets_open_only_with_their_key_and_unaltered() {
        let key = [9u8; 32];
        let sealed = seal::seal(&key, b"twenty bytes secret!");
        assert_eq!(seal::open(&key, &sealed).unwrap(), b"twenty bytes secret!");
        assert_ne!(
            seal::seal(&key, b"twenty bytes secret!"),
            sealed,
            "fresh nonce"
        );
        assert_eq!(seal::open(&[8u8; 32], &sealed), None);
        let mut raw = B64.decode(&sealed).unwrap();
        raw[20] ^= 1;
        assert_eq!(seal::open(&key, &B64.encode(raw)), None);
        assert_eq!(seal::open(&key, "short"), None);
    }

    #[test]
    fn invite_codes() {
        use invite_code::*;
        let code = from_bytes(&[
            0u8, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23,
            24,
        ]);
        assert_eq!(code, "BCDFG-HJKMP-QRTVW-XY234-6789B");
        assert_eq!(normalize("bcdfg hjkmp-qrtvw-xy234-6789b"), normalize(&code));
        assert!(valid_custom("Party Time"));
        assert!(valid_custom("abcd"));
        assert!(!valid_custom("abc"));
        assert!(!valid_custom(&"x".repeat(33)));
        assert!(!valid_custom("caf\u{e9}"));
        assert!(!valid_custom("tab\there"));
    }
}
