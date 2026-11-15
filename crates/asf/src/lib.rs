//! Bounded ASF demuxing. All wire reads are checked; payload bytes borrow the input.
//! Provenance and deliberately unsupported features are recorded in docs/wma.md.
#![forbid(unsafe_code)]

#[cfg(feature = "disc")]
pub mod disc;
mod packet;
pub use packet::{MediaObject, Packet, Payload, Reassembler};

pub type Guid = [u8; 16];
pub const HEADER: Guid = guid(0x75b22630, 0x668e, 0x11cf, 0xa6d900aa0062ce6c);
pub const DATA: Guid = guid(0x75b22636, 0x668e, 0x11cf, 0xa6d900aa0062ce6c);
pub const FILE_PROPERTIES: Guid = guid(0x8cabdca1, 0xa947, 0x11cf, 0x8ee400c00c205365);
pub const STREAM_PROPERTIES: Guid = guid(0xb7dc0791, 0xa9b7, 0x11cf, 0x8ee600c00c205365);
pub const HEADER_EXTENSION: Guid = guid(0x5fbf03b5, 0xa92e, 0x11cf, 0x8ee300c00c205365);
pub const AUDIO: Guid = guid(0xf8699e40, 0x5b4d, 0x11cf, 0xa8fd00805f5c442b);
pub const NO_ERROR_CORRECTION: Guid = guid(0x20fb5700, 0x5b55, 0x11cf, 0xa8fd00805f5c442b);
pub const AUDIO_SPREAD: Guid = guid(0xbfc3cd50, 0x618f, 0x11cf, 0x8bb200aa00b4e220);
const ENCRYPTION: [Guid; 3] = [
    guid(0x2211b3fb, 0xbd23, 0x11d2, 0xb4b700a0c955fc6e),
    guid(0x298ae614, 0x2622, 0x4c17, 0xb935dae07ee9289c),
    guid(0x43058533, 0x6981, 0x49e6, 0x9b74ad12cb86d58c),
];
const fn guid(a: u32, b: u16, c: u16, d: u64) -> Guid {
    let a = a.to_le_bytes();
    let b = b.to_le_bytes();
    let c = c.to_le_bytes();
    let d = d.to_be_bytes();
    [
        a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d[0], d[1], d[2], d[3], d[4], d[5], d[6],
        d[7],
    ]
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("truncated ASF structure")]
    Truncated,
    #[error("invalid ASF: {0}")]
    Invalid(&'static str),
    #[error("unsupported ASF: {0}")]
    Unsupported(&'static str),
    #[error("ASF resource limit: {0}")]
    Limit(&'static str),
}
pub type Result<T> = std::result::Result<T, Error>;

pub(crate) struct Cursor<'a> {
    bytes: &'a [u8],
}
impl<'a> Cursor<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }
    pub(crate) fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let b = self.bytes.get(..n).ok_or(Error::Truncated)?;
        self.bytes = &self.bytes[n..];
        Ok(b)
    }
    pub(crate) fn remaining(&self) -> usize {
        self.bytes.len()
    }
    pub(crate) fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    pub(crate) fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    pub(crate) fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub(crate) fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn guid(&mut self) -> Result<Guid> {
        Ok(self.take(16)?.try_into().unwrap())
    }
    pub(crate) fn variable(&mut self, kind: u8) -> Result<u32> {
        match kind & 3 {
            0 => Ok(0),
            1 => Ok(self.u8()?.into()),
            2 => Ok(self.u16()?.into()),
            _ => self.u32(),
        }
    }
}

/// An unknown header/index object's bytes are preserved without interpreting metadata.
#[derive(Debug, Clone, Copy)]
pub struct Object<'a> {
    pub id: Guid,
    pub body: &'a [u8],
}
fn object<'a>(c: &mut Cursor<'a>) -> Result<Object<'a>> {
    let id = c.guid()?;
    let n = c
        .u64()?
        .checked_sub(24)
        .ok_or(Error::Invalid("object size below 24"))?;
    let n = usize::try_from(n).map_err(|_| Error::Limit("object size"))?;
    Ok(Object {
        id,
        body: c.take(n)?,
    })
}

