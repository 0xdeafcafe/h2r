use crate::{Error, GuestMemory, Result, Storage, Xapi, memory};

pub const LAUNCH_DATA_SIZE: usize = 3072;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchRequest {
    /// None means dashboard; the host runner decides how to fulfill it.
    pub path: Option<String>,
    pub data: Option<Box<[u8; LAUNCH_DATA_SIZE]>>,
}

impl<StorageBackend: Storage> Xapi<StorageBackend> {
    /// Queue the host transition for XLaunchNewImageA (0x002d07f2).
    /// Incoming XGetLaunchInfo (0x002d0915) remains generated over LaunchDataPage.
    pub fn launch_new_image(
        &mut self,
        memory: &impl GuestMemory,
        path_address: u32,
        data_address: u32,
    ) -> Result<()> {
        if self.launch_request.is_some() {
            return Err(Error::Win32(170));
        }
        let path = if path_address == 0 {
            None
        } else {
            let path = memory::ansi(memory, path_address, 260)?;
            if path.is_empty() {
                return Err(Error::INVALID_PARAMETER);
            }
            Some(path)
        };
        let data = if data_address == 0 {
            None
        } else {
            Some(Box::new(memory::read(memory, data_address)?))
        };
        self.launch_request = Some(LaunchRequest { path, data });
        Ok(())
    }

    /// Runner consumes this after Outcome::Launch, then stops/reloads the guest.
    /// Never continue at the original return PC after accepting a launch.
    pub fn take_launch_request(&mut self) -> Option<LaunchRequest> {
        self.launch_request.take()
    }
}
