use crate::{Cursor, Error, Result};

/// One payload, or one expanded compressed sub-payload. Timestamps are the ASF
/// millisecond values, before subtracting file preroll and adding stream offset.
#[derive(Debug, Clone, Copy)]
pub struct Payload<'a> {
    pub stream: u8,
    pub key_frame: bool,
    pub object_number: u32,
    pub offset: u32,
    pub object_size: Option<u32>,
    pub presentation_time_ms: Option<u32>,
    pub data: &'a [u8],
}

#[derive(Debug)]
pub struct Packet<'a> {
    pub send_time_ms: u32,
    pub duration_ms: u16,
    pub payloads: Vec<Payload<'a>>,
}
impl<'a> Packet<'a> {
    /// Parse one fixed-size ASF packet, including its padding. No payload bytes
    /// are copied. Expansion is capped independently of the input's size.
    pub fn parse(bytes: &'a [u8]) -> Result<Self> {
        if bytes.len() > 65536 {
            return Err(Error::Limit("packet size"));
        }
        let mut c = Cursor::new(bytes);
        let first = c.u8()?;
        let flags = if first & 0x80 != 0 {
            if first & 0x70 != 0 {
                return Err(Error::Unsupported("opaque error correction"));
            }
            c.take((first & 15).into())?;
            c.u8()?
        } else {
            first
        };
        let properties = c.u8()?;
        if properties >> 6 != 1 {
            return Err(Error::Unsupported("stream number width"));
        }
        let packet_len = c.variable(flags >> 5)? as usize;
        c.variable(flags >> 1)?; // Sequence is reserved.
        let padding = c.variable(flags >> 3)? as usize;
        let send_time_ms = c.u32()?;
        let duration_ms = c.u16()?;
        let effective_len = if (flags >> 5) & 3 == 0 {
            bytes.len()
        } else {
            packet_len
        };
        if effective_len > bytes.len() {
            return Err(Error::Truncated);
        }
        let end = effective_len
            .checked_sub(padding)
            .ok_or(Error::Invalid("packet padding"))?;
        let header_len = bytes.len() - c.remaining();
        let payload_len = end
            .checked_sub(header_len)
            .ok_or(Error::Invalid("packet length"))?;
        let mut c = Cursor::new(c.take(payload_len)?);
        let (count, len_type) = if flags & 1 != 0 {
            let p = c.u8()?;
            if p & 63 == 0 || p >> 6 == 0 {
                return Err(Error::Invalid("multiple payload flags"));
            }
            (p & 63, p >> 6)
        } else {
            (1, 0)
        };
        let mut payloads = Vec::new();
        for _ in 0..count {
            let stream = c.u8()?;
            if stream & 127 == 0 {
                return Err(Error::Invalid("payload stream zero"));
            }
            let number = c.variable(properties >> 4)?;
            let offset = c.variable(properties >> 2)?;
            let rep_len = c.variable(properties)? as usize;
            if (2..8).contains(&rep_len) {
                return Err(Error::Invalid("replicated data length"));
            }
            let replicated = c.take(rep_len)?;
            let n = if len_type != 0 {
                c.variable(len_type)? as usize
            } else {
                c.remaining()
            };
            let data = c.take(n)?;
            if rep_len == 1 {
                if (properties >> 2) & 3 == 0 {
                    return Err(Error::Invalid("compressed timestamp absent"));
                }
                let mut sub = Cursor::new(data);
                let mut number = number;
                let mut time = offset;
                while sub.remaining() != 0 {
                    if payloads.len() >= 4096 {
                        return Err(Error::Limit("expanded payloads"));
                    }
                    let size = sub.u8()?;
                    if size == 0 {
                        return Err(Error::Invalid("empty compressed sub-payload"));
                    }
                    let data = sub.take(size.into())?;
                    payloads.push(Payload {
                        stream: stream & 127,
                        key_frame: stream & 128 != 0,
                        object_number: number,
                        offset: 0,
                        object_size: Some(size.into()),
                        presentation_time_ms: Some(time),
                        data,
                    });
                    number = match (properties >> 4) & 3 {
                        0 => 0,
                        1 => number.wrapping_add(1) & 0xff,
                        2 => number.wrapping_add(1) & 0xffff,
                        _ => number.wrapping_add(1),
                    };
                    time = time
                        .checked_add(replicated[0].into())
                        .ok_or(Error::Invalid("timestamp overflow"))?;
                }
            } else {
                let (object_size, presentation_time_ms) = if rep_len >= 8 {
                    let mut r = Cursor::new(replicated);
                    (Some(r.u32()?), Some(r.u32()?))
                } else {
                    (None, None)
                };
                if let Some(size) = object_size
                    && (size == 0 || u64::from(offset) + data.len() as u64 > u64::from(size))
                {
                    return Err(Error::Invalid("payload exceeds media object"));
                }
                if payloads.len() >= 4096 {
                    return Err(Error::Limit("expanded payload count"));
                }
                payloads.push(Payload {
                    stream: stream & 127,
                    key_frame: stream & 128 != 0,
                    object_number: number,
                    offset,
                    object_size,
                    presentation_time_ms,
                    data,
                });
            }
        }
        // Multiple payload lengths can delimit implicit trailing padding (§5.2.2).
        Ok(Self {
            send_time_ms,
            duration_ms,
            payloads,
        })
    }
}

