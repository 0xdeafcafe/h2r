//! Dedicated host datagrams, one frame per binary WebSocket message.

pub const MAX_PAYLOAD: usize = 1264;
pub const HEADER_LEN: usize = 8;
pub const MAX_PORT: u16 = 6;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Kind {
    Datagram = 0,
    Hello = 1,
    Bye = 2,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Frame {
    pub kind: Kind,
    /// Destination channel offset from the XNet base port.
    pub port: u16,
    pub peer: u32,
    pub payload: Vec<u8>,
}

#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum TunnelError {
    #[error("truncated tunnel frame ({0} bytes)")]
    Truncated(usize),
    #[error("unsupported tunnel version {0}")]
    Version(u8),
    #[error("unknown tunnel kind {0}")]
    Kind(u8),
    #[error("invalid tunnel port offset {0}")]
    Port(u16),
    #[error("tunnel payload too large ({0} bytes)")]
    Oversize(usize),
}

/// Serializes a frame. Callers enforce the channel and payload limits before sending.
pub fn encode(frame: &Frame) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(HEADER_LEN + frame.payload.len());
    bytes.extend_from_slice(&[1, frame.kind as u8]);
    bytes.extend_from_slice(&frame.port.to_le_bytes());
    bytes.extend_from_slice(&frame.peer.to_le_bytes());
    bytes.extend_from_slice(&frame.payload);
    bytes
}

pub fn decode(bytes: &[u8]) -> Result<Frame, TunnelError> {
    let Some((header, payload)) = bytes.split_at_checked(HEADER_LEN) else {
        return Err(TunnelError::Truncated(bytes.len()));
    };
    if header[0] != 1 {
        return Err(TunnelError::Version(header[0]));
    }
    let kind = match header[1] {
        0 => Kind::Datagram,
        1 => Kind::Hello,
        2 => Kind::Bye,
        other => return Err(TunnelError::Kind(other)),
    };
    let port = u16::from_le_bytes([header[2], header[3]]);
    if port > MAX_PORT {
        return Err(TunnelError::Port(port));
    }
    if payload.len() > MAX_PAYLOAD {
        return Err(TunnelError::Oversize(payload.len()));
    }
    Ok(Frame {
        kind,
        port,
        peer: u32::from_le_bytes([header[4], header[5], header[6], header[7]]),
        payload: payload.to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn golden_datagram_uses_little_endian_fields_and_raw_payload() {
        let frame = Frame {
            kind: Kind::Datagram,
            port: 6,
            peer: 0x12345678,
            payload: vec![0, 0xff],
        };
        let wire = [1, 0, 6, 0, 0x78, 0x56, 0x34, 0x12, 0, 0xff];
        assert_eq!(encode(&frame), wire);
        assert_eq!(decode(&wire), Ok(frame));
    }

    #[test]
    fn every_kind_and_channel_round_trips_at_both_payload_limits() {
        for (tag, kind) in [Kind::Datagram, Kind::Hello, Kind::Bye]
            .into_iter()
            .enumerate()
        {
            for port in 0..=MAX_PORT {
                for size in [0, 1, MAX_PAYLOAD] {
                    let frame = Frame {
                        kind,
                        port,
                        peer: u32::MAX,
                        payload: vec![0xaa; size],
                    };
                    let bytes = encode(&frame);
                    assert_eq!(bytes[1], tag as u8);
                    assert_eq!(bytes.len(), HEADER_LEN + size);
                    assert_eq!(decode(&bytes), Ok(frame));
                }
            }
        }
    }

    #[test]
    fn every_short_header_is_truncated() {
        for len in 0..HEADER_LEN {
            assert_eq!(
                decode(&[1; HEADER_LEN][..len]),
                Err(TunnelError::Truncated(len))
            );
        }
    }

    #[test]
    fn rejects_other_versions_kinds_offsets_and_oversize_messages() {
        for byte in 0..=255 {
            if byte != 1 {
                assert_eq!(
                    decode(&[byte, 0, 0, 0, 0, 0, 0, 0]),
                    Err(TunnelError::Version(byte))
                );
            }
            if byte > 2 {
                assert_eq!(
                    decode(&[1, byte, 0, 0, 0, 0, 0, 0]),
                    Err(TunnelError::Kind(byte))
                );
            }
        }
        for port in [7u16, 256, u16::MAX] {
            let [lo, hi] = port.to_le_bytes();
            assert_eq!(
                decode(&[1, 0, lo, hi, 0, 0, 0, 0]),
                Err(TunnelError::Port(port))
            );
        }
        let mut bytes = vec![0; HEADER_LEN + MAX_PAYLOAD + 1];
        bytes[0] = 1;
        assert_eq!(decode(&bytes), Err(TunnelError::Oversize(MAX_PAYLOAD + 1)));
    }
}
