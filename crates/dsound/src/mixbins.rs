//! A voice's mix-bin routing as the DirectSound library keeps it.
//!
//! The library holds two things per voice (`0x37bc89`, `0x37a4db`,
//! `0x37bd54`, `0x37a500`): an ordered list of up to eight bins the voice
//! feeds, and a volume for every bin, kept whether or not the bin is in the
//! list. The calls touch them differently:
//!
//! - `SetMixBins(pairs)`: the list becomes the pairs' bins and each pair sets
//!   its bin's volume; other bins keep theirs. A voice with a mix-in output
//!   buffer then gets the mix-in's input bin back on the end of the list.
//! - `SetMixBinVolumes(pairs)`: volumes only. A bin not in the list is not
//!   added (its volume is remembered for when it is).
//! - `SetOutputBuffer(mix-in)`: the old mix-in's bin leaves the list; a new
//!   mix-in replaces the whole list with its input bin, once per channel, at
//!   the level `1 / channels` (0 mB mono, -602 mB stereo). Null leaves the
//!   rest of the list as it was.
//!
//! So a stream sent to a mix-in still plays its own speaker bins if the game
//! sets them afterwards, and `SetMixBinVolumes` on the send bin scales the
//! send. Halo 2 puts its distance attenuation in exactly those volumes
//! (`docs/dsound.md`, "Distance attenuation").

use std::collections::BTreeMap;

/// Most bins a voice feeds (`DSMIXBINS` holds eight pairs).
pub const MAX_BINS: usize = 8;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VoiceBins {
    list: Vec<u32>,
    volumes: BTreeMap<u32, i32>,
}

/// The level the library gives a mix-in send: `1 / channels`, in millibels
/// (`0x379d54`: `2000·log10(gain)`).
pub fn send_volume(channels: usize) -> i32 {
    if channels <= 1 {
        0
    } else {
        (2000.0 * (1.0 / channels as f64).log10()).round() as i32
    }
}

impl VoiceBins {
    /// The routing a voice is created with.
    pub fn from_pairs(pairs: &[(u32, i32)]) -> Self {
        let mut v = Self::default();
        v.set_mix_bins(pairs, None);
        v
    }

    /// `SetMixBins`. `send` is the output buffer's input bin and the voice's
    /// channel count, when the voice has a mix-in output buffer.
    pub fn set_mix_bins(&mut self, pairs: &[(u32, i32)], send: Option<(u32, usize)>) {
        self.list = pairs.iter().map(|&(b, _)| b).take(MAX_BINS).collect();
        for &(bin, vol) in pairs {
            self.volumes.insert(bin, vol);
        }
        if let Some((bin, channels)) = send {
            self.add_send(bin, channels);
        }
    }

    /// `SetMixBinVolumes`: the volumes of the named bins, routed or not.
    pub fn set_volumes(&mut self, pairs: &[(u32, i32)]) {
        for &(bin, vol) in pairs {
            self.volumes.insert(bin, vol);
        }
    }

    /// `SetOutputBuffer`: `old` is the previous mix-in's input bin, `new` the
    /// new one's and the voice's channel count.
    pub fn set_output(&mut self, old: Option<u32>, new: Option<(u32, usize)>) {
        if let Some(old) = old {
            self.list.retain(|&b| b != old);
        }
        if let Some((bin, channels)) = new {
            self.list.clear();
            self.add_send(bin, channels);
            self.volumes.insert(bin, send_volume(channels));
        }
    }

    /// `0x37a500`: the bin appears once per channel, making room at the end
    /// of a full list.
    fn add_send(&mut self, bin: u32, channels: usize) {
        let want = channels.clamp(1, MAX_BINS);
        let have = self.list.iter().filter(|&&b| b == bin).count();
        if have >= want {
            return;
        }
        let add = want - have;
        self.list.truncate(MAX_BINS - add);
        self.list.extend(std::iter::repeat_n(bin, add));
    }

    /// The `(bin, volume)` pairs the voice is mixed through, in list order.
    pub fn pairs(&self) -> Vec<(u32, i32)> {
        self.list
            .iter()
            .map(|&b| (b, self.volumes.get(&b).copied().unwrap_or(0)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volumes_only_change_volumes() {
        let mut v = VoiceBins::from_pairs(&[(0, 0), (1, -100)]);
        v.set_volumes(&[(1, -300), (9, -50)]);
        assert_eq!(v.pairs(), vec![(0, 0), (1, -300)], "bin 9 is not added");
        // SetMixBins routes bin 9 with the volume it names.
        v.set_mix_bins(&[(9, -70)], None);
        assert_eq!(v.pairs(), vec![(9, -70)]);
    }

    #[test]
    fn a_mix_in_send_keeps_its_volume_and_the_speaker_bins() {
        let mut v = VoiceBins::from_pairs(&[(0, 0), (1, 0)]);
        v.set_output(None, Some((0x17, 1)));
        assert_eq!(v.pairs(), vec![(0x17, 0)], "the send replaces the list");
        // The game's own routing comes back with the send on the end...
        v.set_mix_bins(&[(6, -1200), (7, -1800)], Some((0x17, 1)));
        assert_eq!(v.pairs(), vec![(6, -1200), (7, -1800), (0x17, 0)]);
        // ...and SetMixBinVolumes on the send bin attenuates the send.
        v.set_volumes(&[(0x17, -2400)]);
        assert_eq!(v.pairs()[2], (0x17, -2400));
        // A pair naming the send bin is not duplicated.
        v.set_mix_bins(&[(0x17, -600)], Some((0x17, 1)));
        assert_eq!(v.pairs(), vec![(0x17, -600)]);
        // Null takes the send out and leaves the rest.
        v.set_mix_bins(&[(0, -10), (0x17, 0)], Some((0x17, 1)));
        v.set_output(Some(0x17), None);
        assert_eq!(v.pairs(), vec![(0, -10)]);
    }

    #[test]
    fn stereo_sends_once_per_channel_at_half_level() {
        assert_eq!(send_volume(1), 0);
        assert_eq!(send_volume(2), -602);
        let mut v = VoiceBins::from_pairs(&[(0, 0), (1, 0)]);
        v.set_output(None, Some((0x18, 2)));
        assert_eq!(v.pairs(), vec![(0x18, -602), (0x18, -602)]);
        // A full list makes room for the send.
        let eight: Vec<(u32, i32)> = (0..8).map(|b| (b, 0)).collect();
        v.set_mix_bins(&eight, Some((0x18, 2)));
        let p = v.pairs();
        assert_eq!(p.len(), MAX_BINS);
        assert_eq!(&p[6..], &[(0x18, -602), (0x18, -602)]);
    }
}
