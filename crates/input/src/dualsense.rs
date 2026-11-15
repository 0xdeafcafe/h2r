//! DualSense HID reports for the mute button, its LED and the speaker path.
//!
//! GameController.framework has no element for the DualSense mute button and
//! no way to light its LED (`GCDualSenseGamepad` exposes the touchpad and the
//! adaptive triggers only), so both go through the raw HID reports. This
//! module is the byte-level half: parse an input report, keep the mute
//! toggle, and build the output report. Opening the HID device (IOKit or
//! hidapi in shared mode) is the host's business; see
//! `docs/playstation-audio.md`.
//!
//! Layouts follow the Linux `hid-playstation` driver
//! (`struct dualsense_input_report`, `struct dualsense_output_report_common`)
//! and SDL's `SDL_hidapi_ps5.c`; offsets here are within the whole report,
//! report ID at byte 0. The DualShock 4 has no mute button or LED.

/// Sony's USB vendor ID.
pub const SONY_VID: u16 = 0x054c;
/// DualSense and DualSense Edge product IDs.
pub const DUALSENSE_PIDS: &[u16] = &[0x0ce6, 0x0df2];
/// DualShock 4 (v1, v2) product IDs.
pub const DS4_PIDS: &[u16] = &[0x05c4, 0x09cc];

/// How the controller is connected; the reports differ in header and CRC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    Usb,
    Bluetooth,
}

const USB_INPUT_ID: u8 = 0x01;
const USB_INPUT_LEN: usize = 64;
const BT_INPUT_ID: u8 = 0x31;
const BT_INPUT_LEN: usize = 78;
/// `buttons[2]` is byte 9 of the common input report.
const BUTTONS2: usize = 9;
const BUTTONS2_MIC_MUTE: u8 = 1 << 2;

const USB_OUTPUT_ID: u8 = 0x02;
/// USB output report length, report ID included: the report descriptor
/// declares report 0x02 as 47 bytes (macOS reports `MaxOutputReportSize`
/// 48). Linux's driver sends 63 bytes, which a descriptor-checking host may
/// refuse; everything we set sits in the first 47.
pub const USB_OUTPUT_LEN: usize = 48;
const BT_OUTPUT_ID: u8 = 0x31;
/// Bluetooth output report length, report ID and CRC included.
pub const BT_OUTPUT_LEN: usize = 78;
/// Bluetooth reports carry a fixed tag after the sequence byte.
const BT_OUTPUT_TAG: u8 = 0x10;
/// The byte hashed in front of a Bluetooth output report.
const BT_CRC_SEED: u8 = 0xa2;

// Offsets within `dualsense_output_report_common`.
const VALID_FLAG0: usize = 0;
const VALID_FLAG1: usize = 1;
const SPEAKER_VOLUME: usize = 5;
const AUDIO_CONTROL: usize = 7;
const MUTE_BUTTON_LED: usize = 8;
const POWER_SAVE_CONTROL: usize = 9;
const AUDIO_CONTROL2: usize = 37;

const FLAG0_SPEAKER_VOLUME: u8 = 1 << 5;
const FLAG0_AUDIO_CONTROL: u8 = 1 << 7;
const FLAG1_MIC_MUTE_LED: u8 = 1 << 0;
const FLAG1_POWER_SAVE: u8 = 1 << 1;
const FLAG1_AUDIO_CONTROL2: u8 = 1 << 7;
const POWER_SAVE_MIC_MUTE: u8 = 1 << 4;
/// Output path select, bits 5:4 of `audio_control`. 3 mutes the headphone
/// and plays the right channel on the speaker; 0 is the headphones (L/R).
const PATH_SPEAKER: u8 = 3 << 4;
/// Speaker volume Linux uses (it reports the accepted range as 0x3d..0x64).
const SPEAKER_VOLUME_MAX: u8 = 0x64;
/// Speaker preamp gain +6 dB (bits 2:0 of `audio_control2`), as Linux sets.
const SPEAKER_PREAMP: u8 = 2;

/// Where the common output block starts.
fn common_offset(t: Transport) -> usize {
    match t {
        Transport::Usb => 1,
        Transport::Bluetooth => 3,
    }
}

