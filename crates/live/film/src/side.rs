//! The action side stream (docs/game/input-film.md, "The action side
//! stream"): a Live host forwards its film file to each client as it is
//! written, so a client's film is the host's, byte for byte.
//!
//! The host sends what its film file gained since the last send as one
//! numbered piece: the header and seed first, then the blocks a closing
//! tick chunk brought, and the trailer at the seal. A piece goes in parts
//! that fit a tunnel frame. The seal sends the sealed header, the only
//! bytes a film file rewrites. A client asks again for a piece it missed;
//! the host holds the last [`RING_PIECES`] pieces, at most [`RING_BYTES`],
//! for that. A piece over [`MAX_PIECE`] is not forwarded, and a piece the
//! host no longer holds is read from the host's film on Live after the game,
//! by the byte range its frames name. The client's film is whole when it
//! has every piece and the seal.
//!
//! ```text
//! frame, little-endian
//!    0   u8 kind (0 part of a piece, 1 piece not forwarded, 2 seal)
//!    1  16  film id (the header's)
//!   17   u32 piece index
//!   21   u64 offset of the piece in the film file (0 for the seal)
//!   29   u32 bytes in the piece (64 for the seal)
//!   33   u16 part, u16 parts
//!   37   the part's bytes
//! ```
use anyhow::{Result, bail, ensure};
use std::collections::{BTreeMap, VecDeque};
use std::ops::Range;

/// The largest frame: a tunnel datagram's payload (`live_proto::tunnel`).
pub const MAX_FRAME: usize = live_proto::tunnel::MAX_PAYLOAD;
const FRAME_HEADER: usize = 37;
pub const MAX_PART: usize = MAX_FRAME - FRAME_HEADER;
/// A piece over this is not forwarded; the client reads it from Live.
pub const MAX_PIECE: usize = 64 << 10;
pub const RING_PIECES: usize = 8;
pub const RING_BYTES: usize = 512 << 10;
const HEADER_BYTES: usize = crate::file::HEADER_BYTES;

const KIND_PART: u8 = 0;
const KIND_SKIPPED: u8 = 1;
const KIND_SEAL: u8 = 2;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Frame {
    /// One part of a forwarded piece.
    Part {
        film: [u8; 16],
        index: u32,
        offset: u64,
        len: u32,
        part: u16,
        parts: u16,
        bytes: Vec<u8>,
    },
    /// A piece the host does not forward: where it is in the film file.
    Skipped {
        film: [u8; 16],
        index: u32,
        offset: u64,
        len: u32,
    },
    /// The sealed header; `pieces` is how many pieces the film has.
    Seal {
        film: [u8; 16],
        pieces: u32,
        header: [u8; HEADER_BYTES],
    },
}

impl Frame {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(MAX_FRAME);
        match self {
            Self::Part {
                film,
                index,
                offset,
                len,
                part,
                parts,
                bytes,
            } => {
                head(
                    &mut out, KIND_PART, film, *index, *offset, *len, *part, *parts,
                );
                out.extend_from_slice(bytes);
            }
            Self::Skipped {
                film,
                index,
                offset,
                len,
            } => {
                head(&mut out, KIND_SKIPPED, film, *index, *offset, *len, 0, 0);
            }
            Self::Seal {
                film,
                pieces,
                header,
            } => {
                head(
                    &mut out,
                    KIND_SEAL,
                    film,
                    *pieces,
                    0,
                    HEADER_BYTES as u32,
                    0,
                    1,
                );
                out.extend_from_slice(header);
            }
        }
        out
    }

    pub fn decode(b: &[u8]) -> Result<Self> {
        ensure!(
            (FRAME_HEADER..=MAX_FRAME).contains(&b.len()),
            "side stream frame of {} bytes",
            b.len()
        );
        let film: [u8; 16] = b[1..17].try_into()?;
        let index = u32::from_le_bytes(b[17..21].try_into()?);
        let offset = u64::from_le_bytes(b[21..29].try_into()?);
        let len = u32::from_le_bytes(b[29..33].try_into()?);
        let part = u16::from_le_bytes(b[33..35].try_into()?);
        let parts = u16::from_le_bytes(b[35..37].try_into()?);
        let bytes = &b[FRAME_HEADER..];
        Ok(match b[0] {
            KIND_PART => {
                ensure!(
                    len as usize <= MAX_PIECE
                        && len > 0
                        && parts as usize == (len as usize).div_ceil(MAX_PART)
                        && part < parts
                        && bytes.len() == (len as usize - part as usize * MAX_PART).min(MAX_PART),
                    "side stream part out of its piece"
                );
                Self::Part {
                    film,
                    index,
                    offset,
                    len,
                    part,
                    parts,
                    bytes: bytes.to_vec(),
                }
            }
            KIND_SKIPPED => {
                ensure!(bytes.is_empty() && len > 0, "side stream skip with bytes");
                Self::Skipped {
                    film,
                    index,
                    offset,
                    len,
                }
            }
            KIND_SEAL => {
                ensure!(
                    bytes.len() == HEADER_BYTES && &bytes[..8] == crate::file::MAGIC,
                    "side stream seal without a film header"
                );
                Self::Seal {
                    film,
                    pieces: index,
                    header: bytes.try_into()?,
                }
            }
            kind => bail!("side stream frame of unknown kind {kind}"),
        })
    }
}

