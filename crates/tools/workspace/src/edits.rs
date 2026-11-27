//! Edit files over the tag tree (docs/editor/tag-workspace.md, "Edits are
//! data").
//!
//! An edit file is the YAML list the `edits` builder reads
//! ([`crate::parse_edits`]), applied here to loose tags instead of a map's
//! tag set. Two homes:
//!
//! | Home | Name in a project | `was` |
//! |---|---|---|
//! | `assets/tag-edits/<name>.yaml`, in Git | the path, `assets/tag-edits/<name>.yaml` | refused: our numbers and names only |
//! | `<data>/workspace/edits/<name>.yaml`, the user's | `<name>` | the [`loose_hash`] of the tag the edit was made on |
//!
//! [`Edited`] is a [`TagSource`] over the layers that gives every tag an
//! edit names with the edits applied, in file order and entry order. A
//! line's `from` and an entry's `was` are checked against the tag as the
//! layers give it, before any edit file: two files that set different
//! fields of one tag both apply, and the later of two that set one field
//! wins and is listed in [`Edited::overrides`]. A tag the layers do not
//! hold, a path that does not resolve, a stale `was` and a `from` the field
//! does not hold are [`Problem`]s with the file and line; [`Edited::check`]
//! lists every one, and a read of an edited tag with one fails. No edit is
//! dropped.
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::path::{Path, PathBuf};

use content::Sha256;
use serde::Serialize;
use tags::defs::{decode, default_record, encode};
use tags::repo::{Body, LooseTag, RawRes, TagKey, TagSource};

use crate::edit::{
    SetLine, TagEdit, change, check_block_line, def_of, edits_text, field_of, from_mismatch,
    parse_edits, parse_group_ref,
};

/// Our edit files, from the checkout's root.
pub const GIT_DIR: &str = "assets/tag-edits";
/// The user's edit files, under `<data>/workspace/`.
pub const LOCAL_DIR: &str = "edits";
/// An edit file's extension.
pub const EXT: &str = "yaml";

/// Where an edit file lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Origin {
    /// `assets/tag-edits/`, in Git.
    Git,
    /// `<data>/workspace/edits/`.
    Local,
}

/// An edit that cannot apply: the file, the entry's line (0 for the file
/// as a whole), the tag (`<group>:<path>`, empty when the file does not
/// parse) and why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Problem {
    pub file: String,
    pub line: usize,
    pub tag: String,
    pub why: String,
}

impl Problem {
    pub fn new(file: &str, line: usize, tag: &str, why: impl Into<String>) -> Self {
        Self {
            file: file.to_owned(),
            line,
            tag: tag.to_owned(),
            why: why.into(),
        }
    }
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.file)?;
        if self.line != 0 {
            write!(f, ":{}", self.line)?;
        }
        if !self.tag.is_empty() {
            write!(f, ": {}", self.tag)?;
        }
        write!(f, ": {}", self.why)
    }
}

/// Problems as one line of text: the count, then the first eight.
pub fn problems_text(problems: &[Problem]) -> String {
    let shown: Vec<String> = problems.iter().take(8).map(ToString::to_string).collect();
    let more = problems.len().saturating_sub(shown.len());
    let tail = if more == 0 {
        String::new()
    } else {
        format!("; and {more} more")
    };
    format!(
        "{} edit problem{}: {}{tail}",
        problems.len(),
        if problems.len() == 1 { "" } else { "s" },
        shown.join("; ")
    )
}

/// A tag as an edit names it: `<group>:<path>`.
pub fn key_text(key: &TagKey) -> String {
    format!("{}:{}", tags::defs::fourcc_text(key.group), key.path)
}

/// The tag `<group>:<path>` names.
pub fn parse_key(text: &str) -> Result<TagKey, String> {
    let (group, path) = parse_group_ref(text)?;
    Ok(TagKey::new(group, path))
}

/// The tag an entry names.
pub fn key_of(e: &TagEdit) -> Result<TagKey, String> {
    parse_key(&e.tag)
}