#[derive(Debug, Clone)]
pub struct FileProperties {
    pub file_id: Guid,
    pub file_size: u64,
    pub packet_count: u64,
    pub play_duration_100ns: u64,
    pub preroll_ms: u64,
    pub packet_size: u32,
    pub max_bitrate: u32,
}
impl FileProperties {
    fn parse(bytes: &[u8]) -> Result<Self> {
        let mut c = Cursor::new(bytes);
        let file_id = c.guid()?;
        let file_size = c.u64()?;
        c.u64()?; // Creation date.
        let packet_count = c.u64()?;
        let play_duration_100ns = c.u64()?;
        c.u64()?; // Send duration.
        let preroll_ms = c.u64()?;
        let flags = c.u32()?;
        if flags & 1 != 0 {
            return Err(Error::Unsupported("broadcast"));
        }
        let min = c.u32()?;
        let max = c.u32()?;
        if min == 0 || min != max {
            return Err(Error::Unsupported("variable packet size"));
        }
        if max > 65536 {
            return Err(Error::Limit("packet size"));
        }
        let max_bitrate = c.u32()?;
        Ok(Self {
            file_id,
            file_size,
            packet_count,
            play_duration_100ns,
            preroll_ms,
            packet_size: max,
            max_bitrate,
        })
    }
}

#[derive(Debug, Clone)]
pub struct WaveFormat<'a> {
    pub format_tag: u16,
    pub channels: u16,
    pub sample_rate: u32,
    pub bytes_per_second: u32,
    pub block_align: u16,
    pub bits_per_sample: u16,
    pub extra: &'a [u8],
}
impl<'a> WaveFormat<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self> {
        let mut c = Cursor::new(bytes);
        let format_tag = c.u16()?;
        let channels = c.u16()?;
        let sample_rate = c.u32()?;
        let bytes_per_second = c.u32()?;
        let block_align = c.u16()?;
        let bits_per_sample = c.u16()?;
        let n = c.u16()?;
        let extra = c.take(n.into())?;
        if channels == 0 || sample_rate == 0 || block_align == 0 {
            return Err(Error::Invalid("zero audio dimension"));
        }
        Ok(Self {
            format_tag,
            channels,
            sample_rate,
            bytes_per_second,
            block_align,
            bits_per_sample,
            extra,
        })
    }
}

#[derive(Debug, Clone)]
pub struct StreamProperties<'a> {
    pub number: u8,
    pub stream_type: Guid,
    pub time_offset_100ns: u64,
    pub type_data: &'a [u8],
    pub error_correction_type: Guid,
    pub error_correction_data: &'a [u8],
    pub audio: Option<WaveFormat<'a>>,
}
impl<'a> StreamProperties<'a> {
    fn parse(bytes: &'a [u8]) -> Result<Self> {
        let mut c = Cursor::new(bytes);
        let stream_type = c.guid()?;
        let error_correction_type = c.guid()?;
        let time_offset_100ns = c.u64()?;
        let type_len = c.u32()? as usize;
        let ec_len = c.u32()? as usize;
        let flags = c.u16()?;
        if flags & 0x8000 != 0 {
            return Err(Error::Unsupported("encrypted stream"));
        }
        let number = (flags & 127) as u8;
        if number == 0 {
            return Err(Error::Invalid("stream number zero"));
        }
        c.u32()?;
        let type_data = c.take(type_len)?;
        let error_correction_data = c.take(ec_len)?;
        let audio = if stream_type == AUDIO {
            Some(WaveFormat::parse(type_data)?)
        } else {
            None
        };
        Ok(Self {
            number,
            stream_type,
            time_offset_100ns,
            type_data,
            error_correction_type,
            error_correction_data,
            audio,
        })
    }
    /// The game uses span-one audio: no descrambling is needed. Fail explicitly
    /// for other schemes, rather than returning incorrectly ordered codec bytes.
    pub fn check_audio_spread(&self) -> Result<()> {
        if self.error_correction_type == NO_ERROR_CORRECTION {
            return Ok(());
        }
        if self.error_correction_type != AUDIO_SPREAD {
            return Err(Error::Unsupported("error correction scheme"));
        }
        let mut c = Cursor::new(self.error_correction_data);
        let span = c.u8()?;
        let packet = c.u16()?;
        let chunk = c.u16()?;
        let silence = c.u16()?;
        c.take(silence.into())?;
        if span != 1 || packet == 0 || packet != chunk {
            return Err(Error::Unsupported("spread audio interleave"));
        }
        Ok(())
    }
}

