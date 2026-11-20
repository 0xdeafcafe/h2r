//! The loose tag repository: every tag as its own position-independent
//! file, keyed by group and path, and the way back into a map.
//!
//! A [`LooseTag`] is a tag with no addresses in it. Its arrays are
//! [`Node`]s (bytes plus the layout-dependent fields, by offset), block
//! fields name child nodes, tag references and datums name `(group, path)`,
//! string IDs are text, and raw resources are byte ranges known by length
//! and SHA-256 (with where they came from). The layout comes from
//! [`crate::schema`]; nothing else.
//!
//! Tags come from a [`TagSource`]: a retail map ([`MapSource`]) or a
//! repository on disk ([`TagRepo`]). [`closure`] follows references from
//! roots, and [`build::to_tag_set`] turns the tags into a
//! [`map::build::TagSet`] the builder lays out from scratch. The file format
//! and the repository layout are in `docs/formats/tag-repo.md`.

use map::FourCC;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

pub mod build;
pub mod extract;
pub mod format;
pub mod store;

pub use build::{BuildOptions, to_tag_set};
pub use extract::{MapSource, SharedFiles, compare, store_map};
pub use store::TagRepo;

/// What can go wrong reading, writing or building tags.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Map(#[from] map::Error),
    #[error(transparent)]
    Tags(#[from] crate::Error),
    #[error("{0}")]
    Invalid(String),
    #[error("tag file: {0}")]
    Format(String),
}

/// Result for the repository.
pub type Result<T> = std::result::Result<T, Error>;

/// A tag's identity outside any map: its group and full path (backslashes,
/// no extension).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TagKey {
    /// Concrete group.
    pub group: FourCC,
    /// Full path, as the map names it.
    pub path: String,
}

impl TagKey {
    /// A key.
    pub fn new(group: FourCC, path: impl Into<String>) -> Self {
        TagKey {
            group,
            path: path.into(),
        }
    }
}

impl std::fmt::Display for TagKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.path, self.group)
    }
}

/// A tag with no addresses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LooseTag {
    /// Its identity.
    pub key: TagKey,
    /// Its contents.
    pub body: Body,
}

/// What a [`LooseTag`] holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Body {
    /// A tree of arrays.
    Tree(Tree),
    /// No bytes of its own: the tag is `offset` bytes into `owner` (retail
    /// caches have whole tags the compiler shared).
    Alias {
        /// The tag whose bytes this one is.
        owner: TagKey,
        /// Offset into the owner's root.
        offset: u32,
    },
}

/// Where a tree is laid out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Region {
    /// Ordinary tag metadata.
    #[default]
    Metadata,
    /// A structure BSP region (`sbsp` and `ltmp` tags).
    Bsp,
}

/// A tag's arrays. `nodes[0]` is the root (one element).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tree {
    /// Ordinary metadata or a BSP region.
    pub region: Region,
    /// Every array, root first. A node reached twice (an array the cache
    /// compiler shared inside the tag) is one node.
    pub nodes: Vec<Node>,
    /// Raw resources the tree names.
    pub raws: Vec<RawRes>,
    /// What extraction could not keep (fields left empty), one line each.
    /// Empty for an exact tag.
    pub losses: Vec<String>,
}

/// One array: `count` elements of `size` bytes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Node {
    /// Element bytes. Every field in `fields` is zero here, except a block's
    /// count (and a tag reference's group word, which is kept).
    pub bytes: Vec<u8>,
    /// Elements.
    pub count: u32,
    /// Element size (1 for a data reference).
    pub size: u32,
    /// Address alignment.
    pub align: u32,
    /// Layout-dependent fields by byte offset in `bytes`, sorted.
    pub fields: Vec<(u32, Value)>,
}