/// The content hash an entry's `was` names: SHA-256 over the tag's
/// canonical bytes (`tags::defs::canonical`), so the file format, node
/// sharing and the tree's layout of files do not move it, and a
/// re-extraction that changes the tag does.
pub fn loose_hash(tag: &LooseTag) -> Sha256 {
    Sha256::of(&tags::defs::canonical(tag))
}

/// Whether `name` can name a local edit file (or a project): letters,
/// digits, `_` and `-`, at most 64.
pub fn is_plain_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// One edit file, read and checked against the tag definitions.
#[derive(Clone, Debug)]
pub struct EditFile {
    /// `assets/tag-edits/<name>.yaml` for a Git file, `<name>` for a local one.
    pub name: String,
    pub origin: Origin,
    pub path: PathBuf,
    pub text: String,
    /// Each entry with the line its `-` is on.
    pub entries: Vec<(TagEdit, usize)>,
}

impl EditFile {
    /// `text` as an edit file. Every entry is checked without a tag: the
    /// group has a definition, each path is one of its fields, each block
    /// line is whole, and a `was` is a digest in a local file only.
    pub fn parse(
        name: &str,
        origin: Origin,
        path: PathBuf,
        text: String,
    ) -> Result<Self, Vec<Problem>> {
        let entries = parse_edits(&text)
            .map_err(|f| vec![Problem::new(name, f.line.unwrap_or(0), "", f.why)])?;
        let problems: Vec<Problem> = entries
            .iter()
            .filter_map(|(e, line)| {
                check_entry(origin, e)
                    .err()
                    .map(|why| Problem::new(name, *line, &e.tag, why))
            })
            .collect();
        if !problems.is_empty() {
            return Err(problems);
        }
        Ok(Self {
            name: name.to_owned(),
            origin,
            path,
            text,
            entries,
        })
    }

    /// The file at `path`.
    pub fn read(name: &str, origin: Origin, path: &Path) -> Result<Self, Vec<Problem>> {
        let text = std::fs::read_to_string(path).map_err(|e| {
            vec![Problem::new(
                name,
                0,
                "",
                format!("reading {}: {e}", path.display()),
            )]
        })?;
        Self::parse(name, origin, path.to_path_buf(), text)
    }

    /// The tags the file edits, each once, in the order the file names them.
    pub fn keys(&self) -> Vec<TagKey> {
        let mut out: Vec<TagKey> = Vec::new();
        for (e, _) in &self.entries {
            if let Ok(k) = key_of(e)
                && !out.contains(&k)
            {
                out.push(k);
            }
        }
        out
    }
}

fn check_entry(origin: Origin, e: &TagEdit) -> Result<(), String> {
    let (group, _) = parse_group_ref(&e.tag)?;
    let def = def_of(group)?;
    match (&e.was, origin) {
        (Some(_), Origin::Git) => {
            return Err(
                "`was` is for local edit files: a file in Git holds our numbers and names only"
                    .into(),
            );
        }
        (Some(was), Origin::Local) => {
            was.parse::<Sha256>()
                .map_err(|_| format!("was {was:?} is not a SHA-256 hex digest"))?;
        }
        (None, _) => {}
    }
    for b in &e.block {
        check_block_line(def, b).map_err(|why| format!("block {:?}: {why}", b.path))?;
    }
    for s in &e.set {
        field_of(def, &s.path).map_err(|why| format!("set {:?}: {why}", s.path))?;
    }
    Ok(())
}

/// The `*.yaml` files of `dir`, sorted by name; none when `dir` is not there.
pub fn files_in(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == EXT))
        .collect();
    out.sort();
    out
}

/// A local edit file's path: `<workspace>/edits/<name>.yaml`.
pub fn local_path(workspace: &Path, name: &str) -> Result<PathBuf, String> {
    if !is_plain_name(name) {
        return Err(format!(
            "{name:?} is not a local edit file's name: letters, digits, '_' and '-', at most 64"
        ));
    }
    Ok(workspace.join(LOCAL_DIR).join(format!("{name}.{EXT}")))
}

