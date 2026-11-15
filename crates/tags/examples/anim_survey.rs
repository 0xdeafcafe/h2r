//! Private-disc validation of every animation graph; emits aggregate counts only.
//! cargo run --release -p tags --example anim_survey -- /path/to/halo2.iso [-v]
//!
//! For every `jmad` tag of every map: decode the graph, load each animation's
//! data from its raw resource, build the clip, and sample every frame.
use map::{FourCC, Map, RawPointer, ResourceFile};
use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use tags::anim::{AnimationGraph, AnimationType};
use tags::anim_sample::{AnimationClip, Codec};

struct Files {
    iso: String,
    /// (offset, size) of local, mainmenu, shared, single_player_shared in the ISO.
    spans: [Option<(u64, u64)>; 4],
}
fn slot(f: ResourceFile) -> usize {
    match f {
        ResourceFile::Local => 0,
        ResourceFile::MainMenu => 1,
        ResourceFile::Shared => 2,
        ResourceFile::SinglePlayerShared => 3,
    }
}
impl Files {
    fn read(&self, p: RawPointer, size: u32) -> Result<Vec<u8>, String> {
        let (base, len) = self.spans[slot(p.file)].ok_or("resource file missing")?;
        if u64::from(p.offset) + u64::from(size) > len {
            return Err(format!("resource beyond file ({:?})", p.file));
        }
        let mut f = File::open(&self.iso).map_err(|e| e.to_string())?;
        f.seek(SeekFrom::Start(base + u64::from(p.offset)))
            .map_err(|e| e.to_string())?;
        let mut v = vec![0; size as usize];
        f.read_exact(&mut v).map_err(|e| e.to_string())?;
        Ok(v)
    }
}

