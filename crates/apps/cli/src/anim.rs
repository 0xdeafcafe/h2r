//! `h2 anim`: inspect Halo 2 animation graphs (`jmad`), sample their compressed
//! data, and validate every animation on the disc.
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use map::{FourCC, Map};
use tags::anim::{Animation, AnimationGraph};
use tags::anim_sample::{AnimationClip, Codec, Pose};

use crate::safe_out::OutDir;
use crate::sound::{Resources, open};

const USAGE: &str = "usage: h2 anim <map-name-or-file> <jmad tag name> [--anim NAME] [--json PATH] [--force] [--actions] [--disc disc.iso]\n       h2 anim --all [map-name-or-file] [--disc disc.iso]\n\nPrints a summary of a jmad animation graph: skeleton nodes, modes/classes/types,\nand the animation list with frame counts, node counts and codecs. --anim NAME\nshows one animation in detail. --json PATH writes the sampled transforms\n(every frame, every node: rotation as an x,y,z,w quaternion, translation, scale)\nof the --anim animation, or of all animations of the graph without --anim, to PATH;\nan existing file is only replaced with --force. --actions lists every action.\n--all decodes and samples every frame of every animation (of all disc maps\nwithout a map argument) and prints codec counts; the exit status is non-zero if\nanything fails to decode.";

pub(super) fn run(args: Vec<String>) -> Result<()> {
    let mut args = args.into_iter();
    let mut positional = Vec::new();
    let (mut anim, mut json, mut disc) = (None, None, None);
    let (mut all, mut force, mut actions) = (false, false, false);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--all" => all = true,
            "--force" => force = true,
            "--actions" => actions = true,
            "--anim" => anim = Some(args.next().context("--anim needs a name")?),
            "--json" => json = Some(PathBuf::from(args.next().context("--json needs a path")?)),
            "--disc" => {
                disc = Some(PathBuf::from(
                    args.next().context("--disc needs an ISO path")?,
                ))
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            s if s.starts_with("--") => bail!("unknown option {s}\n{USAGE}"),
            _ => positional.push(a),
        }
    }
    match (all, positional.as_slice()) {
        (true, []) => validate_disc(disc),
        (true, [map]) => {
            let t = validate_map(map, disc)?;
            t.print();
            t.exit()
        }
        (false, [map, tag]) => one(
            map,
            tag,
            disc,
            anim.as_deref(),
            json.as_deref(),
            force,
            actions,
        ),
        _ => bail!("{USAGE}"),
    }
}

fn sid(strings: &[Option<String>], id: tags::wire::StringId) -> String {
    match id.resolve(strings) {
        Ok(Some(s)) => s.to_owned(),
        _ => format!("#{:#x}", id.0),
    }
}

fn codec_label(c: Option<Codec>) -> String {
    c.map_or("-".into(), |c| format!("{}:{}", c as u8, c.name()))
}

fn json_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}
fn json_f32(out: &mut String, v: f32) {
    if v.is_finite() {
        let _ = write!(out, "{v}");
    } else {
        out.push_str("null");
    }
}
fn json_floats(out: &mut String, v: &[f32]) {
    out.push('[');
    for (i, f) in v.iter().enumerate() {
        if i != 0 {
            out.push(',');
        }
        json_f32(out, *f);
    }
    out.push(']');
}

/// One animation's samples as a JSON object.
fn animation_json(
    w: &mut dyn std::io::Write,
    out: &mut String,
    strings: &[Option<String>],
    nodes: &[tags::anim::Node],
    index: usize,
    a: &Animation,
    clip: &AnimationClip,
) -> Result<()> {
    out.push_str("{\"index\":");
    let _ = write!(out, "{index}");
    out.push_str(",\"name\":");
    json_str(out, &sid(strings, a.name));
    let _ = write!(
        out,
        ",\"type\":{},\"frame_count\":{},\"node_count\":{},\"static_codec\":",
        a.anim_type.as_u8(),
        clip.frame_count(),
        clip.node_count()
    );
    let [s, an] = clip.codecs();
    json_str(out, &codec_label(s));
    out.push_str(",\"animated_codec\":");
    json_str(out, &codec_label(an));
    out.push_str(",\"nodes\":[");
    for (i, n) in nodes.iter().enumerate() {
        if i != 0 {
            out.push(',');
        }
        let _ = write!(out, "{{\"index\":{i},\"parent\":{},\"name\":", n.parent);
        json_str(out, &sid(strings, n.name));
        out.push('}');
    }
    out.push_str("],\"frames\":[");
    let mut pose = Pose { nodes: Vec::new() };
    for f in 0..clip.frame_count() {
        if f != 0 {
            out.push(',');
        }
        clip.sample_frame_into(f, &mut pose)?;
        out.push('[');
        for (i, n) in pose.nodes.iter().enumerate() {
            if i != 0 {
                out.push(',');
            }
            out.push_str("{\"rotation\":");
            json_floats(out, &n.rotation);
            out.push_str(",\"translation\":");
            json_floats(out, &n.translation);
            out.push_str(",\"scale\":");
            json_f32(out, n.scale);
            let _ = write!(
                out,
                ",\"has\":{{\"rotation\":{},\"translation\":{},\"scale\":{}}}}}",
                n.has_rotation, n.has_translation, n.has_scale
            );
        }
        out.push(']');
        flush(w, out)?;
    }
    out.push_str("]}");
    flush(w, out)
}