/// A Git edit file's path: `name` is `assets/tag-edits/<name>.yaml`.
pub fn git_path(checkout: &Path, name: &str) -> Result<PathBuf, String> {
    let stem = name
        .strip_prefix(GIT_DIR)
        .and_then(|r| r.strip_prefix('/'))
        .and_then(|r| r.strip_suffix(&format!(".{EXT}")))
        .filter(|s| is_plain_name(s))
        .ok_or_else(|| format!("{name:?} is not {GIT_DIR}/<name>.{EXT}"))?;
    Ok(checkout.join(GIT_DIR).join(format!("{stem}.{EXT}")))
}

/// One edit file as [`list`] gives it.
#[derive(Clone, Debug, Serialize)]
pub struct Listed {
    pub name: String,
    pub origin: Origin,
    pub path: String,
    pub entries: usize,
    /// The tags it edits, `<group>:<path>`.
    pub tags: Vec<String>,
    /// What does not parse or check, without a tag.
    pub problems: Vec<Problem>,
}

/// Every edit file: the checkout's (when there is one), then the user's.
/// A file that does not parse is listed with its problems.
pub fn list(checkout: Option<&Path>, workspace: &Path) -> Vec<Listed> {
    let mut out = Vec::new();
    let mut add = |name: String, origin: Origin, path: &Path| {
        let (entries, tags, problems) = match EditFile::read(&name, origin, path) {
            Ok(f) => (
                f.entries.len(),
                f.keys().iter().map(key_text).collect(),
                Vec::new(),
            ),
            Err(p) => (0, Vec::new(), p),
        };
        out.push(Listed {
            name,
            origin,
            path: path.display().to_string(),
            entries,
            tags,
            problems,
        });
    };
    if let Some(checkout) = checkout {
        for p in files_in(&checkout.join(GIT_DIR)) {
            let file = p.file_name().unwrap_or_default().to_string_lossy();
            add(format!("{GIT_DIR}/{file}"), Origin::Git, &p);
        }
    }
    for p in files_in(&workspace.join(LOCAL_DIR)) {
        let stem = p
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        add(stem, Origin::Local, &p);
    }
    out
}

/// What [`set_line`] wrote.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Written {
    pub file: String,
    pub path: String,
    /// The entry's line in the file as written.
    pub line: usize,
    /// Whether a line for the same field was replaced.
    pub replaced: bool,
    pub was: Option<String>,
}

/// Write one `set` line into local edit file `name` (made when missing):
/// it replaces the line for the same field of the file's last entry for
/// `tag`, else joins that entry, else starts one. The path is checked
/// against the definition and, when it names no element, the value
/// against the field's kind; an element path is checked when the edit
/// applies. `was` (the [`loose_hash`] of the tag as the layers give it) is
/// written on the entry; an entry already made on another hash is refused,
/// as its other lines were made on other content. A local file is written
/// whole, so its comments do not survive; comments are for files in Git.
pub fn set_line(
    workspace: &Path,
    name: &str,
    tag: &str,
    path: &str,
    value: &serde_json::Value,
    was: Option<Sha256>,
) -> Result<Written, String> {
    let (group, tag_path) = parse_group_ref(tag)?;
    let def = def_of(group)?;
    field_of(def, path).map_err(|why| format!("set {path:?}: {why}"))?;
    let line = SetLine::from_json(path, value)?;
    if !path.contains('[') {
        let mut rec = default_record(def);
        change(def, &mut rec, &[], &[(path, &line.value)])?;
    }
    let file = local_path(workspace, name)?;
    let mut entries: Vec<TagEdit> = if file.exists() {
        EditFile::read(name, Origin::Local, &file)
            .map_err(|p| problems_text(&p))?
            .entries
            .into_iter()
            .map(|(e, _)| e)
            .collect()
    } else {
        Vec::new()
    };
    let key = TagKey::new(group, tag_path);
    let text = key_text(&key);
    let at = entries
        .iter()
        .rposition(|e| key_of(e).is_ok_and(|k| k == key));
    let at = match at {
        Some(i) => i,
        None => {
            entries.push(TagEdit {
                tag: text.clone(),
                was: None,
                block: Vec::new(),
                set: Vec::new(),
            });
            entries.len() - 1
        }
    };
    let entry = &mut entries[at];
    if let Some(want) = was {
        match entry.was.as_deref().map(str::parse::<Sha256>) {
            Some(Ok(old)) if old != want => {
                return Err(format!(
                    "{name}: the entry for {text} was made on the tag with hash {old}, and the layers give {want}: \
                     run edits.check and settle the file first"
                ));
            }
            _ => entry.was = Some(want.to_string()),
        }
    }
    let replaced = match entry.set.iter_mut().find(|s| s.path == path) {
        Some(s) => {
            s.value = line.value;
            s.from = None;
            true
        }
        None => {
            entry.set.push(line);
            false
        }
    };
    let was = entry.was.clone();
    let body = edits_text(&entries)?;
    let parsed = EditFile::parse(name, Origin::Local, file.clone(), body.clone())
        .map_err(|p| problems_text(&p))?;
    let line_no = parsed.entries.get(at).map_or(1, |(_, l)| *l);
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(&file, body).map_err(|e| format!("{}: {e}", file.display()))?;
    Ok(Written {
        file: name.to_owned(),
        path: file.display().to_string(),
        line: line_no,
        replaced,
        was,
    })
}

