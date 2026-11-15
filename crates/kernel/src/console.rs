//! Configured console values, not a fabricated EEPROM dump. See XAPI.md.
use crate::{
    Action, Core,
    abi::{Args, Value},
    ord, status,
};
use runtime::{Guest, GuestFault, guest::Access};
pub const XC_LANGUAGE: u32 = 7;
pub const XC_VIDEO: u32 = 8;
/// XC_AUDIO: analog/digital output flags (queried at retail 0x3739fa).
pub const XC_AUDIO: u32 = 9;
/// XC_AUDIO: analog output modes (low bits) and digital passthrough flags.
pub const XC_AUDIO_STEREO: u32 = 0;
pub const XC_AUDIO_MONO: u32 = 1;
pub const XC_AUDIO_SURROUND: u32 = 2;
pub const XC_AUDIO_FLAG_AC3: u32 = 0x0001_0000;
pub const XC_AUDIO_FLAG_DTS: u32 = 0x0002_0000;
pub const XC_P_CONTROL_GAMES: u32 = 10;
pub const XC_P_CONTROL_PASSWORD: u32 = 11;
pub const XC_P_CONTROL_MOVIES: u32 = 12;
/// User miscellaneous flags (EEPROM user section MiscFlags).
pub const XC_MISC: u32 = 0x11;
/// XC_MISC: automatic power-down enabled (XDK XC_MISC_FLAG_AUTOPOWERDOWN).
pub const XC_MISC_FLAG_AUTOPOWERDOWN: u32 = 0x01;
/// XC_MISC: automatic daylight-saving adjustment disabled.
pub const XC_MISC_FLAG_DISABLE_DST: u32 = 0x02;
pub const XC_FACTORY_AV_REGION: u32 = 0x103;
/// XC_MAX_ALL: the whole 256-byte EEPROM image.
pub const XC_MAX_ALL: u32 = 0xffff;
/// XC_MAX_OS: the whole OS/user settings section (EEPROM 0x60..0xFF). Halo's
/// XAPI GetTimeZoneInformation helper (0x331406) queries it with a 0x100-byte
/// buffer; its field offsets (bias +4, names +8/+0xC, dates +0x18/+0x1C,
/// biases +0x28/+0x2C, misc flags +0x58) are the documented user-section
/// layout (xboxdevwiki "EEPROM", 0x60 onward) rebased to the section start.
pub const XC_MAX_OS: u32 = 0xff;
pub const XC_FACTORY_GAME_REGION: u32 = 0x104;
pub const AV_STANDARD_NTSC_M: u32 = 0x100;
pub const AV_STANDARD_NTSC_J: u32 = 0x200;
pub const AV_STANDARD_PAL_I: u32 = 0x300;
pub const AV_FLAGS_60HZ: u32 = 0x400000;
pub const AV_FLAGS_480P: u32 = 0x80000;
pub const AV_PACK_HDTV: u32 = 4;
/// AvSendTVEncoderOption options Halo passes (call sites in XAPI.md).
/// AV_QUERY_AV_CAPABILITIES.
pub const AV_OPTION_QUERY_SETTINGS: u32 = 6;
pub const AV_OPTION_BLANK_SCREEN: u32 = 9;
pub const AV_OPTION_FLICKER_FILTER: u32 = 11;
pub const AV_OPTION_ENABLE_LUMA_FILTER: u32 = 14;
/// AV_OPTION_GUESS_FIELD: no documented result, so it stops.
pub const AV_OPTION_GUESS_FIELD: u32 = 15;
/// D3DDevice_SetFlickerFilter levels: 0 (off) to 5 (AvSetDisplayMode's).
pub const MAX_FLICKER_FILTER: u32 = 5;

