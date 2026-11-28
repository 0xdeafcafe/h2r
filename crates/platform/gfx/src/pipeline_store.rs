//! The persistent pipeline cache: the render pipelines the game needed,
//! recorded so the next run can build the likely ones before they are
//! needed (docs/graphics/graphics.md, "Pipeline compiles (`compile.rs`) and
//! the pipeline cache", "Persisted keys, prewarmed").
//!
//! wgpu's pipeline cache does nothing on Metal, so what persists is the
//! *keys*, not compiled code: the NV2A program a pipeline runs (vertex
//! microcode or the synthesized pre-transformed program, the combiner
//! program and the code-changing options) and its translated fixed-function
//! state ([`crate::pipeline::PipelineKey`]). On startup, and when a map
//! loads, the guest-program translator translates a bounded
//! set of the recorded programs ([`select`]) on a thread of its own and
//! queues their pipelines on gfx's compile workers ([`crate::compile`]),
//! behind any a draw is waiting for.
//!
//! The file is JSON lines, appended as new pipelines show up, so several
//! runs (or processes) can share it and a run killed half way loses
//! nothing it wrote. Every line is `{"v":V,"r":R}`: `V` is the format
//! version the caller passes to [`PipelineStore::open`], `R` one of
//! - `{"Program":{"id":H,"key":...}}`, where `H` is [`program_id`], the
//!   FNV-1a hash of the key's JSON;
//! - `{"Seen":{"program":H,"map":M}}`: program `H` drew on map `M` (`null`
//!   before the game loaded any), once per program and map;
//! - `{"Pipeline":{"program":H,"count":C,"key":...}}`, naming its program
//!   by hash (`null` for gfx's built-in shader).
//!
//! On open, lines of an older version, lines that do not parse (a torn
//! write, a format before versions), duplicates, programs never seen on a
//! map, and sightings and pipelines of programs never recorded are dead.
//! Lines of a newer version are kept but not loaded. Past
//! [`COMPACT_DEAD_LINES`], or a quarter of the file, the file is rewritten
//! with only the live lines (a temporary file renamed over it): a process
//! appending meanwhile may lose that append, but nobody sees a partial
//! file. Everything in it derives from the game's shaders, so it lives in
//! the user's cache directory ([`default_path`]) and is never committed.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::pipeline::PipelineKey;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// File name under the cache directory. The record format's version is in
/// every line (see the module docs), so the name stays when it changes and
/// the file is compacted in place.
pub const FILE_NAME: &str = "pipelines-v2.jsonl";

/// The map whose programs are prewarmed at startup, with those that drew
/// before any map loaded: Halo 2's main menu, which every boot shows first.
pub const MENU_MAP: &str = "mainmenu";

/// Most programs prewarmed for one scope (startup, or one map load), most
/// recently recorded first. `H2_PREWARM_PROGRAMS` overrides it
/// ([`prewarm_cap`]).
pub const PREWARM_PROGRAMS: usize = 1024;

/// Dead lines that make [`PipelineStore::open`] compact the file whatever
/// its size (fewer do when they are a quarter of it).
pub const COMPACT_DEAD_LINES: usize = 256;

/// Where the cache lives: `H2_PIPELINE_CACHE` (a file path, or `off`),
/// else [`FILE_NAME`] in the cache directory ([`host_dirs::cache`]).
/// `None` turns it off.
pub fn default_path() -> Option<PathBuf> {
    path_from(std::env::var_os("H2_PIPELINE_CACHE"), host_dirs::cache())
}

/// Most programs prewarmed per scope: `H2_PREWARM_PROGRAMS` (a count; `0`
/// or `off` prewarms no program), else [`PREWARM_PROGRAMS`].
pub fn prewarm_cap() -> usize {
    cap_from(std::env::var_os("H2_PREWARM_PROGRAMS"))
}

fn cap_from(var: Option<OsString>) -> usize {
    let Some(v) = var else {
        return PREWARM_PROGRAMS;
    };
    let v = v.to_string_lossy();
    match v.trim() {
        "off" => 0,
        n => n.parse().unwrap_or(PREWARM_PROGRAMS),
    }
}

fn path_from(var: Option<std::ffi::OsString>, cache_dir: Option<PathBuf>) -> Option<PathBuf> {
    match var {
        Some(v) if v == "off" || v == "0" || v.is_empty() => None,
        Some(v) => Some(PathBuf::from(v)),
        None => Some(cache_dir?.join(FILE_NAME)),
    }
}

