//! The loose tag file (`.<group>` in a repository), version 1.
//!
//! Little-endian throughout. A 16-byte header, then sections, each a `u32`
//! count followed by its entries:
//!
//! | Part | Contents |
//! |---|---|
//! | header | `"H2TG"`, `u16` version, `u16` kind (0 tree, 1 alias), `u32` group, `u32` flags (bit 0: BSP region) |
//! | strings | `u16` length + UTF-8 each; string 0 is the tag's own path |
//! | alias only | `u32` owner group, `u32` owner path (string index), `u32` offset; end of file |
//! | refs | `u32` group, `u32` path (string index) |
//! | raws | `u8` file (0 local, 1 mainmenu, 2 shared, 3 single-player shared), 3 zero bytes, `u32` offset (0 for local), `u32` length, 32-byte SHA-256 |
//! | losses | `u32` string index each |
//! | nodes | `u32` data offset, `u32` count, `u32` element size, `u32` align, `u32` first fixup, `u32` fixups |
//! | fixups | `u32` offset in the node, `u8` kind, 3 zero bytes, `u32` value |
//! | data | the nodes' bytes, each at its alignment from the start of data |
//!
//! Fixup kinds: 0 block and 1 data reference (value: child node), 2 datum
//! (ref index), 3 string ID (string index), 4 raw (raw index), 5 BSP
//! descriptor (12 bytes, value 0). In the data, each field holds what the
//! fixup names, so the bytes alone are position independent too: a block's
//! address word is its child's offset from the start of data, a datum word
//! the ref index, a string ID the string index, a raw pointer the raw index.
use super::{Body, Error, LooseTag, Node, Origin, RawRes, Region, Result, TagKey, Tree, Value};
use map::FourCC;
use std::collections::HashMap;

/// File magic.
pub const MAGIC: &[u8; 4] = b"H2TG";
/// Format version this code writes and reads.
pub const VERSION: u16 = 1;

const KIND_TREE: u16 = 0;
const KIND_ALIAS: u16 = 1;

struct Strings {
    list: Vec<String>,
    index: HashMap<String, u32>,
}

impl Strings {
    fn id(&mut self, s: &str) -> u32 {
        if let Some(&i) = self.index.get(s) {
            return i;
        }
        let i = self.list.len() as u32;
        self.list.push(s.to_owned());
        self.index.insert(s.to_owned(), i);
        i
    }
}

fn file_code(f: map::ResourceFile) -> u8 {
    match f {
        map::ResourceFile::Local => 0,
        map::ResourceFile::MainMenu => 1,
        map::ResourceFile::Shared => 2,
        map::ResourceFile::SinglePlayerShared => 3,
    }
}

fn file_of(c: u8) -> Option<map::ResourceFile> {
    Some(match c {
        0 => map::ResourceFile::Local,
        1 => map::ResourceFile::MainMenu,
        2 => map::ResourceFile::Shared,
        3 => map::ResourceFile::SinglePlayerShared,
        _ => return None,
    })
}

