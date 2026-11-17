//! Halo 1's cutscenes in a converted level (docs/game/campaigns.md §9,
//! Cutscenes): what the scripts' cinematic calls need besides the
//! translation `h1scripts` does.
//!
//! - [`impulse_ticks`]: the length of a dialogue line, which
//!   `(sound_impulse_time <sound>)` returns and the scripts sleep on
//!   (`h1scripts::compile::SOUND_LENGTH`).
//! - [`install_graphs`]: the animation graphs the scripts name
//!   (`custom_animation`, `scenery_animation_start`, ...), converted to
//!   `jmad` under their Halo 1 names, and each animation name the scripts
//!   use mapped to the converted animation's
//!   (`h1scripts::compile::ANIMATION_NAME`).
//! - [`read_recordings`], [`recordings_block`]: Halo 1's recorded
//!   animations (unit control streams the scripts play on units and
//!   vehicles with `recording_play` and kin) as Halo 2's scenario block
//!   `+0x110`, and the `cutscene_recording` hooks as indices into it.

use std::collections::{BTreeMap, HashMap};

use h1map::{Group, H1Map};
use h1scripts::compile::{ANIMATION_NAME, Compiled, Hook};
use h1sounds::h1::{Compression, Sound};
use map::FourCC;
use map::build::TagSet;

use crate::{Options, Result, invalid};
use h1scripts::compile::Cutscenes;

/// Each level's cutscene policy when [`Options::cutscenes`] is `None`, and
/// why. A level not listed skips (Halo 1's skip path).
///
/// Played: every graph the cutscenes name converts, every recording and
/// dialogue length is written, and no call left stubbed in a cinematic
/// holds it (`cutscenes_convert_and_read_back` lists them: HUD and
/// screen effects, `object_beautify`, `object_pvs_activate`,
/// `unit_set_emotion`, `unit_set_seat`, command lists).
pub const LEVELS: &[(&str, Cutscenes, &str)] = &[
    (
        "a10",
        Cutscenes::PlayShortPaths,
        "cinematics played; the cryo tutorial still takes fast_setup (its look tests have no Halo 2 builtin)",
    ),
    ("a30", Cutscenes::Play, "every cutscene converts"),
    ("a50", Cutscenes::Play, "every cutscene converts"),
    ("b30", Cutscenes::Play, "every cutscene converts"),
    (
        "b40",
        Cutscenes::Skip,
        "at the tag memory bound: converted without level data, graphs and recordings, so its cutscenes would have no camera points, flags or animations",
    ),
    (
        "c10",
        Cutscenes::Play,
        "every cutscene converts; the marines' command lists in x50 are stubbed (they stand)",
    ),
    ("c20", Cutscenes::Play, "every cutscene converts"),
    (
        "c40",
        Cutscenes::Play,
        "every cutscene converts; command lists stubbed (actors stand)",
    ),
    ("d20", Cutscenes::Play, "every cutscene converts"),
    (
        "d40",
        Cutscenes::Skip,
        "over the campaign cache's file size with the cutscene graphs (they need 0x1baa00 bytes, 0x7c000 are left), so its characters would not animate",
    ),
];

/// The level's own cutscene policy ([`LEVELS`]).
pub fn default_for(level: &str) -> Cutscenes {
    LEVELS
        .iter()
        .find(|(l, _, _)| l.eq_ignore_ascii_case(level))
        .map_or(Cutscenes::Skip, |e| e.1)
}

/// The policy a conversion of `level` uses: the caller's, or the level's.
pub fn policy(opts: &Options, level: &str) -> Cutscenes {
    opts.cutscenes.unwrap_or_else(|| default_for(level))
}

/// Ticks per second of Halo 1's and Halo 2's game time.
pub const TICKS_PER_SECOND: u32 = 30;

/// Samples per channel in one Xbox ADPCM block of 36 bytes.
const ADPCM_BLOCK: usize = 36;
const ADPCM_SAMPLES: usize = 64;

/// Samples per channel of `bytes` encoded samples.
fn samples(compression: Compression, bytes: usize, channels: usize) -> Option<usize> {
    match compression {
        Compression::XboxAdpcm => Some(bytes / (ADPCM_BLOCK * channels) * ADPCM_SAMPLES),
        Compression::None => Some(bytes / (2 * channels)),
        _ => None,
    }
}