/// Whether the mute button is down in a DualSense input report, or `None` if
/// this is not a full input report (the short Bluetooth report 0x01 has no
/// mute bit).
pub fn mute_pressed(t: Transport, report: &[u8]) -> Option<bool> {
    let (id, len, base) = match t {
        Transport::Usb => (USB_INPUT_ID, USB_INPUT_LEN, 1),
        Transport::Bluetooth => (BT_INPUT_ID, BT_INPUT_LEN, 2),
    };
    if report.len() < len || report[0] != id {
        return None;
    }
    Some(report[base + BUTTONS2] & BUTTONS2_MIC_MUTE != 0)
}

/// The mute LED's three states (SDL: 0 off, 1 solid, 2 pulse).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MuteLed {
    Off = 0,
    On = 1,
    Pulse = 2,
}

/// The mic mute toggle the button drives: one press flips it, as on a PS5.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MicMute {
    muted: bool,
    held: bool,
}

impl MicMute {
    pub fn new(muted: bool) -> Self {
        Self { muted, held: false }
    }

    pub fn muted(&self) -> bool {
        self.muted
    }

    /// Feeds the button state from one input report. True when the mute
    /// state changed (the LED needs an update).
    pub fn update(&mut self, pressed: bool) -> bool {
        let edge = pressed && !self.held;
        self.held = pressed;
        if edge {
            self.muted = !self.muted;
        }
        edge
    }

    /// The LED for this state: lit while muted.
    pub fn led(&self) -> MuteLed {
        if self.muted {
            MuteLed::On
        } else {
            MuteLed::Off
        }
    }
}

/// What one output report sets. Fields left `None` are untouched: their
/// valid-flag bits stay clear, so the controller keeps its own state
/// (rumble, lightbar and player LEDs are never touched here).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OutputRequest {
    /// The mute LED, and with it the hardware mic mute (`Some(On)` or
    /// `Some(Pulse)` also mutes the mic in the controller).
    pub mute_led: Option<MuteLed>,
    /// `Some(true)` plays the right channel of the USB audio stream on the
    /// built-in speaker at full volume; `Some(false)` goes back to the
    /// headphone jack. Without this the firmware sends audio to the jack.
    pub speaker: Option<bool>,
}

/// Builds a DualSense output report. `seq` is the Bluetooth sequence number
/// (only its low four bits are sent; the caller increments it per report).
pub fn output_report(t: Transport, req: OutputRequest, seq: u8) -> Vec<u8> {
    let mut r = match t {
        Transport::Usb => {
            let mut r = vec![0u8; USB_OUTPUT_LEN];
            r[0] = USB_OUTPUT_ID;
            r
        }
        Transport::Bluetooth => {
            let mut r = vec![0u8; BT_OUTPUT_LEN];
            r[0] = BT_OUTPUT_ID;
            r[1] = (seq & 0x0f) << 4;
            r[2] = BT_OUTPUT_TAG;
            r
        }
    };
    let c = common_offset(t);
    if let Some(led) = req.mute_led {
        r[c + VALID_FLAG1] |= FLAG1_MIC_MUTE_LED | FLAG1_POWER_SAVE;
        r[c + MUTE_BUTTON_LED] = led as u8;
        if led != MuteLed::Off {
            r[c + POWER_SAVE_CONTROL] |= POWER_SAVE_MIC_MUTE;
        }
    }
    if let Some(speaker) = req.speaker {
        r[c + VALID_FLAG0] |= FLAG0_AUDIO_CONTROL;
        if speaker {
            r[c + AUDIO_CONTROL] = PATH_SPEAKER;
            r[c + VALID_FLAG0] |= FLAG0_SPEAKER_VOLUME;
            r[c + SPEAKER_VOLUME] = SPEAKER_VOLUME_MAX;
            r[c + VALID_FLAG1] |= FLAG1_AUDIO_CONTROL2;
            r[c + AUDIO_CONTROL2] = SPEAKER_PREAMP;
        }
    }
    if t == Transport::Bluetooth {
        let n = BT_OUTPUT_LEN - 4;
        let crc = crc32(&[&[BT_CRC_SEED], &r[..n]]);
        r[n..].copy_from_slice(&crc.to_le_bytes());
    }
    r
}