fn put(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// Serialise a tag.
pub fn encode(tag: &LooseTag) -> Vec<u8> {
    let mut strings = Strings {
        list: Vec::new(),
        index: HashMap::new(),
    };
    strings.id(&tag.key.path);
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    match &tag.body {
        Body::Alias { owner, offset } => {
            let p = strings.id(&owner.path);
            out.extend_from_slice(&KIND_ALIAS.to_le_bytes());
            put(&mut out, tag.key.group.as_u32());
            put(&mut out, 0);
            write_strings(&mut out, &strings.list);
            put(&mut out, owner.group.as_u32());
            put(&mut out, p);
            put(&mut out, *offset);
            out
        }
        Body::Tree(t) => {
            // Intern every string first (refs, string IDs, losses), so the
            // string section can come first.
            let mut refs: Vec<(u32, u32)> = Vec::new();
            let mut ref_index: HashMap<&TagKey, u32> = HashMap::new();
            for n in &t.nodes {
                for (_, v) in &n.fields {
                    match v {
                        Value::Ref(k) => {
                            if !ref_index.contains_key(k) {
                                ref_index.insert(k, refs.len() as u32);
                                let p = strings.id(&k.path);
                                refs.push((k.group.as_u32(), p));
                            }
                        }
                        Value::String(s) => {
                            strings.id(s);
                        }
                        _ => {}
                    }
                }
            }
            let losses: Vec<u32> = t.losses.iter().map(|l| strings.id(l)).collect();
            out.extend_from_slice(&KIND_TREE.to_le_bytes());
            put(&mut out, tag.key.group.as_u32());
            put(&mut out, u32::from(t.region == Region::Bsp));
            write_strings(&mut out, &strings.list);
            put(&mut out, refs.len() as u32);
            for (g, p) in &refs {
                put(&mut out, *g);
                put(&mut out, *p);
            }
            put(&mut out, t.raws.len() as u32);
            for r in &t.raws {
                let (f, off) = match r.origin {
                    Origin::Local => (0, 0),
                    Origin::External { file, offset } => (file_code(file), offset),
                };
                out.extend_from_slice(&[f, 0, 0, 0]);
                put(&mut out, off);
                put(&mut out, r.len);
                out.extend_from_slice(&r.hash);
            }
            put(&mut out, losses.len() as u32);
            for l in losses {
                put(&mut out, l);
            }
            // Lay the nodes out in data.
            let mut offsets = Vec::with_capacity(t.nodes.len());
            let mut len = 0u32;
            for n in &t.nodes {
                let a = n.align.max(1);
                len = len.next_multiple_of(a);
                offsets.push(len);
                len += n.bytes.len() as u32;
            }
            let mut data = vec![0u8; len as usize];
            put(&mut out, t.nodes.len() as u32);
            let mut first = 0u32;
            for (n, &at) in t.nodes.iter().zip(&offsets) {
                put(&mut out, at);
                put(&mut out, n.count);
                put(&mut out, n.size);
                put(&mut out, n.align);
                put(&mut out, first);
                put(&mut out, n.fields.len() as u32);
                first += n.fields.len() as u32;
            }
            put(&mut out, first);
            for (n, &at) in t.nodes.iter().zip(&offsets) {
                let d = &mut data[at as usize..at as usize + n.bytes.len()];
                d.copy_from_slice(&n.bytes);
                for (o, v) in &n.fields {
                    let o = *o as usize;
                    let (kind, value) = match v {
                        Value::Block(c) => (0u8, *c),
                        Value::Data(c) => (1, *c),
                        Value::Ref(k) => (2, ref_index[k]),
                        Value::String(s) => (3, strings.index[s.as_str()]),
                        Value::Raw(r) => (4, *r),
                        Value::Bsp => (5, 0),
                    };
                    match v {
                        Value::Block(c) | Value::Data(c) => {
                            d[o + 4..o + 8].copy_from_slice(&offsets[*c as usize].to_le_bytes())
                        }
                        Value::Bsp => {}
                        _ => d[o..o + 4].copy_from_slice(&value.to_le_bytes()),
                    }
                    put(&mut out, o as u32);
                    out.extend_from_slice(&[kind, 0, 0, 0]);
                    put(&mut out, value);
                }
            }
            put(&mut out, len);
            out.extend_from_slice(&data);
            out
        }
    }
}

fn write_strings(out: &mut Vec<u8>, list: &[String]) {
    put(out, list.len() as u32);
    for s in list {
        out.extend_from_slice(&(s.len() as u16).to_le_bytes());
        out.extend_from_slice(s.as_bytes());
    }
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let s = self
            .b
            .get(self.at..self.at.checked_add(n).ok_or_else(|| bad("length"))?)
            .ok_or_else(|| bad("truncated"))?;
        self.at += n;
        Ok(s)
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().expect("4")))
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().expect("2")))
    }
    /// A count no larger than the bytes left could hold at `min` each.
    fn count(&mut self, min: usize) -> Result<usize> {
        let n = self.u32()? as usize;
        if n.saturating_mul(min) > self.b.len() - self.at {
            return Err(bad("count larger than the file"));
        }
        Ok(n)
    }
}

fn bad(what: &str) -> Error {
    Error::Format(what.to_owned())
}

