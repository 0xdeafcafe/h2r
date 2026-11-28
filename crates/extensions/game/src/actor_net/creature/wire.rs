//! The probe's payloads inside the retail bitstream. The caller still
//! owns the operation framing, including the five-bit entity type.

use guest_layout::{GuestMemory, MemFault};

pub(super) const POSITION_BYTES: usize = 3 * size_of::<f32>();
pub(super) const CREATION_BYTES: usize = 2 + 3 * POSITION_BYTES;
pub(super) const POSITION_BITS: u32 = (8 * POSITION_BYTES) as u32;
pub(super) const CREATION_BITS: u32 = (8 * CREATION_BYTES) as u32;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Creation {
    pub definition: u16,
    pub position: [f32; 3],
    pub forward: [f32; 3],
    pub up: [f32; 3],
}

impl Creation {
    pub fn encode(self) -> [u8; CREATION_BYTES] {
        let mut bytes = [0; CREATION_BYTES];
        bytes[..2].copy_from_slice(&self.definition.to_le_bytes());
        for (dest, vector) in
            bytes[2..]
                .chunks_exact_mut(POSITION_BYTES)
                .zip([self.position, self.forward, self.up])
        {
            dest.copy_from_slice(&encode_position(vector));
        }
        bytes
    }

    pub fn decode(bytes: [u8; CREATION_BYTES]) -> Option<Self> {
        let mut vectors = bytes[2..].chunks_exact(POSITION_BYTES);
        let creation = Self {
            definition: u16::from_le_bytes(bytes[..2].try_into().ok()?),
            position: decode_position(vectors.next()?.try_into().ok()?)?,
            forward: decode_position(vectors.next()?.try_into().ok()?)?,
            up: decode_position(vectors.next()?.try_into().ok()?)?,
        };
        creation.valid().then_some(creation)
    }

    pub fn valid(self) -> bool {
        let length_squared = |v: [f32; 3]| v.into_iter().map(|x| x * x).sum::<f32>();
        self.position.into_iter().all(f32::is_finite)
            && (0.9..=1.1).contains(&length_squared(self.forward))
            && (0.9..=1.1).contains(&length_squared(self.up))
            && self
                .forward
                .into_iter()
                .zip(self.up)
                .map(|(a, b)| a * b)
                .sum::<f32>()
                .abs()
                < 0.1
    }
}

pub(super) fn encode_position(position: [f32; 3]) -> [u8; POSITION_BYTES] {
    let mut bytes = [0; POSITION_BYTES];
    for (dest, axis) in bytes.chunks_exact_mut(4).zip(position) {
        dest.copy_from_slice(&axis.to_le_bytes());
    }
    bytes
}

pub(super) fn decode_position(bytes: [u8; POSITION_BYTES]) -> Option<[f32; 3]> {
    let position = std::array::from_fn(|axis| {
        f32::from_le_bytes(bytes[axis * 4..axis * 4 + 4].try_into().unwrap())
    });
    position.into_iter().all(f32::is_finite).then_some(position)
}

/// The bitstream cursor at +0x10 starts at the low bit of each byte,
/// matching the retail unsigned integer readers and writers.
const CURSOR: u32 = 0x10;
const CAPACITY: u32 = 4;

struct Window {
    data: u32,
    shift: usize,
    len: usize,
    end_cursor: u32,
}

impl Window {
    fn at<const N: usize>(m: &dyn GuestMemory, stream: u32) -> Result<Option<Self>, MemFault> {
        debug_assert!(N <= CREATION_BYTES);
        let cursor = m.u32(stream + CURSOR)?;
        let capacity = m.u32(stream + CAPACITY)?.saturating_mul(8);
        let Some(end_cursor) = cursor.checked_add((N * 8) as u32) else {
            return Ok(None);
        };
        if end_cursor > capacity {
            return Ok(None);
        }
        let data = m.u32(stream)?.wrapping_add(cursor / 8);
        let shift = (cursor % 8) as usize;
        let len = N + usize::from(shift != 0);
        Ok(Some(Self {
            data,
            shift,
            len,
            end_cursor,
        }))
    }
}