/// A layout-dependent field of a [`Node`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    /// 8 bytes: the count (kept in the bytes), then the child's address.
    Block(u32),
    /// 8 bytes: the child's byte length, then its address.
    Data(u32),
    /// 4 bytes: the datum of this tag (a tag reference's second word, or a
    /// bare datum).
    Ref(TagKey),
    /// 4 bytes: a string ID.
    String(String),
    /// 4 bytes: a raw-resource pointer, index into [`Tree::raws`].
    Raw(u32),
    /// 12 bytes: the scenario's BSP descriptor (file offset, size, address
    /// of the region of the `sbsp` its element names). The builder fills it.
    Bsp,
}

/// A raw resource a tag points at.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RawRes {
    /// Which file the bytes came from, and where in it for the shared
    /// caches (a build keeps those pointers; local ones are copied in).
    pub origin: Origin,
    /// Byte length.
    pub len: u32,
    /// SHA-256 of the bytes.
    pub hash: [u8; 32],
}

/// Where a raw resource lives in the retail files.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Origin {
    /// The map's own file (offset not kept: it means nothing elsewhere).
    Local,
    /// One of the shared caches, at this offset.
    External {
        /// `mainmenu.map`, `shared.map` or `single_player_shared.map`.
        file: map::ResourceFile,
        /// File offset.
        offset: u32,
    },
}

impl RawRes {
    /// The hash as lowercase hex.
    pub fn hex(&self) -> String {
        hex(&self.hash)
    }
}

/// Lowercase hex of `bytes`.
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// SHA-256 of `bytes`.
pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    use sha2::Digest;
    sha2::Sha256::digest(bytes).into()
}

impl LooseTag {
    /// Every tag this one names (references, datums, an alias's owner),
    /// in field order, without repeats.
    pub fn references(&self) -> Vec<TagKey> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        match &self.body {
            Body::Alias { owner, .. } => out.push(owner.clone()),
            Body::Tree(t) => {
                for n in &t.nodes {
                    for (_, v) in &n.fields {
                        if let Value::Ref(k) = v
                            && seen.insert(k)
                        {
                            out.push(k.clone());
                        }
                    }
                }
            }
        }
        out
    }

    /// The tree, if the tag has one.
    pub fn tree(&self) -> Option<&Tree> {
        match &self.body {
            Body::Tree(t) => Some(t),
            Body::Alias { .. } => None,
        }
    }
}

/// Where tags come from: a retail map or a repository. Both give the same
/// [`LooseTag`]s, so whatever assembles a map ([`closure`],
/// [`to_tag_set`]) works on either.
pub trait TagSource {
    /// Every tag the source holds.
    fn keys(&self) -> Vec<TagKey>;
    /// One tag, `None` if the source does not hold it.
    fn read(&mut self, key: &TagKey) -> Result<Option<LooseTag>>;
    /// The bytes of a raw resource named by one of this source's tags.
    fn raw(&mut self, raw: &RawRes) -> Result<Vec<u8>>;
    /// The group hierarchy (every group the source's tags use, with its
    /// ancestors), in table order.
    fn groups(&self) -> Vec<map::TagGroup>;
}

/// Tags reached from some roots.
#[derive(Clone, Debug, Default)]
pub struct Closure {
    /// Every tag reached, roots first, then breadth first.
    pub tags: Vec<LooseTag>,
    /// Keys named but not in the source, with the first tag naming each.
    pub missing: BTreeMap<TagKey, TagKey>,
}

impl Closure {
    /// The tag with this key.
    pub fn get(&self, key: &TagKey) -> Option<&LooseTag> {
        self.tags.iter().find(|t| &t.key == key)
    }
}

/// Follow references from `roots` through `src`.
pub fn closure(src: &mut dyn TagSource, roots: &[TagKey]) -> Result<Closure> {
    let mut out = Closure::default();
    let mut seen: HashMap<TagKey, ()> = HashMap::new();
    let mut todo: VecDeque<(TagKey, Option<TagKey>)> =
        roots.iter().map(|k| (k.clone(), None)).collect();
    while let Some((key, from)) = todo.pop_front() {
        if seen.insert(key.clone(), ()).is_some() {
            continue;
        }
        match src.read(&key)? {
            Some(tag) => {
                for r in tag.references() {
                    if !seen.contains_key(&r) {
                        todo.push_back((r, Some(key.clone())));
                    }
                }
                out.tags.push(tag);
            }
            None => {
                out.missing
                    .insert(key.clone(), from.unwrap_or_else(|| key.clone()));
            }
        }
    }
    Ok(out)
}

