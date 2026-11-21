//! Keys, compact JWS, signed events, self-issued tokens and invites
//! (docs/live/live-security.md §1-§3). Everything here is pure: callers supply
//! the time and the key lookups, so the server can verify without I/O and
//! the game-side client and CLI sign with exactly the same code.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::jcs;

/// JWS `typ` of a signed event body.
pub const TYP_EVENT: &str = "h2live-event";
/// JWS `typ` of an account-creation request.
pub const TYP_ACCOUNT: &str = "h2live-account";
/// JWS `typ` of an account-recovery request ([`crate::recovery`]).
pub const TYP_RECOVER: &str = "h2live-recover";
/// JWS `typ` of a server-signed invite code.
pub const TYP_INVITE: &str = "h2live-invite";
/// JWS `typ` of a request token.
pub const TYP_TOKEN: &str = "JWT";

/// How far a signed event's `at` may be from the server's clock.
pub const MAX_EVENT_SKEW_S: i64 = 5 * 60;
/// The longest a request token may live (`exp - iat`).
pub const MAX_TOKEN_LIFETIME_S: i64 = 60;
/// Clock skew allowed on token `iat`/`exp`.
pub const TOKEN_LEEWAY_S: i64 = 5;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SignError {
    #[error("malformed JWS: {0}")]
    Malformed(&'static str),
    #[error("JWS alg must be EdDSA")]
    Alg,
    #[error("JWS typ is {got:?}, expected {want:?}")]
    Typ {
        got: Option<String>,
        want: &'static str,
    },
    #[error("signed payload is not canonical JSON (RFC 8785)")]
    NotCanonical,
    #[error("signature does not verify")]
    BadSignature,
    #[error("kid does not match the key")]
    KidMismatch,
    #[error("not an Ed25519 JWK")]
    BadJwk,
    #[error("{0}")]
    Claims(&'static str),
}

fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).expect("the OS random source failed");
    b
}

/// A random URL-safe identifier (nonces, `jti`s): 128 bits.
pub fn random_id() -> String {
    B64.encode(random::<16>())
}

/// An Ed25519 public key as a JWK (RFC 8037). `kid`, when present, is the
/// RFC 7638 thumbprint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Jwk {
    pub kty: String,
    pub crv: String,
    pub x: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kid: Option<String>,
}

/// An Ed25519 private key as a JWK (RFC 8037: `d` is the 32-byte seed).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrivateJwk {
    pub kty: String,
    pub crv: String,
    #[serde(default)]
    pub x: String,
    pub d: String,
}

impl std::fmt::Debug for PrivateJwk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrivateJwk")
            .field("x", &self.x)
            .finish_non_exhaustive()
    }
}

/// A verifying key with its thumbprint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicKey {
    key: VerifyingKey,
    kid: String,
}

impl PublicKey {
    pub fn from_jwk(jwk: &Jwk) -> Result<Self, SignError> {
        if jwk.kty != "OKP" || jwk.crv != "Ed25519" {
            return Err(SignError::BadJwk);
        }
        let bytes: [u8; 32] = B64
            .decode(&jwk.x)
            .ok()
            .and_then(|b| b.try_into().ok())
            .ok_or(SignError::BadJwk)?;
        let key = VerifyingKey::from_bytes(&bytes).map_err(|_| SignError::BadJwk)?;
        if key.is_weak() {
            return Err(SignError::BadJwk);
        }
        let pk = Self::from_key(key);
        if jwk.kid.as_ref().is_some_and(|k| *k != pk.kid) {
            return Err(SignError::KidMismatch);
        }
        Ok(pk)
    }

    fn from_key(key: VerifyingKey) -> Self {
        let x = B64.encode(key.as_bytes());
        // RFC 7638: the required members in lexicographic order, no spaces.
        let thumb = format!(r#"{{"crv":"Ed25519","kty":"OKP","x":"{x}"}}"#);
        PublicKey {
            key,
            kid: B64.encode(Sha256::digest(thumb)),
        }
    }

    pub fn kid(&self) -> &str {
        &self.kid
    }

    /// The JWK, with `kid`.
    pub fn jwk(&self) -> Jwk {
        Jwk {
            kty: "OKP".into(),
            crv: "Ed25519".into(),
            x: B64.encode(self.key.as_bytes()),
            kid: Some(self.kid.clone()),
        }
    }

    fn verify(&self, signing_input: &[u8], sig: &Signature) -> Result<(), SignError> {
        self.key
            .verify_strict(signing_input, sig)
            .map_err(|_| SignError::BadSignature)
    }
}

/// A private signing key: an account key or the server key.
pub struct KeyPair {
    secret: SigningKey,
    public: PublicKey,
}

impl KeyPair {
    pub fn generate() -> Self {
        Self::from_secret(random::<32>())
    }

