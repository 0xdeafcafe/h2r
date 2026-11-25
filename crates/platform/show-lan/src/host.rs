use std::collections::BTreeMap;
use std::io;
use std::net::SocketAddr;

use actor_net::{Packet, Record, WireError, encode};
use ed25519_dalek::{Signer, SigningKey};

use crate::crypto;
use crate::transport::Datagrams;
use crate::wire::{self, Reason};

struct Peer {
    id: u32,
    key: [u8; 32],
    last_tick: Option<u32>,
    needs_key: bool,
}

/// The host's end: answers hellos and sends packets to the peers that
/// asked. It reads nothing else.
pub struct Host<T: Datagrams> {
    transport: T,
    session: [u8; 16],
    signing: SigningKey,
    entropy: Box<dyn FnMut() -> [u8; 32] + Send>,
    peers: BTreeMap<SocketAddr, Peer>,
    next_id: u32,
    drops: BTreeMap<Reason, u64>,
}

impl<T: Datagrams> Host<T> {
    /// `signing` is the session key whose public half the clients hold;
    /// `entropy` yields a fresh 32-byte secret for each answer.
    pub fn new(
        transport: T,
        session: [u8; 16],
        signing: SigningKey,
        entropy: impl FnMut() -> [u8; 32] + Send + 'static,
    ) -> Self {
        Self {
            transport,
            session,
            signing,
            entropy: Box::new(entropy),
            peers: BTreeMap::new(),
            next_id: 1,
            drops: BTreeMap::new(),
        }
    }

    fn drop_one(&mut self, reason: Reason) {
        *self.drops.entry(reason).or_default() += 1;
    }

    /// Datagrams dropped so far, by reason.
    pub fn drops(&self) -> &BTreeMap<Reason, u64> {
        &self.drops
    }

    /// Read what is waiting before the tick: answer each hello. A second
    /// hello from a peer makes a new key and asks for a fresh `Key`.
    pub fn poll(&mut self) -> io::Result<()> {
        let mut buf = [0u8; wire::MAX_DATAGRAM + 1];
        // A bounded read: a flood costs one tick at most this much.
        for _ in 0..64 {
            let Some((len, from)) = self.transport.recv_from(&mut buf)? else {
                break;
            };
            let bytes = &buf[..len.min(buf.len())];
            if bytes.first() != Some(&wire::HELLO) {
                self.drop_one(if bytes.is_empty() {
                    Reason::Malformed
                } else {
                    Reason::NotAHello
                });
                continue;
            }
            if bytes.len() != wire::HELLO_LEN || bytes[1] != wire::VERSION {
                self.drop_one(Reason::Malformed);
                continue;
            }
            let mut client_public = [0u8; 32];
            client_public.copy_from_slice(&bytes[2..34]);
            let mut nonce = [0u8; 8];
            nonce.copy_from_slice(&bytes[34..42]);
            self.answer(from, client_public, nonce)?;
        }
        Ok(())
    }

    fn answer(
        &mut self,
        from: SocketAddr,
        client_public: [u8; 32],
        nonce: [u8; 8],
    ) -> io::Result<()> {
        let id = match self.peers.get(&from) {
            Some(peer) => peer.id,
            None if self.peers.len() >= crate::MAX_PEERS => {
                self.drop_one(Reason::HostFull);
                return Ok(());
            }
            None => {
                let id = self.next_id;
                self.next_id = self.next_id.wrapping_add(1).max(1);
                id
            }
        };
        let secret = (self.entropy)();
        let host_public = crypto::public_key(&secret);
        let Some(key) = crypto::channel_key(&secret, &client_public, &self.session, id) else {
            self.drop_one(Reason::WeakKey);
            return Ok(());
        };
        let message =
            crypto::answer_message(&self.session, id, &nonce, &client_public, &host_public);
        let signature = self.signing.sign(&message);
        let mut out = Vec::with_capacity(wire::ANSWER_LEN);
        out.extend([wire::ANSWER, wire::VERSION]);
        out.extend(id.to_le_bytes());
        out.extend(nonce);
        out.extend(host_public);
        out.extend(signature.to_bytes());
        self.transport.send_to(&out, from)?;
        self.peers.insert(
            from,
            Peer {
                id,
                key,
                last_tick: None,
                needs_key: true,
            },
        );
        Ok(())
    }

    /// The peers that have a channel, and whether each needs a `Key` at
    /// this tick (the shape of the Live tunnel's `actor_tunnel_peers`).
    pub fn peers(&self) -> Vec<(u32, bool)> {
        self.peers.values().map(|p| (p.id, p.needs_key)).collect()
    }

    /// Send one packet to a peer. False when the peer is unknown or the
    /// tick is not newer than the last sent to it. A packet that holds a
    /// `Key` clears the peer's need for one.
    pub fn send(&mut self, peer: u32, packet: &Packet) -> Result<bool, WireError> {
        let Some((addr, p)) = self.peers.iter_mut().find(|(_, p)| p.id == peer) else {
            return Ok(false);
        };
        if p.last_tick.is_some_and(|last| !wire::newer_tick(packet.tick, last)) {
            return Ok(false);
        }
        let body = encode(packet)?;
        let mut out = Vec::with_capacity(wire::DATA_PREFIX + body.len() + wire::TAG_LEN);
        out.push(wire::DATA);
        out.extend(p.id.to_le_bytes());
        out.extend(body);
        let tag = crypto::tag(&p.key, &out);
        out.extend(tag);
        // A lost datagram is the channel's normal case; the repeats and the
        // key period cover it.
        let _ = self.transport.send_to(&out, *addr);
        p.last_tick = Some(packet.tick);
        if packet.records.iter().any(|r| matches!(r, Record::Key { .. })) {
            p.needs_key = false;
        }
        Ok(true)
    }

    /// Forget every peer (a new match or a left game).
    pub fn reset(&mut self) {
        self.peers.clear();
    }
}