/// A field one edit file set and a later one set again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Override {
    pub tag: String,
    pub path: String,
    pub earlier: String,
    pub later: String,
}

/// `entries` (file name, entry, line), all for `base`'s key, applied to
/// `base` in order. `was` and `from` are checked against `base`.
pub fn apply(
    base: &LooseTag,
    entries: &[(&str, &TagEdit, usize)],
) -> Result<(LooseTag, Vec<Override>), Problem> {
    let Some(&(first_file, _, first_line)) = entries.first() else {
        return Ok((base.clone(), Vec::new()));
    };
    let tag = key_text(&base.key);
    let at_first = |why: String| Problem::new(first_file, first_line, &tag, why);
    let def = def_of(base.key.group).map_err(at_first)?;
    let (region, losses) = match &base.body {
        Body::Tree(t) => (t.region, t.losses.clone()),
        Body::Alias { owner, .. } => {
            return Err(at_first(format!(
                "the tag is an alias of {owner}: edit that tag"
            )));
        }
    };
    let base_rec = decode(def, base).map_err(|e| at_first(format!("reading the tag: {e}")))?;
    let mut rec = base_rec.clone();
    let mut hash: Option<Sha256> = None;
    let mut set_by: HashMap<&str, &str> = HashMap::new();
    let mut overrides = Vec::new();
    let mut last = (first_file, first_line);
    for &(file, e, line) in entries {
        last = (file, line);
        let at = |why: String| Problem::new(file, line, &tag, why);
        if let Some(was) = &e.was {
            let want: Sha256 = was
                .parse()
                .map_err(|_| at(format!("was {was:?} is not a SHA-256 hex digest")))?;
            let have = *hash.get_or_insert_with(|| loose_hash(base));
            if want != have {
                return Err(at(format!(
                    "stale edit: it was made on the tag with hash {want}, but the layers give {have}"
                )));
            }
        }
        for s in &e.set {
            if let Some(why) = from_mismatch(def, &base_rec, s).map_err(at)? {
                return Err(at(format!("`from` {}: {why}", s.path)));
            }
        }
        let sets: Vec<(&str, &serde_yaml_ng::Value)> =
            e.set.iter().map(|s| (s.path.as_str(), &s.value)).collect();
        change(def, &mut rec, &e.block, &sets).map_err(at)?;
        for s in &e.set {
            if let Some(earlier) = set_by.insert(s.path.as_str(), file)
                && earlier != file
            {
                overrides.push(Override {
                    tag: tag.clone(),
                    path: s.path.clone(),
                    earlier: earlier.to_owned(),
                    later: file.to_owned(),
                });
            }
        }
    }
    let mut out = encode(def, base.key.clone(), &rec, region)
        .map_err(|e| Problem::new(last.0, last.1, &tag, format!("writing the tag: {e}")))?;
    // What extraction could not keep is still not kept: an edit does not
    // make a lossy tag exact.
    if let Body::Tree(t) = &mut out.body {
        t.losses = losses;
    }
    Ok((out, overrides))
}