    pub fn from_secret(bytes: [u8; 32]) -> Self {
        let secret = SigningKey::from_bytes(&bytes);
        let public = PublicKey::from_key(secret.verifying_key());
        KeyPair { secret, public }
    }

    /// The 32-byte seed as base64url (the JWK `d`), for key files.
    pub fn secret_b64(&self) -> String {
        B64.encode(self.secret.to_bytes())
    }

    pub fn from_secret_b64(s: &str) -> Result<Self, SignError> {
        let bytes: [u8; 32] = B64
            .decode(s.trim())
            .ok()
            .and_then(|b| b.try_into().ok())
            .ok_or(SignError::BadJwk)?;
        Ok(Self::from_secret(bytes))
    }

    pub fn public(&self) -> &PublicKey {
        &self.public
    }

    /// A 32-byte key for `label` derived from this key's seed with
    /// HKDF-SHA256 ([`crate::recovery::seal::derive_key`]).
    pub fn derive_key(&self, label: &str) -> [u8; 32] {
        crate::recovery::seal::derive_key(&self.secret.to_bytes(), label)
    }

    /// The private JWK (`d` set), the form WebCrypto exports.
    pub fn private_jwk(&self) -> PrivateJwk {
        let p = self.public.jwk();
        PrivateJwk {
            kty: p.kty,
            crv: p.crv,
            x: p.x,
            d: self.secret_b64(),
        }
    }

    /// From a private JWK; `x`, when present, must be `d`'s public key.
    pub fn from_private_jwk(jwk: &PrivateJwk) -> Result<Self, SignError> {
        if jwk.kty != "OKP" || jwk.crv != "Ed25519" {
            return Err(SignError::BadJwk);
        }
        let key = Self::from_secret_b64(&jwk.d)?;
        if !jwk.x.is_empty() && jwk.x != key.public.jwk().x {
            return Err(SignError::KidMismatch);
        }
        Ok(key)
    }

    pub fn kid(&self) -> &str {
        self.public.kid()
    }

    /// Compact JWS over `payload` with header `{alg, kid, typ}`.
    pub fn sign_compact(&self, typ: &str, payload: &[u8]) -> String {
        let header = Header {
            alg: "EdDSA".into(),
            kid: Some(self.kid().into()),
            typ: Some(typ.into()),
        };
        let header = jcs::to_canonical(&header).expect("header serializes");
        let input = format!("{}.{}", B64.encode(header), B64.encode(payload));
        let sig = self.secret.sign(input.as_bytes());
        format!("{input}.{}", B64.encode(sig.to_bytes()))
    }

    /// Compact JWS over the canonical JSON of `body`.
    pub fn sign_json<T: Serialize>(&self, typ: &str, body: &T) -> String {
        let payload = jcs::to_canonical(body).expect("signed bodies serialize");
        self.sign_compact(typ, payload.as_bytes())
    }
}

impl std::fmt::Debug for KeyPair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyPair")
            .field("kid", &self.kid())
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Header {
    pub alg: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub typ: Option<String>,
}

/// A parsed, not yet verified, compact JWS.
#[derive(Debug, Clone)]
pub struct Jws {
    pub header: Header,
    pub payload: Vec<u8>,
    signing_input: String,
    signature: Signature,
}

impl Jws {
    /// Parses a compact JWS. Rejects anything but `alg: EdDSA` and any
    /// `crit` header, so nothing downstream can be talked into another
    /// algorithm.
    pub fn parse(compact: &str) -> Result<Self, SignError> {
        let mut parts = compact.split('.');
        let (Some(h), Some(p), Some(s), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(SignError::Malformed("expected three dot-separated parts"));
        };
        let decode = |part: &str| {
            B64.decode(part)
                .map_err(|_| SignError::Malformed("bad base64url"))
        };
        let header_json: Value = serde_json::from_slice(&decode(h)?)
            .map_err(|_| SignError::Malformed("header is not JSON"))?;
        if header_json.get("crit").is_some() {
            return Err(SignError::Malformed("crit headers are not supported"));
        }
        let header: Header =
            serde_json::from_value(header_json).map_err(|_| SignError::Malformed("bad header"))?;
        if header.alg != "EdDSA" {
            return Err(SignError::Alg);
        }
        let sig: [u8; 64] = decode(s)?
            .try_into()
            .map_err(|_| SignError::Malformed("signature is not 64 bytes"))?;
        Ok(Jws {
            header,
            payload: decode(p)?,
            signing_input: format!("{h}.{p}"),
            signature: Signature::from_bytes(&sig),
        })
    }

