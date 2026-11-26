use std::collections::VecDeque;
use std::io;
use std::net::SocketAddr;

use actor_net::{Packet, decode};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};

use crate::crypto;
use crate::transport::Datagrams;
use crate::wire::{self, DROPS, Reason};
use crate::{CLOSE_AFTER_DROPS, CLOSE_WINDOW_MS};

struct Channel {
    id: u32,
    key: [u8; 32],
    last_tick: Option<u32>,
    /// The counter of the last pick sent.
    picks: u32,
}

/// What one datagram did.
#[derive(Debug, Eq, PartialEq)]
pub enum Event {
    /// The host's answer verified: the channel is up.
    Established,
    Packet(Packet),
    Dropped(Reason),
    /// Too many drops in a second: the channel is shut for the session.
    Closed,
}

/// A System Link guest's end. It sends hellos and applies what verifies.
pub struct Client<T: Datagrams> {
    transport: T,
    host: SocketAddr,
    host_key: VerifyingKey,
    session: [u8; 16],
    secret: [u8; 32],
    hellos: u64,
    channel: Option<Channel>,
    recent_drops: VecDeque<u64>,
    counts: [u64; DROPS.len()],
    closed: bool,
}

impl<T: Datagrams> Client<T> {
    /// `host_key` is the host's session key, known from a source the LAN
    /// peers cannot write; `secret` is this machine's fresh X25519 secret.
    pub fn new(
        transport: T,
        host: SocketAddr,
        host_key: VerifyingKey,
        session: [u8; 16],
        secret: [u8; 32],
    ) -> Self {
        Self {
            transport,
            host,
            host_key,
            session,
            secret,
            hellos: 0,
            channel: None,
            recent_drops: VecDeque::new(),
            counts: [0; DROPS.len()],
            closed: false,
        }
    }

    /// Send a hello. The first starts the channel; a later one, after loss
    /// or a missed `Key`, makes a new key and asks the host for a `Key`.
    pub fn hello(&mut self) -> io::Result<()> {
        if self.closed {
            return Ok(());
        }
        self.hellos += 1;
        let mut out = Vec::with_capacity(wire::HELLO_LEN);
        out.extend([wire::HELLO, wire::VERSION]);
        out.extend(crypto::public_key(&self.secret));
        out.extend(self.hellos.to_le_bytes());
        self.transport.send_to(&out, self.host)
    }

    /// Send one pick (the sending machine's id, then one to 16 message
    /// bytes) behind the channel's tag. False before the channel is up,
    /// after it closed and for a message of the wrong length; a datagram
    /// the LAN loses is the picker's to ask again.
    pub fn send_pick(&mut self, machine: [u8; wire::PICK_MACHINE], message: &[u8]) -> io::Result<bool> {
        if self.closed || !(1..=wire::PICK_MESSAGE_MAX).contains(&message.len()) {
            return Ok(false);
        }
        let Some(channel) = &mut self.channel else {
            return Ok(false);
        };
        channel.picks = channel.picks.wrapping_add(1);
        let mut out = Vec::with_capacity(wire::PICK_PREFIX + wire::PICK_MACHINE + message.len() + wire::TAG_LEN);
        out.push(wire::PICK);
        out.extend(channel.id.to_le_bytes());
        out.extend(channel.picks.to_le_bytes());
        out.extend(machine);
        out.extend(message);
        let tag = crypto::tag(&channel.key, &out);
        out.extend(tag);
        self.transport.send_to(&out, self.host)?;
        Ok(true)
    }

    pub fn established(&self) -> bool {
        self.channel.is_some() && !self.closed
    }

    pub fn closed(&self) -> bool {
        self.closed
    }

    /// Datagrams dropped so far, by reason.
    pub fn drops(&self, reason: Reason) -> u64 {
        DROPS
            .iter()
            .position(|&r| r == reason)
            .map_or(0, |i| self.counts[i])
    }

