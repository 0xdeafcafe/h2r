//! The authored layer (docs/editor/tag-workspace.md, "Layers"): our own
//! tags, committed as sources only and generated on each machine.
//!
//! The sources are the `tag` items (`<path>.<group>.yaml`, the `tag`
//! builder's format) of the packages that take a map by name. Each is made
//! a loose tag over the layers below (`like` and `borrow` read them), and
//! the set is kept under `<data>/workspace/generated/<fingerprint>/` as a
//! tag repository. The fingerprint is SHA-256 over [`AUTHORED_VERSION`]
//! and every made tag's canonical bytes, so a changed source, or a changed
//! tag a source reads, makes another layer. Nothing of it is in Git.
//!
//! Two items adding one path, an item adding a path a lower layer holds,
//! and an item that does not build stop the build, each named by package
//! and item.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use content::package::{Item, Kind};
use tags::repo::{LooseTag, RawRes, TagKey, TagRepo, TagSource};

use crate::build::Loaded;
use crate::builders::Job;
use crate::maps::taken_by;

/// Under `<data>/workspace/`.
pub const GENERATED_DIR: &str = "generated";
/// Raised when the same sources would make other tags.
pub const AUTHORED_VERSION: u32 = 1;
/// Written last into a layer's directory: the layer is whole.
pub const COMPLETE_FILE: &str = "complete.txt";

/// The `tag` items of `packages` a map named `name` takes, with their
/// package and index.
pub fn tag_items<'a>(packages: &'a [Loaded], name: &str) -> Vec<(&'a Loaded, usize, &'a Item)> {
    let mut out = Vec::new();
    for p in packages {
        for (i, item) in p.package.manifest.items.iter().enumerate() {
            if item.kind == Kind::Tag && taken_by(item, name) {
                out.push((p, i, item));
            }
        }
    }
    out
}

/// The made layer, before it is kept.
#[derive(Clone, Debug)]
pub struct Made {
    pub fingerprint: String,
    pub tags: Vec<LooseTag>,
}

fn what(p: &Loaded, i: usize, item: &Item) -> String {
    format!(
        "package {} item {i} ({})",
        p.package.manifest.package.id,
        item.source.as_deref().unwrap_or("no source")
    )
}

/// Make the authored tags of map `name` over `below`; `candidates` are the
/// base map's tags (what `borrow` looks at). `None` when no item adds one.
pub fn make(
    packages: &[Loaded],
    name: &str,
    candidates: &[TagKey],
    below: &mut dyn TagSource,
) -> Result<Option<Made>, String> {
    let items = tag_items(packages, name);
    if items.is_empty() {
        return Ok(None);
    }
    let mut by_key: BTreeMap<TagKey, String> = BTreeMap::new();
    let mut tags = Vec::new();
    for (p, i, item) in items {
        let at = what(p, i, item);
        let source = item
            .source
            .as_ref()
            .and_then(|s| p.files.get(s))
            .ok_or_else(|| format!("{at}: the package has no source file"))?;
        let job = Job {
            item,
            source,
            files: &p.files,
        };
        let tag = crate::tagitem::loose(&job, below, candidates).map_err(|f| match f.line {
            Some(line) => format!("{at}:{line}: {}", f.why),
            None => format!("{at}: {}", f.why),
        })?;
        if let Some(other) = by_key.insert(tag.key.clone(), at.clone()) {
            return Err(format!("{at} and {other} both add {}", tag.key));
        }
        if below
            .read(&tag.key)
            .map_err(|e| format!("{at}: {e}"))?
            .is_some()
        {
            return Err(format!(
                "{at} adds {}, which a lower layer holds: an item adds a free path",
                tag.key
            ));
        }
        tags.push(tag);
    }
    let mut h = content::Hasher::default();
    h.update(&AUTHORED_VERSION.to_le_bytes());
    for t in &tags {
        let bytes = tags::defs::canonical(t);
        h.update(&(bytes.len() as u64).to_le_bytes());
        h.update(&bytes);
    }
    Ok(Some(Made {
        fingerprint: h.finish().to_string(),
        tags,
    }))
}

/// Keep `made` under `<workspace>/generated/<fingerprint>/` (written once;
/// a whole layer there is reused) and open it.
pub fn keep(workspace: &Path, made: &Made) -> tags::repo::Result<(TagRepo, PathBuf)> {
    let root = workspace.join(GENERATED_DIR);
    let dir = root.join(&made.fingerprint);
    if !dir.join(COMPLETE_FILE).is_file() {
        let tmp = root.join(format!("{}.tmp{}", made.fingerprint, std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let mut repo = TagRepo::create(&tmp)?;
        for t in &made.tags {
            repo.put(t)?;
        }
        let list: String = made.tags.iter().map(|t| format!("{}\n", t.key)).collect();
        std::fs::write(tmp.join(COMPLETE_FILE), list)?;
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
        std::fs::rename(&tmp, &dir)?;
    }
    Ok((TagRepo::open(&dir)?, dir))
}

/// Made tags held in memory: the layer when it is not kept (what
/// `workspace.prune` asks about). Raw resources are the lower layers'.
pub struct InMemory(pub BTreeMap<TagKey, LooseTag>);

impl InMemory {
    pub fn of(made: &Made) -> Self {
        Self(
            made.tags
                .iter()
                .map(|t| (t.key.clone(), t.clone()))
                .collect(),
        )
    }
}

impl TagSource for InMemory {
    fn keys(&self) -> Vec<TagKey> {
        self.0.keys().cloned().collect()
    }

    fn read(&mut self, key: &TagKey) -> tags::repo::Result<Option<LooseTag>> {
        Ok(self.0.get(key).cloned())
    }

    fn raw(&mut self, _: &RawRes) -> tags::repo::Result<Vec<u8>> {
        Err(tags::repo::Error::Invalid(
            "the authored layer holds no raw resources: a lower layer gives them".into(),
        ))
    }

    fn groups(&self) -> Vec<map::TagGroup> {
        Vec::new()
    }
}