/// The loaded map, as the game publishes it ([`set_map`]).
static MAP: Mutex<Option<String>> = Mutex::new(None);
/// Bumped (under [`MAP`]'s lock) every time the map changes.
static MAP_GENERATION: AtomicU64 = AtomicU64::new(0);

/// The game loaded map `name` (the main menu is one too). Programs that
/// draw from now on are recorded as seen on it, and the pipeline cache
/// prewarms the programs recorded on it. Called by the game's
/// `loaded_map` feature at each game start; the same map again is no
/// change.
pub fn set_map(name: &str) {
    let name = name.trim().to_ascii_lowercase();
    if name.is_empty() {
        return;
    }
    let mut map = MAP.lock().unwrap_or_else(|e| e.into_inner());
    if map.as_deref() != Some(name.as_str()) {
        *map = Some(name);
        MAP_GENERATION.fetch_add(1, Ordering::Release);
    }
}

/// How many times the map has changed: one atomic load, to poll.
pub fn map_generation() -> u64 {
    MAP_GENERATION.load(Ordering::Acquire)
}

/// [`map_generation`] and the map it names (`None`: none loaded yet).
pub fn current_map() -> (u64, Option<String>) {
    let map = MAP.lock().unwrap_or_else(|e| e.into_inner());
    (MAP_GENERATION.load(Ordering::Acquire), map.clone())
}

/// A line's version, read without building its record.
#[derive(Deserialize)]
struct Head {
    v: u32,
}

/// A record of the format before versions, recognized only to be dropped.
#[derive(Deserialize)]
#[allow(dead_code)] // Parsed to recognize the line, never read.
enum Unversioned {
    Program(serde::de::IgnoredAny),
    Pipeline(serde::de::IgnoredAny),
}

/// One line of the file.
#[derive(Serialize, Deserialize)]
struct Line<R> {
    v: u32,
    r: R,
}

/// What a line records.
#[derive(Serialize, Deserialize)]
enum Record<P> {
    Program {
        id: u64,
        key: P,
    },
    Seen {
        program: u64,
        map: Option<String>,
    },
    Pipeline {
        program: Option<u64>,
        count: bool,
        key: PipelineKey,
    },
}

/// A pipeline as recorded: its program (by [`program_id`]; `None` for
/// gfx's built-in shader), whether it is the draw's count pipeline, and
/// its key (whose shader id is the recording run's, to be replaced).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct StoredPipeline {
    /// The program's id, `None` for the built-in shader.
    pub program: Option<u64>,
    /// The count-pass pipeline rather than the draw's own.
    pub count: bool,
    /// The pipeline key.
    pub key: PipelineKey,
}

/// A program drew on a map (`None`: before the game loaded one).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Seen {
    /// The program's id.
    pub program: u64,
    /// The map.
    pub map: Option<String>,
}

/// What a cache file held, of the caller's version.
pub struct Loaded<P> {
    /// Programs by id, in file order (only ones seen on some map).
    pub programs: Vec<(u64, P)>,
    /// Pipelines, in file order (only ones whose program is known).
    pub pipelines: Vec<StoredPipeline>,
    /// Where each program drew, by its last record: oldest first.
    pub seen: Vec<Seen>,
}

/// The programs to prewarm for a scope: those seen on any of `maps`
/// (`None` is before any map) that `wanted` accepts, most recently
/// recorded first, at most `cap`.
pub fn select(
    seen: &[Seen],
    maps: &[Option<&str>],
    cap: usize,
    mut wanted: impl FnMut(u64) -> bool,
) -> Vec<u64> {
    let mut out = Vec::new();
    let mut taken = HashSet::new();
    for s in seen.iter().rev() {
        if out.len() >= cap {
            break;
        }
        if maps.contains(&s.map.as_deref())
            && !taken.contains(&s.program)
            && wanted(s.program)
        {
            taken.insert(s.program);
            out.push(s.program);
        }
    }
    out
}

