//! Override adapters for Halo 2 (retail 5849, `default.xbe`).
//!
//! This module is independent of `crates/runtime`: it exposes a table of
//! guest addresses and one dispatcher. [`crate::register`] binds them: for
//! each [`ENTRIES`] item it registers a handler that
//!
//! 1. reads `entry.argc` dwords from `[esp+4 ..]` (every entry is `stdcall`,
//!    `this` first; floats arrive as their 32-bit pattern),
//! 2. calls [`invoke`],
//! 3. writes `eax` (`Return::eax`), pops the return address plus
//!    `Return::pop_bytes` and resumes at the caller.
//!
//! A [`Stop`] is raised as a harness stop, never turned into `S_OK`.
//!
//! Nothing here depends on LTCG register conventions: all 66 entries below
//! were disassembled and are plain `stdcall` with stack arguments (the
//! `this`-adjusting thunks only move EAX/ECX/EDX internally).

use crate::host::Host;
use crate::state::{DirectSound, R, Stop};

/// One replaced guest function.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub va: u32,
    pub name: &'static str,
    /// Stack dwords, including `this`.
    pub argc: u32,
}

/// What to put in the guest CPU after the call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Return {
    pub eax: u32,
    /// Bytes the callee pops beyond the return address (`ret N`).
    pub pop_bytes: u32,
}

macro_rules! entries {
    ($(($va:expr, $name:expr, $argc:expr)),* $(,)?) => {
        pub const ENTRIES: &[Entry] = &[$(Entry { va: $va, name: $name, argc: $argc }),*];
    };
}