/// Move the buffered text to the writer, so memory use is one frame's worth.
fn flush(w: &mut dyn std::io::Write, out: &mut String) -> Result<()> {
    w.write_all(out.as_bytes())?;
    out.clear();
    Ok(())
}

fn load(
    graph: &AnimationGraph,
    index: usize,
    res: &mut Resources,
    cache: &mut HashMap<usize, std::sync::Arc<[u8]>>,
) -> Result<AnimationClip> {
    let a = &graph.animations[index];
    let ri = a.resource_index as usize;
    if let std::collections::hash_map::Entry::Vacant(slot) = cache.entry(ri) {
        slot.insert(graph.read_resource(ri, res)?);
    }
    Ok(graph.clip_from_shared(index, &cache[&ri])?)
}

fn one(
    map_name: &str,
    tag: &str,
    disc: Option<PathBuf>,
    anim: Option<&str>,
    json: Option<&Path>,
    force: bool,
    actions: bool,
) -> Result<()> {
    let (mut map, mut res) = open(map_name, disc)?;
    let id = match map.tag(map::TagQuery::NameInGroup(tag, FourCC(*b"jmad"))) {
        Ok(t) => t.id,
        Err(e) => {
            let near: Vec<_> = map
                .tags()
                .iter()
                .filter(|t| t.group == FourCC(*b"jmad"))
                .filter_map(|t| t.name.as_deref())
                .filter(|n| n.contains(tag))
                .take(10)
                .collect();
            if near.is_empty() {
                return Err(e).with_context(|| format!("no jmad tag {tag:?}"));
            }
            bail!(
                "no jmad tag {tag:?}; similar names:\n  {}",
                near.join("\n  ")
            );
        }
    };
    let graph = AnimationGraph::read(&mut map, id)?;
    let strings = map.strings();
    let selected = match anim {
        Some(name) => Some(
            graph
                .find_animation(strings, name)
                .with_context(|| format!("no animation {name:?} in {tag}"))?,
        ),
        None => None,
    };
    let mut cache = HashMap::new();
    match selected {
        Some(i) => detail(&graph, strings, i, &mut res, &mut cache)?,
        None => summary(&graph, strings, tag, actions, &mut res, &mut cache),
    }
    if let Some(path) = json {
        let name = path
            .file_name()
            .context("--json needs a file path")?
            .to_string_lossy()
            .into_owned();
        let dir = match path.parent() {
            Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
            _ => PathBuf::from("."),
        };
        let list: Vec<usize> =
            selected.map_or_else(|| (0..graph.animations.len()).collect(), |i| vec![i]);
        // The path is checked (and the file created) before any decoding; the
        // JSON is streamed to it, never held whole in memory.
        let (file, written) = OutDir::new(dir, force)?.create(&[name])?;
        let mut w = std::io::BufWriter::new(file);
        let result = (|| -> Result<()> {
            let mut out = String::from("{\"tag\":");
            json_str(&mut out, tag);
            out.push_str(",\"animations\":[");
            for (k, i) in list.iter().enumerate() {
                if k != 0 {
                    out.push(',');
                }
                let clip = load(&graph, *i, &mut res, &mut cache).with_context(|| {
                    format!("animation {}", sid(strings, graph.animations[*i].name))
                })?;
                animation_json(
                    &mut w,
                    &mut out,
                    strings,
                    &graph.nodes,
                    *i,
                    &graph.animations[*i],
                    &clip,
                )?;
            }
            out.push_str("]}\n");
            flush(&mut w, &mut out)?;
            std::io::Write::flush(&mut w)?;
            Ok(())
        })();
        if let Err(e) = result {
            drop(w);
            let _ = std::fs::remove_file(&written);
            return Err(e);
        }
        println!(
            "wrote {} ({} animation(s), {} bytes)",
            written.display(),
            list.len(),
            std::fs::metadata(&written).map_or(0, |m| m.len())
        );
    }
    Ok(())
}

