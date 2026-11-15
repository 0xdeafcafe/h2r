//! Xbox service overrides that cannot run through the generated kernel path.
//! See docs/xapi.md. `register` installs them into a runtime `Dispatch`.
#![forbid(unsafe_code)]

pub mod adapter;
pub mod cache;
pub mod config;
mod controllers;
mod launch;
pub mod memory;
pub mod register;
pub mod saves;
pub mod storage;
mod vfs_saves;
pub mod vfs_storage;

pub use cache::CacheSelection;
pub use config::{CacheDrive, CacheGlobals, CachePartition, Config, DeviceTypes};
pub use controllers::{Events, PendingSignal, SignalBatch, SignalReport};
pub use launch::{LAUNCH_DATA_SIZE, LaunchRequest};
pub use memory::{GuestMemory, MemoryFault};
pub use register::{
    CacheRow, Conflict, GuestLayout, RegisterError, Registration, register, service_input,
    unregister,
};
pub use saves::{PendingSearch, SaveDirectory, SaveStore};
pub use storage::{CACHE_DATABASE_SIZE, Storage};
pub use vfs_storage::VfsStorage;

use controllers::{DeviceChanges, OpenInput};
use input::{InputManager, device::Completion};
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Memory(MemoryFault),
    Win32(u32),
}

impl Error {
    pub const INVALID_PARAMETER: Self = Self::Win32(87);
    pub const INVALID_HANDLE: Self = Self::Win32(6);
    pub const NOT_SUPPORTED: Self = Self::Win32(50);
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// Mirrors the current guest thread's XAPI TLS cell; accessors stay generated.
#[derive(Debug, Default)]
pub struct ThreadState {
    pub last_error: u32,
}

pub struct Xapi<StorageBackend> {
    pub config: Config,
    pub input: InputManager,
    pub storage: StorageBackend,
    handles: HashMap<u32, OpenInput>,
    next_handle: u32,
    pending_feedback: HashSet<u32>,
    completions: VecDeque<Completion>,
    signals: VecDeque<PendingSignal>,
    changes: DeviceChanges,
    launch_request: Option<LaunchRequest>,
    pending_search: Option<PendingSearch>,
    searches: std::collections::BTreeMap<u32, PendingSearch>,
}

impl<StorageBackend: Storage> Xapi<StorageBackend> {
    pub fn new(config: Config, input: InputManager, storage: StorageBackend) -> Result<Self> {
        if config.devices.gamepad == 0 || !(3..=5).contains(&config.utility_partition) {
            return Err(Error::INVALID_PARAMETER);
        }
        if let CachePartition::Fixed(partition) = config.cache_drive.partition
            && (!(3..=5).contains(&partition) || partition == config.utility_partition)
        {
            return Err(Error::INVALID_PARAMETER);
        }
        Ok(Self {
            config,
            input,
            storage,
            handles: HashMap::new(),
            next_handle: 0x5800_0000,
            pending_feedback: HashSet::new(),
            completions: VecDeque::new(),
            signals: VecDeque::new(),
            changes: DeviceChanges::default(),
            launch_request: None,
            pending_search: None,
            searches: Default::default(),
        })
    }

    fn reserve_handle(&mut self) -> Result<u32> {
        if self.handles.len() >= 4096 {
            return Err(Error::Win32(8));
        }
        let handle = self.next_handle;
        self.next_handle = self.next_handle.checked_add(4).ok_or(Error::Win32(8))?;
        Ok(handle)
    }

    pub fn mount_utility_drive(&mut self, format: bool) -> Result<()> {
        self.storage
            .mount_utility(self.config.utility_partition, format)
    }

    pub fn format_utility_drive(&mut self) -> Result<()> {
        self.storage.format_utility()
    }
}