#[derive(Debug, Clone)]
pub struct MediaObject {
    pub number: u32,
    pub presentation_time_ms: Option<u32>,
    pub data: Vec<u8>,
}

/// Bounded consecutive-fragment assembler for one selected stream. Rejects
/// gaps, overlaps, abandoned objects and inconsistent repeated metadata. Call
/// `finish` at EOF so a truncated final media object cannot silently disappear.
pub struct Reassembler {
    stream: u8,
    limit: usize,
    pending: Option<(MediaObject, usize)>,
    failed: bool,
}
impl Reassembler {
    pub fn new(stream: u8, max_object_bytes: usize) -> Result<Self> {
        if stream == 0 || stream > 127 || max_object_bytes == 0 {
            return Err(Error::Invalid("reassembler configuration"));
        }
        Ok(Self {
            stream,
            limit: max_object_bytes,
            pending: None,
            failed: false,
        })
    }
    pub fn push(&mut self, p: &Payload<'_>) -> Result<Option<MediaObject>> {
        if self.failed {
            return Err(Error::Invalid("reassembler failed"));
        }
        let result = self.push_inner(p);
        if result.is_err() {
            self.failed = true;
            self.pending = None;
        }
        result
    }
    fn push_inner(&mut self, p: &Payload<'_>) -> Result<Option<MediaObject>> {
        if p.stream != self.stream {
            return Ok(None);
        }
        if p.offset == 0 {
            if self.pending.is_some() {
                return Err(Error::Invalid("incomplete media object"));
            }
            // Absent size cannot establish completeness. Do not guess from the
            // packet boundary, which may be a fragment boundary instead.
            let size = p
                .object_size
                .ok_or(Error::Unsupported("media object size absent"))?
                as usize;
            if size > self.limit {
                return Err(Error::Limit("media object size"));
            }
            if size == 0 {
                return Err(Error::Invalid("empty media object"));
            }
            self.pending = Some((
                MediaObject {
                    number: p.object_number,
                    presentation_time_ms: p.presentation_time_ms,
                    data: Vec::new(),
                },
                size,
            ));
        }
        let (object, size) = self
            .pending
            .as_mut()
            .ok_or(Error::Invalid("orphan media fragment"))?;
        if object.number != p.object_number
            || p.offset as usize != object.data.len()
            || p.object_size.is_some_and(|n| n as usize != *size)
            || p.presentation_time_ms
                .is_some_and(|t| Some(t) != object.presentation_time_ms)
        {
            return Err(Error::Invalid("inconsistent media fragment"));
        }
        if p.data.len() > size.saturating_sub(object.data.len()) {
            return Err(Error::Invalid("media object overflow"));
        }
        object
            .data
            .try_reserve(p.data.len())
            .map_err(|_| Error::Limit("allocation"))?;
        object.data.extend_from_slice(p.data);
        if object.data.len() == *size {
            Ok(self.pending.take().map(|(o, _)| o))
        } else {
            Ok(None)
        }
    }
    pub fn finish(self) -> Result<()> {
        if self.failed || self.pending.is_some() {
            Err(Error::Invalid("incomplete or failed media stream"))
        } else {
            Ok(())
        }
    }
}