#[derive(Default)]
struct Stats {
    graphs: usize,
    animations: usize,
    clips: usize,
    frames_sampled: u64,
    failures: Vec<String>,
    codec_pairs: BTreeMap<String, usize>,
    codec_streams: BTreeMap<String, usize>,
    types: BTreeMap<u8, usize>,
    count_mismatch: usize,
    static_ignored: usize,
    inline: usize,
    no_data: usize,
    nonfinite: usize,
    action_oob: usize,
    actions: usize,
    modes: usize,
    resource_ptr_none: usize,
    max_nodes: u8,
    max_frames: i16,
    frame_info: BTreeMap<u8, usize>,
    /// per animated codec: (clips, clips with a rotation step over 90 degrees, clips with any step over 150)
    smooth: BTreeMap<String, [usize; 3]>,
    keys: BTreeMap<String, [usize; 4]>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("pass your own disc image path")?;
    let verbose = std::env::args().any(|a| a == "-v");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&path)?))?;
    let entries = iso.entries()?;
    let part = iso.partition_offset();
    let span = |name: &str| {
        entries
            .iter()
            .find(|e| e.path.trim_start_matches('/').eq_ignore_ascii_case(name))
            .map(|e| (part + u64::from(e.sector) * xiso::SECTOR, u64::from(e.size)))
    };
    let mut total = Stats::default();
    for entry in entries
        .iter()
        .filter(|e| !e.is_dir && e.path.ends_with(".map"))
    {
        let base = part + u64::from(entry.sector) * xiso::SECTOR;
        let mut map =
            Map::from_region(BufReader::new(File::open(&path)?), base, entry.size.into())?;
        let files = Files {
            iso: path.clone(),
            spans: [
                Some((base, entry.size.into())),
                span("maps/mainmenu.map"),
                span("maps/shared.map"),
                span("maps/single_player_shared.map"),
            ],
        };
        let ids: Vec<_> = map
            .tags()
            .iter()
            .filter(|t| t.group == FourCC(*b"jmad"))
            .map(|t| (t.id, t.name.clone().unwrap_or_default()))
            .collect();
        let mut st = Stats::default();
        for (id, name) in ids {
            st.graphs += 1;
            let graph = match AnimationGraph::read(&mut map, id) {
                Ok(g) => g,
                Err(e) => {
                    st.failures.push(format!("{name}: graph: {e}"));
                    continue;
                }
            };
            st.modes += graph.modes.len();
            for m in &graph.modes {
                for c in &m.classes {
                    for t in &c.types {
                        for a in &t.actions {
                            st.actions += 1;
                            if a.graph_index == -1
                                && (a.animation_index < 0
                                    || a.animation_index as usize >= graph.animations.len())
                                && a.animation_index != -1
                            {
                                st.action_oob += 1;
                            }
                            if a.graph_index >= 0
                                && a.graph_index as usize >= graph.inheritance.len()
                            {
                                st.action_oob += 1;
                            }
                        }
                    }
                }
            }
            let mut cache: HashMap<usize, Vec<u8>> = HashMap::new();
            for (ai, a) in graph.animations.iter().enumerate() {
                st.animations += 1;
                st.max_nodes = st.max_nodes.max(a.node_count);
                st.max_frames = st.max_frames.max(a.frame_count);
                *st.types.entry(a.anim_type.as_u8()).or_default() += 1;
                *st.frame_info.entry(a.frame_info_type).or_default() += 1;
                if a.sizes.static_size != 0 && a.anim_type != AnimationType::Base {
                    st.static_ignored += 1;
                }
                if !a.in_resource() {
                    st.inline += 1;
                    if a.sizes.static_size == 0 && a.sizes.animated_size == 0 {
                        st.no_data += 1;
                    }
                    continue;
                }
                if a.sizes.static_size == 0 && a.sizes.animated_size == 0 {
                    st.no_data += 1;
                    continue;
                }
                let res = match graph.resources.get(a.resource_index as usize) {
                    Some(r) => *r,
                    None => {
                        st.failures
                            .push(format!("{name}#{ai}: resource index {}", a.resource_index));
                        continue;
                    }
                };
                let Some(ptr) = res.pointer else {
                    st.resource_ptr_none += 1;
                    st.failures
                        .push(format!("{name}#{ai}: null resource pointer"));
                    continue;
                };
                if let std::collections::hash_map::Entry::Vacant(slot) =
                    cache.entry(a.resource_index as usize)
                {
                    match files.read(ptr, res.size) {
                        Ok(v) => {
                            slot.insert(v);
                        }
                        Err(e) => {
                            st.failures.push(format!("{name}#{ai}: read {e}"));
                            continue;
                        }
                    }
                }
                let data = &cache[&(a.resource_index as usize)];
                let Some(blob) = data.get(a.data_offset as usize..) else {
                    st.failures
                        .push(format!("{name}#{ai}: data offset beyond resource"));
                    continue;
                };
                let clip = match AnimationClip::new(a, blob.to_vec()) {
                    Ok(c) => c,
                    Err(e) => {
                        st.failures.push(format!("{name}#{ai}: clip: {e}"));
                        continue;
                    }
                };
                st.clips += 1;
                let [cs, ca] = clip.codecs();
                let label = |c: Option<Codec>| {
                    c.map_or("none".to_owned(), |c| format!("{}:{}", c as u8, c.name()))
                };
                *st.codec_pairs
                    .entry(format!("static={} animated={}", label(cs), label(ca)))
                    .or_default() += 1;
                for (i, c) in [cs, ca].into_iter().enumerate() {
                    if let Some(c) = c {
                        *st.codec_streams
                            .entry(format!("{}:{}", c as u8, c.name()))
                            .or_default() += 1;
                        let (counted, declared) = (
                            clip.channel_counts(i).unwrap(),
                            clip.declared_counts(i).unwrap(),
                        );
                        if counted.map(|c| c as u8) != declared {
                            st.count_mismatch += 1;
                            if verbose {
                                eprintln!(
                                    "count mismatch {name}#{ai} stream {i}: flags {counted:?} header {declared:?}"
                                );
                            }
                        }
                    }
                }
                let mut prev: Option<tags::anim_sample::Pose> = None;
                let mut max_step = 0f32;
                for f in 0..clip.frame_count() {
                    match clip.sample_frame(f) {
                        Ok(p) => {
                            if let Some(q) = &prev {
                                for (x, y) in q.nodes.iter().zip(&p.nodes) {
                                    let d: f32 = x
                                        .rotation
                                        .iter()
                                        .zip(&y.rotation)
                                        .map(|(a, b)| a * b)
                                        .sum();
                                    max_step = max_step.max(2.0 * d.abs().min(1.0).acos());
                                }
                            }
                            prev = Some(p.clone());
                            st.frames_sampled += 1;
                            if p.nodes.iter().any(|n| {
                                n.rotation
                                    .iter()
                                    .chain(&n.translation)
                                    .any(|v| !v.is_finite())
                                    || !n.scale.is_finite()
                            }) {
                                st.nonfinite += 1;
                            }
                        }
                        Err(e) => {
                            st.failures.push(format!("{name}#{ai}: frame {f}: {e}"));
                            break;
                        }
                    }
                }
                match clip.key_audit() {
                    Ok(a) => {
                        let k = st.keys.entry(label(ca)).or_default();
                        for i in 0..4 {
                            k[i] += a[i];
                        }
                    }
                    Err(e) => st.failures.push(format!("{name}#{ai}: key audit: {e}")),
                }
                let e = st.smooth.entry(label(ca)).or_default();
                e[0] += 1;
                if max_step > std::f32::consts::FRAC_PI_2 {
                    e[1] += 1;
                }
                if max_step > 150f32.to_radians() {
                    e[2] += 1;
                }
            }
        }
        println!(
            "{:<28} graphs {:>4} anims {:>6} clips {:>6} failures {}",
            entry.path.rsplit('/').next().unwrap_or(&entry.path),
            st.graphs,
            st.animations,
            st.clips,
            st.failures.len()
        );
        for f in st.failures.iter().take(5) {
            println!("    {f}");
        }
        macro_rules! add {
            ($($f:ident),*) => {$(total.$f += st.$f;)*};
        }
        add!(
            graphs,
            animations,
            clips,
            frames_sampled,
            count_mismatch,
            static_ignored,
            inline,
            no_data,
            nonfinite,
            action_oob,
            actions,
            modes,
            resource_ptr_none
        );
        total.max_nodes = total.max_nodes.max(st.max_nodes);
        total.max_frames = total.max_frames.max(st.max_frames);
        for (k, v) in st.codec_pairs {
            *total.codec_pairs.entry(k).or_default() += v;
        }
        for (k, v) in st.codec_streams {
            *total.codec_streams.entry(k).or_default() += v;
        }
        for (k, v) in st.types {
            *total.types.entry(k).or_default() += v;
        }
        for (k, v) in st.frame_info {
            *total.frame_info.entry(k).or_default() += v;
        }
        for (k, v) in st.smooth {
            let t = total.smooth.entry(k).or_default();
            for i in 0..3 {
                t[i] += v[i];
            }
        }
        for (k, v) in st.keys {
            let t = total.keys.entry(k).or_default();
            for i in 0..4 {
                t[i] += v[i];
            }
        }
        total.failures.extend(st.failures);
    }
    println!(
        "\nTOTAL graphs {} animations {} clips {} frames sampled {}",
        total.graphs, total.animations, total.clips, total.frames_sampled
    );
    println!("failures {}", total.failures.len());
    println!(
        "modes {} actions {} (out of range {})",
        total.modes, total.actions, total.action_oob
    );
    println!(
        "inline {} no_data {} static-ignored(type!=0) {} count-mismatch {} nonfinite {}",
        total.inline, total.no_data, total.static_ignored, total.count_mismatch, total.nonfinite
    );
    println!(
        "max nodes {} max frames {}",
        total.max_nodes, total.max_frames
    );
    println!("types {:?}", total.types);
    println!("frame_info_type {:?}", total.frame_info);
    println!("streams by codec:");
    for (k, v) in &total.codec_streams {
        println!("  {k:<48} {v}");
    }
    println!(
        "rotation smoothness per animated codec [clips, with a >90deg frame step, with a >150deg step]:"
    );
    for (k, v) in &total.smooth {
        println!("  {k:<48} {v:?}");
    }
    println!(
        "key audit per animated codec [channels, ascending, first at 0, last at final frame]:"
    );
    for (k, v) in &total.keys {
        println!("  {k:<48} {v:?}");
    }
    println!("clips by codec pair:");
    for (k, v) in &total.codec_pairs {
        println!("  {k:<100} {v}");
    }
    if !total.failures.is_empty() {
        std::process::exit(1);
    }
    Ok(())
}
