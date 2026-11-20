//! Guest-side constants and structure layouts (XDK 5849, 32-bit little endian).
//!
//! Layouts marked "verified" were confirmed in Halo 2's own disassembly; the
//! rest are from the public XDK headers as remembered by Cxbx-Reloaded and are
//! flagged in `docs/audio/dsound.md`.

use crate::host::{GuestMemory, MemFault, at};
use audio::WaveFormat;

// HRESULTs ---------------------------------------------------------------
pub const S_OK: u32 = 0;
pub const E_PENDING: u32 = 0x8000_000A;
pub const E_NOTIMPL: u32 = 0x8000_4001;
pub const E_ABORT: u32 = 0x8000_4004;
pub const E_FAIL: u32 = 0x8000_4005;
pub const E_OUTOFMEMORY: u32 = 0x8007_000E;
pub const E_INVALIDARG: u32 = 0x8007_0057;
/// `DSERR_INVALIDCALL`. Verified: `CDirectSoundStream::Process` returns it
/// when the stream is not ready for a packet.
pub const DSERR_INVALIDCALL: u32 = 0x8878_0032;

// XMEDIAPACKET status values (verified for PENDING: written by
// CMcpxStream_Process at 0x380023; the others from the XDK header).
pub const XMEDIAPACKET_STATUS_SUCCESS: u32 = S_OK;
pub const XMEDIAPACKET_STATUS_PENDING: u32 = E_PENDING;
pub const XMEDIAPACKET_STATUS_FLUSHED: u32 = E_ABORT;
pub const XMEDIAPACKET_STATUS_FAILURE: u32 = E_FAIL;

// IDirectSoundStream::GetStatus bit (verified: Process tests bit 0 and
// fails with DSERR_INVALIDCALL when clear).
pub const XMO_STATUSF_ACCEPT_INPUT_DATA: u32 = 1;
/// Verified in CMcpxStream_GetStatus (0x37f5c2) and Halo's test at 0x2ae4c4.
pub const XMO_STATUSF_PAUSED: u32 = 0x2_0000;

// DSBCAPS. 0x10 and 0x2000 are verified by use; the combined mask 0x182000
// is what CMcpxBuffer_Play tests to force looping (0x382d2d), so those three
// bits mark buffers that carry no sample data of their own (mix-in/FX).
pub const DSBCAPS_CTRL3D: u32 = 0x0000_0010;
pub const DSBCAPS_MIXIN: u32 = 0x0000_2000;
pub const DSBCAPS_MIXIN_LIKE_MASK: u32 = 0x0018_2000;

// Play / Lock flags (Play: bit 4 verified at 0x382d4f, bit 2 at 0x382d7e).
pub const DSBPLAY_LOOPING: u32 = 0x1;
pub const DSBPLAY_FROMSTART: u32 = 0x2;
pub const DSBPLAY_SYNCHPLAYBACK: u32 = 0x4;
pub const DSBLOCK_FROMWRITECURSOR: u32 = 0x1;
pub const DSBLOCK_ENTIREBUFFER: u32 = 0x2;

// GetStatus. Decoded from CMcpxBuffer_GetStatus (0x3828d6): 1 = playing,
// 2 = paused, +4 = looping.
pub const DSBSTATUS_PLAYING: u32 = 0x1;
pub const DSBSTATUS_PAUSED: u32 = 0x2;
pub const DSBSTATUS_LOOPING: u32 = 0x4;

/// DSSPEAKER_STEREO. Halo checks `(cfg & 0xFFFF) == 2` and bit 16 (AC3).
pub const DSSPEAKER_STEREO: u32 = 2;

/// MCPX voices run at 48 kHz; `pitch = 4096 * log2(hz / 48000)`.
pub const PITCH_BASE_HZ: f64 = 48_000.0;

/// `XAudioCalculatePitch` (0x379f7a).
pub fn hz_to_pitch(hz: u32) -> i32 {
    (4096.0 * (hz.max(1) as f64 / PITCH_BASE_HZ).log2()).round() as i32
}
/// Inverse of [`hz_to_pitch`].
pub fn pitch_to_hz(pitch: i32) -> u32 {
    (PITCH_BASE_HZ * (pitch as f64 / 4096.0).exp2()).round().clamp(1.0, 4_000_000.0) as u32
}

