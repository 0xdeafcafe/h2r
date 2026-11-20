//! Extended inputs of converted weapon widgets. Native inputs end at 0x43
//! (0x13ad48). The draw adapter consumes the entire four-byte tuple.
pub const SCREEN: u8 = 0xe0; // widget +0x1b, plus the source screen index
/// Offset of the 24-byte flash record after four signed 16-bit thresholds.
pub const FLASH: usize = 8;
pub const WARNING_BYTES: usize = FLASH + 24;
pub const METER: u8 = 0xe0; // + state 0..5, minimum, multiplier, bias
pub const MAGAZINES: u8 = 0x90; // native/extended number input, digits, flags
pub const RANGE: u8 = 0xa0;
pub const ELEVATION: u8 = 0xa1;
pub const WARNING: u8 = 0xb0; // type, overlay flags, reserved
pub const SECOND_RESERVE: u8 = 0xc0;
pub const SECOND_LOADED: u8 = 0xc1;
pub const HEAT_BYTE: u8 = 0xc2;

/// H1 0xe213a consumes only the low byte before applying the transfer.
pub fn meter_fill(value: i16, [minimum, multiplier, bias]: [u8; 3]) -> f32 {
    let level = u16::from(value as u8) * u16::from(multiplier) + u16::from(bias);
    f32::from(level.min(255).max(u16::from(minimum))) / 255.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meter_uses_the_source_ramp_and_byte_width() {
        assert_eq!(meter_fill(4, [0, 60, 1]), 241.0 / 255.0);
        assert_eq!(meter_fill(256, [12, 60, 1]), 12.0 / 255.0);
        assert_eq!(meter_fill(261, [12, 60, 1]), 1.0);
    }
}