entries![
    // IDirectSound
    (0x0037d797, "DirectSoundCreate", 3),
    (0x0037d7de, "DirectSoundCreateBuffer", 2),
    (0x0037d835, "DirectSoundCreateStream", 2),
    (0x00379f2a, "IDirectSound_Release", 1),
    (0x0037d4be, "IDirectSound_CreateSoundBuffer", 4),
    (0x0037d4e2, "IDirectSound_CreateSoundStream", 4),
    (0x0037b5ae, "IDirectSound_GetCaps", 2),
    (0x0037b5ca, "IDirectSound_GetSpeakerConfig", 2),
    (0x0037b5e6, "IDirectSound_GetEffectData", 5),
    (0x0037b60d, "IDirectSound_SetEffectData", 6),
    (0x0037b637, "IDirectSound_SetMixBinHeadroom", 3),
    (0x0037b657, "IDirectSound_SynchPlayback", 1),
    (0x0037d141, "IDirectSound_CommitDeferredSettings", 1),
    (0x0037d506, "IDirectSound_SetDistanceFactor", 3),
    (0x0037d52a, "IDirectSound_SetDopplerFactor", 3),
    (0x0037d5cd, "IDirectSound_SetRolloffFactor", 3),
    (0x0037d598, "IDirectSound_SetPosition", 5),
    (0x0037d54e, "IDirectSound_SetOrientation", 8),
    (0x00379f5b, "DirectSoundUseLightHRTF4Channel", 0),
    (0x0037b844, "DirectSoundDoWork", 0),
    (0x0037b86d, "XAudioDownloadEffectsImage", 4),
    (0x0037ba6f, "XAudioSetEffectData", 3),
    // IDirectSoundBuffer
    (0x00379f45, "IDirectSoundBuffer_Release", 1),
    (0x00379f40, "IDirectSoundBuffer_Unlock", 5),
    (0x0037cc4a, "IDirectSoundBuffer_SetBufferData", 3),
    (0x0037b7b3, "IDirectSoundBuffer_Lock", 8),
    (0x0037b6df, "IDirectSoundBuffer_Play", 4),
    (0x0037b703, "IDirectSoundBuffer_Stop", 1),
    (0x0037b71b, "IDirectSoundBuffer_StopEx", 4),
    (0x0037b73f, "IDirectSoundBuffer_Pause", 2),
    (0x0037b75b, "IDirectSoundBuffer_GetStatus", 2),
    (0x0037b777, "IDirectSoundBuffer_GetCurrentPosition", 3),
    (0x0037b797, "IDirectSoundBuffer_SetCurrentPosition", 2),
    (0x0037b66f, "IDirectSoundBuffer_SetVolume", 2),
    (0x0037c5c8, "IDirectSoundBuffer_SetFrequency", 2),
    (0x0037b68b, "IDirectSoundBuffer_SetFilter", 2),
    (0x0037b6a7, "IDirectSoundBuffer_SetHeadroom", 2),
    (0x0037b6c3, "IDirectSoundBuffer_SetMixBinVolumes", 2),
    (0x0037c5e4, "IDirectSoundBuffer_SetMixBins", 2),
    (0x0037c668, "IDirectSoundBuffer_SetPosition", 5),
    (0x0037c644, "IDirectSoundBuffer_SetMinDistance", 3),
    (0x0037c620, "IDirectSoundBuffer_SetMaxDistance", 3),
    (0x0037c600, "IDirectSoundBuffer_SetConeOutsideVolume", 3),
    (0x0037c69d, "IDirectSoundBuffer_SetDopplerFactor", 3),
    (0x0037c6c1, "IDirectSoundBuffer_SetRolloffFactor", 3),
    (0x0037c6e5, "IDirectSoundBuffer_SetI3DL2Source", 3),
    (0x0037b7e3, "IDirectSoundBuffer_GetVoiceProperties", 2),
    // IDirectSoundStream, flat thunks (jmp to CDirectSoundStream_*)
    (0x0037b7ff, "IDirectSoundStream_SetVolume", 2),
    (0x0037b804, "IDirectSoundStream_SetPitch", 2),
    (0x0037b809, "IDirectSoundStream_SetLFO", 2),
    (0x0037b80e, "IDirectSoundStream_SetEG", 2),
    (0x0037b813, "IDirectSoundStream_SetFilter", 2),
    (0x0037b818, "IDirectSoundStream_SetHeadroom", 2),
    (0x0037b81d, "IDirectSoundStream_SetMixBinVolumes", 2),
    (0x0037c70a, "IDirectSoundStream_SetMixBins", 2),
    (0x0037c705, "IDirectSoundStream_SetOutputBuffer", 2),
    (0x0037b822, "IDirectSoundStream_Pause", 2),
    (0x0037b827, "IDirectSoundStream_FlushEx", 4),
    (0x0037b83f, "IDirectSoundStream_GetVoiceProperties", 2),
    // IDirectSoundStream, vtable methods at 0x00417170 (XMO order)
    (0x0037ab40, "CDirectSoundStream_AddRef", 1),
    (0x0037ab87, "CDirectSoundStream_Release", 1),
    (0x0037abd5, "CDirectSoundStream_GetInfo", 2),
    (0x0037acd4, "CDirectSoundStream_GetStatus", 2),
    (0x0037ad25, "CDirectSoundStream_Process", 3),
    (0x0037ac3c, "CDirectSoundStream_Discontinuity", 1),
    (0x0037ac89, "CDirectSoundStream_Flush", 1),
];

/// Looks an entry up by guest address.
pub fn entry(va: u32) -> Option<&'static Entry> {
    ENTRIES.iter().find(|e| e.va == va)
}

