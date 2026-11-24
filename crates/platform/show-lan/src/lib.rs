//! The show channel's LAN carrier (docs/game/rules-world-api.md, "The show
//! channel", System Link; slice W11).
//!
//! A System Link session has no Live tunnel, so the actor channel's records
//! ride datagrams of ours beside the game's own. The carrier is one way: a
//! client sends a hello and nothing else, and the host reads nothing but a
//! hello. It is the Live channel's handshake and tag over a plain datagram
//! socket:
//!
//! - the host holds an Ed25519 session key; the client is given its public
//!   half (not learned from the wire, so a peer on the LAN cannot offer its
//!   own);
//! - a hello carries the client's X25519 public key and a nonce; the answer
//!   carries the host's, the peer id and a signature over both;
//! - both derive one key (HKDF-SHA-256 over the shared secret, the session
//!   id and the peer id); every data datagram ends with an 8-byte
//!   HMAC-SHA-256 tag over all before it;
//! - a data datagram's tick is newer than the last accepted, so a recorded
//!   one cannot be replayed.
//!
//! The carrier never draws entropy, reads a clock or opens a socket by
//! itself: secrets, the session id, the time and the [`Datagrams`] it rides
//! come from the caller. Which addresses are peers, and how a client finds
//! the host and its public key, are the caller's: the research row R10
//! (whether the HLE's sockets carry a datagram of ours) is not settled.

mod client;
mod crypto;
mod host;
mod transport;
mod wire;

pub use client::{Client, Event};
pub use host::Host;
pub use transport::Datagrams;
pub use wire::{DROPS, Reason};

/// The most peers a host answers; the game's own limit is 16 machines.
pub const MAX_PEERS: usize = 15;
/// Dropped datagrams in [`CLOSE_WINDOW_MS`] that close a client's channel.
pub const CLOSE_AFTER_DROPS: usize = 30;
pub const CLOSE_WINDOW_MS: u64 = 1_000;

#[cfg(test)]
mod tests;