    /// Checks `typ`, and that the payload is canonical JSON (so a signed
    /// body has exactly one byte form), and returns it parsed.
    pub fn json_payload(&self, want_typ: &'static str) -> Result<Value, SignError> {
        if self.header.typ.as_deref() != Some(want_typ) {
            return Err(SignError::Typ {
                got: self.header.typ.clone(),
                want: want_typ,
            });
        }
        let value: Value = serde_json::from_slice(&self.payload)
            .map_err(|_| SignError::Malformed("payload is not JSON"))?;
        if jcs::canonicalize(&value).as_bytes() != self.payload.as_slice() {
            return Err(SignError::NotCanonical);
        }
        Ok(value)
    }

    pub fn kid(&self) -> Option<&str> {
        self.header.kid.as_deref()
    }

    /// Verifies the signature with `key`; the header `kid`, if any, must
    /// name it.
    pub fn verify(&self, key: &PublicKey) -> Result<(), SignError> {
        if self.kid().is_some_and(|k| k != key.kid()) {
            return Err(SignError::KidMismatch);
        }
        key.verify(self.signing_input.as_bytes(), &self.signature)
    }
}

fn from_value<T: serde::de::DeserializeOwned>(v: Value) -> Result<T, SignError> {
    serde_json::from_value(v).map_err(|_| SignError::Malformed("payload has the wrong shape"))
}

/// RFC 3339 UTC with millisecond precision, the log's time format.
pub fn format_time(t: time::OffsetDateTime) -> String {
    let t = t.to_offset(time::UtcOffset::UTC);
    let t = t
        .replace_nanosecond(t.millisecond() as u32 * 1_000_000)
        .expect("valid nanos");
    t.format(&time::format_description::well_known::Rfc3339)
        .expect("formats")
}

pub fn parse_time(s: &str) -> Option<time::OffsetDateTime> {
    time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
}

/// [`format_time`] for a unix timestamp, for callers (the CLI) that keep
/// seconds rather than a clock type.
pub fn format_unix(t: i64) -> String {
    format_time(time::OffsetDateTime::from_unix_timestamp(t).expect("a timestamp in range"))
}

/// What an author signs for one event (security §2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventBody {
    pub stream: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub data: Value,
    pub author: String,
    pub kid: String,
    pub at: String,
    pub nonce: String,
}

impl EventBody {
    /// A fresh body by `key`'s holder, stamped `at` with a random nonce.
    pub fn new(
        key: &KeyPair,
        author: &str,
        stream: &str,
        kind: &str,
        data: Value,
        at: time::OffsetDateTime,
    ) -> Self {
        EventBody {
            stream: stream.into(),
            kind: kind.into(),
            data,
            author: author.into(),
            kid: key.kid().into(),
            at: format_time(at),
            nonce: random_id(),
        }
    }

    /// [`EventBody::new`] for a unix timestamp in seconds.
    pub fn at_unix(
        key: &KeyPair,
        author: &str,
        stream: &str,
        kind: &str,
        data: Value,
        t: i64,
    ) -> Self {
        Self::new(
            key,
            author,
            stream,
            kind,
            data,
            time::OffsetDateTime::from_unix_timestamp(t).expect("a timestamp in range"),
        )
    }

    pub fn sign(&self, key: &KeyPair) -> String {
        key.sign_json(TYP_EVENT, self)
    }
}

/// A signed event opened but not verified: the caller looks the author's
/// key up by `body.author` and `body.kid`, then calls [`SignedEvent::verify`].
#[derive(Debug, Clone)]
pub struct SignedEvent {
    pub jws: Jws,
    pub body: EventBody,
}