/// How long `sound` plays, in ticks, rounded up: its first pitch range's
/// first permutation and the permutations that continue it (dialogue has
/// one permutation; Halo 1 splits a long line into a chain). `None` when
/// the sound has no permutation or a compression the Xbox does not use.
pub fn impulse_ticks(sound: &Sound) -> Option<i32> {
    let hz = sound.hz().ok()? as usize;
    let channels = sound.channels().ok()?;
    let range = sound.pitch_ranges.first()?;
    let chain = sound.chains(0).ok()?.into_iter().next()?;
    let mut total = 0usize;
    for p in chain {
        let p = range.permutations.get(p)?;
        total += samples(p.compression, p.samples.size as usize, channels)?;
    }
    let ticks = (total * TICKS_PER_SECOND as usize).div_ceil(hz);
    i32::try_from(ticks).ok()
}

/// The data word of a [`h1scripts::compile::SOUND_LENGTH`] hook: the
/// length in ticks of the Halo 1 sound its token names, as the hook's
/// number type. `None` (the literal stays 0) when the level has no such
/// sound.
pub fn sound_length_hook(h1: &h1map::H1Map, h: &h1scripts::compile::Hook) -> Option<u32> {
    let tag = h1.find(h1sounds::h1::SND, &h.token)?;
    let ticks = impulse_ticks(&Sound::read(h1, tag).ok()?)?;
    h1scripts::compile::number_bits(h.h2_type, ticks)
}

/// The animation graphs a level's scripts name, and the Halo 2 name of
/// each animation they name.
#[derive(Debug, Default, Clone)]
pub struct Graphs {
    /// Graphs converted here (not already converted with an object):
    /// Halo 1 tag name, animations.
    pub converted: Vec<(String, usize)>,
    /// Bytes of animation data (raw, in the map file outside tag memory)
    /// the converted graphs add.
    pub raw_bytes: usize,
    /// Graphs named by the scripts that were converted already (an
    /// object's own graph).
    pub shared: Vec<String>,
    /// Graphs that could not be converted, with the reason.
    pub dropped: Vec<String>,
    /// (graph, Halo 1 animation name, both lower case) to the converted
    /// animation's name.
    names: HashMap<(String, String), String>,
}

impl Graphs {
    /// The Halo 2 animation name an [`ANIMATION_NAME`] hook's token
    /// (`<graph>|<animation>`) stands for: the name the converted graph
    /// gave that animation, or, for a graph that was not converted or an
    /// animation it lacks, the Halo 1 name mapped the same way
    /// (`h1models::names::map_name`).
    pub fn h2_name(&self, token: &str) -> String {
        let (graph, anim) = token.split_once('|').unwrap_or(("", token));
        self.names
            .get(&(graph.to_ascii_lowercase(), anim.to_ascii_lowercase()))
            .cloned()
            .unwrap_or_else(|| h1models::names::map_name(anim).full())
    }
}

/// The Halo 1 model whose node list checksum is `checksum`: the skeleton
/// of a graph with no node block of its own (vehicles, scenery and
/// devices animate their model's nodes; their animations carry its
/// checksum).
fn model_by_checksum(h1: &H1Map, checksum: u32) -> Option<&h1map::Tag> {
    let mode = Group::new(b"mode");
    h1.tags().iter().find(|t| {
        t.group == mode
            && h1
                .read(t.data_address.wrapping_add(4), 4)
                .is_ok_and(|b| u32::from_le_bytes(b.try_into().unwrap_or([0; 4])) == checksum)
    })
}