fn summary(
    graph: &AnimationGraph,
    strings: &[Option<String>],
    tag: &str,
    actions: bool,
    res: &mut Resources,
    cache: &mut HashMap<usize, std::sync::Arc<[u8]>>,
) {
    println!("{tag}");
    println!(
        "  {} nodes, {} animations, {} modes, {} sound refs, {} effect refs, {} blend screens, {} resources, {} inherited graphs",
        graph.nodes.len(),
        graph.animations.len(),
        graph.modes.len(),
        graph.sound_refs.len(),
        graph.effect_refs.len(),
        graph.blend_screens.len(),
        graph.resources.len(),
        graph.inheritance.len()
    );
    println!("nodes:");
    for (i, n) in graph.nodes.iter().enumerate() {
        println!(
            "  {i:>3} {:<32} parent {:>3} child {:>3} sibling {:>3}",
            sid(strings, n.name),
            n.parent,
            n.first_child,
            n.next_sibling
        );
    }
    println!("modes:");
    for m in &graph.modes {
        let types: usize = m.classes.iter().map(|c| c.types.len()).sum();
        let acts: usize = m
            .classes
            .iter()
            .flat_map(|c| &c.types)
            .map(|t| t.actions.len())
            .sum();
        println!(
            "  {:<24} {} classes, {types} types, {acts} actions",
            sid(strings, m.name),
            m.classes.len()
        );
        for c in &m.classes {
            println!("    {:<22} {} types", sid(strings, c.name), c.types.len());
            for t in &c.types {
                println!(
                    "      {:<20} {} actions",
                    sid(strings, t.name),
                    t.actions.len()
                );
                if actions {
                    for a in &t.actions {
                        let target = if a.graph_index == -1 {
                            graph
                                .animations
                                .get(a.animation_index as usize)
                                .map(|x| sid(strings, x.name))
                        } else {
                            Some(format!("graph {}", a.graph_index))
                        };
                        println!(
                            "        {:<28} -> {} {}",
                            sid(strings, a.name),
                            a.animation_index,
                            target.unwrap_or_default()
                        );
                    }
                }
            }
        }
    }
    println!("animations:");
    println!(
        "  idx name                                type  frames nodes  static codec / animated codec"
    );
    for (i, a) in graph.animations.iter().enumerate() {
        let (cs, ca) = match load(graph, i, res, cache) {
            Ok(c) => {
                let [s, an] = c.codecs();
                (codec_label(s), codec_label(an))
            }
            Err(e) => (format!("error: {e}"), String::new()),
        };
        println!(
            "  {i:>3} {:<36} {:>4} {:>6} {:>5}  {cs}  {ca}",
            sid(strings, a.name),
            a.anim_type.as_u8(),
            a.frame_count,
            a.node_count
        );
    }
}

fn detail(
    graph: &AnimationGraph,
    strings: &[Option<String>],
    i: usize,
    res: &mut Resources,
    cache: &mut HashMap<usize, std::sync::Arc<[u8]>>,
) -> Result<()> {
    let a = &graph.animations[i];
    println!("animation {i}: {}", sid(strings, a.name));
    println!(
        "  type {:?}, frame info type {}, {} frames, {} nodes, flags {:#06x}",
        a.anim_type, a.frame_info_type, a.frame_count, a.node_count, a.flags
    );
    println!(
        "  resource {} offset {:#x}, static pose {}, links {} / {}",
        a.resource_index, a.data_offset, a.static_pose, a.link_a, a.link_b
    );
    println!("  sizes: {:?}", a.sizes);
    println!(
        "  events: {} frame, {} sound, {} effect",
        a.frame_events.len(),
        a.sound_events.len(),
        a.effect_events.len()
    );
    let clip = load(graph, i, res, cache)?;
    let [s, an] = clip.codecs();
    println!(
        "  static stream:   {} {:?} channels [rot, trans, scale]",
        codec_label(s),
        clip.channel_counts(0)
    );
    println!(
        "  animated stream: {} {:?} channels [rot, trans, scale]",
        codec_label(an),
        clip.channel_counts(1)
    );
    let first = clip.sample_frame(0)?;
    let covered = |f: fn(&tags::anim_sample::NodeTransform) -> bool| {
        first.nodes.iter().filter(|n| f(n)).count()
    };
    println!(
        "  nodes with data: {} rotation, {} translation, {} scale (of {})",
        covered(|n| n.has_rotation),
        covered(|n| n.has_translation),
        covered(|n| n.has_scale),
        clip.node_count()
    );
    Ok(())
}

