#![allow(dead_code)]

use input::{
    HostBackend, HostPad, HostPadId, HostPadState, InputManager,
    host::{ConvertConfig, HostButton},
};
use std::sync::{Arc, Mutex};
use xapi::{
    CACHE_DATABASE_SIZE, CacheGlobals, Config, Error, Events, GuestMemory, MemoryFault, Result,
    Storage, ThreadState, Xapi,
    adapter::{Frame, Function, Outcome},
    memory,
};

pub struct Memory {
    pub bytes: Vec<u8>,
    pub readonly: Option<(u32, usize)>,
}

impl Memory {
    pub fn new() -> Self {
        Self {
            bytes: vec![0xcc; 0x8000],
            readonly: None,
        }
    }

    pub fn put(&mut self, address: u32, bytes: &[u8]) {
        self.write(address, bytes).unwrap();
    }

    pub fn word(&self, address: u32) -> u32 {
        memory::u32_at(self, address).unwrap()
    }
}

impl GuestMemory for Memory {
    fn check(&self, address: u32, length: usize, write: bool) -> Result<()> {
        let end = u64::from(address) + length as u64;
        let denied = write
            && self.readonly.is_some_and(|(start, length)| {
                u64::from(address) < u64::from(start) + length as u64 && end > u64::from(start)
            });
        if address == 0 || end > self.bytes.len() as u64 || denied {
            Err(Error::Memory(MemoryFault {
                address,
                length,
                write,
            }))
        } else {
            Ok(())
        }
    }

    fn read(&self, address: u32, bytes: &mut [u8]) -> Result<()> {
        self.check(address, bytes.len(), false)?;
        bytes.copy_from_slice(&self.bytes[address as usize..address as usize + bytes.len()]);
        Ok(())
    }

    fn write(&mut self, address: u32, bytes: &[u8]) -> Result<()> {
        self.check(address, bytes.len(), true)?;
        self.bytes[address as usize..address as usize + bytes.len()].copy_from_slice(bytes);
        Ok(())
    }
}

/// Test guest globals for the N: override, inside the 0x8000-byte Memory.
pub const UTILITY_ROW: u32 = 0x3000;
pub const CACHE_ROW: u32 = 0x3004;
pub const COUNT_IMPORT: u32 = 0x3010;
pub const COUNT: u32 = 0x3020;
pub const CERTIFICATE_POINTER: u32 = 0x3030;
pub const CERTIFICATE: u32 = 0x3100;
pub const TITLE_ID: u32 = 0x4d53_0064;

pub fn test_cache_globals() -> CacheGlobals {
    CacheGlobals {
        utility_row: UTILITY_ROW,
        cache_row: CACHE_ROW,
        partition_count_import: COUNT_IMPORT,
        certificate_pointer: CERTIFICATE_POINTER,
    }
}

/// A cache database in the kernel's seed format (crates/kernel rawdisk.rs):
/// rows of (title, partition - 3, valid).
pub fn database(rows: &[(u32, u32, u32)]) -> [u8; CACHE_DATABASE_SIZE] {
    let mut s = [0; CACHE_DATABASE_SIZE];
    s[0..4].copy_from_slice(&0x9731_5286u32.to_le_bytes());
    s[4..8].copy_from_slice(&2u32.to_le_bytes());
    for (i, (title, index, valid)) in rows.iter().enumerate() {
        let at = 8 + 12 * i;
        s[at..at + 4].copy_from_slice(&title.to_le_bytes());
        s[at + 4..at + 8].copy_from_slice(&index.to_le_bytes());
        s[at + 8..at + 12].copy_from_slice(&valid.to_le_bytes());
    }
    s[0x1f8..0x1fc].copy_from_slice(&0xaa55_0000u32.to_le_bytes());
    s
}

#[derive(Default)]
pub struct Utility {
    pub calls: Vec<String>,
    pub mounted: Option<u8>,
    pub failure: Option<Error>,
    /// The raw-disk cache database; None fails the read with error 21.
    pub database: Option<[u8; CACHE_DATABASE_SIZE]>,
    pub database_writes: usize,
    pub cache_failure: Option<Error>,
    /// Makes write_cache_database fail (after counting the attempt).
    pub write_failure: Option<Error>,
}