/// Converts every Halo 1 animation graph the scripts' animation hooks name
/// (`animation_graph` atoms and [`ANIMATION_NAME`] tokens) that `set` does
/// not hold as a `jmad` of that name yet, and maps the animation names.
pub fn install_graphs(h1: &H1Map, set: &mut TagSet, c: &Compiled) -> Result<Graphs> {
    let antr = Group::new(b"antr");
    let jmad = FourCC(*b"jmad");
    let mut wanted: BTreeMap<String, ()> = BTreeMap::new();
    for h in &c.hooks {
        if h.type_name == "animation_graph" {
            wanted.insert(h.token.clone(), ());
        } else if h.h1_type == ANIMATION_NAME
            && let Some((g, _)) = h.token.split_once('|')
        {
            wanted.insert(g.to_owned(), ());
        }
    }
    let mut out = Graphs::default();
    for name in wanted.into_keys() {
        let Some(tag) = h1.find(antr, &name) else {
            out.dropped.push(format!("{name}: not in the level"));
            continue;
        };
        let graph = h1models::h1::Graph::read(h1, tag.data_address)?;
        for (a, (n, _)) in graph
            .animations
            .iter()
            .zip(h1models::jmad::animation_names(&graph))
        {
            out.names.insert(
                (name.to_ascii_lowercase(), a.name.to_ascii_lowercase()),
                n.full(),
            );
        }
        let have = set.slots.iter().any(|s| {
            s.name.as_deref() == Some(name.as_str())
                && s.tag.as_ref().is_some_and(|t| t.group == jmad)
        });
        if have {
            out.shared.push(name);
            continue;
        }
        let model = match graph.nodes.is_empty() {
            true => graph
                .animations
                .first()
                .and_then(|a| model_by_checksum(h1, a.node_list_checksum)),
            false => None,
        };
        match h1models::graph_from_map(h1, tag, model) {
            Ok((g, _, conv)) => {
                out.raw_bytes += conv.raw.iter().map(|r| r.0.len()).sum::<usize>();
                conv.install(set, &name)
                    .map_err(|e| invalid(format!("{name}: {e}")))?;
                out.converted.push((name, g.animations.len()));
            }
            Err(e) => out.dropped.push(format!("{name}: {e}")),
        }
    }
    Ok(out)
}

/// Halo 1 `scnr` recorded animations block: count, address; `0x40` each.
pub const H1_RECORDINGS: usize = 0x36c;
const H1_RECORDING_LEN: usize = 0x40;
/// Halo 2 `scnr` recorded animations block (E: `0x001fb360` bounds a
/// recording index by `scnr +0x110` and reads `0x34`-byte elements at
/// `+0x114`; no retail scenario holds one).
pub const H2_RECORDINGS: u32 = 0x110;
pub const H2_RECORDING_LEN: usize = 0x34;
/// Bytes both games' elements share before the event stream's data
/// reference: the name (32), version `+0x20`, raw animation data `+0x21`,
/// unit control data version `+0x22`, length in ticks `+0x24` (E: Halo 2
/// reads `+0x20`, `+0x22` and `+0x24` at `0x001fb3f4`..`0x001fb422`).
const SHARED: usize = 0x2c;

/// One Halo 1 recorded animation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recording {
    pub name: String,
    /// The element's first [`SHARED`] bytes, as Halo 1 stores them.
    pub head: Vec<u8>,
    /// The event stream (Halo 1 data reference `+0x2c`; Halo 2 reads the
    /// pointer at `+0x30`).
    pub stream: Vec<u8>,
}

impl Recording {
    /// Version byte (`+0x20`): Halo 2 decodes versions 1 to 4 (its decoder
    /// table at `0x0046fd5c` holds four entries).
    pub fn version(&self) -> u8 {
        self.head[0x20]
    }
    /// Unit control data version (`+0x22`).
    pub fn control_version(&self) -> u8 {
        self.head[0x22]
    }
    /// Length in ticks (`+0x24`).
    pub fn ticks(&self) -> i16 {
        i16::from_le_bytes([self.head[0x24], self.head[0x25]])
    }
}

