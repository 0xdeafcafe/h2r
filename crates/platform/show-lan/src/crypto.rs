use curve25519_dalek::{constants::X25519_BASEPOINT, montgomery::MontgomeryPoint};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::wire::TAG_LEN;

const DOMAIN_ANSWER: &[u8] = b"h2 show lan v1 answer";
const INFO_KEY: &[u8] = b"h2 show lan v1 key";

pub fn public_key(secret: &[u8; 32]) -> [u8; 32] {
    X25519_BASEPOINT.mul_clamped(*secret).to_bytes()
}

/// The shared secret; none for a low-order peer key (all zero).
fn shared(secret: &[u8; 32], peer_public: &[u8; 32]) -> Option<[u8; 32]> {
    let shared = MontgomeryPoint(*peer_public).mul_clamped(*secret).to_bytes();
    (shared != [0; 32]).then_some(shared)
}

/// The peer's channel key.
pub fn channel_key(
    secret: &[u8; 32],
    peer_public: &[u8; 32],
    session: &[u8; 16],
    peer: u32,
) -> Option<[u8; 32]> {
    let shared = shared(secret, peer_public)?;
    let mut salt = [0u8; 20];
    salt[..16].copy_from_slice(session);
    salt[16..].copy_from_slice(&peer.to_le_bytes());
    let mut key = [0u8; 32];
    Hkdf::<Sha256>::new(Some(&salt), &shared)
        .expand(INFO_KEY, &mut key)
        .ok()?;
    Some(key)
}

/// What the host's session key signs in an answer: the session, the peer,
/// the hello's nonce and both public keys.
pub fn answer_message(
    session: &[u8; 16],
    peer: u32,
    nonce: &[u8; 8],
    client_public: &[u8; 32],
    host_public: &[u8; 32],
) -> Vec<u8> {
    let mut msg = Vec::with_capacity(DOMAIN_ANSWER.len() + 16 + 4 + 8 + 64);
    msg.extend_from_slice(DOMAIN_ANSWER);
    msg.extend_from_slice(session);
    msg.extend_from_slice(&peer.to_le_bytes());
    msg.extend_from_slice(nonce);
    msg.extend_from_slice(client_public);
    msg.extend_from_slice(host_public);
    msg
}

fn mac(key: &[u8; 32], bytes: &[u8]) -> Hmac<Sha256> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC takes any key length");
    mac.update(bytes);
    mac
}

pub fn tag(key: &[u8; 32], bytes: &[u8]) -> [u8; TAG_LEN] {
    let full = mac(key, bytes).finalize().into_bytes();
    let mut tag = [0u8; TAG_LEN];
    tag.copy_from_slice(&full[..TAG_LEN]);
    tag
}

/// Constant-time check of a truncated tag.
pub fn verify(key: &[u8; 32], bytes: &[u8], tag: &[u8]) -> bool {
    mac(key, bytes).verify_truncated_left(tag).is_ok()
}