impl SignedEvent {
    pub fn open(compact: &str) -> Result<Self, SignError> {
        let jws = Jws::parse(compact)?;
        let body: EventBody = from_value(jws.json_payload(TYP_EVENT)?)?;
        if jws.kid() != Some(body.kid.as_str()) {
            return Err(SignError::KidMismatch);
        }
        Ok(SignedEvent { jws, body })
    }

    pub fn verify(&self, key: &PublicKey) -> Result<(), SignError> {
        self.jws.verify(key)
    }

    /// Opens a batch: one JWS over an array of bodies, all by one author
    /// and key (security §2, "Batches").
    pub fn open_batch(compact: &str) -> Result<(Jws, Vec<EventBody>), SignError> {
        let jws = Jws::parse(compact)?;
        let bodies: Vec<EventBody> = from_value(jws.json_payload(TYP_EVENT)?)?;
        let Some(first) = bodies.first() else {
            return Err(SignError::Malformed("empty batch"));
        };
        if bodies
            .iter()
            .any(|b| b.author != first.author || b.kid != first.kid)
            || jws.kid() != Some(first.kid.as_str())
        {
            return Err(SignError::KidMismatch);
        }
        Ok((jws, bodies))
    }

    /// The body at `index` of a stored JWS: the whole payload when `index`
    /// is `None`, else that element of a batch.
    pub fn body_at(compact: &str, index: Option<usize>) -> Result<EventBody, SignError> {
        match index {
            None => Ok(Self::open(compact)?.body),
            Some(i) => Self::open_batch(compact)?
                .1
                .into_iter()
                .nth(i)
                .ok_or(SignError::Malformed("jws_index out of range")),
        }
    }
}

/// Signs several bodies as one batch JWS.
pub fn sign_batch(key: &KeyPair, bodies: &[EventBody]) -> String {
    key.sign_json(TYP_EVENT, &bodies)
}

/// The per-stream hash chain: `hex(sha256(prev || utf8(jws)))`, where
/// `prev` is the raw 32 bytes of the stream's previous hash, and empty for
/// its first event. A malformed `prev_hex` hashes as empty.
pub fn chain_hash(prev_hex: Option<&str>, jws: &str) -> String {
    let mut h = Sha256::new();
    if let Some(p) = prev_hex {
        let bytes: Option<Vec<u8>> = (0..p.len() / 2)
            .map(|i| u8::from_str_radix(p.get(i * 2..i * 2 + 2)?, 16).ok())
            .collect();
        h.update(bytes.unwrap_or_default());
    }
    h.update(jws.as_bytes());
    hex(&h.finalize())
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Self-issued request token claims (security §3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claims {
    pub iss: String,
    pub aud: String,
    pub iat: i64,
    pub exp: i64,
    pub jti: String,
    pub lxm: String,
}

/// Signs a token for one request: `exp` is `iat` + 60 s.
pub fn issue_token(key: &KeyPair, iss: &str, aud: &str, lxm: &str, now: i64) -> String {
    let claims = Claims {
        iss: iss.into(),
        aud: aud.into(),
        iat: now,
        exp: now + MAX_TOKEN_LIFETIME_S,
        jti: random_id(),
        lxm: lxm.into(),
    };
    key.sign_json(TYP_TOKEN, &claims)
}

/// A token parsed but not verified.
#[derive(Debug, Clone)]
pub struct Token {
    pub jws: Jws,
    pub claims: Claims,
}

impl Token {
    pub fn parse(compact: &str) -> Result<Self, SignError> {
        let jws = Jws::parse(compact)?;
        let claims = from_value(jws.json_payload(TYP_TOKEN)?)?;
        Ok(Token { jws, claims })
    }

    /// Verifies the signature and every claim except `jti` freshness, which
    /// needs the server's replay set.
    pub fn verify(&self, key: &PublicKey, aud: &str, lxm: &str, now: i64) -> Result<(), SignError> {
        self.jws.verify(key)?;
        let c = &self.claims;
        if c.aud != aud {
            return Err(SignError::Claims("token aud is not this server"));
        }
        if c.lxm != lxm {
            return Err(SignError::Claims("token lxm does not name this method"));
        }
        if c.exp < c.iat || c.exp.abs_diff(c.iat) > MAX_TOKEN_LIFETIME_S as u64 {
            return Err(SignError::Claims("token lifetime exceeds 60 s"));
        }
        if c.iat > now.saturating_add(TOKEN_LEEWAY_S) {
            return Err(SignError::Claims("token issued in the future"));
        }
        if c.exp < now.saturating_sub(TOKEN_LEEWAY_S) {
            return Err(SignError::Claims("token expired"));
        }
        Ok(())
    }
}

/// `POST /v1/accounts`: signed by the new key itself (proof of possession).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountRequest {
    pub gamertag: String,
    pub jwk: Jwk,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invite: Option<String>,
    /// The server id it is for, so it cannot be replayed elsewhere.
    pub aud: String,
    /// When it was signed (unix seconds); the server allows
    /// [`MAX_EVENT_SKEW_S`] either way.
    pub iat: i64,
}

impl AccountRequest {
    /// Operator approval for this exact signed registration (name, device, invite and nonce).
    pub fn operator_scope(jws: &str) -> String {
        format!("accounts.reserve.{}", hex(&Sha256::digest(jws.as_bytes())))
    }

    pub fn sign(
        key: &KeyPair,
        gamertag: &str,
        invite: Option<String>,
        aud: &str,
        iat: i64,
    ) -> String {
        let body = AccountRequest {
            gamertag: gamertag.into(),
            jwk: key.public().jwk(),
            invite,
            aud: aud.into(),
            iat,
        };
        key.sign_json(TYP_ACCOUNT, &body)
    }

    /// Opens and verifies a creation request against the key it carries,
    /// and checks it is for `aud` and recent.
    pub fn open(compact: &str, aud: &str, now: i64) -> Result<(Self, PublicKey), SignError> {
        let jws = Jws::parse(compact)?;
        let req: AccountRequest = from_value(jws.json_payload(TYP_ACCOUNT)?)?;
        let key = PublicKey::from_jwk(&req.jwk)?;
        if jws.kid().is_none() {
            return Err(SignError::KidMismatch);
        }
        jws.verify(&key)?;
        if req.aud != aud {
            return Err(SignError::Claims("account request is for another server"));
        }
        if req.iat.abs_diff(now) > MAX_EVENT_SKEW_S as u64 {
            return Err(SignError::Claims(
                "account request iat is more than 5 minutes off",
            ));
        }
        Ok((req, key))
    }
}

/// An invite code: a JWS by the server key (`h2 live invite create`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invite {
    pub aud: String,
    pub jti: String,
    pub exp: i64,
}

