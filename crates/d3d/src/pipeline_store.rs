//! The persistent pipeline cache: every render pipeline the game needed,
//! recorded so the next run can build it before it is needed.
//!
//! wgpu's pipeline cache does nothing on Metal, so what persists is the
//! *keys*, not compiled code: the NV2A program a pipeline runs (vertex
//! microcode or the synthesized pre-transformed program, the combiner
//! program and the code-changing options) and its translated fixed-function
//! state ([`gfx::pipeline::PipelineKey`]). On startup
//! [`Nv2aShaders`](crate::nv2a::Nv2aShaders) translates the recorded
//! programs on a thread of its own and queues their pipelines on gfx's
//! compile workers ([`gfx::compile`]), behind any a draw is waiting for.
//!
//! The file is JSON lines, appended as new pipelines show up, so several
//! runs (or processes) can share it and a run killed half way loses
//! nothing it wrote. A program record is `{"Program":{"id":H,"key":...}}`,
//! where `H` is the FNV-1a hash of the key's JSON; a pipeline record names
//! its program by that hash (`null` for gfx's built-in shader). Lines that
//! do not parse (an older format, a torn write) are skipped, duplicates are
//! dropped. Everything in it derives from the game's shaders, so it lives in
//! the user's cache directory ([`default_path`]) and is never committed.

use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use gfx::pipeline::PipelineKey;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// File name under the cache directory. Bump the version when a record
/// changes meaning without failing to parse.
/// v2: v1 files from 1a5aa8d can pair a depth-writing (W-buffered) program
/// with a depth-less pipeline, which fails validation.
pub const FILE_NAME: &str = "pipelines-v2.jsonl";

/// Where the cache lives: `H2_PIPELINE_CACHE` (a file path, or `off`),
/// else `~/Library/Caches/h2recomp/` on macOS and `$XDG_CACHE_HOME/h2recomp/`
/// (`~/.cache/h2recomp/`) elsewhere, under [`FILE_NAME`]. `None` turns it off.
pub fn default_path() -> Option<PathBuf> {
    path_from(
        std::env::var_os("H2_PIPELINE_CACHE"),
        std::env::var_os("XDG_CACHE_HOME"),
        std::env::var_os("HOME"),
    )
}

fn path_from(
    var: Option<std::ffi::OsString>,
    xdg: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    match var {
        Some(v) if v == "off" || v == "0" || v.is_empty() => return None,
        Some(v) => return Some(PathBuf::from(v)),
        None => {}
    }
    let dir = if cfg!(target_vendor = "apple") {
        PathBuf::from(home?).join("Library/Caches")
    } else {
        xdg.map(PathBuf::from)
            .or_else(|| Some(PathBuf::from(home?).join(".cache")))?
    };
    Some(dir.join("h2recomp").join(FILE_NAME))
}

