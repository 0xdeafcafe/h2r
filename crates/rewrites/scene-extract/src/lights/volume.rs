//! Pose the light volume for the read-only constants ports
//! (`engine::render::light_gather::volume::pose`, the port of
//! `0x000c19f0` and `0x000c1a80`). The light pass builds the same scratch
//! record on its stack for the lights the guest draws.

use guest_layout::memory::{GuestMemory, MemFault};
use engine::render::light::LightRecord;
use engine::render::light_gather;
use frame_scene::LightVolume;

const WORDS: usize = light_gather::volume::WORDS;

/// Address used exclusively inside the constants reader.
pub(super) const ADDRESS: u32 = 0xffff_ff00;

pub(super) struct Volume([u32; WORDS]);

impl Volume {
    pub(super) fn scene(&self) -> LightVolume {
        let read = |offset: usize| f32::from_bits(self.0[offset / 4]);
        let vec3 = |offset| [read(offset), read(offset + 4), read(offset + 8)];
        let origin = vec3(0x2c);
        let kind = self.0[0] as u16;
        if kind == 0 {
            LightVolume::Sphere { origin, radius: read(0x44) }
        } else {
            LightVolume::Projected {
                kind, origin, axis: vec3(0x3c),
                near: read(0x60), far: read(0x64), far_extent: [read(0x68), read(0x6c)],
            }
        }
    }

    pub(super) fn at<'a>(&'a self, m: &'a dyn GuestMemory) -> impl GuestMemory + 'a {
        VolumeMemory { words: &self.0, guest: m }
    }
}

struct VolumeMemory<'a> {
    words: &'a [u32; WORDS],
    guest: &'a dyn GuestMemory,
}

impl GuestMemory for VolumeMemory<'_> {
    fn read(&self, addr: u32, buf: &mut [u8]) -> Result<(), MemFault> {
        if let Some(offset) = addr.checked_sub(ADDRESS) {
            let offset = offset as usize;
            if offset + buf.len() <= WORDS * 4 {
                for (i, byte) in buf.iter_mut().enumerate() {
                    let n = offset + i;
                    *byte = self.words[n / 4].to_le_bytes()[n % 4];
                }
                return Ok(());
            }
        }
        self.guest.read(addr, buf)
    }

    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), MemFault> {
        Err(MemFault { addr, len: data.len() as u32, write: true })
    }
}

pub(super) fn read(
    m: &dyn GuestMemory,
    address: u32,
    record: &LightRecord,
    definition: u32,
    definition_kind: u16,
    gel: u32,
) -> Option<Volume> {
    light_gather::volume::pose(m, address, record, definition, definition_kind, gel).map(|posed| Volume(posed.0))
}