#[derive(Default)]
struct Totals {
    maps: usize,
    graphs: usize,
    animations: usize,
    frames: u64,
    failures: Vec<String>,
    streams: BTreeMap<String, usize>,
    pairs: BTreeMap<String, usize>,
}
impl Totals {
    fn merge(&mut self, o: Totals) {
        self.maps += o.maps;
        self.graphs += o.graphs;
        self.animations += o.animations;
        self.frames += o.frames;
        self.failures.extend(o.failures);
        for (k, v) in o.streams {
            *self.streams.entry(k).or_default() += v;
        }
        for (k, v) in o.pairs {
            *self.pairs.entry(k).or_default() += v;
        }
    }
    fn print(&self) {
        println!(
            "{} map(s), {} graphs, {} animations, {} frames sampled, {} failures",
            self.maps,
            self.graphs,
            self.animations,
            self.frames,
            self.failures.len()
        );
        for f in self.failures.iter().take(20) {
            println!("  FAIL {f}");
        }
        println!("streams by codec:");
        for (k, v) in &self.streams {
            println!("  {k:<46} {v}");
        }
        println!("animations by (static, animated) codec:");
        for (k, v) in &self.pairs {
            println!("  {k:<96} {v}");
        }
    }
    fn exit(&self) -> Result<()> {
        if self.failures.is_empty() {
            Ok(())
        } else {
            bail!("{} animation(s) failed to decode", self.failures.len())
        }
    }
}

fn validate_map(map_name: &str, disc: Option<PathBuf>) -> Result<Totals> {
    let (mut map, mut res) = open(map_name, disc)?;
    Ok(validate_open(&mut map, &mut res))
}

fn validate_open(map: &mut Map<std::io::BufReader<std::fs::File>>, res: &mut Resources) -> Totals {
    let mut t = Totals {
        maps: 1,
        ..Default::default()
    };
    let ids: Vec<_> = map
        .tags()
        .iter()
        .filter(|g| g.group == FourCC(*b"jmad"))
        .map(|g| (g.id, g.name.clone().unwrap_or_default()))
        .collect();
    for (id, name) in ids {
        t.graphs += 1;
        let graph = match AnimationGraph::read(map, id) {
            Ok(g) => g,
            Err(e) => {
                t.failures.push(format!("{name}: {e}"));
                continue;
            }
        };
        let mut cache = HashMap::new();
        for i in 0..graph.animations.len() {
            t.animations += 1;
            guarded(&mut t, &format!("{name} animation {i}"), |t| {
                check_animation(&graph, i, res, &mut cache, t, &name)
            });
        }
    }
    t
}

/// Run one animation's check; a panic in the decoder is recorded as that
/// animation's failure instead of aborting the run.
fn guarded(t: &mut Totals, what: &str, check: impl FnOnce(&mut Totals)) {
    let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check(t)));
    if ran.is_err() {
        t.failures.push(format!("{what}: decoder panicked"));
    }
}

/// Decode and fully sample one animation, recording the outcome in `t`.
fn check_animation(
    graph: &AnimationGraph,
    i: usize,
    res: &mut Resources,
    cache: &mut HashMap<usize, std::sync::Arc<[u8]>>,
    t: &mut Totals,
    name: &str,
) {
    let clip = match load(graph, i, res, cache) {
        Ok(c) => c,
        Err(e) => {
            t.failures.push(format!("{name} animation {i}: {e:#}"));
            return;
        }
    };
    let [s, an] = clip.codecs();
    for c in [s, an].into_iter().flatten() {
        *t.streams.entry(codec_label(Some(c))).or_default() += 1;
    }
    *t.pairs
        .entry(format!(
            "static {} / animated {}",
            codec_label(s),
            codec_label(an)
        ))
        .or_default() += 1;
    for f in 0..clip.frame_count() {
        match clip.sample_frame(f) {
            Ok(p)
                if p.nodes.iter().all(|n| {
                    n.rotation
                        .iter()
                        .chain(&n.translation)
                        .chain([&n.scale])
                        .all(|v| v.is_finite())
                }) =>
            {
                t.frames += 1
            }
            Ok(_) => {
                t.failures.push(format!(
                    "{name} animation {i}: non-finite value at frame {f}"
                ));
                return;
            }
            Err(e) => {
                t.failures
                    .push(format!("{name} animation {i}: frame {f}: {e}"));
                return;
            }
        }
    }
}