/// The level's recorded animations, in Halo 1's order.
pub fn read_recordings(h1: &H1Map) -> Result<Vec<Recording>> {
    let scnr = h1.tag_data(h1.scenario())?;
    let w = |o: usize| u32::from_le_bytes(scnr[o..o + 4].try_into().expect("4 bytes"));
    let (count, address) = (w(H1_RECORDINGS) as usize, w(H1_RECORDINGS + 4));
    if count == 0 {
        return Ok(Vec::new());
    }
    let all = h1.read(address, count * H1_RECORDING_LEN)?;
    let mut out = Vec::with_capacity(count);
    for e in all.as_chunks::<H1_RECORDING_LEN>().0 {
        let name_end = e[..32].iter().position(|&c| c == 0).unwrap_or(32);
        let size = u32::from_le_bytes(e[0x2c..0x30].try_into().expect("4 bytes")) as usize;
        let at = u32::from_le_bytes(e[0x38..0x3c].try_into().expect("4 bytes"));
        let stream = match size {
            0 => Vec::new(),
            n => h1.read(at, n)?.to_vec(),
        };
        out.push(Recording {
            name: String::from_utf8_lossy(&e[..name_end]).into_owned(),
            head: e[..SHARED].to_vec(),
            stream,
        });
    }
    Ok(out)
}

/// Halo 2 elements for `recordings`: the shared bytes as they are, the
/// event stream behind the data reference at `+0x2c`.
pub fn recordings_block(recordings: &[Recording]) -> Vec<map::build::Block> {
    recordings
        .iter()
        .map(|r| {
            let mut b = map::build::Block::zeroed(H2_RECORDING_LEN);
            b.bytes[..SHARED].copy_from_slice(&r.head);
            b.data(SHARED as u32, map::build::Block::new(r.stream.clone()));
            b
        })
        .collect()
}

/// A `cutscene_recording` hook's value: the recording's index in Halo 1's
/// order (the block is written in that order), names matched without case.
pub fn recording_index(recordings: &[Recording], token: &str) -> Option<u32> {
    recordings
        .iter()
        .position(|r| r.name.eq_ignore_ascii_case(token))
        .map(|i| i as u32)
}

/// The string a hook on an animation name interns.
pub fn animation_name(graphs: &Graphs, h: &Hook) -> String {
    graphs.h2_name(&h.token)
}

#[cfg(test)]
mod tests {
    use super::*;
    use h1sounds::h1::{DataRef, Permutation, PitchRange};

    fn sound(perms: &[(u32, u16)], rate: u16, encoding: u16) -> Sound {
        let permutations = perms
            .iter()
            .map(|&(size, next)| Permutation {
                name: String::new(),
                skip_fraction: 0.0,
                gain: 1.0,
                compression: Compression::XboxAdpcm,
                next,
                samples: DataRef {
                    size,
                    ..DataRef::default()
                },
                mouth: DataRef::default(),
                subtitles: DataRef::default(),
            })
            .collect();
        Sound {
            flags: 0,
            class: 0,
            sample_rate: rate,
            distance: [0.0; 2],
            skip_fraction: 0.0,
            random_pitch_bounds: [1.0; 2],
            cone_angles: [0.0; 2],
            outer_cone_gain: 0.0,
            gain_modifier: 0.0,
            max_bend_per_second: 0.0,
            zero_modifiers: [0.0; 3],
            one_modifiers: [0.0; 3],
            encoding,
            compression: Compression::XboxAdpcm,
            promotion: h1map::TagRef::read(&[0xff; 16], 0).unwrap(),
            promotion_count: 0,
            pitch_ranges: vec![PitchRange {
                name: String::new(),
                natural_pitch: 1.0,
                bend_bounds: [0.0; 2],
                actual_permutation_count: 1,
                permutations,
            }],
        }
    }

    #[test]
    fn impulse_length_counts_adpcm_blocks_along_the_chain() {
        // 22,050 Hz mono: 1,378 blocks of 64 samples is 88,192 samples,
        // 4.0 s, 120 ticks (rounded up from 119.99...).
        let one = sound(&[(1378 * 36, h1sounds::h1::NO_NEXT)], 0, 0);
        assert_eq!(impulse_ticks(&one), Some(120));
        // A chain plays its continuations: two halves, the same length.
        let chain = sound(&[(689 * 36, 1), (689 * 36, h1sounds::h1::NO_NEXT)], 0, 0);
        assert_eq!(impulse_ticks(&chain), Some(120));
        // 44,100 Hz stereo: a block pair per 64 samples, half as long.
        let stereo = sound(&[(1378 * 72, h1sounds::h1::NO_NEXT)], 1, 1);
        assert_eq!(impulse_ticks(&stereo), Some(60));
    }
}