pub(super) fn write_payload<const N: usize>(
    m: &mut dyn GuestMemory,
    stream: u32,
    payload: [u8; N],
) -> Result<bool, MemFault> {
    let Some(window) = Window::at::<N>(m, stream)? else {
        return Ok(false);
    };
    let mut bytes = [0; CREATION_BYTES + 1];
    let mask = 0xffu16 << window.shift;
    m.read(window.data, &mut bytes[..window.len])?;
    for (index, value) in payload.into_iter().enumerate() {
        let previous = u16::from_le_bytes([bytes[index], bytes[index + 1]]);
        let combined = (previous & !mask) | (u16::from(value) << window.shift);
        bytes[index..index + 2].copy_from_slice(&combined.to_le_bytes());
    }
    m.write(window.data, &bytes[..window.len])?;
    m.set_u32(stream + CURSOR, window.end_cursor)?;
    Ok(true)
}

/// The retail creation writer's virtual call returns void. Make overflow
/// visible in the cursor so the outer writer drops the whole operation.
pub(super) fn fail_creation_write(m: &mut dyn GuestMemory, stream: u32) -> Result<(), MemFault> {
    let capacity = m.u32(stream + CAPACITY)?.saturating_mul(8);
    m.set_u32(stream + CURSOR, capacity.saturating_add(1))
}

pub(super) fn read_payload<const N: usize, T>(
    m: &mut dyn GuestMemory,
    stream: u32,
    decode: impl FnOnce([u8; N]) -> Option<T>,
) -> Result<Option<T>, MemFault> {
    let Some(window) = Window::at::<N>(m, stream)? else {
        return Ok(None);
    };
    let mut bytes = [0; CREATION_BYTES + 1];
    m.read(window.data, &mut bytes[..window.len])?;
    let payload = std::array::from_fn(|index| {
        (u16::from_le_bytes([bytes[index], bytes[index + 1]]) >> window.shift) as u8
    });
    let decoded = decode(payload);
    if decoded.is_some() {
        m.set_u32(stream + CURSOR, window.end_cursor)?;
    }
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    const STREAM: u32 = 0x1000;
    const DATA: u32 = 0x2000;

    #[test]
    fn probe_creation_and_position_round_trip() {
        let mut m = FakeGuest::new();
        m.map(STREAM, 0x20);
        m.map(DATA, 64);
        m.put_u32(STREAM, DATA)
            .put_u32(STREAM + CAPACITY, 64)
            .put_u32(STREAM + CURSOR, 5)
            .put_u8(DATA, 0b10101)
            .put_u8(DATA + 50, 0b1110_0000);
        let spawn = Creation {
            definition: 21,
            position: [1.25, 2.0, 13.0],
            forward: [0.0, 1.0, 0.0],
            up: [0.0, 0.0, 1.0],
        };
        let moved = [1.25, 2.0, 11.5];
        assert!(write_payload(&mut m, STREAM, spawn.encode()).unwrap());
        assert_eq!(m.u32(STREAM + CURSOR).unwrap(), 5 + CREATION_BITS);
        assert!(write_payload(&mut m, STREAM, encode_position(moved)).unwrap());
        assert_eq!(
            m.u32(STREAM + CURSOR).unwrap(),
            5 + CREATION_BITS + POSITION_BITS
        );
        assert_eq!(m.u8(DATA).unwrap() & 0b11111, 0b10101);
        assert_eq!(m.u8(DATA + 50).unwrap() & 0b1110_0000, 0b1110_0000);

        m.put_u32(STREAM + CURSOR, 5);
        assert_eq!(
            read_payload(&mut m, STREAM, Creation::decode).unwrap(),
            Some(spawn)
        );
        assert_eq!(
            read_payload(&mut m, STREAM, decode_position).unwrap(),
            Some(moved)
        );
        assert_eq!(
            m.u32(STREAM + CURSOR).unwrap(),
            5 + CREATION_BITS + POSITION_BITS
        );
    }

    #[test]
    fn bad_or_short_probe_payload_does_not_advance_the_reader() {
        let mut m = FakeGuest::new();
        m.map(STREAM, 0x20);
        m.map(DATA, 32);
        m.put_u32(STREAM, DATA).put_u32(STREAM + CAPACITY, 11);
        assert!(!write_payload(&mut m, STREAM, encode_position([1.0; 3])).unwrap());
        assert_eq!(read_payload(&mut m, STREAM, decode_position).unwrap(), None);
        assert_eq!(m.u32(STREAM + CURSOR).unwrap(), 0);

        m.put_u32(STREAM + CAPACITY, 12);
        assert!(write_payload(&mut m, STREAM, encode_position([f32::NAN; 3])).unwrap());
        m.put_u32(STREAM + CURSOR, 0);
        assert_eq!(read_payload(&mut m, STREAM, decode_position).unwrap(), None);
        assert_eq!(m.u32(STREAM + CURSOR).unwrap(), 0);
    }
}
