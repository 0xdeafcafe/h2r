//! A Forge session in a film (docs/game/forge-design.md, "Films and
//! determinism"): the session's welcome at its start, and each item of the
//! host's stream at the tick the machine applied it. The records are the
//! messages' bytes; the film's tag and framing are the game's
//! (`live_proto::film::stream::FORGE`). Playback feeds them to a [`Peer`],
//! which applies them as the session did.
use std::fmt;

use forge_doc::Limits;

use crate::look::film::{self as looks, Playback};

use super::{Item, MAX_MESSAGE_BYTES, MAX_WELCOME_BYTES, Peer, PeerError, Stamped, Welcome};

/// The first byte of a record's payload: the session's welcome.
pub const WELCOME: u8 = 0;
/// The first byte of a record's payload: one stream item.
pub const ITEM: u8 = 1;

/// The payload of the record that opens a session in a film.
pub fn welcome_payload(welcome: &Welcome) -> Result<Vec<u8>, forge_doc::Refusal> {
    let mut out = vec![WELCOME];
    out.extend(welcome.to_bytes()?);
    Ok(out)
}

/// The payload of the record for an item, which goes at the tick the
/// machine applied it. A watermark is not recorded: playback makes each
/// tick's own.
pub fn item_payload(item: &Stamped) -> Result<Option<Vec<u8>>, forge_doc::Refusal> {
    if item.item == Item::Through {
        return Ok(None);
    }
    let mut out = vec![ITEM];
    out.extend(item.to_bytes()?);
    Ok(Some(out))
}

/// Why a film's Forge records cannot be replayed.
#[derive(Debug, Clone, PartialEq)]
pub enum FilmError {
    /// A payload that is not a welcome or an item.
    Record(String),
    /// An item before the film's welcome, or a second welcome.
    Order(&'static str),
    Peer(PeerError),
}

impl fmt::Display for FilmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Record(reason) => write!(f, "The film's Forge record is not readable: {reason}"),
            Self::Order(reason) => write!(f, "The film's Forge records are out of order: {reason}"),
            Self::Peer(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for FilmError {}

impl From<PeerError> for FilmError {
    fn from(e: PeerError) -> Self {
        Self::Peer(e)
    }
}

/// A film's Forge records, replayed through a peer. Look and pose records
/// (`look::film::LOOK`, `POSE`) are presentation: they go to [`Playback`],
/// not the peer, and need no welcome before them.
pub struct Replay {
    limits: Limits,
    peer: Option<Peer>,
    presence: Playback,
}

impl Replay {
    pub fn new(limits: Limits) -> Self {
        Self {
            limits,
            peer: None,
            presence: Playback::default(),
        }
    }

    /// The peer the records feed, once the welcome has come.
    pub fn peer(&self) -> Option<&Peer> {
        self.peer.as_ref()
    }

    /// The monitor looks and poses the film's records have given so far.
    pub fn presence(&self) -> &Playback {
        &self.presence
    }

    /// Take the welcome, item, look or pose one record holds.
    pub fn feed(&mut self, payload: &[u8]) -> Result<(), FilmError> {
        let (&kind, bytes) = payload
            .split_first()
            .ok_or_else(|| FilmError::Record("empty".into()))?;
        match kind {
            WELCOME => {
                if self.peer.is_some() {
                    return Err(FilmError::Order("a second welcome"));
                }
                if bytes.len() > MAX_WELCOME_BYTES {
                    return Err(FilmError::Record("welcome too large".into()));
                }
                let welcome =
                    Welcome::from_bytes(bytes).map_err(|e| FilmError::Record(e.to_string()))?;
                self.peer = Some(Peer::join(welcome, self.limits.clone())?);
                Ok(())
            }
            ITEM => {
                if bytes.len() > MAX_MESSAGE_BYTES {
                    return Err(FilmError::Record("item too large".into()));
                }
                let peer = self
                    .peer
                    .as_mut()
                    .ok_or(FilmError::Order("an item before the welcome"))?;
                let item =
                    Stamped::from_bytes(bytes).map_err(|e| FilmError::Record(e.to_string()))?;
                // The watermarks were not recorded: each is a gap in `n`.
                while peer.next_n() < item.n {
                    peer.receive(Stamped {
                        n: peer.next_n(),
                        tick: item.tick,
                        item: Item::Through,
                    })?;
                }
                peer.receive(item)?;
                Ok(())
            }
            looks::LOOK | looks::POSE => self
                .presence
                .feed(payload)
                .map_err(|e| FilmError::Record(e.to_string())),
            other => Err(FilmError::Record(format!("unknown kind {other}"))),
        }
    }

    /// The items due at `tick`, in stream order, for the game to apply.
    /// The film has fed every record of the tick, so nothing more is due
    /// at it or before it.
    pub fn due(&mut self, tick: u32) -> Result<Vec<Stamped>, FilmError> {
        let Some(peer) = self.peer.as_mut() else {
            return Ok(vec![]);
        };
        peer.release_through(tick);
        Ok(peer.due(tick)?)
    }
}
