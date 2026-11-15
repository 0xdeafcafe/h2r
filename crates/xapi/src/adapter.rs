//! Runtime-neutral override adapters: decode a guest stdcall frame, run the
//! service and report how to leave. `crate::register` binds them to the
//! runtime's dispatch at their XBE addresses. See docs/xapi.md.
use crate::{Error, GuestMemory, Result, Storage, ThreadState, Xapi, memory};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Function {
    XInitDevices,
    XGetDevices,
    XGetDeviceChanges,
    XPeekDevices,
    XInputOpen,
    XInputClose,
    XInputGetState,
    XInputSetState,
    XInputPoll,
    XInputGetCapabilities,
    XGetLanguage,
    XGetVideoStandard,
    XGetVideoFlags,
    XGetAVPack,
    XGetGameRegion,
    XGetParentalControlSetting,
    XCreateSaveGame,
    XFindFirstSaveGame,
    XFindNextSaveGame,
    XDeleteSaveGame,
    XFindClose,
    DiskFreeSpace,
    DiskFreeSpaceEx,
    DiskClusterSize,
    XLaunchNewImage,
    XGetLaunchInfo,
    XMountUtilityDrive,
    XFormatUtilityDrive,
    GetLastError,
    SetLastError,
    XReadMUMetaData,
    XAutoPowerDownResetTimer,
    XapiInitAutoPowerDown,
}

/// `crate::register` installs and `crate::unregister` removes both together.
/// The reset call must never touch the timer bypassed by the startup hook.
pub const POWER_DOWN_OVERRIDES: [Function; 2] = [
    Function::XapiInitAutoPowerDown,
    Function::XAutoPowerDownResetTimer,
];

impl Function {
    pub const fn stack_words(self) -> usize {
        use Function::*;
        match self {
            XGetLanguage
            | XGetVideoStandard
            | XGetVideoFlags
            | XGetAVPack
            | XGetGameRegion
            | XGetParentalControlSetting
            | XFormatUtilityDrive
            | GetLastError
            | XAutoPowerDownResetTimer
            | XapiInitAutoPowerDown => 0,
            XGetDevices | XInputClose | XInputPoll | XFindClose | DiskClusterSize
            | XMountUtilityDrive | SetLastError => 1,
            XInitDevices
            | XInputGetState
            | XInputSetState
            | XInputGetCapabilities
            | XFindFirstSaveGame
            | XFindNextSaveGame
            | XDeleteSaveGame
            | XLaunchNewImage
            | XGetLaunchInfo => 2,
            XGetDeviceChanges | XPeekDevices => 3,
            XInputOpen | DiskFreeSpaceEx => 4,
            DiskFreeSpace | XReadMUMetaData => 5,
            XCreateSaveGame => 6,
        }
    }

    /// Keep kernel-capable originals generated, including save/config APIs.
    pub const fn is_replacement(self) -> bool {
        use Function::*;
        matches!(
            self,
            XInitDevices
                | XGetDevices
                | XGetDeviceChanges
                | XPeekDevices
                | XInputOpen
                | XInputClose
                | XInputGetState
                | XInputSetState
                | XInputPoll
                | XInputGetCapabilities
                | XLaunchNewImage
                | XMountUtilityDrive
                | XFormatUtilityDrive
                | XReadMUMetaData
                | XAutoPowerDownResetTimer
                | XapiInitAutoPowerDown
        )
    }

    fn is_void(self) -> bool {
        matches!(
            self,
            Self::XInitDevices
                | Self::XInputClose
                | Self::XAutoPowerDownResetTimer
                | Self::XapiInitAutoPowerDown
        )
    }

    fn failure(self, code: u32, thread: &mut ThreadState) -> u32 {
        match self {
            Self::XInputGetState
            | Self::XInputSetState
            | Self::XInputPoll
            | Self::XInputGetCapabilities
            | Self::XLaunchNewImage
            | Self::XReadMUMetaData => code,
            _ => {
                thread.last_error = code;
                0
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    pub eax: u32,
    pub esp: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Convert to TransferAbi::ret(pc), not Jump/Call.
    Return { pc: u32 },
    /// Runner must stop and consume Xapi::take_launch_request().
    Launch,
    /// Invoke the generated original without modifying CPU, memory or stack.
    Original,
}

impl<StorageBackend: Storage> Xapi<StorageBackend> {
    pub fn invoke(
        &mut self,
        function: Function,
        frame: &mut Frame,
        memory: &mut impl GuestMemory,
        thread: &mut ThreadState,
    ) -> Result<Outcome> {
        if !function.is_replacement() {
            return Ok(Outcome::Original);
        }
        let words = function.stack_words();
        let size = (words + 1) * 4;
        memory.check(frame.esp, size, false)?;
        let return_pc = memory::u32_at(memory, frame.esp)?;
        let new_esp = memory::offset(frame.esp, size)?;
        let mut arguments = [0; 6];
        for (index, argument) in arguments.iter_mut().take(words).enumerate() {
            *argument = memory::u32_at(memory, memory::offset(frame.esp, (index + 1) * 4)?)?;
        }
        let result = self.invoke_inner(function, arguments, memory);
        let value = match result {
            Ok(return_value) => return_value,
            Err(Error::Memory(fault)) => return Err(Error::Memory(fault)),
            Err(Error::Win32(code)) => function.failure(code, thread),
        };
        if function == Function::XLaunchNewImage && result.is_ok() {
            return Ok(Outcome::Launch);
        }
        if !function.is_void() {
            frame.eax = value;
        }
        frame.esp = new_esp;
        Ok(Outcome::Return { pc: return_pc })
    }

    fn invoke_inner(
        &mut self,
        function: Function,
        arguments: [u32; 6],
        memory: &mut impl GuestMemory,
    ) -> Result<u32> {
        use Function::*;
        match function {
            XInitDevices => {
                self.init_devices(memory, arguments[0], arguments[1])?;
                Ok(0)
            }
            XGetDevices => Ok(self.get_devices(arguments[0])),
            XGetDeviceChanges => self
                .get_device_changes(memory, arguments[0], arguments[1], arguments[2])
                .map(u32::from),
            XPeekDevices => self.peek_devices(memory, arguments[0], arguments[1], arguments[2]),
            XInputOpen => self.input_open(
                memory,
                arguments[0],
                arguments[1],
                arguments[2],
                arguments[3],
            ),
            XInputClose => {
                self.input_close(arguments[0])?;
                Ok(0)
            }
            XInputGetState => {
                self.input_get_state(memory, arguments[0], arguments[1])?;
                Ok(0)
            }
            XInputGetCapabilities => {
                self.input_get_capabilities(memory, arguments[0], arguments[1])?;
                Ok(0)
            }
            XInputSetState => self.input_set_state(memory, arguments[0], arguments[1]),
            XInputPoll => {
                self.input_poll(arguments[0])?;
                Ok(0)
            }
            XLaunchNewImage => {
                self.launch_new_image(memory, arguments[0], arguments[1])?;
                Ok(0)
            }
            XMountUtilityDrive => {
                self.mount_utility_drive(arguments[0] != 0)?;
                Ok(1)
            }
            XFormatUtilityDrive => {
                self.format_utility_drive()?;
                Ok(1)
            }
            XReadMUMetaData => Err(Error::Win32(1167)),
            XAutoPowerDownResetTimer | XapiInitAutoPowerDown => Ok(0),
            // Unreachable through invoke: originals return before reading memory.
            _ => {
                unreachable!("generated function passed to override")
            }
        }
    }
}
