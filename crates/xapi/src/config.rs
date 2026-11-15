//! Only settings used by native overrides. Locale, video and region queries
//! remain generated over the kernel's ExQueryNonVolatileSetting (ordinal 24).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceTypes {
    /// Address of the guest XPP_DEVICE_TYPE object, not numeric XID type 1.
    pub gamepad: u32,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub devices: DeviceTypes,
    pub utility_partition: u8,
}

impl Config {
    pub fn for_gamepad(gamepad_type_address: u32) -> Self {
        Self {
            devices: DeviceTypes {
                gamepad: gamepad_type_address,
            },
            utility_partition: 5,
        }
    }
}