/// Read a tag file.
pub fn decode(bytes: &[u8]) -> Result<LooseTag> {
    let mut r = Reader { b: bytes, at: 0 };
    if r.take(4)? != MAGIC {
        return Err(bad("not a tag file"));
    }
    let version = r.u16()?;
    if version != VERSION {
        return Err(Error::Format(format!(
            "version {version}, this build reads {VERSION}"
        )));
    }
    let kind = r.u16()?;
    let group = FourCC::from_u32(r.u32()?);
    let flags = r.u32()?;
    let n = r.count(2)?;
    let mut strings = Vec::with_capacity(n);
    for _ in 0..n {
        let len = r.u16()? as usize;
        let s = std::str::from_utf8(r.take(len)?).map_err(|_| bad("string not UTF-8"))?;
        strings.push(s.to_owned());
    }
    let string = |i: u32| -> Result<String> {
        strings
            .get(i as usize)
            .cloned()
            .ok_or_else(|| bad("string index"))
    };
    let key = TagKey::new(group, string(0)?);
    if kind == KIND_ALIAS {
        let og = FourCC::from_u32(r.u32()?);
        let op = string(r.u32()?)?;
        let offset = r.u32()?;
        return Ok(LooseTag {
            key,
            body: Body::Alias {
                owner: TagKey::new(og, op),
                offset,
            },
        });
    }
    if kind != KIND_TREE {
        return Err(bad("unknown kind"));
    }
    let n = r.count(8)?;
    let mut refs = Vec::with_capacity(n);
    for _ in 0..n {
        let g = FourCC::from_u32(r.u32()?);
        refs.push(TagKey::new(g, string(r.u32()?)?));
    }
    let n = r.count(44)?;
    let mut raws = Vec::with_capacity(n);
    for _ in 0..n {
        let f = r.take(4)?[0];
        let offset = r.u32()?;
        let len = r.u32()?;
        let hash: [u8; 32] = r.take(32)?.try_into().expect("32");
        let file = file_of(f).ok_or_else(|| bad("raw file"))?;
        let origin = match file {
            map::ResourceFile::Local => Origin::Local,
            file => Origin::External { file, offset },
        };
        raws.push(RawRes { origin, len, hash });
    }
    let n = r.count(4)?;
    let mut losses = Vec::with_capacity(n);
    for _ in 0..n {
        losses.push(string(r.u32()?)?);
    }
    let n = r.count(24)?;
    let mut heads = Vec::with_capacity(n);
    for _ in 0..n {
        heads.push([r.u32()?, r.u32()?, r.u32()?, r.u32()?, r.u32()?, r.u32()?]);
    }
    let n = r.count(12)?;
    let mut fixups = Vec::with_capacity(n);
    for _ in 0..n {
        let o = r.u32()?;
        let k = r.take(4)?[0];
        fixups.push((o, k, r.u32()?));
    }
    let len = r.u32()? as usize;
    let data = r.take(len)?;
    if r.at != bytes.len() {
        return Err(bad("trailing bytes"));
    }
    let node_at: HashMap<u32, u32> = heads
        .iter()
        .enumerate()
        .map(|(i, h)| (h[0], i as u32))
        .collect();
    let mut nodes = Vec::with_capacity(heads.len());
    for h in &heads {
        let [at, count, size, align, first, nf] = *h;
        let blen = if size <= 1 {
            count as usize
        } else {
            count as usize * size as usize
        };
        let mut b = data
            .get(at as usize..at as usize + blen)
            .ok_or_else(|| bad("node out of data"))?
            .to_vec();
        let fx = fixups
            .get(first as usize..(first + nf) as usize)
            .ok_or_else(|| bad("fixup range"))?;
        let mut fields = Vec::with_capacity(fx.len());
        for &(o, k, v) in fx {
            let ou = o as usize;
            let width = if k == 5 {
                12
            } else if k <= 1 {
                8
            } else {
                4
            };
            if ou + width > b.len() {
                return Err(bad("fixup past its node"));
            }
            let value = match k {
                0 | 1 => {
                    let rel = u32::from_le_bytes(b[ou + 4..ou + 8].try_into().expect("4"));
                    if v as usize >= heads.len() || node_at.get(&rel) != Some(&v) {
                        return Err(bad("block offset and node disagree"));
                    }
                    b[ou + 4..ou + 8].fill(0);
                    if k == 0 {
                        Value::Block(v)
                    } else {
                        Value::Data(v)
                    }
                }
                2 => Value::Ref(
                    refs.get(v as usize)
                        .cloned()
                        .ok_or_else(|| bad("ref index"))?,
                ),
                3 => Value::String(string(v)?),
                4 => {
                    if v as usize >= raws.len() {
                        return Err(bad("raw index"));
                    }
                    Value::Raw(v)
                }
                5 => Value::Bsp,
                _ => return Err(bad("fixup kind")),
            };
            if (2..=4).contains(&k) {
                b[ou..ou + 4].fill(0);
            }
            fields.push((o, value));
        }
        nodes.push(Node {
            bytes: b,
            count,
            size,
            align,
            fields,
        });
    }
    if nodes.is_empty() {
        return Err(bad("no root"));
    }
    Ok(LooseTag {
        key,
        body: Body::Tree(Tree {
            region: if flags & 1 != 0 {
                Region::Bsp
            } else {
                Region::Metadata
            },
            nodes,
            raws,
            losses,
        }),
    })
}