fn validate_disc(disc: Option<PathBuf>) -> Result<()> {
    let disc_path = match &disc {
        Some(d) => d.clone(),
        None => super::default_disc()?,
    };
    let mut iso = super::open_iso(&disc_path)?;
    let mut names: Vec<String> = iso
        .entries()?
        .iter()
        .filter(|e| !e.is_dir)
        .filter_map(|e| {
            e.path
                .trim_start_matches('/')
                .strip_prefix("maps/")
                .map(str::to_owned)
        })
        .filter_map(|n| n.strip_suffix(".map").map(str::to_owned))
        .collect();
    names.sort();
    let mut total = Totals::default();
    for n in &names {
        let t = validate_map(n, Some(disc_path.clone()))?;
        println!(
            "{n:<28} graphs {:>4} animations {:>6} frames {:>8} failures {}",
            t.graphs,
            t.animations,
            t.frames,
            t.failures.len()
        );
        total.merge(t);
    }
    println!();
    total.print();
    total.exit()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tags::anim::{AnimationType, DataSizes};
    use tags::wire::StringId;

    fn animation(frames: i16, static_size: u16) -> Animation {
        Animation {
            name: StringId(0),
            node_list_checksum: 0,
            checksums: [0; 2],
            anim_type: AnimationType::Base,
            frame_info_type: 0,
            unknown_12: 0,
            node_count: 1,
            frame_count: frames,
            flags: 0x10,
            unknown_18: [0; 8],
            unknown_20: 0,
            resource_index: 0,
            data_offset: 0,
            static_pose: -1,
            unknown_2e: 0,
            link_a: -1,
            link_b: -1,
            inline_size: 0,
            inline_address: 0,
            sizes: DataSizes {
                static_flags: 12,
                animated_flags: 0,
                unknown: [0; 2],
                unknown_4: 0,
                static_size,
                unknown_8: 0,
                animated_size: 0,
            },
            frame_events: vec![],
            sound_events: vec![],
            effect_events: vec![],
            extra_block_count: 0,
        }
    }

    /// One node, a static stream with a unit rotation, so every frame is the same.
    fn clip(frames: i16) -> (Animation, AnimationClip) {
        let mut s = vec![0u8; 0x20];
        s[0] = 1;
        s[1] = 1;
        s.extend([0, 0, 0, 0, 0, 0, 100, 0]); // i16x4: x y z w = 0 0 0 100
        let len = s.len();
        s.extend([1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]); // rotation flag array, two empty
        let a = animation(frames, len as u16);
        (a.clone(), AnimationClip::new(&a, s).unwrap())
    }

    #[derive(Default)]
    struct Chunks(Vec<usize>, Vec<u8>);
    impl std::io::Write for Chunks {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.push(b.len());
            self.1.extend(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn json_is_streamed_a_frame_at_a_time() {
        let frames = 200;
        let (a, c) = clip(frames);
        let mut w = Chunks::default();
        let mut out = String::new();
        animation_json(&mut w, &mut out, &[], &[], 0, &a, &c).unwrap();
        assert!(
            out.is_empty(),
            "everything buffered was handed to the writer"
        );
        let text = String::from_utf8(w.1.clone()).unwrap();
        assert!(text.starts_with("{\"index\":0") && text.ends_with("]}"));
        assert_eq!(text.matches("\"translation\":[").count(), frames as usize);
        // one write per frame plus the header and trailer, none holding the whole output
        assert!(w.0.len() >= frames as usize, "{} writes", w.0.len());
        let biggest = *w.0.iter().max().unwrap();
        assert!(
            biggest * 20 < text.len(),
            "largest write {biggest} of {}",
            text.len()
        );
    }

    #[test]
    fn a_decoder_panic_is_one_failure_not_an_abort() {
        let mut t = Totals::default();
        guarded(&mut t, "first", |_| panic!("boom"));
        guarded(&mut t, "second", |t| {
            t.failures.push("second: ordinary".into())
        });
        guarded(&mut t, "third", |t| t.frames += 1);
        assert_eq!(t.failures.len(), 2);
        assert!(t.failures[0].starts_with("first: decoder panicked"));
        assert_eq!(t.frames, 1, "the run continued after the panic");
        assert!(t.exit().is_err());
    }
}