/// A complete, finite ASF file. Construction validates object extents and packet
/// count; packet payload contents are checked lazily by `packets`.
#[derive(Debug)]
pub struct Asf<'a> {
    pub file: FileProperties,
    pub streams: Vec<StreamProperties<'a>>,
    pub header_objects: Vec<Object<'a>>,
    pub extension_objects: Vec<Object<'a>>,
    packets: &'a [u8],
}
impl<'a> Asf<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self> {
        let mut c = Cursor::new(bytes);
        let h = object(&mut c)?;
        if h.id != HEADER {
            return Err(Error::Invalid("missing header"));
        }
        let mut h = Cursor::new(h.body);
        let count = h.u32()? as usize;
        h.u8()?;
        if h.u8()? != 2 {
            return Err(Error::Unsupported("header version"));
        }
        if count > 4096 {
            return Err(Error::Limit("header objects"));
        }
        let mut file = None;
        let mut streams = Vec::new();
        let mut header_objects = Vec::new();
        let mut extension_objects = Vec::new();
        let mut extension_seen = false;
        for _ in 0..count {
            let o = object(&mut h)?;
            if ENCRYPTION.contains(&o.id) {
                return Err(Error::Unsupported("content encryption"));
            }
            match o.id {
                FILE_PROPERTIES => {
                    if file.is_some() {
                        return Err(Error::Invalid("duplicate file properties"));
                    }
                    file = Some(FileProperties::parse(o.body)?);
                }
                STREAM_PROPERTIES => {
                    let s = StreamProperties::parse(o.body)?;
                    if streams
                        .iter()
                        .any(|old: &StreamProperties<'_>| old.number == s.number)
                    {
                        return Err(Error::Invalid("duplicate stream number"));
                    }
                    streams.push(s);
                }
                HEADER_EXTENSION => {
                    if extension_seen {
                        return Err(Error::Invalid("duplicate header extension"));
                    }
                    extension_seen = true;
                    let mut e = Cursor::new(o.body);
                    e.take(18)?; // Reserved GUID and WORD.
                    let n = e.u32()? as usize;
                    let mut e = Cursor::new(e.take(n)?);
                    while e.remaining() > 0 {
                        if extension_objects.len() >= 4096 {
                            return Err(Error::Limit("extension objects"));
                        }
                        let child = object(&mut e)?;
                        if ENCRYPTION.contains(&child.id) {
                            return Err(Error::Unsupported("content encryption"));
                        }
                        extension_objects.push(child);
                    }
                }
                _ => {}
            }
            header_objects.push(o);
        }
        if h.remaining() != 0 {
            return Err(Error::Invalid("header object count"));
        }
        let file = file.ok_or(Error::Invalid("missing file properties"))?;
        if !extension_seen || streams.is_empty() {
            return Err(Error::Invalid("missing required header object"));
        }
        if file.file_size != bytes.len() as u64 {
            return Err(Error::Invalid("file size"));
        }
        let d = object(&mut c)?;
        if d.id != DATA {
            return Err(Error::Invalid("missing data object"));
        }
        let mut d = Cursor::new(d.body);
        if d.guid()? != file.file_id || d.u64()? != file.packet_count {
            return Err(Error::Invalid("data object identity or packet count"));
        }
        if d.u16()? != 0x0101 {
            return Err(Error::Invalid("data reserved field"));
        }
        let n = file
            .packet_count
            .checked_mul(file.packet_size.into())
            .ok_or(Error::Limit("packet bytes"))?;
        if n != d.remaining() as u64 {
            return Err(Error::Invalid("data packet extent"));
        }
        let packets = d.take(d.remaining())?;
        while c.remaining() != 0 {
            let o = object(&mut c)?;
            if o.id == DATA || o.id == HEADER {
                return Err(Error::Invalid("duplicate top-level object"));
            }
            // Optional indexes/unknown top-level objects need no playback work.
        }
        Ok(Self {
            file,
            streams,
            header_objects,
            extension_objects,
            packets,
        })
    }
    pub fn packets(&self) -> impl Iterator<Item = Result<Packet<'a>>> + '_ {
        self.packets
            .chunks_exact(self.file.packet_size as usize)
            .map(Packet::parse)
    }
}

#[cfg(test)]
mod tests;