/// CRC-32 (IEEE, reflected, as zlib and Linux `~crc32_le(~0, ..)`) over the
/// concatenation of `parts`.
fn crc32(parts: &[&[u8]]) -> u32 {
    let mut crc = !0u32;
    for &b in parts.iter().flat_map(|p| p.iter()) {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_the_standard_check_value() {
        // The CRC-32/ISO-HDLC check value for "123456789".
        assert_eq!(crc32(&[b"1234", b"56789"]), 0xcbf4_3926);
    }

    #[test]
    fn mute_bit_in_usb_and_bluetooth_reports() {
        let mut usb = vec![0u8; 64];
        usb[0] = 0x01;
        assert_eq!(mute_pressed(Transport::Usb, &usb), Some(false));
        usb[10] = 0x04;
        assert_eq!(mute_pressed(Transport::Usb, &usb), Some(true));
        usb[10] = 0xfb; // every other buttons[2] bit
        assert_eq!(mute_pressed(Transport::Usb, &usb), Some(false));

        let mut bt = vec![0u8; 78];
        bt[0] = 0x31;
        bt[11] = 0x04;
        assert_eq!(mute_pressed(Transport::Bluetooth, &bt), Some(true));
        // The short Bluetooth report, a wrong ID and a truncated report.
        let short = [0x01u8; 10];
        assert_eq!(mute_pressed(Transport::Bluetooth, &short), None);
        assert_eq!(mute_pressed(Transport::Usb, &bt), None);
        assert_eq!(mute_pressed(Transport::Usb, &usb[..40]), None);
    }

    #[test]
    fn button_toggles_on_press_only() {
        let mut m = MicMute::default();
        assert_eq!(m.led(), MuteLed::Off);
        assert!(m.update(true));
        assert!(m.muted());
        assert!(!m.update(true), "holding does not toggle again");
        assert!(!m.update(false));
        assert!(m.muted());
        assert_eq!(m.led(), MuteLed::On);
        assert!(m.update(true));
        assert!(!m.muted());
    }

    #[test]
    fn usb_mute_led_report() {
        let r = output_report(
            Transport::Usb,
            OutputRequest {
                mute_led: Some(MuteLed::On),
                speaker: None,
            },
            0,
        );
        assert_eq!(r.len(), USB_OUTPUT_LEN);
        assert_eq!(r[0], 0x02);
        assert_eq!(r[1], 0, "flag0 untouched");
        assert_eq!(r[2], 0x03, "mute LED + power save valid");
        assert_eq!(r[9], 1, "LED solid");
        assert_eq!(r[10], 0x10, "mic muted in hardware");
        let off = output_report(
            Transport::Usb,
            OutputRequest {
                mute_led: Some(MuteLed::Off),
                speaker: None,
            },
            0,
        );
        assert_eq!((off[2], off[9], off[10]), (0x03, 0, 0));
        assert!(off[11..].iter().all(|&b| b == 0));
    }

    #[test]
    fn usb_speaker_path_report() {
        let r = output_report(
            Transport::Usb,
            OutputRequest {
                mute_led: None,
                speaker: Some(true),
            },
            0,
        );
        assert_eq!(r[1], 0xa0, "audio control + speaker volume valid");
        assert_eq!(r[2], 0x80, "audio control 2 valid");
        assert_eq!(r[6], 0x64);
        assert_eq!(r[8], 0x30, "path 3: right channel to the speaker");
        assert_eq!(r[38], 2, "+6 dB preamp");
        let jack = output_report(
            Transport::Usb,
            OutputRequest {
                mute_led: None,
                speaker: Some(false),
            },
            0,
        );
        assert_eq!((jack[1], jack[2], jack[8]), (0x80, 0, 0));
    }

    #[test]
    fn bluetooth_report_has_header_and_crc() {
        let req = OutputRequest {
            mute_led: Some(MuteLed::Pulse),
            speaker: None,
        };
        let r = output_report(Transport::Bluetooth, req, 0x15);
        assert_eq!(r.len(), 78);
        assert_eq!((r[0], r[1], r[2]), (0x31, 0x50, 0x10));
        assert_eq!((r[4], r[11], r[12]), (0x03, 2, 0x10));
        let crc = u32::from_le_bytes(r[74..].try_into().unwrap());
        assert_eq!(crc, crc32(&[&[0xa2], &r[..74]]));
        // Same body, different sequence: different CRC.
        let r2 = output_report(Transport::Bluetooth, req, 0x16);
        assert_ne!(r[74..], r2[74..]);
    }
}