/// A tag's address-free canonical form, one line per element in visit order
/// (each element, then its children in field order): the element's bytes
/// with every field blanked, then the field values (references by key,
/// strings by text, raw by length and hash). Equal lines mean equal tags,
/// wherever they sit. Shared nodes appear each time they are reached.
pub fn canonical(tag: &LooseTag) -> Vec<String> {
    canonical_with(tag, true).0
}

/// [`canonical`], with raw resources noted fully (`full`: length and hash)
/// or by place only (a shared cache's file and offset, or "local"), and the
/// local resources in visit order. Comparing a rebuilt map with its source
/// uses the second form: lengths found without a stored size run to the
/// next resource, which padding moves, so local bytes are compared apart.
pub fn canonical_with(tag: &LooseTag, full: bool) -> (Vec<String>, Vec<RawRes>) {
    struct Cx<'a> {
        t: &'a Tree,
        full: bool,
        out: Vec<String>,
        locals: Vec<RawRes>,
    }
    fn go(cx: &mut Cx, n: u32, path: &mut Vec<u32>, depth: u32) {
        let t = cx.t;
        let node = &t.nodes[n as usize];
        if depth > 64 {
            cx.out.push("too deep".into());
            return;
        }
        // A data reference (1-byte elements) is one line.
        let (size, elems) = if node.size <= 1 {
            (node.bytes.len(), u32::from(!node.bytes.is_empty()))
        } else {
            (node.size as usize, node.count)
        };
        for i in 0..elems {
            let lo = i as usize * size;
            let hi = lo + size;
            let bytes = &node.bytes[lo..hi];
            let mut notes = Vec::new();
            let mut kids = Vec::new();
            for (o, v) in &node.fields {
                let o = *o as usize;
                if o < lo || o >= hi {
                    continue;
                }
                match v {
                    Value::Block(c) | Value::Data(c) => kids.push((o - lo, *c)),
                    Value::Ref(k) => notes.push(format!("{:#x}={k}", o - lo)),
                    Value::String(s) => notes.push(format!("{:#x}='{s}'", o - lo)),
                    Value::Raw(r) => {
                        let r = &t.raws[*r as usize];
                        let note = match (cx.full, r.origin) {
                            (true, _) => format!("raw {:#x} {}", r.len, r.hex()),
                            (false, Origin::External { file, offset }) => {
                                format!("raw {file:?} {offset:#x}")
                            }
                            (false, Origin::Local) => "raw local".into(),
                        };
                        if r.origin == Origin::Local {
                            cx.locals.push(r.clone());
                        }
                        notes.push(format!("{:#x}={note}", o - lo));
                    }
                    Value::Bsp => notes.push(format!("{:#x}=bsp", o - lo)),
                }
            }
            cx.out.push(format!("{path:x?} {} {notes:?}", hex(bytes)));
            for (o, c) in kids {
                path.push(o as u32);
                go(cx, c, path, depth + 1);
                path.pop();
            }
        }
    }
    match &tag.body {
        Body::Alias { owner, offset } => (vec![format!("alias {owner} +{offset:#x}")], Vec::new()),
        Body::Tree(t) => {
            let mut cx = Cx {
                t,
                full,
                out: Vec::new(),
                locals: Vec::new(),
            };
            go(&mut cx, 0, &mut Vec::new(), 0);
            for l in &t.losses {
                cx.out.push(format!("loss {l}"));
            }
            (cx.out, cx.locals)
        }
    }
}

#[cfg(test)]
mod tests;
