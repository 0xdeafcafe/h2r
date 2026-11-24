//! The carrier's three datagram shapes. Integers are little endian.
//!
//! - hello (client to host): kind 1, version, client X25519 key (32), nonce (8)
//! - answer (host to client): kind 2, version, peer id (4), nonce (8),
//!   host X25519 key (32), signature (64)
//! - data (host to client): kind 3, peer id (4), one actor-channel packet,
//!   tag (8)

pub const VERSION: u8 = 1;
pub const HELLO: u8 = 1;
pub const ANSWER: u8 = 2;
pub const DATA: u8 = 3;
pub const TAG_LEN: usize = 8;
pub const HELLO_LEN: usize = 2 + 32 + 8;
pub const ANSWER_LEN: usize = 2 + 4 + 8 + 32 + 64;
pub const DATA_PREFIX: usize = 1 + 4;
/// The largest datagram: an actor-channel packet at its cap, framed.
pub const MAX_DATAGRAM: usize = DATA_PREFIX + actor_net::wire::MAX_PACKET_BYTES + TAG_LEN;

/// Why a datagram was dropped, counted by reason.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum Reason {
    /// The wrong length, kind or version.
    Malformed,
    /// A client sent the host something that is not a hello.
    NotAHello,
    /// The host has [`crate::MAX_PEERS`] peers.
    HostFull,
    /// An answer that is not for the client's last hello, or whose
    /// signature the session key did not make.
    BadAnswer,
    /// A key agreement that yields the all-zero secret.
    WeakKey,
    /// A data datagram before the answer, or for another peer.
    NoChannel,
    /// The tag does not verify.
    BadTag,
    /// A tick not newer than the last accepted.
    Stale,
    /// The packet inside does not decode.
    BadPacket,
}

pub const DROPS: [Reason; 9] = [
    Reason::Malformed,
    Reason::NotAHello,
    Reason::HostFull,
    Reason::BadAnswer,
    Reason::WeakKey,
    Reason::NoChannel,
    Reason::BadTag,
    Reason::Stale,
    Reason::BadPacket,
];

/// Serial-number ordering of ticks, including the u32 wrap.
pub fn newer_tick(tick: u32, previous: u32) -> bool {
    let distance = tick.wrapping_sub(previous);
    distance != 0 && distance < (1 << 31)
}