/// What the title has told the TV encoder. `None` until first set; the
/// renderer may read it, nothing here programs a display.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EncoderState {
    pub blanked: Option<bool>,
    pub flicker_filter: Option<u32>,
    pub luma_filter: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsoleConfig {
    pub language: u32,
    pub video_standard: u32,
    pub factory_video_flags: u32,
    pub video_flags: u32,
    pub av_pack: u32,
    pub game_region: u32,
    pub parental_games: u32,
    pub parental_password: u32,
    pub parental_movies: u32,
    /// XC_MISC flags; zero means auto power-down off and DST adjustment on.
    pub misc_flags: u32,
    /// XC_AUDIO setting: stereo with no digital passthrough by default.
    pub audio_flags: u32,
    /// PhyGetLinkState mask (net.rs); 0 is no cable, the offline default.
    pub phy_link: u32,
}
impl Default for ConsoleConfig {
    fn default() -> Self {
        Self {
            language: 1,
            video_standard: AV_STANDARD_NTSC_M,
            factory_video_flags: AV_FLAGS_60HZ,
            video_flags: AV_FLAGS_480P,
            av_pack: AV_PACK_HDTV,
            game_region: 1,
            parental_games: 0,
            parental_password: 0,
            parental_movies: 0,
            misc_flags: 0,
            audio_flags: XC_AUDIO_STEREO,
            phy_link: 0,
        }
    }
}
impl ConsoleConfig {
    pub fn setting(&self, index: u32) -> Option<u32> {
        Some(match index {
            XC_LANGUAGE => self.language,
            XC_VIDEO => self.video_flags,
            XC_AUDIO => self.audio_flags,
            XC_P_CONTROL_GAMES => self.parental_games,
            XC_P_CONTROL_PASSWORD => self.parental_password,
            XC_P_CONTROL_MOVIES => self.parental_movies,
            XC_MISC => self.misc_flags,
            XC_FACTORY_AV_REGION => self.video_standard | self.factory_video_flags,
            XC_FACTORY_GAME_REGION => self.game_region,
            _ => return None,
        })
    }
    pub fn encoder_settings(&self) -> u32 {
        self.av_pack | self.video_standard | self.factory_video_flags | self.video_flags
    }
}
impl Core {
    pub(crate) fn console_call(
        &mut self,
        memory: &mut Guest,
        a: &Args,
    ) -> Result<Action, GuestFault> {
        if a.export.ordinal == ord::AV_SEND_TV_ENCODER_OPTION {
            return self.encoder_option(memory, a);
        }
        // No EEPROM image is configured (and none is fabricated), so the
        // whole-EEPROM and whole-user-section queries find nothing; nothing
        // is written. XAPI's XQueryValue (0x2d6f62) converts this to
        // ERROR_FILE_NOT_FOUND. XC_MAX_OS is answered the same way rather
        // than synthesised: the section layout and checksum (CRC over
        // 0x64..0xBF) are documented, but how the retail kernel sizes and
        // types a XC_MAX_OS reply is not, and the only consumer
        // (GetTimeZoneInformation, 0x331406) fails soft on an error: it
        // returns TIME_ZONE_ID_INVALID and GetLocalTime (0x33198b) then uses
        // a zero bias, the same result as a synthesised UTC section.
        if matches!(a.get(0), XC_MAX_ALL | XC_MAX_OS) {
            return Ok(Action::status(status::OBJECT_NAME_NOT_FOUND));
        }
        let Some(value) = self.console.setting(a.get(0)) else {
            return Ok(Action::Stop("unknown/unconfigured nonvolatile setting"));
        };
        let result_length = a.get(4);
        if result_length != 0 {
            memory.check(result_length, 4, Access::Write, 1)?;
        }
        if a.get(3) < 4 {
            if result_length != 0 {
                memory.set_u32(result_length, 4)?;
            }
            return Ok(Action::status(status::BUFFER_TOO_SMALL));
        }
        memory.check(a.get(1), 4, Access::Write, 1)?;
        memory.check(a.get(2), a.get(3) as usize, Access::Write, 1)?;
        // Match the observed/public query contract: report DWORD and clear any
        // caller-provided tail; preflight every output before writing any of them.
        if result_length != 0 {
            memory.set_u32(result_length, 4)?;
        }
        memory.set_u32(a.get(1), 4)?; // REG_DWORD
        // Bound host scratch independently of guest's chosen buffer length.
        let zero = [0u8; 4096];
        let mut cursor = a.get(2);
        let mut remaining = a.get(3);
        while remaining != 0 {
            let count = remaining.min(zero.len() as u32);
            memory.write_bytes(cursor, &zero[..count as usize], 1)?;
            remaining -= count;
            if remaining != 0 {
                cursor += count;
            }
        }
        memory.set_u32(a.get(2), value)?;
        Ok(Action::status(status::SUCCESS))
    }
}

impl Core {
    /// AvSendTVEncoderOption(RegisterBase, Option, Param, Result), VOID.
    /// RegisterBase (NV2A registers, or null for the query) is not touched.
    /// Setting options write nothing to Result; Halo passes null there.
    fn encoder_option(&mut self, memory: &mut Guest, a: &Args) -> Result<Action, GuestFault> {
        let param = a.get(2);
        match a.get(1) {
            AV_OPTION_QUERY_SETTINGS => {
                if param != 0 {
                    return Ok(Action::Stop("AV capabilities query with a Param"));
                }
                memory.set_u32(a.get(3), self.console.encoder_settings())?;
            }
            // Halo pushes literal 0 (unblank) or 1 (blank).
            AV_OPTION_BLANK_SCREEN => {
                if param > 1 {
                    return Ok(Action::Stop("AV blank-screen Param other than 0/1"));
                }
                self.encoder.blanked = Some(param != 0);
            }
            AV_OPTION_FLICKER_FILTER => {
                if param > MAX_FLICKER_FILTER {
                    return Ok(Action::Stop("AV flicker filter level above 5"));
                }
                self.encoder.flicker_filter = Some(param);
            }
            // A BOOL: D3D (0x3f8c9c) passes its argument unnormalized.
            AV_OPTION_ENABLE_LUMA_FILTER => self.encoder.luma_filter = Some(param != 0),
            AV_OPTION_GUESS_FIELD => {
                return Ok(Action::Stop("AV field guess has no documented result"));
            }
            _ => return Ok(Action::Stop("unsupported video encoder option")),
        }
        Ok(Action::Complete(Value::Void))
    }
}
