//! Xbox mix bins folded down to stereo.
//!
//! An Xbox voice does not have a pan: it has up to eight `(mix bin, volume)`
//! pairs (`DSMIXBINS`). The MCPX summed every voice into 32 mono bins: six
//! speaker bins, four crosstalk bins that the DSP's 3D (HRTF) output used, a
//! reverb send and twenty FX sends that mix-in buffers and DSP effects read.
//! The bin numbers are the XDK's `DSMIXBIN_*` values; Halo 2's own tables
//! agree (default 3D routing `{6, 8, 7, 9, 10}`, mix-in input bins `0x17..`,
//! see `docs/dsound.md`).
//!
//! This model keeps the routing and folds the result to stereo:
//!
//! | bins | name | goes to |
//! |---|---|---|
//! | 0, 4 | front left, back left | left |
//! | 1, 5 | front right, back right | right |
//! | 2 | centre | both, at -3 dB |
//! | 3 | LFE | dropped |
//! | 6, 8 | crosstalk front/back left | left; a 3D voice is panned and split front/back |
//! | 7, 9 | crosstalk front/back right | right; likewise |
//! | 10 | I3DL2 reverb send | dropped (reverb is not modelled) |
//! | 11..=30 | FX sends 0..19 | every playing [`MixIn`](crate::dsound::MixIn) whose input bin it is; otherwise dropped (DSP effects) |
//!
//! Channels: the pairs of a mono voice all carry its one channel. For a
//! multichannel voice, pair `i` carries channel `i % channels` (stereo: even
//! pairs left, odd pairs right), which is how the default routings line up
//! (`{0, 1}` stereo, `{0, 1, 4, 5}` and `{0..5}` for 4 and 6 channels).
//! Above two channels only the first two are kept, as the mixer's voices
//! are stereo.

/// `DSMIXBIN_FRONT_LEFT`.
pub const FRONT_LEFT: u32 = 0;
pub const FRONT_RIGHT: u32 = 1;
pub const FRONT_CENTER: u32 = 2;
pub const LOW_FREQUENCY: u32 = 3;
pub const BACK_LEFT: u32 = 4;
pub const BACK_RIGHT: u32 = 5;
pub const XTLK_FRONT_LEFT: u32 = 6;
pub const XTLK_FRONT_RIGHT: u32 = 7;
pub const XTLK_BACK_LEFT: u32 = 8;
pub const XTLK_BACK_RIGHT: u32 = 9;
pub const I3DL2: u32 = 10;
/// `DSMIXBIN_FXSEND_0`; the sends run to `FXSEND_LAST`.
pub const FXSEND_FIRST: u32 = 11;
pub const FXSEND_LAST: u32 = 30;

/// Centre bin level in each output channel (-3 dB).
pub const CENTER_GAIN: f32 = std::f32::consts::FRAC_1_SQRT_2;

/// Where one mix bin's signal ends up.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Dest {
    /// Straight to the stereo output with these channel weights.
    Out { left: f32, right: f32 },
    /// A crosstalk bin: `left` side or not, and front or back.
    Crosstalk { left: bool, front: bool },
    /// An FX send, read by mix-ins with this input bin.
    Send(u32),
    /// Not modelled (LFE, reverb, unknown).
    Dropped,
}

/// The destination of `bin`, per the table in the module docs.
pub fn dest(bin: u32) -> Dest {
    match bin {
        FRONT_LEFT | BACK_LEFT => Dest::Out { left: 1.0, right: 0.0 },
        FRONT_RIGHT | BACK_RIGHT => Dest::Out { left: 0.0, right: 1.0 },
        FRONT_CENTER => Dest::Out { left: CENTER_GAIN, right: CENTER_GAIN },
        XTLK_FRONT_LEFT => Dest::Crosstalk { left: true, front: true },
        XTLK_FRONT_RIGHT => Dest::Crosstalk { left: false, front: true },
        XTLK_BACK_LEFT => Dest::Crosstalk { left: true, front: false },
        XTLK_BACK_RIGHT => Dest::Crosstalk { left: false, front: false },
        FXSEND_FIRST..=FXSEND_LAST => Dest::Send(bin),
        _ => Dest::Dropped,
    }
}

/// The voice channel (0 left, 1 right of the rendered stereo frame) that mix
/// bin pair `index` carries for a voice with `channels` channels.
pub fn channel_of(index: usize, channels: usize) -> usize {
    if channels <= 1 { 0 } else { index % channels.min(2) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bins_fold_to_stereo() {
        assert_eq!(dest(0), Dest::Out { left: 1.0, right: 0.0 });
        assert_eq!(dest(5), Dest::Out { left: 0.0, right: 1.0 });
        assert_eq!(dest(2), Dest::Out { left: CENTER_GAIN, right: CENTER_GAIN });
        assert_eq!(dest(3), Dest::Dropped);
        assert_eq!(dest(8), Dest::Crosstalk { left: true, front: false });
        assert_eq!(dest(10), Dest::Dropped);
        assert_eq!(dest(0x17), Dest::Send(0x17));
        assert_eq!(dest(31), Dest::Dropped);
        assert_eq!((channel_of(3, 1), channel_of(3, 2), channel_of(5, 6)), (0, 1, 1));
    }
}