#[allow(clippy::too_many_arguments)]
fn head(
    out: &mut Vec<u8>,
    kind: u8,
    film: &[u8; 16],
    index: u32,
    offset: u64,
    len: u32,
    part: u16,
    parts: u16,
) {
    out.push(kind);
    out.extend_from_slice(film);
    out.extend_from_slice(&index.to_le_bytes());
    out.extend_from_slice(&offset.to_le_bytes());
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&part.to_le_bytes());
    out.extend_from_slice(&parts.to_le_bytes());
}

fn split(film: [u8; 16], index: u32, offset: u64, piece: &[u8]) -> Vec<Frame> {
    let n = piece.len().div_ceil(MAX_PART);
    piece
        .chunks(MAX_PART)
        .enumerate()
        .map(|(part, bytes)| Frame::Part {
            film,
            index,
            offset,
            len: piece.len() as u32,
            part: part as u16,
            parts: n as u16,
            bytes: bytes.to_vec(),
        })
        .collect()
}

/// The host's end: the film file's growth in, frames out.
pub struct Forwarder {
    film: [u8; 16],
    next: u32,
    sent: u64,
    /// Each piece's offset and length, for a client to read one from Live.
    places: Vec<(u64, u32)>,
    ring: VecDeque<(u32, u64, Vec<u8>)>,
    held: usize,
}

impl Forwarder {
    /// `head` is the film file as written so far: its header and seed.
    pub fn new(head: &[u8]) -> Result<(Self, Vec<Frame>)> {
        ensure!(
            head.len() > HEADER_BYTES && &head[..8] == crate::file::MAGIC,
            "a side stream starts at a film header and seed"
        );
        let mut this = Self {
            film: head[16..32].try_into()?,
            next: 0,
            sent: 0,
            places: Vec::new(),
            ring: VecDeque::new(),
            held: 0,
        };
        let frames = this.forward(head);
        Ok((this, frames))
    }

    /// The bytes the film file gained since the last call, as one piece.
    pub fn forward(&mut self, grown: &[u8]) -> Vec<Frame> {
        if grown.is_empty() {
            return Vec::new();
        }
        let (index, offset) = (self.next, self.sent);
        self.next += 1;
        self.sent += grown.len() as u64;
        self.places.push((offset, grown.len() as u32));
        if grown.len() > MAX_PIECE {
            return vec![Frame::Skipped {
                film: self.film,
                index,
                offset,
                len: grown.len() as u32,
            }];
        }
        self.ring.push_back((index, offset, grown.to_vec()));
        self.held += grown.len();
        while self.ring.len() > RING_PIECES || self.held > RING_BYTES {
            let (_, _, old) = self.ring.pop_front().expect("a held piece");
            self.held -= old.len();
        }
        split(self.film, index, offset, grown)
    }

    /// A piece again: its parts if the host still holds it, else where it
    /// is in the film file. `None` for a piece not yet sent.
    pub fn resend(&self, index: u32) -> Option<Vec<Frame>> {
        if let Some((_, offset, piece)) = self.ring.iter().find(|(i, _, _)| *i == index) {
            return Some(split(self.film, index, *offset, piece));
        }
        let &(offset, len) = self.places.get(index as usize)?;
        Some(vec![Frame::Skipped {
            film: self.film,
            index,
            offset,
            len,
        }])
    }