/// One line of the file.
#[derive(Serialize, Deserialize)]
enum Record<P> {
    Program {
        id: u64,
        key: P,
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

/// What a cache file held.
pub struct Loaded<P> {
    /// Programs by id, in file order.
    pub programs: Vec<(u64, P)>,
    /// Pipelines, in file order (only ones whose program is known).
    pub pipelines: Vec<StoredPipeline>,
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

/// An open cache file: what it held and the records this run adds.
pub struct PipelineStore {
    path: PathBuf,
    file: Option<std::fs::File>,
    programs: HashSet<u64>,
    pipelines: HashSet<StoredPipeline>,
}

impl PipelineStore {
    /// Reads `path` (missing is empty) and opens it for appending.
    pub fn open<P: DeserializeOwned>(path: &Path) -> (Self, Loaded<P>) {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        let mut programs = Vec::new();
        let mut ids = HashSet::new();
        let mut pipelines = Vec::new();
        let mut seen = HashSet::new();
        for line in text.lines() {
            match serde_json::from_str::<Record<P>>(line) {
                Ok(Record::Program { id, key }) => {
                    if ids.insert(id) {
                        programs.push((id, key));
                    }
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
                    if seen.insert(p.clone()) {
                        pipelines.push(p);
                    }
                }
                Err(_) => {}
            }
        }
        // A pipeline whose program record never made it (torn write) is
        // dropped; it is recorded again when it is next built.
        pipelines.retain(|p| p.program.is_none_or(|id| ids.contains(&id)));
        let file = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|_| {
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
            })
            .map_err(|e| eprintln!("d3d: pipeline cache {}: {e}; not recording", path.display()))
            .ok();
        let store = Self {
            path: path.to_path_buf(),
            file,
            programs: ids,
            pipelines: seen,
        };
        (
            store,
            Loaded {
                programs,
                pipelines,
            },
        )
    }

    /// The file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Records a pipeline (and its program, the first time), unless the
    /// file already has it.
    pub fn record<P: Serialize>(
        &mut self,
        program: Option<(u64, &P)>,
        count: bool,
        key: &PipelineKey,
    ) {
        let p = StoredPipeline {
            program: program.map(|(id, _)| id),
            count,
            key: key.clone(),
        };
        if self.pipelines.contains(&p) {
            return;
        }
        let mut lines = String::new();
        if let Some((id, key)) = program
            && !self.programs.contains(&id)
        {
            lines += &serde_json::to_string(&Record::Program { id, key }).expect("serializes");
            lines.push('\n');
            self.programs.insert(id);
        }
        lines += &serde_json::to_string(&Record::<P>::Pipeline {
            program: p.program,
            count,
            key: p.key.clone(),
        })
        .expect("serializes");
        lines.push('\n');
        self.pipelines.insert(p);
        // One write per call: appends from other processes do not split it.
        if let Some(f) = self.file.as_mut()
            && let Err(e) = f.write_all(lines.as_bytes())
        {
            eprintln!(
                "d3d: pipeline cache {}: {e}; not recording",
                self.path.display()
            );
            self.file = None;
        }
    }

    /// Programs and pipelines known (loaded plus recorded).
    pub fn len(&self) -> (usize, usize) {
        (self.programs.len(), self.pipelines.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gfx::pipeline::{ShaderId, TargetFormats};
    use gfx::state::RenderState;
    use gfx::translate::Capabilities;
    use gfx::vertex::{PrimitiveType, VertexLayout, fvf};

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
            .join(format!("d3d-pipeline-store-{}-{name}", std::process::id()))
            .join(FILE_NAME);
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
        p
    }

    #[test]
    fn records_round_trip_and_dedupe() {
        let path = scratch("round-trip");
        let prog = vec![1u32, 2, 3];
        let id = program_id(&prog);
        {
            let (mut s, loaded) = PipelineStore::open::<Vec<u32>>(&path);
            assert!(loaded.programs.is_empty() && loaded.pipelines.is_empty());
            s.record(Some((id, &prog)), false, &key(7, false));
            s.record(Some((id, &prog)), true, &key(7, false));
            s.record(Some((id, &prog)), false, &key(7, false));
            s.record::<Vec<u32>>(None, false, &key(0, true));
            assert_eq!(s.len(), (1, 3));
        }
        // Junk and a torn last line are skipped.
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        f.write_all(b"not json\n{\"Pipeline\":{\"prog").unwrap();
        drop(f);
        let (mut s, loaded) = PipelineStore::open::<Vec<u32>>(&path);
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
        // Known records are not written again.
        let before = std::fs::metadata(&path).unwrap().len();
        s.record(Some((id, &prog)), false, &key(7, false));
        assert_eq!(std::fs::metadata(&path).unwrap().len(), before);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn pipelines_of_unknown_programs_are_dropped() {
        let path = scratch("orphan");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let line = serde_json::to_string(&Record::<Vec<u32>>::Pipeline {
            program: Some(42),
            count: false,
            key: key(3, false),
        })
        .unwrap();
        std::fs::write(&path, line + "\n").unwrap();
        let (_, loaded) = PipelineStore::open::<Vec<u32>>(&path);
        assert!(loaded.pipelines.is_empty());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn program_ids_are_content_hashes() {
        assert_eq!(program_id(&vec![1u32]), program_id(&vec![1u32]));
        assert_ne!(program_id(&vec![1u32]), program_id(&vec![2u32]));
    }

    #[test]
    fn path_choice() {
        let os = |s: &str| Some(std::ffi::OsString::from(s));
        assert_eq!(path_from(os("off"), None, os("/h")), None);
        assert_eq!(
            path_from(os("/x/p.jsonl"), None, os("/h")),
            Some(PathBuf::from("/x/p.jsonl"))
        );
        let want = if cfg!(target_vendor = "apple") {
            PathBuf::from("/h/Library/Caches/h2recomp").join(FILE_NAME)
        } else {
            PathBuf::from("/c/h2recomp").join(FILE_NAME)
        };
        assert_eq!(path_from(None, os("/c"), os("/h")), Some(want));
    }
}