    /// Drain the socket before the tick; `now_ms` is a monotonic clock.
    /// Returns what verified, in arrival order.
    pub fn poll(&mut self, now_ms: u64) -> io::Result<Vec<Event>> {
        let mut events = Vec::new();
        let mut buf = [0u8; wire::MAX_DATAGRAM + 1];
        for _ in 0..64 {
            let Some((len, from)) = self.transport.recv_from(&mut buf)? else {
                break;
            };
            if self.closed {
                continue;
            }
            // Only the host's address speaks to a client.
            let event = if from != self.host {
                Event::Dropped(Reason::NoChannel)
            } else {
                self.receive(&buf[..len.min(buf.len())])
            };
            if let Event::Dropped(reason) = event {
                if let Some(i) = DROPS.iter().position(|&r| r == reason) {
                    self.counts[i] += 1;
                }
                self.recent_drops.push_back(now_ms);
                while self
                    .recent_drops
                    .front()
                    .is_some_and(|&at| now_ms.saturating_sub(at) >= CLOSE_WINDOW_MS)
                {
                    self.recent_drops.pop_front();
                }
                if self.recent_drops.len() >= CLOSE_AFTER_DROPS {
                    self.closed = true;
                    self.channel = None;
                    events.push(event);
                    events.push(Event::Closed);
                    continue;
                }
            }
            events.push(event);
        }
        Ok(events)
    }

    fn receive(&mut self, bytes: &[u8]) -> Event {
        match bytes.first() {
            Some(&wire::ANSWER) => self.answer(bytes),
            Some(&wire::DATA) => self.data(bytes),
            _ => Event::Dropped(Reason::Malformed),
        }
    }

    fn answer(&mut self, bytes: &[u8]) -> Event {
        if bytes.len() != wire::ANSWER_LEN || bytes[1] != wire::VERSION {
            return Event::Dropped(Reason::Malformed);
        }
        let id = u32::from_le_bytes(bytes[2..6].try_into().unwrap());
        let nonce: [u8; 8] = bytes[6..14].try_into().unwrap();
        let host_public: [u8; 32] = bytes[14..46].try_into().unwrap();
        let signature: [u8; 64] = bytes[46..110].try_into().unwrap();
        // Only the answer to the latest hello counts.
        if u64::from_le_bytes(nonce) != self.hellos {
            return Event::Dropped(Reason::BadAnswer);
        }
        let message = crypto::answer_message(
            &self.session,
            id,
            &nonce,
            &crypto::public_key(&self.secret),
            &host_public,
        );
        if self
            .host_key
            .verify(&message, &Signature::from_bytes(&signature))
            .is_err()
        {
            return Event::Dropped(Reason::BadAnswer);
        }
        let Some(key) = crypto::channel_key(&self.secret, &host_public, &self.session, id) else {
            return Event::Dropped(Reason::WeakKey);
        };
        self.channel = Some(Channel {
            id,
            key,
            last_tick: None,
            picks: 0,
        });
        Event::Established
    }

    fn data(&mut self, bytes: &[u8]) -> Event {
        if bytes.len() < wire::DATA_PREFIX + wire::TAG_LEN {
            return Event::Dropped(Reason::Malformed);
        }
        let Some(channel) = &mut self.channel else {
            return Event::Dropped(Reason::NoChannel);
        };
        let id = u32::from_le_bytes(bytes[1..5].try_into().unwrap());
        if id != channel.id {
            return Event::Dropped(Reason::NoChannel);
        }
        let (body, tag) = bytes.split_at(bytes.len() - wire::TAG_LEN);
        if !crypto::verify(&channel.key, body, tag) {
            return Event::Dropped(Reason::BadTag);
        }
        let Ok(packet) = decode(&body[wire::DATA_PREFIX..]) else {
            return Event::Dropped(Reason::BadPacket);
        };
        if channel
            .last_tick
            .is_some_and(|last| !wire::newer_tick(packet.tick, last))
        {
            return Event::Dropped(Reason::Stale);
        }
        channel.last_tick = Some(packet.tick);
        Event::Packet(packet)
    }
}
