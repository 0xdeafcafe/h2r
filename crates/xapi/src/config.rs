//! Only settings used by native overrides. Locale, video and region queries
//! remain generated over the kernel's ExQueryNonVolatileSetting (ordinal 24).

use crate::register::addr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceTypes {
    /// Address of the guest XPP_DEVICE_TYPE object, not numeric XID type 1.
    pub gamepad: u32,
    /// The communicator's two device types, when the host answers for them
    /// (docs/live/live-voice.md, 2.1). `None` leaves them unknown types:
    /// every device call reports no ports, as before voice.
    pub voice: Option<VoiceTypes>,
}

/// The guest XPP_DEVICE_TYPE objects of a communicator: 12 bytes each
/// (current, changed, previous), as the gamepad's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VoiceTypes {
    pub microphone: u32,
    pub headphone: u32,
}

impl VoiceTypes {
    /// The retail 5849 objects ([`addr::XDEVICE_TYPE_VOICE_MICROPHONE`],
    /// [`addr::XDEVICE_TYPE_VOICE_HEADPHONE`]).
    pub const RETAIL: Self = Self {
        microphone: addr::XDEVICE_TYPE_VOICE_MICROPHONE,
        headphone: addr::XDEVICE_TYPE_VOICE_HEADPHONE,
    };
}

/// Which cache partition backs the second cache volume, N: (`0x002d5d0a`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CachePartition {
    /// The retail choice from the cache database: the highest partition no
    /// valid row holds, other than Z:'s; if none is free, the one N:'s row
    /// already names. With the kernel's seed (one valid row, Z: on
    /// Partition5) this is Partition4.
    Select,
    /// Always this partition (3..=5). It must not be the utility partition.
    Fixed(u8),
}

/// Guest globals the N: override reads and publishes. Defaults are retail 5849.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CacheGlobals {
    /// Z:'s cache-database row, stored by XMountUtilityDrive. Read only.
    pub utility_row: u32,
    /// N:'s cache-database row, published here: the row on success, -1 on a
    /// failure after the Z: row check. Nothing else in the image reads it.
    pub cache_row: u32,
    /// Import slot of HalDiskCachePartitionCount (ordinal 40). The slot holds
    /// the address of the count, as at `0x002d5dab`.
    pub partition_count_import: u32,
    /// XBE header field holding the certificate address; the title ID is at
    /// certificate + 8 (`0x002d5d13`).
    pub certificate_pointer: u32,
}

impl Default for CacheGlobals {
    fn default() -> Self {
        Self {
            utility_row: addr::CACHE_ROW,
            cache_row: addr::CACHE_DRIVE_ROW,
            partition_count_import: addr::HAL_DISK_CACHE_PARTITION_COUNT_IMPORT,
            certificate_pointer: addr::XBE_CERTIFICATE_POINTER,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CacheDrive {
    pub partition: CachePartition,
    pub globals: CacheGlobals,
}

impl Default for CacheDrive {
    fn default() -> Self {
        Self {
            partition: CachePartition::Select,
            globals: CacheGlobals::default(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    pub devices: DeviceTypes,
    pub utility_partition: u8,
    /// The second cache volume, N:. See docs/runtime/xapi.md "The second cache volume".
    pub cache_drive: CacheDrive,
    /// Install the native save-game overrides (XCreateSaveGame,
    /// XDeleteSaveGame, XFindFirstSaveGame, XFindNextSaveGame) over
    /// `Storage::saves` instead of leaving the generated XDK code. Off by
    /// default: nothing about boot changes. See docs/runtime/xapi.md.
    pub save_games: bool,
}

impl Config {
    pub fn for_gamepad(gamepad_type_address: u32) -> Self {
        Self {
            devices: DeviceTypes {
                gamepad: gamepad_type_address,
                voice: None,
            },
            utility_partition: 5,
            cache_drive: CacheDrive::default(),
            save_games: false,
        }
    }
}
