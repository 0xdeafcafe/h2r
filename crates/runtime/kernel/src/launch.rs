//! LaunchDataPage DATA export and explicitly supplied launch information.
use crate::{Error, Kernel, ord};
use runtime::guest::Access;
pub const PAGE_SIZE: usize = 4096;
pub const PATH_SIZE: usize = 520;
pub const DATA_OFFSET: usize = 1024;
pub const DATA_SIZE: usize = 3072;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LaunchData {
    pub kind: u32,
    pub title_id: u32,
    /// ANSI launch path, without its terminating NUL (at most 519 bytes).
    pub path: Vec<u8>,
    pub flags: u32,
    /// At most 3072 bytes; the remainder is zero-filled.
    pub data: Vec<u8>,
}
impl LaunchData {
    fn encode(&self) -> Result<[u8; PAGE_SIZE], Error> {
        if self.path.len() >= PATH_SIZE
            || self.path.contains(&0)
            || !self.path.is_ascii()
            || self.data.len() > DATA_SIZE
        {
            return Err(Error::Invalid("launch path/payload length or encoding"));
        }
        let mut page = [0; PAGE_SIZE];
        page[..4].copy_from_slice(&self.kind.to_le_bytes());
        page[4..8].copy_from_slice(&self.title_id.to_le_bytes());
        page[8..8 + self.path.len()].copy_from_slice(&self.path);
        page[528..532].copy_from_slice(&self.flags.to_le_bytes());
        page[DATA_OFFSET..DATA_OFFSET + self.data.len()].copy_from_slice(&self.data);
        Ok(page)
    }
}
impl Kernel {
    /// Publish launch information before guest execution (or between slices).
    /// None restores a null LaunchDataPage. This does not implement quick reboot
    /// persistence or MmFreeContiguousMemory; the latter remains a stop.
    pub fn set_launch_data(&self, launch: Option<&LaunchData>) -> Result<(), Error> {
        let encoded = launch.map(LaunchData::encode).transpose()?;
        let runtime = self.runtime()?;
        let mut core = self.lock();
        let mut memory = runtime.memory();
        let cell = core.data[&ord::LAUNCH_DATA_PAGE];
        memory.check(cell, 4, Access::Write, 1)?;
        let old = memory.u32(cell)?;
        if old != 0 && old != core.launch_page {
            return Err(Error::Invalid("LaunchDataPage changed by guest"));
        }
        let Some(encoded) = encoded else {
            memory.set_u32(cell, 0)?;
            return Ok(());
        };
        if core.launch_page == 0 {
            core.launch_page = core
                .memory
                .internal(&mut memory, PAGE_SIZE as u32)
                .map_err(Error::Resource)?;
        }
        memory.write_bytes(core.launch_page, &encoded, 1)?;
        memory.set_u32(cell, core.launch_page)?;
        Ok(())
    }
}