/// The layers below with edit files over them (module docs).
pub struct Edited<S> {
    below: S,
    files: Vec<EditFile>,
    /// Each edited tag's entries: (file, entry), in order.
    by_key: BTreeMap<TagKey, Vec<(usize, usize)>>,
    done: HashMap<TagKey, LooseTag>,
    overrides: Vec<Override>,
}

impl<S: TagSource> Edited<S> {
    /// `files` in order over `below`.
    pub fn new(below: S, files: Vec<EditFile>) -> Result<Self, Vec<Problem>> {
        let mut by_key: BTreeMap<TagKey, Vec<(usize, usize)>> = BTreeMap::new();
        let mut problems = Vec::new();
        for (fi, f) in files.iter().enumerate() {
            for (ei, (e, line)) in f.entries.iter().enumerate() {
                match key_of(e) {
                    Ok(k) => by_key.entry(k).or_default().push((fi, ei)),
                    Err(why) => problems.push(Problem::new(&f.name, *line, &e.tag, why)),
                }
            }
        }
        if !problems.is_empty() {
            return Err(problems);
        }
        Ok(Self {
            below,
            files,
            by_key,
            done: HashMap::new(),
            overrides: Vec::new(),
        })
    }

    pub fn files(&self) -> &[EditFile] {
        &self.files
    }

    /// The tags an edit names.
    pub fn edited(&self) -> Vec<TagKey> {
        self.by_key.keys().cloned().collect()
    }

    pub fn below(&self) -> &S {
        &self.below
    }

    pub fn below_mut(&mut self) -> &mut S {
        &mut self.below
    }

    /// Fields set by two files, the later's value kept; filled as tags are
    /// read ([`Edited::check`] reads them all).
    pub fn overrides(&self) -> &[Override] {
        &self.overrides
    }

    /// Every edited tag applied once: each problem, none when every edit
    /// applies.
    pub fn check(&mut self) -> Vec<Problem> {
        let keys = self.edited();
        keys.iter()
            .filter_map(|k| self.edited_tag(k).err())
            .collect()
    }

    /// `key` with its edits, `None` when no edit names it.
    fn edited_tag(&mut self, key: &TagKey) -> Result<Option<LooseTag>, Problem> {
        if let Some(t) = self.done.get(key) {
            return Ok(Some(t.clone()));
        }
        let Some(at) = self.by_key.get(key) else {
            return Ok(None);
        };
        let entries: Vec<(&str, &TagEdit, usize)> = at
            .iter()
            .map(|&(f, e)| {
                let file = &self.files[f];
                let (entry, line) = &file.entries[e];
                (file.name.as_str(), entry, *line)
            })
            .collect();
        let (first_file, _, first_line) = entries[0];
        let tag = key_text(key);
        let base = self
            .below
            .read(key)
            .map_err(|e| {
                Problem::new(
                    first_file,
                    first_line,
                    &tag,
                    format!("reading the tag from the layers: {e}"),
                )
            })?
            .ok_or_else(|| {
                Problem::new(
                    first_file,
                    first_line,
                    &tag,
                    "no layer holds this tag: the edit is for content the project does not have",
                )
            })?;
        let (edited, overrides) = apply(&base, &entries)?;
        self.overrides.extend(overrides);
        self.done.insert(key.clone(), edited.clone());
        Ok(Some(edited))
    }
}

impl<S: TagSource> TagSource for Edited<S> {
    fn keys(&self) -> Vec<TagKey> {
        self.below.keys()
    }

    fn read(&mut self, key: &TagKey) -> tags::repo::Result<Option<LooseTag>> {
        match self.edited_tag(key) {
            Ok(Some(t)) => Ok(Some(t)),
            Ok(None) => self.below.read(key),
            Err(p) => Err(tags::repo::Error::Invalid(p.to_string())),
        }
    }

    fn raw(&mut self, raw: &RawRes) -> tags::repo::Result<Vec<u8>> {
        self.below.raw(raw)
    }

    fn groups(&self) -> Vec<map::TagGroup> {
        self.below.groups()
    }
}