/// The id a program is recorded under: FNV-1a of its JSON.
pub fn program_id<P: Serialize>(key: &P) -> u64 {
    let json = serde_json::to_string(key).expect("program keys serialize");
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in json.bytes() {
        h = (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// A file read: what is live, and which lines a rewrite keeps.
struct Parsed<P> {
    loaded: Loaded<P>,
    /// Indices of the lines to keep, in file order.
    keep: Vec<usize>,
    /// Lines in the file.
    lines: usize,
    /// Lines that are pipeline-cache records of any version.
    recognized: usize,
}

impl<P> Parsed<P> {
    fn dead(&self) -> usize {
        self.lines - self.keep.len()
    }

    /// Mostly dead, and a pipeline cache: a file with no record in it is
    /// not one (`H2_PIPELINE_CACHE` naming the wrong file) and is left
    /// alone.
    fn should_compact(&self) -> bool {
        let dead = self.dead();
        self.recognized > 0 && dead > 0 && (dead >= COMPACT_DEAD_LINES || dead * 4 >= self.lines)
    }
}

fn parse<P: DeserializeOwned>(lines: &[&str], version: u32) -> Parsed<P> {
    let mut keep = Vec::new();
    let mut programs: Vec<(usize, u64, P)> = Vec::new();
    let mut program_ids = HashSet::new();
    let mut seen_last: HashMap<Seen, usize> = HashMap::new();
    let mut pipelines: Vec<(usize, StoredPipeline)> = Vec::new();
    let mut pipeline_set = HashSet::new();
    let mut recognized = 0;
    for (n, line) in lines.iter().enumerate() {
        let Ok(Head { v }) = serde_json::from_str::<Head>(line) else {
            if serde_json::from_str::<Unversioned>(line).is_ok() {
                recognized += 1;
            }
            continue;
        };
        recognized += 1;
        if v > version {
            // A newer build's: not ours to read or to drop.
            keep.push(n);
            continue;
        }
        if v < version {
            continue;
        }
        match serde_json::from_str::<Line<Record<P>>>(line).map(|l| l.r) {
            Ok(Record::Program { id, key }) => {
                if program_ids.insert(id) {
                    programs.push((n, id, key));
                }
            }
            Ok(Record::Seen { program, map }) => {
                seen_last.insert(Seen { program, map }, n);
            }
            Ok(Record::Pipeline {
                program,
                count,
                key,
            }) => {
                let p = StoredPipeline {
                    program,
                    count,
                    key,
                };
                if pipeline_set.insert(p.clone()) {
                    pipelines.push((n, p));
                }
            }
            Err(_) => {}
        }
    }
    // A program is live once it has drawn on some map. A sighting or a
    // pipeline whose program record never made it (a torn write) is
    // dropped; it is recorded again when it is next built.
    let live: HashSet<u64> = seen_last
        .keys()
        .map(|s| s.program)
        .filter(|id| program_ids.contains(id))
        .collect();
    programs.retain(|(_, id, _)| live.contains(id));
    pipelines.retain(|(_, p)| p.program.is_none_or(|id| live.contains(&id)));
    let mut seen: Vec<(usize, Seen)> = seen_last
        .into_iter()
        .filter(|(s, _)| live.contains(&s.program))
        .map(|(s, n)| (n, s))
        .collect();
    seen.sort_unstable_by_key(|(n, _)| *n);
    keep.extend(programs.iter().map(|(n, ..)| *n));
    keep.extend(seen.iter().map(|(n, _)| *n));
    keep.extend(pipelines.iter().map(|(n, _)| *n));
    keep.sort_unstable();
    Parsed {
        loaded: Loaded {
            programs: programs.into_iter().map(|(_, id, k)| (id, k)).collect(),
            pipelines: pipelines.into_iter().map(|(_, p)| p).collect(),
            seen: seen.into_iter().map(|(_, s)| s).collect(),
        },
        keep,
        lines: lines.len(),
        recognized,
    }
}

/// Replaces `path` with `lines[keep]`: written to a temporary file beside
/// it, then renamed over it, so a reader sees the old file or the new one.
fn rewrite(path: &Path, lines: &[&str], keep: &[usize]) -> std::io::Result<()> {
    static SERIAL: AtomicU64 = AtomicU64::new(0);
    let mut text = String::with_capacity(keep.iter().map(|&n| lines[n].len() + 1).sum());
    for &n in keep {
        text += lines[n];
        text.push('\n');
    }
    let name = path
        .file_name()
        .map_or_else(|| FILE_NAME.into(), |n| n.to_string_lossy().into_owned());
    let tmp = path.with_file_name(format!(
        ".{name}.{}-{}.tmp",
        std::process::id(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    ));
    let result = std::fs::File::create(&tmp)
        .and_then(|mut f| {
            f.write_all(text.as_bytes())?;
            f.sync_all()
        })
        .and_then(|_| std::fs::rename(&tmp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// An open cache file: what it held and the records this run adds.
pub struct PipelineStore {
    path: PathBuf,
    file: Option<std::fs::File>,
    version: u32,
    /// The file ends inside a line (a torn write): the next write starts
    /// a line of its own.
    torn_tail: bool,
    programs: HashSet<u64>,
    seen: HashSet<Seen>,
    pipelines: HashSet<StoredPipeline>,
}

impl PipelineStore {
    /// Reads `path` (missing is empty), keeping records of `version` (see
    /// the module docs), compacts it if it is mostly dead, and opens it for
    /// appending records of `version`.
    pub fn open<P: DeserializeOwned>(path: &Path, version: u32) -> (Self, Loaded<P>) {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        let parsed = parse::<P>(&lines, version);
        let mut torn_tail = !text.is_empty() && !text.ends_with('\n');
        if parsed.should_compact() {
            match rewrite(path, &lines, &parsed.keep) {
                Ok(()) => {
                    eprintln!(
                        "gfx: pipeline cache {}: compacted {} lines to {}",
                        path.display(),
                        parsed.lines,
                        parsed.keep.len()
                    );
                    torn_tail = false;
                }
                Err(e) => eprintln!(
                    "gfx: pipeline cache {}: compacting: {e}; left as it is",
                    path.display()
                ),
            }
        }
        drop(lines);
        drop(text);
        let loaded = parsed.loaded;
        let file = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|_| {
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
            })
            .map_err(|e| eprintln!("gfx: pipeline cache {}: {e}; not recording", path.display()))
            .ok();
        let store = Self {
            path: path.to_path_buf(),
            file,
            version,
            torn_tail,
            programs: loaded.programs.iter().map(|(id, _)| *id).collect(),
            seen: loaded.seen.iter().cloned().collect(),
            pipelines: loaded.pipelines.iter().cloned().collect(),
        };
        (store, loaded)
    }

    /// The file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn line<R: Serialize>(&self, out: &mut String, r: R) {
        *out += &serde_json::to_string(&Line { v: self.version, r }).expect("serializes");
        out.push('\n');
    }

    /// One write per call: appends from other processes do not split it.
    fn write(&mut self, mut lines: String) {
        if lines.is_empty() {
            return;
        }
        if self.torn_tail {
            lines.insert(0, '\n');
        }
        if let Some(f) = self.file.as_mut() {
            match f.write_all(lines.as_bytes()) {
                Ok(()) => self.torn_tail = false,
                Err(e) => {
                    eprintln!(
                        "gfx: pipeline cache {}: {e}; not recording",
                        self.path.display()
                    );
                    self.file = None;
                }
            }
        }
    }

    /// Records that program `id` (`key`) drew on `map` (`None`: before any
    /// map loaded): its key the first time, and the sighting once per map.
    pub fn record_program<P: Serialize>(&mut self, id: u64, key: &P, map: Option<&str>) {
        let seen = Seen {
            program: id,
            map: map.map(str::to_owned),
        };
        if self.seen.contains(&seen) {
            return;
        }
        let mut lines = String::new();
        if !self.programs.contains(&id) {
            self.line(&mut lines, Record::Program { id, key });
            self.programs.insert(id);
        }
        self.line(
            &mut lines,
            Record::<&P>::Seen {
                program: id,
                map: seen.map.clone(),
            },
        );
        self.seen.insert(seen);
        self.write(lines);
    }

    /// Records a pipeline, unless the file already has it. `program` is
    /// the [`program_id`] of a program [`record_program`](Self::record_program)
    /// recorded, `None` for gfx's built-in shader.
    pub fn record(&mut self, program: Option<u64>, count: bool, key: &PipelineKey) {
        let p = StoredPipeline {
            program,
            count,
            key: key.clone(),
        };
        if self.pipelines.contains(&p) {
            return;
        }
        let mut lines = String::new();
        self.line(
            &mut lines,
            Record::<()>::Pipeline {
                program,
                count,
                key: p.key.clone(),
            },
        );
        self.pipelines.insert(p);
        self.write(lines);
    }

    /// Programs and pipelines known (loaded plus recorded).
    pub fn len(&self) -> (usize, usize) {
        (self.programs.len(), self.pipelines.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{ShaderId, TargetFormats};
    use crate::state::RenderState;
    use crate::translate::Capabilities;
    use crate::vertex::{PrimitiveType, VertexLayout, fvf};

    const V: u32 = 7;

    fn key(shader: u32, blend: bool) -> PipelineKey {
        PipelineKey::new(
            ShaderId(shader),
            &VertexLayout::from_fvf(fvf::XYZRHW | fvf::DIFFUSE).unwrap(),
            &[],
            PrimitiveType::TriangleStrip,
            &RenderState {
                blend_enable: blend,
                ..Default::default()
            },
            TargetFormats {
                color: wgpu::TextureFormat::Bgra8Unorm,
                depth: Some(wgpu::TextureFormat::Depth24PlusStencil8),
                samples: 1,
            },
            Capabilities::ALL,
        )
    }

    fn scratch(name: &str) -> PathBuf {
        let p = std::env::temp_dir()
            .join(format!("gfx-pipeline-store-{}-{name}", std::process::id()))
            .join(FILE_NAME);
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
        p
    }

    /// A line as a store of version `v` writes it.
    fn line(v: u32, r: Record<Vec<u32>>) -> String {
        serde_json::to_string(&Line { v, r }).unwrap()
    }

    fn program(id: u64, key: Vec<u32>) -> Record<Vec<u32>> {
        Record::Program { id, key }
    }

    fn seen(program: u64, map: Option<&str>) -> Record<Vec<u32>> {
        Record::Seen {
            program,
            map: map.map(str::to_owned),
        }
    }

    fn pipeline(program: Option<u64>, k: PipelineKey) -> Record<Vec<u32>> {
        Record::Pipeline {
            program,
            count: false,
            key: k,
        }
    }

    fn write_lines(path: &Path, lines: &[String]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, lines.join("\n") + "\n").unwrap();
    }

    #[test]
    fn records_round_trip_and_dedupe() {
        let path = scratch("round-trip");
        let prog = vec![1u32, 2, 3];
        let id = program_id(&prog);
        {
            let (mut s, loaded) = PipelineStore::open::<Vec<u32>>(&path, V);
            assert!(loaded.programs.is_empty() && loaded.pipelines.is_empty());
            s.record_program(id, &prog, None);
            s.record(Some(id), false, &key(7, false));
            s.record(Some(id), true, &key(7, false));
            s.record_program(id, &prog, None);
            s.record(Some(id), false, &key(7, false));
            s.record(None, false, &key(0, true));
            s.record_program(id, &prog, Some("mainmenu"));
            assert_eq!(s.len(), (1, 3));
        }
        let text = std::fs::read_to_string(&path).unwrap();
        // One program line, two sightings, three pipelines.
        assert_eq!(text.lines().count(), 6);
        assert!(text.lines().all(|l| l.starts_with("{\"v\":7,\"r\":{")));
        let (mut s, loaded) = PipelineStore::open::<Vec<u32>>(&path, V);
        assert_eq!(loaded.programs, vec![(id, prog.clone())]);
        let got: Vec<_> = loaded
            .pipelines
            .iter()
            .map(|p| (p.program, p.count, p.key.clone()))
            .collect();
        assert_eq!(
            got,
            vec![
                (Some(id), false, key(7, false)),
                (Some(id), true, key(7, false)),
                (None, false, key(0, true))
            ]
        );
        assert_eq!(
            loaded.seen,
            vec![
                Seen {
                    program: id,
                    map: None
                },
                Seen {
                    program: id,
                    map: Some("mainmenu".into())
                }
            ]
        );
        // Known records are not written again.
        let before = std::fs::metadata(&path).unwrap().len();
        s.record(Some(id), false, &key(7, false));
        s.record_program(id, &prog, Some("mainmenu"));
        assert_eq!(std::fs::metadata(&path).unwrap().len(), before);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn pipelines_of_unknown_programs_are_dropped() {
        let path = scratch("orphan");
        write_lines(
            &path,
            &[
                line(V, pipeline(Some(42), key(3, false))),
                // Recorded but never seen on a map: not live either.
                line(V, program(43, vec![4])),
                line(V, pipeline(Some(43), key(4, false))),
                line(V, seen(44, None)),
            ],
        );
        let (s, loaded) = PipelineStore::open::<Vec<u32>>(&path, V);
        assert!(loaded.programs.is_empty());
        assert!(loaded.pipelines.is_empty());
        assert!(loaded.seen.is_empty());
        assert_eq!(s.len(), (0, 0));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// Only the caller's version is loaded. An older version's lines and
    /// lines with no version (the format before versions) are dropped when
    /// the file is compacted; a newer version's stay for the build that
    /// reads them.
    #[test]
    fn other_versions_are_not_loaded() {
        let path = scratch("versions");
        let unversioned = r#"{"Program":{"id":9,"key":[9]}}"#.to_string();
        let newer = [line(V + 1, program(8, vec![8])), line(V + 1, seen(8, None))];
        write_lines(
            &path,
            &[
                unversioned,
                line(V - 1, program(1, vec![1])),
                line(V - 1, seen(1, None)),
                newer[0].clone(),
                newer[1].clone(),
                line(V, program(2, vec![2])),
                line(V, seen(2, None)),
            ],
        );
        let (_, loaded) = PipelineStore::open::<Vec<u32>>(&path, V);
        assert_eq!(loaded.programs, vec![(2, vec![2])]);
        assert_eq!(
            loaded.seen,
            vec![Seen {
                program: 2,
                map: None
            }]
        );
        let text = std::fs::read_to_string(&path).unwrap();
        let want = [
            newer[0].clone(),
            newer[1].clone(),
            line(V, program(2, vec![2])),
            line(V, seen(2, None)),
        ];
        assert_eq!(text, want.join("\n") + "\n");
        // The newer build reads its own and not ours.
        let (_, loaded) = PipelineStore::open::<Vec<u32>>(&path, V + 1);
        assert_eq!(loaded.programs, vec![(8, vec![8])]);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// A mostly dead file is rewritten with its live lines, in order: a
    /// stale-version line, a duplicate, a repeated sighting (the last one
    /// stays) and a torn last line go.
    #[test]
    fn open_compacts_dead_lines() {
        let path = scratch("compact");
        let live = [
            line(V, program(1, vec![1])),
            line(V, pipeline(Some(1), key(1, false))),
            line(V, seen(1, Some("a"))),
        ];
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let text = [
            line(V, program(1, vec![1])),
            line(V, seen(1, Some("a"))),
            line(V - 1, program(5, vec![5])),
            live[1].clone(),
            line(V, program(1, vec![1])),
            line(V, pipeline(Some(1), key(1, false))),
            live[2].clone(),
        ]
        .join("\n")
            + "\n{\"v\":7,\"r\":{\"Pipel";
        std::fs::write(&path, text).unwrap();
        let (mut s, loaded) = PipelineStore::open::<Vec<u32>>(&path, V);
        assert_eq!(loaded.programs, vec![(1, vec![1])]);
        assert_eq!(loaded.pipelines.len(), 1);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            live.join("\n") + "\n"
        );
        // No temporary file is left beside it.
        let entries = std::fs::read_dir(path.parent().unwrap()).unwrap().count();
        assert_eq!(entries, 1);
        // The store appends to the rewritten file.
        s.record(None, false, &key(0, true));
        let (_, loaded) = PipelineStore::open::<Vec<u32>>(&path, V);
        assert_eq!(loaded.pipelines.len(), 2);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// A few dead lines in a large file leave it as it is; an append after
    /// a torn last line starts a line of its own.
    #[test]
    fn a_little_dead_is_kept_and_appended_past() {
        let path = scratch("little-dead");
        let mut lines = vec![line(V, program(1, vec![1])), line(V, seen(1, None))];
        lines.extend((0..8).map(|n| line(V, pipeline(Some(1), key(n, false)))));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let text = lines.join("\n") + "\n{\"v\":7,\"r\":{\"Pipel";
        std::fs::write(&path, &text).unwrap();
        let (mut s, loaded) = PipelineStore::open::<Vec<u32>>(&path, V);
        assert_eq!(loaded.pipelines.len(), 8);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        s.record(Some(1), false, &key(100, false));
        let (_, loaded) = PipelineStore::open::<Vec<u32>>(&path, V);
        assert_eq!(loaded.pipelines.len(), 9);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// A file with no record in it is not a pipeline cache: it is never
    /// rewritten.
    #[test]
    fn a_file_with_no_records_is_left_alone() {
        let path = scratch("not-a-cache");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let text = "export PATH=/bin\n{\"v\":\"not ours\"}\n";
        std::fs::write(&path, text).unwrap();
        let (_, loaded) = PipelineStore::open::<Vec<u32>>(&path, V);
        assert!(loaded.programs.is_empty());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// The prewarm set is the scope's programs, newest sighting first, up
    /// to the cap, skipping the ones not wanted (already installed).
    #[test]
    fn selection_is_scoped_newest_first_and_capped() {
        let s = |program, map: Option<&str>| Seen {
            program,
            map: map.map(str::to_owned),
        };
        let seen = [
            s(1, None),
            s(2, Some("mainmenu")),
            s(3, Some("ascension")),
            s(4, Some("mainmenu")),
            s(5, Some("ascension")),
            // 1 seen again later, on the menu.
            s(1, Some("mainmenu")),
        ];
        let startup = [None, Some(MENU_MAP)];
        assert_eq!(select(&seen, &startup, 10, |_| true), vec![1, 4, 2]);
        assert_eq!(select(&seen, &startup, 2, |_| true), vec![1, 4]);
        assert_eq!(
            select(&seen, &[Some("ascension")], 10, |_| true),
            vec![5, 3]
        );
        assert_eq!(select(&seen, &startup, 2, |id| id != 4), vec![1, 2]);
        assert_eq!(
            select(&seen, &[Some("zanzibar")], 10, |_| true),
            Vec::<u64>::new()
        );
        assert_eq!(select(&seen, &startup, 0, |_| true), Vec::<u64>::new());
    }

    /// Selection order follows the file: a program seen again moves to the
    /// front, once compaction keeps only its last sighting too.
    #[test]
    fn sightings_keep_their_order_through_compaction() {
        let path = scratch("order");
        let mut lines: Vec<String> = (1..=3)
            .flat_map(|id| {
                [
                    line(V, program(id, vec![id as u32])),
                    line(V, seen(id, Some("m"))),
                ]
            })
            .collect();
        lines.push(line(V, seen(1, Some("m"))));
        // Enough dead lines to compact.
        lines.extend((0..8).map(|_| "junk".to_string()));
        write_lines(&path, &lines);
        let (_, loaded) = PipelineStore::open::<Vec<u32>>(&path, V);
        assert_eq!(
            select(&loaded.seen, &[Some("m")], 10, |_| true),
            vec![1, 3, 2]
        );
        let (_, again) = PipelineStore::open::<Vec<u32>>(&path, V);
        assert_eq!(again.seen, loaded.seen);
        assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 6);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn prewarm_cap_choice() {
        let os = |s: &str| Some(OsString::from(s));
        assert_eq!(cap_from(None), PREWARM_PROGRAMS);
        assert_eq!(cap_from(os("64")), 64);
        assert_eq!(cap_from(os("0")), 0);
        assert_eq!(cap_from(os("off")), 0);
        assert_eq!(cap_from(os("lots")), PREWARM_PROGRAMS);
    }

    /// The only test that publishes a map: the slot is process-wide.
    #[test]
    fn a_map_change_bumps_the_generation_once() {
        let before = map_generation();
        set_map(" Pipeline_Store_Test ");
        set_map("pipeline_store_test");
        let (generation, map) = current_map();
        assert_eq!(generation, before + 1);
        assert_eq!(map.as_deref(), Some("pipeline_store_test"));
        set_map("");
        assert_eq!(map_generation(), before + 1);
    }

    #[test]
    fn program_ids_are_content_hashes() {
        assert_eq!(program_id(&vec![1u32]), program_id(&vec![1u32]));
        assert_ne!(program_id(&vec![1u32]), program_id(&vec![2u32]));
    }

    #[test]
    fn path_choice() {
        let os = |s: &str| Some(std::ffi::OsString::from(s));
        let cache = Some(PathBuf::from("/c/h2recomp"));
        for off in ["off", "0", ""] {
            assert_eq!(path_from(os(off), cache.clone()), None);
        }
        assert_eq!(path_from(os("/x/p.jsonl"), cache.clone()), Some(PathBuf::from("/x/p.jsonl")));
        assert_eq!(path_from(None, cache), Some(PathBuf::from("/c/h2recomp").join(FILE_NAME)));
        assert_eq!(path_from(None, None), None, "no cache directory, no cache");
    }
}
