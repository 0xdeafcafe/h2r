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
pub const XC_FACTORY_GAME_REGION: u32 = 0x104;
pub const AV_STANDARD_NTSC_M: u32 = 0x100;
pub const AV_STANDARD_NTSC_J: u32 = 0x200;
pub const AV_STANDARD_PAL_I: u32 = 0x300;
pub const AV_FLAGS_60HZ: u32 = 0x400000;
pub const AV_FLAGS_480P: u32 = 0x80000;
pub const AV_PACK_HDTV: u32 = 4;
pub const AV_OPTION_QUERY_SETTINGS: u32 = 6;

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
            if a.get(1) != AV_OPTION_QUERY_SETTINGS || a.get(2) != 0 {
                return Ok(Action::Stop("unsupported video encoder option"));
            }
            memory.set_u32(a.get(3), self.console.encoder_settings())?;
            return Ok(Action::Complete(Value::Void));
        }
        // No EEPROM image is configured (and none is fabricated), so the
        // whole-EEPROM query finds nothing; nothing is written. XAPI's
        // XQueryValue (0x2d6f62) converts this to ERROR_FILE_NOT_FOUND.
        if a.get(0) == XC_MAX_ALL {
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
