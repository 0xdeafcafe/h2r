//! Halo 1's cutscenes in a converted level (docs/halo1/campaign.md §9,
//! Cutscenes): what the scripts' cinematic calls need besides the
//! translation `h1scripts` does.
//!
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
        "at the tag memory bound: converted without level data and graphs, so its cutscenes would have no camera points, flags or animations",
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
/// The named fields of one recorded animation. The two games have different
/// data references and the reserved bytes are not part of the recording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recording {
    pub name: String,
    pub version: u8,
    pub control_version: u8,
    pub ticks: i16,
    pub stream: Vec<u8>,
}

/// The level's recorded animations, in Halo 1's order.
pub fn read_recordings(h1: &H1Map) -> Result<Vec<Recording>> {
    let scnr = h1.tag_data(h1.scenario())?;
    let rows = h1
        .tag_space()
        .block_at(scnr, H1_RECORDINGS, H1_RECORDING_LEN)?;
    rows.into_iter()
        .map(|e| {
            let name_end = e[..32].iter().position(|&c| c == 0).unwrap_or(32);
            let name = std::str::from_utf8(&e[..name_end])
                .map_err(|_| invalid("invalid recording name"))?
                .to_owned();
            let size = u32::from_le_bytes(e[0x2c..0x30].try_into().unwrap());
            let at = u32::from_le_bytes(e[0x38..0x3c].try_into().unwrap());
            Ok(Recording {
                name,
                version: e[0x20],
                control_version: e[0x22],
                ticks: i16::from_le_bytes([e[0x24], e[0x25]]),
                stream: if size == 0 {
                    Vec::new()
                } else {
                    h1.read(at, size as usize)?.to_vec()
                },
            })
        })
        .collect()
}

/// Build fresh Halo 2 rows. The map builder relocates each converted stream.
pub fn recordings_block(recordings: &[Recording]) -> Result<Vec<map::build::Block>> {
    use tags::defs::{self, Defs, Val};
    let d = &Defs::builtin().structs["recorded_animation"];
    recordings
        .iter()
        .map(|r| {
            let mut name = [0; 32];
            let dest = name
                .get_mut(..r.name.len())
                .ok_or_else(|| invalid("recording name exceeds 32 bytes"))?;
            dest.copy_from_slice(r.name.as_bytes());
            let mut row = defs::default_record(d);
            for (field, value) in [
                (
                    "name",
                    Val::List(name.into_iter().map(|b| Val::Int(i64::from(b))).collect()),
                ),
                ("version", Val::Int(r.version.into())),
                ("control_version", Val::Int(r.control_version.into())),
                ("ticks", Val::Int(r.ticks.into())),
                ("stream", Val::Data(r.stream.clone())),
            ] {
                row.set(field, value).map_err(|e| invalid(e.to_string()))?;
            }
            defs::to_built(d, &row, &mut |_| None).map_err(|e| invalid(e.to_string()))
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
    use map::build::FixupKind;

    #[test]
    fn fresh_recording_rows_keep_source_order_and_relocate_the_stream() {
        let first = Recording {
            name: "Zulu".into(),
            version: 4,
            control_version: 3,
            ticks: 257,
            stream: vec![3, 2, 1],
        };
        let second = Recording {
            name: "Alpha".into(),
            ..first.clone()
        };
        let recordings = [first, second];
        assert_eq!(recording_index(&recordings, "alpha"), Some(1));
        assert_eq!(recording_index(&recordings, "absent"), None);
        assert!(recordings_block(&[]).unwrap().is_empty());
        let rows = recordings_block(&recordings).unwrap();
        for (row, recording) in rows.iter().zip(recordings) {
            assert_eq!(row.bytes.len(), H2_RECORDING_LEN);
            let name_len = recording.name.len();
            assert_eq!(&row.bytes[..name_len], recording.name.as_bytes());
            assert!(row.bytes[name_len..32].iter().all(|&b| b == 0));
            assert_eq!(row.bytes[0x20..0x2c], [4, 0, 3, 0, 1, 1, 0, 0, 0, 0, 0, 0]);
            assert_eq!(row.fixups.len(), 1);
            let fixup = &row.fixups[0];
            assert_eq!(fixup.offset, 0x2c);
            let FixupKind::Data(child) = &fixup.kind else {
                panic!("expected a stream");
            };
            assert_eq!(child.align, 4);
            assert_eq!(child.bytes, recording.stream);
        }
    }
}