impl Storage for Utility {
    fn mount_utility(&mut self, partition: u8, format: bool) -> Result<()> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        self.calls.push(format!("mount {partition} {format}"));
        self.mounted = Some(partition);
        Ok(())
    }

    fn format_utility(&mut self) -> Result<()> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        self.mounted.ok_or(Error::Win32(3))?;
        self.calls.push("format".into());
        Ok(())
    }

    fn read_cache_database(&mut self) -> Result<[u8; CACHE_DATABASE_SIZE]> {
        self.database.ok_or(Error::Win32(21))
    }

    fn write_cache_database(&mut self, sector: &[u8; CACHE_DATABASE_SIZE]) -> Result<()> {
        self.database_writes += 1;
        if let Some(error) = self.write_failure {
            return Err(error);
        }
        self.database = Some(*sector);
        Ok(())
    }

    fn mount_cache(&mut self, partition: u8) -> Result<()> {
        if let Some(error) = self.cache_failure {
            return Err(error);
        }
        if self.mounted == Some(partition) {
            return Err(Error::Win32(32));
        }
        self.calls.push(format!("cache {partition}"));
        Ok(())
    }
}

#[derive(Default)]
pub struct Signals {
    pub calls: Vec<u32>,
    pub failed_handle: Option<u32>,
}

impl Events for Signals {
    fn signal(&mut self, handle: u32) -> Result<()> {
        if self.failed_handle == Some(handle) {
            return Err(Error::INVALID_HANDLE);
        }
        self.calls.push(handle);
        Ok(())
    }
}

#[derive(Clone)]
pub struct Pads(Arc<Mutex<Vec<HostPad>>>);

impl Pads {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(vec![HostPad {
            id: HostPadId(1),
            name: "synthetic".into(),
            state: HostPadState::default(),
            physical: false,
            supports_rumble: true,
        }])))
    }

    pub fn present(&self, present: bool) {
        let mut pads = self.0.lock().unwrap();
        if present {
            pads.push(HostPad {
                id: HostPadId(1),
                name: "replugged".into(),
                state: HostPadState::default(),
                physical: false,
                supports_rumble: true,
            });
        } else {
            pads.clear();
        }
    }

    pub fn south(&self, pressed: bool) {
        self.0.lock().unwrap()[0]
            .state
            .buttons
            .set(HostButton::South, pressed);
    }

    pub fn rumble(&self, supported: bool) {
        self.0.lock().unwrap()[0].supports_rumble = supported;
    }
}

impl HostBackend for Pads {
    fn name(&self) -> &str {
        "synthetic"
    }

    fn poll(&mut self) -> Vec<HostPad> {
        self.0.lock().unwrap().clone()
    }
}

pub fn setup() -> (Xapi<Utility>, Memory, Pads) {
    let pads = Pads::new();
    let mut input = InputManager::new(ConvertConfig::default()).unwrap();
    input.add_backend(Box::new(pads.clone()));
    let mut config = Config::for_gamepad(0x100);
    config.cache_drive.globals = test_cache_globals();
    let xapi = Xapi::new(config, input, Utility::default()).unwrap();
    (xapi, Memory::new(), pads)
}

pub fn initialized() -> (Xapi<Utility>, Memory, Pads) {
    let (mut xapi, mut memory, pads) = setup();
    xapi.init_devices(&mut memory, 0, 0).unwrap();
    (xapi, memory, pads)
}

pub fn open(xapi: &mut Xapi<Utility>, memory: &Memory) -> u32 {
    xapi.input_open(memory, 0x100, 0, 0, 0).unwrap()
}

/// The documented frame sequence: write statuses under the memory guard, then
/// deliver and acknowledge event signals with no guard held.
pub fn sync(xapi: &mut Xapi<Utility>, memory: &mut Memory, signals: &mut Signals) -> Result<()> {
    let written = xapi.sync_input(memory);
    let report = xapi.take_signals().deliver(signals);
    written.and(xapi.acknowledge_signals(report))
}

pub fn call(
    xapi: &mut Xapi<Utility>,
    memory: &mut Memory,
    thread: &mut ThreadState,
    function: Function,
    arguments: &[u32],
) -> Result<(u32, Outcome)> {
    assert_eq!(arguments.len(), function.stack_words());
    memory.put(0x1000, &0x1234_5678u32.to_le_bytes());
    for (index, value) in arguments.iter().enumerate() {
        memory.put(0x1004 + index as u32 * 4, &value.to_le_bytes());
    }
    let mut frame = Frame {
        eax: 0xdead_beef,
        esp: 0x1000,
    };
    let outcome = xapi.invoke(function, &mut frame, memory, thread)?;
    if let Outcome::Return { pc } = outcome {
        assert_eq!(pc, 0x1234_5678);
        assert_eq!(frame.esp, 0x1004 + arguments.len() as u32 * 4);
    } else {
        assert_eq!(
            frame,
            Frame {
                eax: 0xdead_beef,
                esp: 0x1000
            }
        );
    }
    Ok((frame.eax, outcome))
}