/// `DSBUFFERDESC` (6 dwords; dwSize 0x18 verified at the Halo call sites).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BufferDesc {
    pub size: u32,
    pub flags: u32,
    pub buffer_bytes: u32,
    pub format_ptr: u32,
    pub mix_bins_ptr: u32,
    pub input_mix_bin: u32,
}

impl BufferDesc {
    pub const SIZE: u32 = 0x18;
    pub fn read(m: &dyn GuestMemory, addr: u32) -> Result<Self, MemFault> {
        Ok(Self {
            size: m.read_u32(addr)?,
            flags: m.read_u32(at(addr, 4)?)?,
            buffer_bytes: m.read_u32(at(addr, 8)?)?,
            format_ptr: m.read_u32(at(addr, 12)?)?,
            mix_bins_ptr: m.read_u32(at(addr, 16)?)?,
            input_mix_bin: m.read_u32(at(addr, 20)?)?,
        })
    }
}

/// `DSSTREAMDESC`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StreamDesc {
    pub flags: u32,
    pub max_attached_packets: u32,
    pub format_ptr: u32,
    pub callback: u32,
    pub callback_context: u32,
    pub mix_bins_ptr: u32,
}

impl StreamDesc {
    pub fn read(m: &dyn GuestMemory, addr: u32) -> Result<Self, MemFault> {
        Ok(Self {
            flags: m.read_u32(addr)?,
            max_attached_packets: m.read_u32(at(addr, 4)?)?,
            format_ptr: m.read_u32(at(addr, 8)?)?,
            callback: m.read_u32(at(addr, 12)?)?,
            callback_context: m.read_u32(at(addr, 16)?)?,
            mix_bins_ptr: m.read_u32(at(addr, 20)?)?,
        })
    }
}

/// `XMEDIAPACKET`: six dwords (verified: CMcpxStream_Process copies 6 dwords
/// with `rep movsd`, zeroes completed size and writes `E_PENDING`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MediaPacket {
    pub buffer: u32,
    pub max_size: u32,
    pub completed_size_ptr: u32,
    pub status_ptr: u32,
    /// `hCompletionEvent` or, when the stream has a callback,
    /// `dwCompletionContext`.
    pub event_or_context: u32,
    pub timestamp_ptr: u32,
}

impl MediaPacket {
    pub fn read(m: &dyn GuestMemory, addr: u32) -> Result<Self, MemFault> {
        Ok(Self {
            buffer: m.read_u32(addr)?,
            max_size: m.read_u32(at(addr, 4)?)?,
            completed_size_ptr: m.read_u32(at(addr, 8)?)?,
            status_ptr: m.read_u32(at(addr, 12)?)?,
            event_or_context: m.read_u32(at(addr, 16)?)?,
            timestamp_ptr: m.read_u32(at(addr, 20)?)?,
        })
    }
}

/// `DSMIXBINS`: `{ dwMixBinCount, lpMixBinVolumePairs }`, pairs are
/// `{ dwMixBin, lVolume }`. Verified at 0x220b10 (count 5).
pub fn read_mix_bins(m: &dyn GuestMemory, addr: u32) -> Result<Vec<(u32, i32)>, MemFault> {
    let count = m.read_u32(addr)?.min(32);
    let pairs = m.read_u32(at(addr, 4)?)?;
    (0..count)
        .map(|i| {
            let p = at(pairs, i * 8)?;
            Ok((m.read_u32(p)?, m.read_u32(at(p, 4)?)? as i32))
        })
        .collect()
}

/// `WAVEFORMATEX`, reduced to what [`WaveFormat`] needs. `cbSize` and the
/// Xbox ADPCM `wSamplesPerBlock` are not read: the audio crate fixes 64.
pub fn read_wave_format(m: &dyn GuestMemory, addr: u32) -> Result<Result<WaveFormat, audio::AudioError>, MemFault> {
    let b = m.read_vec(addr, 16)?;
    let tag = u16::from_le_bytes([b[0], b[1]]);
    let channels = u16::from_le_bytes([b[2], b[3]]);
    let rate = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
    let bits = u16::from_le_bytes([b[14], b[15]]);
    Ok(WaveFormat::from_wfx(tag, channels, rate, bits))
}