    /// The film is sealed: its final header.
    pub fn seal(&self, header: &[u8; HEADER_BYTES]) -> Frame {
        Frame::Seal {
            film: self.film,
            pieces: self.next,
            header: *header,
        }
    }
}

struct Piece {
    offset: u64,
    len: u32,
    parts: Vec<Option<Vec<u8>>>,
}

impl Piece {
    fn whole(&self) -> bool {
        !self.parts.is_empty() && self.parts.iter().all(Option::is_some)
    }
}

/// A client's end: frames in, in any order and with gaps, the host's film
/// out once it is whole.
#[derive(Default)]
pub struct Assembler {
    film: Option<[u8; 16]>,
    pieces: BTreeMap<u32, Piece>,
    seal: Option<(u32, [u8; HEADER_BYTES])>,
}

impl Assembler {
    pub fn accept(&mut self, frame: Frame) -> Result<()> {
        let id = match &frame {
            Frame::Part { film, .. } | Frame::Skipped { film, .. } | Frame::Seal { film, .. } => {
                *film
            }
        };
        ensure!(
            *self.film.get_or_insert(id) == id,
            "a side stream frame of another film"
        );
        match frame {
            Frame::Part {
                index,
                offset,
                len,
                part,
                parts,
                bytes,
                ..
            } => {
                let piece = self.pieces.entry(index).or_insert_with(|| Piece {
                    offset,
                    len,
                    parts: vec![None; parts as usize],
                });
                ensure!(
                    piece.offset == offset && piece.len == len,
                    "side stream piece {index} changed"
                );
                if piece.parts.is_empty() {
                    // Named as skipped before; its bytes came after all.
                    piece.parts = vec![None; parts as usize];
                }
                ensure!(
                    piece.parts.len() == parts as usize,
                    "side stream piece {index} changed"
                );
                piece.parts[part as usize] = Some(bytes);
            }
            Frame::Skipped {
                index, offset, len, ..
            } => {
                self.pieces.entry(index).or_insert(Piece {
                    offset,
                    len,
                    parts: Vec::new(),
                });
            }
            Frame::Seal { pieces, header, .. } => self.seal = Some((pieces, header)),
        }
        Ok(())
    }

    /// A piece read some other way (from the host's film on Live).
    pub fn fill(&mut self, index: u32, bytes: &[u8]) -> Result<()> {
        let piece = self
            .pieces
            .get_mut(&index)
            .ok_or_else(|| anyhow::anyhow!("side stream piece {index} is not known"))?;
        ensure!(
            bytes.len() == piece.len as usize,
            "side stream piece {index} has another length"
        );
        piece.parts = split([0; 16], index, piece.offset, bytes)
            .into_iter()
            .map(|frame| match frame {
                Frame::Part { bytes, .. } => Some(bytes),
                _ => None,
            })
            .collect();
        Ok(())
    }

    /// Pieces to ask the host for again: every one before the last seen
    /// (or, once sealed, before the end) that is not whole and was sent.
    pub fn missing(&self) -> Vec<u32> {
        let end = match self.seal {
            Some((pieces, _)) => pieces,
            None => self.pieces.keys().next_back().map_or(0, |last| last + 1),
        };
        (0..end)
            .filter(|i| match self.pieces.get(i) {
                None => true,
                Some(piece) => !piece.parts.is_empty() && !piece.whole(),
            })
            .collect()
    }

    /// Pieces the host did not forward, with where they are in its film.
    pub fn wanted(&self) -> Vec<(u32, Range<u64>)> {
        self.pieces
            .iter()
            .filter(|(_, piece)| piece.parts.is_empty())
            .map(|(&i, piece)| (i, piece.offset..piece.offset + u64::from(piece.len)))
            .collect()
    }

    /// The host's film, once every piece and the seal are here.
    pub fn film(&self) -> Option<Vec<u8>> {
        let (pieces, header) = self.seal.as_ref()?;
        let mut out = Vec::new();
        for i in 0..*pieces {
            let piece = self.pieces.get(&i).filter(|p| p.whole())?;
            if piece.offset != out.len() as u64 {
                return None;
            }
            for part in &piece.parts {
                out.extend_from_slice(part.as_ref()?);
            }
        }
        if out.len() < HEADER_BYTES {
            return None;
        }
        out[..HEADER_BYTES].copy_from_slice(header);
        Some(out)
    }
}

#[cfg(test)]
mod tests;
