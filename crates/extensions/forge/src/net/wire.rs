//! The bytes a machine puts on the session's reliable ordered channel
//! (docs/game/forge-design.md, F5): length-prefixed JSON messages, and the
//! beat in the keep-alive.
use forge_doc::{LogHead, Refusal, Sha256};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use super::{Beat, MAX_WELCOME_BYTES, NetRefusal, Reply, Stamped, Up, Welcome};

/// The beat as the keep-alive carries it: the log head's sequence, tick and
/// hash, and the stream position (little endian).
pub const BEAT_BYTES: usize = 4 + 4 + 32 + 8;

/// The largest message a frame may hold: a welcome and its framing.
pub const MAX_FRAME_BYTES: usize = MAX_WELCOME_BYTES + 4096;

impl Beat {
    pub fn to_bytes(&self) -> [u8; BEAT_BYTES] {
        let mut out = [0; BEAT_BYTES];
        out[0..4].copy_from_slice(&self.head.seq.to_le_bytes());
        out[4..8].copy_from_slice(&self.head.tick.to_le_bytes());
        out[8..40].copy_from_slice(&self.head.hash.0);
        out[40..48].copy_from_slice(&self.n.to_le_bytes());
        out
    }

    pub fn from_bytes(bytes: &[u8; BEAT_BYTES]) -> Self {
        let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().expect("4 bytes"));
        Self {
            head: LogHead {
                seq: u32_at(0),
                tick: u32_at(4),
                hash: Sha256(bytes[8..40].try_into().expect("32 bytes")),
            },
            n: u64::from_le_bytes(bytes[40..48].try_into().expect("8 bytes")),
        }
    }
}

/// A machine to the host.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ToHost {
    /// First on the lane: the machine's name. The seat is the lane's.
    Hello { name: String },
    Up(Up),
}

/// The host to a machine.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Down {
    /// First on the lane, after a [`ToHost::Hello`] the host took.
    Welcome(Box<Welcome>),
    /// One item of the stream.
    Item(Stamped),
    /// The answer to a propose or hold.
    Reply(Reply),
    /// The host ends this machine's session, and says why.
    Ended(NetRefusal),
}

/// A message as one frame: its length (u32, little endian), then its JSON.
pub fn frame(message: &impl Serialize) -> Result<Vec<u8>, Refusal> {
    let body = serde_json::to_vec(message).map_err(|e| Refusal::Json(e.to_string()))?;
    if body.len() > MAX_FRAME_BYTES {
        return Err(Refusal::Limit {
            what: "message bytes".into(),
            max: MAX_FRAME_BYTES,
        });
    }
    let mut out = Vec::with_capacity(4 + body.len());
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

/// Cuts the lane's byte stream back into messages.
#[derive(Debug, Default)]
pub struct Framer {
    buf: Vec<u8>,
}

impl Framer {
    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// The next whole message, `None` while the rest has not arrived. An
    /// error ends the lane: the stream cannot be cut again after it.
    pub fn pop<T: DeserializeOwned>(&mut self) -> Result<Option<T>, Refusal> {
        let Some(head) = self.buf.first_chunk::<4>() else {
            return Ok(None);
        };
        let len = u32::from_le_bytes(*head) as usize;
        if len > MAX_FRAME_BYTES {
            return Err(Refusal::Limit {
                what: "message bytes".into(),
                max: MAX_FRAME_BYTES,
            });
        }
        if self.buf.len() < 4 + len {
            return Ok(None);
        }
        let message = serde_json::from_slice(&self.buf[4..4 + len])
            .map_err(|e| Refusal::Json(e.to_string()))?;
        self.buf.drain(..4 + len);
        Ok(Some(message))
    }

    /// Bytes held that no whole message has used.
    pub fn held(&self) -> usize {
        self.buf.len()
    }
}