impl Invite {
    pub fn issue(server_key: &KeyPair, server_id: &str, exp: i64) -> String {
        server_key.sign_json(
            TYP_INVITE,
            &Invite {
                aud: server_id.into(),
                jti: random_id(),
                exp,
            },
        )
    }

    /// Verifies an invite against the server key; returns its `jti`.
    pub fn open(
        compact: &str,
        server_key: &PublicKey,
        server_id: &str,
        now: i64,
    ) -> Result<Self, SignError> {
        let jws = Jws::parse(compact)?;
        let invite: Invite = from_value(jws.json_payload(TYP_INVITE)?)?;
        jws.verify(server_key)?;
        if invite.aud != server_id {
            return Err(SignError::Claims("invite is for another server"));
        }
        if invite.exp < now {
            return Err(SignError::Claims("invite expired"));
        }
        Ok(invite)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NOW: i64 = 1_800_000_000;
    const ACCOUNT_VECTOR: &str = concat!(
        "eyJhbGciOiJFZERTQSIsImtpZCI6ImtQcktfcW14VldhWVZBOXd3QkY2SXVvM3ZWeno3VHhIQ1R3WEJ5Z3JTNGsiLCJ0eXAiOiJoMmxpdmUtYWNjb3VudCJ9.",
        "eyJhdWQiOiJsb2NhbGhvc3QiLCJnYW1lcnRhZyI6IlNuZWFreSBQZXRlIiwiaWF0IjoxODAwMDAwMDAwLCJqd2siOnsiY3J2IjoiRWQyNTUxOSIsImtpZCI6ImtQcktfcW14VldhWVZBOXd3QkY2SXVvM3ZWeno3VHhIQ1R3WEJ5Z3JTNGsiLCJrdHkiOiJPS1AiLCJ4IjoiMTFxWUFZS3hDcmZWU183VHlXUUhPZzdoY3ZQYXBpTWxyd0lhYVBjSFVSbyJ9fQ.",
        "CAbhCapHV4aPxyp0SgMosKYqEXCVZvD5fOSx8sUQo9IpOUII91YGE75JFkE2QN0aojf0I5zQH1lZN3ehfQOGBg"
    );

    #[test]
    fn rfc_8037_vectors() {
        // RFC 8037 appendix A.1-A.4.
        let d = "nWGxne_9WmC6hEr0kuwsxERJxWl7MmkZcDusAxyuf2A";
        let key = KeyPair::from_secret_b64(d).unwrap();
        assert_eq!(
            key.public().jwk().x,
            "11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo"
        );
        assert_eq!(key.kid(), "kPrK_qmxVWaYVA9wwBF6Iuo3vVzz7TxHCTwXBygrS4k");
        let jws = "eyJhbGciOiJFZERTQSJ9.RXhhbXBsZSBvZiBFZDI1NTE5IHNpZ25pbmc.hgyY0il_MGCjP0JzlnLWG1PPOt7-09PGcvMg3AIbQR6dWbhijcNR4ki4iylGjg5BhVsPt9g7sVvpAr_MuM0KAg";
        let parsed = Jws::parse(jws).unwrap();
        assert_eq!(parsed.payload, b"Example of Ed25519 signing");
        parsed.verify(key.public()).unwrap();
        assert_eq!(key.secret_b64(), d);
    }

    #[test]
    fn jwk_round_trip_and_rejects() {
        let key = KeyPair::generate();
        let jwk = key.public().jwk();
        assert_eq!(PublicKey::from_jwk(&jwk).unwrap(), *key.public());
        let wrong_kid = Jwk {
            kid: Some("nope".into()),
            ..jwk.clone()
        };
        assert_eq!(PublicKey::from_jwk(&wrong_kid), Err(SignError::KidMismatch));
        for bad in [
            Jwk {
                kty: "EC".into(),
                ..jwk.clone()
            },
            Jwk {
                crv: "X25519".into(),
                ..jwk.clone()
            },
            Jwk {
                x: "AAAA".into(),
                ..jwk.clone()
            },
            // The identity point is a weak key.
            Jwk {
                x: B64.encode(
                    [1u8]
                        .iter()
                        .chain([0u8; 31].iter())
                        .copied()
                        .collect::<Vec<_>>(),
                ),
                kid: None,
                ..jwk.clone()
            },
        ] {
            assert!(PublicKey::from_jwk(&bad).is_err(), "{bad:?}");
        }
    }

    fn event(key: &KeyPair) -> (EventBody, String) {
        let at = time::OffsetDateTime::from_unix_timestamp(NOW).unwrap();
        let body = EventBody::new(
            key,
            "acct:0009000000000001@h",
            "presence-0009000000000001",
            "SignedIn",
            json!({"state": "online"}),
            at,
        );
        let jws = body.sign(key);
        (body, jws)
    }

    #[test]
    fn signed_event_verifies() {
        let key = KeyPair::generate();
        let (body, jws) = event(&key);
        let opened = SignedEvent::open(&jws).unwrap();
        assert_eq!(opened.body, body);
        opened.verify(key.public()).unwrap();
        assert_eq!(body.at, "2027-01-15T08:00:00Z");
    }

    #[test]
    fn signed_event_rejects_wrong_key_and_tampering() {
        let key = KeyPair::generate();
        let (body, jws) = event(&key);

        // Another key, even with the right kid claimed.
        let other = KeyPair::generate();
        let opened = SignedEvent::open(&jws).unwrap();
        assert_eq!(opened.verify(other.public()), Err(SignError::KidMismatch));

        // A body re-signed by another key but claiming the first key's kid.
        let forged = other.sign_json(TYP_EVENT, &body);
        assert_eq!(
            SignedEvent::open(&forged).unwrap_err(),
            SignError::KidMismatch
        );

        // A tampered payload, kept canonical.
        let mut evil = body.clone();
        evil.data = json!({"state": "away"});
        let payload = B64.encode(jcs::to_canonical(&evil).unwrap());
        let parts: Vec<&str> = jws.split('.').collect();
        let tampered = format!("{}.{payload}.{}", parts[0], parts[2]);
        let opened = SignedEvent::open(&tampered).unwrap();
        assert_eq!(opened.verify(key.public()), Err(SignError::BadSignature));

        // A non-canonical payload, validly signed.
        let loose = key.sign_compact(
            TYP_EVENT,
            serde_json::to_string_pretty(&body).unwrap().as_bytes(),
        );
        assert_eq!(
            SignedEvent::open(&loose).unwrap_err(),
            SignError::NotCanonical
        );

        // A token is not an event.
        let token = issue_token(&key, "a", "b", "c", NOW);
        assert!(matches!(
            SignedEvent::open(&token),
            Err(SignError::Typ { .. })
        ));

        // alg confusion.
        let header = B64.encode(r#"{"alg":"none","kid":"x","typ":"h2live-event"}"#);
        let none = format!("{header}.{}.{}", parts[1], parts[2]);
        assert_eq!(SignedEvent::open(&none).unwrap_err(), SignError::Alg);
        assert!(Jws::parse("a.b").is_err());
        assert!(Jws::parse("a.b.c.d").is_err());
    }

    #[test]
    fn tokens() {
        let key = KeyPair::generate();
        let pk = key.public();
        let t = issue_token(&key, "acct:x@h", "h", "events.write", NOW);
        let token = Token::parse(&t).unwrap();
        assert_eq!(token.claims.exp, NOW + 60);
        token.verify(pk, "h", "events.write", NOW + 30).unwrap();

        assert_eq!(
            token.verify(pk, "other", "events.write", NOW),
            Err(SignError::Claims("token aud is not this server"))
        );
        assert!(token.verify(pk, "h", "titles.write", NOW).is_err());
        assert_eq!(
            token.verify(pk, "h", "events.write", NOW + 66),
            Err(SignError::Claims("token expired"))
        );
        assert!(token.verify(pk, "h", "events.write", NOW - 10).is_err());
        assert!(
            token
                .verify(KeyPair::generate().public(), "h", "events.write", NOW)
                .is_err()
        );

        // A token claiming to live longer than 60 s.
        let long = Claims {
            iss: "i".into(),
            aud: "h".into(),
            iat: NOW,
            exp: NOW + 61,
            jti: "j".into(),
            lxm: "m".into(),
        };
        for (iat, exp) in [(i64::MIN, NOW), (NOW, i64::MAX), (i64::MAX, i64::MIN)] {
            let claims = Claims {
                iat,
                exp,
                ..long.clone()
            };
            let t = Token::parse(&key.sign_json(TYP_TOKEN, &claims)).unwrap();
            assert!(t.verify(pk, "h", "m", NOW).is_err());
        }
        let long = Token::parse(&key.sign_json(TYP_TOKEN, &long)).unwrap();
        assert_eq!(
            long.verify(pk, "h", "m", NOW),
            Err(SignError::Claims("token lifetime exceeds 60 s"))
        );
    }

    #[test]
    fn account_requests() {
        let key = KeyPair::generate();
        let jws = AccountRequest::sign(&key, "Sneaky Pete", Some("inv".into()), "h", NOW);
        let (req, pk) = AccountRequest::open(&jws, "h", NOW + 10).unwrap();
        assert_eq!(req.gamertag, "Sneaky Pete");
        assert_eq!(req.invite.as_deref(), Some("inv"));
        assert_eq!(pk, *key.public());

        // Another server, or stale.
        assert!(AccountRequest::open(&jws, "other", NOW).is_err());
        assert!(AccountRequest::open(&jws, "h", NOW + 301).is_err());
        for at in [i64::MIN, i64::MAX] {
            let stale = AccountRequest::sign(&key, "Example", None, "h", at);
            assert!(AccountRequest::open(&stale, "h", NOW).is_err());
        }
        // Signed by a key other than the one it carries.
        let other = KeyPair::generate();
        let body = AccountRequest {
            gamertag: "X".into(),
            jwk: key.public().jwk(),
            invite: None,
            aud: "h".into(),
            iat: NOW,
        };
        let wrong = other.sign_json(TYP_ACCOUNT, &body);
        assert!(AccountRequest::open(&wrong, "h", NOW).is_err());
    }

    /// A fixed vector for cross-checking a browser (WebCrypto Ed25519 is
    /// deterministic too): RFC 8037's key signing a creation request.
    /// Header `{"alg":"EdDSA","kid":<thumbprint>,"typ":"h2live-account"}`
    /// and payload are JCS; a client producing the same JSON gets the same
    /// bytes.
    #[test]
    fn account_request_vector() {
        let jwk = PrivateJwk {
            kty: "OKP".into(),
            crv: "Ed25519".into(),
            x: "11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo".into(),
            d: "nWGxne_9WmC6hEr0kuwsxERJxWl7MmkZcDusAxyuf2A".into(),
        };
        let key = KeyPair::from_private_jwk(&jwk).unwrap();
        let jws = AccountRequest::sign(&key, "Sneaky Pete", None, "localhost", 1_800_000_000);
        let parts: Vec<&str> = jws.split('.').collect();
        assert_eq!(
            String::from_utf8(B64.decode(parts[0]).unwrap()).unwrap(),
            r#"{"alg":"EdDSA","kid":"kPrK_qmxVWaYVA9wwBF6Iuo3vVzz7TxHCTwXBygrS4k","typ":"h2live-account"}"#
        );
        assert_eq!(
            String::from_utf8(B64.decode(parts[1]).unwrap()).unwrap(),
            r#"{"aud":"localhost","gamertag":"Sneaky Pete","iat":1800000000,"jwk":{"crv":"Ed25519","kid":"kPrK_qmxVWaYVA9wwBF6Iuo3vVzz7TxHCTwXBygrS4k","kty":"OKP","x":"11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo"}}"#
        );
        assert_eq!(jws, ACCOUNT_VECTOR);
        AccountRequest::open(ACCOUNT_VECTOR, "localhost", 1_800_000_000).unwrap();
        assert_eq!(key.private_jwk(), jwk);
        let bad = PrivateJwk {
            x: KeyPair::generate().public().jwk().x,
            ..jwk
        };
        assert_eq!(
            KeyPair::from_private_jwk(&bad).unwrap_err(),
            SignError::KidMismatch
        );
    }

    #[test]
    fn invites() {
        let server = KeyPair::generate();
        let code = Invite::issue(&server, "live.example", NOW + 100);
        Invite::open(&code, server.public(), "live.example", NOW).unwrap();
        assert!(Invite::open(&code, server.public(), "other", NOW).is_err());
        assert!(Invite::open(&code, server.public(), "live.example", NOW + 101).is_err());
        assert!(Invite::open(&code, KeyPair::generate().public(), "live.example", NOW).is_err());
    }

    #[test]
    fn hash_chain() {
        // First event: sha256 of the JWS alone.
        let a = chain_hash(None, "one");
        assert_eq!(a, hex(&Sha256::digest(b"one")));
        // Then the previous hash's raw bytes, then the JWS.
        let b = chain_hash(Some(&a), "two");
        let mut h = Sha256::new();
        h.update(Sha256::digest(b"one"));
        h.update(b"two");
        assert_eq!(b, hex(&h.finalize()));
        assert_ne!(chain_hash(Some(&a), "two!"), b);
        assert_ne!(chain_hash(Some(&b), "two"), b);
    }

    #[test]
    fn batches() {
        let key = KeyPair::generate();
        let at = time::OffsetDateTime::from_unix_timestamp(NOW).unwrap();
        let body = |n: u64| {
            EventBody::new(
                &key,
                "acct:a@h",
                "game-1",
                "PlayerKilled",
                json!({"tick": n}),
                at,
            )
        };
        let bodies = vec![body(1), body(2)];
        let jws = sign_batch(&key, &bodies);
        let (parsed, opened) = SignedEvent::open_batch(&jws).unwrap();
        parsed.verify(key.public()).unwrap();
        assert_eq!(opened, bodies);
        assert_eq!(SignedEvent::body_at(&jws, Some(1)).unwrap(), bodies[1]);
        assert!(SignedEvent::body_at(&jws, Some(2)).is_err());
        assert!(SignedEvent::body_at(&jws, None).is_err());
        assert!(SignedEvent::open_batch(&sign_batch(&key, &[])).is_err());
        // Mixed authors are not one batch.
        let mut odd = body(3);
        odd.author = "acct:b@h".into();
        assert!(SignedEvent::open_batch(&sign_batch(&key, &[body(1), odd])).is_err());
    }

    #[test]
    fn times() {
        let t = time::OffsetDateTime::from_unix_timestamp_nanos(1_800_000_000_123_456_789).unwrap();
        assert_eq!(format_time(t), "2027-01-15T08:00:00.123Z");
        assert_eq!(
            parse_time("2027-01-15T08:00:00.123Z")
                .unwrap()
                .unix_timestamp(),
            NOW
        );
        assert!(parse_time("yesterday").is_none());
    }
}