/// Runs the replacement for the function at `va` with the stack arguments
/// `args` (`args.len()` must equal the entry's `argc`).
pub fn invoke(
    ds: &mut DirectSound,
    h: &mut dyn Host,
    va: u32,
    args: &[u32],
) -> Result<Return, Stop> {
    let e = entry(va).ok_or(Stop::Unimplemented {
        what: "guest address with no DirectSound override",
    })?;
    assert_eq!(
        args.len() as u32,
        e.argc,
        "{}: wrong argument count",
        e.name
    );
    let a = |i: usize| args[i];
    let r: R = match e.name {
        "DirectSoundCreate" => ds.direct_sound_create(h, a(1)),
        "DirectSoundCreateBuffer" => ds.direct_sound_create_buffer(h, a(0), a(1)),
        "DirectSoundCreateStream" => ds.direct_sound_create_stream(h, a(0), a(1)),
        "IDirectSound_Release" | "IDirectSoundBuffer_Release" | "CDirectSoundStream_Release" => {
            ds.release(h, a(0))
        }
        "CDirectSoundStream_AddRef" => ds.add_ref(a(0)),
        "IDirectSound_CreateSoundBuffer" => ds.create_sound_buffer(h, a(0), a(1), a(2), a(3)),
        "IDirectSound_CreateSoundStream" => ds.create_sound_stream(h, a(0), a(1), a(2), a(3)),
        "IDirectSound_GetCaps" => ds.get_caps(h, a(0), a(1)),
        "IDirectSound_GetSpeakerConfig" => ds.get_speaker_config(h, a(0), a(1)),
        "IDirectSound_GetEffectData" => ds.get_effect_data(h, a(0), a(1), a(2), a(3), a(4)),
        "IDirectSound_SetEffectData" => ds.set_effect_data(h, a(0), a(1), a(2), a(3), a(4)),
        "IDirectSound_SetMixBinHeadroom" => ds.set_mix_bin_headroom(a(0), a(1), a(2)),
        "IDirectSound_SynchPlayback" => ds.synch_playback(a(0)),
        "IDirectSound_CommitDeferredSettings" => ds.commit_deferred(a(0)),
        "IDirectSound_SetDistanceFactor" => ds.set_distance_factor(a(0), a(1), a(2)),
        "IDirectSound_SetDopplerFactor" => ds.set_doppler_factor(a(0), a(1), a(2)),
        "IDirectSound_SetRolloffFactor" => ds.set_rolloff_factor(a(0), a(1), a(2)),
        "IDirectSound_SetPosition" => ds.set_listener_position(a(0), a(1), a(2), a(3), a(4)),
        "IDirectSound_SetOrientation" => {
            ds.set_listener_orientation(a(0), [a(1), a(2), a(3), a(4), a(5), a(6)], a(7))
        }
        "DirectSoundUseLightHRTF4Channel" => ds.use_light_hrtf_4_channel(),
        "DirectSoundDoWork" => ds.do_work(h),
        "XAudioDownloadEffectsImage" => ds.download_effects_image(h, a(0), a(1), a(2), a(3)),
        "XAudioSetEffectData" => ds.xaudio_set_effect_data(h, a(0), a(1), a(2)),
        "IDirectSoundBuffer_Unlock" => ds.unlock(h, a(0)),
        "IDirectSoundBuffer_SetBufferData" => ds.set_buffer_data(h, a(0), a(1), a(2)),
        "IDirectSoundBuffer_Lock" => ds.lock(h, a(0), a(1), a(2), a(3), a(4), a(5), a(6), a(7)),
        "IDirectSoundBuffer_Play" => ds.play(h, a(0), a(1), a(2), a(3)),
        "IDirectSoundBuffer_Stop" => ds.stop(a(0), 0),
        // Halo's only call site (0x220052) pushes ebp three times with ebp
        // zeroed at 0x21ff35: timestamp 0, flags 0. A timestamp is scheduled
        // on the DSP clock in the original and stays a stop.
        "IDirectSoundBuffer_StopEx" => {
            if a(1) != 0 || a(2) != 0 {
                Err(Stop::Unimplemented {
                    what: "StopEx with a future timestamp",
                })
            } else {
                ds.stop(a(0), a(3))
            }
        }
        "IDirectSoundBuffer_Pause" => ds.pause(a(0), a(1)),
        "IDirectSoundBuffer_GetStatus" => ds.get_status(h, a(0), a(1)),
        "IDirectSoundBuffer_GetCurrentPosition" => ds.get_current_position(h, a(0), a(1), a(2)),
        "IDirectSoundBuffer_SetCurrentPosition" => ds.set_current_position(a(0), a(1)),
        "IDirectSoundBuffer_SetVolume" => ds.buffer_set_volume(a(0), a(1) as i32),
        "IDirectSoundBuffer_SetFrequency" => ds.buffer_set_frequency(a(0), a(1)),
        "IDirectSoundBuffer_SetFilter" => ds.buffer_set_filter(a(0)),
        "IDirectSoundBuffer_SetHeadroom" => ds.buffer_set_headroom(a(0), a(1)),
        "IDirectSoundBuffer_SetMixBinVolumes" => ds.buffer_set_mix_bin_volumes(h, a(0), a(1)),
        "IDirectSoundBuffer_SetMixBins" => ds.buffer_set_mix_bins(h, a(0), a(1)),
        "IDirectSoundBuffer_SetPosition" => ds.buffer_set_position(a(0), a(1), a(2), a(3)),
        "IDirectSoundBuffer_SetMinDistance" => ds.buffer_set_min_distance(a(0), a(1)),
        "IDirectSoundBuffer_SetMaxDistance" => ds.buffer_set_max_distance(a(0), a(1)),
        "IDirectSoundBuffer_SetConeOutsideVolume" => {
            ds.buffer_set_3d_extra(a(0), "SetConeOutsideVolume")
        }
        "IDirectSoundBuffer_SetDopplerFactor" => ds.buffer_set_3d_extra(a(0), "SetDopplerFactor"),
        "IDirectSoundBuffer_SetRolloffFactor" => ds.buffer_set_3d_extra(a(0), "SetRolloffFactor"),
        "IDirectSoundBuffer_SetI3DL2Source" => ds.buffer_set_3d_extra(a(0), "SetI3DL2Source"),
        "IDirectSoundBuffer_GetVoiceProperties" => ds.buffer_get_voice_properties(h, a(0), a(1)),
        "IDirectSoundStream_GetVoiceProperties" => ds.stream_get_voice_properties(h, a(0), a(1)),
        "IDirectSoundStream_SetVolume" => ds.stream_set_volume(a(0), a(1) as i32),
        "IDirectSoundStream_SetPitch" => ds.stream_set_pitch(a(0), a(1) as i32),
        "IDirectSoundStream_SetLFO" => ds.stream_set_timbre(a(0), "SetLFO"),
        "IDirectSoundStream_SetEG" => ds.stream_set_timbre(a(0), "SetEG"),
        "IDirectSoundStream_SetFilter" => ds.stream_set_timbre(a(0), "SetFilter"),
        "IDirectSoundStream_SetOutputBuffer" => ds.stream_set_timbre(a(0), "SetOutputBuffer"),
        "IDirectSoundStream_SetHeadroom" => ds.stream_set_headroom(a(0), a(1)),
        "IDirectSoundStream_SetMixBinVolumes" => ds.stream_set_mix_bin_volumes(h, a(0), a(1)),
        "IDirectSoundStream_SetMixBins" => ds.stream_set_mix_bins(h, a(0), a(1)),
        "IDirectSoundStream_Pause" => ds.stream_pause(a(0), a(1)),
        "IDirectSoundStream_FlushEx" => {
            ds.stream_flush(h, a(0), (a(2) as u64) << 32 | a(1) as u64, a(3))
        }
        "CDirectSoundStream_GetInfo" => ds.stream_get_info(h, a(0), a(1)),
        "CDirectSoundStream_GetStatus" => ds.stream_get_status(h, a(0), a(1)),
        "CDirectSoundStream_Process" => ds.stream_process(h, a(0), a(1), a(2)),
        "CDirectSoundStream_Discontinuity" => ds.stream_discontinuity(a(0)),
        "CDirectSoundStream_Flush" => ds.stream_flush(h, a(0), 0, 0),
        other => unreachable!("{other} has an entry but no handler"),
    };
    Ok(Return {
        eax: r?,
        pop_bytes: 4 * e.argc,
    })
}
